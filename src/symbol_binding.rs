//! シンボルのインスタンスの設定（繰り返しの種類・色効果・インスタンスの結びつき）。図形 `FillShape`（形の計算の部屋）が
//! 「どのインスタンスの図形か」を項目に持ち、地図・タイムライン・シンボル・作業の部屋・パネルがそれを読むので、使い手の
//! いちばん下の Layer 1 に置く（置き場所：claude/層の分け方の直しの設計.md の 4.）

use eframe::egui::Color32;
use kurbo::{Affine, Point};
use serde::{Deserialize, Serialize};

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
