use eframe::egui::{self, Color32, Stroke, Vec2};

use crate::canvas_tool::{draw_registration_cross, SelectSubMode, Tool};
use crate::symbol::{SymbolDef, SymbolLoopMode, TweenRotate};
use crate::tool_transform::{
    convert_selection_to_symbol, delete_symbol_from_library, enter_symbol_from_library,
    enter_symbol_timeline, place_symbol_instance_from_library, rename_symbol_in_library,
    selected_single_symbol_info, set_active_keyframe_tween_rotate, set_selected_symbol_playback,
};
use crate::vector_engine::arc_mesh_to_shape;
use crate::Is2dApp;

/// OS標準の修飾キー表記（macOS では `"Cmd"`、Windows / Linux では `"Ctrl"`）
pub(crate) const MOD_KEY: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

// ============================================================================
// Material Icons (`fonts/MaterialIcons-Regular.ttf`) コードポイント定義
// ============================================================================

pub(crate) mod icons {
    pub(crate) const ADD: &str = "\u{e145}";          // add (＋)
    pub(crate) const REMOVE: &str = "\u{e15b}";       // remove (－)
    pub(crate) const FLIP_H: &str = "\u{e8d4}";       // swap_horiz (左右反転)
    pub(crate) const FLIP_V: &str = "\u{e8d5}";       // swap_vert (上下反転)
    pub(crate) const ROTATE_LEFT: &str = "\u{e419}";  // rotate_left (-90°回転)
    pub(crate) const ROTATE_RIGHT: &str = "\u{e41a}"; // rotate_right (+90°回転)
    pub(crate) const PIVOT_RESET: &str = "\u{e3b4}";  // center_focus_strong (ピボットリセット)
    pub(crate) const GROUP: &str = "\u{e1bd}";        // widgets (グループ表示)
    pub(crate) const SYMBOL: &str = "\u{e87b}";       // extension (シンボル表示)
    pub(crate) const WIREFRAME: &str = "\u{e922}";    // timeline (アンカー・パス表示)
    pub(crate) const DELETE: &str = "\u{e872}";       // delete (削除)
    pub(crate) const SCENE_MOVIE: &str = "\u{e02c}";  // movie (シーンアイコン)
    pub(crate) const ARROW_BACK: &str = "\u{e5c4}";   // arrow_back (1階層出る)
    pub(crate) const VISIBILITY: &str = "\u{e8f4}";   // visibility (レイヤー表示)
    pub(crate) const LOCK: &str = "\u{e897}";         // lock (レイヤーロック)
    pub(crate) const SKIP_PREV: &str = "\u{e045}";    // skip_previous (先頭フレームへ)
    pub(crate) const STEP_PREV: &str = "\u{e408}";    // navigate_before (前のフレームへ)
    pub(crate) const PLAY: &str = "\u{e037}";         // play_arrow (再生)
    pub(crate) const PAUSE: &str = "\u{e034}";        // pause (一時停止)
    pub(crate) const STEP_NEXT: &str = "\u{e409}";    // navigate_next (次のフレームへ)
    pub(crate) const SKIP_NEXT: &str = "\u{e044}";    // skip_next (最終フレームへ)
    pub(crate) const ONION_SKIN: &str = "\u{e3e0}";   // filter_none (オニオンスキン)
}

// ============================================================================
// 右側パネルの状態・タブ定義（プロパティ / カラー / ライブラリ）
// ============================================================================

/// 右側パネル上部の切り替えタブ（プロパティ / カラー / ライブラリ / キャンバス）
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum RightPanelTab {
    #[default]
    Properties, // プロパティ（現在選択中のツールの設定を表示）
    Color,      // カラー
    Library,    // ライブラリ
    Canvas,     // キャンバス（ステージサイズ・背景色・枠外マスク・表示倍率リセット）
}

impl RightPanelTab {
    /// 右パネル上部に並べるタブの標準順序
    pub(crate) const ALL: [RightPanelTab; 4] = [
        RightPanelTab::Properties,
        RightPanelTab::Color,
        RightPanelTab::Library,
        RightPanelTab::Canvas,
    ];

    /// タブに表示するラベル名
    pub(crate) const fn label(self) -> &'static str {
        match self {
            RightPanelTab::Properties => "プロパティ",
            RightPanelTab::Color => "カラー",
            RightPanelTab::Library => "ライブラリ",
            RightPanelTab::Canvas => "キャンバス",
        }
    }
}

// ============================================================================
// 左端縦ツールバー・ステージ上部バー・右端プロパティパネルの描画実装
// ============================================================================

// ------------------------------------------------------------------------
// 1. 左端縦ツールバー（Adobe Animate 準拠の縦長ツールボックス）
// ------------------------------------------------------------------------

pub(crate) fn draw_left_toolbar(app: &mut Is2dApp, ctx: &egui::Context) {
    egui::SidePanel::left("left_vertical_toolbar")
        .resizable(false)
        .exact_width(44.0)
        .frame(
            egui::Frame::none()
                .fill(Color32::from_rgb(245, 246, 248))
                .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                .stroke(Stroke::new(1.0, Color32::from_rgb(210, 214, 222))),
        )
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let btn_size = Vec2::new(34.0, 30.0);

                // (1) メインツール選択ボタン群（Tool::ALL を自動ループ）
                draw_left_main_tool_buttons(app, ui, btn_size);

                ui.separator();

                // (2) 塗りカラーピッカーボタン
                draw_color_picker_controls(app, ui);
            });
        });
}

