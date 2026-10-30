use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Vec2};
use kurbo::{Affine, Point, QuadBez};
use serde::{Deserialize, Serialize};

use crate::canvas_tool::{
    cancel_active_drag, flush_modified_raw_selection_merge, load_shapes_from_timeline,
    stop_playback, sync_shapes_to_timeline, CanvasState, EditorDoc, SelectSubMode, UndoHistory,
    ViewportState,
};
use crate::clipboard_ops::max_group_id_in_timeline;
use crate::symbol::{
    SymbolDef, SymbolInstance, SymbolInstanceBinding, SymbolLoopMode, SymbolManager, TweenRotate,
};
pub(crate) use crate::timeline::MAX_TIMELINE_FRAMES;
use crate::timeline::{Cel, Keyframe, Layer, Timeline, TimelineCellSelection};
use crate::vector_engine::{Contour, FillShape};

/// `.is2d` プロジェクトファイルの現在のフォーマットバージョン
pub(crate) const PROJECT_FORMAT_VERSION: u32 = 1;

/// プロジェクトファイルの標準拡張子
pub(crate) const PROJECT_FILE_EXTENSION: &str = "is2d";

/// 読み込み可能なプロジェクトファイルの最大バイト数（100MB：異常な巨大ファイルによるOOM防止）
pub(crate) const MAX_PROJECT_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// 1タイムラインあたりの最大許容レイヤー数
pub(crate) const MAX_LAYERS_PER_TIMELINE: usize = 1_000;

/// 1輪郭（Contour）あたりの最大許容ベジェセグメント数
pub(crate) const MAX_CONTOUR_SEGMENTS: usize = 100_000;

/// 読み込み時に許容するエンティティID（`layer_id` / `symbol_id` / `group_id` / `instance_gid`）の最大安全値
/// （`u64::MAX` 付近の細工IDによる `saturating_add` 衝突を防止するため、上限を超えるIDは読み込み時に安全な連番へ再採番する）
pub(crate) const MAX_SAFE_ENTITY_ID: u64 = u64::MAX / 2;

/// 未保存変更があるタブを閉じる際の非ブロッキング確認モーダル状態
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PendingCloseConfirm {
    /// 単一タブのクローズ確認（`close_tab` / `Cmd+W` / タブの `×` ボタン）
    SingleTab {
        tab_index: usize,
        orig_active: usize,
    },
    /// アプリ終了（ウィンドウクローズ）時の未保存タブ順次確認
    ViewportClose {
        current_tab_index: usize,
        orig_active: usize,
        discarded_doc_ids: HashSet<u64>,
    },
}

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

/// ドキュメント状態（`EditorDoc`）と複数タブ管理（`DocumentTabManager`）の組を抽象化するトレイト
pub(crate) trait ProjectWorkspace {
    fn doc(&self) -> &EditorDoc;
    fn doc_mut(&mut self) -> &mut EditorDoc;
    fn tabs(&self) -> &DocumentTabManager;
    fn tabs_mut(&mut self) -> &mut DocumentTabManager;
    fn doc_and_tabs_mut(&mut self) -> (&mut EditorDoc, &mut DocumentTabManager);
}

/// 開いている1つのドキュメントタブが保持する独立状態スロット
pub(crate) struct DocumentSlot {
    /// ドキュメント固有の一意ID（別タブ間コピペのソース・ターゲット識別用）
    pub(crate) doc_id: u64,
    /// 未保存時の「無題-N」番号（1, 2, 3...）
    pub(crate) untitled_number: usize,
    /// 保存先・読み込み元の `.is2d` ファイルパス（未保存なら `None`）
    pub(crate) file_path: Option<PathBuf>,
    /// タブごとの単一ドキュメント状態（アクティブ時はワークスペースの `EditorDoc` とスワップされる）
    pub(crate) doc: EditorDoc,
}

impl std::ops::Deref for DocumentSlot {
    type Target = EditorDoc;
    fn deref(&self) -> &Self::Target {
        &self.doc
    }
}

impl std::ops::DerefMut for DocumentSlot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.doc
    }
}

impl DocumentSlot {
    pub(crate) fn new_empty(doc_id: u64, untitled_number: usize) -> Self {
        Self {
            doc_id,
            untitled_number,
            file_path: None,
            doc: EditorDoc::new_with_id(doc_id),
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
            self.doc.history.is_dirty()
        };
        if is_dirty {
            format!("{base} *")
        } else {
            base
        }
    }
}

/// 複数のドキュメントタブを管理する構造体
pub(crate) struct DocumentTabManager {
    pub(crate) tabs: Vec<DocumentSlot>,
    pub(crate) active_index: usize,
    pub(crate) next_untitled_number: usize,
    pub(crate) next_doc_id: u64,
    /// 直近にOSウィンドウへ送信したタイトル文字列（毎フレームの重複送信防止用キャッシュ）
    pub(crate) last_window_title: String,
    /// 未保存変更確認モーダル（egui ウィンドウ）の表示待機状態
    pub(crate) pending_close_confirm: Option<PendingCloseConfirm>,
    /// 全タブの未保存確認が完了し、OSウィンドウのクローズを許可するフラグ
    pub(crate) allow_viewport_close: bool,
}

impl Default for DocumentTabManager {
    fn default() -> Self {
        Self {
            tabs: vec![DocumentSlot::new_empty(1, 1)],
            active_index: 0,
            next_untitled_number: 2,
            next_doc_id: 2,
            last_window_title: String::new(),
            pending_close_confirm: None,
            allow_viewport_close: false,
        }
    }
}

