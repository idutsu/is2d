//! 塗りの色を選ぶ部品：RGBA の数字・今の色・明るさと鮮やかさの四角・色相の帯・不透明度の帯（egui の
//! `color_picker_color32` と同じ並び）。四角の中の今の色の位置は、十字で示す（egui の部品は、四角の幅に比べた大きさの
//! 丸で示し、その大きさを外から変えられないので、自分で作る）。幅はパネルの残りの幅いっぱい

use eframe::egui::{
    self,
    ecolor::{Hsva, HsvaGamma, Rgba},
    lerp, pos2, remap_clamp, vec2, Color32, Mesh, Sense, Shape, Stroke, StrokeKind,
};

/// 四角・帯の色を塗るときの、1辺の分け数（egui の色の部品と同じ。分けた所の間は色を直線でつなぐ）
const STEPS: u32 = 36;
/// 十字の腕の長さ（中心から、画面の点）
const CROSS_ARM: f32 = 6.0;

/// 色 `color` を選ぶ部品を描き、変えたら `true`
pub(crate) fn color_picker(ui: &mut egui::Ui, color: &mut Color32) -> bool {
    // 灰色（鮮やかさ 0）や黒（明るさ 0）の色からは、色相・鮮やかさが決まらない。そこで、前にこの部品で選んだ値を、
    // 色がそのままである間は使う（色相の帯の位置が、灰色を選んだとたんに赤へ飛ばない）
    let memory = ui.id().with("color_picker_hsva");
    let mut hsvag = ui
        .data(|d| d.get_temp::<(Color32, HsvaGamma)>(memory))
        .filter(|(remembered, _)| remembered == color)
        .map_or_else(|| HsvaGamma::from(Hsva::from(*color)), |(_, h)| h);
    let before = hsvag;

    ui.vertical(|ui| {
        rgba_fields(ui, &mut hsvag);
        let width = ui.available_width();
        let bar_height = ui.spacing().interact_size.y;
        let (swatch, _) = ui.allocate_exact_size(vec2(width, bar_height), Sense::hover());
        egui::color_picker::show_color_at(ui.painter(), Color32::from(hsvag), swatch);

        let opaque = HsvaGamma { a: 1.0, ..hsvag };
        let HsvaGamma { h, s, v, a } = &mut hsvag;
        saturation_value_square(ui, width, s, v, |s, v| HsvaGamma { s, v, ..opaque }.into());
        bar(ui, vec2(width, bar_height), h, |h| {
            HsvaGamma {
                h,
                s: 1.0,
                v: 1.0,
                a: 1.0,
            }
            .into()
        });
        bar(ui, vec2(width, bar_height), a, |a| {
            HsvaGamma { a, ..opaque }.into()
        });
    });

    *color = Color32::from(hsvag);
    ui.data_mut(|d| d.insert_temp(memory, (*color, hsvag)));
    hsvag != before
}

/// R・G・B・A の数字（0〜255）。変えたら、その数字の色にする
fn rgba_fields(ui: &mut egui::Ui, hsvag: &mut HsvaGamma) {
    let mut rgba = Hsva::from(*hsvag).to_srgba_unmultiplied();
    let mut changed = false;
    ui.horizontal(|ui| {
        for (value, prefix) in rgba.iter_mut().zip(["R ", "G ", "B ", "A "]) {
            changed |= ui
                .add(egui::DragValue::new(value).speed(0.5).prefix(prefix))
                .changed();
        }
    });
    if changed {
        *hsvag = HsvaGamma::from(Hsva::from_srgba_unmultiplied(rgba));
    }
}

/// 色の上に重ねる印の色：暗い色の上は白、明るい色の上は黒
fn contrast(color: Color32) -> Color32 {
    if Rgba::from(color).intensity() < 0.5 {
        Color32::WHITE
    } else {
        Color32::BLACK
    }
}

