use eframe::egui::Color32;
use i_overlay9::{
    core::{
        fill_rule::FillRule as OverlayFillRule,
        overlay::{ContourDirection, ShapeType},
        overlay_rule::OverlayRule,
    },
    float::overlay::{FloatOverlay, OverlayOptions},
    i_float::adapter::FloatPointAdapter,
};
use kurbo::{Affine, ParamCurve, Point, QuadBez, Vec2 as KVec2};

use crate::vector_engine::{left_normal, normalize_vec, point_to_segment_dist, Contour, FillShape};

// ============================================================================
// ブラシ：マウスの点 → なめらかな中心線 → 太らせた輪郭 → 2次ベジェ（GEOMETRY.md 2章）
// ============================================================================

/// 曲線に当てはめるときの許容（画面の px）。画面の画素の 1/4 で、当てはめで生じる差は描いた倍率では見分けられない
/// （メッシュを使い回すときに許すずれ `MESH_MAX_ERROR` も同じ決まり）
pub(crate) const FIT_TOL_PX: f64 = 0.25;
/// 中心線と輪郭を折れ線にするときの許容の、当てはめの許容に対する割合（折れ線のずれで当てはめの許容をほとんど使わない）
const FLAT_RATIO: f64 = 0.1;
/// 輪郭の自分との交わりを整理するとき落とす輪郭の面積（画面の px²）
const DUST_AREA_PX: f64 = 0.05;
/// 描いている間に中心線を確定する遅れ（点の数）。8・16・32 点で試し、確定した中心線を太らせた形のアンカーの数と
/// ずれが、全体を一度に解いたものと同じになるいちばん小さい数（8 点では長い線でアンカーが1割ほど増えた）
const LAG: usize = 16;
/// 中心線を確定するまとまり（点の数）
const CHUNK: usize = 16;
/// 窓を解くとき、確定した所から戻って値を決めておく点の数。3次スプラインの影響は節1つごとに 2−√3（約 0.27）倍に
/// 減るので、6 点戻れば、確定した所と窓の所の向きと曲がり方の食い違いは 0.27⁶（約 4×10⁻⁴）倍になる
const BACK: usize = 6;
/// 離したあとの輪郭を整理するとき、中心線の点をいくつずつの区切りに分けるか（`stroke_shapes`）。区切りが大きいほど、
/// きつく曲がる所の内側の帯どうしの交わり（区切りの中の点の数の2乗ほど）が増え、小さいほど合わせる回数が増える。
/// 乱数の手描きの線 2,000 本（`random_hand_strokes_follow_their_centerlines` と同じ線）を Linux で整理した合計の時間：
/// 分けない 47.4秒、32 で 20.0秒、16 で 14.7秒、8 で 13.0秒、4 で 13.2秒（つなぎ目を滑らかな所に選ぶ前の測り）。
/// 縮めても速くならなくなる 8 にする
const PIECE: usize = 8;

/// 中心線（3次の自然スプライン）。節の媒介変数（弦の長さ）`s`、値 `a`、2階微分 `c`
#[derive(Clone, Debug, Default)]
pub(crate) struct Centerline {
    s: Vec<f64>,
    a: Vec<Point>,
    c: Vec<KVec2>,
}

/// 中心線の上の点：位置と単位接線
#[derive(Clone, Copy)]
struct Sample {
    p: Point,
    t: KVec2,
}

impl Centerline {
    /// 節の数
    pub(crate) fn len(&self) -> usize {
        self.a.len()
    }

    /// 節 `from..=to` の部分を、半径 `r` で太らせた輪郭の弦のずれが `eps` 以内になる細かさで点にする。区間（3次ベジェ
    /// b0..b3 に直したもの）ごとの点の数は、次の2つの大きい方：
    /// - 中心線そのものの弦のずれ：Wang の式 √(3/4 · max|Δ²b| / eps)
    /// - 太らせた輪郭の弦のずれ：区間で接線が回る角度を刻み φ（半径 r の円弧で弦のずれが eps になる角度）で割った数。
    ///   接線は導関数のベジェ（3(b1−b0)、3(b2−b1)、3(b3−b2)）の凸包の向きにあるので、回る角度はその3つの向きの
    ///   広がり以下（凸包が原点を含む（速さが 0 になりうる）ときは π）
    fn samples(&self, from: usize, to: usize, r: f64, eps: f64) -> Vec<Sample> {
        if from == to {
            return vec![Sample {
                p: self.a[from],
                t: KVec2::new(1.0, 0.0),
            }];
        }
        let phi = arc_step(r, eps);
        let mut out = Vec::new();
        for j in from..to {
            let h = self.s[j + 1] - self.s[j];
            let (a0, a1) = (self.a[j].to_vec2(), self.a[j + 1].to_vec2());
            let (c0, c1) = (self.c[j], self.c[j + 1]);
            let f = |u: f64| -> KVec2 {
                let v = 1.0 - u;
                a0 * v + a1 * u + (c0 * (v * v * v - v) + c1 * (u * u * u - u)) * (h * h / 6.0)
            };
            // u（0..1）での導関数
            let du = |u: f64| -> KVec2 {
                let v = 1.0 - u;
                (a1 - a0) + (c1 * (3.0 * u * u - 1.0) - c0 * (3.0 * v * v - 1.0)) * (h * h / 6.0)
            };
            let (b1, b2) = (a0 + du(0.0) / 3.0, a1 - du(1.0) / 3.0);
            let dd = (a0 - b1 * 2.0 + b2)
                .hypot()
                .max((b1 - b2 * 2.0 + a1).hypot());
            let k_curve = (0.75 * dd / eps).sqrt().ceil();
            let hodo = [b1 - a0, b2 - b1, a1 - b2];
            let spread = hodo_spread(&hodo);
            let k = k_curve.max((spread / phi).ceil()).max(1.0) as usize;
            for i in 0..k {
                let u = i as f64 / k as f64;
                out.push(Sample {
                    p: f(u).to_point(),
                    t: normalize_vec(du(u)),
                });
            }
        }
        let j = to - 1;
        let h = self.s[to] - self.s[j];
        let d_end = (self.a[to] - self.a[j]) + (self.c[to] * 2.0 + self.c[j]) * (h * h / 6.0);
        out.push(Sample {
            p: self.a[to],
            t: normalize_vec(d_end),
        });
        out
    }
}

#[cfg(test)]
impl Centerline {
    /// テスト用：節の値
    pub(crate) fn knots(&self) -> &[Point] {
        &self.a
    }

    /// テスト用：中心線全体を、弦のずれが `eps` 以内になる細かさで点にしたもの
    pub(crate) fn sample_points(&self, eps: f64) -> Vec<Point> {
        if self.a.is_empty() {
            return Vec::new();
        }
        self.samples(0, self.a.len() - 1, 1.0, eps)
            .into_iter()
            .map(|c| c.p)
            .collect()
    }
}

/// 半径 `r` の円弧を、弦のずれが `eps` 以内の折れ線にするときの1歩の角度 φ（`eps` が半径以上なら直角）
fn arc_step(r: f64, eps: f64) -> f64 {
    if eps < r {
        2.0 * (1.0 - eps / r).acos()
    } else {
        std::f64::consts::FRAC_PI_2
    }
}

