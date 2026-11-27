use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Vec2};
use kurbo::{Affine, Point};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::canvas_tool::{cancel_active_drag, SelectSubMode, Tab};
use crate::stored_map::{ReadTiles, TileReader, WriteTiles};
use crate::symbol::{
    ColorEffect, Document, SymbolDef, SymbolInstance, SymbolInstanceBinding, SymbolLoopMode,
    TweenRotate,
};
pub(crate) use crate::timeline::MAX_LAYERS_PER_TIMELINE;
use crate::timeline::{
    Cel, Keyframe, KeyframeList, Layer, SavedScopeMap, ScopeMap, Timeline, TimelineCellSelection,
};

/// `.is2d` プロジェクトファイルの現在のフォーマットバージョン。頭（印・版・中身の長さ・中身の CRC32）のあとに、保存用の型
/// （`ProjectDocument`）を postcard で書いた中身を置く（`write_project`。版 6 から）。版 7 は、地図をセル画にしまう
/// 形のまま書く：文書に1つの塊の表（升目ごとの、境目の線を1本で持つ辺の並び。辺は側の図形の番号も持つ）と、階層ごとの
/// 地図の塊の番号の並び。キーフレームどうしで共有する塊は1回だけ書き、読むときも共有したまま作る（ファイルの大きさが
/// 変わった塊の量に見合い、開いた後のメモリが見る順に左右されない：レビュー f01d518 の R4 の (a)）。インスタンスの重なり
/// 位置は階層の地図の並びで書く（`order`）。面の図形は文書に持たず、使うときに地図から作る。それより前の版は開かない。
/// 設計：claude/保存の形をバイナリにする設計_版6_7b5f6c4.md、claude/ゼロベースレビューf01d518の直し_まとまり3の説明.md。
/// 版 9 は、辺に線（色の表の番号と太さ）を足した（設計：claude/線の設計_68a596e.md）。版 10 は、線に番号を足した（線の番号を
/// 辺にしまう。設計：claude/線の番号を辺にしまう設計_81b715e.md）。版 11 は、レイヤーの番号の数え札を作品に1つにした（タイムラインごとの
/// 数え札を外す。設計：claude/審査b7208d9の直しの設計.md の 7.）
pub(crate) const PROJECT_FORMAT_VERSION: u32 = 11;

/// `.is2d` ファイルの先頭の印
const PROJECT_FILE_MAGIC: [u8; 4] = *b"IS2D";

/// 復旧ファイル（`.is2dj`：`recovery_journal`）の先頭の印
const JOURNAL_FILE_MAGIC: [u8; 4] = *b"IS2J";

/// 復旧ファイルの記録の版（`recovery_journal::Record` の形を変えたら上げる）。復旧ファイルは作品ファイルと同じ保存の型を
/// 入れるので、頭には保存の形の版 `PROJECT_FORMAT_VERSION` も一緒に書く（どちらかが違えば読まない）。版 1 は頭の印が
/// `IS2DJRN1` で版を持たなかった。版 2 は、音のデータと音（名前）を別の記録にした（設計：claude/審査6fb4faaの直しの設計.md）。版 3 は、目録にレイヤーの番号の
/// 数え札を足した（保存の形の版 11 と同じ時）
pub(crate) const JOURNAL_RECORD_VERSION: u32 = 3;

/// 保存するファイルの種類（頭の初めの種類の印と版で見分ける：作るのは `file_identity`、確かめるのは `read_identity` だけ）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SaveKind {
    /// 作品ファイル（`.is2d`）：印・保存の形の版
    Project,
    /// 復旧ファイル（`.is2dj`）：印・保存の形の版・復旧の記録の版
    Journal,
}

impl SaveKind {
    /// 頭の初め（種類の印と版）の長さ
    pub(crate) const fn identity_len(self) -> usize {
        match self {
            Self::Project => 8,
            Self::Journal => 12,
        }
    }
}

/// いちばん長い頭の初めの長さ（種類を見分けるのに読む長さ）
pub(crate) const IDENTITY_LEN: usize = SaveKind::Journal.identity_len();

/// 種類 `kind` のファイルの頭の初め（種類の印と版）。作るのはここだけ（作品ファイルと復旧ファイルの両方）
pub(crate) fn file_identity(kind: SaveKind) -> Vec<u8> {
    let mut head = Vec::with_capacity(kind.identity_len());
    match kind {
        SaveKind::Project => head.extend_from_slice(&PROJECT_FILE_MAGIC),
        SaveKind::Journal => head.extend_from_slice(&JOURNAL_FILE_MAGIC),
    }
    head.extend_from_slice(&PROJECT_FORMAT_VERSION.to_le_bytes());
    if kind == SaveKind::Journal {
        head.extend_from_slice(&JOURNAL_RECORD_VERSION.to_le_bytes());
    }
    head
}

/// ファイルの頭 `head`（初めから読めた分。短くてよい）の種類と版を確かめる（確かめるのはここだけ）。種類の印が無ければ、
/// 前の形式（JSON）か is2d でないと断り、版が今のアプリと違えば断る
pub(crate) fn read_identity(head: &[u8]) -> Result<SaveKind, String> {
    let not_is2d = || {
        if head.first() == Some(&b'{') {
            "前の形式（JSON）のファイルは開けません".to_owned()
        } else {
            "is2d のファイルではありません".to_owned()
        }
    };
    let kind = match head.get(..4) {
        Some(m) if m == PROJECT_FILE_MAGIC => SaveKind::Project,
        Some(m) if m == JOURNAL_FILE_MAGIC => SaveKind::Journal,
        _ => return Err(not_is2d()),
    };
    if head.len() < kind.identity_len() {
        return Err(not_is2d());
    }
    let u32_at = |i: usize| u32::from_le_bytes(head[i..i + 4].try_into().expect("4 バイト"));
    let version = u32_at(4);
    if version != PROJECT_FORMAT_VERSION {
        return Err(format!("未対応のプロジェクトバージョンです (version: {version})"));
    }
    if kind == SaveKind::Journal && u32_at(8) != JOURNAL_RECORD_VERSION {
        return Err(format!("未対応の復旧ファイルの版です (version: {})", u32_at(8)));
    }
    Ok(kind)
}

/// ファイル `path` の頭の初めを読んで、種類と版を確かめる（`read_identity`）
pub(crate) fn file_kind(path: &Path) -> Result<SaveKind, String> {
    use std::io::Read;
    let mut head = Vec::with_capacity(IDENTITY_LEN);
    std::fs::File::open(path)
        .and_then(|f| f.take(IDENTITY_LEN as u64).read_to_end(&mut head))
        .map_err(read_failed)?;
    read_identity(&head)
}

