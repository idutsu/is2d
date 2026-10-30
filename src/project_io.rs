use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Color32, Vec2};
use kurbo::{Affine, Point, QuadBez};
use serde::{Deserialize, Serialize};

use crate::canvas_tool::{CanvasState, SelectSubMode, ViewportState};
use crate::symbol::{
    SymbolDef, SymbolInstance, SymbolInstanceBinding, SymbolLoopMode, SymbolManager, TweenRotate,
};
use crate::timeline::{Cel, Keyframe, Layer, Timeline, TimelineCellSelection};
use crate::timeline_ui::TimelineUiState;
use crate::vector_engine::{Contour, FillShape};
use crate::{Is2dApp, UndoHistory};

/// `.is2d` プロジェクトファイルの現在のフォーマットバージョン
pub(crate) const PROJECT_FORMAT_VERSION: u32 = 1;

/// プロジェクトファイルの標準拡張子
pub(crate) const PROJECT_FILE_EXTENSION: &str = "is2d";

/// タイムラインおよびシンボル内タイムラインの最大許容フレーム数（破損JSONによる巨大ループ防止）
pub(crate) const MAX_TIMELINE_FRAMES: usize = 100_000;

/// 読み込み可能なプロジェクトファイルの最大バイト数（100MB：異常な巨大ファイルによるOOM防止）
pub(crate) const MAX_PROJECT_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// 1タイムラインあたりの最大許容レイヤー数
pub(crate) const MAX_LAYERS_PER_TIMELINE: usize = 1_000;

/// 1輪郭（Contour）あたりの最大許容ベジェセグメント数
pub(crate) const MAX_CONTOUR_SEGMENTS: usize = 100_000;

/// `open_project_in_tab` の実行結果区分
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenProjectOutcome {
    /// 新規タブ（または未編集の無題タブ）にプロジェクトを読み込んだ
    Loaded,
    /// 既に別タブで同じファイルが開かれていたため、そのタブへ切り替えた
    SwitchedToExistingTab,
    /// 既に現在のアクティブタブで開かれているファイルだった
    AlreadyActiveTab,
}

// ============================================================================
// 0. 複数ドキュメント同時オープン（ドキュメントタブ管理 & アクティブ状態スワップ）
// ============================================================================

/// 開いている1つのドキュメントタブが保持する独立状態スロット
pub(crate) struct DocumentSlot {
    /// ドキュメント固有の一意ID（別タブ間コピペのソース・ターゲット識別用）
    pub(crate) doc_id: u64,
    /// 未保存時の「無題-N」番号（1, 2, 3...）
    pub(crate) untitled_number: usize,
    /// 保存先・読み込み元の `.is2d` ファイルパス（未保存なら `None`）
    pub(crate) file_path: Option<PathBuf>,
    /// プロパティパネル下部の表示対象（`true`: タイムラインフレーム属性、`false`: シンボル属性）
    pub(crate) inspect_timeline_frame: bool,
    /// 「表示 > アンカーを表示」のタブ別ON/OFF状態
    pub(crate) show_wireframe: bool,
    pub(crate) viewport: ViewportState,
    pub(crate) canvas: CanvasState,
    pub(crate) timeline: Timeline,
    pub(crate) timeline_ui: TimelineUiState,
    pub(crate) symbols: SymbolManager,
    pub(crate) history: UndoHistory,
}

impl DocumentSlot {
    pub(crate) fn new_empty(doc_id: u64, untitled_number: usize) -> Self {
        Self {
            doc_id,
            untitled_number,
            file_path: None,
            inspect_timeline_frame: false,
            show_wireframe: false,
            viewport: ViewportState::default(),
            canvas: CanvasState::default(),
            timeline: Timeline::default(),
            timeline_ui: TimelineUiState::default(),
            symbols: SymbolManager::default(),
            history: UndoHistory::default(),
        }
    }

    /// タブバーやタイトルバーに表示するタイトル文字列（未保存変更がある場合は末尾に ` *` 付き）を返す
    pub(crate) fn display_title(
        &self,
        active_file_path: Option<&Path>,
        active_is_dirty: bool,
        is_active: bool,
    ) -> String {
        let path = if is_active {
            active_file_path
        } else {
            self.file_path.as_deref()
        };
        let base = match path.and_then(Path::file_name).and_then(|n| n.to_str()) {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => format!("無題-{}", self.untitled_number.max(1)),
        };
        let is_dirty = if is_active {
            active_is_dirty
        } else {
            self.history.is_dirty()
        };
        if is_dirty {
            format!("{base} *")
        } else {
            base
        }
    }
}

/// 複数のドキュメントタブを管理する構造体（アクティブなタブは `Is2dApp` 直下のフィールドへ展開される）
pub(crate) struct DocumentTabManager {
    pub(crate) tabs: Vec<DocumentSlot>,
    pub(crate) active_index: usize,
    pub(crate) next_untitled_number: usize,
    pub(crate) next_doc_id: u64,
    /// コピー（`Cmd+C`）を実行した元ドキュメントの `doc_id`（不変スナップショット）
    pub(crate) clipboard_source_doc_id: u64,
    /// コピー（`Cmd+C`）を実行した時点のシンボル編集コンテキスト（ルートシーンなら `None`、シンボル内なら `Some(symbol_id)`）
    pub(crate) clipboard_source_symbol_id: Option<u64>,
    /// コピー（`Cmd+C`）を実行した時点のシンボルライブラリの世代番号（同一世代での再コピー時に `Arc<SymbolDef>` を $O(1)$ で共有する）
    pub(crate) clipboard_source_symbols_revision: u64,
    /// 別タブへシンボルをペーストした際に自動移植するためのシンボル定義スナップショット（子→親のトポロジカル順、`Arc` 共有の不変保持）
    pub(crate) clipboard_symbols: Vec<Arc<SymbolDef>>,
    /// 現在のクリップボードから各タブへ移植済みのシンボルID対応表 `(target_doc_id, src_sym_id) -> target_sym_id`
    pub(crate) clipboard_imported_map: HashMap<(u64, u64), u64>,
    /// 直近にOSウィンドウへ送信したタイトル文字列（毎フレームの重複送信防止用キャッシュ）
    pub(crate) last_window_title: String,
}

impl Default for DocumentTabManager {
    fn default() -> Self {
        Self {
            tabs: vec![DocumentSlot::new_empty(1, 1)],
            active_index: 0,
            next_untitled_number: 2,
            next_doc_id: 2,
            clipboard_source_doc_id: 1,
            clipboard_source_symbol_id: None,
            clipboard_source_symbols_revision: 0,
            clipboard_symbols: Vec::new(),
            clipboard_imported_map: HashMap::new(),
            last_window_title: String::new(),
        }
    }
}

impl DocumentTabManager {
    /// 現在アクティブなタブの `doc_id` を返す
    #[inline]
    pub(crate) fn active_doc_id(&self) -> u64 {
        self.tabs
            .get(self.active_index)
            .map(|t| t.doc_id)
            .unwrap_or(1)
    }

    /// 現在アクティブなタブのメタデータ（`file_path`）を `app.ui.current_file_path` と同期する
    #[inline]
    pub(crate) fn sync_active_file_path(&mut self, path: Option<PathBuf>) {
        if let Some(slot) = self.tabs.get_mut(self.active_index) {
            slot.file_path = path;
        }
    }

    pub(crate) fn alloc_doc_id(&mut self) -> u64 {
        let id = self.next_doc_id;
        self.next_doc_id = self.next_doc_id.saturating_add(1);
        id
    }

    pub(crate) fn alloc_untitled_number(&mut self) -> usize {
        let num = self.next_untitled_number;
        self.next_untitled_number = self.next_untitled_number.saturating_add(1);
        num
    }
}

/// アクティブタブの `file_path` を `app.ui.current_file_path` と `app.tabs.tabs[active_index].file_path` の双方に一括設定する
pub(crate) fn set_active_file_path(app: &mut Is2dApp, path: Option<PathBuf>) {
    app.ui.current_file_path = path.clone();
    app.tabs.sync_active_file_path(path);
}

/// タブ切り替え・新規作成・ファイル読込・タブ閉じの前に、egui のフォーカス中テキスト入力ウィジェットからフォーカスを解放する
fn surrender_focused_widget(ctx: &egui::Context) {
    ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}