/// 向き `v`（0 でないもの）の広がりの角度（いちばん広い2つの間の角度）。向きの凸包が原点を含むなら π
fn hodo_spread(v: &[KVec2]) -> f64 {
    let dirs: Vec<KVec2> = v.iter().filter(|d| d.hypot() > 0.0).copied().collect();
    if dirs.len() < 2 {
        return 0.0;
    }
    let mut best = 0.0f64;
    for i in 0..dirs.len() {
        for k in i + 1..dirs.len() {
            best = best.max(dirs[i].cross(dirs[k]).atan2(dirs[i].dot(dirs[k])).abs());
        }
    }
    // 3つの向きが、どの2つの間の角より広く回っている（凸包が原点を含む）とき
    let total: f64 = (0..dirs.len())
        .map(|i| {
            let (a, b) = (dirs[i], dirs[(i + 1) % dirs.len()]);
            a.cross(b).atan2(a.dot(b))
        })
        .sum();
    if total.abs() > std::f64::consts::PI {
        std::f64::consts::PI
    } else {
        best
    }
}

/// 描いている間に中心線を確定していく（遅れを決めた平滑化）。
///
/// 中心線は、マウスの点の丸めの範囲（各点の縦横 ±刻み/2 の正方形）に値が入る3次の自然スプラインのうち、いちばん
/// なめらかなもの（∫|f''|² が最小）。点を受け取るたびに、確定した所の `BACK` 点前から今の点までの窓を解き（確定した
/// 点は値をそのまま使う）、今の点から `LAG` 点より前を `CHUNK` 点ずつ確定する。窓の区切りは点の数だけで決まるので、
/// 同じ点の列からはいつも同じ中心線になる。離したあとの形も同じ点の列から作り直すので、描いている間に見えていた
/// 中心線と同じになる（`centerline`）
#[derive(Clone, Debug)]
pub(crate) struct CenterlineBuilder {
    /// 箱の半分の幅（文書の座標）
    half: f64,
    /// 受け取った点（前の点と同じ点は除く）と、その弦の長さの媒介変数
    pts: Vec<Point>,
    s: Vec<f64>,
    /// 確定した節の値と2階微分（点 0..確定した数）
    fixed_a: Vec<Point>,
    fixed_c: Vec<KVec2>,
}

impl CenterlineBuilder {
    /// 丸めの刻み `step`（文書の座標。画面の1画素を文書の座標に直したもの：`input_step`）
    pub(crate) fn new(step: f64) -> Self {
        Self {
            half: 0.5 * step,
            pts: Vec::new(),
            s: Vec::new(),
            fixed_a: Vec::new(),
            fixed_c: Vec::new(),
        }
    }

    /// 受け取った点
    pub(crate) fn points(&self) -> &[Point] {
        &self.pts
    }

    /// 確定した節の数
    pub(crate) fn fixed(&self) -> usize {
        self.fixed_a.len()
    }

    /// 点を足す（有限でない点と、前の点と同じ点は足さない）。確定した所が増えたら、増える前の確定した節の数を返す
    pub(crate) fn push(&mut self, p: Point) -> Option<usize> {
        if !p.x.is_finite() || !p.y.is_finite() || self.pts.last() == Some(&p) {
            return None;
        }
        let s = match (self.pts.last(), self.s.last()) {
            (Some(&l), Some(&sl)) => sl + (p - l).hypot(),
            _ => 0.0,
        };
        self.pts.push(p);
        self.s.push(s);
        let (end, f) = (self.pts.len() - 1, self.fixed());
        if end < LAG || end + 1 < f + LAG + CHUNK {
            return None;
        }
        let w0 = f.saturating_sub(BACK);
        let (a, c) = self.solve(w0, end);
        for j in f..=end - LAG {
            self.fixed_a.push(a[j - w0]);
            self.fixed_c.push(c[j - w0]);
        }
        Some(f)
    }

    /// 確定した所と、まだ確定していない所（確定した所から今の点までの窓を解いたもの）を合わせた中心線
    pub(crate) fn current(&self) -> Centerline {
        let n = self.pts.len();
        let f = self.fixed();
        let mut a = self.fixed_a.clone();
        let mut c = self.fixed_c.clone();
        if f < n {
            let w0 = f.saturating_sub(BACK);
            let (wa, wc) = self.solve(w0, n - 1);
            a.extend_from_slice(&wa[f - w0..]);
            c.extend_from_slice(&wc[f - w0..]);
        }
        Centerline {
            s: self.s.clone(),
            a,
            c,
        }
    }

    /// 点 `w0..=end` の窓を解く。確定した点は、値をその点の確定した値に決める（箱の幅をごく小さくする）
    fn solve(&self, w0: usize, end: usize) -> (Vec<Point>, Vec<KVec2>) {
        let m = end - w0 + 1;
        let f = self.fixed();
        let value = |j: usize| if j < f { self.fixed_a[j] } else { self.pts[j] };
        if m < 3 {
            return ((w0..=end).map(value).collect(), vec![KVec2::ZERO; m]);
        }
        let s = &self.s[w0..=end];
        let hw: Vec<f64> = (w0..=end)
            .map(|j| if j < f { 1e-6 * self.half } else { self.half })
            .collect();
        let xs: Vec<f64> = (w0..=end).map(|j| value(j).x).collect();
        let ys: Vec<f64> = (w0..=end).map(|j| value(j).y).collect();
        let (ax, cx) = box_spline(s, &xs, &hw);
        let (ay, cy) = box_spline(s, &ys, &hw);
        (
            (0..m).map(|k| Point::new(ax[k], ay[k])).collect(),
            (0..m).map(|k| KVec2::new(cx[k], cy[k])).collect(),
        )
    }
}

/// マウスの点 `points`（文書の座標）の中心線。丸めの刻みは `step`（`CenterlineBuilder`）
pub(crate) fn centerline(points: &[Point], step: f64) -> Centerline {
    let mut b = CenterlineBuilder::new(step);
    for &p in points {
        b.push(p);
    }
    b.current()
}

