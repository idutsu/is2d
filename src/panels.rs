use eframe::egui::{self, Color32, CursorIcon, Pos2, Stroke, Vec2};
use kurbo::{Point, Rect as KRect};

use crate::canvas_tool::{
    ActiveCanvasDrag, Clipboard, CoalesceKey, NoticeKind, SelectDragState, SelectSubMode, Tab,
    Tool, ToolSettings, TransformDragState, active_scope_layer_id, draw_registration_cross,
};
use crate::project_io::PendingCloseConfirm;
use crate::symbol::{SymbolDef, SymbolInstanceBinding, SymbolLoopMode, TweenRotate};
use crate::symbol_ops::selected_single_symbol_info;
pub(crate) use crate::timeline_ui::icons;
use crate::timeline_ui::TimelineUiState;
use crate::vector_engine::arc_mesh_to_shape;

/// OS標準の修飾キー表記（macOS では `"Cmd"`、Windows / Linux では `"Ctrl"`）
pub(crate) const MOD_KEY: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

/// ステータスメッセージの種類に応じた配色（背景色・枠線色・テキスト色）を返す（Layer 4 プレゼンテーション定義）
pub(crate) fn notice_kind_colors(kind: NoticeKind) -> (Color32, Color32, Color32) {
    match kind {
        NoticeKind::Warning => (
            Color32::from_rgba_unmultiplied(255, 245, 232, 240),
            Color32::from_rgb(235, 165, 95),
            Color32::from_rgb(175, 75, 25),
        ),
        NoticeKind::Error => (
            Color32::from_rgba_unmultiplied(255, 236, 236, 244),
            Color32::from_rgb(230, 95, 95),
            Color32::from_rgb(185, 35, 35),
        ),
        NoticeKind::Info => (
            Color32::from_rgba_unmultiplied(235, 246, 255, 242),
            Color32::from_rgb(95, 165, 235),
            Color32::from_rgb(20, 85, 165),
        ),
    }
}

/// 右パネルの4タブ構成（Layer 4 UI専用状態）
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum RightPanelTab {
    /// 現在のツール・選択中フレーム・選択中シンボルの詳細設定タブ
    #[default]
    Properties,
    /// 塗りカラー・プリセットパレット選択タブ
    Color,
    /// シンボル一覧・プレビュー・配置・編集タブ
    Library,
    /// ステージサイズ・クリッピング・ズーム操作タブ
    Canvas,
}

impl RightPanelTab {
    pub(crate) const ALL: [Self; 4] = [Self::Properties, Self::Color, Self::Library, Self::Canvas];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Properties => "プロパティ",
            Self::Color => "カラー",
            Self::Library => "ライブラリ",
            Self::Canvas => "キャンバス",
        }
    }
}

/// ツール種別に対応するアイコン文字（Layer 4 プレゼンテーション定義）
pub(crate) const fn tool_icon(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "\u{e923}",
        Tool::Transform => "\u{e5c3}",
        Tool::Brush => "\u{e3ae}",
        Tool::Eraser => "\u{e6d0}",
        Tool::Bucket => "\u{e23a}",
        Tool::Eyedropper => "\u{e3b8}",
    }
}

/// ツール種別に対応する日本語見出し名（Layer 4 プレゼンテーション定義）
pub(crate) const fn tool_label(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "選択ツール",
        Tool::Transform => "自由変形ツール",
        Tool::Brush => "ブラシツール",
        Tool::Eraser => "消しゴムツール",
        Tool::Bucket => "バケツ塗りツール",
        Tool::Eyedropper => "スポイトツール",
    }
}

/// ツール種別に対応する左ツールバー用ツールチップ文言（Layer 4 プレゼンテーション定義）
pub(crate) const fn tool_tooltip(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => {
            "選択ツール (V) - クリックで選択 / 範囲スライス選択 / 輪郭ドラッグで粘土変形"
        }
        Tool::Transform => {
            "自由変形ツール (Q) - バウンディングボックスで拡大縮小・回転・基準点移動"
        }
        Tool::Brush => "ブラシツール (B) - ベクター塗りストロークを描画",
        Tool::Eraser => "消しゴムツール (E) - ベクターシェイプを削る・分割する",
        Tool::Bucket => "バケツ塗りツール (K) - 閉じた隙間領域の塗りつぶし / 既存塗りの色変更",
        Tool::Eyedropper => "スポイトツール (I) - クリックした図形の色を抽出してブラシへ設定",
    }
}

// ============================================================================
// UIコマンドアクション（パネル描画中に収集し、アプリケーション層で一括ディスパッチする）
// ============================================================================

/// UIパネル（メニュー・ツールバー・タブバー・プロパティ・ライブラリ・確認モーダル）が発行するコマンドアクション
#[derive(Clone, Debug)]
pub(crate) enum UiAction {
    // ファイル・ドキュメントタブ操作
    NewProject,
    OpenProjectDialog,
    SaveProject,
    SaveProjectAsDialog,
    CloseActiveTab,
    CloseTab(usize),
    SwitchToTab(usize),
    ConfirmPendingCloseSave,
    ConfirmPendingCloseDiscard,
    ConfirmPendingCloseCancel,

    // 編集・履歴・クリップボード操作
    Undo,
    Redo,
    CutSelected,
    CopySelected,
    PasteClipboard {
        in_place: bool,
    },
    DeleteSelected,
    ClearActiveFrame,

    // ツール・表示・ステージ設定操作
    SetTool(Tool),
    ToggleSelectSubMode(SelectSubMode),
    ResetView,
    FitStageToWindow,
    SetStageSize(Vec2),
    FinishStageSizeEdit,

    // グループ・階層・変形・シンボル操作
    ExitGroupSteps(usize),
    GroupSelected,
    UngroupSelected,
    FlipSelected {
        flip_x: bool,
        flip_y: bool,
    },
    RotateSelected(f64),
    ResetTransformPivot,
    ConvertSelectionToSymbol,
    SetTweenRotate(TweenRotate),
    SetSymbolPlayback {
        loop_mode: SymbolLoopMode,
        first_frame: usize,
        drag_active: bool,
    },
    EnterSymbolTimeline(SymbolInstanceBinding),
    PlaceSymbolFromLibrary(u64),
    EnterSymbolFromLibrary(u64),
    DeleteSymbolFromLibrary(u64),
    SelectLibrarySymbol(u64),
    RenameSymbolInLibrary {
        sym_id: u64,
        new_name: String,
    },
    FinishRenamingSymbol(u64),
}

// ============================================================================
// 最上部メニューバー・左端縦ツールバー・ステージ上部バー・右端パネルの描画実装
// ============================================================================

// ------------------------------------------------------------------------
// 0. 最上部メニューバー（ファイル・編集・表示）
// ------------------------------------------------------------------------