/// タブ切り替えやプロジェクト保存の前に、選択状態やカスタムピボットを破壊せずに
/// 進行中ドラッグの中断・連続編集フラグのリセット・（保存時のみ）未マージ生シェイプの結合/切り抜き確定・作業キャンバスのセル同期・多段シンボル編集スタックのライブラリ同期を行う
fn prepare_active_tab_for_switch_or_save(app: &mut Is2dApp, flush_raw_merge: bool) {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    app.timeline_ui.cancel_drag();
    app.viewport.pan_last_screen_pos = None;
    app.viewport.editing_stage_size = false;
    app.symbols.actively_renaming_symbol = None;
    app.ui.editing_symbol_playback = false;

    if flush_raw_merge && app.canvas.selection_modified && !app.canvas.selected_shapes.is_empty() {
        let selected_ids = app.canvas.selected_shape_ids();
        let saved_pivot = app.canvas.transform_pivot;
        crate::tool_select::flush_modified_raw_selection_merge(app);
        app.canvas.restore_selection_by_shape_ids(&selected_ids);
        app.canvas.transform_pivot = if app.canvas.selected_shapes.is_empty() {
            None
        } else {
            saved_pivot
        };
    }

    // 矩形マーキースライス直後で未編集の一時分割状態（`has_unsliced_snapshot() == true`）の間は、
    // タイムライン側に既にスライス前の正しい図形が保持されているため上書き同期をスキップし、選択も維持する
    if !app.canvas.has_unsliced_snapshot() {
        crate::canvas_tool::sync_shapes_to_timeline(app);
    }
    app.symbols.sync_active_edit(&app.timeline);

    if app.symbols.edit_stack.len() >= 2 {
        for idx in (1..app.symbols.edit_stack.len()).rev() {
            let sym_id = app.symbols.edit_stack[idx - 1].symbol_id;
            let xform = app.symbols.edit_stack[idx - 1].instance_xform;
            if xform.determinant().abs() <= 1e-9 {
                continue;
            }
            let inv_xform = xform.inverse();
            let working_tl = app.symbols.edit_stack[idx].parent_timeline.clone();
            let mut cache =
                std::mem::take(&mut app.symbols.edit_stack[idx - 1].stage_to_local_cache);
            if let Some(sym_def) = app.symbols.find_symbol_mut(sym_id) {
                sym_def.update_from_working_timeline(&working_tl, inv_xform, &mut cache);
            }
            app.symbols.edit_stack[idx - 1].stage_to_local_cache = cache;
        }
    }
}

/// 現在の `Is2dApp` 上のアクティブドキュメント状態を `app.tabs.tabs[active_index]` スロットへ退避する
/// （タブ間共有の `clipboard` およびウィンドウ共通の `last_canvas_rect` は `app` 側に引き継ぐ）
fn pack_active_into_slot(app: &mut Is2dApp) {
    prepare_active_tab_for_switch_or_save(app, false);
    let shared_clipboard = std::mem::take(&mut app.canvas.clipboard);
    let last_canvas_rect = app.viewport.last_canvas_rect;
    let idx = app.tabs.active_index;
    if let Some(slot) = app.tabs.tabs.get_mut(idx) {
        slot.file_path = app.ui.current_file_path.clone();
        slot.inspect_timeline_frame = app.ui.inspect_timeline_frame;
        slot.show_wireframe = app.ui.show_wireframe;
        slot.viewport = std::mem::take(&mut app.viewport);
        slot.canvas = std::mem::take(&mut app.canvas);
        slot.timeline = std::mem::take(&mut app.timeline);
        slot.timeline_ui = std::mem::take(&mut app.timeline_ui);
        slot.symbols = std::mem::take(&mut app.symbols);
        slot.history = std::mem::take(&mut app.history);
    }
    app.canvas.clipboard = shared_clipboard;
    app.viewport.last_canvas_rect = last_canvas_rect;
}

/// `app.tabs.tabs[target_index]` スロットの状態を `Is2dApp` のアクティブフィールドへ展開する
/// （`std::mem::take` によりアクティブスロット側のフィールドは常に空 `Default` になり、閉じたタブのメモリ残留を防ぐ）
fn unpack_slot_into_active(app: &mut Is2dApp, target_index: usize) {
    let shared_clipboard = std::mem::take(&mut app.canvas.clipboard);
    let last_canvas_rect = app.viewport.last_canvas_rect;
    app.tabs.active_index = target_index;
    if let Some(slot) = app.tabs.tabs.get_mut(target_index) {
        app.ui.current_file_path = slot.file_path.clone();
        app.ui.inspect_timeline_frame = slot.inspect_timeline_frame;
        app.ui.show_wireframe = slot.show_wireframe;
        app.viewport = std::mem::take(&mut slot.viewport);
        app.canvas = std::mem::take(&mut slot.canvas);
        app.timeline = std::mem::take(&mut slot.timeline);
        app.timeline_ui = std::mem::take(&mut slot.timeline_ui);
        app.symbols = std::mem::take(&mut slot.symbols);
        app.history = std::mem::take(&mut slot.history);
    }
    app.viewport.last_canvas_rect = last_canvas_rect;
    app.canvas.clipboard = shared_clipboard;
    app.canvas.paste_count = 0;
    if !app.canvas.selected_shapes.is_empty() && app.ui.select_sub_mode != SelectSubMode::Reshape {
        app.ui.select_sub_mode = SelectSubMode::Reshape;
    }
}

/// 現在アクティブなタブが「起動直後・新規作成直後で一度も編集されていない白紙の無題タブ」であるかを判定する
pub(crate) fn is_active_tab_pristine_empty(app: &Is2dApp) -> bool {
    app.ui.current_file_path.is_none()
        && !app.history.is_dirty()
        && app.history.undo_stack.is_empty()
        && app.history.redo_stack.is_empty()
        && app.symbols.library.is_empty()
        && app.symbols.edit_stack.is_empty()
        && app.viewport.stage_size == Vec2::new(800.0, 450.0)
        && !app.viewport.clip_outside_stage
        && !app.timeline_ui.onion_skin_enabled
        && app.timeline.fps == 24
        && app.timeline.total_frames == 1
        && app.timeline.layers.len() == 1
        && app.timeline.layers[0].name == "Layer 1"
        && app.timeline.layers[0].visible
        && !app.timeline.layers[0].locked
        && app.timeline.layers[0].frame_count == 1
        && app.timeline.layers[0].keyframes.len() == 1
        && app.timeline.layers[0].keyframes[0].cel.is_empty()
        && app.canvas.shapes.is_empty()
}

/// 指定したインデックス `index` のタブに未保存変更（`is_dirty() == true`）があるかを返す
pub(crate) fn is_tab_dirty(app: &Is2dApp, index: usize) -> bool {
    if index == app.tabs.active_index {
        app.history.is_dirty()
    } else {
        app.tabs
            .tabs
            .get(index)
            .is_some_and(|slot| slot.history.is_dirty())
    }
}

/// 対象タブに未保存変更がある場合、対象タブを画面に表示した上でOSネイティブ確認ダイアログ
/// （「はい：保存して閉じる」「いいえ：破棄して閉じる」「キャンセル」）を表示する
fn confirm_unsaved_tab_before_close(
    app: &mut Is2dApp,
    tab_index: usize,
    ctx: &egui::Context,
) -> bool {
    if !is_tab_dirty(app, tab_index) {
        return true;
    }
    let orig_active = app.tabs.active_index;
    if tab_index != orig_active {
        switch_to_tab(app, tab_index, Some(ctx));
    }
    let tab_name = app
        .tabs
        .tabs
        .get(tab_index)
        .map(|slot| slot.display_title(app.ui.current_file_path.as_deref(), false, true))
        .unwrap_or_else(|| "無題".to_owned());

    let result = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("未保存の変更の確認")
        .set_description(format!(
            "「{tab_name}」には未保存の変更があります。\n閉じる前に変更を保存しますか？\n（「はい」= 保存して閉じる / 「いいえ」= 保存せずに破棄 / 「キャンセル」= 閉じない）"
        ))
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show();

    match result {
        rfd::MessageDialogResult::Yes => {
            save_project(app, ctx);
            let saved_ok = !app.history.is_dirty();
            if !saved_ok && tab_index != orig_active && orig_active < app.tabs.tabs.len() {
                switch_to_tab(app, orig_active, Some(ctx));
            }
            saved_ok
        }
        rfd::MessageDialogResult::No => true,
        _ => {
            if tab_index != orig_active && orig_active < app.tabs.tabs.len() {
                switch_to_tab(app, orig_active, Some(ctx));
            }
            false
        }
    }
}

/// OSウィンドウの閉じる操作（`×` ボタンや `Cmd+Q` / `Alt+F4`）が要求された際、
/// 未保存変更があるタブについて保存・破棄・キャンセルの確認を行い、キャンセルされた場合は `CancelClose` を発行する
/// （後半タブの保存によって先行タブのファイルパスが上書きされ `mark_unsaved()` で新たに Dirty 化したケースも漏れなく再チェックする）
pub(crate) fn handle_viewport_close_request(app: &mut Is2dApp, ctx: &egui::Context) {
    if !ctx.input(|i| i.viewport().close_requested()) {
        return;
    }
    let mut discarded_doc_ids = HashSet::new();
    while let Some(idx) = (0..app.tabs.tabs.len())
        .find(|&i| is_tab_dirty(app, i) && !discarded_doc_ids.contains(&app.tabs.tabs[i].doc_id))
    {
        let doc_id = app.tabs.tabs[idx].doc_id;
        if !confirm_unsaved_tab_before_close(app, idx, ctx) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        if is_tab_dirty(app, idx) {
            discarded_doc_ids.insert(doc_id);
        }
    }
}

