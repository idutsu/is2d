use eframe::egui::{self, Color32, Sense, Stroke, Vec2};

use crate::canvas_tool::{SelectSubMode, Tool};
use crate::FlashBrushApp;

// ============================================================================
// 右側プロパティパネルの状態・タブ定義（Adobe Animate 準拠）
// ============================================================================

/// 右側プロパティパネルの切り替えタブ（Adobe Animate 準拠）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PropertyTab {
    Tool,     // ツール
    Object,   // オブジェクト
    Document, // ドキュメント
}

impl PropertyTab {
    /// プロパティパネル上部に並べるタブの標準順序
    pub(crate) const ALL: [PropertyTab; 3] = [
        PropertyTab::Tool,
        PropertyTab::Object,
        PropertyTab::Document,
    ];

    /// タブ切り替えボタンに表示するラベル名
    pub(crate) const fn label(self) -> &'static str {
        match self {
            PropertyTab::Tool => "ツール",
            PropertyTab::Object => "オブジェクト",
            PropertyTab::Document => "ドキュメント",
        }
    }
}

/// 右側プロパティパネルの状態管理構造体（将来的な折りたたみ状態やサブ設定の保持用）
#[derive(Clone, Debug)]
pub(crate) struct PropertiesPanel {
    pub(crate) active_tab: PropertyTab,
}

