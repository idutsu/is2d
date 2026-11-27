use std::collections::HashSet;
use std::path::{Path, PathBuf};

use eframe::egui;

use crate::canvas_tool::{cancel_active_drag, SelectSubMode, Tab};
use crate::save_format::*;
use crate::symbol::Document;


/// プロジェクトファイルの標準拡張子
pub(crate) const PROJECT_FILE_EXTENSION: &str = "is2d";


/// 未保存変更があるタブを閉じる際の非ブロッキング確認モーダル状態
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PendingCloseConfirm {
    /// 単一タブのクローズ確認（`close_tab` / `Cmd+W` / タブの `×` ボタン）
    SingleTab { orig_active: usize },
    /// アプリ終了（ウィンドウクローズ）時の未保存タブ順次確認
    ViewportClose {
        orig_active: usize,
        discarded_doc_ids: HashSet<u64>,
    },
}

/// 読み込みで一部を除いた作品 `name` の知らせ（作品を開くときと復元するときの1つ）
pub(crate) fn trimmed_notice_text(name: &str) -> String {
    format!("「{name}」の一部を読み込めなかったため除きました（保存すると元のファイルからも消えます）")
}

/// `open_project_in_tab` の実行結果区分
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenProjectOutcome {
    /// 新規タブ（または未編集の無題タブ）にプロジェクトを読み込んだ
    Loaded,
    /// 一部の上限超過・不正データを除いてプロジェクトを読み込んだ
    LoadedWithTrimmedData,
    /// 既に別タブで同じファイルが開かれていたため、そのタブへ切り替えた
    SwitchedToExistingTab,
    /// 既に現在のアクティブタブで開かれているファイルだった
    AlreadyActiveTab,
}

// ============================================================================
// 0. 複数ドキュメント同時オープン（ドキュメントタブ管理）
// ============================================================================

/// 複数タブ管理とプロジェクトI/O操作に必要なワークスペース機能を抽象化するトレイト
pub(crate) trait ProjectWorkspace: crate::canvas_tool::doc_actions::ActionHost {
    fn tabs(&self) -> &[Tab];
    fn tabs_mut(&mut self) -> &mut Vec<Tab>;
    fn active_index(&self) -> usize;
    fn set_active_index(&mut self, index: usize);
    fn alloc_doc_id(&mut self) -> crate::canvas_tool::FreshTabId;
    fn alloc_untitled_number(&mut self) -> usize;
    fn set_next_untitled_number(&mut self, num: usize);
    fn last_window_title(&self) -> &str;
    fn set_last_window_title(&mut self, title: String);
    fn pending_close_confirm(&self) -> &Option<PendingCloseConfirm>;
    fn pending_close_confirm_mut(&mut self) -> &mut Option<PendingCloseConfirm>;
    fn allow_viewport_close(&self) -> bool;
    fn set_allow_viewport_close(&mut self, allow: bool);
}

/// タブ切り替え・新規作成・ファイル読込・タブ閉じの前に、egui のフォーカス中テキスト入力ウィジェットからフォーカスを解放する
fn surrender_focused_widget(ctx: &egui::Context) {
    ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}

/// タブ切り替えやプロジェクト保存・動画書き出しの前に、選択状態・`Floating`・カスタムピボットを保持したまま
/// 進行中再生・ドラッグの中断と連続編集フラグのリセットを行う
pub(crate) fn prepare_active_tab_for_switch_or_save(ws: &mut impl ProjectWorkspace) {
    ws.begin_operation();
    cancel_active_drag(ws.tab_mut());
    ws.tab_mut().view.pan_last_screen_pos = None;
}

/// アクティブタブのインデックスを `target_index` へ切り替え、ペーストオフセット等をリセットする
fn activate_tab_index(ws: &mut impl ProjectWorkspace, target_index: usize) {
    ws.set_active_index(target_index);
    ws.clipboard_mut().paste_count = 0;
    ws.clipboard_mut().paste_offset = kurbo::Vec2::ZERO;
    if !ws.tab().session.selection().is_empty() && ws.tool().select_sub_mode != SelectSubMode::Reshape
    {
        ws.tool_mut().select_sub_mode = SelectSubMode::Reshape;
    }
}

