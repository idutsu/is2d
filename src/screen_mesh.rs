//! 画面に描く仕事（Layer 3。使い手は画面の土台・道具・パネル）：文書の座標の三角形を、画面の座標に写し、画面だけの色合い
//! （選んだ図形の色・グループやシンボルの編集中の薄い表示・オニオンスキン）をかける。形の計算の部屋（Layer 1）とシンボルの
//! 部屋（Layer 2）が作るのは、文書の座標の三角形だけ（置き場所：claude/層の分け方の直しの設計.md の 5.）

use eframe::egui::{Color32, Mesh, Pos2, Vec2};

use crate::vector_engine::FillShape;

/// 図形 `shape` を、倍率 `zoom`・パン `pan` の画面の座標に写した三角形（倍率に合わせた細かさ：`FillShape::view_mesh`）。
/// `dim_backdrop` なら薄い表示の色にする
pub(crate) fn shape_screen_mesh(shape: &FillShape, zoom: f32, pan: Vec2, dim_backdrop: bool) -> Mesh {
    let tint: Option<&dyn Fn(Color32) -> Color32> = if dim_backdrop { Some(&dim_backdrop_color) } else { None };
    transform_mesh_to_screen((*shape.view_mesh(zoom)).clone(), zoom, pan, tint)
}

/// 選択表示：図形 `shape` の選択の色を塗るメッシュ（`FillShape::selection_mesh`。線の図形は両端を丸くした帯）を画面座標に
/// 写し、頂点の色を選択の色（`selection_color`：図形の色に `SELECTION_TINT` を重ねた色）にしたもの。同じ段の図形の上に
/// 描く。どの倍率でも、図形と同じ形のまま青みがかって見える
pub(crate) fn shape_selection_screen_mesh(shape: &FillShape, zoom: f32, pan: Vec2) -> Mesh {
    let mut mesh = (*shape.selection_mesh(zoom)).clone();
    for v in &mut mesh.vertices {
        v.pos = to_screen(v.pos.x, v.pos.y, zoom, pan);
        v.color = selection_color(v.color);
    }
    mesh
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