/// 1つの座標の問題：節 `s`、データ `v`、箱の半分の幅 `hw` について、3次の自然スプラインのエネルギー a·Ka
/// （∫|f''|²。K = QR⁻¹Qᵀ は節の値からエネルギーへの行列）に、データへのごく小さい引き ε|a − v|² を足したものを、
/// 各節の値が [v − hw, v + hw] に入る範囲で最小にする。返すのは節の値と2階微分（両端は 0）。
///
/// ごく小さい引きは、エネルギーが同じ答えがいくつもあるとき（箱の中の直線など）に、データにいちばん近いものを選んで
/// 答えを1つにするためのもの。主双対内点法（Mehrotra の予測子・修正子）で解く。ニュートン法の1歩は、障壁の対角 D を
/// 重みにした平滑化スプラインの5重対角の帯行列（R/2 + QᵀD⁻¹Q）を1回分解して解く（Woodbury の式で
/// (2K + D)⁻¹ を帯行列に直す。Reinsch の平滑化スプラインと同じ形）
fn box_spline(s: &[f64], v: &[f64], hw: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = v.len();
    let m = n - 2;
    let h: Vec<f64> = (0..n - 1).map(|j| (s[j + 1] - s[j]).max(1e-12)).collect();
    // Q の列 i（中の節 i+1）の3つの値（行 i, i+1, i+2）
    let qc: Vec<[f64; 3]> = (0..m)
        .map(|i| {
            let (a, b) = (1.0 / h[i], 1.0 / h[i + 1]);
            [a, -(a + b), b]
        })
        .collect();
    let qt = |x: &[f64], out: &mut [f64]| {
        for i in 0..m {
            out[i] = x[i] * qc[i][0] + x[i + 1] * qc[i][1] + x[i + 2] * qc[i][2];
        }
    };
    let qmul = |y: &[f64], out: &mut [f64]| {
        out.iter_mut().for_each(|o| *o = 0.0);
        for i in 0..m {
            out[i] += y[i] * qc[i][0];
            out[i + 1] += y[i] * qc[i][1];
            out[i + 2] += y[i] * qc[i][2];
        }
    };
    // R（3重対角。対角 (h_i + h_{i+1})/3、隣 h/6）を解く
    let mut cp = vec![0.0; m];
    let mut dp = vec![0.0; m];
    let mut r_solve = |rhs: &[f64], out: &mut [f64]| {
        for i in 0..m {
            let lo = if i > 0 { h[i] / 6.0 } else { 0.0 };
            let up = if i + 1 < m { h[i + 1] / 6.0 } else { 0.0 };
            let den = (h[i] + h[i + 1]) / 3.0 - if i > 0 { lo * cp[i - 1] } else { 0.0 };
            cp[i] = up / den;
            dp[i] = (rhs[i] - if i > 0 { lo * dp[i - 1] } else { 0.0 }) / den;
        }
        for i in (0..m).rev() {
            out[i] = dp[i] - if i + 1 < m { cp[i] * out[i + 1] } else { 0.0 };
        }
    };
    // (R/2 + QᵀWQ) の Cholesky 分解（W は対角）と、その解
    let (mut l0, mut l1, mut l2) = (vec![0.0; m], vec![0.0; m], vec![0.0; m]);
    let factor = |w: &[f64], l0: &mut [f64], l1: &mut [f64], l2: &mut [f64]| {
        for i in 0..m {
            let q = qc[i];
            let dd = w[i] * q[0] * q[0] + w[i + 1] * q[1] * q[1] + w[i + 2] * q[2] * q[2];
            let o1 = if i + 1 < m {
                w[i + 1] * q[1] * qc[i + 1][0] + w[i + 2] * q[2] * qc[i + 1][1]
            } else {
                0.0
            };
            let o2 = if i + 2 < m {
                w[i + 2] * q[2] * qc[i + 2][0]
            } else {
                0.0
            };
            let diag = (h[i] + h[i + 1]) / 6.0 + dd;
            let off1 = if i + 1 < m { h[i + 1] / 12.0 } else { 0.0 } + o1;
            let a1 = if i >= 1 { l1[i - 1] } else { 0.0 };
            let a2 = if i >= 2 { l2[i - 2] } else { 0.0 };
            l0[i] = (diag - a1 * a1 - a2 * a2).max(1e-300).sqrt();
            let b2 = if i >= 1 { l2[i - 1] } else { 0.0 };
            l1[i] = (off1 - b2 * a1) / l0[i];
            l2[i] = o2 / l0[i];
        }
    };
    let mut zb = vec![0.0; m];
    let mut band_solve = |l0: &[f64], l1: &[f64], l2: &[f64], rhs: &[f64], out: &mut [f64]| {
        for i in 0..m {
            let mut x = rhs[i];
            if i >= 1 {
                x -= zb[i - 1] * l1[i - 1];
            }
            if i >= 2 {
                x -= zb[i - 2] * l2[i - 2];
            }
            zb[i] = x / l0[i];
        }
        for i in (0..m).rev() {
            let mut x = zb[i];
            if i + 1 < m {
                x -= out[i + 1] * l1[i];
            }
            if i + 2 < m {
                x -= out[i + 2] * l2[i];
            }
            out[i] = x / l0[i];
        }
    };
    // 大きさの目安：節の間隔の3乗（エネルギーの行列の大きさ）と、いちばん広い箱
    let hm = h.iter().sum::<f64>() / h.len() as f64;
    let half = hw.iter().copied().fold(0.0f64, f64::max);
    let eps = 1e-6 / (hm * hm * hm);
    let z0 = half / (hm * hm * hm);
    // 値は箱の中心からのずれ d = a − v で持つ（すきまを箱の幅の桁で測る。座標の桁で測ると、すきまが座標の丸めの
    // 細かさより小さくなったとき 0 になってしまう）
    let lo: Vec<f64> = hw.iter().map(|w| -w).collect();
    let hi: Vec<f64> = hw.to_vec();
    let mut kv = vec![0.0; n];
    {
        let mut t = vec![0.0; m];
        let mut cv = vec![0.0; m];
        qt(v, &mut t);
        r_solve(&t, &mut cv);
        qmul(&cv, &mut kv);
    }
    let mut a = vec![0.0; n];
    let (mut zl, mut zu) = (vec![z0; n], vec![z0; n]);
    let mut c_in = vec![0.0; m];
    let mut tm = vec![0.0; m];
    let mut ka = vec![0.0; n];
    let mut rd = vec![0.0; n];
    let mut dinv = vec![0.0; n];
    let mut w = vec![0.0; n];
    let mut y = vec![0.0; m];
    let mut qy = vec![0.0; n];
    let mut da = vec![0.0; n];
    let mut rhs = vec![0.0; n];
    let (mut dzl, mut dzu) = (vec![0.0; n], vec![0.0; n]);
    let (mut cl, mut cu) = (vec![0.0; n], vec![0.0; n]);
    for _ in 0..80 {
        // 双対の残差 r = Ha + g − zl + zu（H = 2K + 2εI、g = −2εv）。すきまは sl = a − lo、su = hi − a
        qt(&a, &mut tm);
        r_solve(&tm, &mut c_in);
        qmul(&c_in, &mut ka);
        let (mut rmax, mut gap) = (0.0f64, 0.0f64);
        for j in 0..n {
            rd[j] = 2.0 * (ka[j] + kv[j]) + 2.0 * eps * a[j] - zl[j] + zu[j];
            rmax = rmax.max(rd[j].abs());
            let (sl, su) = (a[j] - lo[j], hi[j] - a[j]);
            gap += sl * zl[j] + su * zu[j];
            dinv[j] = 1.0 / (zl[j] / sl + zu[j] / su + 2.0 * eps);
        }
        gap /= (2 * n) as f64;
        if gap < 1e-10 * z0 * half && rmax < 1e-8 * z0 {
            break;
        }
        factor(&dinv, &mut l0, &mut l1, &mut l2);
        // (2K + D)Δa = rhs：Δa = D⁻¹rhs − D⁻¹Q (R/2 + QᵀD⁻¹Q)⁻¹ QᵀD⁻¹rhs
        let mut solve_dir = |rhs: &[f64], da: &mut [f64]| {
            for j in 0..n {
                w[j] = dinv[j] * rhs[j];
            }
            qt(&w, &mut tm);
            band_solve(&l0, &l1, &l2, &tm, &mut y);
            qmul(&y, &mut qy);
            for j in 0..n {
                da[j] = w[j] - dinv[j] * qy[j];
            }
        };
        // 箱の中・双対が正のまま進める最大の歩幅（Δsl = Δa、Δsu = −Δa）
        let max_step = |a: &[f64], da: &[f64], dzl: &[f64], dzu: &[f64]| -> f64 {
            let mut al = 1.0f64;
            for j in 0..n {
                if da[j] < 0.0 {
                    al = al.min((lo[j] - a[j]) / da[j]);
                } else if da[j] > 0.0 {
                    al = al.min((hi[j] - a[j]) / da[j]);
                }
                if dzl[j] < 0.0 {
                    al = al.min(-zl[j] / dzl[j]);
                }
                if dzu[j] < 0.0 {
                    al = al.min(-zu[j] / dzu[j]);
                }
            }
            al
        };
        // 予測子（すきまと双対の積を 0 へ）
        for j in 0..n {
            rhs[j] = -rd[j] - zl[j] + zu[j];
        }
        solve_dir(&rhs, &mut da);
        for j in 0..n {
            let (sl, su) = (a[j] - lo[j], hi[j] - a[j]);
            dzl[j] = (-zl[j] * sl - zl[j] * da[j]) / sl;
            dzu[j] = (-zu[j] * su + zu[j] * da[j]) / su;
        }
        let ap = max_step(&a, &da, &dzl, &dzu);
        let mut gap_aff = 0.0;
        for j in 0..n {
            let (sl, su) = (a[j] - lo[j], hi[j] - a[j]);
            gap_aff += (sl + ap * da[j]) * (zl[j] + ap * dzl[j])
                + (su - ap * da[j]) * (zu[j] + ap * dzu[j]);
        }
        gap_aff /= (2 * n) as f64;
        let tau = (gap_aff / gap).powi(3).min(1.0) * gap;
        // 修正子（予測子の2次の項 Δz·Δs を足し、積を τ へ）
        for j in 0..n {
            let (sl, su) = (a[j] - lo[j], hi[j] - a[j]);
            cl[j] = tau - zl[j] * sl - dzl[j] * da[j];
            cu[j] = tau - zu[j] * su + dzu[j] * da[j];
            rhs[j] = -rd[j] + cl[j] / sl - cu[j] / su;
        }
        solve_dir(&rhs, &mut da);
        for j in 0..n {
            let (sl, su) = (a[j] - lo[j], hi[j] - a[j]);
            dzl[j] = (cl[j] - zl[j] * da[j]) / sl;
            dzu[j] = (cu[j] + zu[j] * da[j]) / su;
        }
        // 2次計画なので、主と双対は同じ歩幅で進める（双対の残差が (1 − 歩幅) 倍になる）。丸めで箱の端に着いて
        // しまう（すきまや双対が正でなくなる）なら、これ以上は進めないので、その手前の答えで止める
        let al = (0.99 * max_step(&a, &da, &dzl, &dzu)).min(1.0);
        let inside = (0..n).all(|j| {
            let x = a[j] + al * da[j];
            x - lo[j] > 0.0
                && hi[j] - x > 0.0
                && zl[j] + al * dzl[j] > 0.0
                && zu[j] + al * dzu[j] > 0.0
        });
        if !inside {
            break;
        }
        for j in 0..n {
            a[j] += al * da[j];
            zl[j] += al * dzl[j];
            zu[j] += al * dzu[j];
        }
    }
    for j in 0..n {
        a[j] += v[j];
    }
    qt(&a, &mut tm);
    r_solve(&tm, &mut c_in);
    let mut c = vec![0.0; n];
    c[1..m + 1].copy_from_slice(&c_in);
    (a, c)
}