/// 指定したインデックス `target_index` のドキュメントタブへ切り替える
pub(crate) fn switch_to_tab(
    app: &mut Is2dApp,
    target_index: usize,
    ctx: Option<&egui::Context>,
) -> bool {
    if target_index >= app.tabs.tabs.len() || target_index == app.tabs.active_index {
        return false;
    }
    pack_active_into_slot(app);
    unpack_slot_into_active(app, target_index);

    if let Some(c) = ctx {
        surrender_focused_widget(c);
        update_window_title(app, c);
    }
    true
}

/// 指定したインデックス `close_index` のドキュメントタブを閉じる（最後の1タブを閉じた場合は新規の無題タブ `無題-1` にリセット）
pub(crate) fn close_tab(app: &mut Is2dApp, close_index: usize, ctx: Option<&egui::Context>) {
    if close_index >= app.tabs.tabs.len() {
        return;
    }
    let orig_active = app.tabs.active_index;
    if let Some(c) = ctx {
        if !confirm_unsaved_tab_before_close(app, close_index, c) {
            return;
        }
    }

    if app.tabs.tabs.len() <= 1 {
        // 最後の1タブを閉じた場合は連番をリセットし、白紙の「無題-1」へ初期化する
        crate::canvas_tool::stop_playback(app);
        crate::canvas_tool::cancel_active_drag(app);
        let shared_clipboard = std::mem::take(&mut app.canvas.clipboard);
        let last_canvas_rect = app.viewport.last_canvas_rect;
        app.tabs.next_untitled_number = 1;
        let doc_id = app.tabs.alloc_doc_id();
        let untitled_num = app.tabs.alloc_untitled_number();

        app.tabs.tabs[0] = DocumentSlot::new_empty(doc_id, untitled_num);
        app.tabs.active_index = 0;
        app.timeline = Timeline::default();
        app.timeline_ui = TimelineUiState::default();
        app.symbols = SymbolManager::default();
        app.history = UndoHistory::default();
        app.canvas = CanvasState::default();
        app.canvas.clipboard = shared_clipboard;
        app.viewport = ViewportState {
            last_canvas_rect,
            ..Default::default()
        };
        app.viewport.reset_view();
        set_active_file_path(app, None);
        app.ui.inspect_timeline_frame = false;
        app.ui.show_wireframe = false;
        app.ui.editing_symbol_playback = false;
        app.ui.select_sub_mode = SelectSubMode::Reshape;
        crate::canvas_tool::load_shapes_from_timeline(app);
        if let Some(c) = ctx {
            surrender_focused_widget(c);
            update_window_title(app, c);
        }
        return;
    }

    if close_index == app.tabs.active_index {
        crate::canvas_tool::stop_playback(app);
        crate::canvas_tool::cancel_active_drag(app);
        app.timeline_ui.cancel_drag();
        app.viewport.pan_last_screen_pos = None;

        app.tabs.tabs.remove(close_index);
        let next_active = if orig_active != close_index {
            if close_index < orig_active {
                orig_active - 1
            } else {
                orig_active.min(app.tabs.tabs.len() - 1)
            }
        } else {
            close_index.min(app.tabs.tabs.len() - 1)
        };
        unpack_slot_into_active(app, next_active);
        if let Some(c) = ctx {
            surrender_focused_widget(c);
            update_window_title(app, c);
        }
    } else {
        app.tabs.tabs.remove(close_index);
        if close_index < app.tabs.active_index {
            app.tabs.active_index -= 1;
        }
        if let Some(c) = ctx {
            update_window_title(app, c);
        }
    }
}

/// 現在アクティブなドキュメントタブを閉じる（`Cmd/Ctrl + W`）
pub(crate) fn close_active_tab(app: &mut Is2dApp, ctx: Option<&egui::Context>) {
    let active = app.tabs.active_index;
    close_tab(app, active, ctx);
}

// ============================================================================
// 0.5 別タブ間のクリップボード・シンボル定義自動移植ロジック
// ============================================================================

fn collect_symbol_and_descendants(
    sym_id: u64,
    library: &[SymbolDef],
    reusable: &[Arc<SymbolDef>],
    visited: &mut Vec<u64>,
    out: &mut Vec<Arc<SymbolDef>>,
) {
    if visited.contains(&sym_id) {
        return;
    }
    visited.push(sym_id);
    let Some(sym) = library.iter().find(|s| s.id == sym_id) else {
        return;
    };
    for layer in &sym.timeline.layers {
        for kf in &layer.keyframes {
            for inst in kf.cel.instances.iter() {
                collect_symbol_and_descendants(
                    inst.binding.symbol_id,
                    library,
                    reusable,
                    visited,
                    out,
                );
            }
        }
    }
    if let Some(existing) = reusable.iter().find(|s| s.id == sym_id) {
        out.push(Arc::clone(existing));
    } else {
        let mut cloned = sym.clone();
        cloned.strip_transient_caches();
        out.push(Arc::new(cloned));
    }
}

/// シンボルの内部タイムライン内で使用されている最大のグループID（`group_path` / `instance_gid` / `parent_scope`）を返す
fn max_group_id_in_timeline(tl: &Timeline) -> u64 {
    let mut max_gid = 0u64;
    for layer in &tl.layers {
        for kf in &layer.keyframes {
            for sh in kf.cel.shapes.iter() {
                if let Some(&gid) = sh.group_path.iter().max() {
                    max_gid = max_gid.max(gid);
                }
            }
            for inst in kf.cel.instances.iter() {
                max_gid = max_gid.max(inst.binding.instance_gid);
                if let Some(&gid) = inst.parent_scope.iter().max() {
                    max_gid = max_gid.max(gid);
                }
            }
        }
    }
    max_gid
}

/// 別タブから移植されるシンボルの内部タイムライン内の全グループID（`group_path` / `instance_gid` / `parent_scope`）を
/// 貼り付け先タブの `SymbolManager` で新規採番したIDへ一括リマップし、貼り付け先タブの既存グループとのID衝突を防ぐ
fn remap_timeline_group_ids(tl: &mut Timeline, symbols: &mut SymbolManager) {
    let mut gid_map: HashMap<u64, u64> = HashMap::new();
    let mut map_gid = |old_gid: u64, symbols: &mut SymbolManager| -> u64 {
        *gid_map
            .entry(old_gid)
            .or_insert_with(|| symbols.alloc_group_id())
    };
    for layer in &mut tl.layers {
        for kf in &mut layer.keyframes {
            if kf
                .cel
                .shapes
                .iter()
                .any(|s| !s.group_path.is_empty() || s.symbol_instance.is_some())
            {
                for sh in kf.cel.shapes_mut() {
                    for gid in &mut sh.group_path {
                        *gid = map_gid(*gid, symbols);
                    }
                    if let Some(mut b) = sh.symbol_instance {
                        b.instance_gid = map_gid(b.instance_gid, symbols);
                        sh.symbol_instance = Some(b);
                    }
                }
            }
            if !kf.cel.instances.is_empty() {
                for inst in kf.cel.instances_mut() {
                    inst.binding.instance_gid = map_gid(inst.binding.instance_gid, symbols);
                    for gid in &mut inst.parent_scope {
                        *gid = map_gid(*gid, symbols);
                    }
                }
            }
        }
    }
}

/// `copy_selected_shapes`（`Cmd+C`）実行時に、クリップボード内の図形が参照しているシンボル定義（入れ子含む）を `Arc` 共有スナップショットとして退避する
pub(crate) fn capture_clipboard_symbols(app: &mut Is2dApp) {
    let active_doc_id = app.tabs.active_doc_id();
    if active_doc_id != app.tabs.clipboard_source_doc_id {
        app.tabs.clipboard_imported_map.clear();
    }
    let reusable = if active_doc_id == app.tabs.clipboard_source_doc_id
        && app.symbols.revision == app.tabs.clipboard_source_symbols_revision
    {
        std::mem::take(&mut app.tabs.clipboard_symbols)
    } else {
        app.tabs.clipboard_symbols.clear();
        Vec::new()
    };

    app.tabs.clipboard_source_doc_id = active_doc_id;
    app.tabs.clipboard_source_symbol_id = app.symbols.edit_stack.last().map(|s| s.symbol_id);
    app.tabs.clipboard_source_symbols_revision = app.symbols.revision;

    let mut visited = Vec::new();
    let mut collected = Vec::new();
    for shape in &app.canvas.clipboard {
        if let Some(b) = shape.symbol_instance {
            collect_symbol_and_descendants(
                b.symbol_id,
                &app.symbols.library,
                &reusable,
                &mut visited,
                &mut collected,
            );
        }
    }
    app.tabs.clipboard_symbols = collected;
}