/// 左ツールバー用の選択トグルボタン（アイコンサイズ 20px で大きく見やすく描画）
fn add_left_selectable_icon(
    ui: &mut egui::Ui,
    btn_size: Vec2,
    selected: bool,
    icon: &str,
    tooltip: &str,
) -> bool {
    ui.add_sized(
        btn_size,
        egui::SelectableLabel::new(selected, egui::RichText::new(icon).size(20.0)),
    )
    .on_hover_text(tooltip)
    .clicked()
}

fn draw_left_main_tool_buttons(app: &mut Is2dApp, ui: &mut egui::Ui, btn_size: Vec2) {
    for tool in Tool::ALL {
        if add_left_selectable_icon(
            ui,
            btn_size,
            app.ui.tool == tool,
            tool.icon(),
            tool.tooltip(),
        ) {
            crate::canvas_tool::set_tool(app, tool);
        }
    }
}

fn draw_color_picker_controls(app: &mut Is2dApp, ui: &mut egui::Ui) {
    ui.color_edit_button_srgba(&mut app.ui.brush_color)
        .on_hover_text("塗りカラー変更");
}

// ------------------------------------------------------------------------
// 2. ステージ上部のスリムバー（シーン／グループ階層パンくず・右側ズーム倍率表示）
// ------------------------------------------------------------------------

pub(crate) fn draw_stage_header_bar(app: &mut Is2dApp, ctx: &egui::Context) {
    egui::TopBottomPanel::top("stage_header_bar")
        .exact_height(30.0)
        .frame(
            egui::Frame::none()
                .fill(Color32::from_rgb(245, 246, 248))
                .inner_margin(egui::Margin::symmetric(10.0, 0.0))
                .stroke(Stroke::new(1.0, Color32::from_rgb(214, 218, 226))),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                draw_group_breadcrumb(app, ui);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let zoom_pct = (app.viewport.zoom * 100.0).round() as i32;
                    ui.label(
                        egui::RichText::new(format!("{zoom_pct}%"))
                            .size(11.5)
                            .color(Color32::from_rgb(90, 98, 110)),
                    );
                });
            });
        });
}

fn draw_group_breadcrumb(app: &mut Is2dApp, ui: &mut egui::Ui) {
    let mut items: Vec<String> = vec![format!("{} シーン 1", icons::SCENE_MOVIE)];
    for sess in &app.symbols.edit_stack {
        for &gid in &sess.parent_editing_group_path {
            items.push(format!("{} Group #{gid}", icons::GROUP));
        }
        let sym_name = app
            .symbols
            .library
            .iter()
            .find(|s| s.id == sess.symbol_id)
            .map(|s| s.name.as_str())
            .unwrap_or("Symbol");
        items.push(format!("{} {sym_name}", icons::SYMBOL));
    }
    for &gid in &app.canvas.editing_group_path {
        items.push(format!("{} Group #{gid}", icons::GROUP));
    }

    if items.len() == 1 {
        ui.label(&items[0]);
        return;
    }

    if ui
        .button(icons::ARROW_BACK)
        .on_hover_text("1つ上の階層へ戻る (Esc)")
        .clicked()
    {
        crate::tool_select::exit_current_group(app);
        return;
    }

    let last_idx = items.len() - 1;
    let mut exit_steps = 0usize;
    for (idx, label) in items.iter().enumerate() {
        if idx > 0 {
            ui.label(
                egui::RichText::new("›")
                    .size(12.0)
                    .color(Color32::from_rgb(130, 138, 150)),
            );
        }
        if idx < last_idx {
            if ui
                .add(egui::Button::new(label).frame(false))
                .on_hover_text("クリックしてこの階層へ直接戻る")
                .clicked()
            {
                exit_steps = last_idx - idx;
            }
        } else {
            ui.colored_label(Color32::from_rgb(0, 120, 215), label);
        }
    }

    for _ in 0..exit_steps {
        if !crate::tool_select::exit_current_group(app) {
            break;
        }
    }
}

// ------------------------------------------------------------------------
// 3. 右端パネル（プロパティ / カラー / ライブラリ / キャンバス の 4 タブ切り替え）
// ------------------------------------------------------------------------

pub(crate) fn draw_right_properties_panel(app: &mut Is2dApp, ctx: &egui::Context) {
    egui::SidePanel::right("right_properties_panel")
        .exact_width(280.0)
        .resizable(false)
        .frame(
            egui::Frame::none()
                .fill(Color32::from_rgb(245, 246, 248))
                .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                .stroke(Stroke::new(1.0, Color32::from_rgb(214, 218, 226))),
        )
        .show(ctx, |ui| {
            // 上部タブバー（プロパティ | カラー | ライブラリ | キャンバス）
            draw_right_panel_tab_bar(app, ui);

            ui.add_space(4.0);
            ui.separator();
            ui.add_space(6.0);

            // アクティブなタブに応じた中身を描画
            match app.ui.right_panel_tab {
                RightPanelTab::Properties => draw_properties_tab(app, ui),
                RightPanelTab::Color => draw_color_palette_section(app, ui),
                RightPanelTab::Library => draw_library_tab(app, ui),
                RightPanelTab::Canvas => draw_canvas_tab(app, ui),
            }
        });
}