/// なめらかな中心線の点 `cs` を半径 `r` で太らせた形の輪郭（NonZero で塗る1本の閉じた折れ線）と、各点の輪郭の接線。
///
/// 帯の点は、各点の法線の向き（`p ± r·n`）に置く。隣の法線と半径より内で交わる所（曲率半径が `r` より小さい所。
/// 以下「きつい所」）は、区間の法線の帯にする（その所の点は、中心の点の並びを弦のずれ `eps` で間引いてから使う）。
/// 帯の向きが変わるつなぎ目の決まりは1つ：その側から見て外へ回るなら、中心の点のまわりの半径 r の円弧（刻み φ）、
/// きつい所の中で内へ回るなら、中心の点を通る折り返し（どちらも、塗った形が各区間を太らせた形を合わせたものになる）。
/// きつくない点どうし・きつくない点ときつい所の境・きつい所の中の、どのつなぎ目にも同じ決まりを当てる（外へ回る所を
/// 折り返しにすると、速さが 0 に近い折り返しの先の丸い端が欠ける：レビュー d91cd4c の N-2001）。両端は半円のキャップで、
/// 刻みは弦のずれ `eps` から決める。`caps`（始め・終わり）が `false` の端は、キャップの代わりに、その点の法線に沿った
/// 直径で平らに閉じる（区切りに分けて整理するとき：`stroke_shapes`）。1点だけなら円
fn brush_outline(cs: &[Sample], r: f64, eps: f64, caps: [bool; 2]) -> (Vec<Point>, Vec<KVec2>) {
    let phi = arc_step(r, eps);
    let cap_n = ((std::f64::consts::PI / phi).ceil() as usize).max(2);
    let (mut pts, mut tans) = (Vec::new(), Vec::new());
    if cs.len() == 1 {
        let c = cs[0].p;
        let k = 2 * cap_n;
        for i in 0..k {
            let a = std::f64::consts::TAU * i as f64 / k as f64;
            pts.push(Point::new(c.x + r * a.cos(), c.y + r * a.sin()));
            tans.push(KVec2::new(-a.sin(), a.cos()));
        }
        return (pts, tans);
    }
    let n = cs.len();
    let nrm: Vec<KVec2> = cs.iter().map(|c| left_normal(c.t)).collect();
    let seg_dir: Vec<KVec2> = (0..n - 1)
        .map(|i| normalize_vec(cs[i + 1].p - cs[i].p))
        .collect();
    // 側 side（+1 は左、−1 は右）で、区間 i の両端の法線が半径より内で交わるか
    let tight = |side: f64| -> Vec<bool> {
        let mut t = vec![false; n];
        for i in 0..n - 1 {
            let (a, b) = (nrm[i] * side, nrm[i + 1] * side);
            let det = b.cross(a);
            if det.abs() < 1e-12 {
                continue;
            }
            let d = cs[i + 1].p - cs[i].p;
            let (al, be) = (b.cross(d) / det, a.cross(d) / det);
            if al > 0.0 && al < r && be > 0.0 && be < r {
                t[i] = true;
                t[i + 1] = true;
            }
        }
        t
    };
    // 中心 c で、帯の向き a から b へ外へ回るつなぎ目の円弧の点（側 side。両端の点は入れない。刻みは φ 以下）
    let arc = |c: Point, a: KVec2, b: KVec2, side: f64, v: &mut Vec<(Point, KVec2)>| {
        let th = a.cross(b).atan2(a.dot(b));
        let steps = (th.abs() / phi).ceil() as usize;
        let d = left_normal(a) * side;
        for k in 1..steps {
            let (sn, cn) = (th * k as f64 / steps as f64).sin_cos();
            let dir = KVec2::new(d.x * cn - d.y * sn, d.x * sn + d.y * cn);
            v.push((c + dir * r, KVec2::new(-dir.y, dir.x)));
        }
    };
    // 帯の向き a から b へ、側 side から見て外へ回るか（回る角は円弧と同じ atan2 で求める：ちょうど逆向きのときは +π
    // になり、右の側が外になる。外積の符号だけで見ると、逆向きでどちらの側も外にならず、折り返しの先の丸い端が無くなる）
    let outward = |a: KVec2, b: KVec2, side: f64| side * a.cross(b).atan2(a.dot(b)) < 0.0;
    let side_pts = |side: f64| -> Vec<(Point, KVec2)> {
        let tt = tight(side);
        let mut v = Vec::new();
        let mut i = 0;
        while i < n {
            if !tt[i] {
                // 前の点の帯の向き（前がきつい所なら、その最後の区間の向き）から外へ回るなら、この点を中心にした円弧
                if i > 0 {
                    let a = if tt[i - 1] { seg_dir[i - 1] } else { cs[i - 1].t };
                    if outward(a, cs[i].t, side) {
                        arc(cs[i].p, a, cs[i].t, side, &mut v);
                    }
                }
                v.push((cs[i].p + nrm[i] * (side * r), cs[i].t));
                i += 1;
                continue;
            }
            let i0 = i;
            while i + 1 < n && tt[i + 1] {
                i += 1;
            }
            let run: Vec<Point> = cs[i0..=i].iter().map(|c| c.p).collect();
            let mut keep = vec![false; run.len()];
            keep[0] = true;
            keep[run.len() - 1] = true;
            if run.len() > 2 {
                rdp_keep(&run, 0, run.len() - 1, eps, &mut keep);
            }
            let kept: Vec<usize> = (i0..=i).filter(|&k| keep[k - i0]).collect();
            for (kk, &k) in kept.iter().enumerate() {
                let p = cs[k].p;
                let din = if kk > 0 {
                    normalize_vec(p - cs[kept[kk - 1]].p)
                } else if k > 0 {
                    seg_dir[k - 1]
                } else {
                    cs[k].t
                };
                let dout = if kk + 1 < kept.len() {
                    normalize_vec(cs[kept[kk + 1]].p - p)
                } else if k + 1 < n {
                    seg_dir[k]
                } else {
                    cs[k].t
                };
                // きつくない点からきつい所に入るつなぎ目：前の点の向きから外へ回るなら、前の点を中心にした円弧
                if kk == 0 && k > 0 && !tt[k - 1] && outward(cs[k - 1].t, din, side) {
                    arc(cs[k - 1].p, cs[k - 1].t, din, side, &mut v);
                }
                v.push((p + left_normal(din) * (side * r), din));
                if outward(din, dout, side) {
                    arc(p, din, dout, side, &mut v);
                } else {
                    v.push((p, cs[k].t));
                }
                v.push((p + left_normal(dout) * (side * r), dout));
            }
            i += 1;
        }
        v
    };
    let mut push = |(p, t): (Point, KVec2)| {
        pts.push(p);
        tans.push(t);
    };
    // 右側（始点 → 終点）、終点の半円、左側（終点 → 始点）、始点の半円
    side_pts(-1.0).into_iter().for_each(&mut push);
    let (pe, te, ne) = (cs[n - 1].p, cs[n - 1].t, nrm[n - 1]);
    for k in (1..cap_n).filter(|_| caps[1]) {
        let a = k as f64 / cap_n as f64 * std::f64::consts::PI;
        push((
            pe + (-ne * a.cos() + te * a.sin()) * r,
            ne * a.sin() + te * a.cos(),
        ));
    }
    side_pts(1.0).into_iter().rev().for_each(&mut push);
    let (ps, ts, ns) = (cs[0].p, cs[0].t, nrm[0]);
    for k in (1..cap_n).filter(|_| caps[0]) {
        let a = k as f64 / cap_n as f64 * std::f64::consts::PI;
        push((
            ps + (ns * a.cos() - ts * a.sin()) * r,
            -ns * a.sin() - ts * a.cos(),
        ));
    }
    (pts, tans)
}

