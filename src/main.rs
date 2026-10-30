mod canvas_tool;
mod panels;
mod project_io;
mod symbol;
mod timeline;
mod timeline_ui;
mod tool_paint;
mod tool_select;
mod tool_transform;
mod vector_boolean;
mod vector_engine;
mod vector_stroke;

use canvas_tool::{CanvasState, SelectSubMode, Tool, ViewportState};
use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Sense, Stroke, Vec2};
use kurbo::Point;
use panels::RightPanelTab;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use symbol::{SymbolDef, SymbolEditSession, SymbolManager};
use timeline::Timeline;
use timeline_ui::TimelineUiState;
use vector_engine::FillShape;

const MAX_UNDO_HISTORY: usize = 100;
const DEFAULT_TOAST_DURATION: Duration = Duration::from_millis(2800);

/// キャンバス左上に表示する通知・警告メッセージの種別
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeKind {
    /// 操作不可や条件不足の警告（黄色系バッジ）
    Warning,
    /// エラー通知（赤系バッジ）
    Error,
    /// 通常の案内・情報（青系バッジ）
    Info,
}

impl NoticeKind {
    /// 種別に応じた `(背景色, 枠線色, 文字色)` を返す
    pub(crate) fn colors(self) -> (Color32, Color32, Color32) {
        match self {
            NoticeKind::Warning => (
                Color32::from_rgba_unmultiplied(255, 248, 225, 244),
                Color32::from_rgb(215, 165, 55),
                Color32::from_rgb(145, 90, 10),
            ),
            NoticeKind::Error => (
                Color32::from_rgba_unmultiplied(255, 236, 236, 244),
                Color32::from_rgb(220, 75, 75),
                Color32::from_rgb(155, 25, 25),
            ),
            NoticeKind::Info => (
                Color32::from_rgba_unmultiplied(235, 245, 255, 244),
                Color32::from_rgb(55, 140, 225),
                Color32::from_rgb(20, 75, 145),
            ),
        }
    }
}

/// 操作時に一定時間表示して自動で消えるトースト通知メッセージ
#[derive(Clone, Debug)]
pub(crate) struct StatusToast {
    pub(crate) kind: NoticeKind,
    pub(crate) message: String,
    pub(crate) expires_at: Instant,
}

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

// ============================================================================
// メインアプリケーション状態 & ショートカット制御
// ============================================================================

#[derive(Clone, Debug)]
pub(crate) struct UndoEntry {
    pub(crate) timeline: Timeline,
    pub(crate) library: std::sync::Arc<Vec<SymbolDef>>,
    pub(crate) editing_group_path: Vec<u64>,
    pub(crate) symbol_edit_stack: std::sync::Arc<Vec<SymbolEditSession>>,
    pub(crate) stage_size: Vec2,
    pub(crate) symbols_revision: u64,
    /// この履歴スナップショットが表すドキュメント状態世代ID（未保存変更 `is_dirty` 判定用）
    pub(crate) commit_id: u64,
}

impl UndoEntry {
    /// スナップショットから随時再生成可能な一時メッシュキャッシュ（`fill_mesh`, `selection_mesh`, `CelMeshCache`）を除去し、
    /// 直前の履歴スナップショット `prev_timeline` と未変更のセルは `Arc` を $O(1)$ 共有した軽量状態にする
    pub(crate) fn strip_transient_caches(mut self, prev_timeline: Option<&Timeline>) -> Self {
        self.timeline
            .strip_transient_caches_with_prev(prev_timeline);
        if let Some(lib) = std::sync::Arc::get_mut(&mut self.library) {
            for sym in lib {
                sym.strip_transient_caches();
            }
        }
        if let Some(stack) = std::sync::Arc::get_mut(&mut self.symbol_edit_stack) {
            for sess in stack {
                sess.strip_transient_caches();
            }
        }
        self
    }
}

/// Undo / Redo スナップショット履歴および保存時点のドキュメント世代IDを管理する構造体
pub(crate) struct UndoHistory {
    pub(crate) undo_stack: VecDeque<UndoEntry>,
    pub(crate) redo_stack: Vec<UndoEntry>,
    /// 現在のドキュメント状態の世代ID（初期状態・読み込み直後は `0`）
    pub(crate) current_commit_id: u64,
    /// 直近の保存時点（または未編集の初期状態）の世代ID（`Some(current_commit_id)` と一致していれば未変更）
    pub(crate) saved_commit_id: Option<u64>,
    /// 次に発行する新しい状態世代ID（単調増加）
    pub(crate) next_commit_id: u64,
}