/// `target` に書くときの一時ファイル（同じフォルダの `.{名前}.{プロセス番号}.tmp`。名前の付け方はここだけ：作品の保存・
/// 復旧ファイル・動画の書き出し）
pub(crate) fn tmp_path_for(target: &Path) -> PathBuf {
    target.with_file_name(format!(".{}.{}.tmp", file_name_or(target, "file"), std::process::id()))
}

/// 書き終えた一時ファイル `tmp` を `target` に置き換える（置き換えるのはここだけ）：一時ファイルを確定し、名前を替え、
/// フォルダを確定する（電源が落ちても、置き換える前か後のどちらかが残る）。失敗したら一時ファイルを消す
pub(crate) fn replace_atomically(tmp: &Path, target: &Path) -> std::io::Result<()> {
    // 確定は書ける開き方で開いて行う（Windows は、読むだけの開き方ではファイルを確定できない）
    let res = std::fs::OpenOptions::new()
        .write(true)
        .open(tmp)
        .and_then(|f| f.sync_all())
        .and_then(|()| std::fs::rename(tmp, target));
    if res.is_err() {
        let _ = std::fs::remove_file(tmp);
        return res;
    }
    if let Some(parent) = target.parent() {
        if let Ok(dir_file) = std::fs::File::open(parent) {
            let _ = dir_file.sync_all();
        }
    }
    Ok(())
}

/// `target` を、一時ファイル（`tmp_path_for`）に `write` で書いてから置き換える（`replace_atomically`）。書くのに失敗
/// したら一時ファイルを消す
pub(crate) fn write_atomically(
    target: &Path,
    write: impl FnOnce(std::fs::File) -> std::io::Result<std::fs::File>,
) -> std::io::Result<()> {
    let tmp = tmp_path_for(target);
    if let Err(e) = std::fs::File::create(&tmp).and_then(write) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    replace_atomically(&tmp, target)
}

/// `.is2d` ファイルの頭の大きさ：印（4）・版（u32）・中身の長さ（u64）・中身の CRC32（u32）。数はリトルエンディアン
const PROJECT_FILE_HEADER_LEN: usize = 20;

/// プロジェクトファイルの標準拡張子
pub(crate) const PROJECT_FILE_EXTENSION: &str = "is2d";