/// 閉じた折れ線 `ring`（輪郭を自分との交わりで整理したもの）を、許容 `eps` で2次ベジェにする。`tan_of` は、輪郭を
/// 作ったときの点ならその点の接線（向きは問わない）、交わりで新しくできた点（角）なら `None`。
///
/// 角と角の間を、始めの点からいちばん遠くまで1本の曲線で当てはまる所まで伸ばしてつなぐ。曲線の制御点は両端の
/// 接線の交点（角ではない点で分けるときは、その点の本当の接線を両側の曲線で使うので、つなぎ目はなめらか（G1））。
/// 当てはまるかは、折れ線の点から曲線へと、曲線の点から折れ線へとの両方の距離が `eps` 以内か（曲線は弦のずれ
/// eps/10 の折れ線にして測る）
fn fit_ring(ring: &[Point], tan_of: &dyn Fn(Point) -> Option<KVec2>, eps: f64) -> Vec<QuadBez> {
    let n = ring.len();
    let tans: Vec<Option<KVec2>> = ring.iter().map(|&p| tan_of(p)).collect();
    let corners: Vec<usize> = (0..n).filter(|&i| tans[i].is_none()).collect();
    let runs: Vec<(usize, usize)> = if corners.is_empty() {
        vec![(0, n)]
    } else {
        (0..corners.len())
            .map(|k| {
                let a = corners[k];
                let b = corners[(k + 1) % corners.len()];
                (a, if b > a { b } else { b + n })
            })
            .collect()
    };
    let mut out = Vec::new();
    for (a, b) in runs {
        let pts: Vec<Point> = (a..=b).map(|i| ring[i % n]).collect();
        let tv: Vec<Option<KVec2>> = (a..=b).map(|i| tans[i % n]).collect();
        out.extend(fit_run(&pts, &tv, eps));
    }
    out
}

/// 点の並び `pts`（進む向きに並び、両端は動かさない）を、点 `k` の接線 `tv[k]`（角は `None`）に沿って、2次ベジェの並びに
/// 当てはめる（当てはめの許容 `eps`。輪 `fit_ring` の角と角の間と、鉛筆の線 `pencil_curves` が使う）
fn fit_run(pts: &[Point], tv: &[Option<KVec2>], eps: f64) -> Vec<QuadBez> {
    let mut out = Vec::new();
    let last = pts.len() - 1;
    // 点 k の、進む向きにそろえた接線（角なら、区間の始めは出ていく弦、終わりは入ってくる弦の向き）
    let tan_at = |k: usize, end: bool| -> KVec2 {
        match tv[k] {
            Some(t) => {
                let chord = if k == 0 {
                    pts[1] - pts[0]
                } else if k == last {
                    pts[last] - pts[last - 1]
                } else {
                    pts[k + 1] - pts[k - 1]
                };
                if t.dot(chord) >= 0.0 {
                    t
                } else {
                    -t
                }
            }
            None if end => normalize_vec(pts[k] - pts[k - 1]),
            None => normalize_vec(pts[k + 1] - pts[k]),
        }
    };
    let fit = |i: usize, j: usize| -> Option<QuadBez> {
        let (p0, p2) = (pts[i], pts[j]);
        let line = QuadBez::new(p0, p0.lerp(p2, 0.5), p2);
        if j == i + 1 {
            return Some(line);
        }
        // 点の列 `from` のどれもが、折れ線 `to` から eps 以内か（どちらも進む向きに並ぶので、近い区間を順にたどる）
        let within = |from: &[Point], to: &[Point]| -> bool {
            let mut cur = 0usize;
            for &q in from {
                let mut best = point_to_segment_dist(q, to[cur], to[cur + 1]);
                while cur + 2 < to.len() {
                    let nd = point_to_segment_dist(q, to[cur + 1], to[cur + 2]);
                    if nd > best {
                        break;
                    }
                    best = nd;
                    cur += 1;
                }
                if best > eps {
                    return false;
                }
            }
            true
        };
        let fits = |quad: &QuadBez| -> bool {
            let dd = (quad.p0.to_vec2() - quad.p1.to_vec2() * 2.0 + quad.p2.to_vec2()).hypot();
            let mq = ((dd / (0.4 * eps)).sqrt().ceil() as usize).max(1);
            let fq: Vec<Point> = (0..=mq).map(|u| quad.eval(u as f64 / mq as f64)).collect();
            within(&pts[i + 1..j], &fq) && within(&fq[1..mq], &pts[i..=j])
        };
        // 直線で当てはまるなら直線にする（制御点が端に寄った、ほとんど直線の曲線にしない。つなぎ目の向きの違いは、
        // 当てはめの許容の中）
        if fits(&line) {
            return Some(line);
        }
        let (t0, t2) = (tan_at(i, false), tan_at(j, true));
        let d = p2 - p0;
        let det = t0.cross(t2);
        if det.abs() < 1e-12 {
            return None;
        }
        let (al, be) = (d.cross(t2) / det, t0.cross(d) / det);
        if al <= 0.0 || be <= 0.0 {
            return None;
        }
        // 速さ（両端で 2|p1 − p0| と 2|p2 − p1|）の比が 1/2 より偏る曲線は使わず、もっと短く分ける。地図は、切った
        // 所の媒介変数を、折れ線の区間の中で比例で決めて曲線を戻す（`Prov`）ので、速さが大きく偏る曲線では、戻した
        // 片が曲線から外れ、同じ線を重ねるたびに欠片や切れ目の頂点が残る（試しで、比 0.25 では欠片が10通り中1通り、
        // 1/2 では0通り。今のブラシは1通り）
        let quad = QuadBez::new(p0, p0 + t0 * al, p2);
        let (l0, l1) = ((quad.p1 - quad.p0).hypot(), (quad.p2 - quad.p1).hypot());
        if 2.0 * l0.min(l1) < l0.max(l1) {
            return None;
        }
        fits(&quad).then_some(quad)
    };
    // 始めの点から、当てはまるいちばん遠い点を、倍に伸ばしてから2分して探す
    let mut i = 0;
    while i < last {
        let mut good = i + 1;
        let mut good_q = fit(i, good).expect("隣どうしは直線で当てはまる");
        let mut step = 2;
        loop {
            let j = (i + step).min(last);
            if j <= good {
                break;
            }
            if let Some(q) = fit(i, j) {
                (good, good_q) = (j, q);
                step *= 2;
                continue;
            }
            let (mut lo, mut hi) = (good, j);
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                match fit(i, mid) {
                    Some(q) => (lo, good_q) = (mid, q),
                    None => hi = mid,
                }
            }
            good = lo;
            break;
        }
        out.push(good_q);
        i = good;
    }
    out
}