/// 現在のタブのライブラリに `src_sym_id` に対応する有効な既存シンボルが存在し、
/// かつそのシンボルが現在参照しているすべての子シンボルもライブラリ内に実在していればそのIDを返す
fn existing_reusable_target_symbol_id(app: &Is2dApp, src_sym_id: u64) -> Option<u64> {
    let target_doc_id = app.tabs.active_doc_id();
    let source_doc_id = app.tabs.clipboard_source_doc_id;
    let candidate = (target_doc_id == source_doc_id)
        .then_some(src_sym_id)
        .filter(|&id| app.symbols.find_symbol(id).is_some())
        .or_else(|| {
            app.tabs
                .clipboard_imported_map
                .get(&(target_doc_id, src_sym_id))
                .copied()
                .filter(|&id| app.symbols.find_symbol(id).is_some())
        })?;

    let sym_def = app.symbols.find_symbol(candidate)?;
    let all_children_exist = sym_def.timeline.layers.iter().all(|l| {
        l.keyframes.iter().all(|kf| {
            kf.cel
                .instances
                .iter()
                .all(|inst| app.symbols.find_symbol(inst.binding.symbol_id).is_some())
        })
    });
    all_children_exist.then_some(candidate)
}

fn can_paste_clipboard_symbol_inner(
    app: &Is2dApp,
    src_sym_id: u64,
    visited: &mut Vec<u64>,
) -> bool {
    if app.symbols.edit_stack.is_empty() {
        return true;
    }
    if visited.contains(&src_sym_id) {
        return true;
    }
    visited.push(src_sym_id);

    if let Some(live_id) = existing_reusable_target_symbol_id(app, src_sym_id) {
        return app.symbols.can_place_symbol_in_current_context(live_id);
    }

    if app.tabs.active_doc_id() == app.tabs.clipboard_source_doc_id
        && app
            .symbols
            .edit_stack
            .iter()
            .any(|sess| sess.symbol_id == src_sym_id)
    {
        return false;
    }

    let Some(src_sym) = app
        .tabs
        .clipboard_symbols
        .iter()
        .find(|s| s.id == src_sym_id)
    else {
        return true;
    };

    for layer in &src_sym.timeline.layers {
        for kf in &layer.keyframes {
            for inst in kf.cel.instances.iter() {
                if !can_paste_clipboard_symbol_inner(app, inst.binding.symbol_id, visited) {
                    return false;
                }
            }
        }
    }
    true
}

/// クリップボード内のシンボル `src_sym_id` が現在の編集コンテキスト（`edit_stack`）に対して循環参照を起こさずに配置可能かを判定する
pub(crate) fn can_paste_clipboard_symbol(app: &Is2dApp, src_sym_id: u64) -> bool {
    let mut visited = Vec::new();
    can_paste_clipboard_symbol_inner(app, src_sym_id, &mut visited)
}

/// `paste_clipboard`（`Cmd+V`）実行前に、現在のタブでクリップボード内の図形がすべて自己・親シンボルの循環参照により配置不可能かを判定する
pub(crate) fn would_clipboard_paste_all_cycle(app: &Is2dApp) -> bool {
    if app.canvas.clipboard.is_empty() || app.symbols.edit_stack.is_empty() {
        return false;
    }
    app.canvas.clipboard.iter().all(|shape| {
        shape
            .symbol_instance
            .is_some_and(|b| !can_paste_clipboard_symbol(app, b.symbol_id))
    })
}

fn collect_needed_clipboard_imports(
    app: &Is2dApp,
    src_sym_id: u64,
    local_sym_map: &mut HashMap<u64, u64>,
    needed_imports: &mut HashSet<u64>,
    visited: &mut HashSet<u64>,
) {
    if !visited.insert(src_sym_id) {
        return;
    }
    if let Some(valid_id) = existing_reusable_target_symbol_id(app, src_sym_id) {
        local_sym_map.insert(src_sym_id, valid_id);
        return;
    }
    needed_imports.insert(src_sym_id);
    if let Some(src_sym) = app
        .tabs
        .clipboard_symbols
        .iter()
        .find(|s| s.id == src_sym_id)
    {
        for layer in &src_sym.timeline.layers {
            for kf in &layer.keyframes {
                for inst in kf.cel.instances.iter() {
                    collect_needed_clipboard_imports(
                        app,
                        inst.binding.symbol_id,
                        local_sym_map,
                        needed_imports,
                        visited,
                    );
                }
            }
        }
    }
}

/// `paste_clipboard`（`Cmd+V`）実行時に、クリップボードの不変スナップショットを破壊せず、
/// 循環参照となる図形を事前に除外した上で、現在のタブのライブラリに不足しているシンボル定義のみを自動移植し、
/// 現在のタブの有効な `symbol_id` に解決されたペースト用 `(Vec<FillShape>, had_cycle)` を返す
pub(crate) fn resolve_clipboard_for_paste(app: &mut Is2dApp) -> (Vec<FillShape>, bool) {
    let mut had_cycle = false;
    let mut resolved_shapes = Vec::with_capacity(app.canvas.clipboard.len());
    for shape in &app.canvas.clipboard {
        if let Some(b) = shape.symbol_instance {
            if !can_paste_clipboard_symbol(app, b.symbol_id) {
                had_cycle = true;
                continue;
            }
        }
        resolved_shapes.push(shape.clone());
    }

    if resolved_shapes.is_empty() || app.tabs.clipboard_symbols.is_empty() {
        return (resolved_shapes, had_cycle);
    }

    let target_doc_id = app.tabs.active_doc_id();
    let source_doc_id = app.tabs.clipboard_source_doc_id;
    let mut local_sym_map: HashMap<u64, u64> = HashMap::new();
    let mut needed_imports: HashSet<u64> = HashSet::new();
    let mut visited: HashSet<u64> = HashSet::new();

    for shape in &resolved_shapes {
        if let Some(b) = shape.symbol_instance {
            collect_needed_clipboard_imports(
                app,
                b.symbol_id,
                &mut local_sym_map,
                &mut needed_imports,
                &mut visited,
            );
        }
    }

    let mut any_imported = false;

    for idx in 0..app.tabs.clipboard_symbols.len() {
        let src_sym_id = app.tabs.clipboard_symbols[idx].id;
        if !needed_imports.contains(&src_sym_id) {
            continue;
        }
        let mut imported = (*app.tabs.clipboard_symbols[idx]).clone();

        let new_id =
            if target_doc_id == source_doc_id && app.symbols.find_symbol(imported.id).is_none() {
                app.symbols.next_symbol_id = app
                    .symbols
                    .next_symbol_id
                    .max(imported.id.saturating_add(1));
                imported.id
            } else {
                let id = app.symbols.next_symbol_id;
                app.symbols.next_symbol_id = app.symbols.next_symbol_id.saturating_add(1);
                id
            };

        local_sym_map.insert(imported.id, new_id);
        app.tabs
            .clipboard_imported_map
            .insert((target_doc_id, imported.id), new_id);

        imported.id = new_id;
        if app.symbols.library.iter().any(|s| s.name == imported.name) {
            let base_name = imported.name.clone();
            let mut suffix = 2usize;
            loop {
                let candidate = format!("{base_name} ({suffix})");
                if !app.symbols.library.iter().any(|s| s.name == candidate) {
                    imported.name = candidate;
                    break;
                }
                suffix = suffix.saturating_add(1);
            }
        }
        for layer in &mut imported.timeline.layers {
            for kf in &mut layer.keyframes {
                for inst in kf.cel.instances_mut() {
                    if let Some(&mapped_child_id) = local_sym_map.get(&inst.binding.symbol_id) {
                        inst.binding.symbol_id = mapped_child_id;
                    }
                }
            }
        }
        remap_timeline_group_ids(&mut imported.timeline, &mut app.symbols);
        let max_internal_gid = max_group_id_in_timeline(&imported.timeline);
        app.symbols.next_group_id = app
            .symbols
            .next_group_id
            .max(max_internal_gid.saturating_add(1));

        app.symbols.library.push(imported);
        if app.symbols.selected_library_symbol.is_none() {
            app.symbols.selected_library_symbol = Some(new_id);
        }
        any_imported = true;
    }

    if any_imported {
        app.symbols.bump_revision();
    }

    for shape in &mut resolved_shapes {
        if let Some(mut b) = shape.symbol_instance {
            if let Some(&mapped_id) = local_sym_map.get(&b.symbol_id) {
                b.symbol_id = mapped_id;
                shape.symbol_instance = Some(b);
            }
        }
    }
    (resolved_shapes, had_cycle)
}

