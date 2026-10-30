mod canvas_tool;
mod clipboard_ops;
mod panels;
mod project_io;
mod select_ops;
mod symbol;
mod symbol_ops;
mod timeline;
mod timeline_ui;
mod tool_paint;
mod tool_select;
mod tool_transform;
mod vector_boolean;
mod vector_engine;
mod vector_stroke;

use std::time::Instant;

use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Sense, Stroke, Vec2};
use kurbo::Point;

use canvas_tool::{EditorDoc, NoticeKind, SelectSubMode, Tool};
#[cfg(test)]
pub(crate) use canvas_tool::MAX_UNDO_HISTORY;
use panels::UiAction;
use project_io::{DocumentTabManager, ProjectWorkspace};
#[cfg(test)]
pub(crate) use timeline::Timeline;
use timeline_ui::{TimelineHost, TimelineUiState};
#[cfg(test)]
pub(crate) use vector_engine::FillShape;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("is2d - 無題-1")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 520.0]),
        ..Default::default()
    };

    eframe::run_native(
        "is2d",
        native_options,
        Box::new(|cc| {
            configure_japanese_fonts(&cc.egui_ctx);
            configure_sharp_light_theme(&cc.egui_ctx);
            Ok(Box::new(Is2dApp::default()))
        }),
    )
}

/// `fonts/` フォルダ内の日本語フォント（NotoSansJP）とアイコンフォント（MaterialIcons）を読み込む
fn configure_japanese_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "noto_sans_jp_regular".to_owned(),
        egui::FontData::from_static(include_bytes!("../fonts/NotoSansJP-Regular.ttf")),
    );
    fonts.font_data.insert(
        "noto_sans_jp_bold".to_owned(),
        egui::FontData::from_static(include_bytes!("../fonts/NotoSansJP-Bold.ttf")),
    );
    fonts.font_data.insert(
        "material_icons".to_owned(),
        egui::FontData::from_static(include_bytes!("../fonts/MaterialIcons-Regular.ttf")),
    );

    if let Some(proportional) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        proportional.insert(0, "noto_sans_jp_regular".to_owned());
        proportional.insert(1, "noto_sans_jp_bold".to_owned());
        proportional.insert(2, "material_icons".to_owned());
    }
    if let Some(monospace) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        monospace.push("noto_sans_jp_regular".to_owned());
        monospace.push("material_icons".to_owned());
    }

    ctx.set_fonts(fonts);
}

/// アプリ全体のUIをライトテーマ＋角丸なし（すべて直角のシャープな矩形）に設定する
fn configure_sharp_light_theme(ctx: &egui::Context) {
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
    let mut visuals = egui::Visuals::light();
    visuals.window_rounding = egui::Rounding::ZERO;
    visuals.menu_rounding = egui::Rounding::ZERO;
    visuals.widgets.noninteractive.rounding = egui::Rounding::ZERO;
    visuals.widgets.inactive.rounding = egui::Rounding::ZERO;
    visuals.widgets.hovered.rounding = egui::Rounding::ZERO;
    visuals.widgets.active.rounding = egui::Rounding::ZERO;
    visuals.widgets.open.rounding = egui::Rounding::ZERO;
    ctx.set_visuals(visuals);
}

/// キーリピートを除外して「押された最初のフレームのみ `true`」を返すヘルパー
fn key_pressed_no_repeat(i: &egui::InputState, target: Key) -> bool {
    i.events.iter().any(|ev| {
        matches!(
            ev,
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                ..
            } if *key == target
        )
    })
}

// ============================================================================
// メインアプリケーション状態 & ショートカット・アクションディスパッチ制御
// ============================================================================

/// アプリケーション全体のルート構造体（単一ドキュメント状態 `EditorDoc`・タイムラインUI状態・複数タブ管理で構成）
#[derive(Default)]
pub(crate) struct Is2dApp {
    /// 現在アクティブなドキュメント状態（UI・ビューポート・作業キャンバス・タイムライン・シンボル・Undo履歴）
    pub(crate) doc: EditorDoc,
    /// タイムラインパネルの再生・スクロール・D&D等の一時UI状態
    pub(crate) timeline_ui: TimelineUiState,
    /// 複数ドキュメント同時オープン用のタブ管理状態（`EditorDoc` 一括スワップ方式）
    pub(crate) tabs: DocumentTabManager,
}