/// 線の端の形（`quad_band`）
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LineEnd {
    /// 半円の端（線が途切れる所：行き止まり・ほかの線との交わり・線の色や太さの変わり目）
    Cap,
    /// 同じ線が続くつなぎ目で、外側のすき間は隣の線が埋める（何も足さない）
    Open,
    /// 同じ線が続くつなぎ目で、こちらが外側のすき間を扇で埋める（丸いつなぎ目）。中身は、隣の線の、その点から出ていく向き
    Join(KVec2),
}

impl LineEnd {
    /// 変換 `affine` をかけた所での同じ端の形（つなぎ目の隣の線の向きは、変換の線形の部分で写して長さ 1 にする）
    pub(crate) fn transformed(self, affine: Affine) -> Self {
        match self {
            LineEnd::Join(v) => {
                let [a, b, c, d, _, _] = affine.as_coeffs();
                LineEnd::Join(normalize_vec(KVec2::new(a * v.x + c * v.y, b * v.x + d * v.y)))
            }
            e => e,
        }
    }
}

/// 2次ベジェ `q` を中心線にして、半径 `r` で太らせた形（線の図形：`planar_map::line_shape`）の、輪郭と、色 `color` の
/// メッシュ。中心線の点（`quad_samples`）の左右に半径ずつずらした帯と、両端（始め・終わり）の形 `ends` でできる（刻みは
/// 弦のずれ `eps` から決める：`arc_step`）：途切れる端は半円、つなぎ目は何も足さないか、2本の帯の外側のすき間を埋める扇
/// （2本の帯の外側の角を、つなぎ目の点を中心に結ぶ円弧。向きが少しでも変われば足す：すき間は拡大すると見えるので、描画の許容 `eps` で省かない）。
/// 輪郭は、左の点を前へ・終わりの端・右の点を後ろへ・始めの端の順に並べた1本の閉じた折れ線と、扇ごとの輪（曲がる内側で
/// 自分と重なることがあるので NonZero で塗る形）。メッシュは、同じ点を帯の三角形と扇でつなぐ（多角形を三角形に分ける計算を
/// しない）。どれも手間は点の数に比例する
pub(crate) fn quad_band(
    q: QuadBez,
    r: f64,
    eps: f64,
    color: Color32,
    ends: [LineEnd; 2],
) -> (Vec<Vec<Point>>, eframe::egui::Mesh) {
    use eframe::egui::{epaint::Vertex, Mesh, Pos2};
    let cs = quad_samples(q, r, eps);
    let phi = arc_step(r, eps);
    let rot = |v: KVec2, a: f64| KVec2::new(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
    // 中心 c で、向き from から角 a だけ回した円弧の点（両端を含む。刻みは phi 以下）
    let arc = |c: Point, from: KVec2, a: f64| -> Vec<Point> {
        let k = ((a.abs() / phi).ceil() as usize).max(1);
        (0..=k).map(|j| c + rot(from, a * j as f64 / k as f64) * r).collect()
    };
    let left: Vec<Point> = cs.iter().map(|c| c.p + left_normal(c.t) * r).collect();
    let right: Vec<Point> = cs.iter().map(|c| c.p - left_normal(c.t) * r).collect();
    let (first, last) = (cs[0], cs[cs.len() - 1]);
    // 端の形：輪郭に入れる点（帯の角どうしの間。半円なら円弧の中の点）と、つなぎ目の扇（中心と円弧の点）
    let end_shape = |c: Point, out: KVec2, end: LineEnd, cap_from: KVec2| -> (Vec<Point>, Option<Vec<Point>>) {
        match end {
            LineEnd::Cap => {
                let pts = arc(c, cap_from, -std::f64::consts::PI);
                (pts[1..pts.len() - 1].to_vec(), None)
            }
            LineEnd::Open => (Vec::new(), None),
            LineEnd::Join(v) => {
                // 隣の線からつなぎ目へ入り（向き −v）、こちらへ出ていく（向き out）ときの曲がる角。外側は曲がる向きと逆の側
                let (t_in, t_out) = (-v, out);
                let a = t_in.cross(t_out).atan2(t_in.dot(t_out));
                // 向きが少しでも変われば埋める（すき間がどれだけ小さくても、拡大すると見えるので、描画の許容では省かない。
                // 小さい角なら扇は三角形1つ）
                if a == 0.0 {
                    return (Vec::new(), None);
                }
                let outer = if a > 0.0 { KVec2::new(t_in.y, -t_in.x) } else { KVec2::new(-t_in.y, t_in.x) };
                let mut fan = vec![c];
                fan.extend(arc(c, outer, a));
                (Vec::new(), Some(fan))
            }
        }
    };
    // 終わりの端は、左の角から前を回って右の角へ（出ていく向きは後ろ向き）。始めの端は、右の角から後ろを回って左の角へ
    let (end_pts, end_fan) = end_shape(last.p, -last.t, ends[1], left_normal(last.t));
    let (start_pts, start_fan) = end_shape(first.p, first.t, ends[0], -left_normal(first.t));
    let mut ring: Vec<Point> = left.clone();
    ring.extend(&end_pts);
    ring.extend(right.iter().rev());
    ring.extend(&start_pts);
    let mut rings = vec![ring];
    rings.extend(end_fan.iter().chain(start_fan.iter()).cloned());
    // メッシュ
    let mut mesh = Mesh::default();
    let mut vert = |p: Point| -> u32 {
        mesh.vertices.push(Vertex {
            pos: Pos2::new(p.x as f32, p.y as f32),
            uv: eframe::egui::epaint::WHITE_UV,
            color,
        });
        mesh.vertices.len() as u32 - 1
    };
    let side: Vec<(u32, u32)> = left.iter().zip(&right).map(|(&l, &r)| (vert(l), vert(r))).collect();
    let mut tri: Vec<u32> = Vec::with_capacity(6 * side.len());
    for w in side.windows(2) {
        let ((l0, r0), (l1, r1)) = (w[0], w[1]);
        tri.extend([l0, r0, l1, r0, r1, l1]);
    }
    // 半円の端：帯の角から円弧の点を通って反対の角へ、端の点を中心にした扇
    for (c, from, to, pts) in [
        (last.p, side[side.len() - 1].0, side[side.len() - 1].1, &end_pts),
        (first.p, side[0].1, side[0].0, &start_pts),
    ] {
        if pts.is_empty() {
            continue;
        }
        let center = vert(c);
        let mut prev = from;
        for &p in pts.iter() {
            let cur = vert(p);
            tri.extend([center, prev, cur]);
            prev = cur;
        }
        tri.extend([center, prev, to]);
    }
    // つなぎ目の扇
    for fan in end_fan.iter().chain(start_fan.iter()) {
        let ids: Vec<u32> = fan.iter().map(|&p| vert(p)).collect();
        for w in ids[1..].windows(2) {
            tri.extend([ids[0], w[0], w[1]]);
        }
    }
    mesh.indices = tri;
    (rings, mesh)
}

/// 2次ベジェ `q` の、媒介変数 `t` での向き（単位。制御点が端の点と重なって速さが 0 の所は弦の向き。線の帯の点と、線の
/// つなぎ目の向きの決まりはこれ1つ：`quad_samples`・`planar_map::line_ends`）
pub(crate) fn quad_tangent(q: &QuadBez, t: f64) -> KVec2 {
    let d = crate::vector_engine::quad_deriv(q, t);
    if d.hypot() > 0.0 {
        normalize_vec(d)
    } else {
        normalize_vec(q.p2 - q.p0)
    }
}

/// 2次ベジェ `q` を太らせるための中心線の点（`quad_band`）：点の数は、中心線そのものの弦のずれ（2次
/// ベジェは n 等分で |p0 − 2·p1 + p2| / (8n²)）と、太らせた輪郭の弦のずれ（曲線が回る角度を、半径 `r` の円弧で弦のずれが
/// `eps` になる角度 `arc_step` で割った数）の、どちらも `eps` 以内になる数
fn quad_samples(q: QuadBez, r: f64, eps: f64) -> Vec<Sample> {
    let (d0, d1) = (q.p1 - q.p0, q.p2 - q.p1);
    let dd = (d1 - d0).hypot();
    let turn = if d0.hypot() > 0.0 && d1.hypot() > 0.0 {
        d0.cross(d1).atan2(d0.dot(d1)).abs()
    } else {
        0.0
    };
    let n = ((dd / (8.0 * eps)).sqrt().ceil())
        .max((turn / arc_step(r, eps)).ceil())
        .max(1.0) as usize;
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            Sample {
                p: q.eval(t),
                t: quad_tangent(&q, t),
            }
        })
        .collect()
}