/// 今のタブに文書 `doc` を入れる（タブに新しい文書を入れる道はみなここを通る：新規作成・開く・まっさらなタブに開く・
/// 最後のタブを閉じる）。文書・履歴・作業の状態・表示を作り直し、画面の土台を作り、貼り付けのずらし・選択の小さなモード・
/// 入力欄の的を初めに戻す（前は道ごとに後片付けが違い、まっさらなタブに開いたときと最後のタブを閉じたときは貼り付けの
/// ずらしが前の作品の続きになった：約束の棚卸し 5616fcc の C の B2・D2）
fn install_document(ws: &mut impl ProjectWorkspace, doc: Document, ctx: Option<&egui::Context>) {
    // 作品を入れたら、タブの番号はいつも新しくする（「どの作品か」の見分けを、入れる所の1か所で替える）。前の番号はもう
    // いないので、クリップボードの対応表から捨てる（`Clipboard::forget_tab`）
    let gone = ws.tab().id();
    ws.clipboard_mut().forget_tab(gone);
    let id = ws.alloc_doc_id();
    ws.tab_mut().install(id, doc);
    ws.clipboard_mut().paste_count = 0;
    ws.clipboard_mut().paste_offset = kurbo::Vec2::ZERO;
    ws.tool_mut().select_sub_mode = SelectSubMode::Reshape;
    if let Some(c) = ctx {
        surrender_focused_widget(c);
    }
}



/// 現在アクティブなタブが「起動直後・新規作成直後で一度も編集されていない白紙の無題タブ」であるかを判定する。文書を
/// 変えるのはみな `apply` で、そのたびに世代 ID を進める（`UndoHistory::push`）ので、編集の有無はその1つの印で決める
/// （既定値を写して推さない：レビュー f01d518 の N-5008）。ファイルが無いこと（保存も読み込みもしていない）と、未保存の
/// 印が無いこと（別のタブの上書きでファイルから外したタブは、中身があっても編集していないので、この印で見分ける）も見る
pub(crate) fn is_active_tab_pristine_empty(tab: &Tab) -> bool {
    tab.file.is_none() && !tab.history().is_dirty() && tab.history().next_commit_id == 1
}

/// 指定したインデックス `index` のタブに未保存変更（`is_dirty() == true`）があるかを返す
pub(crate) fn is_tab_dirty(ws: &impl ProjectWorkspace, index: usize) -> bool {
    ws.tabs()
        .get(index)
        .is_some_and(|tab| tab.history().is_dirty())
}

