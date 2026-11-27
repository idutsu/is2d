#[cfg(target_os = "macos")]
mod app_menu_macos;
#[cfg(target_os = "macos")]
mod audio_latency_macos;
mod audio_out;
mod canvas_tool;
// 作業の部屋（`canvas_tool`）の中のファイルを、今までの名前で呼べるようにする
mod chunked_list;
mod color_picker;
mod floating;
mod guard;
mod map_hover;
mod menu;
mod panels;
mod planar_map;
mod prefetch;
mod project_io;
mod recovery;
mod recovery_journal;
mod save_format;
mod screen_mesh;
mod serial;
mod sound;
mod stored_map;
mod sync_cache;
mod symbol;
mod symbol_binding;
mod timeline;
mod timeline_ui;
mod tool_paint;
mod tool_select;
mod tool_transform;
mod vector_engine;
mod vector_stroke;
mod video_export;
#[cfg(target_os = "macos")]
mod video_export_macos;
#[cfg(target_os = "windows")]
mod session_end_windows;
#[cfg(target_os = "windows")]
mod video_export_windows;

use std::time::Instant;

use eframe::egui::{self, Color32, CursorIcon, Pos2, Sense, Stroke, Vec2};
use kurbo::Point;

use canvas_tool::{
    Clipboard, NoticeKind, SelectSubMode, Notices, Tab, Tool,
    ToolSettings,
};
use canvas_tool::doc_actions::{apply, Action, ActionHost};
use panels::{ArrowDir, FrameEnd, RightPanelTab, UiAction, ZoomStep};
use project_io::{PendingCloseConfirm, ProjectWorkspace};
use timeline::EditBlock;
use timeline_ui::{TimelineHost, TimelineUiState};

/// 知らせの描く文：取り消しの案内の印があれば、メニューの表から作った案内を付ける（`menu::undo_exit_text`）
fn shown_notice_text(n: &canvas_tool::NoticeText) -> String {
    if n.undo_exit {
        format!("{}{}", n.text, menu::undo_exit_text())
    } else {
        n.text.clone()
    }
}

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("is2d")
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
        // アプリの始まりの入口：手順ごとに砦の中で走らせる（止まった手順は飛ばして始め、知らせる：設計
        // claude/最後の砦を仕事の入口に置く設計.md）
        Box::new(|cc| {
            let mut app = Is2dApp::default();
            let mut step = |name: &str, f: &mut dyn FnMut(&mut Is2dApp)| {
                if guard::guarded(|| f(&mut app)).is_none() {
                    app.notify_warning(format!("内部の不具合で、始めるときの「{name}」を飛ばしました"));
                }
            };
            step("文字と色の用意", &mut |_| {
                configure_japanese_fonts(&cc.egui_ctx);
                configure_sharp_light_theme(&cc.egui_ctx);
            });
            #[cfg(target_os = "macos")]
            step("メニュー", &mut |app| app.os_menu = app_menu_macos::install(&cc.egui_ctx));
            step("音の装置", &mut |app| app.timeline_ui.audio = audio_out::AudioOut::open(&cc.egui_ctx));
            step("復旧ファイルの用意", &mut |app| {
                app.recovery.dir = recovery::default_recovery_dir();
                app.recovery.find_leftovers();
                app.recovery_writer = app.recovery.dir.clone().map(recovery::RecoveryWriter::start);
            });
            // Windows のログアウト・再起動・シャットダウンでも、止められる前に復旧ファイルを書く（winit はその知らせを受け
            // 取らないので、窓の受け口を包む）
            #[cfg(target_os = "windows")]
            step("終わるときの控え", &mut |app| {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Some(RawWindowHandle::Win32(w)) = cc.window_handle().ok().map(|h| h.as_raw()) {
                    if let Some(writer) = app.recovery_writer.clone() {
                        session_end_windows::install(w.hwnd.get(), writer, app.tab_copies.clone());
                    }
                }
            });
            Ok(Box::new(app))
        }),
    )
}

/// `fonts/` フォルダ内の日本語フォント（NotoSansJP）とアイコンフォント（MaterialIcons）を読み込む。文字を描くフォントは
/// この2つだけ（egui の既定のフォントは入れない：Cargo.toml。画面の字はみなこの2つにある）。太字のフォントは持たない
/// （egui は同じ字を持つフォントを探し順の後ろから選ばず、`.strong()` も色を変えるだけなので、使われない：レビュー
/// f01d518 の N-5006）
fn configure_japanese_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::empty();

    fonts.font_data.insert(
        "noto_sans_jp_regular".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../fonts/NotoSansJP-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "material_icons".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../fonts/MaterialIcons-Regular.ttf"
        ))),
    );

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.insert(
            family,
            vec![
                "noto_sans_jp_regular".to_owned(),
                "material_icons".to_owned(),
            ],
        );
    }

    ctx.set_fonts(fonts);
}

/// アプリ全体のUIをライトテーマ＋角丸なし（すべて直角のシャープな矩形）に設定する
fn configure_sharp_light_theme(ctx: &egui::Context) {
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
    let mut visuals = egui::Visuals::light();
    visuals.window_corner_radius = egui::CornerRadius::ZERO;
    visuals.menu_corner_radius = egui::CornerRadius::ZERO;
    visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::ZERO;
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
    visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
    visuals.widgets.open.corner_radius = egui::CornerRadius::ZERO;
    ctx.set_visuals(visuals);
}