/// 鉛筆の線：マウスの点 `points`（文書の座標）から、ブラシと同じなめらかな中心線（`centerline`。丸めの刻み `step`）を作り、
/// 2次ベジェの並びに当てはめたもの（文書の座標）。当てはめはブラシと同じく画面の px で行う（表示の倍率 `z`。許容
/// `FIT_TOL_PX`）。点が1つだけなら空
pub(crate) fn pencil_curves(points: &[Point], step: f64, z: f64) -> Vec<QuadBez> {
    if !step.is_finite() || step <= 0.0 || !z.is_finite() || z <= 0.0 {
        return Vec::new();
    }
    let cl = centerline(points, step);
    if cl.len() < 2 {
        return Vec::new();
    }
    // 中心線の点（画面の px）。太らせないので、点の細かさは中心線そのものの弦のずれだけで決まる（半径 0）
    let cs = cl.samples(0, cl.len() - 1, 0.0, FIT_TOL_PX * FLAT_RATIO / z);
    let pts: Vec<Point> = cs.iter().map(|c| (c.p.to_vec2() * z).to_point()).collect();
    let tv: Vec<Option<KVec2>> = cs.iter().map(|c| Some(c.t)).collect();
    fit_run(&pts, &tv, FIT_TOL_PX)
        .into_iter()
        .map(|q| QuadBez::new(
            (q.p0.to_vec2() / z).to_point(),
            (q.p1.to_vec2() / z).to_point(),
            (q.p2.to_vec2() / z).to_point(),
        ))
        .collect()
}

/// 表示の倍率 `zoom` と画面の倍率 `pixels_per_point` での、丸めの刻み（画面の1画素を文書の座標に直したもの）
pub(crate) fn input_step(zoom: f64, pixels_per_point: f64) -> f64 {
    1.0 / (pixels_per_point * zoom)
}

/// 中心線の節 `from..=to` を半径 `radius` で太らせた輪郭（描いている間に見せる形。NonZero で塗る閉じた折れ線、
/// 文書の座標）。折れ線の細かさは表示の倍率 `zoom` で画面に合わせる
pub(crate) fn stroke_preview_outline(
    cl: &Centerline,
    from: usize,
    to: usize,
    radius: f64,
    zoom: f64,
) -> Vec<Point> {
    if cl.len() == 0 || from >= cl.len() || !radius.is_finite() || radius <= 0.0 {
        return Vec::new();
    }
    let eps = FIT_TOL_PX * FLAT_RATIO / zoom;
    let cs = cl.samples(from, to.min(cl.len() - 1), radius, eps);
    brush_outline(&cs, radius, eps, [true, true]).0
}

/// マウスの点 `points`（文書の座標）と半径 `radius` から、離したあとの図形を作る。丸めの刻みは `step`
/// （`input_step`）、当てはめの許容は表示の倍率 `z`（`View::zoom`。いつも範囲の中）で画面に合わせる（`FIT_TOL_PX`）
pub(crate) fn stroke_shapes(
    points: &[Point],
    radius: f64,
    step: f64,
    z: f64,
    color: Color32,
) -> Vec<FillShape> {
    if !radius.is_finite() || radius <= 0.0 || !step.is_finite() || step <= 0.0 {
        return Vec::new();
    }
    let cl = centerline(points, step);
    if cl.len() == 0 {
        return Vec::new();
    }
    // 輪郭の整理と当てはめは画面の px で行い、最後に文書の座標へ戻す
    let eps_flat = FIT_TOL_PX * FLAT_RATIO;
    let cs: Vec<Sample> = cl
        .samples(0, cl.len() - 1, radius, eps_flat / z)
        .into_iter()
        .map(|c| Sample {
            p: (c.p.to_vec2() * z).to_point(),
            t: c.t,
        })
        .collect();
    // 輪郭を自分との交わりで整理する手間（i_overlay）は、交わりの数だけ増える。きつく曲がる所の内側の帯は、曲がりの
    // 中心の近くで互いに交わる（点 K 個で K² 個ほど）。そこで、中心線の点を `PIECE` 個ほどずつの区切りに分け、区切り
    // ごとの輪郭を整理してから合わせる（`union_rings`）。隣の区切りとは1区間（点 j..j+1）重ね、つなぎ目の端は平らに
    // 閉じる：前の区切りは j+1 の直径で、後の区切りは j の直径で閉じ、どちらの直径も隣の区切りの中に入る（端の点は
    // 隣の帯の点と同じ）。つなぎ目 j は、前後の区間（j−1..j+2）で接線の回りが刻み φ（`arc_step`）以下の所にする。
    // 速さが 0 に近い折り返しでは、隣どうしの点の法線がほぼ逆向きで、2本の直径がほぼ重なり、区切りごとに整理した
    // 輪郭の間に細いすき間が残る。キャップで閉じないのは、キャップの円が隣の帯に接してわずかにはみ出し、つなぎ目に
    // 角ができるため
    let last = cs.len() - 1;
    let phi = arc_step(radius * z, eps_flat);
    let turn = |i: usize| {
        cs[i]
            .t
            .cross(cs[i + 1].t)
            .atan2(cs[i].t.dot(cs[i + 1].t))
            .abs()
    };
    let joint = |j: usize| j + 2 <= last && (j - 1..=j + 1).all(|i| turn(i) <= phi);
    let mut parts = Vec::new();
    let mut a = 0;
    loop {
        let b = ((a + PIECE).max(1)..last)
            .find(|&j| joint(j))
            .map_or(last, |j| j + 1);
        parts.push(brush_outline(
            &cs[a..=b],
            radius * z,
            eps_flat,
            [a == 0, b == last],
        ));
        if b == last {
            break;
        }
        a = b - 1;
    }
    let rings: Vec<Vec<[f64; 2]>> = parts
        .iter()
        .map(|(o, _)| o.iter().map(|p| [p.x, p.y]).collect())
        .collect();
    let (outline, tans): (Vec<Point>, Vec<KVec2>) = parts
        .into_iter()
        .flat_map(|(o, t)| o.into_iter().zip(t))
        .unzip();
    // 輪郭の点 → 接線。i_overlay は交わりで新しい点を作るが、もとの点は動かさない（整数の格子に置いて戻すときの
    // ずれは 10⁻⁶ px ほど）ので、ごく近い点を探せばもとの点が分かる。見つからない点は交わりでできた角
    let cell = 1e-3;
    let key = |p: Point| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut look: std::collections::HashMap<(i64, i64), Vec<usize>> = Default::default();
    for (i, &p) in outline.iter().enumerate() {
        look.entry(key(p)).or_default().push(i);
    }
    let tan_of = |p: Point| -> Option<KVec2> {
        let (kx, ky) = key(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &i in look.get(&(kx + dx, ky + dy)).into_iter().flatten() {
                    if (outline[i] - p).hypot() < 1e-4 {
                        return Some(tans[i]);
                    }
                }
            }
        }
        None
    };
    let mut options = OverlayOptions::default();
    options.output_direction = ContourDirection::Clockwise;
    options.min_output_area = DUST_AREA_PX;
    let to_world = Affine::scale(1.0 / z);
    let mut result = Vec::new();
    for shape in union_rings(&rings, options) {
        let mut contours: Vec<Contour> = Vec::new();
        for (ri, ring) in shape.iter().enumerate() {
            if ring.len() < 3 {
                continue;
            }
            let ring: Vec<Point> = ring.iter().map(|p| Point::new(p[0], p[1])).collect();
            let mut ct = Contour::new(fit_ring(&ring, &tan_of, FIT_TOL_PX));
            // 外周は面積が正、穴は負の向きにそろえる
            if (ri == 0) != (ct.signed_area() > 0.0) {
                ct.reverse();
            }
            ct.transform(to_world);
            contours.push(ct);
        }
        if !contours.is_empty() {
            let exterior = contours.remove(0);
            result.push(FillShape::new(color, exterior, contours));
        }
    }
    result
}