impl Default for UndoHistory {
    fn default() -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            current_commit_id: 0,
            saved_commit_id: Some(0),
            next_commit_id: 1,
        }
    }
}

impl UndoHistory {
    /// 現在のドキュメントに未保存の変更があるか（保存時点の世代IDと現在世代IDが異なるか）を返す
    #[inline]
    pub(crate) fn is_dirty(&self) -> bool {
        self.saved_commit_id != Some(self.current_commit_id)
    }

    /// 現在のドキュメント世代IDを「保存済み」として記録する
    #[inline]
    pub(crate) fn mark_saved(&mut self) {
        self.saved_commit_id = Some(self.current_commit_id);
    }

    /// 保存済み世代IDをクリアして常時「未保存」状態にする（別タブによる同名ファイル上書き時のデタッチ用）
    #[inline]
    pub(crate) fn mark_unsaved(&mut self) {
        self.saved_commit_id = None;
    }

    /// シンボルライブラリおよび編集スタックが直近の履歴エントリから未変更（同一 `revision`）であれば
    /// `Arc` ポインタを $O(1)$ で共有し、変更されている場合のみクローン＆キャッシュ除去して新しい `Arc` を返す
    pub(crate) fn snapshot_symbols(
        &self,
        symbols: &SymbolManager,
    ) -> (
        std::sync::Arc<Vec<SymbolDef>>,
        std::sync::Arc<Vec<SymbolEditSession>>,
        u64,
    ) {
        if let Some(reused) = self
            .undo_stack
            .back()
            .or_else(|| self.redo_stack.last())
            .filter(|e| e.symbols_revision == symbols.revision)
        {
            return (
                std::sync::Arc::clone(&reused.library),
                std::sync::Arc::clone(&reused.symbol_edit_stack),
                symbols.revision,
            );
        }
        let mut lib = symbols.library.clone();
        for sym in &mut lib {
            sym.strip_transient_caches();
        }
        let mut stack = symbols.edit_stack.clone();
        for sess in &mut stack {
            sess.strip_transient_caches();
        }
        (
            std::sync::Arc::new(lib),
            std::sync::Arc::new(stack),
            symbols.revision,
        )
    }

    /// 新しい Undo エントリをスタックへ追加し、Redo スタックをクリアして現在の状態世代IDを進める
    pub(crate) fn push(&mut self, mut entry: UndoEntry) {
        entry.commit_id = self.current_commit_id;
        self.current_commit_id = self.next_commit_id;
        self.next_commit_id = self.next_commit_id.saturating_add(1);
        let stripped = {
            let prev_tl = self
                .undo_stack
                .back()
                .or_else(|| self.redo_stack.last())
                .map(|e| &e.timeline);
            entry.strip_transient_caches(prev_tl)
        };
        if self.undo_stack.len() >= MAX_UNDO_HISTORY {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(stripped);
        self.redo_stack.clear();
    }

    /// Undo スタック末尾から直前のスナップショットを取り出す
    pub(crate) fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.undo_stack.pop_back()
    }

    /// Redo スタック末尾から次のスナップショットを取り出す
    pub(crate) fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.redo_stack.pop()
    }

    /// Undo 実行前の現在状態を Redo スタックへ退避する
    pub(crate) fn push_redo(&mut self, entry: UndoEntry) {
        let stripped = {
            let prev_tl = self
                .redo_stack
                .last()
                .or_else(|| self.undo_stack.back())
                .map(|e| &e.timeline);
            entry.strip_transient_caches(prev_tl)
        };
        self.redo_stack.push(stripped);
    }

    /// Redo 実行前の現在状態を Undo スタックへ戻す（Redo スタックは消去しない）
    pub(crate) fn push_undo_from_redo(&mut self, entry: UndoEntry) {
        let stripped = {
            let prev_tl = self
                .undo_stack
                .back()
                .or_else(|| self.redo_stack.last())
                .map(|e| &e.timeline);
            entry.strip_transient_caches(prev_tl)
        };
        if self.undo_stack.len() >= MAX_UNDO_HISTORY {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(stripped);
    }
}

