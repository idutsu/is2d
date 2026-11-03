mod canvas_tool;
mod clipboard_ops;
mod doc_actions;
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
mod video_export;
#[cfg(target_os = "macos")]
mod video_export_macos;

use std::time::Instant;

use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Sense, Stroke, Vec2};
use kurbo::Point;

#[cfg(test)]
pub(crate) use canvas_tool::MAX_UNDO_HISTORY;
use canvas_tool::{Clipboard, NoticeKind, SelectSubMode, StatusToast, Tab, Tool, ToolSettings};
use doc_actions::{apply, Action, ActionHost};
use panels::{RightPanelTab, UiAction};
use project_io::{PendingCloseConfirm, ProjectWorkspace};
use select_ops::ArrangeMode;
use timeline_ui::{TimelineHost, TimelineUiState};
#[cfg(test)]
pub(crate) use vector_engine::FillShape;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("is2d - 無題-1")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 520.0]),
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            builder.with_default_menu(false);
        })),
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

/// アプリケーション全体のルート構造体（開いているタブ列 `tabs`・現在タブ `active`・アプリ共通のツール/クリップボード/トースト・パネルUI状態）
pub(crate) struct Is2dApp {
    /// 開いている全ドキュメントタブ（各タブが独立した `Document` / `Session` / `View` / `UndoHistory` を保持）
    pub(crate) tabs: Vec<Tab>,
    /// 現在アクティブなタブのインデックス
    pub(crate) active: usize,
    /// アプリ全体で共有するツール・ブラシサイズ・カラー設定
    pub(crate) tool: ToolSettings,
    /// アプリ全体で共有するクリップボード状態
    pub(crate) clipboard: Clipboard,
    /// アプリ全体で共有する一時トースト通知
    pub(crate) toast: Option<StatusToast>,
    /// 右端パネルで現在選択中のタブ（プロパティ / カラー / ライブラリ / キャンバス）
    pub(crate) right_panel_tab: RightPanelTab,
    /// タイムラインパネルの再生・スクロール・D&D等の一時UI状態
    pub(crate) timeline_ui: TimelineUiState,
    /// 次回新規作成される無題タブの連番（「無題-N」）
    pub(crate) next_untitled_number: usize,
    /// 次回新規作成されるドキュメントタブの一意ID
    pub(crate) next_doc_id: u64,
    /// 直近にOSウィンドウへ送信したタイトル文字列（重複送信防止用キャッシュ）
    pub(crate) last_window_title: String,
    /// 未保存変更確認モーダル（egui ウィンドウ）の表示待機状態
    pub(crate) pending_close_confirm: Option<PendingCloseConfirm>,
    /// 全タブの未保存確認が完了し、OSウィンドウのクローズを許可するフラグ
    pub(crate) allow_viewport_close: bool,
    /// バックグラウンドで進行中の動画（MP4）書き出しジョブ
    pub(crate) video_export: Option<video_export::VideoExportJob>,
}

impl Default for Is2dApp {
    fn default() -> Self {
        Self {
            tabs: vec![Tab::new_empty(1, 1)],
            active: 0,
            tool: ToolSettings::default(),
            clipboard: Clipboard::default(),
            toast: None,
            right_panel_tab: RightPanelTab::default(),
            timeline_ui: TimelineUiState::default(),
            next_untitled_number: 2,
            next_doc_id: 2,
            last_window_title: String::new(),
            pending_close_confirm: None,
            allow_viewport_close: false,
            video_export: None,
        }
    }
}

impl ActionHost for Is2dApp {
    fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    fn tool(&self) -> &ToolSettings {
        &self.tool
    }

    fn tool_mut(&mut self) -> &mut ToolSettings {
        &mut self.tool
    }

    fn clipboard_mut(&mut self) -> &mut Clipboard {
        &mut self.clipboard
    }

    fn toast_mut(&mut self) -> &mut Option<StatusToast> {
        &mut self.toast
    }

    fn parts_mut(
        &mut self,
    ) -> (
        &mut Tab,
        &mut ToolSettings,
        &mut Clipboard,
        &mut Option<StatusToast>,
    ) {
        (
            &mut self.tabs[self.active],
            &mut self.tool,
            &mut self.clipboard,
            &mut self.toast,
        )
    }