/// 鮮やかさ（横）と明るさ（縦）を選ぶ、幅 `width` の正方形。今の位置を十字で示す
fn saturation_value_square(
    ui: &mut egui::Ui,
    width: f32,
    s: &mut f32,
    v: &mut f32,
    color_at: impl Fn(f32, f32) -> Color32,
) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, width), Sense::click_and_drag());
    if let Some(p) = response.interact_pointer_pos() {
        *s = remap_clamp(p.x, rect.left()..=rect.right(), 0.0..=1.0);
        *v = remap_clamp(p.y, rect.bottom()..=rect.top(), 0.0..=1.0);
    }
    let mut mesh = Mesh::default();
    for yi in 0..=STEPS {
        for xi in 0..=STEPS {
            let (xt, yt) = (xi as f32 / STEPS as f32, yi as f32 / STEPS as f32);
            let at = pos2(
                lerp(rect.left()..=rect.right(), xt),
                lerp(rect.bottom()..=rect.top(), yt),
            );
            mesh.colored_vertex(at, color_at(xt, yt));
            if xi < STEPS && yi < STEPS {
                let tl = yi * (STEPS + 1) + xi;
                mesh.add_triangle(tl, tl + 1, tl + STEPS + 1);
                mesh.add_triangle(tl + 1, tl + STEPS + 1, tl + STEPS + 2);
            }
        }
    }
    let painter = ui.painter();
    painter.add(Shape::mesh(mesh));
    let visuals = ui.style().interact(&response);
    painter.rect_stroke(rect, 0.0, visuals.bg_stroke, StrokeKind::Inside);
    let center = pos2(
        lerp(rect.left()..=rect.right(), *s),
        lerp(rect.bottom()..=rect.top(), *v),
    );
    let stroke = Stroke::new(visuals.fg_stroke.width, contrast(color_at(*s, *v)));
    let painter = painter.with_clip_rect(rect);
    painter.line_segment(
        [center - vec2(CROSS_ARM, 0.0), center + vec2(CROSS_ARM, 0.0)],
        stroke,
    );
    painter.line_segment(
        [center - vec2(0.0, CROSS_ARM), center + vec2(0.0, CROSS_ARM)],
        stroke,
    );
}

/// 値 `value`（0〜1）を左右で選ぶ、大きさ `size` の帯。下に透けた所の市松模様を敷き、今の位置を三角で示す
fn bar(ui: &mut egui::Ui, size: egui::Vec2, value: &mut f32, color_at: impl Fn(f32) -> Color32) {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if let Some(p) = response.interact_pointer_pos() {
        *value = remap_clamp(p.x, rect.left()..=rect.right(), 0.0..=1.0);
    }
    let painter = ui.painter();
    egui::color_picker::show_color_at(painter, Color32::TRANSPARENT, rect);
    let mut mesh = Mesh::default();
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let x = lerp(rect.left()..=rect.right(), t);
        mesh.colored_vertex(pos2(x, rect.top()), color_at(t));
        mesh.colored_vertex(pos2(x, rect.bottom()), color_at(t));
        if i < STEPS {
            mesh.add_triangle(2 * i, 2 * i + 1, 2 * i + 2);
            mesh.add_triangle(2 * i + 1, 2 * i + 2, 2 * i + 3);
        }
    }
    painter.add(Shape::mesh(mesh));
    let visuals = ui.style().interact(&response);
    painter.rect_stroke(rect, 0.0, visuals.bg_stroke, StrokeKind::Inside);
    let x = lerp(rect.left()..=rect.right(), *value);
    let r = rect.height() / 4.0;
    let picked = color_at(*value);
    painter.add(Shape::convex_polygon(
        vec![
            pos2(x, rect.center().y),
            pos2(x + r, rect.bottom()),
            pos2(x - r, rect.bottom()),
        ],
        picked,
        Stroke::new(visuals.fg_stroke.width, contrast(picked)),
    ));
}