// ============================================================================
// メインアプリケーション状態 & ショートカット・アクションディスパッチ制御
// ============================================================================

/// 出ている間、ほかの操作を止めるダイアログ（`Is2dApp::shown_dialog`）
#[derive(Clone, Copy)]
enum Dialog {
    /// 動画の書き出し中（書いたコマの数, 全部のコマの数）
    VideoExport((usize, usize)),
    Recovery,
    CloseConfirm,
}

impl Dialog {
    /// このダイアログが出ている間も受け付ける操作か（ダイアログのボタンと、動画の書き出し中にタブを閉じること、どの
    /// ダイアログでも終了：持ち主が決めた 2026-11-26。Mac の OS のメニューの終了と同じ）。
    /// 操作を受け付けるか（`Is2dApp::apply_action`）と、メニューの項目を使えるか（`Is2dApp::menu_item_state`）は、
    /// これ1つで決める
    fn accepts(self, action: &UiAction) -> bool {
        match self {
            Dialog::VideoExport(_) => matches!(
                action,
                UiAction::CancelVideoExport
                    | UiAction::CloseActiveTab
                    | UiAction::CloseTab(_)
                    | UiAction::Quit
            ),
            Dialog::Recovery => {
                matches!(
                    action,
                    UiAction::RestoreRecovered | UiAction::DiscardRecovered | UiAction::Quit
                )
            }
            Dialog::CloseConfirm => matches!(
                action,
                UiAction::ConfirmPendingCloseSave
                    | UiAction::ConfirmPendingCloseDiscard
                    | UiAction::ConfirmPendingCloseCancel
                    | UiAction::Quit
            ),
        }
    }
}

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
    pub(crate) notices: Notices,
    /// 右端パネルで現在選択中のタブ（プロパティ / カラー / ライブラリ / キャンバス）
    pub(crate) right_panel_tab: RightPanelTab,
    /// タイムラインパネルの再生・スクロール・D&D等の一時UI状態
    pub(crate) timeline_ui: TimelineUiState,
    /// 次回新規作成される無題タブの連番（「無題-N」）
    pub(crate) next_untitled_number: usize,
    /// タブの番号を配る係（`canvas_tool::TabIds`）
    pub(crate) tab_ids: canvas_tool::TabIds,
    /// 直近にOSウィンドウへ送信したタイトル文字列（重複送信防止用キャッシュ）
    pub(crate) last_window_title: String,
    /// 未保存変更確認モーダル（egui ウィンドウ）の表示待機状態
    pub(crate) pending_close_confirm: Option<PendingCloseConfirm>,
    /// 全タブの未保存確認が完了し、OSウィンドウのクローズを許可するフラグ
    pub(crate) allow_viewport_close: bool,
    /// バックグラウンドで進行中の動画（MP4）書き出しジョブ
    pub(crate) video_export: Option<video_export::VideoExportJob>,
    /// 確認なし終了時の復旧ファイル書き出し・次回起動時の復元管理状態
    pub(crate) recovery: recovery::Recovery,
    /// 開いているタブの控え（毎コマの終わりに、変わったタブがあれば控え直して、裏の書き手に渡す。Windows のセッションの
    /// 終わりも、これを書く：`session_end_windows`）
    pub(crate) tab_copies: recovery::TabCopies,
    /// 復旧ファイルの裏の書き手（復旧の置き場があるときだけ。テストの既定では無い）
    pub(crate) recovery_writer: Option<recovery::RecoveryWriter>,
    /// 先読みの係（次に見るセル画の写しを、裏で作って持つ：`prefetch`）
    pub(crate) prefetch: prefetch::Prefetch,
    /// 出ている読み込みの知らせに足し合わせた、ファイルから読んだものの知らせ（知らせが消えたら捨てる）
    pub(crate) shown_load_notices: Vec<timeline::LoadNotice>,
    /// 砦の外まで止まって終わった裏のスレッドの記録を、どこまで読んだか（毎コマ読んで知らせる）
    pub(crate) stopped_threads: guard::StoppedThreads,
    /// 直近フレームの中央キャンバス描画領域（スクリーン座標。画面に1つだけで、どのタブでも同じ）
    pub(crate) canvas_rect: egui::Rect,
    /// OS のメニューバーの「ファイル」「編集」「表示」（Mac。起動のときに作る。テストでは作らない）
    #[cfg(target_os = "macos")]
    pub(crate) os_menu: Option<app_menu_macos::OsMenu>,
}

impl Default for Is2dApp {
    fn default() -> Self {
        let mut tab_ids = canvas_tool::TabIds::default();
        let first = tab_ids.fresh();
        Self {
            tabs: vec![Tab::new_empty(first, 1)],
            active: 0,
            tool: ToolSettings::default(),
            clipboard: Clipboard::default(),
            notices: Notices::default(),
            right_panel_tab: RightPanelTab::default(),
            timeline_ui: TimelineUiState::default(),
            next_untitled_number: 2,
            tab_ids,
            last_window_title: String::new(),
            pending_close_confirm: None,
            allow_viewport_close: false,
            video_export: None,
            recovery: recovery::Recovery::default(),
            tab_copies: recovery::TabCopies::default(),
            recovery_writer: None,
            prefetch: prefetch::Prefetch::default(),
            shown_load_notices: Vec::new(),
            stopped_threads: guard::StoppedThreads::default(),
            canvas_rect: egui::Rect::from_min_max(Pos2::new(52.0, 30.0), Pos2::new(1000.0, 590.0)),
            #[cfg(target_os = "macos")]
            os_menu: None,
        }
    }
}