/// 右パネル上部の4タブ切り替えバー（`プロパティ | カラー | ライブラリ | キャンバス`）を描画
fn draw_right_panel_tab_bar(app: &mut Is2dApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.spacing_mut().button_padding = Vec2::new(3.0, 4.0);
        let tab_count = RightPanelTab::ALL.len() as f32;
        let btn_w = (ui.available_width() - 3.0 * (tab_count - 1.0)) / tab_count;
        for tab in RightPanelTab::ALL {
            let selected = app.ui.right_panel_tab == tab;
            let fill = if selected {
                Color32::from_rgb(255, 255, 255)
            } else {
                Color32::from_rgb(235, 237, 242)
            };
            let text_color = if selected {
                Color32::from_rgb(20, 25, 35)
            } else {
                Color32::from_rgb(115, 122, 135)
            };
            let mut text = egui::RichText::new(tab.label())
                .size(11.0)
                .color(text_color);
            if selected {
                text = text.strong();
            }
            let btn = egui::Button::new(text)
                .fill(fill)
                .stroke(Stroke::new(
                    1.0,
                    if selected {
                        Color32::from_rgb(190, 196, 208)
                    } else {
                        Color32::from_rgb(216, 220, 228)
                    },
                ))
                .min_size(Vec2::new(btn_w, 24.0));

            if ui.add(btn).clicked() {
                app.ui.right_panel_tab = tab;
            }
        }
    });
}

/// 「プロパティ」タブの描画（現在選択中の `app.ui.tool` に応じてツール別の設定メソッドへ振り分け）
fn draw_properties_tab(app: &mut Is2dApp, ui: &mut egui::Ui) {
    // 現在選択中のツール見出し
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(app.ui.tool.icon()).size(16.0));
        ui.label(
            egui::RichText::new(app.ui.tool.label())
                .size(12.5)
                .strong()
                .color(Color32::from_rgb(35, 40, 50)),
        );
    });
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);

    // 選択中のツールごとに専用メソッドへディスパッチ
    match app.ui.tool {
        Tool::Select => draw_select_tool_properties(app, ui),
        Tool::Transform => draw_transform_tool_properties(app, ui),
        Tool::Brush => draw_radius_slider_section(
            ui,
            "ブラシサイズ",
            &mut app.ui.brush_radius,
            0.5,
            70.0,
            0.5,
        ),
        Tool::Eraser => draw_radius_slider_section(
            ui,
            "消しゴムサイズ",
            &mut app.ui.eraser_radius,
            1.0,
            80.0,
            1.0,
        ),
        Tool::Bucket | Tool::Eyedropper => draw_color_palette_section(app, ui),
    }

    // 下部ぶら下がりセクション：
    // - タイムラインのフレームを明示的に選択したときは「キーフレーム / トゥイーン設定（回転の向き等）」
    // - キャンバス上のシンボルを選択したとき（選択 / 自由変形ツール時）は「シンボル再生オプション」
    if app.ui.inspect_timeline_frame {
        draw_selected_frame_properties(app, ui);
    } else if matches!(app.ui.tool, Tool::Select | Tool::Transform) {
        draw_selected_symbol_properties(app, ui);
    }
}

/// 選択ツール (`Tool::Select`) のプロパティ設定項目
fn draw_select_tool_properties(app: &mut Is2dApp, ui: &mut egui::Ui) {
    draw_panel_section_label(ui, "アンカー編集モード", 4.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let half_w = (ui.available_width() - 6.0) * 0.5;
        if ui
            .add_sized(
                Vec2::new(half_w, 28.0),
                egui::Button::new(format!("{} アンカー追加", icons::ADD))
                    .selected(app.ui.select_sub_mode == SelectSubMode::AddAnchor),
            )
            .on_hover_text("輪郭上をクリックしてアンカーポイントを追加 (+)")
            .clicked()
        {
            crate::canvas_tool::toggle_select_sub_mode(app, SelectSubMode::AddAnchor);
        }
        if ui
            .add_sized(
                Vec2::new(half_w, 28.0),
                egui::Button::new(format!("{} アンカー削除", icons::REMOVE))
                    .selected(app.ui.select_sub_mode == SelectSubMode::DeleteAnchor),
            )
            .on_hover_text("アンカーポイントをクリックして削除 (-)")
            .clicked()
        {
            crate::canvas_tool::toggle_select_sub_mode(app, SelectSubMode::DeleteAnchor);
        }
    });

    ui.add_space(6.0);

    if ui
        .add_sized(
            Vec2::new(ui.available_width(), 28.0),
            egui::Button::new(format!("{} アンカーを表示", icons::WIREFRAME))
                .selected(app.ui.show_wireframe),
        )
        .on_hover_text("選択ツール使用中にベクター輪郭のアンカーポイントとパスを表示")
        .clicked()
    {
        app.ui.show_wireframe = !app.ui.show_wireframe;
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    draw_symbol_and_group_section(app, ui);

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    draw_panel_section_label(ui, "消去アクション", 4.0);

    let has_sel = !app.canvas.selected_shapes.is_empty();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let half_w = (ui.available_width() - 6.0) * 0.5;
        if ui
            .add_enabled(
                has_sel,
                egui::Button::new(format!("{} 選択削除", icons::DELETE))
                    .min_size(Vec2::new(half_w, 28.0)),
            )
            .on_hover_text("選択中の図形を削除 (Backspace / Delete)")
            .clicked()
        {
            crate::tool_select::delete_selected_shapes(app);
        }
        let has_any = app
            .canvas
            .shapes
            .iter()
            .any(|s| s.is_in_editing_scope(&app.canvas.editing_group_path));
        if ui
            .add_enabled(
                has_any,
                egui::Button::new("フレーム全消去").min_size(Vec2::new(half_w, 28.0)),
            )
            .on_hover_text("現在の編集スコープ・フレーム内の図形をすべて消去")
            .clicked()
        {
            crate::tool_select::clear_active_frame_shapes(app);
        }
    });
}

