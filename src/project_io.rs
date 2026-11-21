use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Vec2};
use kurbo::{Affine, Point};
use serde::{Deserialize, Serialize};

use crate::canvas_tool::{cancel_active_drag, SelectSubMode, Session, Tab, UndoHistory, View};
use crate::stored_map::{SavedTiles, TileReader, TileWriter};
use crate::symbol::{
    ColorEffect, Document, SymbolDef, SymbolInstance, SymbolInstanceBinding, SymbolLoopMode,
    TweenRotate,
};
pub(crate) use crate::timeline::MAX_LAYERS_PER_TIMELINE;
use crate::timeline::{
    Cel, Keyframe, KeyframeList, Layer, SavedScopeMap, ScopeMap, Timeline, TimelineCellSelection,
};

/// `.is2d` プロジェクトファイルの現在のフォーマットバージョン。頭（印・版・中身の長さ・中身の CRC32）のあとに、保存用の型
/// （`ProjectDocument`）を postcard で書いた中身を置く（`project_file_parts`。版 6 から）。版 7 は、地図をセル画にしまう
/// 形のまま書く：文書に1つの塊の表（升目ごとの、境目の線を1本で持つ辺の並び。辺は側の図形の番号も持つ）と、階層ごとの
/// 地図の塊の番号の並び。キーフレームどうしで共有する塊は1回だけ書き、読むときも共有したまま作る（ファイルの大きさが
/// 変わった塊の量に見合い、開いた後のメモリが見る順に左右されない：レビュー f01d518 の R4 の (a)）。インスタンスの重なり
/// 位置は階層の地図の並びで書く（`order`）。面の図形は文書に持たず、使うときに地図から作る。それより前の版は開かない。
/// 設計：claude/保存の形をバイナリにする設計_版6_7b5f6c4.md、claude/ゼロベースレビューf01d518の直し_まとまり3の説明.md
pub(crate) const PROJECT_FORMAT_VERSION: u32 = 8;

/// `.is2d` ファイルの先頭の印
const PROJECT_FILE_MAGIC: [u8; 4] = *b"IS2D";

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
    fn alloc_doc_id(&mut self) -> u64;
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
    ws.stop_playback();
    crate::doc_actions::apply(ws, crate::doc_actions::Action::EndCoalesce { key: None });
    cancel_active_drag(ws.tab_mut());
    ws.tab_mut().view.pan_last_screen_pos = None;
}

/// アクティブタブのインデックスを `target_index` へ切り替え、ペーストオフセット等をリセットする
fn activate_tab_index(ws: &mut impl ProjectWorkspace, target_index: usize) {
    ws.set_active_index(target_index);
    ws.clipboard_mut().paste_count = 0;
    ws.clipboard_mut().paste_offset = kurbo::Vec2::ZERO;
    if !ws.tab().session.selection.is_empty() && ws.tool().select_sub_mode != SelectSubMode::Reshape
    {
        ws.tool_mut().select_sub_mode = SelectSubMode::Reshape;
    }
}

/// アクティブタブのドキュメント・履歴・作業セッション・ビュー表示を一括初期化し、
/// 現在フレームの土台キャッシュを構築する共通ヘルパー
fn reset_active_document_state(tab: &mut Tab, doc: Document) {
    let selected_library_symbol = doc.library.first().map(|s| s.id);

    tab.doc = doc;
    tab.history = UndoHistory::default();
    tab.session = Session::default();
    tab.view = View::default();
    tab.view.selected_library_symbol = selected_library_symbol;
    tab.refresh_scene_base();
}

/// 現在アクティブなタブが「起動直後・新規作成直後で一度も編集されていない白紙の無題タブ」であるかを判定する。文書を
/// 変えるのはみな `apply` で、そのたびに世代 ID を進める（`UndoHistory::push`）ので、編集の有無はその1つの印で決める
/// （既定値を写して推さない：レビュー f01d518 の N-5008）。ファイルが無いこと（保存も読み込みもしていない）と、未保存の
/// 印が無いこと（別のタブの上書きでファイルから外したタブは、中身があっても編集していないので、この印で見分ける）も見る
pub(crate) fn is_active_tab_pristine_empty(tab: &Tab) -> bool {
    tab.file.is_none() && !tab.history.is_dirty() && tab.history.next_commit_id == 1
}