    fn stop_playback(&mut self) {
        self.timeline_ui.stop_playback(&mut self.tabs[self.active]);
    }
}

impl ProjectWorkspace for Is2dApp {
    fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    fn tabs_mut(&mut self) -> &mut Vec<Tab> {
        &mut self.tabs
    }

    fn active_index(&self) -> usize {
        self.active
    }

    fn set_active_index(&mut self, index: usize) {
        self.active = index;
    }

    fn alloc_doc_id(&mut self) -> u64 {
        let id = self.next_doc_id;
        self.next_doc_id = self.next_doc_id.saturating_add(1);
        id
    }

    fn alloc_untitled_number(&mut self) -> usize {
        let num = self.next_untitled_number;
        self.next_untitled_number = self.next_untitled_number.saturating_add(1);
        num
    }

    fn set_next_untitled_number(&mut self, num: usize) {
        self.next_untitled_number = num;
    }

    fn last_window_title(&self) -> &str {
        &self.last_window_title
    }

    fn set_last_window_title(&mut self, title: String) {
        self.last_window_title = title;
    }

    fn pending_close_confirm(&self) -> &Option<PendingCloseConfirm> {
        &self.pending_close_confirm
    }

    fn pending_close_confirm_mut(&mut self) -> &mut Option<PendingCloseConfirm> {
        &mut self.pending_close_confirm
    }

    fn allow_viewport_close(&self) -> bool {
        self.allow_viewport_close
    }

    fn set_allow_viewport_close(&mut self, allow: bool) {
        self.allow_viewport_close = allow;
    }
}

impl TimelineHost for Is2dApp {
    fn timeline_ui(&self) -> &TimelineUiState {
        &self.timeline_ui
    }

    fn timeline_ui_mut(&mut self) -> &mut TimelineUiState {
        &mut self.timeline_ui
    }

    fn tab_and_timeline_ui_mut(&mut self) -> (&mut Tab, &mut TimelineUiState) {
        (&mut self.tabs[self.active], &mut self.timeline_ui)
    }
}

impl Is2dApp {
    pub(crate) fn undo(&mut self) {
        apply(self, Action::Undo);
    }

    pub(crate) fn redo(&mut self) {
        apply(self, Action::Redo);
    }

    /// 保存先選択ダイアログを表示し、現在のタブの動画（MP4）書き出しをバックグラウンドで開始する
    pub(crate) fn export_video_dialog(&mut self) {
        if self.video_export.is_some() {
            return;
        }
        project_io::prepare_active_tab_for_switch_or_save(self);

        let stem = self
            .tab()
            .file
            .as_deref()
            .and_then(std::path::Path::file_stem)
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| self.tab().base_title());
        let default_name = format!("{stem}.mp4");

        let mut dialog = rfd::FileDialog::new()
            .add_filter("MP4 動画 (*.mp4)", &["mp4"])
            .set_file_name(&default_name);
        if let Some(parent) = self.tab().file.as_deref().and_then(std::path::Path::parent) {
            dialog = dialog.set_directory(parent);
        }