/// 閉じた折れ線 `rings`（どれも NonZero で塗る）を合わせた形（i_overlay の図形：外周と穴の並び）。1つずつ自分との
/// 交わりを整理してから、隣どうしを2つずつ合わせていく（分割統治）。どの段も、全部の点の外接矩形から決めた同じ
/// 整数の格子で計算するので、前の段で格子に置いた点は動かない。`options` の塵の面積は最後の段だけで使う（途中で
/// 落とすと、ほかと合わさるはずの小さい所も消える）。1つずつの整理と、同じ段の組どうしの合わせは、互いを見ないので、
/// CPU の数までのスレッドで同時に計算する（`planar_map::parallel_map`。結果は順に計算したのと同じ）
fn union_rings(rings: &[Vec<[f64; 2]>], options: OverlayOptions<f64>) -> Vec<Vec<Vec<[f64; 2]>>> {
    let adapter =
        FloatPointAdapter::<[f64; 2], i32>::with_iter_conservative(rings.iter().flatten());
    let mut inner = options;
    inner.min_output_area = 0.0;
    let run = |opts: OverlayOptions<f64>,
               subj: &dyn Fn(FloatOverlay<[f64; 2]>) -> FloatOverlay<[f64; 2]>,
               cap: usize,
               rule: OverlayRule| {
        subj(FloatOverlay::new_custom(
            adapter.clone(),
            opts,
            Default::default(),
            cap,
        ))
        .overlay(rule, OverlayFillRule::NonZero)
    };
    if let [ring] = rings {
        return run(
            options,
            &|o| o.unsafe_add_source(ring, ShapeType::Subject),
            ring.len(),
            OverlayRule::Subject,
        );
    }
    let mut level: Vec<Vec<Vec<Vec<[f64; 2]>>>> = crate::planar_map::parallel_map(rings, |r| {
        run(
            inner,
            &|o| o.unsafe_add_source(r, ShapeType::Subject),
            r.len(),
            OverlayRule::Subject,
        )
    });
    while level.len() > 1 {
        let opts = if level.len() == 2 { options } else { inner };
        let pairs: Vec<&[_]> = level.chunks(2).collect();
        level = crate::planar_map::parallel_map(&pairs, |pair| match pair {
            [a, b] => {
                let cap = a.iter().chain(b).flatten().map(|c| c.len()).sum();
                run(
                    opts,
                    &|o| {
                        o.unsafe_add_source(a, ShapeType::Subject)
                            .unsafe_add_source(b, ShapeType::Clip)
                    },
                    cap,
                    OverlayRule::Union,
                )
            }
            [a] => a.clone(),
            _ => Vec::new(),
        });
    }
    level.pop().unwrap_or_default()
}

/// 点の列 `all_pts[start..=end]` を、両端を結ぶ線から `epsilon` 以内のずれで間引き（Ramer–Douglas–Peucker）、
/// 残す点の `keep` を立てる。分ける区間を積んで1つずつ処理するので、点がいくつあっても途中で打ち切らない
/// （積む区間は、残す点の数を超えない）
fn rdp_keep(all_pts: &[Point], start: usize, end: usize, epsilon: f64, keep: &mut [bool]) {
    let mut spans = vec![(start, end)];
    while let Some((s, e)) = spans.pop() {
        if e <= s + 1 {
            continue;
        }
        let (a, b) = (all_pts[s], all_pts[e]);
        let (split, max_dist) = (s + 1..e)
            .map(|i| (i, point_to_segment_dist(all_pts[i], a, b)))
            .fold((s, -1.0), |best, c| if c.1 > best.1 { c } else { best });
        if max_dist > epsilon {
            keep[split] = true;
            spans.push((s, split));
            spans.push((split, e));
        }
    }
}

/// テスト用：閉じた折れ線 `raw`（NonZero で塗る）の形を、自分との交わりを整理して、直線の輪郭のまま図形にする
#[cfg(test)]
pub(crate) fn polygon_shapes(raw: &[Point], color: Color32) -> Vec<FillShape> {
    use i_overlay9::float::simplify::SimplifyShape;
    let pts: Vec<[f64; 2]> = raw.iter().map(|p| [p.x, p.y]).collect();
    let mut options = OverlayOptions::default();
    options.output_direction = ContourDirection::Clockwise;
    options.min_output_area = DUST_AREA_PX;
    pts.simplify_shape_custom(OverlayFillRule::NonZero, options, Default::default())
        .into_iter()
        .filter_map(|shape| {
            let mut contours: Vec<Contour> = shape
                .iter()
                .enumerate()
                .filter(|(_, ring)| ring.len() >= 3)
                .map(|(ri, ring)| {
                    let segs = (0..ring.len())
                        .map(|i| {
                            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                            let (a, b) = (Point::new(a[0], a[1]), Point::new(b[0], b[1]));
                            QuadBez::new(a, a.lerp(b, 0.5), b)
                        })
                        .collect();
                    let mut ct = Contour::new(segs);
                    if (ri == 0) != (ct.signed_area() > 0.0) {
                        ct.reverse();
                    }
                    ct
                })
                .collect();
            if contours.is_empty() {
                return None;
            }
            let exterior = contours.remove(0);
            Some(FillShape::new(color, exterior, contours))
        })
        .collect()
}