/// 選択・自由変形ツール共通の「シンボル・グループ」操作UIセクション
fn draw_symbol_and_group_section(app: &mut Is2dApp, ui: &mut egui::Ui) {
    draw_panel_section_label(ui, "シンボル・グループ", 4.0);

    let has_sel = !app.canvas.selected_shapes.is_empty();
    let has_group = app.canvas.selection_has_group();
    if ui
        .add_enabled(
            has_sel,
            egui::Button::new(format!("{} シンボルに変換 (F8)", icons::SYMBOL))
                .min_size(Vec2::new(ui.available_width(), 28.0)),
        )
        .on_hover_text("選択中の図形をシンボルに変換してライブラリに登録します (F8)")
        .clicked()
    {
        convert_selection_to_symbol(app);
    }

    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let half_w = (ui.available_width() - 6.0) * 0.5;
        if ui
            .add_enabled(
                has_sel,
                egui::Button::new(format!("{} グループ化", icons::GROUP))
                    .min_size(Vec2::new(half_w, 28.0)),
            )
            .on_hover_text(format!(
                "選択中のオブジェクトを1つのグループにまとめる ({MOD_KEY}+G)"
            ))
            .clicked()
        {
            crate::tool_select::group_selected_shapes(app);
        }
        if ui
            .add_enabled(
                has_group,
                egui::Button::new(format!("解除 ({MOD_KEY}+B)")).min_size(Vec2::new(half_w, 28.0)),
            )
            .on_hover_text(format!(
                "選択中のグループまたはシンボルインスタンスを1段階解除する ({MOD_KEY}+B)"
            ))
            .clicked()
        {
            crate::tool_select::ungroup_selected_shapes(app);
        }
    });
}

/// 自由変形ツール (`Tool::Transform`) のプロパティ設定項目
fn draw_transform_tool_properties(app: &mut Is2dApp, ui: &mut egui::Ui) {
    let has_sel = !app.canvas.selected_shapes.is_empty();

    draw_panel_section_label(ui, "変形アクション", 4.0);

    ui.add_enabled_ui(has_sel, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let half_w = (ui.available_width() - 6.0) * 0.5;
            if ui
                .add_sized(
                    Vec2::new(half_w, 28.0),
                    egui::Button::new(format!("{} 左右反転", icons::FLIP_H)),
                )
                .on_hover_text("選択シェイプを左右反転")
                .clicked()
            {
                crate::tool_transform::flip_selected_shapes(app, true, false);
            }
            if ui
                .add_sized(
                    Vec2::new(half_w, 28.0),
                    egui::Button::new(format!("{} 上下反転", icons::FLIP_V)),
                )
                .on_hover_text("選択シェイプを上下反転")
                .clicked()
            {
                crate::tool_transform::flip_selected_shapes(app, false, true);
            }
        });

        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let half_w = (ui.available_width() - 6.0) * 0.5;
            if ui
                .add_sized(
                    Vec2::new(half_w, 28.0),
                    egui::Button::new(format!("{} -90° 回転", icons::ROTATE_LEFT)),
                )
                .on_hover_text("選択シェイプを反時計回りに 90° 回転")
                .clicked()
            {
                crate::tool_transform::rotate_selected_shapes(app, -std::f64::consts::FRAC_PI_2);
            }
            if ui
                .add_sized(
                    Vec2::new(half_w, 28.0),
                    egui::Button::new(format!("{} +90° 回転", icons::ROTATE_RIGHT)),
                )
                .on_hover_text("選択シェイプを時計回りに 90° 回転")
                .clicked()
            {
                crate::tool_transform::rotate_selected_shapes(app, std::f64::consts::FRAC_PI_2);
            }
        });
    });

    ui.add_space(6.0);

    if ui
        .add_enabled(
            has_sel && app.canvas.has_custom_transform_pivot(),
            egui::Button::new(format!("{} 変形中心（ピボット）をリセット", icons::PIVOT_RESET))
                .min_size(Vec2::new(ui.available_width(), 28.0)),
        )
        .on_hover_text("移動した変形中心（ピボット）を初期位置（選択中央 / シンボル基準点）へ戻す")
        .clicked()
    {
        crate::tool_transform::reset_selected_transform_pivot(app);
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    draw_symbol_and_group_section(app, ui);
}