pub(crate) fn draw_top_menu_bar(
    tab: &mut Tab,
    clipboard: &Clipboard,
    actions: &mut Vec<UiAction>,
    ctx: &egui::Context,
) {
    egui::TopBottomPanel::top("top_menu_bar").show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            draw_file_menu(actions, ui);
            draw_edit_menu(tab, clipboard, actions, ui);
            draw_view_menu(tab, actions, ui);
        });
    });
}

fn menu_action_button(ui: &mut egui::Ui, enabled: bool, label: &str, shortcut: &str) -> bool {
    let btn = if shortcut.is_empty() {
        egui::Button::new(label)
    } else {
        egui::Button::new(label).shortcut_text(shortcut)
    };
    let clicked = ui.add_enabled(enabled, btn).clicked();
    if clicked {
        ui.close_menu();
    }
    clicked
}

fn draw_file_menu(actions: &mut Vec<UiAction>, ui: &mut egui::Ui) {
    ui.menu_button("ファイル", |ui| {
        ui.set_min_width(210.0);
        if menu_action_button(ui, true, "新規作成", &format!("{MOD_KEY}+N")) {
            actions.push(UiAction::NewProject);
        }

        ui.separator();

        if menu_action_button(ui, true, "開く...", &format!("{MOD_KEY}+O")) {
            actions.push(UiAction::OpenProjectDialog);
        }
        if menu_action_button(ui, true, "保存", &format!("{MOD_KEY}+S")) {
            actions.push(UiAction::SaveProject);
        }
        if menu_action_button(
            ui,
            true,
            "名前を付けて保存...",
            &format!("{MOD_KEY}+Shift+S"),
        ) {
            actions.push(UiAction::SaveProjectAsDialog);
        }

        ui.separator();

        if menu_action_button(ui, true, "タブを閉じる", &format!("{MOD_KEY}+W")) {
            actions.push(UiAction::CloseActiveTab);
        }
    });
}

fn draw_edit_menu(
    tab: &Tab,
    clipboard: &Clipboard,
    actions: &mut Vec<UiAction>,
    ui: &mut egui::Ui,
) {
    ui.menu_button("編集", |ui| {
        ui.set_min_width(230.0);
        let can_undo = !tab.history.undo_stack.is_empty();
        let can_redo = !tab.history.redo_stack.is_empty();
        let has_selection = !tab.session.selected_shapes.is_empty();
        let can_paste = !clipboard.shapes.is_empty()
            && tab
                .doc
                .root
                .is_layer_editable_by_id(active_scope_layer_id(&tab.session, &tab.doc.root));

        if menu_action_button(ui, can_undo, "元に戻す", &format!("{MOD_KEY}+Z")) {
            actions.push(UiAction::Undo);
        }
        if menu_action_button(ui, can_redo, "やり直し", &format!("{MOD_KEY}+Y")) {
            actions.push(UiAction::Redo);
        }

        ui.separator();

        if menu_action_button(ui, has_selection, "カット", &format!("{MOD_KEY}+X")) {
            actions.push(UiAction::CutSelected);
        }
        if menu_action_button(ui, has_selection, "コピー", &format!("{MOD_KEY}+C")) {
            actions.push(UiAction::CopySelected);
        }
        if menu_action_button(ui, can_paste, "ペースト", &format!("{MOD_KEY}+V")) {
            actions.push(UiAction::PasteClipboard { in_place: false });
        }
        if menu_action_button(
            ui,
            can_paste,
            "同じ位置にペースト",
            &format!("{MOD_KEY}+Shift+V"),
        ) {
            actions.push(UiAction::PasteClipboard { in_place: true });
        }

        ui.separator();

        if menu_action_button(ui, has_selection, "選択削除", "Backspace") {
            actions.push(UiAction::DeleteSelected);
        }
    });
}

fn draw_view_menu(tab: &mut Tab, actions: &mut Vec<UiAction>, ui: &mut egui::Ui) {
    ui.menu_button("表示", |ui| {
        ui.set_min_width(220.0);
        if menu_action_button(ui, true, "100%表示", &format!("{MOD_KEY}+1")) {
            actions.push(UiAction::ResetView);
        }
        if menu_action_button(ui, true, "ウィンドウに合わせる", &format!("{MOD_KEY}+2")) {
            actions.push(UiAction::FitStageToWindow);
        }

        ui.separator();

        if ui
            .checkbox(
                &mut tab.view.clip_outside_stage,
                format!("枠外を隠す ({MOD_KEY}+3)"),
            )
            .clicked()
        {
            ui.close_menu();
        }
        if ui
            .checkbox(&mut tab.view.show_wireframe, "アンカーを表示")
            .clicked()
        {
            ui.close_menu();
        }
        if ui
            .checkbox(&mut tab.view.onion_skin_enabled, "オニオンスキン (O)")
            .clicked()
        {
            ui.close_menu();
        }
    });
}

// ------------------------------------------------------------------------
// 1. 左端縦ツールバー（Adobe Animate 準拠の縦長ツールボックス）
// ------------------------------------------------------------------------

pub(crate) fn draw_left_toolbar(
    tool: &mut ToolSettings,
    actions: &mut Vec<UiAction>,
    ctx: &egui::Context,
) {
    egui::SidePanel::left("left_vertical_toolbar")
        .resizable(false)
        .exact_width(44.0)
        .frame(
            egui::Frame::none()
                .fill(Color32::from_rgb(245, 246, 248))
                .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(210, 214, 222))),
        )
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let btn_size = Vec2::new(34.0, 30.0);

                // (1) メインツール選択ボタン群（Tool::ALL を自動ループ）
                draw_left_main_tool_buttons(tool, actions, ui, btn_size);

                ui.separator();

                // (2) 塗りカラーピッカーボタン
                draw_color_picker_controls(tool, ui);
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

fn draw_left_main_tool_buttons(
    tool_settings: &ToolSettings,
    actions: &mut Vec<UiAction>,
    ui: &mut egui::Ui,
    btn_size: Vec2,
) {
    for tool in Tool::ALL {
        if add_left_selectable_icon(
            ui,
            btn_size,
            tool_settings.tool == tool,
            tool_icon(tool),
            tool_tooltip(tool),
        ) {
            actions.push(UiAction::SetTool(tool));
        }
    }
}

fn draw_color_picker_controls(tool: &mut ToolSettings, ui: &mut egui::Ui) {
    ui.color_edit_button_srgba(&mut tool.brush_color)
        .on_hover_text("塗りカラー変更");
}

// ------------------------------------------------------------------------
// 1.5 ドキュメントタブバー（複数ファイルの同時オープン・切り替え・閉じる・新規追加）
// ------------------------------------------------------------------------