/// 読み込み時に許容するエンティティID（`layer_id` / `symbol_id` / `group_id` / `instance_gid`）の最大安全値
/// （`u64::MAX` 付近の細工IDによる `saturating_add` 衝突を防止するため、上限を超えるIDは読み込み時に安全な連番へ再採番する）
pub(crate) const MAX_SAFE_ENTITY_ID: u64 = u64::MAX / 2;

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
pub(crate) trait ProjectWorkspace: crate::doc_actions::ActionHost {
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
// 1. 保存用シリアライズデータ構造（DTO）
// ============================================================================

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSymbolInstanceBinding {
    pub(crate) instance_gid: u64,
    pub(crate) symbol_id: u64,
    pub(crate) xform: [f64; 6],
    #[serde(default)]
    pub(crate) pivot: [f64; 2],
    #[serde(default)]
    pub(crate) loop_mode: SymbolLoopMode,
    #[serde(default)]
    pub(crate) first_frame: usize,
    #[serde(default)]
    pub(crate) color_effect: ColorEffect,
}

impl From<SymbolInstanceBinding> for SavedSymbolInstanceBinding {
    fn from(b: SymbolInstanceBinding) -> Self {
        Self {
            instance_gid: b.instance_gid,
            symbol_id: b.symbol_id,
            xform: b.xform.as_coeffs(),
            pivot: [b.pivot.x, b.pivot.y],
            loop_mode: b.loop_mode,
            first_frame: b.first_frame,
            color_effect: b.color_effect,
        }
    }
}

impl From<SavedSymbolInstanceBinding> for SymbolInstanceBinding {
    fn from(s: SavedSymbolInstanceBinding) -> Self {
        // 書いてある数をそのまま読む。潰れた変換も読む（そのインスタンスは描かない：`instantiate_shapes`）。数が有限かは
        // 読み込みの初めに、書く入口と同じ判定で確かめる（`only_finite_numbers`）。色効果を描くとき（`ColorEffect::apply`）は
        // 色の範囲に収めるので、範囲の外の色効果も描ける
        let xform = Affine::new(s.xform);
        Self {
            instance_gid: s.instance_gid,
            symbol_id: s.symbol_id,
            xform,
            pivot: Point::new(s.pivot[0], s.pivot[1]),
            loop_mode: s.loop_mode,
            first_frame: s.first_frame,
            color_effect: s.color_effect,
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
    /// 重なり位置：セル画の階層の地図の並びの何番目の手前か
    #[serde(default)]
    pub(crate) order: usize,
}

impl From<&SymbolInstance> for SavedSymbolInstance {
    fn from(inst: &SymbolInstance) -> Self {
        Self {
            binding: inst.binding.into(),
            layer_id: inst.layer_id,
            parent_scope: inst.parent_scope.clone(),
            order: inst.order,
        }
    }
}

impl From<SavedSymbolInstance> for SymbolInstance {
    fn from(s: SavedSymbolInstance) -> Self {
        Self {
            binding: s.binding.into(),
            layer_id: s.layer_id,
            parent_scope: s.parent_scope,
            order: s.order,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedKeyframe {
    pub(crate) start_frame: usize,
    #[serde(default)]
    pub(crate) tween: bool,
    #[serde(default)]
    pub(crate) tween_rotate: TweenRotate,
    /// 階層ごとの地図（図形の並びで、その階層が最初に出てくる順）
    #[serde(default)]
    pub(crate) maps: Vec<SavedScopeMap>,
    #[serde(default)]
    pub(crate) instances: Vec<SavedSymbolInstance>,
    /// 付けた音の区間（版 8 から）
    #[serde(default)]
    pub(crate) sound: Option<SavedSoundClip>,
}

/// キーフレームの音の区間の保存の形（`SoundClip`：音の番号と、音の中の始まり・終わり。`SOUND_RATE` の単位）
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSoundClip {
    pub(crate) sound: u64,
    pub(crate) start: u64,
    pub(crate) end: u64,
}

/// 音のライブラリの1つの音の保存の形：番号・名前・読み込んだ WAV・MP3 のバイトのまま（書くときは文書の音のバイトを
/// 共有して書き、写さない）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSound {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) bytes: Arc<[u8]>,
}

impl From<&crate::sound::Sound> for SavedSound {
    fn from(s: &crate::sound::Sound) -> Self {
        Self {
            id: s.id,
            name: s.name.clone(),
            bytes: s.encoded.clone(),
        }
    }
}

impl SavedKeyframe {
    /// 始まりのコマが `start_frame` のキーフレーム `kf` の保存用の写し
    pub(crate) fn new(start_frame: usize, kf: &Keyframe) -> Self {
        Self {
            start_frame,
            tween: kf.tween,
            tween_rotate: kf.tween_rotate,
            maps: kf.cel.maps().iter().map(ScopeMap::saved_form).collect(),
            instances: kf
                .cel
                .instances()
                .iter()
                .map(SavedSymbolInstance::from)
                .collect(),
            sound: kf.sound.map(|c| SavedSoundClip {
                sound: c.sound,
                start: c.start,
                end: c.end,
            }),
        }
    }
}

impl Keyframe {
    /// 読んだキーフレーム `s`。地図は塊の表から作ったしまう形（`into_document`）で、正しいかは確かめずに、初めて使うときに
    /// 確かめる（`Cel::from_saved`）。しまう形が無いか、同じ階層の地図が2つあれば、壊れたファイルとして `Err`
    /// 同じ番号のインスタンスを外したら `trimmed` に記す（読み込みで中身を除いたかは、除いた所で記す）。
    /// 書いてある始まりのコマと組にして返す
    fn from_saved(s: SavedKeyframe, trimmed: &mut bool) -> Result<(usize, Self), String> {
        let broken = || "プロジェクトファイルの地図が壊れています".to_owned();
        if s.maps.iter().any(|m| m.stored.is_none()) {
            return Err(broken());
        }
        let mut seen_gids = HashSet::with_capacity(s.instances.len());
        let saved_instances = s.instances.len();
        let instances: Vec<SymbolInstance> = s
            .instances
            .into_iter()
            .map(SymbolInstance::from)
            // 同じキーフレームに同じ番号のインスタンスは1つだけ。使えない番号（`is_safe_id`）は、文書を作るときに
            // 新しい番号にし、無いシンボルを指すインスタンスはそこで外す
            .filter(|inst| seen_gids.insert(inst.binding.instance_gid))
            .collect();
        *trimmed |= instances.len() < saved_instances;
        // セル画の作りの決まり（書く入口と同じ1つ：`check_cel_form`）。同じ番号のインスタンスは上で外してある
        crate::timeline::check_cel_form(
            s.maps.iter().map(|m| m.group_path.as_slice()),
            instances.iter().map(|i| (i.binding.instance_gid, i.parent_scope.as_slice())),
        )
        .map_err(|_| broken())?;
        let cel = Cel::from_saved(s.maps, instances);
        // 音の付いたキーフレームは音だけ（セル画が空でなければ壊れたファイル）。音の区間が正しいかは、音のライブラリを
        // 読んでから確かめる（`into_document_noting`）
        if s.sound.is_some() && !cel.is_empty() {
            return Err(broken());
        }
        let sound = s.sound.map(|c| crate::sound::SoundClip {
            sound: c.sound,
            start: c.start,
            end: c.end,
        });
        Ok((
            s.start_frame,
            Self::with_cel(cel, s.tween, s.tween_rotate).with_sound(sound),
        ))
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
            keyframes: layer
                .keyframes_with_start()
                .map(|(start, kf)| SavedKeyframe::new(start, kf))
                .collect(),
            ..Self::header(layer)
        }
    }
}

impl SavedLayer {
    /// レイヤー `layer` の、キーフレームを除いた保存の形（保存の形と、復旧の書き足すファイル `recovery_journal` の両方が
    /// これを使う）
    pub(crate) fn header(layer: &Layer) -> Self {
        Self {
            id: layer.id,
            name: layer.name.clone(),
            visible: layer.visible,
            locked: layer.locked,
            frame_count: layer.frame_count,
            keyframes: Vec::new(),
        }
    }
}

impl Layer {
    /// 読んだレイヤー `s`。レイヤーの長さの外のキーフレームと、同じコマの2つ目からのキーフレームは外し、外したら
    /// `trimmed` に記す（0 コマ目にキーフレームが無ければ空のキーフレームを足す。これは何も除かない）
    fn from_saved(s: SavedLayer, trimmed: &mut bool) -> Result<Self, String> {
        // コマ数に上限は設けない（編集で作れる長さはみな読み戻す。読み込みはキーフレームの数だけ回る：レビュー f01d518
        // の N-2004）
        let frame_count = s.frame_count.max(1);
        let saved_keyframes = s.keyframes.len();
        let mut keyframes: Vec<(usize, Arc<Keyframe>)> = Vec::with_capacity(s.keyframes.len());
        for saved in s.keyframes {
            let (start, kf) = Keyframe::from_saved(saved, trimmed)?;
            if start < frame_count {
                keyframes.push((start, Arc::new(kf)));
            }
        }
        keyframes.sort_by_key(|&(start, _)| start);
        keyframes.dedup_by_key(|&mut (start, _)| start);
        *trimmed |= keyframes.len() < saved_keyframes;
        if keyframes.first().is_none_or(|&(start, _)| start != 0) {
            keyframes.insert(0, (0, Arc::new(Keyframe::new_blank())));
        }
        Ok(Self {
            // 使えない番号（`is_safe_id`）は、タイムラインを作るときに新しい番号にする
            id: s.id,
            name: s.name,
            visible: s.visible,
            locked: s.locked,
            frame_count,
            keyframes: KeyframeList::from_keyed(keyframes),
        })
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
    pub(crate) layers: Vec<SavedLayer>,
}

impl From<&Timeline> for SavedTimeline {
    fn from(tl: &Timeline) -> Self {
        Self {
            layers: tl.layers.iter().map(SavedLayer::from).collect(),
            ..Self::header(tl)
        }
    }
}

impl SavedTimeline {
    /// タイムライン `tl` の、レイヤーを除いた保存の形（fps は 24。文書の fps は作品の一番上で入れる：`saved_fps`。保存の
    /// 形と、復旧の書き足すファイル `recovery_journal` の両方がこれを使う）
    pub(crate) fn header(tl: &Timeline) -> Self {
        Self {
            fps: 24,
            total_frames: tl.total_frames,
            current_frame: tl.current_frame,
            active_layer: tl.active_layer,
            next_layer_number: tl.next_layer_number,
            layers: Vec::new(),
        }
    }
}

impl Timeline {
    /// 読んだタイムライン `s`。レイヤーが上限（`MAX_LAYERS_PER_TIMELINE`）より多ければ後ろを外し、外したら `trimmed` に
    /// 記す（レイヤー・キーフレーム・インスタンスで外したものも、そこで記す）
    fn from_saved(s: SavedTimeline, trimmed: &mut bool) -> Result<Self, String> {
        *trimmed |= s.layers.len() > MAX_LAYERS_PER_TIMELINE;
        let mut layers: Vec<Layer> = s
            .layers
            .into_iter()
            .take(MAX_LAYERS_PER_TIMELINE)
            .map(|l| Layer::from_saved(l, trimmed))
            .collect::<Result<_, _>>()?;
        if layers.is_empty() {
            layers.push(Layer::new_with_id(1, "Layer 1"));
        }

        // 使えない・重なるレイヤーの番号は新しい番号にし（`FreshIds`）、各キーフレームのインスタンスの layer_id もそろえる
        let mut seen_layer_ids = HashSet::with_capacity(layers.len());
        let mut layer_ids = FreshIds::after(layers.iter().map(|l| l.id));
        for layer in &mut layers {
            if !is_safe_id(layer.id) || !seen_layer_ids.insert(layer.id) {
                layer.id = layer_ids.take();
            }
            let lid = layer.id;
            for i in 0..layer.keyframes.len() {
                let kf = layer.keyframe_mut(i);
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
            .max(1);
        let active_layer = s.active_layer.min(layers.len().saturating_sub(1));
        let current_frame = s.current_frame.min(total_frames.saturating_sub(1));
        let mut tl = Self {
            layers,
            current_frame,
            active_layer,
            total_frames,
            next_layer_number: s.next_layer_number.max(2),
            cell_selection: Some(TimelineCellSelection::single(active_layer, current_frame)),
        };
        tl.sync_total_frames();
        Ok(tl)
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

impl SymbolDef {
    /// 読んだシンボル `s`（タイムラインで外したものは `trimmed` に記す）
    fn from_saved(s: SavedSymbolDef, trimmed: &mut bool) -> Result<Self, String> {
        Ok(SymbolDef::with_timeline(
            s.id,
            s.name,
            Timeline::from_saved(s.timeline, trimmed)?,
        ))
    }
}

/// `.is2d` ファイルの中身のルート構造体（版の番号はファイルの頭にだけ持つ：`project_file_head`）。postcard は項目を
/// 並びの位置で読むので、書かない項目（`skip_serializing_if`）は作らない。塊の表は、書くときは文書の塊から流す形
/// （`WriteTiles`）、アプリが読むときは読みながら塊にした形（`ReadTiles`）。テストは、中身を見たり細工したりするために
/// 塊の表を保存の形のまま読み書きする（読むのはどれも同じ読み込みの道 `read_project` を通る）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProjectDocument<T = WriteTiles> {
    pub(crate) stage_size: [f32; 2],
    pub(crate) timeline: SavedTimeline,
    #[serde(default)]
    pub(crate) library: Vec<SavedSymbolDef>,
    #[serde(default)]
    pub(crate) next_symbol_id: u64,
    #[serde(default)]
    pub(crate) next_group_id: u64,
    #[serde(default)]
    pub(crate) recovery_source: Option<PathBuf>,
    /// 地図の塊の表（キーフレームの地図は、この表の塊の番号の並び）
    #[serde(default)]
    pub(crate) map_tiles: T,
    /// 音のライブラリ（版 8 から）
    #[serde(default)]
    pub(crate) sounds: Vec<SavedSound>,
    #[serde(default)]
    pub(crate) next_sound_id: u64,
    /// 次に足すレイヤーの番号（版 11 から：作品の数え札。前はタイムラインごとに持った）
    #[serde(default)]
    pub(crate) next_layer_id: u64,
}

/// 番号 `id`（レイヤー・シンボル・グループ・インスタンス）が、ファイルから読んでそのまま使える番号か（0 でなく、安全な
/// 上限 `MAX_SAFE_ENTITY_ID` 以下）。読み込みで番号を確かめる決まりはこれ1つ
fn is_safe_id(id: u64) -> bool {
    id > 0 && id <= MAX_SAFE_ENTITY_ID
}

/// 読み込みで新しく振る番号（レイヤー・シンボル・グループ・音。番号を新しくする決まりはこれ1つ）：ファイルにある使える
/// 番号（`is_safe_id`）のいちばん大きいものの次から順に振るので、ファイルにある使える番号とも、振った番号どうしとも
/// 重ならない
struct FreshIds {
    next: u64,
}

impl FreshIds {
    /// ファイルにある番号 `ids` のあとから振る
    fn after(ids: impl Iterator<Item = u64>) -> Self {
        let max = ids.filter(|&id| is_safe_id(id)).max().unwrap_or(0);
        Self { next: max + 1 }
    }

    /// 新しい番号を1つ取る
    fn take(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// 文書に入れる「次に振る番号」：ファイルに書いてある次の番号 `saved`（使えない番号なら無いものとする）を、ファイル
    /// にある番号と振った番号のどれよりも後にしたもの
    fn next_to_save(&self, saved: u64) -> u64 {
        if is_safe_id(saved) {
            saved.max(self.next)
        } else {
            self.next
        }
    }
}

impl ProjectDocument {
    /// 現在の `Tab` から保存用の `ProjectDocument` を生成する
    /// （`Floating` がある場合は `Document` のコピーに対して確定した内容を書き出し、`Session` も `Document` も書き換えない）。
    /// 浮いた片を置けなければ `Err`（画面に見えている片が入らないファイルは書かない）
    pub(crate) fn from_app(tab: &Tab) -> Result<Self, String> {
        let doc = tab.committed_document_copy().ok_or_else(|| {
            format!("選んだ図形を置けないため保存できません。{}", crate::canvas_tool::undo_exit_text())
        })?;
        Ok(Self::of(&doc))
    }

    /// 文書 `doc` の保存用の写し
    pub(crate) fn of(doc: &Document) -> Self {
        let mut saved_timeline: SavedTimeline = (&doc.root).into();
        saved_timeline.fps = saved_fps(doc);

        let mut out = Self {
            stage_size: [doc.stage_size.x, doc.stage_size.y],
            timeline: saved_timeline,
            library: doc
                .library()
                .iter_in_order()
                .map(|s| SavedSymbolDef::from(&**s))
                .collect(),
            next_symbol_id: doc.next_symbol_id,
            next_group_id: doc.next_group_id,
            recovery_source: None,
            map_tiles: WriteTiles::default(),
            sounds: doc.sounds.iter().map(|s| SavedSound::from(&**s)).collect(),
            next_sound_id: doc.next_sound_id,
            next_layer_id: doc.next_layer_id,
        };
        out.write_map_tiles();
        out
    }

    /// 地図のしまう形（`SavedScopeMap::stored`）を、塊の表（`map_tiles`）の塊の番号の並びにする。共有している塊は1回だけ
    /// 表に足す（塊は写さない：書くときに文書の塊から流す）
    pub(crate) fn write_map_tiles(&mut self) {
        let mut writer = std::mem::take(&mut self.map_tiles);
        self.for_each_map(|m| m.write_tiles(&mut writer));
        self.map_tiles = writer;
    }
}

/// 文書 `doc` の保存する fps（範囲に収める。保存の形と復旧の書き足すファイルの両方がこれを使う）
pub(crate) fn saved_fps(doc: &Document) -> u32 {
    doc.fps.clamp(1, crate::symbol::MAX_FPS)
}

impl SavedScopeMap {
    /// 地図のしまう形（`stored`）を、塊の表 `writer` の塊の番号の並びにする（共有している塊は1回だけ表に足す。保存の形
    /// `write_map_tiles` と、復旧の書き足すファイル `recovery_journal` の両方がこれを使う）
    pub(crate) fn write_tiles(&mut self, writer: &mut WriteTiles) {
        if let Some(stored) = self.stored.take() {
            self.tiles = writer.tiles_of(&stored);
        }
    }
}

/// テスト用：保存用の写し（書く形 `WriteTiles` でも、テストの保存の形でも）を、書いて読み戻してから使う
#[cfg(test)]
impl<T: Serialize> ProjectDocument<T> {
    /// 書いて読み戻してから文書を作る（アプリと同じ読み込みの道：`read_project`・`into_document_noting`）
    pub(crate) fn into_document(self) -> Result<Document, String> {
        read_project::<ReadTiles>(&project_file_bytes(&self)?[..])?.into_document()
    }

    /// 読み込んだ `ProjectDocument` を現在のアクティブタブへ入れる（アプリと同じ、作品を入れる道 `install_document`）
    pub(crate) fn apply_to_app(self, ws: &mut impl ProjectWorkspace) {
        let doc = self.into_document().expect("正しい地図の文書");
        install_document(ws, doc, None);
    }
}

impl<T> ProjectDocument<T> {
    /// 書いてある地図（ルートとシンボルのタイムラインの、キーフレームの順）みなに `f`
    pub(crate) fn for_each_map(&mut self, mut f: impl FnMut(&mut SavedScopeMap)) {
        let timelines = std::iter::once(&mut self.timeline)
            .chain(self.library.iter_mut().map(|s| &mut s.timeline));
        for tl in timelines {
            for kf in tl.layers.iter_mut().flat_map(|l| l.keyframes.iter_mut()) {
                kf.maps.iter_mut().for_each(&mut f);
            }
        }
    }
}

impl ProjectDocument<ReadTiles> {
    /// 書いてある小数（ステージの大きさ、地図の頂点と制御点、インスタンスの変換・中心点・色効果）がみな有限か。文書には
    /// 有限な数しか入らない（ステージの大きさは書く入口と同じ `stage_size_of`、地図は格子の内：`fits_grid`、インスタンスは
    /// 書く入口と同じ `SymbolInstanceBinding::is_finite`）ので、そうでなければ壊れたファイル（読むときに断る：
    /// `into_document_noting`。前の版のアプリが書いた有限でない数のあるファイルも、ここで断る）
    fn only_finite_numbers(&self) -> bool {
        let timeline_ok = |tl: &SavedTimeline| {
            tl.layers.iter().flat_map(|l| &l.keyframes).all(|kf| {
                // インスタンスは書く入口と同じ判定
                kf.instances
                    .iter()
                    .all(|inst| SymbolInstanceBinding::from(inst.binding).is_finite())
            })
        };
        crate::symbol::stage_size_of(Vec2::from(self.stage_size)).is_some()
            && self.map_tiles.only_finite_numbers()
            && std::iter::once(&self.timeline)
                .chain(self.library.iter().map(|s| &s.timeline))
                .all(timeline_ok)
    }

    /// テスト用：読んだ中身から文書を作る（中身の一部を除いたかを見ないテストのため。アプリは除いたものをいつも知らせる
    /// ので `into_document_noting` を使う）
    #[cfg(test)]
    pub(crate) fn into_document(self) -> Result<Document, String> {
        self.into_document_noting(&mut false)
    }

    /// 読んだ中身から文書を作る。壊れた・使えない所を除いたら `trimmed` に記す（除いた所でその場で記す：レイヤー・
    /// キーフレーム・インスタンスの `from_saved` と、無いシンボルを指すインスタンス。番号の振り直しや、空のキーフレーム・
    /// レイヤーを足すことは何も除かないので記さない）
    pub(crate) fn into_document_noting(mut self, trimmed: &mut bool) -> Result<Document, String> {
        if !self.only_finite_numbers() {
            return Err("ファイルが壊れています（有限でない数があります）".to_owned());
        }
        // 地図：塊の表から、塊を共有したしまう形を作る（番号が表の外なら壊れたファイル。塊に記した図形の印は新しい番号に
        // 替える：`TileReader::new`）
        let broken = || "プロジェクトファイルの地図が壊れています".to_owned();
        let reader = TileReader::new(std::mem::take(&mut self.map_tiles)).ok_or_else(broken)?;
        let mut all_read = true;
        self.for_each_map(|m| {
            m.stored = reader.map(&m.tiles);
            all_read &= m.stored.is_some();
        });
        if !all_read {
            return Err(broken());
        }
        // 音のライブラリ：どの音も、使える番号（`is_safe_id`）で番号が重ならないこと。そうでなければ壊れたファイル（黙って
        // 音を外さない）。波形にできない音は「読めない音」として開いて知らせる（`Sound::loaded`）
        let mut sounds: Vec<Arc<crate::sound::Sound>> = Vec::with_capacity(self.sounds.len());
        let mut sound_ids = HashSet::with_capacity(self.sounds.len());
        for s in std::mem::take(&mut self.sounds) {
            if !is_safe_id(s.id) || !sound_ids.insert(s.id) {
                return Err("プロジェクトファイルの音が壊れています".to_owned());
            }
            sounds.push(Arc::new(crate::sound::Sound::loaded(s.id, s.name, s.bytes)));
        }
        let fps = self.timeline.fps.clamp(1, crate::symbol::MAX_FPS);
        let mut timeline = Timeline::from_saved(self.timeline, trimmed)?;

        // 使えない・重なるシンボルの番号は新しい番号にして（`FreshIds`）ライブラリを作る
        let mut seen_sym_ids = HashSet::with_capacity(self.library.len());
        let mut sym_ids = FreshIds::after(self.library.iter().map(|s| s.id));
        let mut sym_id_remap: HashMap<u64, u64> = HashMap::new();
        let mut library: Vec<SymbolDef> = Vec::with_capacity(self.library.len());
        for saved_sym in self.library {
            let mut sym = SymbolDef::from_saved(saved_sym, trimmed)?;
            let orig_id = sym.id;
            if !is_safe_id(sym.id) || !seen_sym_ids.insert(sym.id) {
                sym.id = sym_ids.take();
                seen_sym_ids.insert(sym.id);
                if orig_id > 0 && !seen_sym_ids.contains(&orig_id) {
                    sym_id_remap.entry(orig_id).or_insert(sym.id);
                }
            }
            library.push(sym);
        }

        // 使えないグループの番号（group_id / instance_gid）を振り直す番号（`FreshIds`。同じ番号には同じ新しい番号）
        let used_gids = std::iter::once(&timeline)
            .chain(library.iter().map(|s| &s.timeline))
            .flat_map(|tl| &tl.layers)
            .flat_map(|layer| &layer.keyframes)
            .flat_map(|kf| {
                let maps = kf
                    .cel
                    .maps()
                    .iter()
                    .flat_map(|m| m.group_path.iter().copied());
                let insts = kf.cel.instances().iter().flat_map(|inst| {
                    inst.parent_scope
                        .iter()
                        .copied()
                        .chain([inst.binding.instance_gid])
                });
                maps.chain(insts)
            });
        let mut gids = FreshIds::after(used_gids);
        let mut gid_map: HashMap<u64, u64> = HashMap::new();

        // 存在しない symbol_id（孤児インスタンス）をルートタイムラインおよび各シンボルタイムラインから除去し、
        // 再採番された symbol_id / group_id / instance_gid の反映と tween フラグの正規化を行う
        for tl in std::iter::once(&mut timeline).chain(library.iter_mut().map(|s| &mut s.timeline))
        {
            for layer in &mut tl.layers {
                for i in 0..layer.keyframes.len() {
                    let kf = layer.keyframe_mut(i);
                    let needs_remap = kf
                        .cel
                        .maps()
                        .iter()
                        .any(|m| m.group_path.iter().any(|&gid| !is_safe_id(gid)))
                        || kf.cel.instances().iter().any(|inst| {
                            sym_id_remap.contains_key(&inst.binding.symbol_id)
                                || !is_safe_id(inst.binding.instance_gid)
                                || inst.parent_scope.iter().any(|&gid| !is_safe_id(gid))
                        });
                    if needs_remap {
                        kf.cel.remap_ids(
                            |gid| {
                                if is_safe_id(gid) {
                                    gid
                                } else {
                                    *gid_map.entry(gid).or_insert_with(|| gids.take())
                                }
                            },
                            |sym| sym_id_remap.get(&sym).copied().unwrap_or(sym),
                            None,
                        );
                    }
                    if kf
                        .cel
                        .instances()
                        .iter()
                        .any(|inst| !seen_sym_ids.contains(&inst.binding.symbol_id))
                    {
                        kf.cel
                            .instances_mut()
                            .retain(|inst| seen_sym_ids.contains(&inst.binding.symbol_id));
                        *trimmed = true;
                    }
                }
            }
        }

        // 数が有限なことは、読み込みの初めに確かめてある（`only_finite_numbers`。ステージの大きさも同じ `stage_size_of`）
        let stage_size = crate::symbol::stage_size_of(Vec2::from(self.stage_size))
            .expect("読み込みの初めに有限か確かめてある");
        // キーフレームの音の区間は、どれも音のライブラリの音を指し、始まりが終わりより前であること（`is_well_formed`）。
        // 音の長さを越える区間は残し、その音を知らせる（`exceeds`）
        let sounds = crate::sound::SoundLibrary::from(sounds);
        let mut short_sounds: Vec<u64> = Vec::new();
        for c in std::iter::once(&timeline).chain(library.iter().map(|s| &s.timeline)).flat_map(|tl| tl.sound_clips()) {
            if !c.is_well_formed(&sounds) {
                return Err("プロジェクトファイルの音が壊れています".to_owned());
            }
            if sounds.get(c.sound).is_some_and(|s| c.exceeds(s)) {
                short_sounds.push(c.sound);
            }
        }
        short_sounds.sort_unstable();
        short_sounds.dedup();
        let fresh_sound_ids = FreshIds::after(sounds.iter().map(|s| s.id));
        // レイヤーの番号は、どのタイムラインのレイヤーの番号よりも後から配る
        let fresh_layer_ids = FreshIds::after(
            std::iter::once(&timeline).chain(library.iter().map(|s| &s.timeline)).flat_map(|tl| tl.layers.iter().map(|l| l.id)),
        );
        let doc = Document::loaded(
            timeline,
            library.into_iter().map(Arc::new).collect(),
            sounds,
            stage_size,
            fps,
            [
                sym_ids.next_to_save(self.next_symbol_id),
                gids.next_to_save(self.next_group_id),
                fresh_sound_ids.next_to_save(self.next_sound_id),
                fresh_layer_ids.next_to_save(self.next_layer_id),
            ],
        );
        doc.watch_broken_maps();
        for s in doc.sounds.iter().filter(|s| !s.readable()) {
            doc.load_notices.push(crate::timeline::LoadNotice::UnreadableSound(s.name.clone()));
        }
        for s in short_sounds.iter().filter_map(|&id| doc.sounds.get(id)) {
            doc.load_notices.push(crate::timeline::LoadNotice::ShortSound(s.name.clone()));
        }
        Ok(doc)
    }
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

/// `.is2d` ファイルの頭：印・版・中身の長さ `len`・中身の CRC32 `crc`。頭を作るのはここだけ（確かめるのは
/// `read_project` だけ）
fn project_file_head(len: u64, crc: u32) -> [u8; PROJECT_FILE_HEADER_LEN] {
    let mut head = [0u8; PROJECT_FILE_HEADER_LEN];
    head[..8].copy_from_slice(&file_identity(SaveKind::Project));
    head[8..16].copy_from_slice(&len.to_le_bytes());
    head[16..].copy_from_slice(&crc.to_le_bytes());
    head
}

/// 書いたバイトの長さと CRC32 を数えながら `inner` に書く書き手（中身を手元に持たずに、頭の数を出す）。postcard は
/// 書き手の失敗を自分の失敗に置き換えるので、書き手の失敗は返さずにここに取っておき（失敗した後は `inner` に書かない）、
/// 書き終えてから見る
struct BodyWriter<W> {
    inner: W,
    len: u64,
    crc: crc32fast::Hasher,
    failed: Option<std::io::Error>,
}

impl<W: std::io::Write> std::io::Write for BodyWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.failed.is_none() {
            self.failed = self.inner.write_all(buf).err();
        }
        self.len += buf.len() as u64;
        self.crc.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.failed.is_none() {
            self.failed = self.inner.flush().err();
        }
        Ok(())
    }
}

/// 保存用の写し `doc` の `.is2d` ファイル（頭と、postcard で書いた中身）を `out` に書き、`out` を返す。頭の場所を空けて
/// 中身を流し込み（長さと CRC32 を数えながら：`BodyWriter`）、最後に頭へ戻って頭を書く。中身全体を1つの並びに持たない
/// （ファイルの大きさの一時領域を取らない：レビュー e90f27c の N-5001）
pub(crate) fn write_project<W: std::io::Write + std::io::Seek, T: Serialize>(
    mut out: W,
    doc: &ProjectDocument<T>,
) -> std::io::Result<W> {
    out.write_all(&[0; PROJECT_FILE_HEADER_LEN])?;
    let mut buffered = std::io::BufWriter::new(BodyWriter {
        inner: out,
        len: 0,
        crc: crc32fast::Hasher::new(),
        failed: None,
    });
    let streamed = postcard::to_io(doc, &mut buffered).map(drop);
    let body = buffered.into_inner().map_err(|e| e.into_error())?;
    if let Some(e) = body.failed {
        return Err(e);
    }
    streamed.map_err(|e| std::io::Error::other(format!("保存する形にできませんでした: {e}")))?;
    let head = project_file_head(body.len, body.crc.finalize());
    let mut out = body.inner;
    out.seek(std::io::SeekFrom::Start(0))?;
    out.write_all(&head)?;
    Ok(out)
}

/// テスト用：`.is2d` ファイルの中身を1つの並びにしたもの（`write_project`）
#[cfg(test)]
pub(crate) fn project_file_bytes<T: Serialize>(
    doc: &ProjectDocument<T>,
) -> Result<Vec<u8>, String> {
    write_project(std::io::Cursor::new(Vec::new()), doc)
        .map(std::io::Cursor::into_inner)
        .map_err(|e| e.to_string())
}

/// 読んだバイトの長さと CRC32 を数えながら `inner` から読む読み手（中身を手元に持たずに、頭の数と照らす：
/// `read_project`）。postcard は読み手の失敗を自分の失敗（中身が途中で終わった）に置き換えるので、読み手の失敗は返さずに
/// ここに取っておき（失敗した後は終わりとして何も読まない）、読み終えてから見る
pub(crate) struct BodyReader<R> {
    inner: R,
    len: u64,
    crc: crc32fast::Hasher,
    failed: Option<std::io::Error>,
}

impl<R> BodyReader<R> {
    /// `inner` から読んだ長さと CRC32 を数えながら読む読み手
    pub(crate) fn new(inner: R) -> Self {
        Self { inner, len: 0, crc: crc32fast::Hasher::new(), failed: None }
    }

    /// 読んだ長さと CRC32（読めなかったら、そのわけ）
    pub(crate) fn finish(self) -> std::io::Result<(u64, u32)> {
        match self.failed {
            Some(e) => Err(e),
            None => Ok((self.len, self.crc.finalize())),
        }
    }
}

impl<R: std::io::Read> std::io::Read for BodyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.failed.is_none() {
            match self.inner.read(buf) {
                Ok(n) => {
                    self.len += n as u64;
                    self.crc.update(&buf[..n]);
                    return Ok(n);
                }
                // 割り込まれただけなら読み直す（`Read` の決まり）
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => self.failed = Some(e),
            }
        }
        Ok(0)
    }
}

/// 中身を流しながら postcard に渡す所（postcard の `Flavor`）。数バイトずつ渡すときは、`input` の手元の一続きをそのまま
/// 貸し（写さない）、貸した所は次に読むときに読み終えたことにする（`lent`）。手元の一続きに収まらない長さ（長い文字列・
/// 一続きの切れ目にかかる数）は `scratch` に、読めたバイトの分だけ広げて置く（長さに上限を置かず、細工した長さでも
/// ファイルの大きさを越えて取らない）。保存用の型は借りる値を持たないので、借りたまま返す読み方（`try_take_n`）は無い
struct BodyFlavor<R> {
    input: std::io::BufReader<R>,
    lent: usize,
    scratch: Vec<u8>,
}

impl<R: std::io::Read> BodyFlavor<R> {
    /// 手元の、まだ渡していない一続き（無ければ次を読む。中身の終わりなら空）
    fn buffered(&mut self) -> postcard::Result<&[u8]> {
        use std::io::BufRead;
        self.input.consume(std::mem::take(&mut self.lent));
        self.input
            .fill_buf()
            .map_err(|_| postcard::Error::DeserializeUnexpectedEnd)
    }
}

impl<'de, R: std::io::Read + 'de> postcard::de_flavors::Flavor<'de> for BodyFlavor<R> {
    type Remainder = std::io::BufReader<R>;
    type Source = R;

    fn pop(&mut self) -> postcard::Result<u8> {
        let byte = *self
            .buffered()?
            .first()
            .ok_or(postcard::Error::DeserializeUnexpectedEnd)?;
        self.lent = 1;
        Ok(byte)
    }

    fn try_take_n(&mut self, _ct: usize) -> postcard::Result<&'de [u8]> {
        Err(postcard::Error::WontImplement)
    }

    fn try_take_n_temp<'a>(&'a mut self, ct: usize) -> postcard::Result<&'a [u8]>
    where
        'de: 'a,
    {
        use std::io::Read;
        if self.buffered()?.len() >= ct {
            self.lent = ct;
            return Ok(&self.input.buffer()[..ct]);
        }
        self.scratch.clear();
        let got = (&mut self.input)
            .take(ct as u64)
            .read_to_end(&mut self.scratch)
            .map_err(|_| postcard::Error::DeserializeUnexpectedEnd)?;
        if got < ct {
            return Err(postcard::Error::DeserializeUnexpectedEnd);
        }
        Ok(&self.scratch)
    }

    fn finalize(mut self) -> postcard::Result<std::io::BufReader<R>> {
        use std::io::BufRead;
        self.input.consume(self.lent);
        Ok(self.input)
    }
}

/// 流して読んだもの：読んだ値（postcard の答え）・読んだ長さ・その CRC32・読み残した長さ
pub(crate) struct Streamed<T> {
    pub(crate) value: postcard::Result<T>,
    pub(crate) len: u64,
    pub(crate) crc: u32,
    pub(crate) left: u64,
}

/// 読み手 `input` の中身を、型 `T` として postcard で流しながら読む（中身を1つの並びに持たず、取るのは読めたバイトの分
/// だけ：`BodyFlavor`）。読んだ長さと CRC32（読み残しも含めて読み切った分）も返す。作品ファイルの中身と、復旧ファイルの
/// 大きな記録を読む、1つの読み手（設計審査 b7208d9 の 6.。前は復旧ファイルだけファイル全体をメモリへ読んだ）
pub(crate) fn read_streamed<T: DeserializeOwned>(input: impl std::io::Read) -> Result<Streamed<T>, String> {
    let mut de = postcard::Deserializer::from_flavor(BodyFlavor {
        input: std::io::BufReader::with_capacity(READ_CHUNK_LEN, BodyReader::new(input)),
        lent: 0,
        scratch: Vec::new(),
    });
    let value = T::deserialize(&mut de);
    let mut rest = de.finalize().expect("流す所の終わりは失敗しない");
    let left = std::io::copy(&mut rest, &mut std::io::sink()).map_err(read_failed)?;
    let (len, crc) = rest.into_inner().finish().map_err(read_failed)?;
    Ok(Streamed { value, len, crc, left })
}

/// 作品のファイルを1回に読む大きさ（`read_project`）。読む1回ごとにシステムコールの手間がかかるので、小さいと開くのが
/// 遅くなる（`BufReader` の標準の 8KB では、作品 A（277MB）を読む回数が約 3 万 4 千回になり、読み込みが 465ms。256KB で
/// 368ms）。1MB・4MB でも 365〜370ms で変わらないので、速さが頭打ちになるいちばん小さい大きさにする（計測：Apple M5）
pub(crate) const READ_CHUNK_LEN: usize = 256 * 1024;

/// ファイルを読めなかったときの知らせ
fn read_failed(e: std::io::Error) -> String {
    format!("ファイルの読み込みに失敗しました: {e}")
}

/// `.is2d` ファイルを `input` から流しながら読む（ファイル全体のバイトも、保存の形の塊の表も手元に持たない：レビュー
/// e90f27c の N-5002）。頭（印・版）を確かめてから、中身を postcard で読みつつ長さと CRC32 を数え（`BodyReader`）、最後
/// まで読み切ってから、長さ・CRC32・postcard で読めたか（後ろに余りが無いか）を順に確かめる。どれかが合わなければ、
/// 何が違うかを書いた `Err` で、読んだ物は返さない（CRC32 を確かめる前に、読んだ物はどこにも使わない。中身の確かめは
/// `into_document_noting`）。CRC32 を確かめる前に読むので、壊れたファイルの長さの数で大きく取らない：取るのは、読めた
/// バイトに見合う分だけ（並びは長さの数によらず読めた要素の分だけ育つ。serde は長さの数で前もって取る分を小さく打ち切る。
/// 文字列は `BodyFlavor`）。頭を確かめるのはここだけ。塊の表は `T` の形で読む（アプリは `ReadTiles`）
pub(crate) fn read_project<T: DeserializeOwned + Default>(
    mut input: impl std::io::Read,
) -> Result<ProjectDocument<T>, String> {
    use std::io::Read;
    let mut head = Vec::with_capacity(PROJECT_FILE_HEADER_LEN);
    (&mut input)
        .take(PROJECT_FILE_HEADER_LEN as u64)
        .read_to_end(&mut head)
        .map_err(read_failed)?;
    if read_identity(&head)? != SaveKind::Project || head.len() < PROJECT_FILE_HEADER_LEN {
        return Err("is2d のファイルではありません".to_owned());
    }
    let u32_at = |i: usize| u32::from_le_bytes(head[i..i + 4].try_into().expect("4 バイト"));
    let body_len = u64::from_le_bytes(head[8..16].try_into().expect("8 バイト"));
    let Streamed { value: read, len, crc, left } = read_streamed::<ProjectDocument<T>>(input)?;
    if len != body_len {
        return Err("ファイルが壊れています（長さが合いません）".to_owned());
    }
    if crc != u32_at(16) {
        return Err("ファイルが壊れています（中身が変わっています）".to_owned());
    }
    match read {
        Ok(doc) if left == 0 => Ok(doc),
        Ok(_) => Err("プロジェクトファイルの形式が正しくありません（余りがあります）".to_owned()),
        Err(e) => Err(format!("プロジェクトファイルの形式が正しくありません: {e}")),
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

/// パスのファイル名を文字列で返す（取り出せなければ `fallback`）
pub(crate) fn file_name_or<'a>(path: &'a Path, fallback: &'a str) -> &'a str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
}

/// 指定されたパスへ `ProjectDocument` を一時ファイル経由で置き換えて書く（`write_atomically`）
pub(crate) fn write_project_file_atomically<T: Serialize>(
    target_path: &Path,
    proj_doc: &ProjectDocument<T>,
) -> Result<(), String> {
    write_atomically(target_path, |file| write_project(file, proj_doc))
        .map_err(|e| format!("ファイルの保存に失敗しました: {e}"))
}

/// 指定されたファイルパス `path` へプロジェクトを一時ファイル経由でアトミック保存する
pub(crate) fn save_project_to_path(
    ws: &mut impl ProjectWorkspace,
    path: &Path,
) -> Result<(), String> {
    prepare_active_tab_for_switch_or_save(ws);

    let proj_doc = ProjectDocument::from_app(ws.tab())?;

    let target_path = resolve_target_path(path);
    write_project_file_atomically(&target_path, &proj_doc)?;

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

/// ファイル `path` の作品を読み、確かめて返す（`read_project`。塊の表は、読みながら塊にする）
pub(crate) fn read_project_document(path: &Path) -> Result<ProjectDocument<ReadTiles>, String> {
    read_project(std::fs::File::open(path).map_err(read_failed)?)
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
    let doc = read_project_document(path)?.into_document_noting(&mut trimmed)?;
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