impl DocumentTabManager {
    /// 現在アクティブなタブのメタデータ（`file_path`）を `doc.ui.current_file_path` と同期する
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

/// アクティブタブの `file_path` を `doc.ui.current_file_path` と `tabs.tabs[active_index].file_path` の双方に一括設定する
pub(crate) fn set_active_file_path(ws: &mut impl ProjectWorkspace, path: Option<PathBuf>) {
    let (doc, tabs) = ws.doc_and_tabs_mut();
    doc.ui.current_file_path = path.clone();
    tabs.sync_active_file_path(path);
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
fn prepare_active_tab_for_switch_or_save(doc: &mut EditorDoc, flush_raw_merge: bool) {
    stop_playback(doc);
    cancel_active_drag(doc);
    doc.viewport.pan_last_screen_pos = None;
    doc.viewport.editing_stage_size = false;
    doc.symbols.actively_renaming_symbol = None;
    doc.ui.editing_symbol_playback = false;

    if flush_raw_merge && doc.canvas.selection_modified && !doc.canvas.selected_shapes.is_empty() {
        let selected_ids = doc.canvas.selected_shape_ids();
        let saved_pivot = doc.canvas.transform_pivot;
        flush_modified_raw_selection_merge(doc);
        doc.canvas.restore_selection_by_shape_ids(&selected_ids);
        doc.canvas.transform_pivot = if doc.canvas.selected_shapes.is_empty() {
            None
        } else {
            saved_pivot
        };
    }

    // 矩形マーキースライス直後で未編集の一時分割状態（`has_unsliced_snapshot() == true`）の間は、
    // タイムライン側に既にスライス前の正しい図形が保持されているため上書き同期をスキップし、選択も維持する
    if !doc.canvas.has_unsliced_snapshot() {
        sync_shapes_to_timeline(doc);
    }
    doc.symbols.sync_active_edit(&doc.timeline);

    if doc.symbols.edit_stack.len() >= 2 {
        for idx in (1..doc.symbols.edit_stack.len()).rev() {
            let sym_id = doc.symbols.edit_stack[idx - 1].symbol_id;
            let xform = doc.symbols.edit_stack[idx - 1].instance_xform;
            if xform.determinant().abs() <= 1e-9 {
                continue;
            }
            let inv_xform = xform.inverse();
            let working_tl = doc.symbols.edit_stack[idx].parent_timeline.clone();
            let mut cache =
                std::mem::take(&mut doc.symbols.edit_stack[idx - 1].stage_to_local_cache);
            if let Some(sym_def) = doc.symbols.find_symbol_mut(sym_id) {
                sym_def.update_from_working_timeline(&working_tl, inv_xform, &mut cache);
            }
            doc.symbols.edit_stack[idx - 1].stage_to_local_cache = cache;
        }
    }
}

/// ウィンドウ共通の状態（タブ間共有クリップボード・ツール/ブラシ設定・トースト・キャンバス矩形）を保持したまま
/// アクティブ `EditorDoc` とタブスロットの `EditorDoc` を一括 `std::mem::swap` する
fn swap_doc_preserving_shared_state(active: &mut EditorDoc, slot_doc: &mut EditorDoc) {
    let shared_clipboard = active.canvas.take_shared_clipboard();
    let last_canvas_rect = active.viewport.last_canvas_rect;
    let tool = active.ui.tool;
    let select_sub_mode = active.ui.select_sub_mode;
    let right_panel_tab = active.ui.right_panel_tab;
    let brush_radius = active.ui.brush_radius;
    let eraser_radius = active.ui.eraser_radius;
    let brush_color = active.ui.brush_color;
    let status_toast = active.ui.status_toast.take();

    std::mem::swap(active, slot_doc);

    active.canvas.restore_shared_clipboard(shared_clipboard);
    active.viewport.last_canvas_rect = last_canvas_rect;
    active.ui.tool = tool;
    active.ui.select_sub_mode = select_sub_mode;
    active.ui.right_panel_tab = right_panel_tab;
    active.ui.brush_radius = brush_radius;
    active.ui.eraser_radius = eraser_radius;
    active.ui.brush_color = brush_color;
    active.ui.status_toast = status_toast;
}

/// 現在のアクティブドキュメント状態を `tabs.tabs[active_index]` スロットへ退避する
fn pack_active_into_slot(ws: &mut impl ProjectWorkspace) {
    let (doc, tabs) = ws.doc_and_tabs_mut();
    prepare_active_tab_for_switch_or_save(doc, false);
    let idx = tabs.active_index;
    if let Some(slot) = tabs.tabs.get_mut(idx) {
        slot.file_path = doc.ui.current_file_path.clone();
        swap_doc_preserving_shared_state(doc, &mut slot.doc);
    }
}

/// `tabs.tabs[target_index]` スロットの状態をアクティブ `EditorDoc` へ展開する
fn unpack_slot_into_active(ws: &mut impl ProjectWorkspace, target_index: usize) {
    let (doc, tabs) = ws.doc_and_tabs_mut();
    tabs.active_index = target_index;
    if let Some(slot) = tabs.tabs.get_mut(target_index) {
        swap_doc_preserving_shared_state(doc, &mut slot.doc);
        doc.doc_id = slot.doc_id;
        doc.ui.current_file_path = slot.file_path.clone();
    }
    doc.canvas.paste_count = 0;
    doc.canvas.paste_offset = kurbo::Vec2::ZERO;
    doc.timeline_switched = true;
    if !doc.canvas.selected_shapes.is_empty() && doc.ui.select_sub_mode != SelectSubMode::Reshape {
        doc.ui.select_sub_mode = SelectSubMode::Reshape;
    }
}

/// アクティブドキュメントのタイムライン・シンボル・履歴・キャンバス・ビューポート・UIフラグを一括初期化し、
/// タブ間共有のクリップボードと `last_canvas_rect` を引き継いだ上で現在フレームの図形を展開する共通ヘルパー
fn reset_active_document_state(
    doc: &mut EditorDoc,
    timeline: Timeline,
    symbols: SymbolManager,
    stage_size: Vec2,
) {
    let shared_clipboard = doc.canvas.take_shared_clipboard();
    let last_canvas_rect = doc.viewport.last_canvas_rect;

    doc.timeline = timeline;
    doc.symbols = symbols;
    doc.history = UndoHistory::default();
    doc.canvas = CanvasState::default();
    doc.canvas.restore_shared_clipboard(shared_clipboard);
    doc.viewport = ViewportState {
        stage_size,
        last_canvas_rect,
        ..Default::default()
    };
    doc.viewport.reset_view();
    doc.ui.inspect_timeline_frame = false;
    doc.ui.show_wireframe = false;
    doc.ui.editing_symbol_playback = false;
    doc.ui.select_sub_mode = SelectSubMode::Reshape;
    doc.is_playing = false;
    doc.timeline_switched = true;
    load_shapes_from_timeline(doc);
}

/// 現在アクティブなタブが「起動直後・新規作成直後で一度も編集されていない白紙の無題タブ」であるかを判定する
pub(crate) fn is_active_tab_pristine_empty(doc: &EditorDoc) -> bool {
    doc.ui.current_file_path.is_none()
        && !doc.history.is_dirty()
        && doc.history.undo_stack.is_empty()
        && doc.history.redo_stack.is_empty()
        && doc.symbols.library.is_empty()
        && doc.symbols.edit_stack.is_empty()
        && doc.viewport.stage_size == Vec2::new(800.0, 450.0)
        && !doc.viewport.clip_outside_stage
        && doc.timeline.fps == 24
        && doc.timeline.total_frames == 1
        && doc.timeline.layers.len() == 1
        && doc.timeline.layers[0].name == "Layer 1"
        && doc.timeline.layers[0].visible
        && !doc.timeline.layers[0].locked
        && doc.timeline.layers[0].frame_count == 1
        && doc.timeline.layers[0].keyframes.len() == 1
        && doc.timeline.layers[0].keyframes[0].cel.is_empty()
        && doc.canvas.shapes.is_empty()
}

/// 指定したインデックス `index` のタブに未保存変更（`is_dirty() == true`）があるかを返す
pub(crate) fn is_tab_dirty(ws: &impl ProjectWorkspace, index: usize) -> bool {
    if index == ws.tabs().active_index {
        ws.doc().history.is_dirty()
    } else {
        ws.tabs()
            .tabs
            .get(index)
            .is_some_and(|slot| slot.doc.history.is_dirty())
    }
}

/// OSウィンドウの閉じる操作（`×` ボタンや `Cmd+Q` / `Alt+F4`）が要求された際、
/// 未保存変更があるタブが存在する場合は `CancelClose` を発行してアプリ内の非ブロッキング確認モーダルを表示する
pub(crate) fn handle_viewport_close_request(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    if !ctx.input(|i| i.viewport().close_requested()) {
        return;
    }
    if ws.tabs().allow_viewport_close {
        return;
    }
    if ws.tabs().pending_close_confirm.is_some() {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        return;
    }
    let tab_count = ws.tabs().tabs.len();
    if let Some(idx) = (0..tab_count).find(|&i| is_tab_dirty(ws, i)) {
        let orig_active = ws.tabs().active_index;
        if idx != orig_active {
            switch_to_tab(ws, idx, Some(ctx));
        }
        ws.tabs_mut().pending_close_confirm = Some(PendingCloseConfirm::ViewportClose {
            current_tab_index: idx,
            orig_active,
            discarded_doc_ids: HashSet::new(),
        });
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
    }
}

fn advance_viewport_close_confirm(
    ws: &mut impl ProjectWorkspace,
    orig_active: usize,
    discarded_doc_ids: HashSet<u64>,
    ctx: &egui::Context,
) {
    let tab_count = ws.tabs().tabs.len();
    let next_idx = (0..tab_count).find(|&i| {
        is_tab_dirty(ws, i) && !discarded_doc_ids.contains(&ws.tabs().tabs[i].doc_id)
    });
    if let Some(next_idx) = next_idx {
        if next_idx != ws.tabs().active_index {
            switch_to_tab(ws, next_idx, Some(ctx));
        }
        ws.tabs_mut().pending_close_confirm = Some(PendingCloseConfirm::ViewportClose {
            current_tab_index: next_idx,
            orig_active,
            discarded_doc_ids,
        });
    } else {
        ws.tabs_mut().allow_viewport_close = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// 未保存変更確認モーダルで「保存して閉じる」が選択された際の処理を実行する
pub(crate) fn confirm_pending_close_save(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let Some(pending) = ws.tabs_mut().pending_close_confirm.take() else {
        return;
    };
    match pending {
        PendingCloseConfirm::SingleTab {
            tab_index,
            orig_active,
        } => {
            save_project(ws, ctx);
            if !is_tab_dirty(ws, tab_index) {
                close_tab_immediately(ws, tab_index, orig_active, Some(ctx));
            } else if tab_index != orig_active && orig_active < ws.tabs().tabs.len() {
                switch_to_tab(ws, orig_active, Some(ctx));
            }
        }
        PendingCloseConfirm::ViewportClose {
            current_tab_index,
            orig_active,
            discarded_doc_ids,
        } => {
            save_project(ws, ctx);
            if !is_tab_dirty(ws, current_tab_index) {
                advance_viewport_close_confirm(ws, orig_active, discarded_doc_ids, ctx);
            } else if ws.tabs().active_index != orig_active && orig_active < ws.tabs().tabs.len() {
                switch_to_tab(ws, orig_active, Some(ctx));
            }
        }
    }
}

/// 未保存変更確認モーダルで「保存せずに破棄」が選択された際の処理を実行する
pub(crate) fn confirm_pending_close_discard(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let Some(pending) = ws.tabs_mut().pending_close_confirm.take() else {
        return;
    };
    match pending {
        PendingCloseConfirm::SingleTab {
            tab_index,
            orig_active,
        } => {
            close_tab_immediately(ws, tab_index, orig_active, Some(ctx));
        }
        PendingCloseConfirm::ViewportClose {
            current_tab_index,
            orig_active,
            mut discarded_doc_ids,
        } => {
            if let Some(slot) = ws.tabs().tabs.get(current_tab_index) {
                discarded_doc_ids.insert(slot.doc_id);
            }
            advance_viewport_close_confirm(ws, orig_active, discarded_doc_ids, ctx);
        }
    }
}

/// 未保存変更確認モーダルで「キャンセル」が選択された際の処理を実行する
pub(crate) fn confirm_pending_close_cancel(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let Some(pending) = ws.tabs_mut().pending_close_confirm.take() else {
        return;
    };
    let orig_active = match pending {
        PendingCloseConfirm::SingleTab { orig_active, .. }
        | PendingCloseConfirm::ViewportClose { orig_active, .. } => orig_active,
    };
    if ws.tabs().active_index != orig_active && orig_active < ws.tabs().tabs.len() {
        switch_to_tab(ws, orig_active, Some(ctx));
    }
}

/// 指定したインデックス `target_index` のドキュメントタブへ切り替える
pub(crate) fn switch_to_tab(
    ws: &mut impl ProjectWorkspace,
    target_index: usize,
    ctx: Option<&egui::Context>,
) -> bool {
    if target_index >= ws.tabs().tabs.len() || target_index == ws.tabs().active_index {
        return false;
    }
    ws.tabs_mut().pending_close_confirm = None;
    pack_active_into_slot(ws);
    unpack_slot_into_active(ws, target_index);

    if let Some(c) = ctx {
        surrender_focused_widget(c);
        update_window_title(ws, c);
    }
    true
}

/// 指定したインデックス `close_index` のドキュメントタブを閉じる（未保存変更があり `ctx` が与えられた場合は非ブロッキング確認モーダルを開く）
pub(crate) fn close_tab(
    ws: &mut impl ProjectWorkspace,
    close_index: usize,
    ctx: Option<&egui::Context>,
) {
    if close_index >= ws.tabs().tabs.len() {
        return;
    }
    let orig_active = ws.tabs().active_index;
    if let Some(c) = ctx {
        if is_tab_dirty(ws, close_index) {
            if close_index != orig_active {
                switch_to_tab(ws, close_index, Some(c));
            }
            ws.tabs_mut().pending_close_confirm = Some(PendingCloseConfirm::SingleTab {
                tab_index: close_index,
                orig_active,
            });
            return;
        }
    }
    close_tab_immediately(ws, close_index, orig_active, ctx);
}

fn close_tab_immediately(
    ws: &mut impl ProjectWorkspace,
    close_index: usize,
    orig_active: usize,
    ctx: Option<&egui::Context>,
) {
    if close_index >= ws.tabs().tabs.len() {
        return;
    }
    ws.tabs_mut().pending_close_confirm = None;

    if ws.tabs().tabs.len() <= 1 {
        // 最後の1タブを閉じた場合は連番をリセットし、白紙の「無題-1」へ初期化する
        stop_playback(ws.doc_mut());
        cancel_active_drag(ws.doc_mut());
        ws.tabs_mut().next_untitled_number = 1;
        let doc_id = ws.tabs_mut().alloc_doc_id();
        let untitled_num = ws.tabs_mut().alloc_untitled_number();

        ws.tabs_mut().tabs[0] = DocumentSlot::new_empty(doc_id, untitled_num);
        ws.tabs_mut().active_index = 0;
        ws.doc_mut().doc_id = doc_id;
        set_active_file_path(ws, None);
        reset_active_document_state(
            ws.doc_mut(),
            Timeline::default(),
            SymbolManager::default(),
            ViewportState::default().stage_size,
        );
        if let Some(c) = ctx {
            surrender_focused_widget(c);
            update_window_title(ws, c);
        }
        return;
    }

    if close_index == ws.tabs().active_index {
        stop_playback(ws.doc_mut());
        cancel_active_drag(ws.doc_mut());
        ws.doc_mut().viewport.pan_last_screen_pos = None;

        ws.tabs_mut().tabs.remove(close_index);
        let remaining_len = ws.tabs().tabs.len();
        let next_active = if orig_active != close_index {
            if close_index < orig_active {
                orig_active - 1
            } else {
                orig_active.min(remaining_len - 1)
            }
        } else {
            close_index.min(remaining_len - 1)
        };
        unpack_slot_into_active(ws, next_active);
        if let Some(c) = ctx {
            surrender_focused_widget(c);
            update_window_title(ws, c);
        }
    } else {
        ws.tabs_mut().tabs.remove(close_index);
        if close_index < ws.tabs().active_index {
            ws.tabs_mut().active_index -= 1;
        }
        if let Some(c) = ctx {
            update_window_title(ws, c);
        }
    }
}

/// 現在アクティブなドキュメントタブを閉じる（`Cmd/Ctrl + W`）
pub(crate) fn close_active_tab(ws: &mut impl ProjectWorkspace, ctx: Option<&egui::Context>) {
    let active = ws.tabs().active_index;
    close_tab(ws, active, ctx);
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
            SavedTweenRotate::Auto => TweenRotate::Auto,
            SavedTweenRotate::Clockwise => TweenRotate::Clockwise,
            SavedTweenRotate::CounterClockwise => TweenRotate::CounterClockwise,
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
            SavedSymbolLoopMode::Loop => SymbolLoopMode::Loop,
            SavedSymbolLoopMode::PlayOnce => SymbolLoopMode::PlayOnce,
            SavedSymbolLoopMode::SingleFrame => SymbolLoopMode::SingleFrame,
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

        // 重複または安全上限（MAX_SAFE_ENTITY_ID）超過の Layer::id があれば一意な安全IDへ再採番し、各キーフレーム内の図形・インスタンスの layer_id も正規化する
        let mut seen_layer_ids = HashSet::with_capacity(layers.len());
        let mut max_layer_id = layers
            .iter()
            .map(|l| l.id)
            .filter(|&id| id > 0 && id <= MAX_SAFE_ENTITY_ID)
            .max()
            .unwrap_or(0);
        for layer in &mut layers {
            if layer.id == 0 || layer.id > MAX_SAFE_ENTITY_ID || !seen_layer_ids.insert(layer.id) {
                loop {
                    max_layer_id = max_layer_id.saturating_add(1).max(1);
                    if seen_layer_ids.insert(max_layer_id) {
                        layer.id = max_layer_id;
                        break;
                    }
                }
            } else {
                max_layer_id = max_layer_id.max(layer.id);
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
        let raw_next_layer_id = if s.next_layer_id <= MAX_SAFE_ENTITY_ID {
            s.next_layer_id
        } else {
            2
        };
        let mut tl = Self {
            layers,
            current_frame,
            active_layer,
            total_frames,
            fps: s.fps.clamp(1, 60),
            next_layer_number: s.next_layer_number.max(2),
            next_layer_id: raw_next_layer_id.max(max_layer_id.saturating_add(1)).max(2),
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
    /// 現在の `EditorDoc` から保存用の `ProjectDocument` を生成する
    pub(crate) fn from_app(doc: &EditorDoc) -> Self {
        let root_timeline = doc
            .symbols
            .edit_stack
            .first()
            .map(|sess| &sess.parent_timeline)
            .unwrap_or(&doc.timeline);
        let mut saved_timeline: SavedTimeline = root_timeline.into();
        saved_timeline.fps = doc.timeline.fps.clamp(1, 60);

        Self {
            version: PROJECT_FORMAT_VERSION,
            stage_size: [doc.viewport.stage_size.x, doc.viewport.stage_size.y],
            timeline: saved_timeline,
            library: doc
                .symbols
                .library
                .iter()
                .map(SavedSymbolDef::from)
                .collect(),
            next_symbol_id: doc.symbols.next_symbol_id,
            next_group_id: doc.symbols.next_group_id,
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
    pub(crate) fn apply_to_app(self, doc: &mut EditorDoc) {
        let mut timeline: Timeline = self.timeline.into();

        // 重複または不正（id == 0 / id > MAX_SAFE_ENTITY_ID）な SymbolDef::id を一意な安全IDへ再採番してライブラリを構築
        let mut seen_sym_ids = HashSet::with_capacity(self.library.len());
        let mut next_fallback_sym_id = self
            .library
            .iter()
            .map(|s| s.id)
            .filter(|&id| id > 0 && id <= MAX_SAFE_ENTITY_ID)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        let mut sym_id_remap: HashMap<u64, u64> = HashMap::new();
        let mut library: Vec<SymbolDef> = Vec::with_capacity(self.library.len());
        for saved_sym in self.library {
            let mut sym = SymbolDef::from(saved_sym);
            let orig_id = sym.id;
            if sym.id == 0 || sym.id > MAX_SAFE_ENTITY_ID || !seen_sym_ids.insert(sym.id) {
                while seen_sym_ids.contains(&next_fallback_sym_id) {
                    next_fallback_sym_id = next_fallback_sym_id.saturating_add(1);
                }
                sym.id = next_fallback_sym_id;
                next_fallback_sym_id = next_fallback_sym_id.saturating_add(1);
                seen_sym_ids.insert(sym.id);
                if orig_id > 0 && !seen_sym_ids.contains(&orig_id) {
                    sym_id_remap.entry(orig_id).or_insert(sym.id);
                }
            }
            library.push(sym);
        }

        // 安全上限を超える group_id / instance_gid の上限を把握し、必要時に再採番するためのカウンタを準備
        let mut max_valid_gid = 0_u64;
        for tl in std::iter::once(&timeline).chain(library.iter().map(|s| &s.timeline)) {
            for layer in &tl.layers {
                for kf in &layer.keyframes {
                    for s in kf.cel.shapes.iter() {
                        for &gid in &s.group_path {
                            if gid > 0 && gid <= MAX_SAFE_ENTITY_ID {
                                max_valid_gid = max_valid_gid.max(gid);
                            }
                        }
                    }
                    for inst in kf.cel.instances.iter() {
                        for &gid in &inst.parent_scope {
                            if gid > 0 && gid <= MAX_SAFE_ENTITY_ID {
                                max_valid_gid = max_valid_gid.max(gid);
                            }
                        }
                        if inst.binding.instance_gid > 0
                            && inst.binding.instance_gid <= MAX_SAFE_ENTITY_ID
                        {
                            max_valid_gid = max_valid_gid.max(inst.binding.instance_gid);
                        }
                    }
                }
            }
        }
        let mut next_safe_gid = max_valid_gid.saturating_add(1).max(1);
        let mut unsafe_gid_remap: HashMap<u64, u64> = HashMap::new();

        // 存在しない symbol_id（孤児インスタンス）をルートタイムラインおよび各シンボルタイムラインから除去し、
        // 再採番された symbol_id / group_id / instance_gid の反映と tween フラグの正規化を行う
        for tl in std::iter::once(&mut timeline).chain(library.iter_mut().map(|s| &mut s.timeline))
        {
            for layer in &mut tl.layers {
                for kf in &mut layer.keyframes {
                    let has_unsafe_shape_gid = kf.cel.shapes.iter().any(|s| {
                        s.group_path
                            .iter()
                            .any(|&gid| gid == 0 || gid > MAX_SAFE_ENTITY_ID)
                    });
                    if has_unsafe_shape_gid {
                        for s in kf.cel.shapes_mut() {
                            for gid in &mut s.group_path {
                                if *gid == 0 || *gid > MAX_SAFE_ENTITY_ID {
                                    *gid = *unsafe_gid_remap.entry(*gid).or_insert_with(|| {
                                        let id = next_safe_gid;
                                        next_safe_gid = next_safe_gid.saturating_add(1);
                                        id
                                    });
                                }
                            }
                        }
                    }
                    let needs_inst_update = kf.cel.instances.iter().any(|inst| {
                        sym_id_remap.contains_key(&inst.binding.symbol_id)
                            || !seen_sym_ids.contains(&inst.binding.symbol_id)
                            || inst.binding.instance_gid == 0
                            || inst.binding.instance_gid > MAX_SAFE_ENTITY_ID
                            || inst
                                .parent_scope
                                .iter()
                                .any(|&gid| gid == 0 || gid > MAX_SAFE_ENTITY_ID)
                    });
                    if needs_inst_update {
                        let mut seen_kf_gids = HashSet::new();
                        let mut updated_instances = Vec::with_capacity(kf.cel.instances.len());
                        for mut inst in (*kf.cel.instances).clone() {
                            if let Some(&mapped_sym_id) = sym_id_remap.get(&inst.binding.symbol_id)
                            {
                                inst.binding.symbol_id = mapped_sym_id;
                            }
                            if !seen_sym_ids.contains(&inst.binding.symbol_id) {
                                continue;
                            }
                            for gid in &mut inst.parent_scope {
                                if *gid == 0 || *gid > MAX_SAFE_ENTITY_ID {
                                    *gid = *unsafe_gid_remap.entry(*gid).or_insert_with(|| {
                                        let id = next_safe_gid;
                                        next_safe_gid = next_safe_gid.saturating_add(1);
                                        id
                                    });
                                }
                            }
                            if inst.binding.instance_gid == 0
                                || inst.binding.instance_gid > MAX_SAFE_ENTITY_ID
                            {
                                inst.binding.instance_gid = *unsafe_gid_remap
                                    .entry(inst.binding.instance_gid)
                                    .or_insert_with(|| {
                                        let id = next_safe_gid;
                                        next_safe_gid = next_safe_gid.saturating_add(1);
                                        id
                                    });
                            }
                            if !seen_kf_gids.insert(inst.binding.instance_gid) {
                                inst.binding.instance_gid = next_safe_gid;
                                next_safe_gid = next_safe_gid.saturating_add(1);
                                seen_kf_gids.insert(inst.binding.instance_gid);
                            }
                            updated_instances.push(inst);
                        }
                        *kf.cel.instances_mut() = updated_instances;
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

        let raw_next_sym_id = if self.next_symbol_id <= MAX_SAFE_ENTITY_ID {
            self.next_symbol_id
        } else {
            1
        };
        let raw_next_gid = if self.next_group_id <= MAX_SAFE_ENTITY_ID {
            self.next_group_id
        } else {
            1
        };
        let selected_library_symbol = library.first().map(|s| s.id);
        let mut symbols = SymbolManager {
            library,
            next_symbol_id: raw_next_sym_id.max(max_sym_id.saturating_add(1)).max(1),
            selected_library_symbol,
            actively_renaming_symbol: None,
            edit_stack: Vec::new(),
            next_group_id: raw_next_gid.max(max_gid.saturating_add(1)).max(1),
            revision: crate::vector_engine::next_mesh_revision(),
        };

        // 外部改変された JSON にシンボルの自己・相互循環参照が含まれていた場合は、
        // 3色DFS（0 = 未訪問, 1 = 現在の探索経路上, 2 = 探索完了・DAG保証済み）により O(V + E) の単一パスで後退辺（サイクル辺）を除去する
        let n_syms = symbols.library.len();
        if n_syms > 0 {
            let id_to_idx: HashMap<u64, usize> = symbols
                .library
                .iter()
                .enumerate()
                .map(|(i, s)| (s.id, i))
                .collect();
            let mut color = vec![0_u8; n_syms];
            let mut dfs_stack: Vec<(usize, Vec<usize>, usize)> = Vec::new();
            for start_idx in 0..n_syms {
                if color[start_idx] != 0 {
                    continue;
                }
                color[start_idx] = 1;
                let children = collect_and_strip_cycle_back_edges(
                    &mut symbols.library[start_idx],
                    &id_to_idx,
                    &color,
                );
                dfs_stack.push((start_idx, children, 0));
                while let Some((u_idx, children, next_i)) = dfs_stack.last_mut() {
                    if *next_i < children.len() {
                        let v_idx = children[*next_i];
                        *next_i += 1;
                        if color[v_idx] == 0 {
                            color[v_idx] = 1;
                            let v_children = collect_and_strip_cycle_back_edges(
                                &mut symbols.library[v_idx],
                                &id_to_idx,
                                &color,
                            );
                            dfs_stack.push((v_idx, v_children, 0));
                        }
                    } else {
                        let u = *u_idx;
                        color[u] = 2;
                        dfs_stack.pop();
                    }
                }
            }
        }

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
        reset_active_document_state(doc, timeline, symbols, Vec2::new(w, h));
    }
}

/// シンボル定義 `sym` 内の全キーフレームを走査し、現在DFSスタック上の祖先（`color[v_idx] == 1`）へ向かう後退辺（サイクル辺）を除去しつつ、
/// 未訪問（`color[v_idx] == 0`）の子シンボルインデックス一覧を重複なしで返す
fn collect_and_strip_cycle_back_edges(
    sym: &mut SymbolDef,
    id_to_idx: &HashMap<u64, usize>,
    color: &[u8],
) -> Vec<usize> {
    let mut unvisited_children = Vec::new();
    let mut seen_children = HashSet::new();
    for layer in &mut sym.timeline.layers {
        for kf in &mut layer.keyframes {
            let has_back_edge = kf.cel.instances.iter().any(|inst| {
                id_to_idx
                    .get(&inst.binding.symbol_id)
                    .is_none_or(|&v_idx| color[v_idx] == 1)
            });
            if has_back_edge {
                kf.cel.instances_mut().retain(|inst| {
                    id_to_idx
                        .get(&inst.binding.symbol_id)
                        .is_some_and(|&v_idx| color[v_idx] != 1)
                });
                kf.tween &= kf.cel.single_symbol_instance().is_some();
            }
            for inst in kf.cel.instances.iter() {
                if let Some(&v_idx) = id_to_idx.get(&inst.binding.symbol_id) {
                    if color[v_idx] == 0 && seen_children.insert(v_idx) {
                        unvisited_children.push(v_idx);
                    }
                }
            }
        }
    }
    unvisited_children
}

// ============================================================================
// 2. ファイル読み書きコア関数 & OSファイルダイアログ連携
// ============================================================================

/// 2つのパスが同一のファイルを指しているか（文字列表現の一致・`canonicalize`・Unix ファイルID `dev/ino`）を判定する
fn paths_refer_to_same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            if ma.dev() == mb.dev() && ma.ino() == mb.ino() {
                return true;
            }
        }
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// 現在のアクティブタブ（ファイル名・無題番号・未保存 `*` 状態）に応じてウィンドウタイトルバーの表示を更新する
pub(crate) fn update_window_title(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let active_idx = ws.tabs().active_index;
    let current_file_path = ws.doc().ui.current_file_path.clone();
    let is_dirty = ws.doc().history.is_dirty();
    let tab_title = ws
        .tabs()
        .tabs
        .get(active_idx)
        .map(|slot| slot.display_title(current_file_path.as_deref(), is_dirty, true))
        .unwrap_or_else(|| "無題-1".to_owned());
    let full_title = format!("is2d - {tab_title}");
    if ws.tabs().last_window_title != full_title {
        ws.tabs_mut().last_window_title = full_title.clone();
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

fn restore_pre_save_state(doc: &mut EditorDoc, pre_save_state: Option<PreSaveRollbackState>) {
    if let Some((shapes, sel, pivot, tl, syms)) = pre_save_state {
        doc.canvas.shapes = shapes;
        doc.canvas.selected_shapes = sel;
        doc.canvas.selection_modified = true;
        doc.canvas.transform_pivot = pivot;
        doc.timeline = tl;
        doc.symbols = syms;
        doc.canvas.record_loaded_signatures();
    }
}

/// 指定されたファイルパス `path` へプロジェクトを一時ファイル経由でアトミック保存する
pub(crate) fn save_project_to_path(
    ws: &mut impl ProjectWorkspace,
    path: &Path,
) -> Result<(), String> {
    let pre_save_state = {
        let doc = ws.doc();
        (doc.canvas.selection_modified && !doc.canvas.selected_shapes.is_empty()).then(|| {
            (
                doc.canvas.shapes.clone(),
                doc.canvas.selected_shapes.clone(),
                doc.canvas.transform_pivot,
                doc.timeline.clone(),
                doc.symbols.clone(),
            )
        })
    };
    prepare_active_tab_for_switch_or_save(ws.doc_mut(), true);

    let proj_doc = ProjectDocument::from_app(ws.doc());
    if let Err(err) = proj_doc.validate_finite() {
        restore_pre_save_state(ws.doc_mut(), pre_save_state);
        return Err(err);
    }
    let json = match serde_json::to_string(&proj_doc) {
        Ok(s) => s,
        Err(e) => {
            restore_pre_save_state(ws.doc_mut(), pre_save_state);
            return Err(format!("プロジェクトデータの変換に失敗しました: {e}"));
        }
    };
    if json.len() as u64 > MAX_PROJECT_FILE_BYTES {
        restore_pre_save_state(ws.doc_mut(), pre_save_state);
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
        restore_pre_save_state(ws.doc_mut(), pre_save_state);
        return Err(format!("一時ファイルの書き込みに失敗しました: {e}"));
    }

    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        restore_pre_save_state(ws.doc_mut(), pre_save_state);
        return Err(format!("ファイルの保存に失敗しました: {e}"));
    }
    if let Some(parent) = path.parent() {
        if let Ok(dir_file) = std::fs::File::open(parent) {
            let _ = dir_file.sync_all();
        }
    }

    // 別タブが同じファイルパスを開いていた場合、重複タブ化を防ぐため別タブ側のファイル紐付けを解除して未保存状態にする
    let active_idx = ws.tabs().active_index;
    let mut detached_indices = Vec::new();
    for (idx, slot) in ws.tabs().tabs.iter().enumerate() {
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
        let alloc_num = if ws.tabs().tabs[idx].untitled_number == 0 {
            Some(ws.tabs_mut().alloc_untitled_number())
        } else {
            None
        };
        if let Some(slot) = ws.tabs_mut().tabs.get_mut(idx) {
            if let Some(num) = alloc_num {
                slot.untitled_number = num;
            }
            slot.file_path = None;
            slot.doc.ui.current_file_path = None;
            slot.doc.history.mark_unsaved();
        }
    }

    set_active_file_path(ws, Some(path.to_path_buf()));
    ws.doc_mut().history.mark_saved();
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
    ws: &mut impl ProjectWorkspace,
    path: &Path,
    ctx: Option<&egui::Context>,
) -> Result<OpenProjectOutcome, String> {
    // 1. すでに同じファイルパスが別タブ（または現在タブ）で開かれていれば、そのタブへ切り替える
    let active_idx = ws.tabs().active_index;
    let current_file_path = ws.doc().ui.current_file_path.clone();
    let existing_tab = ws.tabs().tabs.iter().enumerate().find_map(|(idx, slot)| {
        let slot_path = if idx == active_idx {
            current_file_path.as_deref()
        } else {
            slot.file_path.as_deref()
        }?;
        paths_refer_to_same_file(slot_path, path).then_some(idx)
    });

    if let Some(existing_idx) = existing_tab {
        if existing_idx == ws.tabs().active_index {
            return Ok(OpenProjectOutcome::AlreadyActiveTab);
        }
        switch_to_tab(ws, existing_idx, ctx);
        return Ok(OpenProjectOutcome::SwitchedToExistingTab);
    }

    // 2. ファイル内容を先に検証・パース（エラー時にタブを増やさない）
    let proj_doc = read_project_document(path)?;

    // 3. 現在のタブが未編集の空の無題タブでなければ、アクティブタブの右隣に新しいタブスロットを追加して切り替える
    // （ファイル読込タブでは `untitled_number` を `0` として遅延採番し、`無題-N` 連番の欠番スキップを防ぐ）
    if !is_active_tab_pristine_empty(ws.doc()) {
        pack_active_into_slot(ws);
        let doc_id = ws.tabs_mut().alloc_doc_id();
        let insert_idx = (ws.tabs().active_index + 1).min(ws.tabs().tabs.len());
        ws.tabs_mut()
            .tabs
            .insert(insert_idx, DocumentSlot::new_empty(doc_id, 0));
        unpack_slot_into_active(ws, insert_idx);
    } else {
        stop_playback(ws.doc_mut());
        cancel_active_drag(ws.doc_mut());
        ws.doc_mut().viewport.pan_last_screen_pos = None;
        let new_doc_id = ws.tabs_mut().alloc_doc_id();
        let active_idx = ws.tabs().active_index;
        if let Some(slot) = ws.tabs_mut().tabs.get_mut(active_idx) {
            slot.doc_id = new_doc_id;
            slot.doc.doc_id = new_doc_id;
            slot.untitled_number = 0;
        }
        ws.doc_mut().doc_id = new_doc_id;
        let max_untitled = ws
            .tabs()
            .tabs
            .iter()
            .map(|t| t.untitled_number)
            .max()
            .unwrap_or(0);
        ws.tabs_mut().next_untitled_number = max_untitled + 1;
    }

    // 4. 読み込んだドキュメントを適用
    proj_doc.apply_to_app(ws.doc_mut());
    set_active_file_path(ws, Some(path.to_path_buf()));
    ws.doc_mut().history.mark_saved();
    if let Some(c) = ctx {
        surrender_focused_widget(c);
        update_window_title(ws, c);
    }
    Ok(OpenProjectOutcome::Loaded)
}

/// アクティブタブの右隣に新しい白紙ドキュメントタブを追加して切り替える（`Cmd/Ctrl + N` / タブバーの `＋` ボタン）
pub(crate) fn new_project(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    pack_active_into_slot(ws);

    let doc_id = ws.tabs_mut().alloc_doc_id();
    let untitled_num = ws.tabs_mut().alloc_untitled_number();
    let insert_idx = (ws.tabs().active_index + 1).min(ws.tabs().tabs.len());
    ws.tabs_mut()
        .tabs
        .insert(insert_idx, DocumentSlot::new_empty(doc_id, untitled_num));
    unpack_slot_into_active(ws, insert_idx);
    ws.doc_mut().viewport.reset_view();
    load_shapes_from_timeline(ws.doc_mut());

    surrender_focused_widget(ctx);
    update_window_title(ws, ctx);
    ws.doc_mut()
        .notify_info(format!("新規タブ「無題-{untitled_num}」を作成しました"));
}

/// OS標準の「開く」ダイアログを表示し、選択された `.is2d` ファイルを新しいタブ（または空の無題タブ）に読み込む
pub(crate) fn open_project_dialog(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let mut dialog =
        rfd::FileDialog::new().add_filter("is2d プロジェクト (*.is2d)", &[PROJECT_FILE_EXTENSION]);
    if let Some(parent) = ws.doc().ui.current_file_path.as_deref().and_then(Path::parent) {
        dialog = dialog.set_directory(parent);
    }

    let Some(path) = dialog.pick_file() else {
        return;
    };

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("ファイル");
    match open_project_in_tab(ws, &path, Some(ctx)) {
        Ok(OpenProjectOutcome::Loaded) => {
            ws.doc_mut().notify_info(format!("「{name}」を開きました"));
        }
        Ok(OpenProjectOutcome::SwitchedToExistingTab) => {
            ws.doc_mut()
                .notify_info(format!("既に開いている「{name}」のタブへ切り替えました"));
        }
        Ok(OpenProjectOutcome::AlreadyActiveTab) => {
            ws.doc_mut()
                .notify_info(format!("「{name}」は既に現在のタブで開かれています"));
        }
        Err(err) => {
            ws.doc_mut().notify_error(err);
        }
    }
}

/// 上書き保存（`current_file_path` が未設定なら「名前を付けて保存」ダイアログを開く）
pub(crate) fn save_project(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let Some(path) = ws.doc().ui.current_file_path.clone() else {
        save_project_as_dialog(ws, ctx);
        return;
    };

    match save_project_to_path(ws, &path) {
        Ok(()) => {
            update_window_title(ws, ctx);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("ファイル");
            ws.doc_mut()
                .notify_info(format!("「{name}」に上書き保存しました"));
        }
        Err(err) => {
            ws.doc_mut().notify_error(err);
        }
    }
}

/// OS標準の「名前を付けて保存」ダイアログを表示し、選択されたパスへ `.is2d` ファイルを保存する
pub(crate) fn save_project_as_dialog(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let default_name = ws
        .doc()
        .ui
        .current_file_path
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let num = ws
                .tabs()
                .tabs
                .get(ws.tabs().active_index)
                .map(|s| s.untitled_number.max(1))
                .unwrap_or(1);
            format!("untitled-{num}.is2d")
        });

    let mut dialog = rfd::FileDialog::new()
        .add_filter("is2d プロジェクト (*.is2d)", &[PROJECT_FILE_EXTENSION])
        .set_file_name(&default_name);
    if let Some(parent) = ws.doc().ui.current_file_path.as_deref().and_then(Path::parent) {
        dialog = dialog.set_directory(parent);
    }

    let Some(mut path): Option<PathBuf> = dialog.save_file() else {
        return;
    };
    if path.extension().is_none() {
        path.set_extension(PROJECT_FILE_EXTENSION);
    }

    match save_project_to_path(ws, &path) {
        Ok(()) => {
            update_window_title(ws, ctx);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("ファイル");
            ws.doc_mut()
                .notify_info(format!("「{name}」を保存しました"));
        }
        Err(err) => {
            ws.doc_mut().notify_error(err);
        }
    }
}
