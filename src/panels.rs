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
}

impl RightPanelTab {
    /// 右パネル上部に並べるタブの標準順序
    pub(crate) const ALL: [RightPanelTab; 3] = [
        RightPanelTab::Properties,
        RightPanelTab::Color,
        RightPanelTab::Library,
    ];

    /// タブに表示するラベル名
    pub(crate) const fn label(self) -> &'static str {
        match self {
            RightPanelTab::Properties => "プロパティ",
            RightPanelTab::Color => "カラー",
            RightPanelTab::Library => "ライブラリ",
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
                        if ui
                            .button(format!("{zoom_pct}%"))
                            .on_hover_text(
                                "クリック（または Cmd+0）で 100% 表示にリセット / Space+ドラッグでパン移動",
                            )
                            .clicked()
                        {
                            self.reset_view();
                        }
                    });
                });
            });
    }

    fn draw_group_breadcrumb(&mut self, ui: &mut egui::Ui) {
        let mut crumb = format!("{} シーン 1", icons::SCENE_MOVIE);
        for sess in &self.symbol_edit_stack {
            let sym_name = self
                .timeline
                .library
                .iter()
                .find(|s| s.id == sess.symbol_id)
                .map(|s| s.name.as_str())
                .unwrap_or(&sess.symbol_name);
            crumb.push_str(&format!(" > {} {sym_name}", icons::SYMBOL));
        }
        for &gid in &self.editing_group_path {
            crumb.push_str(&format!(" > {} Group #{gid}", icons::GROUP));
        }
        if self.symbol_edit_stack.is_empty() && self.editing_group_path.is_empty() {
            ui.label(crumb);
        } else {
            ui.colored_label(Color32::from_rgb(0, 120, 215), crumb);
            if ui
                .button(format!("{} 1階層出る", icons::ARROW_BACK))
                .clicked()
            {
                self.exit_current_group();
            }
        }
    }

    // ------------------------------------------------------------------------
    // 3. 右端パネル（プロパティ / カラー / ライブラリ の 3 タブ切り替え）
    // ------------------------------------------------------------------------

    pub(crate) fn draw_right_properties_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("right_properties_panel")
            .exact_width(250.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(245, 246, 248))
                    .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(214, 218, 226))),
            )
            .show(ctx, |ui| {
                // 上部タブバー（プロパティ | カラー | ライブラリ）
                self.draw_right_panel_tab_bar(ui);

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(6.0);

                // アクティブなタブに応じた中身を描画
                match self.properties_panel.active_tab {
                    RightPanelTab::Properties => self.draw_properties_tab(ui),
                    RightPanelTab::Color => self.draw_color_tab(ui),
                    RightPanelTab::Library => self.draw_library_tab(ui),
                }
            });
    }

    /// 右パネル上部の3タブ切り替えバー（`プロパティ | カラー | ライブラリ`）を描画
    fn draw_right_panel_tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let tab_count = RightPanelTab::ALL.len() as f32;
            let btn_w = (ui.available_width() - 4.0 * (tab_count - 1.0)) / tab_count;
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
                    .size(11.5)
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
    }

    /// 選択ツール (`Tool::Select`) のプロパティ設定項目
    fn draw_select_tool_properties(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("アンカー編集モード")
                .size(11.5)
                .strong()
                .color(Color32::from_rgb(55, 62, 74)),
        );
        ui.add_space(4.0);

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

        ui.label(
            egui::RichText::new("シンボル・グループ")
                .size(11.5)
                .strong()
                .color(Color32::from_rgb(55, 62, 74)),
        );
        ui.add_space(4.0);

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

        if let Some((_, sym_name)) = self.selected_single_symbol_info() {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!("{} 選択中: {sym_name}", icons::SYMBOL))
                    .size(11.5)
                    .color(Color32::from_rgb(0, 120, 215)),
            );
        }
    }

    /// 自由変形ツール (`Tool::Transform`) のプロパティ設定項目
    fn draw_transform_tool_properties(&mut self, ui: &mut egui::Ui) {
        let has_sel = !self.selected_shapes.is_empty();

        ui.label(
            egui::RichText::new("変形アクション")
                .size(11.5)
                .strong()
                .color(Color32::from_rgb(55, 62, 74)),
        );
        ui.add_space(4.0);

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
                has_sel && self.transform_pivot.is_some(),
                egui::Button::new(format!("{} 変形中心（ピボット）をリセット", icons::PIVOT_RESET))
                    .min_size(Vec2::new(ui.available_width(), 28.0)),
            )
            .on_hover_text("移動した変形中心（ピボット）を選択範囲の中央へ戻す")
            .clicked()
        {
            self.transform_pivot = None;
        }
    }

    /// ブラシツール (`Tool::Brush`) のプロパティ設定項目
    fn draw_brush_tool_properties(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("ブラシサイズ")
                .size(11.5)
                .strong()
                .color(Color32::from_rgb(55, 62, 74)),
        );
        ui.add_space(4.0);
        ui.add(
            egui::Slider::new(&mut self.brush_radius, 1.0..=70.0)
                .step_by(1.0)
                .integer()
                .suffix(" px"),
        );
    }

    /// 消しゴムツール (`Tool::Eraser`) のプロパティ設定項目
    fn draw_eraser_tool_properties(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("消しゴムサイズ")
                .size(11.5)
                .strong()
                .color(Color32::from_rgb(55, 62, 74)),
        );
        ui.add_space(4.0);
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

    /// 「ライブラリ」タブの描画（シンボルのプレビュー・一覧・名前変更・ステージ配置・削除）
    fn draw_library_tab(&mut self, ui: &mut egui::Ui) {
        let has_sel = !self.selected_shapes.is_empty();
        if ui
            .add_enabled(
                has_sel,
                egui::Button::new(format!("{} 選択図形をシンボル化 (F8)", icons::ADD))
                    .min_size(Vec2::new(ui.available_width(), 28.0)),
            )
            .on_hover_text("キャンバスで選択中の図形を新しいシンボルとして登録 (F8)")
            .clicked()
        {
            self.convert_selection_to_symbol();
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        if self.timeline.library.is_empty() {
            ui.label(
                egui::RichText::new(
                    "シンボルがありません。\n図形を選択して F8 キーを押すとシンボルを作成できます。",
                )
                .size(11.5)
                .color(Color32::from_rgb(115, 122, 135)),
            );
            return;
        }

        if self.timeline.selected_library_symbol.is_none()
            || !self
                .timeline
                .library
                .iter()
                .any(|s| Some(s.id) == self.timeline.selected_library_symbol)
        {
            self.timeline.selected_library_symbol = self.timeline.library.first().map(|s| s.id);
        }

        // 選択中のシンボルのミニプレビュー描画領域
        if let Some(sel_id) = self.timeline.selected_library_symbol {
            if let Some(sym) = self.timeline.library.iter().find(|s| s.id == sel_id) {
                let preview_shapes = sym.evaluated_local_shapes_at(sym.current_frame, false);
                self.draw_symbol_mini_preview(ui, &preview_shapes);
                ui.add_space(6.0);
            }

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let place_w = ui.available_width() - 66.0;
                if ui
                    .add_sized(
                        Vec2::new(place_w.max(80.0), 28.0),
                        egui::Button::new(format!("{} ステージに配置", icons::ADD)),
                    )
                    .on_hover_text("選択中のシンボルを現在のフレーム・レイヤーに配置します")
                    .clicked()
                {
                    self.place_symbol_instance_from_library(sel_id);
                }
                if ui
                    .add_sized(
                        Vec2::new(60.0, 28.0),
                        egui::Button::new(format!("{} 削除", icons::DELETE)),
                    )
                    .on_hover_text("選択中のシンボルをライブラリから削除します")
                    .clicked()
                {
                    self.delete_symbol_from_library(sel_id);
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);
        }

        ui.label(
            egui::RichText::new(format!("登録シンボル一覧 ({})", self.timeline.library.len()))
                .size(11.5)
                .strong()
                .color(Color32::from_rgb(55, 62, 74)),
        );
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for sym in &mut self.timeline.library {
                    let is_sel = self.timeline.selected_library_symbol == Some(sym.id);
                    let row_fill = if is_sel {
                        Color32::from_rgb(220, 234, 252)
                    } else {
                        Color32::from_rgb(252, 253, 255)
                    };
                    egui::Frame::none()
                        .fill(row_fill)
                        .stroke(Stroke::new(
                            1.0,
                            if is_sel {
                                Color32::from_rgb(0, 125, 230)
                            } else {
                                Color32::from_rgb(218, 222, 230)
                            },
                        ))
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
                                    self.timeline.selected_library_symbol = Some(sym.id);
                                }
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut sym.name)
                                        .desired_width(ui.available_width()),
                                );
                                if resp.clicked() || resp.gained_focus() {
                                    self.timeline.selected_library_symbol = Some(sym.id);
                                }
                            });
                        });
                    ui.add_space(3.0);
                }
            });
    }

    /// ライブラリパネル上部にシンボルの縮小ベクタープレビューを描画する
    fn draw_symbol_mini_preview(&self, ui: &mut egui::Ui, shapes: &[crate::vector_engine::FillShape]) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 110.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(252, 252, 254));
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_rgb(206, 212, 222)));

        if shapes.is_empty() {
            return;
        }
        let mut bbox = shapes[0].bounding_box();
        for s in &shapes[1..] {
            bbox = bbox.union(s.bounding_box());
        }
        let w = (bbox.x1 - bbox.x0).max(1.0) as f32;
        let h = (bbox.y1 - bbox.y0).max(1.0) as f32;
        let pad = 14.0;
        let avail_w = (rect.width() - pad * 2.0).max(10.0);
        let avail_h = (rect.height() - pad * 2.0).max(10.0);
        let scale = (avail_w / w).min(avail_h / h).clamp(0.05, 4.0);
        let cx = ((bbox.x0 + bbox.x1) * 0.5) as f32;
        let cy = ((bbox.y0 + bbox.y1) * 0.5) as f32;
        let pan = rect.center().to_vec2() - Vec2::new(cx * scale, cy * scale);

        for s in shapes {
            let mesh =
                crate::vector_engine::transform_mesh_to_screen(&s.fill_mesh, scale, pan, false);
            painter.add(egui::Shape::mesh(mesh));
        }

        // 中心基準点 `+` マーク
        let origin_scr = egui::Pos2::new(pan.x, pan.y);
        if rect.contains(origin_scr) {
            crate::symbol::draw_registration_cross(&painter, origin_scr, 4.0, false);
        }
    }
}