pub(crate) fn draw_document_tab_bar(
    tabs: &[Tab],
    active_idx: usize,
    actions: &mut Vec<UiAction>,
    ctx: &egui::Context,
) {
    egui::TopBottomPanel::top("document_tab_bar").show(ctx, |ui| {
        egui::ScrollArea::horizontal()
            .id_salt("document_tab_scroll")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    let mut switch_target = None;
                    let mut close_target = None;

                    for (idx, tab) in tabs.iter().enumerate() {
                        let is_active = idx == active_idx;
                        let title = tab.display_title();
                        let bg_fill = if is_active {
                            Color32::from_rgb(255, 255, 255)
                        } else {
                            Color32::from_rgb(232, 235, 240)
                        };
                        let border_color = if is_active {
                            Color32::from_rgb(0, 120, 215)
                        } else {
                            Color32::from_rgb(205, 210, 218)
                        };

                        egui::Frame::none()
                            .fill(bg_fill)
                            .stroke(Stroke::new(1.0_f32, border_color))
                            .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    let text = egui::RichText::new(title).color(if is_active {
                                        Color32::from_rgb(15, 25, 40)
                                    } else {
                                        Color32::from_rgb(90, 98, 110)
                                    });
                                    let btn_resp = ui.add(egui::Button::new(text).frame(false));
                                    if btn_resp.clicked() {
                                        switch_target = Some(idx);
                                    }
                                    btn_resp.on_hover_ui(|ui| match tab.file.as_deref() {
                                        Some(p) => {
                                            ui.label(format!(
                                                "{}\nクリックしてこのドキュメントへ切り替え",
                                                p.display()
                                            ));
                                        }
                                        None => {
                                            ui.label(
                                                "未保存ドキュメント\nクリックしてこのドキュメントへ切り替え",
                                            );
                                        }
                                    });
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("×")
                                                    .size(12.0)
                                                    .color(Color32::from_rgb(120, 128, 140)),
                                            )
                                            .frame(false),
                                        )
                                        .on_hover_text(format!("タブを閉じる ({MOD_KEY}+W)"))
                                        .clicked()
                                    {
                                        close_target = Some(idx);
                                    }
                                });
                            });
                    }

                    if ui
                        .button(icons::ADD)
                        .on_hover_text(format!("新規ドキュメントタブを追加 ({MOD_KEY}+N)"))
                        .clicked()
                    {
                        actions.push(UiAction::NewProject);
                    }

                    if let Some(close_idx) = close_target {
                        actions.push(UiAction::CloseTab(close_idx));
                    } else if let Some(target_idx) = switch_target {
                        actions.push(UiAction::SwitchToTab(target_idx));
                    }
                });
            });
    });
}

// ------------------------------------------------------------------------
// 2. ステージ上部のスリムバー（シーン／グループ階層パンくず・右側ズーム倍率表示）
// ------------------------------------------------------------------------

pub(crate) fn draw_stage_header_bar(
    tab: &Tab,
    actions: &mut Vec<UiAction>,
    ctx: &egui::Context,
) {
    egui::TopBottomPanel::top("stage_header_bar")
        .exact_height(30.0)
        .frame(
            egui::Frame::none()
                .fill(Color32::from_rgb(245, 246, 248))
                .inner_margin(egui::Margin::symmetric(10.0, 0.0))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(214, 218, 226))),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                draw_group_breadcrumb(tab, actions, ui);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let zoom_pct = (tab.view.zoom * 100.0).round() as i32;
                    ui.label(
                        egui::RichText::new(format!("{zoom_pct}%"))
                            .size(11.5)
                            .color(Color32::from_rgb(90, 98, 110)),
                    );
                });
            });
        });
}

fn draw_group_breadcrumb(tab: &Tab, actions: &mut Vec<UiAction>, ui: &mut egui::Ui) {
    let mut items: Vec<String> = vec![format!("{} シーン 1", icons::SCENE_MOVIE)];
    for sess in &tab.session.edit_stack {
        for &gid in &sess.parent_editing_group_path {
            items.push(format!("{} Group #{gid}", icons::GROUP));
        }
        let sym_name = tab
            .doc
            .library
            .iter()
            .find(|s| s.id == sess.symbol_id)
            .map(|s| s.name.as_str())
            .unwrap_or("Symbol");
        items.push(format!("{} {sym_name}", icons::SYMBOL));
    }
    for &gid in &tab.session.editing_group_path {
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
        actions.push(UiAction::ExitGroupSteps(1));
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

    if exit_steps > 0 {
        actions.push(UiAction::ExitGroupSteps(exit_steps));
    }
}

// ------------------------------------------------------------------------
// 3. 右端パネル（プロパティ / カラー / ライブラリ / キャンバス の 4 タブ切り替え）
// ------------------------------------------------------------------------

pub(crate) fn draw_right_properties_panel(
    tab: &mut Tab,
    tool: &mut ToolSettings,
    right_panel_tab: &mut RightPanelTab,
    actions: &mut Vec<UiAction>,
    ctx: &egui::Context,
) {
    egui::SidePanel::right("right_properties_panel")
        .exact_width(280.0)
        .resizable(false)
        .frame(
            egui::Frame::none()
                .fill(Color32::from_rgb(245, 246, 248))
                .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(214, 218, 226))),
        )
        .show(ctx, |ui| {
            // 上部タブバー（プロパティ | カラー | ライブラリ | キャンバス）
            draw_right_panel_tab_bar(right_panel_tab, ui);

            ui.add_space(4.0);
            ui.separator();
            ui.add_space(6.0);

            // アクティブなタブに応じた中身を描画
            match *right_panel_tab {
                RightPanelTab::Properties => draw_properties_tab(tab, tool, actions, ui),
                RightPanelTab::Color => draw_color_palette_section(tool, ui),
                RightPanelTab::Library => draw_library_tab(tab, actions, ui),
                RightPanelTab::Canvas => draw_canvas_tab(tab, actions, ui),
            }
        });
}

/// 右パネル上部の4タブ切り替えバー（`プロパティ | カラー | ライブラリ | キャンバス`）を描画
fn draw_right_panel_tab_bar(right_panel_tab: &mut RightPanelTab, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.spacing_mut().button_padding = Vec2::new(3.0, 4.0);
        let tab_count = RightPanelTab::ALL.len() as f32;
        let btn_w = (ui.available_width() - 3.0 * (tab_count - 1.0)) / tab_count;
        for tab in RightPanelTab::ALL {
            let selected = *right_panel_tab == tab;
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
                    1.0_f32,
                    if selected {
                        Color32::from_rgb(190, 196, 208)
                    } else {
                        Color32::from_rgb(216, 220, 228)
                    },
                ))
                .min_size(Vec2::new(btn_w, 24.0));

            if ui.add(btn).clicked() {
                *right_panel_tab = tab;
            }
        }
    });
}