/// ツール選択・ブラシ/消しゴム設定・右パネルタブ・トースト通知などアプリ全体のUI設定状態
pub(crate) struct AppUiState {
    pub(crate) tool: Tool,
    pub(crate) select_sub_mode: SelectSubMode,
    pub(crate) right_panel_tab: RightPanelTab,
    pub(crate) brush_radius: f32,
    pub(crate) eraser_radius: f32,
    pub(crate) brush_color: Color32,
    pub(crate) show_wireframe: bool,
    /// プロパティパネルの下部表示対象（`true`: タイムラインのフレーム/トゥイーン属性、`false`: キャンバス上のシンボル属性）
    pub(crate) inspect_timeline_frame: bool,
    /// シンボル再生オプションの開始/表示フレーム数値フィールドを連続編集（ドラッグ/入力）中かどうか
    pub(crate) editing_symbol_playback: bool,
    /// 現在開いている・保存先の `.is2d` プロジェクトファイルパス（未保存の新規ドキュメント時は `None`）
    pub(crate) current_file_path: Option<std::path::PathBuf>,
    /// 操作トリガーで一定時間表示するトースト通知メッセージ
    pub(crate) status_toast: Option<StatusToast>,
}

impl Default for AppUiState {
    fn default() -> Self {
        Self {
            tool: Tool::Brush,
            select_sub_mode: SelectSubMode::Reshape,
            right_panel_tab: RightPanelTab::default(),
            brush_radius: 8.0,
            eraser_radius: 22.0,
            brush_color: Color32::from_rgb(40, 45, 55),
            show_wireframe: false,
            inspect_timeline_frame: false,
            editing_symbol_playback: false,
            current_file_path: None,
            status_toast: None,
        }
    }
}

impl AppUiState {
    /// キャンバス左上に一定時間自動で消えるトースト通知メッセージを登録する
    pub(crate) fn show_notice(&mut self, kind: NoticeKind, message: impl Into<String>) {
        self.status_toast = Some(StatusToast {
            kind,
            message: message.into(),
            expires_at: Instant::now() + DEFAULT_TOAST_DURATION,
        });
    }
}

/// アプリケーション全体のルート構造体（責務ごとに独立したドメイン構造体で構成）
#[derive(Default)]
pub(crate) struct Is2dApp {
    /// ツール設定・右パネルタブ・ステータストースト通知などのUI状態
    pub(crate) ui: AppUiState,
    /// キャンバスのズーム・パン・ステージ枠設定
    pub(crate) viewport: ViewportState,
    /// 現在フレームの作業図形・選択・変形・排他ドラッグ・クリップボード状態
    pub(crate) canvas: CanvasState,
    /// タイムライン・レイヤー・キーフレーム・セル構造
    pub(crate) timeline: Timeline,
    /// タイムラインパネルの再生・スクロール・D&D等の一時UI状態
    pub(crate) timeline_ui: TimelineUiState,
    /// プロジェクト共通のシンボルライブラリ・シンボルインプレイス編集スタック・ID採番
    pub(crate) symbols: SymbolManager,
    /// Undo / Redo 履歴スタック
    pub(crate) history: UndoHistory,
    /// 複数ドキュメント同時オープン用のタブ管理状態（アクティブ状態スワップ方式）
    pub(crate) tabs: project_io::DocumentTabManager,
}

impl Is2dApp {
    // ------------------------------------------------------------------------
    // 1. Undo / Redo 履歴管理 & トランザクション変更API
    // ------------------------------------------------------------------------

    fn restore_state_snapshot(&mut self, entry: UndoEntry) -> UndoEntry {
        self.timeline_ui.cancel_drag();
        self.viewport.editing_stage_size = false;
        self.timeline_ui.editing_total_frames = false;
        self.timeline_ui.editing_fps = false;
        self.symbols.actively_renaming_symbol = None;
        self.ui.editing_symbol_playback = false;
        let (prev_lib, prev_stack, prev_rev) = self.history.snapshot_symbols(&self.symbols);
        let prev_entry = UndoEntry {
            timeline: std::mem::replace(&mut self.timeline, entry.timeline),
            library: prev_lib,
            editing_group_path: std::mem::replace(
                &mut self.canvas.editing_group_path,
                entry.editing_group_path,
            ),
            symbol_edit_stack: prev_stack,
            stage_size: std::mem::replace(&mut self.viewport.stage_size, entry.stage_size),
            symbols_revision: prev_rev,
            commit_id: std::mem::replace(&mut self.history.current_commit_id, entry.commit_id),
        };
        self.symbols.library = std::sync::Arc::unwrap_or_clone(entry.library);
        self.symbols.edit_stack = std::sync::Arc::unwrap_or_clone(entry.symbol_edit_stack);
        self.symbols.revision = entry.symbols_revision;
        self.symbols.sync_after_restore();
        canvas_tool::load_shapes_from_timeline(self);
        prev_entry
    }