impl Is2dApp {
    /// 今のタブのステージ全体が、キャンバス領域に収まるようズーム・パンを調整する（「ウィンドウに合わせる」）
    pub(crate) fn fit_stage_to_window(&mut self) {
        let (stage_size, rect) = (self.tab().doc().stage_size, self.canvas_rect);
        self.tab_mut().view.fit_stage_to_window(stage_size, rect);
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

    fn notices_mut(&mut self) -> &mut Notices {
        &mut self.notices
    }

    fn parts_mut(
        &mut self,
    ) -> (
        &mut Tab,
        &mut ToolSettings,
        &mut Clipboard,
        &mut Notices,
    ) {
        (
            &mut self.tabs[self.active],
            &mut self.tool,
            &mut self.clipboard,
            &mut self.notices,
        )
    }

    fn stop_playback(&mut self) {
        self.timeline_ui.stop_playback(&mut self.tabs[self.active]);
    }

    fn restart_playback(&mut self) {
        self.timeline_ui.restart_playback(&self.tabs[self.active]);
    }

    fn dialog_shown(&self) -> bool {
        self.shown_dialog().is_some()
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

    fn alloc_doc_id(&mut self) -> canvas_tool::FreshTabId {
        self.tab_ids.fresh()
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

    fn send_ui_action(&mut self, action: UiAction, ctx: &egui::Context) {
        self.apply_action(action, ctx);
    }
}

impl Is2dApp {
    /// テストで元に戻す（アプリではキーとメニューから `UiAction::Doc(Action::Undo)` が来る）
    #[cfg(test)]
    pub(crate) fn undo(&mut self) {
        apply(self, Action::Undo);
    }

    /// テストでやり直す
    #[cfg(test)]
    pub(crate) fn redo(&mut self) {
        apply(self, Action::Redo);
    }

    /// 保存先選択ダイアログを表示し、現在のタブの動画（MP4）書き出しをバックグラウンドで開始する
    /// 音のファイル（WAV・MP3）を選んで読み、今のコマのキーフレームに付ける（`Action::ImportSound`）。読めなければ知らせる
    pub(crate) fn import_sound_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("音 (*.wav, *.mp3)", &["wav", "mp3"])
            .pick_file()
        else {
            return;
        };
        let name = save_format::file_name_or(&path, "音").to_owned();
        match std::fs::read(&path) {
            Ok(bytes) => {
                apply(
                    self,
                    Action::ImportSound {
                        name,
                        bytes: bytes.into(),
                    },
                );
            }
            Err(e) => self.notify_error(format!("音のファイルを読めません: {e}")),
        }
    }

    pub(crate) fn export_video_dialog(&mut self) {
        if self.video_export.is_some() {
            return;
        }
        project_io::prepare_active_tab_for_switch_or_save(self);

        let default_name = format!("{}.mp4", self.tab().name_stem());

        let Some(path) =
            project_io::pick_save_path(self.tab(), "MP4 動画 (*.mp4)", "mp4", &default_name)
        else {
            return;
        };

        // 浮いた片を置けなければ、画面に見えている片が入らない動画は書き出さない
        let Some(doc) = self.tab().committed_document_copy() else {
            self.notify_warning(canvas_tool::NoticeText::with_undo_exit("選んだ図形を置けないため書き出せません。"));
            return;
        };
        match video_export::VideoExportJob::start(doc, path) {
            Ok(job) => self.video_export = Some(job),
            Err(e) => self.notify_warning(format!("書き出しを始められませんでした（{e}）")),
        }
    }

    fn finish_video_export_outcome(
        &mut self,
        path: &std::path::Path,
        outcome: video_export::VideoExportOutcome,
    ) {
        let name = save_format::file_name_or(path, "動画");
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

    /// いま出ているダイアログ（いちばん上に描くもの：動画の書き出し・復旧・閉じる前の確認の順）。出ている間は、その
    /// ダイアログの操作のほか（ほかの UI の操作・ショートカット・キャンバスの操作）を止める。判定はこれ1つ
    fn shown_dialog(&self) -> Option<Dialog> {
        if let Some(job) = self.video_export.as_ref().filter(|j| !j.is_cancelled()) {
            Some(Dialog::VideoExport(job.progress()))
        } else if !self.recovery.found.is_empty() {
            Some(Dialog::Recovery)
        } else if self.pending_close_confirm.is_some() {
            Some(Dialog::CloseConfirm)
        } else {
            None
        }
    }

    /// 進行中の動画書き出しに中止の合図を送る（裏の処理の終わりと通知は `poll_video_export` が受け取る）
    pub(crate) fn cancel_active_video_export(&mut self) {
        if let Some(job) = &self.video_export {
            job.cancel();
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
        if self.shown_dialog().is_none_or(|d| d.accepts(&action)) {
            self.apply_ui_action(action, ctx);
        }
    }

    /// メニューの項目 `index`（`menu::COMMANDS` の番号）の今の見せ方。入力欄へ渡すとき（`MenuCommand::text_op_for`）は
    /// いつも使え、アプリへ渡すときは、ダイアログが受け付け（`Dialog::accepts`）、表の決まりで使えるときだけ使える
    fn menu_item_state(&self, index: usize, typing: bool) -> menu::ItemState {
        let command = &menu::COMMANDS[index];
        let enabled = command.text_op_for(typing).is_some()
            || (self
                .shown_dialog()
                .is_none_or(|d| d.accepts(&command.action))
                && (command.enabled)(self.tab(), &self.clipboard));
        menu::ItemState {
            enabled,
            checked: command.checked(&self.tab().view),
        }
    }

    /// メニューの全項目の今の見せ方（`menu::COMMANDS` と同じ並び）
    fn menu_states(&self, ctx: &egui::Context) -> [menu::ItemState; menu::COMMAND_COUNT] {
        let typing = ctx.wants_keyboard_input();
        std::array::from_fn(|index| self.menu_item_state(index, typing))
    }

    /// `UiAction` の適用（受け付けるかは `apply_action` が決める。ダイアログが出ているときと出ていないときで同じ関数：
    /// 前は2つの関数に同じ扱いを書き、ダイアログの側は書き忘れると黙って何もしなかった：約束の棚卸し 5616fcc の C の
    /// D5・E1）。タブを閉じるときは、書き出しを止める（書き出していなければ何もしない）。ダイアログのボタンの操作は、
    /// そのダイアログが無いときは何もしない（確かめるものが無い）
    fn apply_ui_action(&mut self, action: UiAction, ctx: &egui::Context) {
        match action {
            UiAction::SelectLibrarySymbol(id) => self.tab_mut().view.select_library_symbol(id),
            UiAction::NewProject => project_io::new_project(self, ctx),
            UiAction::ResetView => self.tab_mut().view.reset_view(),
            UiAction::FitStageToWindow => self.fit_stage_to_window(),
            UiAction::ToggleView(switch) => self.tab_mut().view.toggle(switch),
            UiAction::Zoom(step) => {
                let factor = match step {
                    ZoomStep::In => 1.25,
                    ZoomStep::Out => 0.8,
                };
                let center = ctx
                    .input(|i| i.pointer.hover_pos())
                    .unwrap_or_else(|| self.canvas_rect.center());
                self.tab_mut().view.zoom_around_screen_pos(center, factor);
            }
            UiAction::TogglePlayback => timeline_ui::toggle_playback(self),
            UiAction::StepFrame(delta) => timeline_ui::step_frame(self, delta),
            UiAction::GoToFrame(end) => {
                let frame = match end {
                    FrameEnd::First => 0,
                    FrameEnd::Last => self.tab().active_timeline().total_frames.saturating_sub(1),
                };
                timeline_ui::go_to_frame(self, frame);
            }
            UiAction::Arrow { dir, big } => {
                if self.tab().session.selection().is_empty() {
                    match dir {
                        ArrowDir::Left => timeline_ui::step_frame(self, -1),
                        ArrowDir::Right => timeline_ui::step_frame(self, 1),
                        ArrowDir::Up | ArrowDir::Down => {}
                    }
                } else {
                    let step = if big { 10.0 } else { 1.0 };
                    let delta = match dir {
                        ArrowDir::Left => kurbo::Vec2::new(-step, 0.0),
                        ArrowDir::Right => kurbo::Vec2::new(step, 0.0),
                        ArrowDir::Up => kurbo::Vec2::new(0.0, -step),
                        ArrowDir::Down => kurbo::Vec2::new(0.0, step),
                    };
                    apply(self, Action::NudgeSelection { delta });
                }
            }
            UiAction::Escape => {
                canvas_tool::cancel_active_drag(self.tab_mut());
                if !self.tab().session.selection().is_empty() {
                    apply(self, Action::Deselect);
                } else {
                    apply(self, Action::ExitGroupSteps { steps: 1 });
                }
                self.tool.select_sub_mode = SelectSubMode::Reshape;
            }
            UiAction::AnchorEdit(mode) => self.anchor_edit_key(mode),
            UiAction::OpenProjectDialog => project_io::open_project_dialog(self, ctx),
            UiAction::SaveProject => project_io::save_project(self),
            UiAction::SaveProjectAsDialog => project_io::save_project_as_dialog(self),
            UiAction::ExportVideoDialog => self.export_video_dialog(),
            UiAction::ImportSoundDialog => self.import_sound_dialog(),
            UiAction::CancelVideoExport => self.cancel_active_video_export(),
            UiAction::CloseActiveTab => {
                self.cancel_active_video_export();
                project_io::close_active_tab(self, Some(ctx));
            }
            UiAction::CloseTab(idx) => {
                self.cancel_active_video_export();
                project_io::close_tab(self, idx, Some(ctx));
            }
            UiAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
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
            UiAction::RestoreRecovered => {
                let found = std::mem::take(&mut self.recovery.found);
                recovery::Recovery::restore_found(found, self, ctx);
            }
            UiAction::DiscardRecovered => self.recovery.discard_found(),
            UiAction::Doc(action) => {
                apply(self, action);
            }
        }
    }

    // ------------------------------------------------------------------------
    // 2. キーボードショートカット制御
    // ------------------------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let input = ctx.input(|i| i.clone());
        // V の押す・離すの合図は、ほかの判定で抜ける前に毎コマ見る（`Clipboard::swallowed_paste`）
        let swallowed_paste = self.clipboard.swallowed_paste(&input.events);
        let typing = ctx.wants_keyboard_input();
        // キーはみな表（`menu::COMMANDS`）から引く（前は表の外に、1文字のキーと `Cmd` のキーを決める所が2つあった：
        // 約束の棚卸し 5616fcc の C の E3）。ダイアログが出ている間に受け付けるかは `Dialog::accepts` が決める
        for (pressed, repeat) in menu::pressed_shortcuts(&input, swallowed_paste) {
            self.run_menu_key(pressed, repeat, typing, ctx);
        }
    }

    /// メニューの項目のキー `shortcut`（`repeat` は押しっぱなしの繰り返しか）を、表（`menu::COMMANDS`）で決めて動かす。
    /// 入力欄に文字を入れている最中（`typing`）は、入力欄が使うキー（`Cmd` の付かないキーと、文字の操作のある項目の
    /// キー：`MenuCommand::text_op_for`）は入力欄のもの（キーの合図は egui が入力欄へ渡している）
    /// メニューに出さないキーだけの項目（`MenuCommand::menu` が `None`）は、入力欄に文字を入れている最中は受け付けない
    /// （キャンバスとタイムラインのキー）。押し直した（繰り返しでない）キーは、押し直しで区切るまとめを区切ってから動かす
    /// （矢印で動かすのは押しているあいだ1手にまとめ、押し直すと別の1手にする）
    fn run_menu_key(
        &mut self,
        pressed: menu::Pressed,
        repeat: bool,
        typing: bool,
        ctx: &egui::Context,
    ) {
        let Some(index) = menu::command_for_key(pressed, self.os_menu_has_keys()) else {
            return;
        };
        let command = &menu::COMMANDS[index];
        if repeat && !command.repeats {
            return;
        }
        if typing && (!pressed.command || command.menu.is_none() || command.text_op_for(typing).is_some()) {
            return;
        }
        // キーを押し直したら、押し直しで区切るまとめ（`CoalesceKey::ends`）を区切る
        if !repeat {
            apply(self, Action::EndCoalesceOn { end: canvas_tool::CoalesceEnd::KeyPressedAgain });
        }
        self.apply_action(command.action.clone(), ctx);
    }

    /// OS のメニューが、表の `Cmd` のショートカットを受け取るか（Mac で OS のメニューを作ったとき。
    /// `MenuCommand::egui_keys`）
    #[cfg(target_os = "macos")]
    fn os_menu_has_keys(&self) -> bool {
        self.os_menu.is_some()
    }

    /// OS のメニューが、表の `Cmd` のショートカットを受け取るか（Windows・Linux では OS のメニューを作らない）
    #[cfg(not(target_os = "macos"))]
    fn os_menu_has_keys(&self) -> bool {
        false
    }

    /// OS のメニューで押された項目 `chosen`（`menu::COMMANDS` の番号）を、押した順に渡す。渡す先は
    /// `MenuCommand::text_op_for` で決める：入力欄に文字を入れている最中の文字の操作は、egui の入力の合図にして入力欄へ
    /// （このあと描く入力欄が受け取る）、ほかはキーと同じ `apply_action` へ
    #[cfg(target_os = "macos")]
    fn run_os_menu_choices(&mut self, chosen: Vec<usize>, ctx: &egui::Context) {
        let typing = ctx.wants_keyboard_input();
        for index in chosen {
            let command = &menu::COMMANDS[index];
            match command.text_op_for(typing) {
                Some(op) => {
                    let event = op.event(app_menu_macos::pasteboard_text);
                    ctx.input_mut(|i| i.events.push(event));
                }
                None => self.apply_action(command.action.clone(), ctx),
            }
        }
    }

    // ------------------------------------------------------------------------
    // 3. 中央キャンバスのツール入力ディスパッチ・バッジ・カーソル更新
    // ------------------------------------------------------------------------

    fn is_current_tool_allowed(&self) -> bool {
        match self.tool.tool {
            Tool::Eyedropper => true,
            Tool::Brush | Tool::Pencil | Tool::Eraser | Tool::Bucket | Tool::InkBottle => {
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
                self.begin_operation();
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
                Tool::Brush | Tool::Pencil | Tool::Eraser => {
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
                Tool::InkBottle => {
                    let just_pressed =
                        response.hovered() && ui.input(|i| i.pointer.primary_pressed());
                    if just_pressed {
                        if let Some(pt) = mouse_pt {
                            let stroke = self.tool.stroke;
                            apply(self, Action::InkBottle { pt, stroke });
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

    /// 画面の1コマ（`update` の中身）：画面の仕事と、コマの終わりの仕事を、それぞれ砦の中で走らせる
    pub(crate) fn frame(&mut self, ctx: &egui::Context) {
        self.run_frame(|app| app.update_ui(ctx));
        self.run_frame(Self::finish_frame);
    }

    /// 画面の1コマの仕事 `work` を、画面のスレッドの砦の中で走らせる（画面の仕事の入口はここ1つ：設計
    /// claude/最後の砦を仕事の入口に置く設計.md）。途中で止まったら（panic）、そのコマを無かったことにする：タイムラインの
    /// ドラッグを取り消し、止まった仕事の戻し方 `recover_from_panic`（画面の土台を捨てて作り直す・キャンバスのドラッグを取り
    /// 消す・操作の途中なら操作の前に戻す・知らせる）を、これも砦の中で走らせ、次のコマから続ける。戻し方まで止まったら、
    /// アプリの状態が分からないので、今の未保存のタブを復旧ファイルに書き終えてから終わる（設計審査 de92ceb の D-2）
    pub(crate) fn run_frame(&mut self, work: impl FnOnce(&mut Self)) {
        if guard::guarded(|| work(self)).is_some() {
            return;
        }
        self.timeline_ui.cancel_drag();
        if guard::guarded(|| canvas_tool::doc_actions::recover_from_panic(self)).is_none() {
            self.flush_recovery();
            panic!("止まった仕事を戻す途中で、また止まった");
        }
    }

    /// 現在のレイヤー・フレーム状態から常時表示すべき状態連動メッセージを判定する
    fn current_state_notice(&self, tool_allowed: bool) -> Option<(NoticeKind, String)> {
        if self.tab().session.drag.unplaceable() {
            return Some((NoticeKind::Warning, shown_notice_text(&canvas_tool::Refusal::OffGrid.message())));
        }
        if tool_allowed {
            return None;
        }
        let tl = self.tab().active_timeline();
        let block = tl
            .layers
            .get(tl.active_layer)
            .and_then(|l| l.edit_block_at(tl.current_frame));
        let msg = match block {
            Some(EditBlock::NoFrame) => {
                "現在のフレームは未作成です（F5 / F6 / F7 でフレームを作成してください）"
            }
            Some(EditBlock::TweenMiddle) => {
                "トゥイーンの途中のコマは編集できません（F6 でキーフレームを挿入してください）"
            }
            Some(EditBlock::SoundKeyframe) => {
                "音の付いたキーフレームには絵を描けません（音を「なし」にするか、別のレイヤーを使ってください）"
            }
            Some(EditBlock::HiddenOrLocked) | None => {
                "\u{e897} 現在のレイヤーはロック中または非表示のため編集できません"
            }
        };
        Some((NoticeKind::Warning, msg.to_owned()))
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

        let now = Instant::now();
        self.notices.drop_expired(now);
        for toast in self.notices.iter() {
            ctx.request_repaint_after(toast.expires_at.saturating_duration_since(now));
            let text = shown_notice_text(&canvas_tool::NoticeText { text: toast.message.clone(), undo_exit: toast.undo_exit });
            if !notices.iter().any(|(_, m)| m == &text) {
                notices.push((toast.kind, text));
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
            painter.rect_stroke(
                badge_rect,
                0.0,
                Stroke::new(1.0_f32, border_col),
                egui::StrokeKind::Middle,
            );
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
        if ctx.input(|i| i.viewport().close_requested()) {
            self.cancel_active_video_export();
        }
        self.poll_video_export(ctx);
        project_io::handle_viewport_close_request(self, ctx);
        timeline_ui::update_playback(self, ctx);
        self.handle_shortcuts(ctx);
        #[cfg(target_os = "macos")]
        if let Some(chosen) = self
            .os_menu
            .as_ref()
            .map(app_menu_macos::OsMenu::take_chosen)
        {
            self.run_os_menu_choices(chosen, ctx);
        }

        let mut actions = Vec::new();
        // Mac ではメニューは OS のメニューバーにあるので、窓の中のメニューバーは出さない
        #[cfg(not(target_os = "macos"))]
        {
            panels::draw_top_menu_bar(&self.menu_states(ctx), &mut actions, ctx);
            for action in actions.drain(..) {
                self.apply_action(action, ctx);
            }
        }

        panels::draw_left_toolbar(&self.tool, &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        panels::draw_right_properties_panel(
            &self.tabs[self.active],
            &mut self.tool,
            &mut self.right_panel_tab,
            &mut actions,
            ctx,
        );
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        timeline_ui::draw_timeline_panel(self, ctx);

        panels::draw_document_tab_bar(&self.tabs, self.active, &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        panels::draw_stage_header_bar(&self.tabs[self.active], &mut actions, ctx);
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(244, 245, 248)))
            .show(ctx, |ui| {
                let (canvas_rect, response) =
                    ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
                self.canvas_rect = canvas_rect;
                if self.tab().view.fit_pending {
                    self.fit_stage_to_window();
                    self.tab_mut().view.fit_pending = false;
                }
                let painter = ui.painter_at(canvas_rect);

                let pointer_pos = response
                    .interact_pointer_pos()
                    .or_else(|| ctx.pointer_hover_pos());

                // 0. ビューポートのズーム・パン移動操作ハンドリング
                let modal_open = self.shown_dialog().is_some();
                let pan_state @ (is_panning, _) = if modal_open {
                    self.tab_mut().view.pan_last_screen_pos = None;
                    (false, false)
                } else {
                    let allow_pan =
                        matches!(self.tab().session.drag, canvas_tool::ActiveCanvasDrag::None);
                    self.tab_mut()
                        .view
                        .handle_navigation(ui, canvas_rect, pointer_pos, allow_pan)
                };

                let mouse_pt = pointer_pos.map(|p| self.tab().view.screen_to_world(p));
                let tool_allowed = !modal_open && self.is_current_tool_allowed();

                // 1. ツール別のマウス入力ハンドリング（パン操作開始時・モーダル表示時は進行中のドラッグをキャンセルしてスキップ。
                //    道具が使えない（コマが無い・ロックなど）ときも、進行中のドラッグを取り消す（スポイトは使えるので入力は渡す）。
                //    前は道具が使えなくなってもドラッグが残り、描きかけの形が画面に残った：約束の棚卸し 5616fcc の C の C3）
                if is_panning || modal_open {
                    canvas_tool::cancel_active_drag(self.tab_mut());
                } else {
                    if !tool_allowed {
                        canvas_tool::cancel_active_drag(self.tab_mut());
                    }
                    self.handle_canvas_tool_input(ui, &response, mouse_pt, tool_allowed);
                }

                let stage_size = self.tab().doc().stage_size;
                let shows_stage = self.tab().session.edit_path().shows_stage();

                // 2. ステージ背景（ペーストボード＋白いステージ矩形。ライブラリから入ったシンボルの編集では全面を白）の描画
                self.tab().view.draw_stage_background(
                    &painter,
                    canvas_rect,
                    stage_size,
                    shows_stage,
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

        // 出ているダイアログ（`shown_dialog`。受け付ける操作 `Dialog::accepts` と同じ1つの判定）だけを描く
        match self.shown_dialog() {
            Some(Dialog::VideoExport(progress)) => {
                panels::draw_video_export_modal(progress, &mut actions, ctx)
            }
            Some(Dialog::Recovery) => panels::draw_recovery_modal(&self.recovery.found, &mut actions, ctx),
            Some(Dialog::CloseConfirm) => {
                panels::draw_unsaved_close_confirm_modal(self.tab(), &mut actions, ctx)
            }
            None => {}
        }
        for action in actions.drain(..) {
            self.apply_action(action, ctx);
        }
        // ファイルから読んだもので、切り直した地図・何も無いものとして見せている地図・読めない音があれば知らせる（地図は
        // 使うときに確かめるので、あとから増える）
        // 知らせが出ている間に見つかった分を足し合わせて、1つの知らせを書き直す（裏の確かめで少しずつ見つかっても「ほか N か所」
        // にまとまる。知らせが消えたら足し合わせを捨てる：設計審査 b7208d9 の 4.）
        let found = self.tab().doc().load_notices.take();
        if !self.notices.has(canvas_tool::NoticeKey::Load) {
            self.shown_load_notices.clear();
        }
        if !found.is_empty() {
            self.shown_load_notices.extend(found);
            if let Some(text) = load_notice_text(&self.shown_load_notices) {
                canvas_tool::show_keyed_notice(&mut self.notices, canvas_tool::NoticeKey::Load, NoticeKind::Warning, text);
                ctx.request_repaint();
            }
        }
        // そのフレームの操作をすべて適用した後の状態から、OS のメニューの見せ方とウィンドウタイトルを更新し、コマが
        // 変わっていれば音を鳴らす
        #[cfg(target_os = "macos")]
        if let Some(os_menu) = &self.os_menu {
            os_menu.show_states(&self.menu_states(ctx));
        }
        project_io::update_window_title(self, ctx);
        timeline_ui::scrub_on_frame_change(self);
        self.prefetch_next_cels();

    }

    /// コマの終わりの仕事：タブの控えを渡す・裏の仕事の知らせ（画面の仕事 `update_ui` とは別の仕事の単位。画面の仕事が
    /// 止まっても走る：`update`。設計審査 6fb4faa の D-3）
    pub(crate) fn finish_frame(&mut self) {
        // タブの控えを今のタブにし（文書が変わったタブがあるときだけ写し直す）、変わったら裏の書き手に渡して、復旧ファイルに
        // 書き足してもらう（待たない）
        if self.tab_copies.refresh(&self.tabs) {
            if let Some(w) = &self.recovery_writer {
                w.submit(self.tab_copies.get());
            }
        }
        // 作業の控え（復旧ファイル）を書けなかったことを知らせる（書き手の失敗はみな1つの受け箱から：設計審査 b7208d9 の 1.）
        let failed = self.recovery_writer.as_ref().map(recovery::RecoveryWriter::take_failures).unwrap_or_default();
        for f in failed {
            let what = f.tab.map_or_else(|| "作業の控え（復旧ファイル）".to_owned(), |t| format!("タブ「{t}」の作業の控え（復旧ファイル）"));
            self.notify_warning(format!("{what}を書けません（{}）。保存しておいてください", f.why));
        }
        // 砦の外まで止まって終わった裏のスレッド（内部の不具合）を知らせる
        let stopped = self.stopped_threads.take_new();
        if !stopped.is_empty() {
            self.notify_warning(format!(
                "内部の不具合で、裏の仕事（{}）が止まりました。保存して、アプリを開き直してください",
                stopped.join("・")
            ));
        }
    }

    /// 今のタブを、復旧ファイルに書き終えるまで待つ（確認なしに終わるとき・画面の処理で止まったとき）
    fn flush_recovery(&self) {
        if let Some(w) = &self.recovery_writer {
            self.tab_copies.refresh(&self.tabs);
            w.flush(self.tab_copies.get());
        }
    }
}

impl Is2dApp {
    /// アンカー編集のキー（A：表示（`mode` が `None`）、D：追加、F：削除。右パネルの選択ツールの3つのボタンと同じ）。選択
    /// ツールでは切り替える（もう1回押すと戻る）。ほかのツールでは、選択ツールにして ON にする（どのツールからでも効く。
    /// 押したのに何も起きないことが無いよう、ほかのツールから押したときは切り替えで OFF にしない。利用者が決めた）
    fn anchor_edit_key(&mut self, mode: Option<SelectSubMode>) {
        let entering = self.tool.tool != Tool::Select;
        if entering {
            apply(self, Action::SetTool { tool: Tool::Select });
        }
        match mode {
            None => {
                if !(entering && self.tab().view.show_wireframe) {
                    self.tab_mut().view.toggle(canvas_tool::ViewSwitch::Anchors);
                }
            }
            Some(mode) => {
                if !(entering && self.tool.select_sub_mode == mode) {
                    apply(self, Action::ToggleSelectSubMode { mode });
                }
            }
        }
    }

    /// 今のタブの、次に見るセル画の写しを先読みの係に作らせる（`prefetch`）。今描いているコマの範囲（今のコマ。止まって
    /// いてオニオンスキンが有れば、その範囲：オニオンスキンは再生中は描かない）のすぐ外の、レイヤーごとの次のセル画。
    /// 再生中は進む向きだけ、止まっているときは前後とも（どちらへ動くか分からない）。くり返し再生なら、端の先は反対の端
    /// （コマ送りも同じくり返しの決まり：`timeline_ui::step_frame`）
    fn prefetch_next_cels(&mut self) {
        let playing = self.timeline_ui.is_playing;
        let tab = self.tab();
        let tl = tab.active_timeline();
        let (first, last) = if tab.view.onion_skin_enabled && !playing {
            tab.onion_window()
        } else {
            (tl.current_frame, tl.current_frame)
        };
        let maps = prefetch::wanted_maps(tl, first, last, !playing, self.timeline_ui.loop_playback);
        self.prefetch.want(maps);
    }
}

/// ファイルから読んだものの知らせ `notices` を、種類ごとに1文にまとめた文（無ければ `None`）
fn load_notice_text(notices: &[timeline::LoadNotice]) -> Option<String> {
    use timeline::LoadNotice;
    let places = |pick: fn(&LoadNotice) -> Option<&String>| -> Option<String> {
        let all: Vec<&String> = notices.iter().filter_map(pick).collect();
        let first = all.first()?;
        Some(match all.len() {
            1 => (*first).clone(),
            n => format!("{first} ほか {} か所", n - 1),
        })
    };
    let repaired = places(|n| match n {
        LoadNotice::Repaired(at) => Some(at),
        _ => None,
    })
    .map(|at| format!("{at}の絵が今の決まりに合わなかったので、切り直して表示しています（保存すると切り直した絵を書きます）"));
    let blank = places(|n| match n {
        LoadNotice::Blank(at) => Some(at),
        _ => None,
    })
    .map(|at| {
        format!("{at}の絵が壊れていたので、何も無いものとして表示しています（この絵には描けません。保存すると元のまま書きます）")
    });
    let sounds: Vec<&str> = notices
        .iter()
        .filter_map(|n| match n {
            LoadNotice::UnreadableSound(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let sound = (!sounds.is_empty())
        .then(|| format!("音「{}」を読めなかったので、鳴らさずに開きました（保存すると元のまま書きます）", sounds.join("」「")));
    let short: Vec<&str> = notices
        .iter()
        .filter_map(|n| match n {
            LoadNotice::ShortSound(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let short = (!short.is_empty()).then(|| {
        format!("音「{}」が、付けた区間より短く読めたので、足りない所は鳴らさずに開きました（保存すると区間は元のまま書きます）", short.join("」「"))
    });
    let lines: Vec<String> = [repaired, blank, sound, short].into_iter().flatten().collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

impl eframe::App for Is2dApp {
    /// 画面の毎コマは、仕事の単位を2つに分けて、それぞれ砦の中で走らせる：画面の仕事（操作を受ける・描く）と、コマの終わりの
    /// 仕事（控えを渡す・裏の仕事の知らせ）。画面の仕事が止まっても、コマの終わりの仕事は走る（前は1つの単位だったので、
    /// 描く所で止まり続けると、作業の控えが復旧の係へ渡らなかった：設計審査 6fb4faa の D-3）
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }

    /// 確認なしに終わるときは、今の未保存のタブを書き終えてから書き手を止める。確認ありで終わる（保存するか捨てるかを
    /// 決めた）ときは、書いた復旧ファイルをみな消して止める
    /// 終わるときの入口（砦の中。止まったら、書けなかったことを捨てて終わる。書き手はタブ1つ分ずつ砦の中で書くので、ふつう
    /// ここへは来ない）
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = guard::guarded(|| {
            if !self.allow_viewport_close {
                self.flush_recovery();
            }
            if let Some(w) = self.recovery_writer.take() {
                w.finish(self.allow_viewport_close);
            }
        });
    }
}