/// 「プロパティ」タブの描画（現在選択中の `tool.tool` に応じてツール別の設定メソッドへ振り分け）
fn draw_properties_tab(
    tab: &mut Tab,
    tool: &mut ToolSettings,
    actions: &mut Vec<UiAction>,
    ui: &mut egui::Ui,
) {
    // 現在選択中のツール見出し
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tool_icon(tool.tool)).size(16.0));
        ui.label(
            egui::RichText::new(tool_label(tool.tool))
                .size(12.5)
                .strong()
                .color(Color32::from_rgb(35, 40, 50)),
        );
    });
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);

    // 選択中のツールごとに専用メソッドへディスパッチ
    match tool.tool {
        Tool::Select => draw_select_tool_properties(tab, tool, actions, ui),
        Tool::Transform => draw_transform_tool_properties(tab, actions, ui),
        Tool::Brush => {
            draw_radius_slider_section(ui, "ブラシサイズ", &mut tool.brush_radius, 0.5, 70.0, 0.5)
        }
        Tool::Eraser => draw_radius_slider_section(
            ui,
            "消しゴムサイズ",
            &mut tool.eraser_radius,
            1.0,
            80.0,
            1.0,
        ),
        Tool::Bucket | Tool::Eyedropper => draw_color_palette_section(tool, ui),
    }

    // 下部ぶら下がりセクション：
    // - タイムラインのフレームを明示的に選択したときは「キーフレーム / トゥイーン設定（回転の向き等）」
    // - キャンバス上のシンボルを選択したとき（選択 / 自由変形ツール時）は「シンボル再生オプション」
    if tab.view.inspect_timeline_frame {
        draw_selected_frame_properties(tab, actions, ui);
    }
    if matches!(tool.tool, Tool::Select | Tool::Transform) {
        draw_selected_symbol_properties(tab, actions, ui);
    }
}

/// 選択ツール (`Tool::Select`) のプロパティ設定項目
fn draw_select_tool_properties(
    tab: &mut Tab,
    tool: &ToolSettings,
    actions: &mut Vec<UiAction>,
    ui: &mut egui::Ui,
) {
    draw_panel_section_label(ui, "アンカー編集モード", 4.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let half_w = (ui.available_width() - 6.0) * 0.5;
        if ui
            .add_sized(
                Vec2::new(half_w, 28.0),
                egui::Button::new(format!("{} アンカー追加", icons::ADD))
                    .selected(tool.select_sub_mode == SelectSubMode::AddAnchor),
            )
            .on_hover_text("輪郭上をクリックしてアンカーポイントを追加 (+)")
            .clicked()
        {
            actions.push(UiAction::ToggleSelectSubMode(SelectSubMode::AddAnchor));
        }
        if ui
            .add_sized(
                Vec2::new(half_w, 28.0),
                egui::Button::new(format!("{} アンカー削除", icons::REMOVE))
                    .selected(tool.select_sub_mode == SelectSubMode::DeleteAnchor),
            )
            .on_hover_text("アンカーポイントをクリックして削除 (-)")
            .clicked()
        {
            actions.push(UiAction::ToggleSelectSubMode(SelectSubMode::DeleteAnchor));
        }
    });

    ui.add_space(6.0);

    if ui
        .add_sized(
            Vec2::new(ui.available_width(), 28.0),
            egui::Button::new(format!("{} アンカーを表示", icons::WIREFRAME))
                .selected(tab.view.show_wireframe),
        )
        .on_hover_text("選択ツール使用中にベクター輪郭のアンカーポイントとパスを表示")
        .clicked()
    {
        tab.view.show_wireframe = !tab.view.show_wireframe;
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    draw_symbol_and_group_section(tab, actions, ui);

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    draw_panel_section_label(ui, "消去アクション", 4.0);

    let has_sel = !tab.session.selected_shapes.is_empty();
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
            actions.push(UiAction::DeleteSelected);
        }
        let has_any = tab
            .session
            .shapes
            .iter()
            .any(|s| s.is_in_editing_scope(&tab.session.editing_group_path));
        if ui
            .add_enabled(
                has_any,
                egui::Button::new("フレーム全消去").min_size(Vec2::new(half_w, 28.0)),
            )
            .on_hover_text("現在の編集スコープ・フレーム内の図形をすべて消去")
            .clicked()
        {
            actions.push(UiAction::ClearActiveFrame);
        }
    });
}

/// 選択・自由変形ツール共通の「シンボル・グループ」操作UIセクション
fn draw_symbol_and_group_section(tab: &Tab, actions: &mut Vec<UiAction>, ui: &mut egui::Ui) {
    draw_panel_section_label(ui, "シンボル・グループ", 4.0);

    let has_sel = !tab.session.selected_shapes.is_empty();
    let has_group = tab.session.selection_has_group();
    if ui
        .add_enabled(
            has_sel,
            egui::Button::new(format!("{} シンボルに変換 (F8)", icons::SYMBOL))
                .min_size(Vec2::new(ui.available_width(), 28.0)),
        )
        .on_hover_text("選択中の図形をシンボルに変換してライブラリに登録します (F8)")
        .clicked()
    {
        actions.push(UiAction::ConvertSelectionToSymbol);
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
            actions.push(UiAction::GroupSelected);
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
            actions.push(UiAction::UngroupSelected);
        }
    });
}

/// 自由変形ツール (`Tool::Transform`) のプロパティ設定項目
fn draw_transform_tool_properties(
    tab: &Tab,
    actions: &mut Vec<UiAction>,
    ui: &mut egui::Ui,
) {
    let has_sel = !tab.session.selected_shapes.is_empty();

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
                actions.push(UiAction::FlipSelected {
                    flip_x: true,
                    flip_y: false,
                });
            }
            if ui
                .add_sized(
                    Vec2::new(half_w, 28.0),
                    egui::Button::new(format!("{} 上下反転", icons::FLIP_V)),
                )
                .on_hover_text("選択シェイプを上下反転")
                .clicked()
            {
                actions.push(UiAction::FlipSelected {
                    flip_x: false,
                    flip_y: true,
                });
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
                actions.push(UiAction::RotateSelected(-std::f64::consts::FRAC_PI_2));
            }
            if ui
                .add_sized(
                    Vec2::new(half_w, 28.0),
                    egui::Button::new(format!("{} +90° 回転", icons::ROTATE_RIGHT)),
                )
                .on_hover_text("選択シェイプを時計回りに 90° 回転")
                .clicked()
            {
                actions.push(UiAction::RotateSelected(std::f64::consts::FRAC_PI_2));
            }
        });
    });

    ui.add_space(6.0);

    if ui
        .add_enabled(
            has_sel && tab.session.has_custom_transform_pivot(),
            egui::Button::new(format!(
                "{} 変形中心（ピボット）をリセット",
                icons::PIVOT_RESET
            ))
            .min_size(Vec2::new(ui.available_width(), 28.0)),
        )
        .on_hover_text("移動した変形中心（ピボット）を初期位置（選択中央 / シンボル基準点）へ戻す")
        .clicked()
    {
        actions.push(UiAction::ResetTransformPivot);
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    draw_symbol_and_group_section(tab, actions, ui);
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
fn draw_color_palette_section(tool: &mut ToolSettings, ui: &mut egui::Ui) {
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
        ui.color_edit_button_srgba(&mut tool.brush_color);
        let [r, g, b, _] = tool.brush_color.to_srgba_unmultiplied();
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
                let is_active = tool.brush_color == col;
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(swatch_w, 24.0), egui::Sense::click());
                let painter = ui.painter();
                painter.rect_filled(rect, 0.0, col);
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(
                        if is_active { 2.0_f32 } else { 1.0_f32 },
                        if is_active {
                            Color32::from_rgb(0, 125, 230)
                        } else {
                            Color32::from_rgb(185, 192, 204)
                        },
                    ),
                );
                if resp.clicked() {
                    tool.brush_color = col;
                }
            }
        });
        ui.add_space(4.0);
    }
}