impl std::ops::Deref for Is2dApp {
    type Target = EditorDoc;
    fn deref(&self) -> &Self::Target {
        &self.doc
    }
}

impl std::ops::DerefMut for Is2dApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.doc
    }
}

impl ProjectWorkspace for Is2dApp {
    fn doc(&self) -> &EditorDoc {
        &self.doc
    }

    fn doc_mut(&mut self) -> &mut EditorDoc {
        &mut self.doc
    }

    fn tabs(&self) -> &DocumentTabManager {
        &self.tabs
    }

    fn tabs_mut(&mut self) -> &mut DocumentTabManager {
        &mut self.tabs
    }

    fn doc_and_tabs_mut(&mut self) -> (&mut EditorDoc, &mut DocumentTabManager) {
        (&mut self.doc, &mut self.tabs)
    }
}

impl TimelineHost for Is2dApp {
    fn doc(&self) -> &EditorDoc {
        &self.doc
    }

    fn doc_mut(&mut self) -> &mut EditorDoc {
        &mut self.doc
    }

    fn timeline_ui(&self) -> &TimelineUiState {
        &self.timeline_ui
    }

    fn timeline_ui_mut(&mut self) -> &mut TimelineUiState {
        &mut self.timeline_ui
    }

    fn doc_and_timeline_ui_mut(&mut self) -> (&mut EditorDoc, &mut TimelineUiState) {
        (&mut self.doc, &mut self.timeline_ui)
    }
}

impl Is2dApp {
    pub(crate) fn undo(&mut self) {
        self.timeline_ui.sync_with_doc(&mut self.doc);
        self.doc.undo();
        self.timeline_ui.sync_with_doc(&mut self.doc);
    }

    pub(crate) fn redo(&mut self) {
        self.timeline_ui.sync_with_doc(&mut self.doc);
        self.doc.redo();
        self.timeline_ui.sync_with_doc(&mut self.doc);
    }

