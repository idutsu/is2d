use eframe::egui::{self, Color32, Stroke, Vec2};

use crate::canvas_tool::{SelectSubMode, Tool};
use crate::FlashBrushApp;

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

/// 右側パネル上部の切り替えタブ（プロパティ / カラー / ライブラリ）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RightPanelTab {
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

/// 右側パネルの状態管理構造体
#[derive(Clone, Debug)]
pub(crate) struct PropertiesPanel {
    pub(crate) active_tab: RightPanelTab,
}

impl Default for PropertiesPanel {
    fn default() -> Self {
        Self {
            active_tab: RightPanelTab::Properties,
        }
    }
}

// ============================================================================
// 左端縦ツールバー・ステージ上部バー・右端プロパティパネルの描画実装
// ============================================================================

impl FlashBrushApp {
    // ------------------------------------------------------------------------
    // 1. 左端縦ツールバー（Adobe Animate 準拠の縦長ツールボックス）
    // ------------------------------------------------------------------------

    pub(crate) fn draw_left_toolbar(&mut self, ctx: &egui::Context) {
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
                    self.draw_left_main_tool_buttons(ui, btn_size);

                    ui.separator();

                    // (2) 塗りカラーピッカーボタン
                    self.draw_color_picker_controls(ui);
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

    fn draw_left_main_tool_buttons(&mut self, ui: &mut egui::Ui, btn_size: Vec2) {
        for tool in Tool::ALL {
            if Self::add_left_selectable_icon(
                ui,
                btn_size,
                self.tool == tool,
                tool.icon(),
                tool.tooltip(),
            ) {
                self.set_tool(tool);
            }
        }
    }

    fn draw_color_picker_controls(&mut self, ui: &mut egui::Ui) {
        ui.color_edit_button_srgba(&mut self.brush_color)
            .on_hover_text("塗りカラー変更");
    }

    // ------------------------------------------------------------------------
    // 2. ステージ上部のスリムバー（シーン／グループ階層パンくず・右側ズーム倍率表示）
    // ------------------------------------------------------------------------

    pub(crate) fn draw_stage_header_bar(&mut self, ctx: &egui::Context) {
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
                    self.draw_group_breadcrumb(ui);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let zoom_pct = (self.view_zoom * 100.0).round() as i32;
                        ui.label(
                            egui::RichText::new(format!("{zoom_pct}%"))
                                .size(11.5)
                                .color(Color32::from_rgb(90, 98, 110)),
                        );
                    });
                });
            });
    }

    fn draw_group_breadcrumb(&mut self, ui: &mut egui::Ui) {
        let mut items: Vec<String> = vec![format!("{} シーン 1", icons::SCENE_MOVIE)];
        for sess in &self.symbol_edit_stack {
            for &gid in &sess.parent_editing_group_path {
                items.push(format!("{} Group #{gid}", icons::GROUP));
            }
            let sym_name = self
                .timeline
                .library
                .iter()
                .find(|s| s.id == sess.symbol_id)
                .map(|s| s.name.as_str())
                .unwrap_or(&sess.symbol_name);
            items.push(format!("{} {sym_name}", icons::SYMBOL));
        }
        for &gid in &self.editing_group_path {
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
            self.exit_current_group();
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
            if !self.exit_current_group() {
                break;
            }
        }
    }

    // ------------------------------------------------------------------------
    // 3. 右端パネル（プロパティ / カラー / ライブラリ / キャンバス の 4 タブ切り替え）
    // ------------------------------------------------------------------------

    pub(crate) fn draw_right_properties_panel(&mut self, ctx: &egui::Context) {
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
                self.draw_right_panel_tab_bar(ui);

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(6.0);

                // アクティブなタブに応じた中身を描画
                match self.properties_panel.active_tab {
                    RightPanelTab::Properties => self.draw_properties_tab(ui),
                    RightPanelTab::Color => self.draw_color_tab(ui),
                    RightPanelTab::Library => self.draw_library_tab(ui),
                    RightPanelTab::Canvas => self.draw_canvas_tab(ui),
                }
            });
    }

    /// 右パネル上部の4タブ切り替えバー（`プロパティ | カラー | ライブラリ | キャンバス`）を描画
    fn draw_right_panel_tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.spacing_mut().button_padding = Vec2::new(3.0, 4.0);
            let tab_count = RightPanelTab::ALL.len() as f32;
            let btn_w = (ui.available_width() - 3.0 * (tab_count - 1.0)) / tab_count;
            for tab in RightPanelTab::ALL {
                let selected = self.properties_panel.active_tab == tab;
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
                    self.properties_panel.active_tab = tab;
                }
            }
        });
    }

    /// 「プロパティ」タブの描画（現在選択中の `self.tool` に応じてツール別の設定メソッドへ振り分け）
    fn draw_properties_tab(&mut self, ui: &mut egui::Ui) {
        // 現在選択中のツール見出し
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(self.tool.icon()).size(16.0));
            ui.label(
                egui::RichText::new(self.tool.label())
                    .size(12.5)
                    .strong()
                    .color(Color32::from_rgb(35, 40, 50)),
            );
        });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        // 選択中のツールごとに専用メソッドへディスパッチ
        match self.tool {
            Tool::Select => self.draw_select_tool_properties(ui),
            Tool::Transform => self.draw_transform_tool_properties(ui),
            Tool::Brush => self.draw_brush_tool_properties(ui),
            Tool::Eraser => self.draw_eraser_tool_properties(ui),
            Tool::Bucket => self.draw_bucket_tool_properties(ui),
            Tool::Eyedropper => self.draw_eyedropper_tool_properties(ui),
        }

        // 下部ぶら下がりセクション：
        // - タイムラインのフレームを明示的に選択したときは「キーフレーム / トゥイーン設定（回転の向き等）」
        // - キャンバス上のシンボルを選択したとき（選択 / 自由変形ツール時）は「シンボル再生オプション」
        if self.inspect_timeline_frame {
            self.draw_selected_frame_properties(ui);
        } else if matches!(self.tool, Tool::Select | Tool::Transform) {
            self.draw_selected_symbol_properties(ui);
        }
    }

    /// 選択ツール (`Tool::Select`) のプロパティ設定項目
    fn draw_select_tool_properties(&mut self, ui: &mut egui::Ui) {
        draw_panel_section_label(ui, "アンカー編集モード", 4.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let half_w = (ui.available_width() - 6.0) * 0.5;
            if ui
                .add_sized(
                    Vec2::new(half_w, 28.0),
                    egui::Button::new(format!("{} アンカー追加", icons::ADD))
                        .selected(self.select_sub_mode == SelectSubMode::AddAnchor),
                )
                .on_hover_text("輪郭上をクリックしてアンカーポイントを追加 (+)")
                .clicked()
            {
                self.toggle_select_sub_mode(SelectSubMode::AddAnchor);
            }
            if ui
                .add_sized(
                    Vec2::new(half_w, 28.0),
                    egui::Button::new(format!("{} アンカー削除", icons::REMOVE))
                        .selected(self.select_sub_mode == SelectSubMode::DeleteAnchor),
                )
                .on_hover_text("アンカーポイントをクリックして削除 (-)")
                .clicked()
            {
                self.toggle_select_sub_mode(SelectSubMode::DeleteAnchor);
            }
        });

        ui.add_space(6.0);

        if ui
            .add_sized(
                Vec2::new(ui.available_width(), 28.0),
                egui::Button::new(format!("{} アンカーを表示", icons::WIREFRAME))
                    .selected(self.show_wireframe),
            )
            .on_hover_text("選択ツール使用中にベクター輪郭のアンカーポイントとパスを表示")
            .clicked()
        {
            self.show_wireframe = !self.show_wireframe;
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        draw_panel_section_label(ui, "シンボル・グループ", 4.0);

        let has_sel = !self.selected_shapes.is_empty();
        if ui
            .add_enabled(
                has_sel,
                egui::Button::new(format!("{} シンボルに変換 (F8)", icons::SYMBOL))
                    .min_size(Vec2::new(ui.available_width(), 28.0)),
            )
            .on_hover_text("選択中の図形をシンボルに変換してライブラリに登録します (F8)")
            .clicked()
        {
            self.convert_selection_to_symbol();
        }
    }

    /// 自由変形ツール (`Tool::Transform`) のプロパティ設定項目
    fn draw_transform_tool_properties(&mut self, ui: &mut egui::Ui) {
        let has_sel = !self.selected_shapes.is_empty();

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
                    self.flip_selected_shapes(true, false);
                }
                if ui
                    .add_sized(
                        Vec2::new(half_w, 28.0),
                        egui::Button::new(format!("{} 上下反転", icons::FLIP_V)),
                    )
                    .on_hover_text("選択シェイプを上下反転")
                    .clicked()
                {
                    self.flip_selected_shapes(false, true);
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
                    self.rotate_selected_shapes(-std::f64::consts::FRAC_PI_2);
                }
                if ui
                    .add_sized(
                        Vec2::new(half_w, 28.0),
                        egui::Button::new(format!("{} +90° 回転", icons::ROTATE_RIGHT)),
                    )
                    .on_hover_text("選択シェイプを時計回りに 90° 回転")
                    .clicked()
                {
                    self.rotate_selected_shapes(std::f64::consts::FRAC_PI_2);
                }
            });
        });

        ui.add_space(6.0);

        if ui
            .add_enabled(
                has_sel && self.has_custom_transform_pivot(),
                egui::Button::new(format!("{} 変形中心（ピボット）をリセット", icons::PIVOT_RESET))
                    .min_size(Vec2::new(ui.available_width(), 28.0)),
            )
            .on_hover_text("移動した変形中心（ピボット）を初期位置（選択中央 / シンボル基準点）へ戻す")
            .clicked()
        {
            self.reset_selected_transform_pivot();
        }
    }

    /// ブラシツール (`Tool::Brush`) のプロパティ設定項目
    fn draw_brush_tool_properties(&mut self, ui: &mut egui::Ui) {
        draw_panel_section_label(ui, "ブラシサイズ", 4.0);
        ui.add(
            egui::Slider::new(&mut self.brush_radius, 1.0..=70.0)
                .step_by(1.0)
                .integer()
                .suffix(" px"),
        );
    }

    /// 消しゴムツール (`Tool::Eraser`) のプロパティ設定項目
    fn draw_eraser_tool_properties(&mut self, ui: &mut egui::Ui) {
        draw_panel_section_label(ui, "消しゴムサイズ", 4.0);
        ui.add(
            egui::Slider::new(&mut self.eraser_radius, 1.0..=80.0)
                .step_by(1.0)
                .integer()
                .suffix(" px"),
        );
    }

    /// バケツ塗りツール (`Tool::Bucket`) のプロパティ設定項目（ここへ順次追加）
    fn draw_bucket_tool_properties(&mut self, ui: &mut egui::Ui) {
        ui.label("バケツ塗りツールのプロパティ (仮)");
    }

    /// スポイトツール (`Tool::Eyedropper`) のプロパティ設定項目（ここへ順次追加）
    fn draw_eyedropper_tool_properties(&mut self, ui: &mut egui::Ui) {
        ui.label("スポイトツールのプロパティ (仮)");
    }

    /// 「カラー」タブの描画（仮）
    fn draw_color_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("▼ カラーパネル (仮)")
                .size(12.0)
                .strong()
                .color(Color32::from_rgb(45, 50, 60)),
        );
    }

    /// 「キャンバス」タブの描画（ステージサイズ・枠外マスク・ウィンドウに合わせる・表示倍率リセット）
    fn draw_canvas_tab(&mut self, ui: &mut egui::Ui) {
        draw_panel_section_label(ui, "キャンバス情報", 6.0);

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("ステージ:")
                    .size(11.5)
                    .color(Color32::from_rgb(70, 78, 90)),
            );
            let mut w = self.stage_size.x.round() as i32;
            let mut h = self.stage_size.y.round() as i32;
            if ui
                .add(
                    egui::DragValue::new(&mut w)
                        .range(100..=3840)
                        .speed(2.0),
                )
                .on_hover_text("ステージの幅 (px)")
                .changed()
            {
                self.stage_size.x = w as f32;
            }
            ui.label("×");
            if ui
                .add(
                    egui::DragValue::new(&mut h)
                        .range(100..=2160)
                        .speed(2.0)
                        .suffix(" px"),
                )
                .on_hover_text("ステージの高さ (px)")
                .changed()
            {
                self.stage_size.y = h as f32;
            }
        });

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        draw_panel_section_label(ui, "キャンバス操作", 6.0);

        if ui
            .add_sized(
                Vec2::new(ui.available_width(), 28.0),
                egui::Button::new("枠外を隠す").selected(self.clip_outside_stage),
            )
            .on_hover_text("ステージ枠の外側にはみ出た図形を隠して完成画面をプレビューします (Cmd+3)")
            .clicked()
        {
            self.clip_outside_stage = !self.clip_outside_stage;
        }

        ui.add_space(6.0);

        if ui
            .add_sized(
                Vec2::new(ui.available_width(), 28.0),
                egui::Button::new("ウィンドウに合わせる"),
            )
            .on_hover_text("ステージ全体が中央キャンバス領域に収まるようにズームと位置を自動調整します (Cmd+2)")
            .clicked()
        {
            self.fit_stage_to_window();
        }

        ui.add_space(6.0);

        if ui
            .add_sized(
                Vec2::new(ui.available_width(), 28.0),
                egui::Button::new("100%表示"),
            )
            .on_hover_text("クリック（または Cmd+1）で 100% 表示・初期位置にリセットします")
            .clicked()
        {
            self.reset_view();
        }
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