/// 「キャンバス」タブの描画（ステージサイズ・枠外マスク・ウィンドウに合わせる・表示倍率リセット）
fn draw_canvas_tab(tab: &mut Tab, actions: &mut Vec<UiAction>, ui: &mut egui::Ui) {
    draw_panel_section_label(ui, "キャンバス情報", 6.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("ステージ:")
                .size(11.5)
                .color(Color32::from_rgb(70, 78, 90)),
        );
        let mut w = tab.doc.stage_size.x.round() as i32;
        let mut h = tab.doc.stage_size.y.round() as i32;
        let w_resp = ui
            .add(
                egui::DragValue::new(&mut w)
                    .range(100..=3840)
                    .speed(2.0)
                    .update_while_editing(false),
            )
            .on_hover_text("ステージの幅 (px)");
        ui.label("×");
        let h_resp = ui
            .add(
                egui::DragValue::new(&mut h)
                    .range(100..=2160)
                    .speed(2.0)
                    .suffix(" px")
                    .update_while_editing(false),
            )
            .on_hover_text("ステージの高さ (px)");
        let next_size = Vec2::new(
            if w_resp.changed() {
                w as f32
            } else {
                tab.doc.stage_size.x
            },
            if h_resp.changed() {
                h as f32
            } else {
                tab.doc.stage_size.y
            },
        );
        if (w_resp.changed() || h_resp.changed()) && next_size != tab.doc.stage_size {
            actions.push(UiAction::SetStageSize(next_size));
        }
        if tab.history.last_coalesce_key == Some(CoalesceKey::StageSize)
            && (w_resp.drag_stopped()
                || w_resp.lost_focus()
                || h_resp.drag_stopped()
                || h_resp.lost_focus()
                || (!w_resp.dragged()
                    && !w_resp.has_focus()
                    && !h_resp.dragged()
                    && !h_resp.has_focus()))
        {
            actions.push(UiAction::FinishStageSizeEdit);
        }
    });

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    draw_panel_section_label(ui, "キャンバス操作", 6.0);

    if ui
        .add_sized(
            Vec2::new(ui.available_width(), 28.0),
            egui::Button::new("枠外を隠す").selected(tab.view.clip_outside_stage),
        )
        .on_hover_text(format!(
            "ステージ枠の外側にはみ出た図形を隠して完成画面をプレビューします ({MOD_KEY}+3)"
        ))
        .clicked()
    {
        tab.view.clip_outside_stage = !tab.view.clip_outside_stage;
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
        actions.push(UiAction::FitStageToWindow);
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
        actions.push(UiAction::ResetView);
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
pub(crate) fn draw_selected_frame_properties(
    tab: &Tab,
    actions: &mut Vec<UiAction>,
    ui: &mut egui::Ui,
) {
    let Some((kf_start_frame, current_rotate, is_complete)) =
        tab.doc.root.active_tween_keyframe_info()
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
            egui::Button::new("自動（最短角度）").selected(current_rotate == TweenRotate::Auto),
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
        actions.push(UiAction::SetTweenRotate(next_rotate));
    }
}

/// キャンバス上で単一のシンボルインスタンスを選択しているときにプロパティパネル下部に表示する再生オプション（1回再生 / ループ再生 / 任意のフレーム）
pub(crate) fn draw_selected_symbol_properties(
    tab: &mut Tab,
    actions: &mut Vec<UiAction>,
    ui: &mut egui::Ui,
) {
    let Some((binding, sym_name, sym_total_frames)) = selected_single_symbol_info(tab) else {
        if matches!(
            tab.history.last_coalesce_key,
            Some(CoalesceKey::SymbolPlayback(_))
        ) {
            tab.history.end_coalesce();
        }
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
    let current_first_1based = if binding.loop_mode == SymbolLoopMode::Loop {
        (binding.first_frame % max_first_1based) + 1
    } else {
        (binding.first_frame + 1).clamp(1, max_first_1based)
    };
    let mut next_first_1based = current_first_1based;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let half_w = (ui.available_width() - 6.0) * 0.5;
        if ui
            .add_sized(
                Vec2::new(half_w, 26.0),
                egui::Button::new("ループ再生").selected(binding.loop_mode == SymbolLoopMode::Loop),
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
    let mut drag_active = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(
            egui::RichText::new(frame_label)
                .size(11.0)
                .color(Color32::from_rgb(55, 62, 74)),
        );
        let mut step_clicked = false;
        if ui
            .add_enabled(
                next_first_1based > 1,
                egui::Button::new(icons::STEP_PREV).min_size(Vec2::new(22.0, 22.0)),
            )
            .on_hover_text("前のシンボル内フレーム")
            .clicked()
        {
            next_first_1based = next_first_1based.saturating_sub(1).max(1);
            step_clicked = true;
        }
        let drag_resp = ui.add_sized(
            Vec2::new(56.0, 22.0),
            egui::DragValue::new(&mut next_first_1based)
                .range(1..=max_first_1based)
                .speed(0.15)
                .suffix(" f")
                .update_while_editing(false),
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
            step_clicked = true;
        }
        drag_active = !step_clicked
            && (drag_resp.dragged() || drag_resp.has_focus())
            && !drag_resp.drag_stopped()
            && !drag_resp.lost_focus();
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
                            egui::Button::new(format!("{f}F")).selected(next_first_1based == f),
                        )
                        .clicked()
                    {
                        next_first_1based = f;
                        drag_active = false;
                    }
                }
            });
            if chunk_end < max_first_1based {
                ui.add_space(3.0);
            }
        }
    }

    if (next_mode != binding.loop_mode || !drag_active)
        && matches!(
            tab.history.last_coalesce_key,
            Some(CoalesceKey::SymbolPlayback(_))
        )
    {
        tab.history.end_coalesce();
    }
    if next_mode != binding.loop_mode || next_first_1based != current_first_1based {
        actions.push(UiAction::SetSymbolPlayback {
            loop_mode: next_mode,
            first_frame: next_first_1based.saturating_sub(1),
            drag_active: drag_active && next_mode == binding.loop_mode,
        });
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
        actions.push(UiAction::EnterSymbolTimeline(binding));
    }
}