/// 指定したインデックス `index` のタブに未保存変更（`is_dirty() == true`）があるかを返す
pub(crate) fn is_tab_dirty(ws: &impl ProjectWorkspace, index: usize) -> bool {
    ws.tabs()
        .get(index)
        .is_some_and(|tab| tab.history.is_dirty())
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
    if ws.pending_close_confirm().is_some() {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        return;
    }
    if (0..ws.tabs().len()).any(|i| is_tab_dirty(ws, i)) {
        advance_viewport_close_confirm(ws, ws.active_index(), HashSet::new(), ctx);
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
        .find(|&i| is_tab_dirty(ws, i) && !discarded_doc_ids.contains(&ws.tabs()[i].id));
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

/// 未保存変更確認モーダルで「保存せずに破棄」が選択された際の処理を実行する
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
                discarded_doc_ids.insert(tab.id);
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
        let doc_id = ws.alloc_doc_id();
        let untitled_num = ws.alloc_untitled_number();

        ws.set_active_index(0);
        ws.tab_mut().id = doc_id;
        ws.tab_mut().untitled_number = untitled_num;
        ws.tab_mut().file = None;
        reset_active_document_state(ws.tab_mut(), Document::default());
        ws.tool_mut().select_sub_mode = SelectSubMode::Reshape;
        if let Some(c) = ctx {
            surrender_focused_widget(c);
        }
        return;
    }

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

/// グループの入れ子の深さの上限（読み込むときに、これより深い階層は壊れたファイルとして断る。手で作る入れ子はこれに
/// 届かない。階層を並べ直す所（`arranged`）は深さの2乗の手間とメモリを使うので、細工したファイルで使い果たさないため）
pub(crate) const MAX_GROUP_DEPTH: usize = 1_000;

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

/// 音のライブラリの1つの音の保存の形：番号・名前・読み込んだ WAV・MP3 のバイトのまま
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSound {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
}

impl SavedKeyframe {
    /// 始まりのコマが `start_frame` のキーフレーム `kf` の保存用の写し
    fn new(start_frame: usize, kf: &Keyframe) -> Self {
        Self {
            start_frame,
            tween: kf.tween,
            tween_rotate: kf.tween_rotate,
            maps: kf.cel.maps.iter().map(ScopeMap::saved_form).collect(),
            instances: kf
                .cel
                .instances
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
        // 階層（地図の階層とインスタンスの親の階層）の深さは `MAX_GROUP_DEPTH` まで。グループの番号ごとに、すぐ上の
        // グループは1つ（同じグループが2つの所に無い。すぐ上が1つなら、上の並び全体も1つに決まる）
        let mut parent_of: HashMap<u64, Option<u64>> = HashMap::new();
        let scopes = s
            .maps
            .iter()
            .map(|m| &m.group_path)
            .chain(s.instances.iter().map(|i| &i.parent_scope));
        for path in scopes {
            if path.len() > MAX_GROUP_DEPTH {
                return Err(broken());
            }
            for (k, &id) in path.iter().enumerate() {
                let parent = k.checked_sub(1).map(|j| path[j]);
                if *parent_of.entry(id).or_insert(parent) != parent {
                    return Err(broken());
                }
            }
        }
        let mut paths = HashSet::with_capacity(s.maps.len());
        for m in &s.maps {
            if !paths.insert(&m.group_path) || m.stored.is_none() {
                return Err(broken());
            }
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
            id: layer.id,
            name: layer.name.clone(),
            visible: layer.visible,
            locked: layer.locked,
            frame_count: layer.frame_count,
            keyframes: layer
                .keyframes_with_start()
                .map(|(start, kf)| SavedKeyframe::new(start, kf))
                .collect(),
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
    #[serde(default)]
    pub(crate) next_layer_id: u64,
    pub(crate) layers: Vec<SavedLayer>,
}

impl From<&Timeline> for SavedTimeline {
    fn from(tl: &Timeline) -> Self {
        Self {
            fps: 24,
            total_frames: tl.total_frames,
            current_frame: tl.current_frame,
            active_layer: tl.active_layer,
            next_layer_number: tl.next_layer_number,
            next_layer_id: tl.next_layer_id,
            layers: tl.layers.iter().map(SavedLayer::from).collect(),
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

        // 重複または安全上限（MAX_SAFE_ENTITY_ID）超過の Layer::id があれば一意な安全IDへ再採番し、各キーフレーム内の図形・インスタンスの layer_id も正規化する
        let mut seen_layer_ids = HashSet::with_capacity(layers.len());
        let mut max_layer_id = layers
            .iter()
            .map(|l| l.id)
            .filter(|&id| is_safe_id(id))
            .max()
            .unwrap_or(0);
        for layer in &mut layers {
            if !is_safe_id(layer.id) || !seen_layer_ids.insert(layer.id) {
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
            next_layer_number: s.next_layer_number.max(2),
            next_layer_id: raw_next_layer_id.max(max_layer_id.saturating_add(1)).max(2),
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

/// `.is2d` ファイルの中身のルート構造体（版の番号はファイルの頭にだけ持つ：`project_file_parts`）。postcard は項目を
/// 並びの位置で読むので、書かない項目（`skip_serializing_if`）は作らない
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProjectDocument {
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
    pub(crate) map_tiles: SavedTiles,
    /// 音のライブラリ（版 8 から）
    #[serde(default)]
    pub(crate) sounds: Vec<SavedSound>,
    #[serde(default)]
    pub(crate) next_sound_id: u64,
}

/// 番号 `id`（レイヤー・シンボル・グループ・インスタンス）が、ファイルから読んでそのまま使える番号か（0 でなく、安全な
/// 上限 `MAX_SAFE_ENTITY_ID` 以下）。読み込みで番号を確かめる決まりはこれ1つ
fn is_safe_id(id: u64) -> bool {
    id > 0 && id <= MAX_SAFE_ENTITY_ID
}

/// 使えない番号 `gid` の、新しいグループの番号（同じ番号には同じ新しい番号。新しい番号は `next` から順に振る）
fn renumbered_gid(map: &mut HashMap<u64, u64>, next: &mut u64, gid: u64) -> u64 {
    *map.entry(gid).or_insert_with(|| {
        let id = *next;
        *next = next.saturating_add(1);
        id
    })
}

impl ProjectDocument {
    /// 現在の `Tab` から保存用の `ProjectDocument` を生成する
    /// （`Floating` がある場合は `Document` のコピーに対して確定した内容を書き出し、`Session` も `Document` も書き換えない）。
    /// 浮いた片を置けなければ `Err`（画面に見えている片が入らないファイルは書かない）
    pub(crate) fn from_app(tab: &Tab) -> Result<Self, String> {
        let doc = tab.committed_document_copy().ok_or_else(|| {
            "選んだ図形を置けないため保存できません（動かすか、取り消してから保存してください）"
                .to_owned()
        })?;
        Ok(Self::of(&doc))
    }

    /// 文書 `doc` の保存用の写し
    pub(crate) fn of(doc: &Document) -> Self {
        let mut saved_timeline: SavedTimeline = (&doc.root).into();
        saved_timeline.fps = doc.fps.clamp(1, 60);

        let mut out = Self {
            stage_size: [doc.stage_size.x, doc.stage_size.y],
            timeline: saved_timeline,
            library: doc
                .library
                .iter()
                .map(|s| SavedSymbolDef::from(&**s))
                .collect(),
            next_symbol_id: doc.next_symbol_id,
            next_group_id: doc.next_group_id,
            recovery_source: None,
            map_tiles: SavedTiles::default(),
            sounds: doc
                .sounds
                .iter()
                .map(|s| SavedSound {
                    id: s.id,
                    name: s.name.clone(),
                    bytes: s.encoded.to_vec(),
                })
                .collect(),
            next_sound_id: doc.next_sound_id,
        };
        out.write_map_tiles();
        out
    }

    /// 地図のしまう形（`SavedScopeMap::stored`）を、塊の表（`map_tiles`）と塊の番号の並びにする。共有している塊は1回だけ
    /// 書く
    pub(crate) fn write_map_tiles(&mut self) {
        let mut writer = TileWriter::default();
        self.for_each_map(|m| {
            if let Some(stored) = m.stored.take() {
                m.tiles = writer.tiles_of(&stored);
            }
        });
        self.map_tiles = writer.finish();
    }

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

    /// 書いてある小数（ステージの大きさ、地図の頂点と制御点、インスタンスの変換・中心点・色効果）がみな有限か。文書には
    /// 有限な数しか入らない（ステージの大きさは書く入口と同じ `stage_size_of`、地図は格子の内：`fits_grid`、インスタンスは
    /// 書く入口と同じ `SymbolInstanceBinding::is_finite`）ので、そうでなければ壊れたファイル（読むときに断る：
    /// `project_from_bytes`。前の版のアプリが書いた有限でない数のあるファイルも、ここで断る）
    pub(crate) fn only_finite_numbers(&self) -> bool {
        let tiles_ok = self.map_tiles.tiles.iter().all(|t| {
            t.nodes.iter().flatten().all(|v| v.is_finite())
                && t.edges.iter().all(|e| e.ctrl.iter().all(|v| v.is_finite()))
        });
        let timeline_ok = |tl: &SavedTimeline| {
            tl.layers.iter().flat_map(|l| &l.keyframes).all(|kf| {
                // インスタンスは書く入口と同じ判定
                kf.instances
                    .iter()
                    .all(|inst| SymbolInstanceBinding::from(inst.binding).is_finite())
            })
        };
        crate::symbol::stage_size_of(Vec2::from(self.stage_size)).is_some()
            && tiles_ok
            && std::iter::once(&self.timeline)
                .chain(self.library.iter().map(|s| &s.timeline))
                .all(timeline_ok)
    }

    /// 読み込んだ `ProjectDocument` から正規化済みの `Document` を構築する。地図が壊れていれば `Err`
    /// 読んだ中身から文書を作る（中身の一部を除いたかは要らないとき：`into_document_noting`）
    pub(crate) fn into_document(self) -> Result<Document, String> {
        self.into_document_noting(&mut false)
    }

    /// 読んだ中身から文書を作る。壊れた・使えない所を除いたら `trimmed` に記す（除いた所でその場で記す：レイヤー・
    /// キーフレーム・インスタンスの `from_saved` と、無いシンボルを指すインスタンス。番号の振り直しや、空のキーフレーム・
    /// レイヤーを足すことは何も除かないので記さない）
    pub(crate) fn into_document_noting(mut self, trimmed: &mut bool) -> Result<Document, String> {
        // 地図：塊の表から、塊を共有したしまう形を作る（番号が表の外なら壊れたファイル。塊に記した図形の印は新しい番号に
        // 替える：`TileReader::new`）
        let broken = || "プロジェクトファイルの地図が壊れています".to_owned();
        let reader = TileReader::new(&self.map_tiles).ok_or_else(broken)?;
        let mut all_read = true;
        self.for_each_map(|m| {
            m.stored = reader.map(&m.tiles);
            all_read &= m.stored.is_some();
        });
        if !all_read {
            return Err(broken());
        }
        self.map_tiles = SavedTiles::default();
        // 音のライブラリ：どの音も、使える番号（`is_safe_id`）で番号が重ならず、波形にできること。そうでなければ壊れた
        // ファイル（黙って音を外さない）
        let mut sounds: Vec<Arc<crate::sound::Sound>> = Vec::with_capacity(self.sounds.len());
        let mut sound_ids = HashSet::with_capacity(self.sounds.len());
        for s in std::mem::take(&mut self.sounds) {
            if !is_safe_id(s.id) || !sound_ids.insert(s.id) {
                return Err("プロジェクトファイルの音が壊れています".to_owned());
            }
            sounds.push(Arc::new(crate::sound::Sound::new(s.id, s.name, s.bytes.into())?));
        }
        let fps = self.timeline.fps.clamp(1, 60);
        let mut timeline = Timeline::from_saved(self.timeline, trimmed)?;

        // 重複または不正（id == 0 / id > MAX_SAFE_ENTITY_ID）な SymbolDef::id を一意な安全IDへ再採番してライブラリを構築
        let mut seen_sym_ids = HashSet::with_capacity(self.library.len());
        let mut next_fallback_sym_id = self
            .library
            .iter()
            .map(|s| s.id)
            .filter(|&id| is_safe_id(id))
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        let mut sym_id_remap: HashMap<u64, u64> = HashMap::new();
        let mut library: Vec<SymbolDef> = Vec::with_capacity(self.library.len());
        for saved_sym in self.library {
            let mut sym = SymbolDef::from_saved(saved_sym, trimmed)?;
            let orig_id = sym.id;
            if !is_safe_id(sym.id) || !seen_sym_ids.insert(sym.id) {
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
                    for m in kf.cel.maps.iter() {
                        for &gid in &m.group_path {
                            if is_safe_id(gid) {
                                max_valid_gid = max_valid_gid.max(gid);
                            }
                        }
                    }
                    for inst in kf.cel.instances.iter() {
                        for &gid in &inst.parent_scope {
                            if is_safe_id(gid) {
                                max_valid_gid = max_valid_gid.max(gid);
                            }
                        }
                        if is_safe_id(inst.binding.instance_gid) {
                            max_valid_gid = max_valid_gid.max(inst.binding.instance_gid);
                        }
                    }
                }
            }
        }
        let mut next_gid = max_valid_gid.saturating_add(1).max(1);
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
                        .maps
                        .iter()
                        .any(|m| m.group_path.iter().any(|&gid| !is_safe_id(gid)))
                        || kf.cel.instances.iter().any(|inst| {
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
                                    renumbered_gid(&mut gid_map, &mut next_gid, gid)
                                }
                            },
                            |sym| sym_id_remap.get(&sym).copied().unwrap_or(sym),
                            None,
                        );
                    }
                    if kf
                        .cel
                        .instances
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

        let max_sym_id = library.iter().map(|s| s.id).max().unwrap_or(0);
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
        // 数が有限なことは、読み込みの初めに確かめてある（`only_finite_numbers`。ステージの大きさも同じ `stage_size_of`）
        let stage_size = crate::symbol::stage_size_of(Vec2::from(self.stage_size))
            .expect("読み込みの初めに有限か確かめてある");
        // キーフレームの音の区間は、どれも音のライブラリの音を指し、その音の長さに収まること（`SoundClip::fits`）
        let clips_fit = std::iter::once(&timeline)
            .chain(library.iter().map(|s| &s.timeline))
            .flat_map(|tl| tl.layers.iter())
            .flat_map(|l| l.keyframes.iter())
            .filter_map(|kf| kf.sound)
            .all(|c| c.fits(&sounds));
        if !clips_fit {
            return Err("プロジェクトファイルの音が壊れています".to_owned());
        }
        let max_sound_id = sounds.iter().map(|s| s.id).max().unwrap_or(0);
        let raw_next_sound_id = if self.next_sound_id <= MAX_SAFE_ENTITY_ID {
            self.next_sound_id
        } else {
            1
        };
        let doc = Document {
            root: timeline,
            library: library.into_iter().map(Arc::new).collect(),
            sounds: Arc::new(sounds),
            stage_size,
            fps,
            next_symbol_id: raw_next_sym_id.max(max_sym_id.saturating_add(1)).max(1),
            next_group_id: raw_next_gid.max(next_gid).max(1),
            next_sound_id: raw_next_sound_id.max(max_sound_id.saturating_add(1)).max(1),
            revision: crate::vector_engine::next_mesh_revision(),
            broken_maps: Default::default(),
        };
        doc.watch_broken_maps();
        Ok(doc)
    }

    /// 読み込んだ `ProjectDocument` を現在のアクティブタブへ適用し、作業キャンバスや履歴を初期化する
    #[cfg(test)]
    pub(crate) fn apply_to_app(self, tab: &mut Tab) {
        let doc = self.into_document().expect("正しい地図の文書");
        reset_active_document_state(tab, doc);
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

/// 保存用の写し `doc` の `.is2d` ファイルの中身を、頭（印・版・中身の長さ・中身の CRC32）と、postcard で書いた中身に
/// 分けて作る。頭を作るのはここだけ（確かめるのは `project_from_bytes` だけ）。ファイルにはこの順に続けて書く
/// （`write_project_bytes`。中身を頭と1つの並びに写し直さない：レビュー f01d518 の N-5007）
pub(crate) fn project_file_parts(
    doc: &ProjectDocument,
) -> Result<([u8; PROJECT_FILE_HEADER_LEN], Vec<u8>), String> {
    let body =
        postcard::to_allocvec(doc).map_err(|e| format!("保存する形にできませんでした: {e}"))?;
    let mut head = [0u8; PROJECT_FILE_HEADER_LEN];
    head[..4].copy_from_slice(&PROJECT_FILE_MAGIC);
    head[4..8].copy_from_slice(&PROJECT_FORMAT_VERSION.to_le_bytes());
    head[8..16].copy_from_slice(&(body.len() as u64).to_le_bytes());
    head[16..].copy_from_slice(&crc32fast::hash(&body).to_le_bytes());
    Ok((head, body))
}

/// テスト用：`.is2d` ファイルの中身を1つの並びにしたもの（頭と中身：`project_file_parts`）
#[cfg(test)]
pub(crate) fn project_file_bytes(doc: &ProjectDocument) -> Result<Vec<u8>, String> {
    let (head, body) = project_file_parts(doc)?;
    Ok([&head[..], &body].concat())
}

/// `.is2d` ファイルの中身 `bytes` を読む。頭（印・版・中身の長さ・CRC32）を順に確かめてから中身を読み、数がみな有限か
/// 確かめる（`only_finite_numbers`）。どれかが合わなければ、何が違うかを書いた `Err`
pub(crate) fn project_from_bytes(bytes: &[u8]) -> Result<ProjectDocument, String> {
    if bytes.len() < PROJECT_FILE_HEADER_LEN || bytes[..4] != PROJECT_FILE_MAGIC {
        return Err(if bytes.first() == Some(&b'{') {
            "前の形式（JSON）のファイルは開けません".to_owned()
        } else {
            "is2d のファイルではありません".to_owned()
        });
    }
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("4 バイト"));
    let version = u32_at(4);
    if version != PROJECT_FORMAT_VERSION {
        return Err(format!(
            "未対応のプロジェクトバージョンです (version: {version})"
        ));
    }
    let body_len = u64::from_le_bytes(bytes[8..16].try_into().expect("8 バイト"));
    let body = &bytes[PROJECT_FILE_HEADER_LEN..];
    if body.len() as u64 != body_len {
        return Err("ファイルが壊れています（長さが合いません）".to_owned());
    }
    if crc32fast::hash(body) != u32_at(16) {
        return Err("ファイルが壊れています（中身が変わっています）".to_owned());
    }
    let doc = match postcard::take_from_bytes::<ProjectDocument>(body) {
        Ok((doc, [])) => doc,
        Ok(_) => {
            return Err("プロジェクトファイルの形式が正しくありません（余りがあります）".to_owned())
        }
        Err(e) => return Err(format!("プロジェクトファイルの形式が正しくありません: {e}")),
    };
    if !doc.only_finite_numbers() {
        return Err("ファイルが壊れています（有限でない数があります）".to_owned());
    }
    Ok(doc)
}

/// `.is2d` ファイルの頭 `head` と中身 `body`（`project_file_parts`）を、この順にファイル `file` に書いて、ディスクまで書き
/// 込む
pub(crate) fn write_project_bytes(
    file: std::fs::File,
    head: &[u8],
    body: &[u8],
) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = file;
    file.write_all(head)?;
    file.write_all(body)?;
    file.sync_all()
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

/// 指定されたパスへ `ProjectDocument` を一時ファイル経由でアトミックに書き出し、親ディレクトリを同期する
pub(crate) fn write_project_file_atomically(
    target_path: &Path,
    proj_doc: &ProjectDocument,
) -> Result<(), String> {
    let tmp_name = format!(
        ".{}.{}.tmp",
        file_name_or(target_path, "project.is2d"),
        std::process::id()
    );
    let tmp_path = target_path.with_file_name(tmp_name);
    let (head, body) = project_file_parts(proj_doc)?;
    let write_res =
        std::fs::File::create(&tmp_path).and_then(|file| write_project_bytes(file, &head, &body));
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("一時ファイルの書き込みに失敗しました: {e}"));
    }

    if let Err(e) = std::fs::rename(&tmp_path, target_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("ファイルの保存に失敗しました: {e}"));
    }
    if let Some(parent) = target_path.parent() {
        if let Ok(dir_file) = std::fs::File::open(parent) {
            let _ = dir_file.sync_all();
        }
    }
    Ok(())
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
            tab.history.mark_unsaved();
        }
    }

    ws.tab_mut().file = Some(path.to_path_buf());
    ws.tab_mut().history.mark_saved();
    Ok(())
}

/// 指定されたファイルパス `path` から `ProjectDocument` を読み込み・確かめて返す（`project_from_bytes`）
pub(crate) fn read_project_document(path: &Path) -> Result<ProjectDocument, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("ファイルの読み込みに失敗しました: {e}"))?;
    project_from_bytes(&bytes)
}

