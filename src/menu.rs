//! メニューの項目の表（ファイル・編集・表示）。項目の名前・ショートカット・使えるかどうか・押したときに渡すものは、
//! ここにだけ書く。Mac では OS のメニューバー（`app_menu_macos`）、Windows・Linux では窓の中の egui のメニューバー
//! （`panels::draw_top_menu_bar`）が、この表を見せる。キーのショートカットも、この表から決める（`Is2dApp::handle_shortcuts`）。
//! 設計：claude/OSメニューバーの設計_605fa23.md

use eframe::egui::{self, Key};

use crate::canvas_tool::{active_scope_layer_id, Clipboard, Tab, View, ViewSwitch};
use crate::doc_actions::Action;
use crate::panels::UiAction;
#[cfg(not(target_os = "macos"))]
use crate::panels::MOD_KEY;

/// メニューバーに並ぶメニュー
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Menu {
    File,
    Edit,
    View,
}

impl Menu {
    /// メニューバーに並べる順
    pub(crate) const ALL: [Self; 3] = [Self::File, Self::Edit, Self::View];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::File => "ファイル",
            Self::Edit => "編集",
            Self::View => "表示",
        }
    }
}

/// ショートカットのキー（キーと、`Cmd`（Windows・Linux では `Ctrl`）・`Shift` を押しているか。ほかの修飾キーは見ない）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Shortcut {
    pub(crate) key: Key,
    pub(crate) command: bool,
    pub(crate) shift: bool,
}

impl Shortcut {
    /// `Cmd`（`Ctrl`）と合わせて押すキー
    const fn command(key: Key) -> Self {
        Self {
            key,
            command: true,
            shift: false,
        }
    }

    /// `Cmd`（`Ctrl`）と `Shift` と合わせて押すキー
    const fn command_shift(key: Key) -> Self {
        Self {
            key,
            command: true,
            shift: true,
        }
    }

    /// 修飾キーなしで押すキー
    const fn plain(key: Key) -> Self {
        Self {
            key,
            command: false,
            shift: false,
        }
    }

    /// 窓の中のメニュー（Windows・Linux）に出す文字（例：`Ctrl+Shift+S`・`Backspace`）。Mac の OS のメニューは、
    /// OS が `Cmd` のショートカットを記号で出す
    #[cfg(not(target_os = "macos"))]
    pub(crate) fn text(self) -> String {
        let mut parts = Vec::new();
        if self.command {
            parts.push(MOD_KEY);
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(self.key.name());
        parts.join("+")
    }
}

/// 入力欄に文字を入れている最中に、項目を入力欄へ渡すときの文字の操作（3章の1）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TextOp {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    /// 選んだ文字（無ければカーソルの前の1文字）を消す
    Delete,
}

impl TextOp {
    /// 入力欄へ渡す egui の入力の合図（egui の入力欄 `egui::TextEdit` が受け取る形。貼り付ける文字は `pasted` から取る）
    #[cfg(target_os = "macos")]
    pub(crate) fn event(self, pasted: impl FnOnce() -> String) -> egui::Event {
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        };
        match self {
            Self::Undo => key(Key::Z, egui::Modifiers::COMMAND),
            Self::Redo => key(Key::Y, egui::Modifiers::COMMAND),
            Self::Cut => egui::Event::Cut,
            Self::Copy => egui::Event::Copy,
            Self::Paste => egui::Event::Paste(pasted()),
            Self::Delete => key(Key::Backspace, egui::Modifiers::NONE),
        }
    }
}

/// メニューの項目1つ
pub(crate) struct MenuCommand {
    pub(crate) menu: Menu,
    /// この項目の前に区切り線を引くか
    pub(crate) separator_before: bool,
    pub(crate) label: &'static str,
    /// メニューに出すショートカット。Mac では、`Cmd` の付くものだけを OS のメニューに付ける（付けると OS が先に
    /// 受け取るので、`Cmd` の付かないキーを付けると入力欄で文字を消したり打ったりできなくなる）
    pub(crate) shortcut: Option<Shortcut>,
    /// メニューに出さないもう1つのキー（egui のキー処理だけが受け取る）
    pub(crate) alias: Option<Shortcut>,
    /// キーを押しっぱなしにしたときの繰り返しでも動くか（egui のキー処理。Mac の OS のメニューのキーは OS の決まりのまま）
    pub(crate) repeats: bool,
    /// 表の決まりで使えるか（今と同じ決まり。入力欄へ渡すときと、ダイアログが出ている間の扱いは
    /// `Is2dApp::menu_item_state`）
    pub(crate) enabled: fn(&Tab, &Clipboard) -> bool,
    /// 入力欄に文字を入れている最中は、入力欄へ渡す文字の操作（`MenuCommand::text_op_for`）
    pub(crate) text_op: Option<TextOp>,
    /// 押したときに渡すもの
    pub(crate) action: UiAction,
}