    pub(crate) fn record_undo_snapshot(&mut self, mut snapshot: Vec<FillShape>) {
        let mut tl_snap = self.timeline.clone();
        tl_snap.normalize_shape_layer_ids(&mut snapshot);
        tl_snap.write_shapes_to_editable_cels(
            &snapshot,
            &self.symbols.library,
            self.canvas.loaded_signatures(),
        );
        let (library, symbol_edit_stack, symbols_revision) =
            self.history.snapshot_symbols(&self.symbols);
        let entry = UndoEntry {
            timeline: tl_snap,
            library,
            editing_group_path: self.canvas.editing_group_path.clone(),
            symbol_edit_stack,
            stage_size: self.viewport.stage_size,
            symbols_revision,
            commit_id: self.history.current_commit_id,
        };
        self.history.push(entry);
    }

    pub(crate) fn push_undo(&mut self) {
        if self.timeline_ui.is_playing {
            canvas_tool::stop_playback(self);
        } else if let Some(mut unsliced) = self.canvas.take_unsliced_snapshot() {
            let selected_ids = self.canvas.selected_shape_ids();
            self.timeline.normalize_shape_layer_ids(&mut unsliced);
            self.canvas.shapes = unsliced;
            self.canvas.restore_selection_by_shape_ids(&selected_ids);
            canvas_tool::sync_shapes_to_timeline(self);
        } else {
            canvas_tool::sync_shapes_to_timeline(self);
        }
        let (library, symbol_edit_stack, symbols_revision) =
            self.history.snapshot_symbols(&self.symbols);
        let entry = UndoEntry {
            timeline: self.timeline.clone(),
            library,
            editing_group_path: self.canvas.editing_group_path.clone(),
            symbol_edit_stack,
            stage_size: self.viewport.stage_size,
            symbols_revision,
            commit_id: self.history.current_commit_id,
        };
        self.history.push(entry);
    }

    /// キャンバス図形に対する変更トランザクションを一括実行する：
    /// 1. 再生中であれば停止して現在フレームの図形を展開 (`stop_playback`)
    /// 2. 必要に応じて未確定選択を確定解除 (`clear_selection`)
    /// 3. 変更前の Undo スナップショットを記録
    /// 4. `mutate` クロージャでキャンバス図形を更新
    /// 5. 変更後の図形を現在フレームのタイムライン `Cel` へ自動同期 (`sync_shapes_to_timeline`)
    pub(crate) fn mutate_canvas_with_undo(
        &mut self,
        clear_selection_first: bool,
        mutate: impl FnOnce(&mut Self),
    ) {
        canvas_tool::stop_playback(self);
        if clear_selection_first {
            tool_select::clear_selection(self);
        }
        let snapshot = self.canvas.take_or_clone_unsliced_snapshot();
        self.record_undo_snapshot(snapshot);
        mutate(self);
        canvas_tool::sync_shapes_to_timeline(self);
    }

    /// 事前取得済みの変更前スナップショット `pre_mutation_snapshot` を Undo 履歴に記録し、
    /// 現在の `self.canvas.shapes` をタイムライン `Cel` へ自動同期する単一コミットAPI
    pub(crate) fn commit_canvas_mutation(&mut self, pre_mutation_snapshot: Vec<FillShape>) {
        canvas_tool::stop_playback(self);
        self.record_undo_snapshot(pre_mutation_snapshot);
        canvas_tool::sync_shapes_to_timeline(self);
    }

    pub(crate) fn undo(&mut self) {
        canvas_tool::flush_canvas_before_timeline_op(self);
        if let Some(prev_entry) = self.history.pop_undo() {
            let curr = self.restore_state_snapshot(prev_entry);
            self.history.push_redo(curr);
        }
    }