/// ブラシ・消しゴムのサイズスライダー共通UIセクション
fn draw_radius_slider_section(
    ui: &mut egui::Ui,
    label: &str,
    radius: &mut f32,
    min_r: f32,
    max_r: f32,
    step: f64,
) {
    draw_panel_section_label(ui, label, 4.0);
    let mut slider = egui::Slider::new(radius, min_r..=max_r)
        .step_by(step)
        .suffix(" px");
    if (step - 1.0).abs() < 1e-6 {
        slider = slider.integer();
    }
    ui.add(slider);
}

/// カラーピッカーと10色プリセットパレットの共通UIセクション
fn draw_color_palette_section(app: &mut Is2dApp, ui: &mut egui::Ui) {
    const PRESET_COLORS: [Color32; 10] = [
        Color32::from_rgb(40, 45, 55),
        Color32::from_rgb(255, 255, 255),
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(245, 145, 55),
        Color32::from_rgb(245, 210, 60),
        Color32::from_rgb(65, 195, 105),
        Color32::from_rgb(50, 190, 210),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(145, 95, 235),
        Color32::from_rgb(235, 105, 175),
    ];

    draw_panel_section_label(ui, "現在の塗りカラー", 4.0);
    ui.horizontal(|ui| {
        ui.color_edit_button_srgba(&mut app.ui.brush_color);
        let [r, g, b, _] = app.ui.brush_color.to_srgba_unmultiplied();
        ui.label(
            egui::RichText::new(format!("#{r:02X}{g:02X}{b:02X}"))
                .monospace()
                .size(11.5)
                .color(Color32::from_rgb(70, 78, 90)),
        );
    });

    ui.add_space(8.0);
    draw_panel_section_label(ui, "プリセットパレット (10色)", 4.0);

    for row in PRESET_COLORS.chunks(5) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let swatch_w = ((ui.available_width() - 6.0 * 4.0) / 5.0).max(24.0);
            for &col in row {
                let is_active = app.ui.brush_color == col;
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(swatch_w, 24.0), egui::Sense::click());
                let painter = ui.painter();
                painter.rect_filled(rect, 0.0, col);
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(
                        if is_active { 2.0 } else { 1.0 },
                        if is_active {
                            Color32::from_rgb(0, 125, 230)
                        } else {
                            Color32::from_rgb(185, 192, 204)
                        },
                    ),
                );
                if resp.clicked() {
                    app.ui.brush_color = col;
                }
            }
        });
        ui.add_space(4.0);
    }
}

/// 「キャンバス」タブの描画（ステージサイズ・枠外マスク・ウィンドウに合わせる・表示倍率リセット）
fn draw_canvas_tab(app: &mut Is2dApp, ui: &mut egui::Ui) {
    draw_panel_section_label(ui, "キャンバス情報", 6.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("ステージ:")
                .size(11.5)
                .color(Color32::from_rgb(70, 78, 90)),
        );
        let mut w = app.viewport.stage_size.x.round() as i32;
        let mut h = app.viewport.stage_size.y.round() as i32;
        let w_resp = ui
            .add(
                egui::DragValue::new(&mut w)
                    .range(100..=3840)
                    .speed(2.0),
            )
            .on_hover_text("ステージの幅 (px)");
        if w_resp.changed() && (w as f32 - app.viewport.stage_size.x).abs() > f32::EPSILON {
            if !app.viewport.editing_stage_size {
                app.push_undo();
                app.viewport.editing_stage_size = true;
            }
            app.viewport.stage_size.x = w as f32;
        }
        ui.label("×");
        let h_resp = ui
            .add(
                egui::DragValue::new(&mut h)
                    .range(100..=2160)
                    .speed(2.0)
                    .suffix(" px"),
            )
            .on_hover_text("ステージの高さ (px)");
        if h_resp.changed() && (h as f32 - app.viewport.stage_size.y).abs() > f32::EPSILON {
            if !app.viewport.editing_stage_size {
                app.push_undo();
                app.viewport.editing_stage_size = true;
            }
            app.viewport.stage_size.y = h as f32;
        }
        if w_resp.drag_stopped()
            || w_resp.lost_focus()
            || h_resp.drag_stopped()
            || h_resp.lost_focus()
        {
            app.viewport.editing_stage_size = false;
        }
    });

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    draw_panel_section_label(ui, "キャンバス操作", 6.0);

    if ui
        .add_sized(
            Vec2::new(ui.available_width(), 28.0),
            egui::Button::new("枠外を隠す").selected(app.viewport.clip_outside_stage),
        )
        .on_hover_text(format!(
            "ステージ枠の外側にはみ出た図形を隠して完成画面をプレビューします ({MOD_KEY}+3)"
        ))
        .clicked()
    {
        app.viewport.clip_outside_stage = !app.viewport.clip_outside_stage;
    }

    ui.add_space(6.0);

    if ui
        .add_sized(
            Vec2::new(ui.available_width(), 28.0),
            egui::Button::new("ウィンドウに合わせる"),
        )
        .on_hover_text(format!(
            "ステージ全体が中央キャンバス領域に収まるようにズームと位置を自動調整します ({MOD_KEY}+2)"
        ))
        .clicked()
    {
        app.viewport.fit_stage_to_window();
    }

    ui.add_space(6.0);

    if ui
        .add_sized(
            Vec2::new(ui.available_width(), 28.0),
            egui::Button::new("100%表示"),
        )
        .on_hover_text(format!(
            "クリック（または {MOD_KEY}+1）で 100% 表示・初期位置にリセットします"
        ))
        .clicked()
    {
        app.viewport.reset_view();
    }
}