impl Default for PropertiesPanel {
    fn default() -> Self {
        Self {
            active_tab: PropertyTab::Document,
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
            .exact_width(42.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(245, 246, 248))
                    .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(210, 214, 222))),
            )
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let btn_size = Vec2::new(32.0, 26.0);

                    // (1) メインツール選択ボタン群（Tool::ALL を自動ループ）
                    self.draw_left_main_tool_buttons(ui, btn_size);

                    ui.separator();

                    // (2) 選択中ツール固有のサブモード・アクションボタン群
                    self.draw_left_tool_sub_actions(ui, btn_size);

                    // (3) グループ操作・アンカー表示切替ボタン群
                    self.draw_left_group_and_view_buttons(ui, btn_size);

                    ui.separator();

                    // (4) 塗りカラー選択スウォッチ＆プリセットパレット
                    self.draw_color_picker_controls(ui);

                    ui.separator();

                    // (5) 履歴操作（Undo / Redo）＆全消去ボタン
                    self.draw_left_history_buttons(ui, btn_size);
                });
            });
    }

    fn draw_left_main_tool_buttons(&mut self, ui: &mut egui::Ui, btn_size: Vec2) {
        for tool in Tool::ALL {
            if ui
                .add_sized(
                    btn_size,
                    egui::SelectableLabel::new(self.tool == tool, tool.icon()),
                )
                .on_hover_text(tool.tooltip())
                .clicked()
            {
                self.set_tool(tool);
            }
        }
    }

    fn draw_left_tool_sub_actions(&mut self, ui: &mut egui::Ui, btn_size: Vec2) {
        match self.tool {
            Tool::Select => {
                if ui
                    .add_sized(
                        btn_size,
                        egui::SelectableLabel::new(
                            self.select_sub_mode == SelectSubMode::AddAnchor,
                            "＋",
                        ),
                    )
                    .on_hover_text("アンカー追加モード (+)")
                    .clicked()
                {
                    self.toggle_select_sub_mode(SelectSubMode::AddAnchor);
                }
                if ui
                    .add_sized(
                        btn_size,
                        egui::SelectableLabel::new(
                            self.select_sub_mode == SelectSubMode::DeleteAnchor,
                            "－",
                        ),
                    )
                    .on_hover_text("アンカー削除モード (-)")
                    .clicked()
                {
                    self.toggle_select_sub_mode(SelectSubMode::DeleteAnchor);
                }
                ui.separator();
            }
            Tool::Transform => {
                let has_sel = !self.selected_shapes.is_empty();
                if ui
                    .add_enabled_ui(has_sel, |ui| {
                        ui.add_sized(btn_size, egui::Button::new("⇄"))
                            .on_hover_text("左右反転")
                    })
                    .inner
                    .clicked()
                {
                    self.flip_selected_shapes(true, false);
                }
                if ui
                    .add_enabled_ui(has_sel, |ui| {
                        ui.add_sized(btn_size, egui::Button::new("⇅"))
                            .on_hover_text("上下反転")
                    })
                    .inner
                    .clicked()
                {
                    self.flip_selected_shapes(false, true);
                }
                if ui
                    .add_enabled_ui(has_sel, |ui| {
                        ui.add_sized(btn_size, egui::Button::new("↺"))
                            .on_hover_text("-90° 回転")
                    })
                    .inner
                    .clicked()
                {
                    self.rotate_selected_shapes(-std::f64::consts::FRAC_PI_2);
                }
                if ui
                    .add_enabled_ui(has_sel, |ui| {
                        ui.add_sized(btn_size, egui::Button::new("↻"))
                            .on_hover_text("+90° 回転")
                    })
                    .inner
                    .clicked()
                {
                    self.rotate_selected_shapes(std::f64::consts::FRAC_PI_2);
                }
                if ui
                    .add_enabled_ui(has_sel && self.transform_pivot.is_some(), |ui| {
                        ui.add_sized(btn_size, egui::Button::new("◎"))
                            .on_hover_text("変形中心（ピボット）をリセット")
                    })
                    .inner
                    .clicked()
                {
                    self.transform_pivot = None;
                }
                ui.separator();
            }
            Tool::Brush | Tool::Eraser | Tool::Bucket | Tool::Eyedropper => {}
        }
    }

    fn draw_left_group_and_view_buttons(&mut self, ui: &mut egui::Ui, btn_size: Vec2) {
        let can_group = !self.selected_shapes.is_empty();
        let can_ungroup = self.selection_has_group();
        if ui
            .add_enabled_ui(can_group, |ui| {
                ui.add_sized(btn_size, egui::Button::new("📦"))
                    .on_hover_text("グループ化 (Cmd+G)")
            })
            .inner
            .clicked()
        {
            self.group_selected_shapes();
        }
        if ui
            .add_enabled_ui(can_ungroup, |ui| {
                ui.add_sized(btn_size, egui::Button::new("💥"))
                    .on_hover_text("グループ解除 (Cmd+B)")
            })
            .inner
            .clicked()
        {
            self.ungroup_selected_shapes();
        }
        if ui
            .add_sized(
                btn_size,
                egui::SelectableLabel::new(self.show_wireframe, "⚓"),
            )
            .on_hover_text("アンカー・パス表示切替")
            .clicked()
        {
            self.show_wireframe = !self.show_wireframe;
        }
    }

    fn draw_color_picker_controls(&mut self, ui: &mut egui::Ui) {
        ui.color_edit_button_srgba(&mut self.brush_color)
            .on_hover_text("塗りカラー変更");

        let presets = [
            Color32::from_rgb(235, 75, 75),
            Color32::from_rgb(55, 135, 245),
            Color32::from_rgb(65, 195, 105),
            Color32::from_rgb(245, 190, 50),
            Color32::from_rgb(160, 95, 235),
            Color32::from_rgb(40, 45, 55),
        ];
        for pair in presets.chunks_exact(2) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for &c in pair {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::click());
                    ui.painter().rect_filled(rect, 0.0, c);
                    if self.brush_color == c {
                        ui.painter().rect_stroke(
                            rect,
                            0.0,
                            Stroke::new(2.0, Color32::from_rgb(20, 25, 35)),
                        );
                    } else {
                        ui.painter().rect_stroke(
                            rect,
                            0.0,
                            Stroke::new(1.0, Color32::from_rgb(175, 180, 190)),
                        );
                    }
                    if resp.clicked() {
                        self.brush_color = c;
                    }
                }
            });
        }
    }

    fn draw_left_history_buttons(&mut self, ui: &mut egui::Ui, btn_size: Vec2) {
        if ui
            .add_enabled_ui(!self.undo_stack.is_empty(), |ui| {
                ui.add_sized(btn_size, egui::Button::new("↩"))
                    .on_hover_text("元に戻す (Cmd+Z)")
            })
            .inner
            .clicked()
        {
            self.undo();
        }
        if ui
            .add_enabled_ui(!self.redo_stack.is_empty(), |ui| {
                ui.add_sized(btn_size, egui::Button::new("↪"))
                    .on_hover_text("やり直す (Cmd+Shift+Z)")
            })
            .inner
            .clicked()
        {
            self.redo();
        }
        if ui
            .add_enabled_ui(!self.shapes.is_empty(), |ui| {
                ui.add_sized(btn_size, egui::Button::new("🗑"))
                    .on_hover_text("現在フレームのキャンバスを全消去")
            })
            .inner
            .clicked()
        {
            self.clear_selection();
            self.push_undo();
            self.shapes.clear();
            self.editing_group_path.clear();
            self.sync_shapes_to_timeline();
        }
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

                    match self.tool {
                        Tool::Brush => {
                            ui.separator();
                            ui.label("Brush Size:");
                            ui.add(
                                egui::Slider::new(&mut self.brush_radius, 1.0..=70.0)
                                    .step_by(1.0)
                                    .integer()
                                    .suffix(" px"),
                            );
                        }
                        Tool::Eraser => {
                            ui.separator();
                            ui.label("Eraser Size:");
                            ui.add(
                                egui::Slider::new(&mut self.eraser_radius, 1.0..=80.0)
                                    .step_by(1.0)
                                    .integer()
                                    .suffix(" px"),
                            );
                        }
                        _ => {}
                    }

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
        let mut crumb = String::from("🎬 シーン 1");
        for &gid in &self.editing_group_path {
            crumb.push_str(&format!(" ▸ 📦 Group #{gid}"));
        }
        if self.editing_group_path.is_empty() {
            ui.label(crumb);
        } else {
            ui.colored_label(Color32::from_rgb(0, 120, 215), crumb);
            if ui.button("⬅ 1階層出る").clicked() {
                self.exit_current_group();
            }
        }
    }

    // ------------------------------------------------------------------------
    // 3. 右端のプロパティパネル（ツール / オブジェクト / ドキュメント の 3 タブ切り替え）
    // ------------------------------------------------------------------------

    pub(crate) fn draw_right_properties_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("right_properties_panel")
            .exact_width(250.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(245, 246, 248))
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(214, 218, 226))),
            )
            .show(ctx, |ui| {
                // 上部パネルヘッダー（「プロパティ」見出し）
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("プロパティ")
                            .size(12.0)
                            .strong()
                            .color(Color32::from_rgb(35, 40, 50)),
                    );
                });
                ui.add_space(6.0);

                // サブタブ切り替えバー（PropertyTab::ALL を自動ループ）
                self.draw_property_tab_bar(ui);

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                // アクティブタブ別の描画メソッドへディスパッチ
                match self.properties_panel.active_tab {
                    PropertyTab::Tool => self.draw_tool_property_tab(ui),
                    PropertyTab::Object => self.draw_object_property_tab(ui),
                    PropertyTab::Document => self.draw_document_property_tab(ui),
                }
            });
    }

    fn draw_property_tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let tab_count = PropertyTab::ALL.len() as f32;
            let btn_w = (ui.available_width() - 4.0 * (tab_count - 1.0)) / tab_count;
            for tab in PropertyTab::ALL {
                let selected = self.properties_panel.active_tab == tab;
                let fill = if selected {
                    Color32::from_rgb(212, 216, 224)
                } else {
                    Color32::from_rgb(236, 238, 242)
                };
                let text_color = if selected {
                    Color32::from_rgb(20, 25, 35)
                } else {
                    Color32::from_rgb(95, 102, 115)
                };
                let btn = egui::Button::new(
                    egui::RichText::new(tab.label())
                        .size(11.5)
                        .color(text_color),
                )
                .fill(fill)
                .stroke(Stroke::new(1.0, Color32::from_rgb(200, 205, 215)))
                .min_size(Vec2::new(btn_w, 24.0));

                if ui.add(btn).clicked() {
                    self.properties_panel.active_tab = tab;
                }
            }
        });
    }

    /// 「ツール」タブの描画（将来ここに各ツール固有の詳細設定を追加）
    fn draw_tool_property_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("▼ ツール設定 (仮)")
                .size(12.0)
                .strong()
                .color(Color32::from_rgb(45, 50, 60)),
        );
        ui.add_space(6.0);
        ui.label(format!(
            "選択中: {} {}",
            self.tool.icon(),
            self.tool.label()
        ));
    }

    /// 「オブジェクト」タブの描画（将来ここに選択シェイプの座標・サイズ・カラー等を追加）
    fn draw_object_property_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("▼ オブジェクト設定 (仮)")
                .size(12.0)
                .strong()
                .color(Color32::from_rgb(45, 50, 60)),
        );
        ui.add_space(6.0);
        ui.label(format!(
            "選択シェイプ数: {} 個",
            self.selected_shapes.len()
        ));
    }

    /// 「ドキュメント」タブの描画（将来ここにステージサイズ・背景色・パブリッシュ設定等を追加）
    fn draw_document_property_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("▼ ドキュメント設定 (仮)")
                .size(12.0)
                .strong()
                .color(Color32::from_rgb(45, 50, 60)),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("FPS:");
            ui.add(
                egui::DragValue::new(&mut self.timeline.fps)
                    .range(1.0..=120.0)
                    .speed(0.5)
                    .fixed_decimals(0),
            );
        });
    }
}