    /// UIパネルから収集した `UiAction` を一括ディスパッチする
    pub(crate) fn apply_action(&mut self, action: UiAction, ctx: &egui::Context) {
        match action {
            UiAction::NewProject => project_io::new_project(self, ctx),
            UiAction::OpenProjectDialog => project_io::open_project_dialog(self, ctx),
            UiAction::SaveProject => project_io::save_project(self, ctx),
            UiAction::SaveProjectAsDialog => project_io::save_project_as_dialog(self, ctx),
            UiAction::CloseActiveTab => project_io::close_active_tab(self, Some(ctx)),
            UiAction::CloseTab(idx) => project_io::close_tab(self, idx, Some(ctx)),
            UiAction::SwitchToTab(idx) => {
                project_io::switch_to_tab(self, idx, Some(ctx));
            }
            UiAction::ConfirmPendingCloseSave => project_io::confirm_pending_close_save(self, ctx),
            UiAction::ConfirmPendingCloseDiscard => {
                project_io::confirm_pending_close_discard(self, ctx)
            }
            UiAction::ConfirmPendingCloseCancel => {
                project_io::confirm_pending_close_cancel(self, ctx)
            }
            UiAction::Undo => self.undo(),
            UiAction::Redo => self.redo(),
            UiAction::CutSelected => {
                canvas_tool::stop_playback(&mut self.doc);
                clipboard_ops::cut_selected_shapes(&mut self.doc);
            }
            UiAction::CopySelected => {
                canvas_tool::stop_playback(&mut self.doc);
                clipboard_ops::copy_selected_shapes(&mut self.doc);
            }
            UiAction::PasteClipboard { in_place } => {
                canvas_tool::stop_playback(&mut self.doc);
                clipboard_ops::paste_clipboard(&mut self.doc, in_place);
            }
            UiAction::DeleteSelected => {
                canvas_tool::stop_playback(&mut self.doc);
                select_ops::delete_selected_shapes(&mut self.doc);
            }
            UiAction::ClearActiveFrame => {
                select_ops::clear_active_frame_shapes(&mut self.doc);
            }
            UiAction::SetTool(tool) => {
                canvas_tool::set_tool(&mut self.doc, tool);
            }
            UiAction::ToggleSelectSubMode(mode) => {
                canvas_tool::toggle_select_sub_mode(&mut self.doc, mode);
            }
            UiAction::ResetView => {
                self.doc.viewport.reset_view();
            }
            UiAction::FitStageToWindow => {
                self.doc.viewport.fit_stage_to_window();
            }
            UiAction::ExitGroupSteps(steps) => {
                for _ in 0..steps {
                    if !select_ops::exit_current_group(&mut self.doc) {
                        break;
                    }
                }
            }
            UiAction::GroupSelected => {
                select_ops::group_selected_shapes(&mut self.doc);
            }
            UiAction::UngroupSelected => {
                select_ops::ungroup_selected_shapes(&mut self.doc);
            }
            UiAction::FlipSelected { flip_x, flip_y } => {
                tool_transform::flip_selected_shapes(&mut self.doc, flip_x, flip_y);
            }
            UiAction::RotateSelected(angle) => {
                tool_transform::rotate_selected_shapes(&mut self.doc, angle);
            }
            UiAction::ResetTransformPivot => {
                tool_transform::reset_selected_transform_pivot(&mut self.doc);
            }
            UiAction::ConvertSelectionToSymbol => {
                symbol_ops::convert_selection_to_symbol(&mut self.doc);
            }
            UiAction::SetTweenRotate(rotate) => {
                symbol_ops::set_active_keyframe_tween_rotate(&mut self.doc, rotate);
            }
            UiAction::SetSymbolPlayback {
                loop_mode,
                first_frame,
                drag_active,
            } => {
                symbol_ops::set_selected_symbol_playback(&mut self.doc, loop_mode, first_frame);
                self.doc.ui.editing_symbol_playback = drag_active;
            }
            UiAction::EnterSymbolTimeline(binding) => {
                symbol_ops::enter_symbol_timeline(&mut self.doc, binding);
            }
            UiAction::PlaceSymbolFromLibrary(sym_id) => {
                symbol_ops::place_symbol_instance_from_library(&mut self.doc, sym_id);
            }
            UiAction::EnterSymbolFromLibrary(sym_id) => {
                symbol_ops::enter_symbol_from_library(&mut self.doc, sym_id);
            }
            UiAction::DeleteSymbolFromLibrary(sym_id) => {
                symbol_ops::delete_symbol_from_library(&mut self.doc, sym_id);
            }
            UiAction::RenameSymbolInLibrary { sym_id, new_name } => {
                if symbol_ops::rename_symbol_in_library(&mut self.doc, sym_id, new_name) {
                    self.doc.symbols.actively_renaming_symbol = Some(sym_id);
                }
            }
        }
        self.timeline_ui.sync_with_doc(&mut self.doc);
    }