/// 右パネル内の各セクション見出しラベルと下部余白を描画する共通ヘルパー
pub(crate) fn draw_panel_section_label(ui: &mut egui::Ui, title: &str, bottom_space: f32) {
    ui.label(
        egui::RichText::new(title)
            .size(11.5)
            .strong()
            .color(Color32::from_rgb(55, 62, 74)),
    );
    ui.add_space(bottom_space);
}

// ------------------------------------------------------------------------
// 4. フレーム（トゥイーン）プロパティ・シンボルプロパティ・ライブラリパネルUI描画
// ------------------------------------------------------------------------

/// タイムラインでキーフレーム（またはトゥイーン区間）を明示的に選択しているときにプロパティパネル下部に表示するトゥイーン設定（回転の向き）
pub(crate) fn draw_selected_frame_properties(app: &mut Is2dApp, ui: &mut egui::Ui) {
    let Some((kf_start_frame, current_rotate, is_complete)) =
        app.timeline.active_tween_keyframe_info()
    else {
        return;
    };

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("⇄ クラシックトゥイーン")
                .size(11.5)
                .strong()
                .color(Color32::from_rgb(105, 55, 185)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let status_label = if is_complete {
                format!("KF {}f", kf_start_frame + 1)
            } else {
                format!("KF {}f (未完)", kf_start_frame + 1)
            };
            ui.label(
                egui::RichText::new(status_label)
                    .size(10.5)
                    .color(Color32::from_rgb(110, 118, 130)),
            );
        });
    });

    ui.add_space(6.0);
    draw_panel_section_label(ui, "回転の向き", 4.0);

    let mut next_rotate = current_rotate;
    if ui
        .add_sized(
            Vec2::new(ui.available_width(), 26.0),
            egui::Button::new("自動（最短角度）")
                .selected(current_rotate == TweenRotate::Auto),
        )
        .on_hover_text("開始角度から終了角度へ最短の角度差で回転します")
        .clicked()
    {
        next_rotate = TweenRotate::Auto;
    }

    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let half_w = (ui.available_width() - 6.0) * 0.5;
        if ui
            .add_sized(
                Vec2::new(half_w, 26.0),
                egui::Button::new(format!("{} 時計回り (CW)", icons::ROTATE_RIGHT))
                    .selected(current_rotate == TweenRotate::Clockwise),
            )
            .on_hover_text("時計回り（右回り）方向に回転して補間します")
            .clicked()
        {
            next_rotate = TweenRotate::Clockwise;
        }
        if ui
            .add_sized(
                Vec2::new(half_w, 26.0),
                egui::Button::new(format!("{} 反時計回り (CCW)", icons::ROTATE_LEFT))
                    .selected(current_rotate == TweenRotate::CounterClockwise),
            )
            .on_hover_text("反時計回り（左回り）方向に回転して補間します")
            .clicked()
        {
            next_rotate = TweenRotate::CounterClockwise;
        }
    });

    if next_rotate != current_rotate {
        set_active_keyframe_tween_rotate(app, next_rotate);
    }
}