impl MenuCommand {
    /// 押したときに入力欄へ渡す文字の操作。入力欄に文字を入れている最中（`typing`。egui の `wants_keyboard_input`）なら、
    /// 文字の操作のある項目は入力欄へ渡す。`None` ならアプリへ渡す（`action`）。渡す先を決めるのはこれ1つ
    pub(crate) fn text_op_for(&self, typing: bool) -> Option<TextOp> {
        self.text_op.filter(|_| typing)
    }

    /// チェックの付く項目（表示の切り替え）なら、今オンか
    pub(crate) fn checked(&self, view: &View) -> Option<bool> {
        match self.action {
            UiAction::ToggleView(switch) => Some(view.switch(switch)),
            _ => None,
        }
    }

    /// egui のキー処理が受け取るキー。OS のメニューに付けた `Cmd` のショートカット（`os_menu_has_keys`）は、OS が先に
    /// 受け取るので含めない（1回のキーで両方が動かないように）
    fn egui_keys(&self, os_menu_has_keys: bool) -> impl Iterator<Item = Shortcut> {
        let shown = self.shortcut.filter(|s| !(os_menu_has_keys && s.command));
        shown.into_iter().chain(self.alias)
    }
}

/// メニューの項目それぞれの、今の見せ方
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ItemState {
    pub(crate) enabled: bool,
    /// チェックの付く項目なら、今オンか
    pub(crate) checked: Option<bool>,
}

fn always(_: &Tab, _: &Clipboard) -> bool {
    true
}

/// 音を付けられるのは、作業中のレイヤーの今のコマが空白のキーフレームのときだけ（`Timeline::active_sound_slot`）
fn can_import_sound(tab: &Tab, _: &Clipboard) -> bool {
    tab.active_timeline().active_sound_slot().is_some()
}

fn can_undo(tab: &Tab, _: &Clipboard) -> bool {
    !tab.history.undo_stack.is_empty()
}

fn can_redo(tab: &Tab, _: &Clipboard) -> bool {
    !tab.history.redo_stack.is_empty()
}

fn has_selection(tab: &Tab, _: &Clipboard) -> bool {
    !tab.session.selection.is_empty()
}

fn can_paste(tab: &Tab, clipboard: &Clipboard) -> bool {
    let tl = tab.active_timeline();
    clipboard.content.is_some()
        && tl.is_layer_editable_by_id(active_scope_layer_id(&tab.session, tl))
}

/// 項目の数
pub(crate) const COMMAND_COUNT: usize = 19;