/// 「ライブラリ」タブの描画（シンボルのプレビュー・一覧・名前変更・ステージ配置・編集・削除）
pub(crate) fn draw_library_tab(
    tab: &mut Tab,
    actions: &mut Vec<UiAction>,
    ui: &mut egui::Ui,
) {
    let has_sel = !tab.session.selected_shapes.is_empty();
    if ui
        .add_enabled(
            has_sel,
            egui::Button::new(format!("{} 選択図形をシンボル化 (F8)", icons::ADD))
                .min_size(Vec2::new(ui.available_width(), 28.0)),
        )
        .on_hover_text("キャンバスで選択中の図形を新しいシンボルとして登録 (F8)")
        .clicked()
    {
        actions.push(UiAction::ConvertSelectionToSymbol);
    }

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);

    if tab.doc.library.is_empty() {
        ui.label(
            egui::RichText::new(
                "シンボルがありません。\n図形を選択して F8 キーを押すとシンボルを作成できます。",
            )
            .size(11.5)
            .color(Color32::from_rgb(115, 122, 135)),
        );
        return;
    }

    tab.ensure_valid_library_selection();

    if let Some(sel_id) = tab.view.selected_library_symbol {
        draw_selected_library_symbol_controls(tab, actions, ui, sel_id);
    }

    draw_panel_section_label(
        ui,
        &format!("登録シンボル一覧 ({})", tab.doc.library.len()),
        4.0,
    );
    draw_library_symbol_list(tab, actions, ui);
}

/// ライブラリで選択中のシンボルのミニプレビューおよび操作ボタン群（配置・編集・削除）を描画
fn draw_selected_library_symbol_controls(
    tab: &Tab,
    actions: &mut Vec<UiAction>,
    ui: &mut egui::Ui,
    sel_id: u64,
) {
    if let Some(sym) = tab.doc.find_symbol(sel_id) {
        draw_symbol_mini_preview(tab, ui, sym);
        ui.add_space(6.0);
    }

    let is_editing_self = tab
        .session
        .edit_stack
        .iter()
        .any(|sess| sess.symbol_id == sel_id);
    let can_place = tab.can_place_symbol_in_current_context(sel_id)
        && tab
            .doc
            .root
            .is_layer_editable_by_id(active_scope_layer_id(&tab.session, &tab.doc.root));
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
            actions.push(UiAction::PlaceSymbolFromLibrary(sel_id));
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
            actions.push(UiAction::EnterSymbolFromLibrary(sel_id));
        }
        if ui
            .add_sized(
                Vec2::new(54.0, 28.0),
                egui::Button::new(format!("{} 削除", icons::DELETE)),
            )
            .on_hover_text("選択中のシンボルをライブラリから削除します")
            .clicked()
        {
            actions.push(UiAction::DeleteSymbolFromLibrary(sel_id));
        }
    });
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
}

/// ライブラリ登録シンボルのスクロール一覧を描画し、選択切替・名前変更コマンドを発行する
fn draw_library_symbol_list(
    tab: &Tab,
    actions: &mut Vec<UiAction>,
    ui: &mut egui::Ui,
) {
    let entries: Vec<(u64, String, usize)> = tab
        .doc
        .library
        .iter()
        .map(|s| (s.id, s.name.clone(), s.timeline.total_frames.max(1)))
        .collect();
    let mut select_req: Option<u64> = None;
    let mut rename_req: Option<(u64, String)> = None;
    let mut clear_renaming_id: Option<u64> = None;

    let active_doc_id = tab.id;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (sym_id, mut name, total_frames) in entries {
                ui.push_id((active_doc_id, sym_id), |ui| {
                    let is_sel = tab.view.selected_library_symbol == Some(sym_id);
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
                        .stroke(Stroke::new(1.0_f32, stroke_col))
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
                                    select_req = Some(sym_id);
                                }
                                let text_w = (ui.available_width() - 36.0).max(60.0);
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut name).desired_width(text_w),
                                );
                                if resp.clicked() || resp.gained_focus() {
                                    select_req = Some(sym_id);
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
                });
                ui.add_space(3.0);
            }
        });

    if let Some(sym_id) = select_req {
        actions.push(UiAction::SelectLibrarySymbol(sym_id));
    }
    if let Some((sym_id, new_name)) = rename_req {
        actions.push(UiAction::RenameSymbolInLibrary { sym_id, new_name });
    }
    if let Some(finished_id) = clear_renaming_id {
        actions.push(UiAction::FinishRenamingSymbol(finished_id));
    }
}

/// ライブラリパネル上部にシンボルの縮小ベクタープレビューを描画する
fn draw_symbol_mini_preview(tab: &Tab, ui: &mut egui::Ui, sym: &SymbolDef) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 110.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(252, 252, 254));
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0_f32, Color32::from_rgb(206, 212, 222)),
    );

    let Some(bbox) = sym.preview_local_bounds(tab.doc.revision, &tab.doc.library) else {
        return;
    };
    let w = (bbox.x1 - bbox.x0).max(1.0) as f32;
    let h = (bbox.y1 - bbox.y0).max(1.0) as f32;
    let pad = 14.0;
    let avail_w = (rect.width() - pad * 2.0).max(10.0);
    let avail_h = (rect.height() - pad * 2.0).max(10.0);
    let scale = (avail_w / w).min(avail_h / h).clamp(0.05, 4.0);
    let cx = ((bbox.x0 + bbox.x1) * 0.5) as f32;
    let cy = ((bbox.y0 + bbox.y1) * 0.5) as f32;
    let pan = rect.center().to_vec2() - Vec2::new(cx * scale, cy * scale);
    let origin_scr = Pos2::new(pan.x, pan.y);

    if let Some(mesh) =
        sym.cached_preview_screen_mesh(scale, pan, tab.doc.revision, &tab.doc.library)
    {
        painter.add(arc_mesh_to_shape(mesh));
        if rect.contains(origin_scr) {
            draw_registration_cross(&painter, origin_scr, 4.0, false);
        }
    }
}

// ============================================================================
// 中央ステージ・アートワーク・オニオンスキン・ツールカーソル描画
// ============================================================================