/// キャンバス上で単一のシンボルインスタンスを選択しているときにプロパティパネル下部に表示する再生オプション（1回再生 / ループ再生 / 任意のフレーム）
pub(crate) fn draw_selected_symbol_properties(app: &mut Is2dApp, ui: &mut egui::Ui) {
    let Some((binding, sym_name, sym_total_frames)) = selected_single_symbol_info(app) else {
        return;
    };

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} {sym_name}", icons::SYMBOL))
                .size(11.5)
                .strong()
                .color(Color32::from_rgb(0, 120, 215)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("全 {sym_total_frames} f"))
                    .size(10.5)
                    .color(Color32::from_rgb(110, 118, 130)),
            );
        });
    });

    ui.add_space(6.0);
    draw_panel_section_label(ui, "シンボル再生オプション", 4.0);

    let mut next_mode = binding.loop_mode;
    let max_first_1based = sym_total_frames.max(1);
    let current_first_1based = (binding.first_frame + 1).clamp(1, max_first_1based);
    let mut next_first_1based = current_first_1based;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let half_w = (ui.available_width() - 6.0) * 0.5;
        if ui
            .add_sized(
                Vec2::new(half_w, 26.0),
                egui::Button::new("ループ再生")
                    .selected(binding.loop_mode == SymbolLoopMode::Loop),
            )
            .on_hover_text("キーフレーム開始位置からシンボル内部フレームをループ再生します")
            .clicked()
        {
            next_mode = SymbolLoopMode::Loop;
        }
        if ui
            .add_sized(
                Vec2::new(half_w, 26.0),
                egui::Button::new("1回再生")
                    .selected(binding.loop_mode == SymbolLoopMode::PlayOnce),
            )
            .on_hover_text(
                "キーフレーム開始位置からシンボル内部の最終フレームまで1回再生して停止します",
            )
            .clicked()
        {
            next_mode = SymbolLoopMode::PlayOnce;
        }
    });

    ui.add_space(4.0);

    if ui
        .add_sized(
            Vec2::new(ui.available_width(), 26.0),
            egui::Button::new("任意のフレーム")
                .selected(binding.loop_mode == SymbolLoopMode::SingleFrame),
        )
        .on_hover_text("指定したシンボル内部フレーム（1コマ）を固定表示します")
        .clicked()
    {
        next_mode = SymbolLoopMode::SingleFrame;
    }

    ui.add_space(6.0);

    let frame_label = match next_mode {
        SymbolLoopMode::SingleFrame => "表示フレーム:",
        SymbolLoopMode::Loop | SymbolLoopMode::PlayOnce => "開始フレーム:",
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(
            egui::RichText::new(frame_label)
                .size(11.0)
                .color(Color32::from_rgb(55, 62, 74)),
        );
        if ui
            .add_enabled(
                next_first_1based > 1,
                egui::Button::new(icons::STEP_PREV).min_size(Vec2::new(22.0, 22.0)),
            )
            .on_hover_text("前のシンボル内フレーム")
            .clicked()
        {
            next_first_1based = next_first_1based.saturating_sub(1).max(1);
        }
        ui.add_sized(
            Vec2::new(56.0, 22.0),
            egui::DragValue::new(&mut next_first_1based)
                .range(1..=max_first_1based)
                .speed(0.15)
                .suffix(" f"),
        );
        if ui
            .add_enabled(
                next_first_1based < max_first_1based,
                egui::Button::new(icons::STEP_NEXT).min_size(Vec2::new(22.0, 22.0)),
            )
            .on_hover_text("次のシンボル内フレーム")
            .clicked()
        {
            next_first_1based = (next_first_1based + 1).min(max_first_1based);
        }
    });

    if next_mode == SymbolLoopMode::SingleFrame && (2..=12).contains(&max_first_1based) {
        ui.add_space(4.0);
        for chunk_start in (1..=max_first_1based).step_by(6) {
            let chunk_end = (chunk_start + 5).min(max_first_1based);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let cols = (chunk_end - chunk_start + 1) as f32;
                let btn_w = ((ui.available_width() - 3.0 * (cols - 1.0)) / cols).max(24.0);
                for f in chunk_start..=chunk_end {
                    if ui
                        .add_sized(
                            Vec2::new(btn_w, 22.0),
                            egui::Button::new(format!("{f}F"))
                                .selected(next_first_1based == f),
                        )
                        .clicked()
                    {
                        next_first_1based = f;
                    }
                }
            });
            if chunk_end < max_first_1based {
                ui.add_space(3.0);
            }
        }
    }

    if next_mode != binding.loop_mode || next_first_1based != current_first_1based {
        set_selected_symbol_playback(app, next_mode, next_first_1based.saturating_sub(1));
    }

    ui.add_space(6.0);
    if ui
        .add_sized(
            Vec2::new(ui.available_width(), 26.0),
            egui::Button::new(format!("{} シンボル編集に入る", icons::SYMBOL)),
        )
        .on_hover_text("ダブルクリックと同様にシンボル内部のタイムライン編集モードへ入ります")
        .clicked()
    {
        enter_symbol_timeline(app, binding);
    }
}

/// 「ライブラリ」タブの描画（シンボルのプレビュー・一覧・名前変更・ステージ配置・編集・削除）
pub(crate) fn draw_library_tab(app: &mut Is2dApp, ui: &mut egui::Ui) {
    let has_sel = !app.canvas.selected_shapes.is_empty();
    if ui
        .add_enabled(
            has_sel,
            egui::Button::new(format!("{} 選択図形をシンボル化 (F8)", icons::ADD))
                .min_size(Vec2::new(ui.available_width(), 28.0)),
        )
        .on_hover_text("キャンバスで選択中の図形を新しいシンボルとして登録 (F8)")
        .clicked()
    {
        convert_selection_to_symbol(app);
    }

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);

    if app.symbols.library.is_empty() {
        ui.label(
            egui::RichText::new(
                "シンボルがありません。\n図形を選択して F8 キーを押すとシンボルを作成できます。",
            )
            .size(11.5)
            .color(Color32::from_rgb(115, 122, 135)),
        );
        return;
    }

    app.symbols.sync_after_restore();
    if app.symbols.selected_library_symbol.is_none() {
        app.symbols.selected_library_symbol = app.symbols.library.first().map(|s| s.id);
    }

    if let Some(sel_id) = app.symbols.selected_library_symbol {
        draw_selected_library_symbol_controls(app, ui, sel_id);
    }

    draw_panel_section_label(
        ui,
        &format!("登録シンボル一覧 ({})", app.symbols.library.len()),
        4.0,
    );
    draw_library_symbol_list(app, ui);
}