/// メニューの項目の表（メニューバーの順・メニューの中の上からの順）。番号（並びの位置）で項目を指す
pub(crate) static COMMANDS: [MenuCommand; COMMAND_COUNT] = [
    // ファイル
    MenuCommand {
        menu: Menu::File,
        separator_before: false,
        label: "新規作成",
        shortcut: Some(Shortcut::command(Key::N)),
        alias: None,
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::NewProject,
    },
    MenuCommand {
        menu: Menu::File,
        separator_before: true,
        label: "開く...",
        shortcut: Some(Shortcut::command(Key::O)),
        alias: None,
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::OpenProjectDialog,
    },
    MenuCommand {
        menu: Menu::File,
        separator_before: false,
        label: "保存",
        shortcut: Some(Shortcut::command(Key::S)),
        alias: None,
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::SaveProject,
    },
    MenuCommand {
        menu: Menu::File,
        separator_before: false,
        label: "名前を付けて保存...",
        shortcut: Some(Shortcut::command_shift(Key::S)),
        alias: None,
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::SaveProjectAsDialog,
    },
    MenuCommand {
        menu: Menu::File,
        separator_before: true,
        label: "音を読み込む（WAV・MP3）…",
        shortcut: None,
        alias: None,
        repeats: false,
        enabled: can_import_sound,
        text_op: None,
        action: UiAction::ImportSoundDialog,
    },
    MenuCommand {
        menu: Menu::File,
        separator_before: true,
        label: "動画を書き出し（MP4）…",
        shortcut: None,
        alias: None,
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::ExportVideoDialog,
    },
    MenuCommand {
        menu: Menu::File,
        separator_before: true,
        label: "タブを閉じる",
        shortcut: Some(Shortcut::command(Key::W)),
        alias: None,
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::CloseActiveTab,
    },
    // 編集
    MenuCommand {
        menu: Menu::Edit,
        separator_before: false,
        label: "元に戻す",
        shortcut: Some(Shortcut::command(Key::Z)),
        alias: None,
        repeats: true,
        enabled: can_undo,
        text_op: Some(TextOp::Undo),
        action: UiAction::Doc(Action::Undo),
    },
    MenuCommand {
        menu: Menu::Edit,
        separator_before: false,
        label: "やり直し",
        shortcut: Some(Shortcut::command(Key::Y)),
        alias: Some(Shortcut::command_shift(Key::Z)),
        repeats: true,
        enabled: can_redo,
        text_op: Some(TextOp::Redo),
        action: UiAction::Doc(Action::Redo),
    },
    MenuCommand {
        menu: Menu::Edit,
        separator_before: true,
        label: "カット",
        shortcut: Some(Shortcut::command(Key::X)),
        alias: None,
        repeats: false,
        enabled: has_selection,
        text_op: Some(TextOp::Cut),
        action: UiAction::Doc(Action::Cut),
    },
    MenuCommand {
        menu: Menu::Edit,
        separator_before: false,
        label: "コピー",
        shortcut: Some(Shortcut::command(Key::C)),
        alias: None,
        repeats: false,
        enabled: has_selection,
        text_op: Some(TextOp::Copy),
        action: UiAction::Doc(Action::Copy),
    },
    MenuCommand {
        menu: Menu::Edit,
        separator_before: false,
        label: "ペースト",
        shortcut: Some(Shortcut::command(Key::V)),
        alias: None,
        repeats: false,
        enabled: can_paste,
        text_op: Some(TextOp::Paste),
        action: UiAction::Doc(Action::Paste { in_place: false }),
    },
    MenuCommand {
        menu: Menu::Edit,
        separator_before: false,
        label: "同じ位置にペースト",
        shortcut: Some(Shortcut::command_shift(Key::V)),
        alias: None,
        repeats: false,
        enabled: can_paste,
        text_op: Some(TextOp::Paste),
        action: UiAction::Doc(Action::Paste { in_place: true }),
    },
    MenuCommand {
        menu: Menu::Edit,
        separator_before: true,
        label: "選択削除",
        shortcut: Some(Shortcut::plain(Key::Backspace)),
        alias: Some(Shortcut::plain(Key::Delete)),
        repeats: false,
        enabled: has_selection,
        text_op: Some(TextOp::Delete),
        action: UiAction::Doc(Action::DeleteSelection),
    },
    // 表示
    MenuCommand {
        menu: Menu::View,
        separator_before: false,
        label: "100%表示",
        shortcut: Some(Shortcut::command(Key::Num1)),
        alias: Some(Shortcut::command(Key::Num0)),
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::ResetView,
    },
    MenuCommand {
        menu: Menu::View,
        separator_before: false,
        label: "ウィンドウに合わせる",
        shortcut: Some(Shortcut::command(Key::Num2)),
        alias: None,
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::FitStageToWindow,
    },
    MenuCommand {
        menu: Menu::View,
        separator_before: true,
        label: "枠外を隠す",
        shortcut: Some(Shortcut::command(Key::Num3)),
        alias: None,
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::ToggleView(ViewSwitch::ClipOutsideStage),
    },
    MenuCommand {
        menu: Menu::View,
        separator_before: false,
        label: "アンカーを表示",
        shortcut: None,
        alias: None,
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::ToggleView(ViewSwitch::Anchors),
    },
    MenuCommand {
        menu: Menu::View,
        separator_before: false,
        label: "オニオンスキン",
        shortcut: Some(Shortcut::plain(Key::O)),
        alias: None,
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::ToggleView(ViewSwitch::OnionSkin),
    },
];

/// キー `shortcut` で動く項目の番号。OS のメニューがキーを受け取るとき（`os_menu_has_keys`）は、そのキーは含めない
pub(crate) fn command_for_key(shortcut: Shortcut, os_menu_has_keys: bool) -> Option<usize> {
    COMMANDS
        .iter()
        .position(|c| c.egui_keys(os_menu_has_keys).any(|k| k == shortcut))
}

/// このコマに押したショートカットと、押しっぱなしの繰り返しか。egui-winit は `Cmd+C`・`Cmd+X`・`Cmd+V` を
/// キーの合図でなくコピー・カット・貼り付けの合図で送るので、それをキーに戻す（`Shift` はそのときの修飾キー）。
/// 捨てられた `Cmd+V`（`swallowed_paste`：`Clipboard::swallowed_paste`）も `Cmd+V` にする
pub(crate) fn pressed_shortcuts(
    input: &egui::InputState,
    swallowed_paste: bool,
) -> Vec<(Shortcut, bool)> {
    let clipboard_key = |key| Shortcut {
        key,
        command: true,
        shift: input.modifiers.shift,
    };
    let mut pressed: Vec<(Shortcut, bool)> = input
        .events
        .iter()
        .filter_map(|ev| match ev {
            egui::Event::Key {
                key,
                pressed: true,
                repeat,
                modifiers,
                ..
            } => Some((
                Shortcut {
                    key: *key,
                    command: modifiers.command || modifiers.ctrl,
                    shift: modifiers.shift,
                },
                *repeat,
            )),
            egui::Event::Copy => Some((clipboard_key(Key::C), false)),
            egui::Event::Cut => Some((clipboard_key(Key::X), false)),
            egui::Event::Paste(_) => Some((clipboard_key(Key::V), false)),
            _ => None,
        })
        .collect();
    if swallowed_paste {
        pressed.push((clipboard_key(Key::V), false));
    }
    pressed
}