pub(crate) fn draw_tool_cursor_and_feedback(
    tab: &Tab,
    tool: &ToolSettings,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match tool.tool {
        Tool::Brush | Tool::Eraser => {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
            let r = if tool.tool == Tool::Brush {
                tool.brush_radius
            } else {
                tool.eraser_radius
            };
            let ring_col = if tool.tool == Tool::Brush {
                tool.brush_color
            } else {
                Color32::from_rgb(220, 60, 60)
            };
            painter.circle_stroke(pos, r * tab.view.zoom, Stroke::new(1.5_f32, ring_col));
        }
        Tool::Bucket => {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
            painter.circle_filled(pos, 5.5, tool.brush_color);
            painter.circle_stroke(pos, 5.5, Stroke::new(1.8_f32, Color32::WHITE));
        }
        Tool::Eyedropper => {
            let hovered_color =
                crate::tool_paint::topmost_visible_color_at(tab, tab.view.screen_to_world(pos));
            ctx.set_cursor_icon(if hovered_color.is_some() {
                CursorIcon::PointingHand
            } else {
                CursorIcon::Crosshair
            });
            crate::tool_paint::draw_eyedropper_cursor_badge(
                painter,
                pos,
                hovered_color.unwrap_or(tool.brush_color),
                hovered_color.is_some(),
            );
        }
        Tool::Transform => {
            crate::tool_transform::draw_transform_tool_cursor(tab, ctx, painter, pos)
        }
        Tool::Select => {
            crate::tool_select::draw_select_tool_cursor(tab, tool.select_sub_mode, ctx, painter, pos)
        }
    }
}

/// キャンバス上の全シェイプ（オニオンスキン・全可視レイヤー合成）、選択オーバーレイ、グループ選択枠、自由変形ボックス、矩形選択枠、およびストロークプレビューを描画
pub(crate) fn draw_canvas_contents(
    tab: &Tab,
    tool: &ToolSettings,
    timeline_ui: &TimelineUiState,
    painter: &egui::Painter,
) {
    let in_group_edit = !tab.session.editing_group_path.is_empty();
    let in_symbol_edit = !tab.session.edit_stack.is_empty();
    let zoom = tab.view.zoom;
    let pan = tab.view.pan;
    let stage_size = tab.doc.stage_size;
    let artwork_painter = tab
        .view
        .artwork_painter(painter, painter.clip_rect(), stage_size, in_symbol_edit);

    // 0. シンボル内部タイムライン編集中の場合、親シーンの背景図形を1つの結合メッシュで薄暗く（インプレース背景）描画
    if let Some(session) = tab.session.edit_stack.last() {
        if let Some(mesh) = session.backdrop_cel.cached_backdrop_screen_mesh(
            session.parent_timeline.current_frame,
            zoom,
            pan,
            &tab.doc.library,
        ) {
            artwork_painter.add(arc_mesh_to_shape(mesh));
        }
    }

    // 0.5 オニオンスキン描画（前のフレーム＝青系半透明、次のフレーム＝緑系半透明）
    if tab.view.onion_skin_enabled {
        draw_onion_skins(tab, &artwork_painter);
    }

    // 1. 下層レイヤーから上層レイヤーへ順に合成描画（`tab.session.shapes` はレイヤー順に整列されているため単一カーソルで O(N) 走査）
    let num_layers = tab.doc.root.layers.len();
    let mut selected_group_boxes: Vec<(u64, Option<KRect>, KRect, Option<Point>)> = Vec::new();
    let (is_sel, has_sel) = tab.session.selected_shape_mask();
    let curr_f = tab.doc.root.current_frame;
    let view_world_rect =
        tab.view
            .visible_world_rect(&artwork_painter, stage_size, in_symbol_edit);
    let mut shape_cursor = 0usize;

    for layer_idx in 0..num_layers {
        let Some((lid, is_visible, is_locked)) = tab
            .doc
            .root
            .layers
            .get(layer_idx)
            .map(|l| (l.id, l.visible, l.locked))
        else {
            continue;
        };
        if !is_visible {
            continue;
        }
        if timeline_ui.is_playing || is_locked {
            if let Some(mesh) = tab.doc.root.resolved_cel_screen_mesh(
                layer_idx,
                curr_f,
                zoom,
                pan,
                in_group_edit && !timeline_ui.is_playing,
                &tab.doc.library,
            ) {
                artwork_painter.add(arc_mesh_to_shape(mesh));
            }
            continue;
        }

        if num_layers > 1 {
            while shape_cursor < tab.session.shapes.len()
                && tab.session.shapes[shape_cursor].layer_id != lid
                && tab
                    .doc
                    .root
                    .layer_index_by_id(tab.session.shapes[shape_cursor].layer_id)
                    .is_none_or(|ord| ord < layer_idx)
            {
                shape_cursor += 1;
            }
        }
        let start_idx = shape_cursor;
        while shape_cursor < tab.session.shapes.len()
            && (num_layers <= 1 || tab.session.shapes[shape_cursor].layer_id == lid)
        {
            shape_cursor += 1;
        }
        draw_editable_layer_shapes(
            tab,
            tool,
            (&artwork_painter, painter),
            start_idx..shape_cursor,
            view_world_rect,
            (&is_sel, has_sel),
            &mut selected_group_boxes,
        );
    }

    tab.view
        .draw_stage_foreground_overlay(painter, painter.clip_rect(), stage_size, in_symbol_edit);
    draw_selected_group_and_symbol_boxes(tab, painter, &selected_group_boxes);

    if tool.tool == Tool::Transform {
        crate::tool_transform::draw_free_transform_box(&tab.session, painter, &tab.view);
    }
    draw_active_marquee_box(tab, painter);
    crate::tool_paint::draw_active_stroke_preview(tab, tool.tool, &artwork_painter, painter);
}