    // ------------------------------------------------------------------------
    // 2. キーボードショートカット制御
    // ------------------------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.tabs.pending_close_confirm.is_some() {
            return;
        }
        let input = ctx.input(|i| i.clone());
        let cmd = input.modifiers.command || input.modifiers.ctrl;
        let file_cmd = if cmd && key_pressed_no_repeat(&input, Key::N) {
            Some(0u8)
        } else if cmd && key_pressed_no_repeat(&input, Key::O) {
            Some(1u8)
        } else if cmd && key_pressed_no_repeat(&input, Key::S) {
            Some(if input.modifiers.shift { 3u8 } else { 2u8 })
        } else if cmd && key_pressed_no_repeat(&input, Key::W) {
            Some(4u8)
        } else {
            None
        };
        if let Some(cmd_id) = file_cmd {
            match cmd_id {
                0 => project_io::new_project(self, ctx),
                1 => project_io::open_project_dialog(self, ctx),
                2 => project_io::save_project(self, ctx),
                3 => project_io::save_project_as_dialog(self, ctx),
                4 => project_io::close_active_tab(self, Some(ctx)),
                _ => {}
            }
            return;
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        let copy_event = input
            .events
            .iter()
            .any(|ev| matches!(ev, egui::Event::Copy));
        let cut_event = input.events.iter().any(|ev| matches!(ev, egui::Event::Cut));
        let paste_event = input
            .events
            .iter()
            .any(|ev| matches!(ev, egui::Event::Paste(_)));

        if !cmd && !copy_event && !cut_event && !paste_event {
            self.handle_single_key_shortcuts(&input, input.modifiers.shift);
        }

        if key_pressed_no_repeat(&input, Key::Delete)
            || key_pressed_no_repeat(&input, Key::Backspace)
        {
            select_ops::delete_selected_shapes(&mut self.doc);
        }

        self.handle_command_shortcuts(&input, cmd, copy_event, cut_event, paste_event);
    }

    fn handle_single_key_shortcuts(&mut self, i: &egui::InputState, shift: bool) {
        if !self.doc.canvas.selected_shapes.is_empty() {
            let step = if shift { 10.0 } else { 1.0 };
            let mut delta = kurbo::Vec2::ZERO;
            let mut any_non_repeat = false;
            for ev in &i.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    ..
                } = ev
                {
                    match key {
                        Key::ArrowLeft => {
                            delta.x -= step;
                            any_non_repeat |= !*repeat;
                        }
                        Key::ArrowRight => {
                            delta.x += step;
                            any_non_repeat |= !*repeat;
                        }
                        Key::ArrowUp => {
                            delta.y -= step;
                            any_non_repeat |= !*repeat;
                        }
                        Key::ArrowDown => {
                            delta.y += step;
                            any_non_repeat |= !*repeat;
                        }
                        _ => {}
                    }
                }
            }
            if delta != kurbo::Vec2::ZERO {
                if any_non_repeat {
                    let _ = select_ops::nudge_selected_shapes(&mut self.doc, delta);
                } else {
                    canvas_tool::stop_playback(&mut self.doc);
                    canvas_tool::cancel_active_drag(&mut self.doc);
                    self.doc.canvas.translate_selected(delta);
                    self.doc.canvas.selection_modified = true;
                    canvas_tool::sync_shapes_to_timeline(&mut self.doc);
                }
                return;
            }
        }

        if key_pressed_no_repeat(i, Key::Enter) {
            timeline_ui::toggle_playback(self);
        } else if i.key_pressed(Key::Comma) || i.key_pressed(Key::ArrowLeft) {
            timeline_ui::step_frame(self, -1);
        } else if i.key_pressed(Key::Period) || i.key_pressed(Key::ArrowRight) {
            timeline_ui::step_frame(self, 1);
        } else if key_pressed_no_repeat(i, Key::Home) {
            let act = self.doc.timeline.active_layer;
            timeline_ui::switch_to_layer_and_frame(&mut self.doc, act, 0);
        } else if key_pressed_no_repeat(i, Key::End) {
            let last = self.doc.timeline.total_frames.saturating_sub(1);
            let act = self.doc.timeline.active_layer;
            timeline_ui::switch_to_layer_and_frame(&mut self.doc, act, last);
        } else if i.key_pressed(Key::F6) {
            if shift {
                timeline_ui::timeline_delete_keyframe(&mut self.doc);
            } else {
                timeline_ui::timeline_insert_keyframe(&mut self.doc, true);
            }
        } else if i.key_pressed(Key::F7) {
            timeline_ui::timeline_insert_keyframe(&mut self.doc, false);
        } else if key_pressed_no_repeat(i, Key::F8) {
            symbol_ops::convert_selection_to_symbol(&mut self.doc);
        } else if i.key_pressed(Key::F5) {
            if shift {
                timeline_ui::timeline_remove_frame(&mut self.doc);
            } else {
                timeline_ui::timeline_extend_frame(&mut self.doc);
            }
        } else if key_pressed_no_repeat(i, Key::O) {
            self.timeline_ui.onion_skin_enabled = !self.timeline_ui.onion_skin_enabled;
        } else if key_pressed_no_repeat(i, Key::T) {
            symbol_ops::timeline_toggle_tween(&mut self.doc);
        } else if let Some(tool) = Tool::from_shortcut_input(i) {
            canvas_tool::set_tool(&mut self.doc, tool);
        } else if key_pressed_no_repeat(i, Key::Escape) {
            canvas_tool::cancel_active_drag(&mut self.doc);
            if !self.doc.canvas.selected_shapes.is_empty() {
                canvas_tool::clear_selection(&mut self.doc);
            } else {
                select_ops::exit_current_group(&mut self.doc);
            }
            self.doc.ui.select_sub_mode = SelectSubMode::Reshape;
        } else if self.doc.ui.tool == Tool::Select {
            let has_repeat_key = i.events.iter().any(|ev| {
                matches!(
                    ev,
                    egui::Event::Key {
                        pressed: true,
                        repeat: true,
                        ..
                    }
                )
            });
            let plus_pressed = key_pressed_no_repeat(i, Key::Plus)
                || key_pressed_no_repeat(i, Key::Equals)
                || key_pressed_no_repeat(i, Key::Semicolon)
                || (!has_repeat_key
                    && i.events.iter().any(|ev| {
                        matches!(ev, egui::Event::Text(t) if t == "+" || t == "=" || t == ";" || t == "＋")
                    }));
            let minus_pressed = key_pressed_no_repeat(i, Key::Minus)
                || (!has_repeat_key
                    && i.events.iter().any(
                        |ev| matches!(ev, egui::Event::Text(t) if t == "-" || t == "ー" || t == "－"),
                    ));

            if plus_pressed {
                canvas_tool::toggle_select_sub_mode(&mut self.doc, SelectSubMode::AddAnchor);
            } else if minus_pressed {
                canvas_tool::toggle_select_sub_mode(&mut self.doc, SelectSubMode::DeleteAnchor);
            }
        }
    }

    fn handle_command_shortcuts(
        &mut self,
        i: &egui::InputState,
        cmd: bool,
        copy_event: bool,
        cut_event: bool,
        paste_event: bool,
    ) {
        let v_pressed_no_repeat = cmd && key_pressed_no_repeat(i, Key::V);
        let v_release_event = i.events.iter().any(|ev| {
            matches!(
                ev,
                egui::Event::Key {
                    key: Key::V,
                    pressed: false,
                    ..
                }
            )
        });
        let v_released_fallback =
            cmd && v_release_event && !self.doc.canvas.paste_triggered_on_press;
        if v_release_event {
            self.doc.canvas.paste_triggered_on_press = false;
        }

        if copy_event || (cmd && !i.modifiers.shift && key_pressed_no_repeat(i, Key::C)) {
            clipboard_ops::copy_selected_shapes(&mut self.doc);
        } else if cut_event || (cmd && !i.modifiers.shift && key_pressed_no_repeat(i, Key::X)) {
            clipboard_ops::cut_selected_shapes(&mut self.doc);
        } else if paste_event || v_pressed_no_repeat {
            self.doc.canvas.paste_triggered_on_press = true;
            clipboard_ops::paste_clipboard(&mut self.doc, i.modifiers.shift);
        } else if v_released_fallback {
            clipboard_ops::paste_clipboard(&mut self.doc, i.modifiers.shift);
        } else if cmd
            && (key_pressed_no_repeat(i, Key::Num1) || key_pressed_no_repeat(i, Key::Num0))
        {
            self.doc.viewport.reset_view();
        } else if cmd && key_pressed_no_repeat(i, Key::Num2) {
            self.doc.viewport.fit_stage_to_window();
        } else if cmd && key_pressed_no_repeat(i, Key::Num3) {
            self.doc.viewport.clip_outside_stage = !self.doc.viewport.clip_outside_stage;
        } else if cmd
            && (i.key_pressed(Key::Plus)
                || i.key_pressed(Key::Equals)
                || i.key_pressed(Key::Semicolon))
        {
            let center = i
                .pointer
                .hover_pos()
                .unwrap_or_else(|| self.doc.viewport.last_canvas_rect.center());
            self.doc.viewport.zoom_around_screen_pos(center, 1.25);
        } else if cmd && i.key_pressed(Key::Minus) {
            let center = i
                .pointer
                .hover_pos()
                .unwrap_or_else(|| self.doc.viewport.last_canvas_rect.center());
            self.doc.viewport.zoom_around_screen_pos(center, 0.8);
        } else if cmd && key_pressed_no_repeat(i, Key::G) {
            if i.modifiers.shift {
                select_ops::ungroup_selected_shapes(&mut self.doc);
            } else {
                select_ops::group_selected_shapes(&mut self.doc);
            }
        } else if cmd && key_pressed_no_repeat(i, Key::B) {
            select_ops::ungroup_selected_shapes(&mut self.doc);
        } else if cmd && i.key_pressed(Key::Z) {
            if i.modifiers.shift {
                self.redo();
            } else {
                self.undo();
            }
        } else if cmd && i.key_pressed(Key::Y) {
            self.redo();
        }
    }

    // ------------------------------------------------------------------------
    // 3. 中央キャンバスのツール入力ディスパッチ・バッジ・カーソル更新
    // ------------------------------------------------------------------------

    fn is_current_tool_allowed(&self) -> bool {
        match self.doc.ui.tool {
            Tool::Eyedropper => true,
            Tool::Brush | Tool::Eraser | Tool::Bucket => {
                self.doc.timeline.is_active_layer_editable()
            }
            Tool::Select | Tool::Transform => self.doc.timeline.has_any_editable_layer(),
        }
    }

    fn handle_canvas_tool_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        mouse_pt: Option<Point>,
        tool_allowed: bool,
    ) {
        if self.timeline_ui.is_playing {
            let canvas_interacted = (response.hovered()
                && ui.input(|i| i.pointer.primary_pressed()))
                || response.drag_started_by(egui::PointerButton::Primary)
                || response.clicked_by(egui::PointerButton::Primary);
            if canvas_interacted {
                canvas_tool::stop_playback(&mut self.doc);
                self.timeline_ui.sync_with_doc(&mut self.doc);
            } else {
                return;
            }
        }
        if self.doc.ui.tool == Tool::Eyedropper {
            let sampling = (response.hovered() && ui.input(|i| i.pointer.primary_pressed()))
                || response.dragged_by(egui::PointerButton::Primary)
                || response.clicked_by(egui::PointerButton::Primary);
            if sampling {
                if let Some(pt) = mouse_pt {
                    tool_paint::pick_color_at(&mut self.doc, pt);
                }
            }
        } else if tool_allowed {
            match self.doc.ui.tool {
                Tool::Brush | Tool::Eraser => {
                    tool_paint::handle_brush_or_eraser_input(&mut self.doc, response, mouse_pt);
                }
                Tool::Bucket => {
                    let just_pressed =
                        response.hovered() && ui.input(|i| i.pointer.primary_pressed());
                    if just_pressed {
                        if let Some(pt) = mouse_pt {
                            tool_paint::bucket_fill_at(&mut self.doc, pt);
                        }
                    }
                }
                Tool::Eyedropper => {}
                Tool::Select => {
                    tool_select::handle_select_tool_input(&mut self.doc, ui, response, mouse_pt);
                }
                Tool::Transform => {
                    tool_transform::handle_transform_tool_input(
                        &mut self.doc,
                        ui,
                        response,
                        mouse_pt,
                    );
                }
            }
        }
        self.timeline_ui.sync_with_doc(&mut self.doc);
    }

    /// 現在のレイヤー・フレーム状態から常時表示すべき状態連動メッセージを判定する
    fn current_state_notice(&self, tool_allowed: bool) -> Option<(NoticeKind, &'static str)> {
        if tool_allowed {
            return None;
        }
        let is_uncreated_frame = self
            .doc
            .timeline
            .layers
            .get(self.doc.timeline.active_layer)
            .is_some_and(|l| {
                l.visible && !l.locked && self.doc.timeline.current_frame >= l.frame_count
            });
        if is_uncreated_frame {
            Some((
                NoticeKind::Warning,
                "現在のフレームは未作成です（F5 / F6 / F7 でフレームを作成してください）",
            ))
        } else {
            Some((
                NoticeKind::Warning,
                "\u{e897} 現在のレイヤーはロック中または非表示のため編集できません",
            ))
        }
    }

    /// 状態連動の警告バッジおよび一時トースト通知をキャンバス左上へ統一フォーマットで縦積み描画する
    fn draw_canvas_notices(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        canvas_rect: egui::Rect,
        tool_allowed: bool,
    ) {
        let mut notices: Vec<(NoticeKind, String)> = Vec::new();
        if let Some((kind, msg)) = self.current_state_notice(tool_allowed) {
            notices.push((kind, msg.to_owned()));
        }

        if let Some(toast) = &self.doc.ui.status_toast {
            let now = Instant::now();
            if now < toast.expires_at {
                ctx.request_repaint_after(toast.expires_at.saturating_duration_since(now));
                if !notices.iter().any(|(_, m)| m == &toast.message) {
                    notices.push((toast.kind, toast.message.clone()));
                }
            } else {
                self.doc.ui.status_toast = None;
            }
        }

        let mut y_offset = 12.0_f32;
        for (kind, text) in notices {
            let (bg_col, border_col, text_col) = kind.colors();
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), text_col);
            let badge_w = (galley.size().x + 24.0).max(200.0);
            let badge_h = 26.0;
            let badge_rect = egui::Rect::from_min_size(
                canvas_rect.left_top() + Vec2::new(12.0, y_offset),
                Vec2::new(badge_w, badge_h),
            );
            painter.rect_filled(badge_rect, 0.0, bg_col);
            painter.rect_stroke(badge_rect, 0.0, Stroke::new(1.0_f32, border_col));
            let text_pos = Pos2::new(
                badge_rect.left() + 12.0,
                badge_rect.center().y - galley.size().y * 0.5,
            );
            painter.galley(text_pos, galley, text_col);
            y_offset += badge_h + 6.0;
        }
    }

    fn update_canvas_cursor(
        &self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        canvas_rect: egui::Rect,
        pointer_pos: Option<Pos2>,
        pan_state: (bool, bool),
        tool_allowed: bool,
    ) {
        let Some(pos) = pointer_pos else {
            return;
        };
        if !canvas_rect.contains(pos) {
            return;
        }
        let (is_panning, pan_gesture_active) = pan_state;
        if is_panning {
            ctx.set_cursor_icon(if pan_gesture_active {
                CursorIcon::Grabbing
            } else {
                CursorIcon::Grab
            });
        } else if !tool_allowed {
            ctx.set_cursor_icon(CursorIcon::NotAllowed);
        } else {
            panels::draw_tool_cursor_and_feedback(&self.doc, ctx, painter, pos);
        }
    }

    pub(crate) fn update_ui(&mut self, ctx: &egui::Context) {
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        project_io::handle_viewport_close_request(self, ctx);
        timeline_ui::update_playback(self, ctx);
        self.handle_shortcuts(ctx);
        self.timeline_ui.sync_with_doc(&mut self.doc);

        let mut actions = Vec::new();
        panels::draw_top_menu_bar(&mut self.doc, &mut self.timeline_ui, &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        panels::draw_left_toolbar(&mut self.doc, &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        panels::draw_right_properties_panel(&mut self.doc, &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        timeline_ui::draw_timeline_panel(self, ctx);

        project_io::update_window_title(self, ctx);
        panels::draw_document_tab_bar(&self.doc, &self.tabs, &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        panels::draw_stage_header_bar(&self.doc, &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(244, 245, 248)))
            .show(ctx, |ui| {
                let (canvas_rect, response) =
                    ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
                self.doc.viewport.last_canvas_rect = canvas_rect;
                let painter = ui.painter_at(canvas_rect);

                let pointer_pos = response
                    .interact_pointer_pos()
                    .or_else(|| ctx.pointer_hover_pos());

                // 0. ビューポートのズーム・パン移動操作ハンドリング
                let pan_state @ (is_panning, _) =
                    self.doc
                        .viewport
                        .handle_navigation(ui, canvas_rect, pointer_pos);

                let mouse_pt = pointer_pos.map(|p| self.doc.viewport.screen_to_world(p));
                let tool_allowed = self.is_current_tool_allowed();

                // 1. ツール別のマウス入力ハンドリング（パン操作開始時は進行中のドラッグをキャンセルしてスキップ）
                if is_panning {
                    canvas_tool::cancel_active_drag(&mut self.doc);
                } else {
                    self.handle_canvas_tool_input(ui, &response, mouse_pt, tool_allowed);
                }

                let in_symbol_edit = !self.doc.symbols.edit_stack.is_empty();

                // 2. ステージ背景（ペーストボード＋白いステージ矩形）の描画
                self.doc
                    .viewport
                    .draw_stage_background(&painter, canvas_rect, in_symbol_edit);

                // 3. キャンバス上のシェイプ・オニオンスキン・ステージ枠線・選択枠・プレビューの描画
                panels::draw_canvas_contents(&mut self.doc, &self.timeline_ui, &painter);

                // 4. 状態連動の警告バッジおよび一時トースト通知を描画
                self.draw_canvas_notices(ctx, &painter, canvas_rect, tool_allowed);

                // 5. カーソル表示とホバー・ドラッグ視覚フィードバック
                self.update_canvas_cursor(
                    ctx,
                    &painter,
                    canvas_rect,
                    pointer_pos,
                    pan_state,
                    tool_allowed,
                );
            });

        panels::draw_unsaved_close_confirm_modal(&self.doc, &self.tabs, &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }
    }
}

impl eframe::App for Is2dApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_ui(ctx);
    }
}

#[cfg(test)]
mod tests;