/// ライブラリで選択中のシンボルのミニプレビューおよび操作ボタン群（配置・編集・削除）を描画
fn draw_selected_library_symbol_controls(app: &mut Is2dApp, ui: &mut egui::Ui, sel_id: u64) {
    if let Some(sym) = app.symbols.find_symbol(sel_id) {
        draw_symbol_mini_preview(app, ui, sym);
        ui.add_space(6.0);
    }

    let is_editing_self = app
        .symbols
        .edit_stack
        .iter()
        .any(|sess| sess.symbol_id == sel_id);
    let can_place = app.symbols.can_place_symbol_in_current_context(sel_id);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let place_w = (ui.available_width() - 116.0).max(70.0);
        if ui
            .add_enabled(
                can_place,
                egui::Button::new(format!("{} ステージに配置", icons::ADD))
                    .min_size(Vec2::new(place_w, 28.0)),
            )
            .on_hover_text("選択中のシンボルを現在のフレーム・レイヤーに配置します")
            .clicked()
        {
            place_symbol_instance_from_library(app, sel_id);
        }
        if ui
            .add_enabled(
                !is_editing_self,
                egui::Button::new(format!("{} 編集", icons::SYMBOL))
                    .min_size(Vec2::new(54.0, 28.0)),
            )
            .on_hover_text("選択中のシンボルの内部タイムライン編集へ入ります")
            .clicked()
        {
            enter_symbol_from_library(app, sel_id);
        }
        if ui
            .add_sized(
                Vec2::new(54.0, 28.0),
                egui::Button::new(format!("{} 削除", icons::DELETE)),
            )
            .on_hover_text("選択中のシンボルをライブラリから削除します")
            .clicked()
        {
            delete_symbol_from_library(app, sel_id);
        }
    });
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
}

/// ライブラリ登録シンボルのスクロール一覧を描画し、選択切替・名前変更コマンドを発行する
fn draw_library_symbol_list(app: &mut Is2dApp, ui: &mut egui::Ui) {
    let entries: Vec<(u64, String, usize)> = app
        .symbols
        .library
        .iter()
        .map(|s| (s.id, s.name.clone(), s.timeline.total_frames.max(1)))
        .collect();
    let mut rename_req: Option<(u64, String)> = None;
    let mut clear_renaming_id: Option<u64> = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (sym_id, mut name, total_frames) in entries {
                let is_sel = app.symbols.selected_library_symbol == Some(sym_id);
                let row_fill = if is_sel {
                    Color32::from_rgb(220, 234, 252)
                } else {
                    Color32::from_rgb(252, 253, 255)
                };
                let stroke_col = if is_sel {
                    Color32::from_rgb(0, 125, 230)
                } else {
                    Color32::from_rgb(218, 222, 230)
                };
                egui::Frame::none()
                    .fill(row_fill)
                    .stroke(Stroke::new(1.0, stroke_col))
                    .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(
                                    is_sel,
                                    egui::RichText::new(icons::SYMBOL)
                                        .size(15.0)
                                        .color(Color32::from_rgb(0, 130, 220)),
                                )
                                .on_hover_text("クリックして選択")
                                .clicked()
                            {
                                app.symbols.selected_library_symbol = Some(sym_id);
                            }
                            let text_w = (ui.available_width() - 36.0).max(60.0);
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut name).desired_width(text_w),
                            );
                            if resp.clicked() || resp.gained_focus() {
                                app.symbols.selected_library_symbol = Some(sym_id);
                            }
                            if resp.changed() {
                                rename_req = Some((sym_id, name));
                            }
                            if resp.lost_focus() {
                                clear_renaming_id = Some(sym_id);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("{total_frames} f"))
                                            .size(10.5)
                                            .color(Color32::from_rgb(115, 122, 135)),
                                    );
                                },
                            );
                        });
                    });
                ui.add_space(3.0);
            }
        });

    if let Some((sym_id, new_name)) = rename_req {
        if rename_symbol_in_library(app, sym_id, new_name) {
            app.symbols.actively_renaming_symbol = Some(sym_id);
        }
    }
    if let Some(finished_id) = clear_renaming_id {
        if app.symbols.actively_renaming_symbol == Some(finished_id) {
            app.symbols.actively_renaming_symbol = None;
        }
    }
}

/// ライブラリパネル上部にシンボルの縮小ベクタープレビューを描画する
fn draw_symbol_mini_preview(app: &Is2dApp, ui: &mut egui::Ui, sym: &SymbolDef) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 110.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(252, 252, 254));
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_rgb(206, 212, 222)));

    if let Some((mesh, origin_scr)) =
        sym.cached_mini_preview(rect, app.symbols.revision, &app.symbols.library)
    {
        painter.add(arc_mesh_to_shape(mesh));
        if rect.contains(origin_scr) {
            draw_registration_cross(&painter, origin_scr, 4.0, false);
        }
    }
}