/// 編集可能レイヤー1枚分の図形スライス範囲 `range` を描画し、選択中のグループ／シンボル外枠情報を集計する
fn draw_editable_layer_shapes(
    tab: &Tab,
    tool: &ToolSettings,
    (artwork_painter, gizmo_painter): (&egui::Painter, &egui::Painter),
    range: std::ops::Range<usize>,
    view_world_rect: KRect,
    (is_sel, has_sel): (&[bool], bool),
    selected_group_boxes: &mut Vec<(u64, Option<KRect>, KRect, Option<Point>)>,
) {
    let show_wf = tool.tool == Tool::Select
        && (tab.view.show_wireframe || tool.select_sub_mode != SelectSubMode::Reshape);
    let active_affine_drag = matches!(
        tab.session.drag,
        ActiveCanvasDrag::Transform(
            TransformDragState::Scale { .. } | TransformDragState::Rotate { .. }
        )
    );
    let in_group_edit = !tab.session.editing_group_path.is_empty();
    let zoom = tab.view.zoom;
    let pan = tab.view.pan;

    for idx in range {
        let Some(shape) = tab.session.shapes.get(idx) else {
            continue;
        };
        let in_scope = shape.is_in_editing_scope(&tab.session.editing_group_path);
        let shape_sel =
            in_scope && has_sel && !active_affine_drag && is_sel.get(idx).copied().unwrap_or(false);
        let sel_gid = if shape_sel {
            let gid = shape.active_group_id_at_scope(&tab.session.editing_group_path);
            if let Some(gid) = gid {
                let geom_bbox = shape.geometry_bounding_box();
                let fallback_bbox = shape.bounding_box();
                let sym_reg = shape
                    .symbol_instance
                    .filter(|b| b.instance_gid == gid)
                    .map(|b| b.xform * Point::ZERO);
                if let Some((_, g_geom, g_fallback, _)) = selected_group_boxes
                    .iter_mut()
                    .find(|(id, _, _, _)| *id == gid)
                {
                    if let Some(gb) = geom_bbox {
                        *g_geom = Some(g_geom.map_or(gb, |acc| acc.union(gb)));
                    }
                    *g_fallback = g_fallback.union(fallback_bbox);
                } else {
                    selected_group_boxes.push((gid, geom_bbox, fallback_bbox, sym_reg));
                }
            }
            gid
        } else {
            None
        };

        let Some(geom_bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if !crate::vector_engine::rects_overlap(geom_bbox, view_world_rect) {
            continue;
        }

        let dim = in_group_edit && !in_scope;
        artwork_painter.add(arc_mesh_to_shape(shape.cached_screen_mesh(zoom, pan, dim)));
        if !in_scope {
            continue;
        }
        if show_wf && shape.is_raw_at_scope(&tab.session.editing_group_path) {
            crate::tool_select::draw_shape_contour_lines(
                tab,
                gizmo_painter,
                shape,
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(30, 80, 180, 140)),
                true,
            );
        }
        if shape_sel && sel_gid.is_none() {
            crate::tool_select::draw_flash_selection_overlay(tab, gizmo_painter, idx);
        }
    }
}

/// 選択中のグループ塊（緑枠）およびシンボルインスタンス（水色枠＋基準点 `＋` マーク）、シンボル編集中の基準点を描画
fn draw_selected_group_and_symbol_boxes(
    tab: &Tab,
    painter: &egui::Painter,
    selected_group_boxes: &[(u64, Option<KRect>, KRect, Option<Point>)],
) {
    for &(_, geom_bbox, fallback_bbox, sym_reg) in selected_group_boxes {
        let g_bbox = geom_bbox.unwrap_or(fallback_bbox);
        let rect = tab.view.world_rect_to_screen(g_bbox).expand(2.0);
        if let Some(origin) = sym_reg {
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.8_f32, Color32::from_rgb(0, 165, 255)),
            );
            draw_registration_cross(painter, tab.view.world_to_screen(origin), 3.2, true);
        } else {
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.8_f32, Color32::from_rgb(30, 185, 95)),
            );
        }
    }

    if let Some(session) = tab.session.edit_stack.last() {
        let origin = session.instance_xform * Point::ZERO;
        draw_registration_cross(painter, tab.view.world_to_screen(origin), 5.5, true);
    }
}

fn draw_onion_skins(tab: &Tab, painter: &egui::Painter) {
    let curr_f = tab.doc.root.current_frame;
    for step in (1..=tab.view.onion_prev_frames).rev() {
        if curr_f >= step {
            let alpha = (84 / step as u8).clamp(24, 84);
            draw_onion_skin_frame(tab, painter, curr_f - step, [45, 115, 245], alpha);
        }
    }
    for step in 1..=tab.view.onion_next_frames {
        let f = curr_f + step;
        if f < tab.doc.root.total_frames {
            let alpha = (84 / step as u8).clamp(24, 84);
            draw_onion_skin_frame(tab, painter, f, [35, 185, 105], alpha);
        }
    }
}

fn draw_onion_skin_frame(
    tab: &Tab,
    painter: &egui::Painter,
    frame: usize,
    tint_rgb: [u8; 3],
    alpha: u8,
) {
    let zoom = tab.view.zoom;
    let pan = tab.view.pan;
    for (layer_idx, layer) in tab.doc.root.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        if let Some(mesh) = tab.doc.root.resolved_cel_onion_mesh(
            layer_idx,
            frame,
            zoom,
            pan,
            (tint_rgb, alpha),
            &tab.doc.library,
        ) {
            painter.add(arc_mesh_to_shape(mesh));
        }
    }
}

fn draw_active_marquee_box(tab: &Tab, painter: &egui::Painter) {
    let active_marquee = match &tab.session.drag {
        ActiveCanvasDrag::Select(SelectDragState::MarqueeSelect(m))
        | ActiveCanvasDrag::Transform(TransformDragState::MarqueeSelect(m)) => Some(*m),
        _ => None,
    };
    if let Some(m) = active_marquee {
        let marquee_rect = tab
            .view
            .world_rect_to_screen(KRect::from_points(m.start_pos, m.current_pos));
        painter.rect_filled(
            marquee_rect,
            0.0,
            Color32::from_rgba_unmultiplied(0, 145, 255, 32),
        );
        painter.rect_stroke(
            marquee_rect,
            0.0,
            Stroke::new(1.5_f32, Color32::from_rgb(0, 145, 255)),
        );
    }
}

/// 未保存変更があるタブを閉じる際の非ブロッキング確認モーダルダイアログ（egui Window）を描画する
pub(crate) fn draw_unsaved_close_confirm_modal(
    tabs: &[Tab],
    pending_close_confirm: &Option<PendingCloseConfirm>,
    actions: &mut Vec<UiAction>,
    ctx: &egui::Context,
) {
    let Some(pending) = pending_close_confirm.clone() else {
        return;
    };
    let tab_index = match &pending {
        PendingCloseConfirm::SingleTab { tab_index, .. } => *tab_index,
        PendingCloseConfirm::ViewportClose {
            current_tab_index, ..
        } => *current_tab_index,
    };
    let tab_name = tabs
        .get(tab_index)
        .map(|tab| tab.base_title())
        .unwrap_or_else(|| "無題".to_owned());

    egui::Area::new(egui::Id::new("unsaved_close_confirm_backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(Pos2::ZERO)
        .show(ctx, |ui| {
            let screen_rect = ctx.screen_rect();
            ui.allocate_rect(screen_rect, egui::Sense::click_and_drag());
            ui.painter().rect_filled(
                screen_rect,
                0.0,
                Color32::from_rgba_unmultiplied(15, 20, 28, 96),
            );
        });

    egui::Window::new("未保存の変更の確認")
        .id(egui::Id::new("unsaved_close_confirm_modal"))
        .order(egui::Order::Tooltip)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.add_space(4.0);
            ui.label(format!(
                "「{tab_name}」には未保存の変更があります。\n閉じる前に変更を保存しますか？"
            ));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("保存して閉じる").clicked() {
                    actions.push(UiAction::ConfirmPendingCloseSave);
                }
                if ui.button("保存せずに破棄").clicked() {
                    actions.push(UiAction::ConfirmPendingCloseDiscard);
                }
                if ui.button("キャンセル").clicked() {
                    actions.push(UiAction::ConfirmPendingCloseCancel);
                }
            });
        });
}