/// 検証済みの `Document` を新しいタブ（または未編集の白紙無題タブ）へ読み込んでアクティブ化する
pub(crate) fn open_document_in_new_tab(
    ws: &mut impl ProjectWorkspace,
    doc: Document,
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
        let new_doc_id = ws.alloc_doc_id();
        ws.tab_mut().id = new_doc_id;
        ws.tab_mut().untitled_number = 0;
        let max_untitled = ws
            .tabs()
            .iter()
            .map(|t| t.untitled_number)
            .max()
            .unwrap_or(0);
        ws.set_next_untitled_number(max_untitled + 1);
    }

    reset_active_document_state(ws.tab_mut(), doc);
    ws.tool_mut().select_sub_mode = SelectSubMode::Reshape;
    // テストでは裏で確かめない（まだ確かめていない地図を調べるテストが、裏のスレッドと競わないように。裏で確かめる
    // テストは `check_maps_in_background` を直接呼ぶ）
    #[cfg(not(test))]
    let _ = ws.tab().doc.check_maps_in_background();
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
    open_document_in_new_tab(ws, doc)?;

    ws.tab_mut().file = Some(path.to_path_buf());
    if trimmed {
        ws.tab_mut().history.mark_unsaved();
    } else {
        ws.tab_mut().history.mark_saved();
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
    ws.tab_mut().view.reset_view();

    surrender_focused_widget(ctx);
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
            crate::canvas_tool::show_notice(
                ws.toast_mut(),
                crate::canvas_tool::NoticeKind::Warning,
                format!(
                    "「{name}」の一部を読み込めなかったため除きました（保存すると元のファイルからも消えます）"
                ),
            );
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
    let name = ws
        .tab()
        .file
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let num = ws.tab().untitled_number.max(1);
            format!("untitled-{num}.is2d")
        });

    let Some(path) = pick_save_path(ws.tab(), PROJECT_FILTER, PROJECT_FILE_EXTENSION, &name) else {
        return;
    };
    save_project_and_notify(ws, &path, "を保存しました");
}