    pub(crate) fn redo(&mut self) {
        canvas_tool::flush_canvas_before_timeline_op(self);
        if let Some(next_entry) = self.history.pop_redo() {
            let curr = self.restore_state_snapshot(next_entry);
            self.history.push_undo_from_redo(curr);
        }
    }

    // ------------------------------------------------------------------------
    // 2. キーボードショートカット制御
    // ------------------------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let mut file_cmd = None;
        ctx.input(|i| {
            let cmd = i.modifiers.command || i.modifiers.ctrl;
            if cmd && i.key_pressed(Key::N) {
                file_cmd = Some(0u8);
            } else if cmd && i.key_pressed(Key::O) {
                file_cmd = Some(1u8);
            } else if cmd && i.key_pressed(Key::S) {
                file_cmd = Some(if i.modifiers.shift { 3u8 } else { 2u8 });
            } else if cmd && i.key_pressed(Key::W) {
                file_cmd = Some(4u8);
            }
        });
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
        ctx.input(|i| {
            let cmd = i.modifiers.command || i.modifiers.ctrl;
            let copy_event = i.events.iter().any(|ev| matches!(ev, egui::Event::Copy));
            let cut_event = i.events.iter().any(|ev| matches!(ev, egui::Event::Cut));
            let paste_event = i
                .events
                .iter()
                .any(|ev| matches!(ev, egui::Event::Paste(_)));

            if !cmd && !copy_event && !cut_event && !paste_event {
                self.handle_single_key_shortcuts(i, i.modifiers.shift);
            }

            if i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace) {
                tool_select::delete_selected_shapes(self);
            }

            self.handle_command_shortcuts(i, cmd, copy_event, cut_event, paste_event);
        });
    }

    fn handle_single_key_shortcuts(&mut self, i: &egui::InputState, shift: bool) {
        if !self.canvas.selected_shapes.is_empty() {
            let step = if shift { 10.0 } else { 1.0 };
            let mut delta = kurbo::Vec2::ZERO;
            if i.key_pressed(Key::ArrowLeft) {
                delta.x -= step;
            }
            if i.key_pressed(Key::ArrowRight) {
                delta.x += step;
            }
            if i.key_pressed(Key::ArrowUp) {
                delta.y -= step;
            }
            if i.key_pressed(Key::ArrowDown) {
                delta.y += step;
            }
            if delta != kurbo::Vec2::ZERO {
                let _ = tool_select::nudge_selected_shapes(self, delta);
                return;
            }
        }

        if i.key_pressed(Key::Enter) {
            timeline_ui::toggle_playback(self);
        } else if i.key_pressed(Key::Comma) || i.key_pressed(Key::ArrowLeft) {
            timeline_ui::step_frame(self, -1);
        } else if i.key_pressed(Key::Period) || i.key_pressed(Key::ArrowRight) {
            timeline_ui::step_frame(self, 1);
        } else if i.key_pressed(Key::Home) {
            timeline_ui::switch_to_layer_and_frame(self, self.timeline.active_layer, 0);
        } else if i.key_pressed(Key::End) {
            let last = self.timeline.total_frames.saturating_sub(1);
            timeline_ui::switch_to_layer_and_frame(self, self.timeline.active_layer, last);
        } else if i.key_pressed(Key::F6) {
            if shift {
                timeline_ui::timeline_delete_keyframe(self);
            } else {
                timeline_ui::timeline_insert_keyframe(self, true);
            }
        } else if i.key_pressed(Key::F7) {
            timeline_ui::timeline_insert_keyframe(self, false);
        } else if i.key_pressed(Key::F8) {
            tool_transform::convert_selection_to_symbol(self);
        } else if i.key_pressed(Key::F5) {
            if shift {
                timeline_ui::timeline_remove_frame(self);
            } else {
                timeline_ui::timeline_extend_frame(self);
            }
        } else if i.key_pressed(Key::O) {
            self.timeline_ui.onion_skin_enabled = !self.timeline_ui.onion_skin_enabled;
        } else if i.key_pressed(Key::T) {
            tool_transform::timeline_toggle_tween(self);
        } else if let Some(tool) = Tool::from_shortcut_input(i) {
            canvas_tool::set_tool(self, tool);
        } else if i.key_pressed(Key::Escape) {
            if !self.canvas.selected_shapes.is_empty() {
                tool_select::clear_selection(self);
            } else {
                tool_select::exit_current_group(self);
            }
            canvas_tool::set_tool(self, Tool::Select);
        } else if self.ui.tool == Tool::Select {
            let plus_pressed = i.key_pressed(Key::Plus)
                || i.key_pressed(Key::Equals)
                || i.key_pressed(Key::Semicolon)
                || i.events.iter().any(|ev| {
                    matches!(ev, egui::Event::Text(t) if t == "+" || t == "=" || t == ";" || t == "＋")
                });
            let minus_pressed = i.key_pressed(Key::Minus)
                || i.events.iter().any(
                    |ev| matches!(ev, egui::Event::Text(t) if t == "-" || t == "ー" || t == "－"),
                );

            if plus_pressed {
                canvas_tool::toggle_select_sub_mode(self, SelectSubMode::AddAnchor);
            } else if minus_pressed {
                canvas_tool::toggle_select_sub_mode(self, SelectSubMode::DeleteAnchor);
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
        if copy_event || (cmd && !i.modifiers.shift && i.key_pressed(Key::C)) {
            tool_select::copy_selected_shapes(self);
        } else if cut_event || (cmd && !i.modifiers.shift && i.key_pressed(Key::X)) {
            tool_select::cut_selected_shapes(self);
        } else if paste_event || (cmd && i.key_pressed(Key::V)) {
            tool_select::paste_clipboard(self, i.modifiers.shift);
        } else if cmd && (i.key_pressed(Key::Num1) || i.key_pressed(Key::Num0)) {
            self.viewport.reset_view();
        } else if cmd && i.key_pressed(Key::Num2) {
            self.viewport.fit_stage_to_window();
        } else if cmd && i.key_pressed(Key::Num3) {
            self.viewport.clip_outside_stage = !self.viewport.clip_outside_stage;
        } else if cmd
            && (i.key_pressed(Key::Plus)
                || i.key_pressed(Key::Equals)
                || i.key_pressed(Key::Semicolon))
        {
            let center = i
                .pointer
                .hover_pos()
                .unwrap_or_else(|| self.viewport.last_canvas_rect.center());
            self.viewport.zoom_around_screen_pos(center, 1.25);
        } else if cmd && i.key_pressed(Key::Minus) {
            let center = i
                .pointer
                .hover_pos()
                .unwrap_or_else(|| self.viewport.last_canvas_rect.center());
            self.viewport.zoom_around_screen_pos(center, 0.8);
        } else if cmd && i.key_pressed(Key::G) {
            if i.modifiers.shift {
                tool_select::ungroup_selected_shapes(self);
            } else {
                tool_select::group_selected_shapes(self);
            }
        } else if cmd && i.key_pressed(Key::B) {
            tool_select::ungroup_selected_shapes(self);
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
        match self.ui.tool {
            Tool::Eyedropper => true,
            Tool::Brush => self.timeline.is_active_layer_editable(),
            Tool::Eraser | Tool::Bucket | Tool::Select | Tool::Transform => {
                self.timeline.has_any_editable_layer()
            }
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
                || response.drag_started()
                || response.clicked();
            if canvas_interacted {
                canvas_tool::stop_playback(self);
            } else {
                return;
            }
        }
        if self.ui.tool == Tool::Eyedropper {
            let sampling = (response.hovered() && ui.input(|i| i.pointer.primary_pressed()))
                || response.dragged()
                || response.clicked();
            if sampling {
                if let Some(pt) = mouse_pt {
                    tool_paint::pick_color_at(self, pt);
                }
            }
        } else if tool_allowed {
            match self.ui.tool {
                Tool::Brush | Tool::Eraser => {
                    tool_paint::handle_brush_or_eraser_input(self, response, mouse_pt);
                }
                Tool::Bucket => {
                    let just_pressed =
                        response.hovered() && ui.input(|i| i.pointer.primary_pressed());
                    if just_pressed {
                        if let Some(pt) = mouse_pt {
                            tool_paint::bucket_fill_at(self, pt);
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

    /// キャンバス左上に一定時間自動で消える通知メッセージを表示する共通メソッド
    pub(crate) fn show_notice(&mut self, kind: NoticeKind, message: impl Into<String>) {
        self.ui.show_notice(kind, message);
    }

    /// 警告・条件不足のトースト通知をキャンバス左上に表示する
    pub(crate) fn notify_warning(&mut self, message: impl Into<String>) {
        self.show_notice(NoticeKind::Warning, message);
    }

    /// エラーのトースト通知をキャンバス左上に表示する
    pub(crate) fn notify_error(&mut self, message: impl Into<String>) {
        self.show_notice(NoticeKind::Error, message);
    }

    /// 案内・情報のトースト通知をキャンバス左上に表示する
    pub(crate) fn notify_info(&mut self, message: impl Into<String>) {
        self.show_notice(NoticeKind::Info, message);
    }

    /// 現在のレイヤー・フレーム状態から常時表示すべき状態連動メッセージを判定する
    fn current_state_notice(&self, tool_allowed: bool) -> Option<(NoticeKind, &'static str)> {
        if tool_allowed {
            return None;
        }
        let is_uncreated_frame = self
            .timeline
            .layers
            .get(self.timeline.active_layer)
            .is_some_and(|l| {
                l.visible && !l.locked && self.timeline.current_frame >= l.frame_count
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

        if let Some(toast) = &self.ui.status_toast {
            let now = Instant::now();
            if now < toast.expires_at {
                ctx.request_repaint_after(toast.expires_at.saturating_duration_since(now));
                if !notices.iter().any(|(_, m)| m == &toast.message) {
                    notices.push((toast.kind, toast.message.clone()));
                }
            } else {
                self.ui.status_toast = None;
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
            painter.rect_stroke(badge_rect, 0.0, Stroke::new(1.0, border_col));
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
            canvas_tool::draw_tool_cursor_and_feedback(self, ctx, painter, pos);
        }
    }
}

impl eframe::App for Is2dApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        project_io::handle_viewport_close_request(self, ctx);
        timeline_ui::update_playback(self, ctx);
        self.handle_shortcuts(ctx);
        panels::draw_top_menu_bar(self, ctx);
        panels::draw_left_toolbar(self, ctx);
        panels::draw_right_properties_panel(self, ctx);
        timeline_ui::draw_timeline_panel(self, ctx);
        panels::draw_document_tab_bar(self, ctx);
        panels::draw_stage_header_bar(self, ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(244, 245, 248)))
            .show(ctx, |ui| {
                let (canvas_rect, response) =
                    ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
                self.viewport.last_canvas_rect = canvas_rect;
                let painter = ui.painter_at(canvas_rect);

                let pointer_pos = response
                    .interact_pointer_pos()
                    .or_else(|| ctx.pointer_hover_pos());

                // 0. ビューポートのズーム・パン移動操作ハンドリング
                let pan_state @ (is_panning, _) =
                    self.viewport
                        .handle_navigation(ui, canvas_rect, pointer_pos);

                let mouse_pt = pointer_pos.map(|p| self.viewport.screen_to_world(p));
                let tool_allowed = self.is_current_tool_allowed();

                // 1. ツール別のマウス入力ハンドリング（パン操作開始時は進行中のドラッグをキャンセルしてスキップ）
                if is_panning {
                    canvas_tool::cancel_active_drag(self);
                } else {
                    self.handle_canvas_tool_input(ui, &response, mouse_pt, tool_allowed);
                }

                let in_symbol_edit = !self.symbols.edit_stack.is_empty();

                // 2. ステージ背景（ペーストボード＋白いステージ矩形）の描画
                self.viewport
                    .draw_stage_background(&painter, canvas_rect, in_symbol_edit);

                // 3. キャンバス上のシェイプ・オニオンスキン・ステージ枠線・選択枠・プレビューの描画
                canvas_tool::draw_canvas_contents(self, &painter);

                // 4. 状態連動の警告バッジおよび一時トースト通知を描画
                self.draw_canvas_notices(ctx, &painter, canvas_rect, tool_allowed);

                // 3. カーソル表示とホバー・ドラッグ視覚フィードバック
                self.update_canvas_cursor(
                    ctx,
                    &painter,
                    canvas_rect,
                    pointer_pos,
                    pan_state,
                    tool_allowed,
                );
            });
    }
}

#[cfg(test)]
mod tests;