// ============================================================================
// 1. 保存用シリアライズデータ構造（DTO）
// ============================================================================

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SavedTweenRotate {
    #[default]
    Auto,
    Clockwise,
    CounterClockwise,
}

impl From<TweenRotate> for SavedTweenRotate {
    fn from(value: TweenRotate) -> Self {
        match value {
            TweenRotate::Auto => Self::Auto,
            TweenRotate::Clockwise => Self::Clockwise,
            TweenRotate::CounterClockwise => Self::CounterClockwise,
        }
    }
}

impl From<SavedTweenRotate> for TweenRotate {
    fn from(value: SavedTweenRotate) -> Self {
        match value {
            SavedTweenRotate::Auto => Self::Auto,
            SavedTweenRotate::Clockwise => Self::Clockwise,
            SavedTweenRotate::CounterClockwise => Self::CounterClockwise,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SavedSymbolLoopMode {
    #[default]
    Loop,
    PlayOnce,
    SingleFrame,
}

impl From<SymbolLoopMode> for SavedSymbolLoopMode {
    fn from(value: SymbolLoopMode) -> Self {
        match value {
            SymbolLoopMode::Loop => Self::Loop,
            SymbolLoopMode::PlayOnce => Self::PlayOnce,
            SymbolLoopMode::SingleFrame => Self::SingleFrame,
        }
    }
}

impl From<SavedSymbolLoopMode> for SymbolLoopMode {
    fn from(value: SavedSymbolLoopMode) -> Self {
        match value {
            SavedSymbolLoopMode::Loop => Self::Loop,
            SavedSymbolLoopMode::PlayOnce => Self::PlayOnce,
            SavedSymbolLoopMode::SingleFrame => Self::SingleFrame,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSymbolInstanceBinding {
    pub(crate) instance_gid: u64,
    pub(crate) symbol_id: u64,
    pub(crate) xform: [f64; 6],
    #[serde(default)]
    pub(crate) pivot: [f64; 2],
    #[serde(default)]
    pub(crate) loop_mode: SavedSymbolLoopMode,
    #[serde(default)]
    pub(crate) first_frame: usize,
}

impl From<SymbolInstanceBinding> for SavedSymbolInstanceBinding {
    fn from(b: SymbolInstanceBinding) -> Self {
        Self {
            instance_gid: b.instance_gid,
            symbol_id: b.symbol_id,
            xform: b.xform.as_coeffs(),
            pivot: [b.pivot.x, b.pivot.y],
            loop_mode: b.loop_mode.into(),
            first_frame: b.first_frame,
        }
    }
}

impl From<SavedSymbolInstanceBinding> for SymbolInstanceBinding {
    fn from(s: SavedSymbolInstanceBinding) -> Self {
        let xform = if s.xform.iter().all(|v| v.is_finite())
            && (s.xform[0] * s.xform[3] - s.xform[1] * s.xform[2]).abs() > 1e-12
        {
            Affine::new(s.xform)
        } else {
            Affine::IDENTITY
        };
        let pivot = if s.pivot[0].is_finite() && s.pivot[1].is_finite() {
            Point::new(s.pivot[0], s.pivot[1])
        } else {
            Point::ZERO
        };
        Self {
            instance_gid: s.instance_gid,
            symbol_id: s.symbol_id,
            xform,
            pivot,
            loop_mode: s.loop_mode.into(),
            first_frame: s.first_frame.min(MAX_TIMELINE_FRAMES.saturating_sub(1)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSymbolInstance {
    pub(crate) binding: SavedSymbolInstanceBinding,
    #[serde(default)]
    pub(crate) layer_id: u64,
    #[serde(default)]
    pub(crate) parent_scope: Vec<u64>,
    #[serde(default)]
    pub(crate) shape_order_index: usize,
}

impl From<&SymbolInstance> for SavedSymbolInstance {
    fn from(inst: &SymbolInstance) -> Self {
        Self {
            binding: inst.binding.into(),
            layer_id: inst.layer_id,
            parent_scope: inst.parent_scope.clone(),
            shape_order_index: inst.shape_order_index,
        }
    }
}

impl From<SavedSymbolInstance> for SymbolInstance {
    fn from(s: SavedSymbolInstance) -> Self {
        Self {
            binding: s.binding.into(),
            layer_id: s.layer_id,
            parent_scope: s.parent_scope,
            shape_order_index: s.shape_order_index,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedContour {
    pub(crate) segments: Vec<[[f64; 2]; 3]>,
}

impl From<&Contour> for SavedContour {
    fn from(c: &Contour) -> Self {
        Self {
            segments: c
                .segments
                .iter()
                .map(|seg| {
                    [
                        [seg.p0.x, seg.p0.y],
                        [seg.p1.x, seg.p1.y],
                        [seg.p2.x, seg.p2.y],
                    ]
                })
                .collect(),
        }
    }
}

impl From<SavedContour> for Contour {
    fn from(s: SavedContour) -> Self {
        let segs = s
            .segments
            .into_iter()
            .take(MAX_CONTOUR_SEGMENTS)
            .filter(|pts| pts.iter().flatten().all(|v| v.is_finite()))
            .map(|pts| {
                QuadBez::new(
                    Point::new(pts[0][0], pts[0][1]),
                    Point::new(pts[1][0], pts[1][1]),
                    Point::new(pts[2][0], pts[2][1]),
                )
            })
            .collect();
        let mut contour = Contour::new(segs);
        contour.enforce_continuity();
        contour
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedFillShape {
    pub(crate) color: [u8; 4],
    pub(crate) exterior: SavedContour,
    #[serde(default)]
    pub(crate) holes: Vec<SavedContour>,
    #[serde(default)]
    pub(crate) layer_id: u64,
    #[serde(default)]
    pub(crate) group_path: Vec<u64>,
}

impl From<&FillShape> for SavedFillShape {
    fn from(shape: &FillShape) -> Self {
        Self {
            color: shape.color.to_array(),
            exterior: (&shape.exterior).into(),
            holes: shape.holes.iter().map(SavedContour::from).collect(),
            layer_id: shape.layer_id,
            group_path: shape.group_path.clone(),
        }
    }
}

impl From<SavedFillShape> for FillShape {
    fn from(s: SavedFillShape) -> Self {
        let color =
            Color32::from_rgba_premultiplied(s.color[0], s.color[1], s.color[2], s.color[3]);
        let mut exterior: Contour = s.exterior.into();
        if exterior.signed_area() < 0.0 {
            exterior.reverse();
        }
        let holes: Vec<Contour> = s
            .holes
            .into_iter()
            .map(Contour::from)
            .filter(|h| h.segments.len() >= 2 && h.signed_area().abs() > 1e-4)
            .map(|mut h| {
                if h.signed_area() > 0.0 {
                    h.reverse();
                }
                h
            })
            .collect();
        FillShape::new(color, exterior, holes)
            .with_layer_id(s.layer_id)
            .with_group_path(s.group_path)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedKeyframe {
    pub(crate) start_frame: usize,
    #[serde(default)]
    pub(crate) tween: bool,
    #[serde(default)]
    pub(crate) tween_rotate: SavedTweenRotate,
    #[serde(default)]
    pub(crate) shapes: Vec<SavedFillShape>,
    #[serde(default)]
    pub(crate) instances: Vec<SavedSymbolInstance>,
}

impl From<&Keyframe> for SavedKeyframe {
    fn from(kf: &Keyframe) -> Self {
        Self {
            start_frame: kf.start_frame,
            tween: kf.tween,
            tween_rotate: kf.tween_rotate.into(),
            shapes: kf.cel.shapes.iter().map(SavedFillShape::from).collect(),
            instances: kf
                .cel
                .instances
                .iter()
                .map(SavedSymbolInstance::from)
                .collect(),
        }
    }
}

impl From<SavedKeyframe> for Keyframe {
    fn from(s: SavedKeyframe) -> Self {
        let shapes: Vec<FillShape> = s
            .shapes
            .into_iter()
            .map(FillShape::from)
            .filter(|sh| sh.exterior.segments.len() >= 2 && sh.exterior.signed_area().abs() > 1e-4)
            .collect();
        let mut seen_gids = HashSet::with_capacity(s.instances.len());
        let instances: Vec<SymbolInstance> = s
            .instances
            .into_iter()
            .map(SymbolInstance::from)
            .filter(|inst| {
                inst.binding.instance_gid > 0
                    && inst.binding.symbol_id > 0
                    && seen_gids.insert(inst.binding.instance_gid)
            })
            .collect();
        let cel = Cel::new_with_instances(shapes, instances);
        let tween = s.tween && cel.single_symbol_instance().is_some();
        Self {
            start_frame: s.start_frame,
            cel,
            tween,
            tween_rotate: s.tween_rotate.into(),
        }
    }
}

fn default_layer_visible() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedLayer {
    pub(crate) id: u64,
    pub(crate) name: String,
    #[serde(default = "default_layer_visible")]
    pub(crate) visible: bool,
    #[serde(default)]
    pub(crate) locked: bool,
    pub(crate) frame_count: usize,
    #[serde(default)]
    pub(crate) keyframes: Vec<SavedKeyframe>,
}

impl From<&Layer> for SavedLayer {
    fn from(layer: &Layer) -> Self {
        Self {
            id: layer.id,
            name: layer.name.clone(),
            visible: layer.visible,
            locked: layer.locked,
            frame_count: layer.frame_count,
            keyframes: layer.keyframes.iter().map(SavedKeyframe::from).collect(),
        }
    }
}

impl From<SavedLayer> for Layer {
    fn from(s: SavedLayer) -> Self {
        let frame_count = s.frame_count.clamp(1, MAX_TIMELINE_FRAMES);
        let mut keyframes: Vec<Keyframe> = s
            .keyframes
            .into_iter()
            .map(Keyframe::from)
            .filter(|kf| kf.start_frame < frame_count)
            .collect();
        keyframes.sort_by_key(|kf| kf.start_frame);
        keyframes.dedup_by_key(|kf| kf.start_frame);
        if keyframes.first().is_none_or(|kf| kf.start_frame != 0) {
            keyframes.insert(0, Keyframe::new_blank(0));
        }
        Self {
            id: s.id.max(1),
            name: s.name,
            visible: s.visible,
            locked: s.locked,
            frame_count,
            keyframes,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedTimeline {
    pub(crate) fps: u32,
    pub(crate) total_frames: usize,
    pub(crate) current_frame: usize,
    pub(crate) active_layer: usize,
    #[serde(default)]
    pub(crate) next_layer_number: usize,
    #[serde(default)]
    pub(crate) next_layer_id: u64,
    pub(crate) layers: Vec<SavedLayer>,
}

impl From<&Timeline> for SavedTimeline {
    fn from(tl: &Timeline) -> Self {
        Self {
            fps: tl.fps,
            total_frames: tl.total_frames,
            current_frame: tl.current_frame,
            active_layer: tl.active_layer,
            next_layer_number: tl.next_layer_number,
            next_layer_id: tl.next_layer_id,
            layers: tl.layers.iter().map(SavedLayer::from).collect(),
        }
    }
}

impl From<SavedTimeline> for Timeline {
    fn from(s: SavedTimeline) -> Self {
        let mut layers: Vec<Layer> = s
            .layers
            .into_iter()
            .take(MAX_LAYERS_PER_TIMELINE)
            .map(Layer::from)
            .collect();
        if layers.is_empty() {
            layers.push(Layer::new_with_id(1, "Layer 1"));
        }

        // 重複する Layer::id があれば一意なIDへ再採番し、各キーフレーム内の図形・インスタンスの layer_id も正規化する
        let mut seen_layer_ids = HashSet::with_capacity(layers.len());
        let mut max_layer_id = layers.iter().map(|l| l.id).max().unwrap_or(1).max(1);
        for layer in &mut layers {
            if !seen_layer_ids.insert(layer.id) {
                max_layer_id = max_layer_id.saturating_add(1);
                layer.id = max_layer_id;
                seen_layer_ids.insert(layer.id);
            }
            let lid = layer.id;
            for kf in &mut layer.keyframes {
                for sh in kf.cel.shapes_mut() {
                    sh.layer_id = lid;
                }
                for inst in kf.cel.instances_mut() {
                    inst.layer_id = lid;
                }
            }
        }

        let total_frames = layers
            .iter()
            .map(|l| l.frame_count)
            .max()
            .unwrap_or(1)
            .max(s.total_frames.clamp(1, MAX_TIMELINE_FRAMES));
        let active_layer = s.active_layer.min(layers.len().saturating_sub(1));
        let current_frame = s.current_frame.min(total_frames.saturating_sub(1));
        let mut tl = Self {
            layers,
            current_frame,
            active_layer,
            total_frames,
            fps: s.fps.clamp(1, 60),
            next_layer_number: s.next_layer_number.max(2),
            next_layer_id: s.next_layer_id.max(max_layer_id.saturating_add(1)).max(2),
            cell_selection: Some(TimelineCellSelection::single(active_layer, current_frame)),
        };
        tl.sync_total_frames();
        tl
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSymbolDef {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) timeline: SavedTimeline,
}

impl From<&SymbolDef> for SavedSymbolDef {
    fn from(sym: &SymbolDef) -> Self {
        Self {
            id: sym.id,
            name: sym.name.clone(),
            timeline: (&sym.timeline).into(),
        }
    }
}

impl From<SavedSymbolDef> for SymbolDef {
    fn from(s: SavedSymbolDef) -> Self {
        let mut sym = SymbolDef::new(s.id, s.name, Vec::new());
        sym.timeline = s.timeline.into();
        sym
    }
}

/// `.is2d` ファイルのルート構造体
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProjectDocument {
    pub(crate) version: u32,
    pub(crate) stage_size: [f32; 2],
    pub(crate) timeline: SavedTimeline,
    #[serde(default)]
    pub(crate) library: Vec<SavedSymbolDef>,
    #[serde(default)]
    pub(crate) next_symbol_id: u64,
    #[serde(default)]
    pub(crate) next_group_id: u64,
}

impl ProjectDocument {
    /// 現在の `Is2dApp` から保存用の `ProjectDocument` を生成する
    pub(crate) fn from_app(app: &Is2dApp) -> Self {
        let root_timeline = app
            .symbols
            .edit_stack
            .first()
            .map(|sess| &sess.parent_timeline)
            .unwrap_or(&app.timeline);
        let mut saved_timeline: SavedTimeline = root_timeline.into();
        saved_timeline.fps = app.timeline.fps.clamp(1, 60);

        Self {
            version: PROJECT_FORMAT_VERSION,
            stage_size: [app.viewport.stage_size.x, app.viewport.stage_size.y],
            timeline: saved_timeline,
            library: app
                .symbols
                .library
                .iter()
                .map(SavedSymbolDef::from)
                .collect(),
            next_symbol_id: app.symbols.next_symbol_id,
            next_group_id: app.symbols.next_group_id,
        }
    }

    /// 保存前にドキュメント内の全浮動小数点値（ステージサイズ・座標・変換行列・ピボット）が有限値（非 `NaN` / 非 `Infinity`）であるかを検証する
    pub(crate) fn validate_finite(&self) -> Result<(), String> {
        if !self.stage_size[0].is_finite() || !self.stage_size[1].is_finite() {
            return Err("ステージサイズに無効な数値（NaN / Infinity）が含まれています".to_owned());
        }
        let check_timeline = |tl: &SavedTimeline| -> bool {
            tl.layers.iter().all(|layer| {
                layer.keyframes.iter().all(|kf| {
                    let shapes_ok = kf.shapes.iter().all(|sh| {
                        sh.exterior
                            .segments
                            .iter()
                            .all(|pts| pts.iter().flatten().all(|v| v.is_finite()))
                            && sh.holes.iter().all(|h| {
                                h.segments
                                    .iter()
                                    .all(|pts| pts.iter().flatten().all(|v| v.is_finite()))
                            })
                    });
                    let instances_ok = kf.instances.iter().all(|inst| {
                        inst.binding.xform.iter().all(|v| v.is_finite())
                            && inst.binding.pivot.iter().all(|v| v.is_finite())
                    });
                    shapes_ok && instances_ok
                })
            })
        };
        if !check_timeline(&self.timeline)
            || !self.library.iter().all(|sym| check_timeline(&sym.timeline))
        {
            return Err(
                "プロジェクトデータに無効な数値（NaN / Infinity）が含まれています".to_owned(),
            );
        }
        Ok(())
    }

    /// 読み込んだ `ProjectDocument` を現在のアクティブドキュメントへ適用し、作業キャンバスや履歴を初期化する
    pub(crate) fn apply_to_app(self, app: &mut Is2dApp) {
        let shared_clipboard = std::mem::take(&mut app.canvas.clipboard);
        let mut timeline: Timeline = self.timeline.into();

        // 重複または不正（id == 0）な SymbolDef::id を一意な新しいIDへ再採番してライブラリを構築
        let mut seen_sym_ids = HashSet::with_capacity(self.library.len());
        let mut next_fallback_sym_id = self
            .library
            .iter()
            .map(|s| s.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        let mut library: Vec<SymbolDef> = Vec::with_capacity(self.library.len());
        for saved_sym in self.library {
            let mut sym = SymbolDef::from(saved_sym);
            if sym.id == 0 || !seen_sym_ids.insert(sym.id) {
                sym.id = next_fallback_sym_id;
                next_fallback_sym_id = next_fallback_sym_id.saturating_add(1);
                seen_sym_ids.insert(sym.id);
            }
            library.push(sym);
        }

        // 存在しない symbol_id（孤児インスタンス）をルートタイムラインおよび各シンボルタイムラインから除去し、
        // 単一シンボル条件を満たさなくなったキーフレームの tween フラグも正規化する
        for tl in std::iter::once(&mut timeline).chain(library.iter_mut().map(|s| &mut s.timeline))
        {
            for layer in &mut tl.layers {
                for kf in &mut layer.keyframes {
                    if kf
                        .cel
                        .instances
                        .iter()
                        .any(|inst| !seen_sym_ids.contains(&inst.binding.symbol_id))
                    {
                        kf.cel
                            .instances_mut()
                            .retain(|inst| seen_sym_ids.contains(&inst.binding.symbol_id));
                    }
                    kf.tween &= kf.cel.single_symbol_instance().is_some();
                }
            }
        }

        let max_sym_id = library.iter().map(|s| s.id).max().unwrap_or(0);
        let mut max_gid = max_group_id_in_timeline(&timeline);
        for sym in &library {
            max_gid = max_gid.max(max_group_id_in_timeline(&sym.timeline));
        }

        let selected_library_symbol = library.first().map(|s| s.id);
        app.timeline = timeline;
        app.timeline_ui = TimelineUiState::default();
        app.symbols = SymbolManager {
            library,
            next_symbol_id: self.next_symbol_id.max(max_sym_id.saturating_add(1)).max(1),
            selected_library_symbol,
            actively_renaming_symbol: None,
            edit_stack: Vec::new(),
            next_group_id: self.next_group_id.max(max_gid.saturating_add(1)).max(1),
            revision: crate::vector_engine::next_mesh_revision(),
        };

        // 外部改変された JSON にシンボルの自己・相互循環参照が含まれていた場合はサイクル辺を除去して DAG を保証する
        let mut cycle_edge_cache: HashMap<(u64, u64), bool> = HashMap::new();
        for sym_idx in 0..app.symbols.library.len() {
            let parent_sym_id = app.symbols.library[sym_idx].id;
            let num_layers = app.symbols.library[sym_idx].timeline.layers.len();
            for l_idx in 0..num_layers {
                let num_kfs = app.symbols.library[sym_idx].timeline.layers[l_idx]
                    .keyframes
                    .len();
                for kf_idx in 0..num_kfs {
                    let has_cycle = app.symbols.library[sym_idx].timeline.layers[l_idx].keyframes
                        [kf_idx]
                        .cel
                        .instances
                        .iter()
                        .any(|inst| {
                            let child_id = inst.binding.symbol_id;
                            *cycle_edge_cache
                                .entry((parent_sym_id, child_id))
                                .or_insert_with(|| {
                                    app.symbols.would_create_symbol_cycle(parent_sym_id, child_id)
                                })
                        });
                    if has_cycle {
                        let instances: Vec<SymbolInstance> =
                            app.symbols.library[sym_idx].timeline.layers[l_idx].keyframes[kf_idx]
                                .cel
                                .instances
                                .iter()
                                .filter(|inst| {
                                    let child_id = inst.binding.symbol_id;
                                    !*cycle_edge_cache
                                        .entry((parent_sym_id, child_id))
                                        .or_insert_with(|| {
                                            app.symbols
                                                .would_create_symbol_cycle(parent_sym_id, child_id)
                                        })
                                })
                                .cloned()
                                .collect();
                        let kf = &mut app.symbols.library[sym_idx].timeline.layers[l_idx].keyframes
                            [kf_idx];
                        *kf.cel.instances_mut() = instances;
                        kf.tween &= kf.cel.single_symbol_instance().is_some();
                        cycle_edge_cache.clear();
                    }
                }
            }
        }

        app.history = UndoHistory::default();
        app.canvas = CanvasState::default();
        app.canvas.clipboard = shared_clipboard;
        app.ui.inspect_timeline_frame = false;
        app.ui.show_wireframe = false;
        app.ui.editing_symbol_playback = false;
        app.ui.select_sub_mode = SelectSubMode::Reshape;

        let w = if self.stage_size[0].is_finite() {
            self.stage_size[0].clamp(100.0, 3840.0)
        } else {
            800.0
        };
        let h = if self.stage_size[1].is_finite() {
            self.stage_size[1].clamp(100.0, 2160.0)
        } else {
            450.0
        };
        let last_canvas_rect = app.viewport.last_canvas_rect;
        app.viewport = ViewportState {
            stage_size: Vec2::new(w, h),
            last_canvas_rect,
            ..Default::default()
        };
        app.viewport.reset_view();
        crate::canvas_tool::load_shapes_from_timeline(app);
    }
}

// ============================================================================
// 2. ファイル読み書きコア関数 & OSファイルダイアログ連携
// ============================================================================

/// 2つのパスが同一のファイルを指しているか（`canonicalize` または文字列表現の一致）を判定する
fn paths_refer_to_same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// 現在のアクティブタブ（ファイル名・無題番号・未保存 `*` 状態）に応じてウィンドウタイトルバーの表示を更新する
pub(crate) fn update_window_title(app: &mut Is2dApp, ctx: &egui::Context) {
    let tab_title = app
        .tabs
        .tabs
        .get(app.tabs.active_index)
        .map(|slot| {
            slot.display_title(
                app.ui.current_file_path.as_deref(),
                app.history.is_dirty(),
                true,
            )
        })
        .unwrap_or_else(|| "無題-1".to_owned());
    let full_title = format!("is2d - {tab_title}");
    if app.tabs.last_window_title != full_title {
        app.tabs.last_window_title = full_title.clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(full_title));
    }
}

type PreSaveRollbackState = (
    Vec<FillShape>,
    Vec<usize>,
    Option<Point>,
    Timeline,
    SymbolManager,
);

fn restore_pre_save_state(app: &mut Is2dApp, pre_save_state: Option<PreSaveRollbackState>) {
    if let Some((shapes, sel, pivot, tl, syms)) = pre_save_state {
        app.canvas.shapes = shapes;
        app.canvas.selected_shapes = sel;
        app.canvas.selection_modified = true;
        app.canvas.transform_pivot = pivot;
        app.timeline = tl;
        app.symbols = syms;
        app.canvas.record_loaded_signatures();
    }
}

/// 指定されたファイルパス `path` へプロジェクトを一時ファイル経由でアトミック保存する
pub(crate) fn save_project_to_path(app: &mut Is2dApp, path: &Path) -> Result<(), String> {
    let pre_save_state = (app.canvas.selection_modified && !app.canvas.selected_shapes.is_empty())
        .then(|| {
            (
                app.canvas.shapes.clone(),
                app.canvas.selected_shapes.clone(),
                app.canvas.transform_pivot,
                app.timeline.clone(),
                app.symbols.clone(),
            )
        });
    prepare_active_tab_for_switch_or_save(app, true);

    let doc = ProjectDocument::from_app(app);
    if let Err(err) = doc.validate_finite() {
        restore_pre_save_state(app, pre_save_state);
        return Err(err);
    }
    let json = match serde_json::to_string(&doc) {
        Ok(s) => s,
        Err(e) => {
            restore_pre_save_state(app, pre_save_state);
            return Err(format!("プロジェクトデータの変換に失敗しました: {e}"));
        }
    };
    if json.len() as u64 > MAX_PROJECT_FILE_BYTES {
        restore_pre_save_state(app, pre_save_state);
        return Err(format!(
            "保存データが最大サイズ上限を超えています ({} MB > 上限 {} MB)",
            json.len() as u64 / (1024 * 1024),
            MAX_PROJECT_FILE_BYTES / (1024 * 1024)
        ));
    }

    let tmp_name = format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project.is2d"),
        std::process::id()
    );
    let tmp_path = path.with_file_name(tmp_name);
    let write_res = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        restore_pre_save_state(app, pre_save_state);
        return Err(format!("一時ファイルの書き込みに失敗しました: {e}"));
    }

    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        restore_pre_save_state(app, pre_save_state);
        return Err(format!("ファイルの保存に失敗しました: {e}"));
    }
    if let Some(parent) = path.parent() {
        if let Ok(dir_file) = std::fs::File::open(parent) {
            let _ = dir_file.sync_all();
        }
    }

    // 別タブが同じファイルパスを開いていた場合、重複タブ化を防ぐため別タブ側のファイル紐付けを解除して未保存状態にする
    let active_idx = app.tabs.active_index;
    let mut detached_indices = Vec::new();
    for (idx, slot) in app.tabs.tabs.iter().enumerate() {
        if idx != active_idx
            && slot
                .file_path
                .as_deref()
                .is_some_and(|sp| paths_refer_to_same_file(sp, path))
        {
            detached_indices.push(idx);
        }
    }
    for idx in detached_indices {
        let alloc_num = if app.tabs.tabs[idx].untitled_number == 0 {
            Some(app.tabs.alloc_untitled_number())
        } else {
            None
        };
        if let Some(slot) = app.tabs.tabs.get_mut(idx) {
            if let Some(num) = alloc_num {
                slot.untitled_number = num;
            }
            slot.file_path = None;
            slot.history.mark_unsaved();
        }
    }

    set_active_file_path(app, Some(path.to_path_buf()));
    app.history.mark_saved();
    Ok(())
}

/// 指定されたファイルパス `path` から `ProjectDocument` を読み込み・バージョン検証して返す
fn read_project_document(path: &Path) -> Result<ProjectDocument, String> {
    use std::io::Read;
    let file =
        std::fs::File::open(path).map_err(|e| format!("ファイルの読み込みに失敗しました: {e}"))?;
    if let Ok(meta) = file.metadata() {
        if meta.len() > MAX_PROJECT_FILE_BYTES {
            return Err(format!(
                "プロジェクトファイルが大きすぎます ({} MB > 上限 {} MB)",
                meta.len() / (1024 * 1024),
                MAX_PROJECT_FILE_BYTES / (1024 * 1024)
            ));
        }
    }
    let mut bytes = Vec::new();
    file.take(MAX_PROJECT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("ファイルの読み込みに失敗しました: {e}"))?;
    if bytes.len() as u64 > MAX_PROJECT_FILE_BYTES {
        return Err(format!(
            "プロジェクトファイルが大きすぎます ({} MB > 上限 {} MB)",
            bytes.len() as u64 / (1024 * 1024),
            MAX_PROJECT_FILE_BYTES / (1024 * 1024)
        ));
    }
    let doc: ProjectDocument = serde_json::from_slice(&bytes)
        .map_err(|e| format!("プロジェクトファイルの形式が正しくありません: {e}"))?;

    if doc.version == 0 || doc.version > PROJECT_FORMAT_VERSION {
        return Err(format!(
            "未対応のプロジェクトバージョンです (version: {})",
            doc.version
        ));
    }
    Ok(doc)
}

/// 指定パスのプロジェクトを「新しいタブ」（または既存の同一ファイルタブ／空の無題タブ）として開く
pub(crate) fn open_project_in_tab(
    app: &mut Is2dApp,
    path: &Path,
    ctx: Option<&egui::Context>,
) -> Result<OpenProjectOutcome, String> {
    // 1. すでに同じファイルパスが別タブ（または現在タブ）で開かれていれば、そのタブへ切り替える
    let existing_tab = app.tabs.tabs.iter().enumerate().find_map(|(idx, slot)| {
        let slot_path = if idx == app.tabs.active_index {
            app.ui.current_file_path.as_deref()
        } else {
            slot.file_path.as_deref()
        }?;
        paths_refer_to_same_file(slot_path, path).then_some(idx)
    });

    if let Some(existing_idx) = existing_tab {
        if existing_idx == app.tabs.active_index {
            return Ok(OpenProjectOutcome::AlreadyActiveTab);
        }
        switch_to_tab(app, existing_idx, ctx);
        return Ok(OpenProjectOutcome::SwitchedToExistingTab);
    }

    // 2. ファイル内容を先に検証・パース（エラー時にタブを増やさない）
    let doc = read_project_document(path)?;

    // 3. 現在のタブが未編集の空の無題タブでなければ、アクティブタブの右隣に新しいタブスロットを追加して切り替える
    // （ファイル読込タブでは `untitled_number` を `0` として遅延採番し、`無題-N` 連番の欠番スキップを防ぐ）
    if !is_active_tab_pristine_empty(app) {
        pack_active_into_slot(app);
        let doc_id = app.tabs.alloc_doc_id();
        let insert_idx = (app.tabs.active_index + 1).min(app.tabs.tabs.len());
        app.tabs
            .tabs
            .insert(insert_idx, DocumentSlot::new_empty(doc_id, 0));
        unpack_slot_into_active(app, insert_idx);
    } else {
        crate::canvas_tool::stop_playback(app);
        crate::canvas_tool::cancel_active_drag(app);
        app.timeline_ui.cancel_drag();
        app.viewport.pan_last_screen_pos = None;
        let new_doc_id = app.tabs.alloc_doc_id();
        if let Some(slot) = app.tabs.tabs.get_mut(app.tabs.active_index) {
            slot.doc_id = new_doc_id;
            slot.untitled_number = 0;
        }
        app.tabs.next_untitled_number = app
            .tabs
            .tabs
            .iter()
            .map(|t| t.untitled_number)
            .max()
            .unwrap_or(0)
            + 1;
    }

    // 4. 読み込んだドキュメントを適用
    doc.apply_to_app(app);
    set_active_file_path(app, Some(path.to_path_buf()));
    app.history.mark_saved();
    if let Some(c) = ctx {
        surrender_focused_widget(c);
        update_window_title(app, c);
    }
    Ok(OpenProjectOutcome::Loaded)
}

/// アクティブタブの右隣に新しい白紙ドキュメントタブを追加して切り替える（`Cmd/Ctrl + N` / タブバーの `＋` ボタン）
pub(crate) fn new_project(app: &mut Is2dApp, ctx: &egui::Context) {
    pack_active_into_slot(app);

    let doc_id = app.tabs.alloc_doc_id();
    let untitled_num = app.tabs.alloc_untitled_number();
    let insert_idx = (app.tabs.active_index + 1).min(app.tabs.tabs.len());
    app.tabs
        .tabs
        .insert(insert_idx, DocumentSlot::new_empty(doc_id, untitled_num));
    unpack_slot_into_active(app, insert_idx);
    app.viewport.reset_view();
    crate::canvas_tool::load_shapes_from_timeline(app);

    surrender_focused_widget(ctx);
    update_window_title(app, ctx);
    app.notify_info(format!("新規タブ「無題-{untitled_num}」を作成しました"));
}

/// OS標準の「開く」ダイアログを表示し、選択された `.is2d` ファイルを新しいタブ（または空の無題タブ）に読み込む
pub(crate) fn open_project_dialog(app: &mut Is2dApp, ctx: &egui::Context) {
    let mut dialog =
        rfd::FileDialog::new().add_filter("is2d プロジェクト (*.is2d)", &[PROJECT_FILE_EXTENSION]);
    if let Some(parent) = app.ui.current_file_path.as_deref().and_then(Path::parent) {
        dialog = dialog.set_directory(parent);
    }

    let Some(path) = dialog.pick_file() else {
        return;
    };

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("ファイル");
    match open_project_in_tab(app, &path, Some(ctx)) {
        Ok(OpenProjectOutcome::Loaded) => {
            app.notify_info(format!("「{name}」を開きました"));
        }
        Ok(OpenProjectOutcome::SwitchedToExistingTab) => {
            app.notify_info(format!("既に開いている「{name}」のタブへ切り替えました"));
        }
        Ok(OpenProjectOutcome::AlreadyActiveTab) => {
            app.notify_info(format!("「{name}」は既に現在のタブで開かれています"));
        }
        Err(err) => {
            app.notify_error(err);
        }
    }
}

/// 上書き保存（`current_file_path` が未設定なら「名前を付けて保存」ダイアログを開く）
pub(crate) fn save_project(app: &mut Is2dApp, ctx: &egui::Context) {
    let Some(path) = app.ui.current_file_path.clone() else {
        save_project_as_dialog(app, ctx);
        return;
    };

    match save_project_to_path(app, &path) {
        Ok(()) => {
            update_window_title(app, ctx);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("ファイル");
            app.notify_info(format!("「{name}」に上書き保存しました"));
        }
        Err(err) => {
            app.notify_error(err);
        }
    }
}

/// OS標準の「名前を付けて保存」ダイアログを表示し、選択されたパスへ `.is2d` ファイルを保存する
pub(crate) fn save_project_as_dialog(app: &mut Is2dApp, ctx: &egui::Context) {
    let default_name = app
        .ui
        .current_file_path
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let num = app
                .tabs
                .tabs
                .get(app.tabs.active_index)
                .map(|s| s.untitled_number.max(1))
                .unwrap_or(1);
            format!("untitled-{num}.is2d")
        });

    let mut dialog = rfd::FileDialog::new()
        .add_filter("is2d プロジェクト (*.is2d)", &[PROJECT_FILE_EXTENSION])
        .set_file_name(&default_name);
    if let Some(parent) = app.ui.current_file_path.as_deref().and_then(Path::parent) {
        dialog = dialog.set_directory(parent);
    }

    let Some(mut path): Option<PathBuf> = dialog.save_file() else {
        return;
    };
    if path.extension().is_none() {
        path.set_extension(PROJECT_FILE_EXTENSION);
    }

    match save_project_to_path(app, &path) {
        Ok(()) => {
            update_window_title(app, ctx);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("ファイル");
            app.notify_info(format!("「{name}」を保存しました"));
        }
        Err(err) => {
            app.notify_error(err);
        }
    }
}