/// OSウィンドウの閉じる操作（`×` ボタンや `Cmd+Q` / `Alt+F4`）が要求された際、
/// 未保存変更があるタブが存在する場合は `CancelClose` を発行してアプリ内の非ブロッキング確認モーダルを表示する
pub(crate) fn handle_viewport_close_request(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    if !ctx.input(|i| i.viewport().close_requested()) {
        return;
    }
    if ws.allow_viewport_close() {
        return;
    }
    // もうアプリを終える確認をしているなら、その確認を続ける。1つのタブを閉じる確認の最中なら、アプリ全体を終える確認に
    // 切り替える（終了はどのダイアログでも効く：持ち主が決めた 2026-11-27。前はタブを閉じる確認の最中に終了を押しても何も
    // 起きなかった：約束の棚卸し 5616fcc の C の D6）
    let orig_active = match ws.pending_close_confirm() {
        Some(PendingCloseConfirm::ViewportClose { .. }) => {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        Some(PendingCloseConfirm::SingleTab { orig_active }) => *orig_active,
        None => ws.active_index(),
    };
    *ws.pending_close_confirm_mut() = None;
    if (0..ws.tabs().len()).any(|i| is_tab_dirty(ws, i)) {
        advance_viewport_close_confirm(ws, orig_active, HashSet::new(), ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
    }
}

fn advance_viewport_close_confirm(
    ws: &mut impl ProjectWorkspace,
    orig_active: usize,
    discarded_doc_ids: HashSet<u64>,
    ctx: &egui::Context,
) {
    let tab_count = ws.tabs().len();
    let next_idx = (0..tab_count)
        .find(|&i| is_tab_dirty(ws, i) && !discarded_doc_ids.contains(&ws.tabs()[i].id()));
    if let Some(next_idx) = next_idx {
        switch_to_tab(ws, next_idx, Some(ctx));
        *ws.pending_close_confirm_mut() = Some(PendingCloseConfirm::ViewportClose {
            orig_active,
            discarded_doc_ids,
        });
    } else {
        ws.set_allow_viewport_close(true);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// 未保存変更確認モーダルで「保存して閉じる」が選択された際の処理を実行する
pub(crate) fn confirm_pending_close_save(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let Some(pending) = ws.pending_close_confirm_mut().take() else {
        return;
    };
    let tab_index = ws.active_index();
    save_project(ws);
    match pending {
        PendingCloseConfirm::SingleTab { orig_active } => {
            if !is_tab_dirty(ws, tab_index) {
                close_tab_immediately(ws, tab_index, orig_active, Some(ctx));
            } else {
                switch_to_tab(ws, orig_active, Some(ctx));
            }
        }
        PendingCloseConfirm::ViewportClose {
            orig_active,
            discarded_doc_ids,
        } => {
            if !is_tab_dirty(ws, tab_index) {
                advance_viewport_close_confirm(ws, orig_active, discarded_doc_ids, ctx);
            } else {
                switch_to_tab(ws, orig_active, Some(ctx));
            }
        }
    }
}

/// 未保存変更確認モーダルで「保存せずに閉じる」が選択された際の処理を実行する
pub(crate) fn confirm_pending_close_discard(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let Some(pending) = ws.pending_close_confirm_mut().take() else {
        return;
    };
    let tab_index = ws.active_index();
    match pending {
        PendingCloseConfirm::SingleTab { orig_active } => {
            close_tab_immediately(ws, tab_index, orig_active, Some(ctx));
        }
        PendingCloseConfirm::ViewportClose {
            orig_active,
            mut discarded_doc_ids,
        } => {
            if let Some(tab) = ws.tabs().get(tab_index) {
                discarded_doc_ids.insert(tab.id());
            }
            advance_viewport_close_confirm(ws, orig_active, discarded_doc_ids, ctx);
        }
    }
}

/// 未保存変更確認モーダルで「キャンセル」が選択された際の処理を実行する
pub(crate) fn confirm_pending_close_cancel(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let Some(pending) = ws.pending_close_confirm_mut().take() else {
        return;
    };
    let orig_active = match pending {
        PendingCloseConfirm::SingleTab { orig_active }
        | PendingCloseConfirm::ViewportClose { orig_active, .. } => orig_active,
    };
    switch_to_tab(ws, orig_active, Some(ctx));
}

/// 指定したインデックス `target_index` のドキュメントタブへ切り替える
pub(crate) fn switch_to_tab(
    ws: &mut impl ProjectWorkspace,
    target_index: usize,
    ctx: Option<&egui::Context>,
) -> bool {
    if target_index >= ws.tabs().len() || target_index == ws.active_index() {
        return false;
    }
    *ws.pending_close_confirm_mut() = None;
    prepare_active_tab_for_switch_or_save(ws);
    activate_tab_index(ws, target_index);

    if let Some(c) = ctx {
        surrender_focused_widget(c);
    }
    true
}

/// 指定したインデックス `close_index` のドキュメントタブを閉じる（未保存変更があり `ctx` が与えられた場合は非ブロッキング確認モーダルを開く）
pub(crate) fn close_tab(
    ws: &mut impl ProjectWorkspace,
    close_index: usize,
    ctx: Option<&egui::Context>,
) {
    if close_index >= ws.tabs().len() {
        return;
    }
    let orig_active = ws.active_index();
    if let Some(c) = ctx {
        if is_tab_dirty(ws, close_index) {
            switch_to_tab(ws, close_index, Some(c));
            *ws.pending_close_confirm_mut() = Some(PendingCloseConfirm::SingleTab { orig_active });
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
    if close_index >= ws.tabs().len() {
        return;
    }
    *ws.pending_close_confirm_mut() = None;

    if ws.tabs().len() <= 1 {
        // 最後の1タブを閉じた場合は連番をリセットし、白紙の「無題-1」へ初期化する
        prepare_active_tab_for_switch_or_save(ws);
        ws.set_next_untitled_number(1);
        let untitled_num = ws.alloc_untitled_number();

        ws.set_active_index(0);
        ws.tab_mut().untitled_number = untitled_num;
        ws.tab_mut().file = None;
        install_document(ws, Document::default(), ctx);
        return;
    }
    // 閉じるタブの番号はもういないので、クリップボードの対応表から捨てる（最後のタブは、作品を入れる道 `install_document`
    // が捨てる）
    let gone = ws.tabs()[close_index].id();
    ws.clipboard_mut().forget_tab(gone);

    if close_index == ws.active_index() {
        prepare_active_tab_for_switch_or_save(ws);

        ws.tabs_mut().remove(close_index);
        let remaining_len = ws.tabs().len();
        let next_active = if orig_active != close_index {
            if close_index < orig_active {
                orig_active - 1
            } else {
                orig_active.min(remaining_len - 1)
            }
        } else {
            close_index.min(remaining_len - 1)
        };
        activate_tab_index(ws, next_active);
        if let Some(c) = ctx {
            surrender_focused_widget(c);
        }
    } else {
        ws.tabs_mut().remove(close_index);
        if close_index < ws.active_index() {
            ws.set_active_index(ws.active_index() - 1);
        }
    }
}

/// 現在アクティブなドキュメントタブを閉じる（`Cmd/Ctrl + W`）
pub(crate) fn close_active_tab(ws: &mut impl ProjectWorkspace, ctx: Option<&egui::Context>) {
    let active = ws.active_index();
    close_tab(ws, active, ctx);
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
    let tab_title = ws.tab().display_title();
    let full_title = format!("is2d - {tab_title}");
    if ws.last_window_title() != full_title {
        ws.set_last_window_title(full_title.clone());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(full_title));
    }
}

/// 保存・書き出し先のパスがシンボリックリンクである場合は実体パスへ解決して返す
pub(crate) fn resolve_target_path(path: &Path) -> PathBuf {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}


/// 指定されたファイルパス `path` へプロジェクトを一時ファイル経由でアトミック保存する。保存できなければ、知らせ（持ち上げた
/// 絵を置けないときは、取り消しの案内の印つき）
pub(crate) fn save_project_to_path(
    ws: &mut impl ProjectWorkspace,
    path: &Path,
) -> Result<(), crate::canvas_tool::NoticeText> {
    prepare_active_tab_for_switch_or_save(ws);

    // 浮いた片を置いた文書を書く（置けなければ書かない：画面に見えている片が入らないファイルは作らない）
    let doc = ws
        .tab()
        .committed_document_copy()
        .ok_or_else(|| crate::canvas_tool::NoticeText::with_undo_exit("選んだ図形を置けないため保存できません。"))?;
    let target_path = resolve_target_path(path);
    write_project_file(&target_path, &doc).map_err(crate::canvas_tool::NoticeText::from)?;

    // 別タブが同じファイルパスを開いていた場合、重複タブ化を防ぐため別タブ側のファイル紐付けを解除して未保存状態にする
    let active_idx = ws.active_index();
    let mut detached_indices = Vec::new();
    for (idx, tab) in ws.tabs().iter().enumerate() {
        if idx != active_idx
            && tab
                .file
                .as_deref()
                .is_some_and(|sp| paths_refer_to_same_file(sp, path))
        {
            detached_indices.push(idx);
        }
    }
    for idx in detached_indices {
        let alloc_num = if ws.tabs()[idx].untitled_number == 0 {
            Some(ws.alloc_untitled_number())
        } else {
            None
        };
        if let Some(tab) = ws.tabs_mut().get_mut(idx) {
            if let Some(num) = alloc_num {
                tab.untitled_number = num;
            }
            tab.file = None;
            tab.mark_unsaved();
        }
    }

    ws.tab_mut().file = Some(path.to_path_buf());
    ws.tab_mut().mark_saved();
    Ok(())
}

/// 検証済みの `Document` を新しいタブ（または未編集の白紙無題タブ）へ読み込んでアクティブ化する
pub(crate) fn open_document_in_new_tab(
    ws: &mut impl ProjectWorkspace,
    doc: Document,
    ctx: Option<&egui::Context>,
) -> Result<(), String> {
    if doc.symbol_nest_exceeded() {
        return Err("シンボルの入れ子が8段を超えているか、循環しています".to_owned());
    }

    // 現在のタブが未編集の空の無題タブでなければ、アクティブタブの右隣に新しいタブを追加して切り替える
    // （ファイル読込タブでは `untitled_number` を `0` として遅延採番し、`無題-N` 連番の欠番スキップを防ぐ）
    prepare_active_tab_for_switch_or_save(ws);
    if !is_active_tab_pristine_empty(ws.tab()) {
        let doc_id = ws.alloc_doc_id();
        let insert_idx = (ws.active_index() + 1).min(ws.tabs().len());
        ws.tabs_mut().insert(insert_idx, Tab::new_empty(doc_id, 0));
        activate_tab_index(ws, insert_idx);
    } else {
        ws.tab_mut().untitled_number = 0;
        let max_untitled = ws
            .tabs()
            .iter()
            .map(|t| t.untitled_number)
            .max()
            .unwrap_or(0);
        ws.set_next_untitled_number(max_untitled + 1);
    }

    install_document(ws, doc, ctx);
    // テストでは裏で確かめない（まだ確かめていない地図を調べるテストが、裏のスレッドと競わないように。裏で確かめる
    // テストは `check_maps_in_background` を直接呼ぶ）
    #[cfg(not(test))]
    let _ = ws.tab().doc().check_maps_in_background();
    Ok(())
}

/// 指定パスのプロジェクトを「新しいタブ」（または既存の同一ファイルタブ／空の無題タブ）として開く
pub(crate) fn open_project_in_tab(
    ws: &mut impl ProjectWorkspace,
    path: &Path,
    ctx: Option<&egui::Context>,
) -> Result<OpenProjectOutcome, String> {
    // 1. すでに同じファイルパスが別タブ（または現在タブ）で開かれていれば、そのタブへ切り替える
    let existing_tab = ws.tabs().iter().enumerate().find_map(|(idx, tab)| {
        let tab_path = tab.file.as_deref()?;
        paths_refer_to_same_file(tab_path, path).then_some(idx)
    });

    if let Some(existing_idx) = existing_tab {
        if existing_idx == ws.active_index() {
            return Ok(OpenProjectOutcome::AlreadyActiveTab);
        }
        switch_to_tab(ws, existing_idx, ctx);
        return Ok(OpenProjectOutcome::SwitchedToExistingTab);
    }

    // 2. ファイル内容を先に検証・パース（エラー時にタブを増やさない）
    let mut trimmed = false;
    let doc = read_file(path)?.1.into_document_noting(&mut trimmed)?;
    open_document_in_new_tab(ws, doc, ctx)?;

    ws.tab_mut().file = Some(path.to_path_buf());
    if trimmed {
        ws.tab_mut().mark_unsaved();
    } else {
        ws.tab_mut().mark_saved();
    }
    if let Some(c) = ctx {
        surrender_focused_widget(c);
    }
    Ok(if trimmed {
        OpenProjectOutcome::LoadedWithTrimmedData
    } else {
        OpenProjectOutcome::Loaded
    })
}

/// アクティブタブの右隣に新しい白紙ドキュメントタブを追加して切り替える（`Cmd/Ctrl + N` / タブバーの `＋` ボタン）
pub(crate) fn new_project(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    prepare_active_tab_for_switch_or_save(ws);

    let doc_id = ws.alloc_doc_id();
    let untitled_num = ws.alloc_untitled_number();
    let insert_idx = (ws.active_index() + 1).min(ws.tabs().len());
    ws.tabs_mut()
        .insert(insert_idx, Tab::new_empty(doc_id, untitled_num));
    activate_tab_index(ws, insert_idx);
    install_document(ws, Document::default(), Some(ctx));
    ws.notify_info(format!("新規タブ「無題-{untitled_num}」を作成しました"));
}

const PROJECT_FILTER: &str = "is2d プロジェクト (*.is2d)";

fn file_dialog_in_tab_dir(tab: &Tab, filter: &str, ext: &str) -> rfd::FileDialog {
    let dialog = rfd::FileDialog::new().add_filter(filter, &[ext]);
    match tab.file.as_deref().and_then(Path::parent) {
        Some(parent) => dialog.set_directory(parent),
        None => dialog,
    }
}

pub(crate) fn pick_save_path(tab: &Tab, filter: &str, ext: &str, name: &str) -> Option<PathBuf> {
    let mut path = file_dialog_in_tab_dir(tab, filter, ext)
        .set_file_name(name)
        .save_file()?;
    if path.extension().is_none() {
        path.set_extension(ext);
    }
    Some(path)
}

fn save_project_and_notify(ws: &mut impl ProjectWorkspace, path: &Path, action_desc: &str) {
    match save_project_to_path(ws, path) {
        Ok(()) => {
            let name = file_name_or(path, "ファイル");
            ws.notify_info(format!("「{name}」{action_desc}"));
        }
        Err(err) => ws.notify_error(err),
    }
}

/// OS標準の「開く」ダイアログを表示し、選択された `.is2d` ファイルを新しいタブ（または空の無題タブ）に読み込む
pub(crate) fn open_project_dialog(ws: &mut impl ProjectWorkspace, ctx: &egui::Context) {
    let Some(path) =
        file_dialog_in_tab_dir(ws.tab(), PROJECT_FILTER, PROJECT_FILE_EXTENSION).pick_file()
    else {
        return;
    };

    let name = file_name_or(&path, "ファイル");
    match open_project_in_tab(ws, &path, Some(ctx)) {
        Ok(OpenProjectOutcome::Loaded) => {
            ws.notify_info(format!("「{name}」を開きました"));
        }
        Ok(OpenProjectOutcome::LoadedWithTrimmedData) => {
            ws.notify_warning(trimmed_notice_text(name));
        }
        Ok(OpenProjectOutcome::SwitchedToExistingTab) => {
            ws.notify_info(format!("既に開いている「{name}」のタブへ切り替えました"));
        }
        Ok(OpenProjectOutcome::AlreadyActiveTab) => {
            ws.notify_info(format!("「{name}」は既に現在のタブで開かれています"));
        }
        Err(err) => {
            ws.notify_error(err);
        }
    }
}

/// 上書き保存（`file` が未設定なら「名前を付けて保存」ダイアログを開く）
pub(crate) fn save_project(ws: &mut impl ProjectWorkspace) {
    let Some(path) = ws.tab().file.clone() else {
        save_project_as_dialog(ws);
        return;
    };
    save_project_and_notify(ws, &path, "に上書き保存しました");
}

/// OS標準の「名前を付けて保存」ダイアログを表示し、選択されたパスへ `.is2d` ファイルを保存する
pub(crate) fn save_project_as_dialog(ws: &mut impl ProjectWorkspace) {
    let name = format!("{}.{PROJECT_FILE_EXTENSION}", ws.tab().name_stem());

    let Some(path) = pick_save_path(ws.tab(), PROJECT_FILTER, PROJECT_FILE_EXTENSION, &name) else {
        return;
    };
    save_project_and_notify(ws, &path, "を保存しました");
}