        let Some(mut path) = dialog.save_file() else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("mp4");
        }

        let doc = self.tab().committed_document_copy();
        self.video_export = Some(video_export::VideoExportJob::start(doc, path));
    }

    fn finish_video_export_outcome(
        &mut self,
        path: &std::path::Path,
        outcome: video_export::VideoExportOutcome,
    ) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("動画");
        match outcome {
            video_export::VideoExportOutcome::Completed => {
                self.notify_info(format!("「{name}」を書き出しました"));
            }
            video_export::VideoExportOutcome::Cancelled => {
                self.notify_info("動画の書き出しを中止しました");
            }
            video_export::VideoExportOutcome::Failed(err) => {
                self.notify_error(format!("動画の書き出しに失敗しました: {err}"));
            }
        }
    }

    /// 進行中の動画書き出しを中止し、裏の処理が終わるまで待ってからトースト通知を出す
    pub(crate) fn cancel_active_video_export(&mut self) {
        if let Some(mut job) = self.video_export.take() {
            let path = job.target_path.clone();
            let outcome = job.cancel_and_wait();
            self.finish_video_export_outcome(&path, outcome);
        }
    }

    fn poll_video_export(&mut self, ctx: &egui::Context) {
        let Some(outcome) = self
            .video_export
            .as_mut()
            .and_then(video_export::VideoExportJob::try_take_outcome)
        else {
            if self.video_export.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
            return;
        };
        if let Some(job) = self.video_export.take() {
            self.finish_video_export_outcome(&job.target_path, outcome);
        }
    }

    /// UIパネルから収集した `UiAction` を一括ディスパッチする
    pub(crate) fn apply_action(&mut self, action: UiAction, ctx: &egui::Context) {
        if self.video_export.is_some() {
            match action {
                UiAction::CancelVideoExport => self.cancel_active_video_export(),
                UiAction::CloseActiveTab => {
                    self.cancel_active_video_export();
                    project_io::close_active_tab(self, Some(ctx));
                }
                UiAction::CloseTab(idx) => {
                    self.cancel_active_video_export();
                    project_io::close_tab(self, idx, Some(ctx));
                }
                _ => {}
            }
            return;
        }
        match action {
            UiAction::NewProject => project_io::new_project(self, ctx),
            UiAction::OpenProjectDialog => project_io::open_project_dialog(self, ctx),
            UiAction::SaveProject => project_io::save_project(self, ctx),
            UiAction::SaveProjectAsDialog => project_io::save_project_as_dialog(self, ctx),
            UiAction::ExportVideoDialog => self.export_video_dialog(),
            UiAction::CancelVideoExport => self.cancel_active_video_export(),
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
            UiAction::ResetView => {
                self.tab_mut().view.reset_view();
            }
            UiAction::FitStageToWindow => {
                self.tab_mut().fit_stage_to_window();
            }
            UiAction::SetSymbolPlayback {
                loop_mode,
                first_frame,
                drag_active,
            } => {
                apply(
                    self,
                    Action::SetSymbolPlayback {
                        loop_mode: Some(loop_mode),
                        first_frame: Some(first_frame),
                    },
                );
                if !drag_active {
                    apply(self, Action::EndCoalesce { key: None });
                }
            }
            UiAction::SelectLibrarySymbol(sym_id) => {
                self.tab_mut().view.selected_library_symbol = Some(sym_id);
            }
            UiAction::Doc(action) => {
                apply(self, action);
            }
        }
    }

    // ------------------------------------------------------------------------
    // 2. キーボードショートカット制御
    // ------------------------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.pending_close_confirm.is_some() {
            return;
        }
        let input = ctx.input(|i| i.clone());
        let cmd = input.modifiers.command || input.modifiers.ctrl;
        if self.video_export.is_some() {
            if cmd && key_pressed_no_repeat(&input, Key::W) {
                self.cancel_active_video_export();
                project_io::close_active_tab(self, Some(ctx));
            } else if cmd && key_pressed_no_repeat(&input, Key::Q) {
                self.cancel_active_video_export();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        if cmd && key_pressed_no_repeat(&input, Key::N) {
            project_io::new_project(self, ctx);
            return;
        } else if cmd && key_pressed_no_repeat(&input, Key::O) {
            project_io::open_project_dialog(self, ctx);
            return;
        } else if cmd && key_pressed_no_repeat(&input, Key::S) {
            if input.modifiers.shift {
                project_io::save_project_as_dialog(self, ctx);
            } else {
                project_io::save_project(self, ctx);
            }
            return;
        } else if cmd && key_pressed_no_repeat(&input, Key::W) {
            project_io::close_active_tab(self, Some(ctx));
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

        self.handle_command_shortcuts(ctx, &input, cmd, copy_event, cut_event, paste_event);

        if !cmd && !copy_event && !cut_event && !paste_event {
            self.handle_single_key_shortcuts(&input, input.modifiers.shift);
        }

        if key_pressed_no_repeat(&input, Key::Delete)
            || key_pressed_no_repeat(&input, Key::Backspace)
        {
            apply(self, Action::DeleteSelection);
        }

        if cmd {
            self.clipboard.cmd_held_grace_frames = 4;
        } else {
            self.clipboard.cmd_held_grace_frames =
                self.clipboard.cmd_held_grace_frames.saturating_sub(1);
        }
    }

    fn handle_single_key_shortcuts(&mut self, i: &egui::InputState, shift: bool) {
        if !self.tab().session.selection.is_empty() {
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
                    apply(
                        self,
                        Action::EndCoalesce {
                            key: Some(canvas_tool::CoalesceKey::NudgeSelection),
                        },
                    );
                }
                apply(self, Action::NudgeSelection { delta });
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
            let layer_idx = self.tab().active_timeline().active_layer;
            apply(
                self,
                Action::SwitchLayerAndFrame {
                    layer_idx,
                    frame: 0,
                },
            );
        } else if key_pressed_no_repeat(i, Key::End) {
            let tl = self.tab().active_timeline();
            let frame = tl.total_frames.saturating_sub(1);
            let layer_idx = tl.active_layer;
            apply(self, Action::SwitchLayerAndFrame { layer_idx, frame });
        } else if i.key_pressed(Key::F6) {
            if shift {
                apply(self, Action::DeleteKeyframe);
            } else {
                apply(self, Action::InsertKeyframe { copy_prev: true });
            }
        } else if i.key_pressed(Key::F7) {
            apply(self, Action::InsertKeyframe { copy_prev: false });
        } else if key_pressed_no_repeat(i, Key::F8) {
            apply(self, Action::ConvertToSymbol);
        } else if i.key_pressed(Key::F5) {
            if shift {
                apply(self, Action::RemoveFrame);
            } else {
                apply(self, Action::ExtendFrame);
            }
        } else if key_pressed_no_repeat(i, Key::O) {
            let next = !self.tab().view.onion_skin_enabled;
            self.tab_mut().view.onion_skin_enabled = next;
        } else if key_pressed_no_repeat(i, Key::T) {
            apply(self, Action::ToggleTween);
        } else if let Some(tool) = Tool::from_shortcut_input(i) {
            apply(self, Action::SetTool { tool });
        } else if key_pressed_no_repeat(i, Key::Escape) {
            canvas_tool::cancel_active_drag(self.tab_mut());
            if !self.tab().session.selection.is_empty() {
                apply(self, Action::Deselect);
            } else {
                apply(self, Action::ExitGroup);
            }
            self.tool.select_sub_mode = SelectSubMode::Reshape;
        } else if self.tool.tool == Tool::Select {
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
                apply(
                    self,
                    Action::ToggleSelectSubMode {
                        mode: SelectSubMode::AddAnchor,
                    },
                );
            } else if minus_pressed {
                apply(
                    self,
                    Action::ToggleSelectSubMode {
                        mode: SelectSubMode::DeleteAnchor,
                    },
                );
            }
        }
    }

    fn handle_command_shortcuts(
        &mut self,
        ctx: &egui::Context,
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
        let v_released_fallback = (cmd || self.clipboard.cmd_held_grace_frames > 0)
            && v_release_event
            && !self.clipboard.paste_triggered_on_press;
        if v_release_event {
            self.clipboard.paste_triggered_on_press = false;
            self.clipboard.cmd_held_grace_frames = 0;
        }

        if cmd && key_pressed_no_repeat(i, Key::Q) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if copy_event || (cmd && !i.modifiers.shift && key_pressed_no_repeat(i, Key::C)) {
            apply(self, Action::Copy);
        } else if cut_event || (cmd && !i.modifiers.shift && key_pressed_no_repeat(i, Key::X)) {
            apply(self, Action::Cut);
        } else if paste_event || v_pressed_no_repeat {
            self.clipboard.paste_triggered_on_press = true;
            apply(
                self,
                Action::Paste {
                    in_place: i.modifiers.shift,
                },
            );
        } else if v_released_fallback {
            apply(
                self,
                Action::Paste {
                    in_place: i.modifiers.shift,
                },
            );
        } else if cmd
            && (key_pressed_no_repeat(i, Key::Num1) || key_pressed_no_repeat(i, Key::Num0))
        {
            self.tab_mut().view.reset_view();
        } else if cmd && key_pressed_no_repeat(i, Key::Num2) {
            let stage_size = self.tab().doc.stage_size;
            self.tab_mut().view.fit_stage_to_window(stage_size);
        } else if cmd && key_pressed_no_repeat(i, Key::Num3) {
            let next = !self.tab().view.clip_outside_stage;
            self.tab_mut().view.clip_outside_stage = next;
        } else if cmd
            && (i.key_pressed(Key::Plus)
                || i.key_pressed(Key::Equals)
                || i.key_pressed(Key::Semicolon))
        {
            let center = i
                .pointer
                .hover_pos()
                .unwrap_or_else(|| self.tab().view.last_canvas_rect.center());
            self.tab_mut().view.zoom_around_screen_pos(center, 1.25);
        } else if cmd && i.key_pressed(Key::Minus) {
            let center = i
                .pointer
                .hover_pos()
                .unwrap_or_else(|| self.tab().view.last_canvas_rect.center());
            self.tab_mut().view.zoom_around_screen_pos(center, 0.8);
        } else if cmd && key_pressed_no_repeat(i, Key::G) {
            if i.modifiers.shift {
                apply(self, Action::Ungroup);
            } else {
                apply(self, Action::Group);
            }
        } else if cmd && key_pressed_no_repeat(i, Key::B) {
            apply(self, Action::Ungroup);
        } else if cmd && key_pressed_no_repeat(i, Key::ArrowUp) {
            let mode = if i.modifiers.shift {
                ArrangeMode::ToFront
            } else {
                ArrangeMode::Forward
            };
            apply(self, Action::Arrange { mode });
        } else if cmd && key_pressed_no_repeat(i, Key::ArrowDown) {
            let mode = if i.modifiers.shift {
                ArrangeMode::ToBack
            } else {
                ArrangeMode::Backward
            };
            apply(self, Action::Arrange { mode });
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
        match self.tool.tool {
            Tool::Eyedropper => true,
            Tool::Brush | Tool::Eraser | Tool::Bucket => {
                self.tab().active_timeline().is_active_layer_editable()
            }
            Tool::Select | Tool::Transform => self.tab().active_timeline().has_any_editable_layer(),
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
                self.stop_playback();
                apply(self, Action::EndCoalesce { key: None });
            } else {
                return;
            }
        }
        if self.tool.tool == Tool::Eyedropper {
            let sampling = (response.hovered() && ui.input(|i| i.pointer.primary_pressed()))
                || response.dragged_by(egui::PointerButton::Primary)
                || response.clicked_by(egui::PointerButton::Primary);
            if sampling {
                if let Some(pt) = mouse_pt {
                    tool_paint::pick_color_at(self, pt);
                }
            }
        } else if tool_allowed {
            match self.tool.tool {
                Tool::Brush | Tool::Eraser => {
                    tool_paint::handle_brush_or_eraser_input(self, response, mouse_pt);
                }
                Tool::Bucket => {
                    let just_pressed =
                        response.hovered() && ui.input(|i| i.pointer.primary_pressed());
                    if just_pressed {
                        if let Some(pt) = mouse_pt {
                            let color = self.tool.brush_color;
                            apply(self, Action::BucketFill { pt, color });
                        }
                    }
                }
                Tool::Eyedropper => {}
                Tool::Select => {
                    tool_select::handle_select_tool_input(self, ui, response, mouse_pt);
                }
                Tool::Transform => {
                    tool_transform::handle_transform_tool_input(self, ui, response, mouse_pt);
                }
            }
        }
    }

    /// 現在のレイヤー・フレーム状態から常時表示すべき状態連動メッセージを判定する
    fn current_state_notice(&self, tool_allowed: bool) -> Option<(NoticeKind, &'static str)> {
        if tool_allowed {
            return None;
        }
        let tl = self.tab().active_timeline();
        let active_layer = tl.layers.get(tl.active_layer);
        let is_uncreated_frame = active_layer
            .is_some_and(|l| l.visible && !l.locked && tl.current_frame >= l.frame_count);
        let is_tween_mid = active_layer.is_some_and(|l| {
            l.visible
                && !l.locked
                && tl.current_frame < l.frame_count
                && l.interpolated_tween_binding_at(tl.current_frame).is_some()
        });
        if is_uncreated_frame {
            Some((
                NoticeKind::Warning,
                "現在のフレームは未作成です（F5 / F6 / F7 でフレームを作成してください）",
            ))
        } else if is_tween_mid {
            Some((
                NoticeKind::Warning,
                "トゥイーンの途中のコマは編集できません（F6 でキーフレームを挿入してください）",
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

        if let Some(toast) = &self.toast {
            let now = Instant::now();
            if now < toast.expires_at {
                ctx.request_repaint_after(toast.expires_at.saturating_duration_since(now));
                if !notices.iter().any(|(_, m)| m == &toast.message) {
                    notices.push((toast.kind, toast.message.clone()));
                }
            } else {
                self.toast = None;
            }
        }

        let mut y_offset = 12.0_f32;
        for (kind, text) in notices {
            let (bg_col, border_col, text_col) = panels::notice_kind_colors(kind);
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
        };
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
            panels::draw_tool_cursor_and_feedback(self.tab(), &self.tool, ctx, painter, pos);
        }
    }

    pub(crate) fn update_ui(&mut self, ctx: &egui::Context) {
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        if ctx.input(|i| i.viewport().close_requested()) && self.video_export.is_some() {
            self.cancel_active_video_export();
        }
        self.poll_video_export(ctx);
        project_io::handle_viewport_close_request(self, ctx);
        timeline_ui::update_playback(self, ctx);
        self.handle_shortcuts(ctx);

        let mut actions = Vec::new();
        panels::draw_top_menu_bar(
            &mut self.tabs[self.active],
            &self.clipboard,
            &mut actions,
            ctx,
        );
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        panels::draw_left_toolbar(&mut self.tool, &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        panels::draw_right_properties_panel(
            &mut self.tabs[self.active],
            &mut self.tool,
            &mut self.right_panel_tab,
            &mut actions,
            ctx,
        );
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        timeline_ui::draw_timeline_panel(self, ctx);

        project_io::update_window_title(self, ctx);
        panels::draw_document_tab_bar(&self.tabs, self.active, &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        panels::draw_stage_header_bar(&self.tabs[self.active], &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(244, 245, 248)))
            .show(ctx, |ui| {
                let (canvas_rect, response) =
                    ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
                self.tab_mut().view.last_canvas_rect = canvas_rect;
                let painter = ui.painter_at(canvas_rect);

                let pointer_pos = response
                    .interact_pointer_pos()
                    .or_else(|| ctx.pointer_hover_pos());

                // 0. ビューポートのズーム・パン移動操作ハンドリング
                let modal_open =
                    self.pending_close_confirm.is_some() || self.video_export.is_some();
                let pan_state @ (is_panning, _) = if modal_open {
                    self.tab_mut().view.pan_last_screen_pos = None;
                    (false, false)
                } else {
                    self.tab_mut()
                        .view
                        .handle_navigation(ui, canvas_rect, pointer_pos)
                };

                let mouse_pt = pointer_pos.map(|p| self.tab().view.screen_to_world(p));
                let tool_allowed = !modal_open && self.is_current_tool_allowed();

                // 1. ツール別のマウス入力ハンドリング（パン操作開始時・モーダル表示時は進行中のドラッグをキャンセルしてスキップ）
                if is_panning || modal_open {
                    canvas_tool::cancel_active_drag(self.tab_mut());
                } else {
                    self.handle_canvas_tool_input(ui, &response, mouse_pt, tool_allowed);
                }

                let in_symbol_edit = !self.tab().session.edit_path.is_empty();
                let stage_size = self.tab().doc.stage_size;

                // 2. ステージ背景（ペーストボード＋白いステージ矩形）の描画
                self.tab().view.draw_stage_background(
                    &painter,
                    canvas_rect,
                    stage_size,
                    in_symbol_edit,
                );

                // 3. キャンバス上のシェイプ・オニオンスキン・ステージ枠線・選択枠・プレビューの描画
                panels::draw_canvas_contents(self.tab(), &self.tool, &self.timeline_ui, &painter);

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

        panels::draw_unsaved_close_confirm_modal(
            &self.tabs,
            &self.pending_close_confirm,
            &mut actions,
            ctx,
        );
        panels::draw_video_export_modal(
            self.video_export.as_ref().map(|j| j.progress()),
            &mut actions,
            ctx,
        );
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
