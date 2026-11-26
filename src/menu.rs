//! メニューの項目の表（ファイル・編集・表示）。項目の名前・ショートカット・使えるかどうか・押したときに渡すものは、
//! ここにだけ書く。Mac では OS のメニューバー（`app_menu_macos`）、Windows・Linux では窓の中の egui のメニューバー
//! （`panels::draw_top_menu_bar`）が、この表を見せる。キーのショートカットも、この表から決める（`Is2dApp::handle_shortcuts`）。
//! 設計：claude/OSメニューバーの設計_605fa23.md

use eframe::egui::{self, Key};

use crate::canvas_tool::{active_scope_layer_id, Clipboard, Tab, View, ViewSwitch};
use crate::doc_actions::Action;
use crate::panels::{ArrowDir, FrameEnd, UiAction, ZoomStep};
use crate::canvas_tool::{SelectSubMode, Tool};
use crate::select_ops::ArrangeMode;
use crate::panels::MOD_KEY;

/// メニュー。`App` はアプリのメニュー（終了）：Mac では OS のアプリメニュー（「is2d」）に、Windows・Linux では窓の中の
/// メニューバーにアプリのメニューが無いので「ファイル」の一番下に出す（`in_window_bar`）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Menu {
    App,
    File,
    Edit,
    View,
}

impl Menu {
    /// メニューバーに並べる順（アプリのメニューは、Mac では OS が先頭に置き、Windows・Linux では「ファイル」に入れる）
    pub(crate) const ALL: [Self; 3] = [Self::File, Self::Edit, Self::View];

    /// 窓の中のメニューバー（Windows・Linux）で、このメニューの項目を出すメニュー
    #[cfg(not(target_os = "macos"))]
    pub(crate) const fn in_window_bar(self) -> Self {
        match self {
            Self::App => Self::File,
            m => m,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::App => "is2d",
            Self::File => "ファイル",
            Self::Edit => "編集",
            Self::View => "表示",
        }
    }
}

/// ショートカットの `Shift` の扱い
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ShiftKey {
    /// 押していないとき
    Off,
    /// 押しているとき
    On,
    /// どちらでも（前からキーの決まりが `Shift` を見ていなかったキー）
    Either,
}

/// ショートカットのキー（キーと、`Cmd`（Windows・Linux では `Ctrl`）・`Shift`。ほかの修飾キーは見ない）。`by_position` の
/// キーは、キーの意味でなくキーの位置（`Event::Key` の `physical_key`）で見る（配列で刻印の位置が違う `+` のズーム）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Shortcut {
    pub(crate) key: Key,
    pub(crate) command: bool,
    pub(crate) shift: ShiftKey,
    pub(crate) by_position: bool,
}

/// 押したキー1つ（キーの意味・キーの位置・`Cmd`（`Ctrl`）・`Shift`）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Pressed {
    pub(crate) key: Key,
    pub(crate) position: Option<Key>,
    pub(crate) command: bool,
    pub(crate) shift: bool,
}

impl Shortcut {
    const fn new(key: Key, command: bool, shift: ShiftKey) -> Self {
        Self {
            key,
            command,
            shift,
            by_position: false,
        }
    }

    /// `Cmd`（`Ctrl`）と合わせて押すキー
    const fn command(key: Key) -> Self {
        Self::new(key, true, ShiftKey::Off)
    }

    /// `Cmd`（`Ctrl`）と `Shift` と合わせて押すキー
    const fn command_shift(key: Key) -> Self {
        Self::new(key, true, ShiftKey::On)
    }

    /// `Cmd`（`Ctrl`）と合わせて押すキー（`Shift` はどちらでも）
    const fn command_either(key: Key) -> Self {
        Self::new(key, true, ShiftKey::Either)
    }

    /// 修飾キーなしで押すキー
    const fn plain(key: Key) -> Self {
        Self::new(key, false, ShiftKey::Off)
    }

    /// `Shift` と合わせて押すキー
    const fn plain_shift(key: Key) -> Self {
        Self::new(key, false, ShiftKey::On)
    }

    /// `Cmd` なしで押すキー（`Shift` はどちらでも）
    const fn plain_either(key: Key) -> Self {
        Self::new(key, false, ShiftKey::Either)
    }

    /// キーの位置で見る
    const fn by_position(self) -> Self {
        Self {
            by_position: true,
            ..self
        }
    }

    /// 押したキー `p` がこのショートカットか（決めるのはこれ1つ）
    pub(crate) fn matches(self, p: Pressed) -> bool {
        let key = if self.by_position { p.position } else { Some(p.key) };
        key == Some(self.key)
            && self.command == p.command
            && match self.shift {
                ShiftKey::Off => !p.shift,
                ShiftKey::On => p.shift,
                ShiftKey::Either => true,
            }
    }

    /// 窓の中のメニュー（Windows・Linux）とボタンの吹き出しに出す文字（例：`Ctrl+Shift+S`・`Backspace`・`,`・`←`）。Mac の
    /// OS のメニューは、OS が `Cmd` のショートカットを記号で出す
    pub(crate) fn text(self) -> String {
        let mut parts = Vec::new();
        if self.command {
            parts.push(MOD_KEY);
        }
        if self.shift == ShiftKey::On {
            parts.push("Shift");
        }
        parts.push(match self.key {
            Key::ArrowLeft => "←",
            Key::ArrowRight => "→",
            Key::ArrowUp => "↑",
            Key::ArrowDown => "↓",
            Key::Comma | Key::Period | Key::Minus | Key::Equals | Key::Semicolon | Key::Plus => {
                self.key.symbol_or_name()
            }
            key => key.name(),
        });
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

/// メニューの項目1つ。メニューに出さないキーだけの項目（`menu` が `None`）も表に入れる（キーの決まりは、みなこの表から
/// 引く：約束の棚卸し 5616fcc の C の E3）
pub(crate) struct MenuCommand {
    /// 出すメニュー（`None`：メニューに出さないキーだけの項目）
    pub(crate) menu: Option<Menu>,
    /// この項目の前に区切り線を引くか
    pub(crate) separator_before: bool,
    pub(crate) label: &'static str,
    /// メニューに出すショートカット。Mac では、`Cmd` の付くものだけを OS のメニューに付ける（付けると OS が先に
    /// 受け取るので、`Cmd` の付かないキーを付けると入力欄で文字を消したり打ったりできなくなる）
    pub(crate) shortcut: Option<Shortcut>,
    /// メニューに出さないほかのキー（egui のキー処理だけが受け取る）
    pub(crate) aliases: &'static [Shortcut],
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
        let shown = self.shortcut.filter(|s| !(os_menu_has_keys && s.command && self.menu.is_some()));
        shown.into_iter().chain(self.aliases.iter().copied())
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
    !tab.session.selection().is_empty()
}

fn can_paste(tab: &Tab, clipboard: &Clipboard) -> bool {
    let tl = tab.active_timeline();
    clipboard.content.is_some()
        && tl.is_layer_editable_by_id(active_scope_layer_id(&tab.session, tl))
}

/// メニューに出さないキーだけの項目（キーは `keys`）
const fn keys_only(label: &'static str, keys: &'static [Shortcut], repeats: bool, action: UiAction) -> MenuCommand {
    MenuCommand {
        menu: None,
        separator_before: false,
        label,
        shortcut: None,
        aliases: keys,
        repeats,
        enabled: always,
        text_op: None,
        action,
    }
}

/// 道具を選ぶキーの項目（前から `Shift` を見ていなかった：Shift と押しても道具が変わる）
const fn tool_key(label: &'static str, key: Key, tool: Tool) -> MenuCommand {
    MenuCommand {
        menu: None,
        separator_before: false,
        label,
        shortcut: Some(Shortcut::plain_either(key)),
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::Doc(Action::SetTool { tool }),
    }
}

/// 項目の数
pub(crate) const COMMAND_COUNT: usize = 60;

/// メニューの項目の表（メニューバーの順・メニューの中の上からの順）。番号（並びの位置）で項目を指す
pub(crate) static COMMANDS: [MenuCommand; COMMAND_COUNT] = [
    // ファイル
    MenuCommand {
        menu: Some(Menu::File),
        separator_before: false,
        label: "新規作成",
        shortcut: Some(Shortcut::command(Key::N)),
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::NewProject,
    },
    MenuCommand {
        menu: Some(Menu::File),
        separator_before: true,
        label: "開く...",
        shortcut: Some(Shortcut::command(Key::O)),
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::OpenProjectDialog,
    },
    MenuCommand {
        menu: Some(Menu::File),
        separator_before: false,
        label: "保存",
        shortcut: Some(Shortcut::command(Key::S)),
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::SaveProject,
    },
    MenuCommand {
        menu: Some(Menu::File),
        separator_before: false,
        label: "名前を付けて保存...",
        shortcut: Some(Shortcut::command_shift(Key::S)),
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::SaveProjectAsDialog,
    },
    MenuCommand {
        menu: Some(Menu::File),
        separator_before: true,
        label: "音を読み込む（WAV・MP3）…",
        shortcut: None,
        aliases: &[],
        repeats: false,
        enabled: can_import_sound,
        text_op: None,
        action: UiAction::ImportSoundDialog,
    },
    MenuCommand {
        menu: Some(Menu::File),
        separator_before: true,
        label: "動画を書き出し（MP4）…",
        shortcut: None,
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::ExportVideoDialog,
    },
    MenuCommand {
        menu: Some(Menu::File),
        separator_before: true,
        label: "タブを閉じる",
        shortcut: Some(Shortcut::command(Key::W)),
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::CloseActiveTab,
    },
    // アプリ（Windows・Linux では「ファイル」の一番下：`Menu::in_window_bar`）
    MenuCommand {
        menu: Some(Menu::App),
        separator_before: true,
        // Mac の OS のアプリメニューの決まりの名前（前と同じ）
        label: if cfg!(target_os = "macos") {
            "is2d を終了"
        } else {
            "終了"
        },
        shortcut: Some(Shortcut::command(Key::Q)),
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::Quit,
    },
    // 編集
    MenuCommand {
        menu: Some(Menu::Edit),
        separator_before: false,
        label: "元に戻す",
        shortcut: Some(Shortcut::command(Key::Z)),
        aliases: &[],
        repeats: true,
        enabled: can_undo,
        text_op: Some(TextOp::Undo),
        action: UiAction::Doc(Action::Undo),
    },
    MenuCommand {
        menu: Some(Menu::Edit),
        separator_before: false,
        label: "やり直し",
        shortcut: Some(Shortcut::command(Key::Y)),
        aliases: &[Shortcut::command_shift(Key::Z)],
        repeats: true,
        enabled: can_redo,
        text_op: Some(TextOp::Redo),
        action: UiAction::Doc(Action::Redo),
    },
    MenuCommand {
        menu: Some(Menu::Edit),
        separator_before: true,
        label: "カット",
        shortcut: Some(Shortcut::command(Key::X)),
        aliases: &[],
        repeats: false,
        enabled: has_selection,
        text_op: Some(TextOp::Cut),
        action: UiAction::Doc(Action::Cut),
    },
    MenuCommand {
        menu: Some(Menu::Edit),
        separator_before: false,
        label: "コピー",
        shortcut: Some(Shortcut::command(Key::C)),
        aliases: &[],
        repeats: false,
        enabled: has_selection,
        text_op: Some(TextOp::Copy),
        action: UiAction::Doc(Action::Copy),
    },
    MenuCommand {
        menu: Some(Menu::Edit),
        separator_before: false,
        label: "ペースト",
        shortcut: Some(Shortcut::command(Key::V)),
        aliases: &[],
        repeats: false,
        enabled: can_paste,
        text_op: Some(TextOp::Paste),
        action: UiAction::Doc(Action::Paste { in_place: false }),
    },
    MenuCommand {
        menu: Some(Menu::Edit),
        separator_before: false,
        label: "同じ位置にペースト",
        shortcut: Some(Shortcut::command_shift(Key::V)),
        aliases: &[],
        repeats: false,
        enabled: can_paste,
        text_op: Some(TextOp::Paste),
        action: UiAction::Doc(Action::Paste { in_place: true }),
    },
    MenuCommand {
        menu: Some(Menu::Edit),
        separator_before: true,
        label: "選択削除",
        shortcut: Some(Shortcut::plain(Key::Backspace)),
        aliases: &[Shortcut::plain(Key::Delete), Shortcut::plain(Key::G)],
        repeats: false,
        enabled: has_selection,
        text_op: Some(TextOp::Delete),
        action: UiAction::Doc(Action::DeleteSelection),
    },
    // 表示
    MenuCommand {
        menu: Some(Menu::View),
        separator_before: false,
        label: "100%表示",
        shortcut: Some(Shortcut::command(Key::Num1)),
        aliases: &[Shortcut::command(Key::Num0)],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::ResetView,
    },
    MenuCommand {
        menu: Some(Menu::View),
        separator_before: false,
        label: "ウィンドウに合わせる",
        shortcut: Some(Shortcut::command(Key::Num2)),
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::FitStageToWindow,
    },
    MenuCommand {
        menu: Some(Menu::View),
        separator_before: true,
        label: "枠外を隠す",
        shortcut: Some(Shortcut::command(Key::Num3)),
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::ToggleView(ViewSwitch::ClipOutsideStage),
    },
    MenuCommand {
        menu: Some(Menu::View),
        separator_before: false,
        label: "アンカーを表示",
        shortcut: None,
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::ToggleView(ViewSwitch::Anchors),
    },
    MenuCommand {
        menu: Some(Menu::View),
        separator_before: false,
        label: "オニオンスキン",
        shortcut: None,
        aliases: &[],
        repeats: false,
        enabled: always,
        text_op: None,
        action: UiAction::ToggleView(ViewSwitch::OnionSkin),
    },
    // メニューに出さないキーだけの項目
    keys_only(
        "拡大",
        const { &[
            Shortcut::command_either(Key::Equals).by_position(),
            Shortcut::command_either(Key::Semicolon).by_position(),
            Shortcut::command_either(Key::Plus).by_position(),
        ] },
        true,
        UiAction::Zoom(ZoomStep::In),
    ),
    keys_only(
        "縮小",
        const { &[Shortcut::command_either(Key::Minus).by_position()] },
        true,
        UiAction::Zoom(ZoomStep::Out),
    ),
    keys_only("再生 / 一時停止", const { &[Shortcut::plain_either(Key::Enter)] }, false, UiAction::TogglePlayback),
    keys_only("前のフレームへ", const { &[Shortcut::plain_either(Key::Comma)] }, true, UiAction::StepFrame(-1)),
    keys_only("次のフレームへ", const { &[Shortcut::plain_either(Key::Period)] }, true, UiAction::StepFrame(1)),
    keys_only(
        "←",
        const { &[Shortcut::plain(Key::ArrowLeft)] },
        true,
        UiAction::Arrow { dir: ArrowDir::Left, big: false },
    ),
    keys_only(
        "→",
        const { &[Shortcut::plain(Key::ArrowRight)] },
        true,
        UiAction::Arrow { dir: ArrowDir::Right, big: false },
    ),
    keys_only(
        "↑",
        const { &[Shortcut::plain(Key::ArrowUp)] },
        true,
        UiAction::Arrow { dir: ArrowDir::Up, big: false },
    ),
    keys_only(
        "↓",
        const { &[Shortcut::plain(Key::ArrowDown)] },
        true,
        UiAction::Arrow { dir: ArrowDir::Down, big: false },
    ),
    keys_only(
        "Shift+←",
        const { &[Shortcut::plain_shift(Key::ArrowLeft)] },
        true,
        UiAction::Arrow { dir: ArrowDir::Left, big: true },
    ),
    keys_only(
        "Shift+→",
        const { &[Shortcut::plain_shift(Key::ArrowRight)] },
        true,
        UiAction::Arrow { dir: ArrowDir::Right, big: true },
    ),
    keys_only(
        "Shift+↑",
        const { &[Shortcut::plain_shift(Key::ArrowUp)] },
        true,
        UiAction::Arrow { dir: ArrowDir::Up, big: true },
    ),
    keys_only(
        "Shift+↓",
        const { &[Shortcut::plain_shift(Key::ArrowDown)] },
        true,
        UiAction::Arrow { dir: ArrowDir::Down, big: true },
    ),
    keys_only("最初のフレームへ", const { &[Shortcut::plain_either(Key::Home)] }, false, UiAction::GoToFrame(FrameEnd::First)),
    keys_only("最後のフレームへ", const { &[Shortcut::plain_either(Key::End)] }, false, UiAction::GoToFrame(FrameEnd::Last)),
    keys_only("フレームを延長", const { &[Shortcut::plain(Key::F5)] }, true, UiAction::Doc(Action::ExtendFrame)),
    keys_only("フレームを削除", const { &[Shortcut::plain_shift(Key::F5)] }, true, UiAction::Doc(Action::RemoveFrame)),
    keys_only(
        "キーフレームを挿入",
        const { &[Shortcut::plain(Key::F6)] },
        true,
        UiAction::Doc(Action::InsertKeyframe { copy_prev: true }),
    ),
    keys_only("キーフレームを削除", const { &[Shortcut::plain_shift(Key::F6)] }, true, UiAction::Doc(Action::DeleteKeyframe)),
    keys_only(
        "空白キーフレームを挿入",
        const { &[Shortcut::plain_either(Key::F7)] },
        true,
        UiAction::Doc(Action::InsertKeyframe { copy_prev: false }),
    ),
    keys_only("シンボルに変換", const { &[Shortcut::plain_either(Key::F8)] }, false, UiAction::Doc(Action::ConvertToSymbol)),
    keys_only("トゥイーン", const { &[Shortcut::plain_either(Key::T)] }, false, UiAction::Doc(Action::ToggleTween)),
    tool_key("選択ツール", Key::V, Tool::Select),
    tool_key("自由変形ツール", Key::Q, Tool::Transform),
    tool_key("ブラシツール", Key::B, Tool::Brush),
    tool_key("鉛筆ツール", Key::S, Tool::Pencil),
    tool_key("消しゴムツール", Key::E, Tool::Eraser),
    tool_key("バケツツール", Key::K, Tool::Bucket),
    tool_key("インクボトルツール", Key::O, Tool::InkBottle),
    tool_key("スポイトツール", Key::I, Tool::Eyedropper),
    keys_only("Esc", const { &[Shortcut::plain_either(Key::Escape)] }, false, UiAction::Escape),
    keys_only("アンカーを表示", const { &[Shortcut::plain_either(Key::A)] }, false, UiAction::AnchorEdit(None)),
    keys_only(
        "アンカーを追加",
        const { &[Shortcut::plain_either(Key::D)] },
        false,
        UiAction::AnchorEdit(Some(SelectSubMode::AddAnchor)),
    ),
    keys_only(
        "アンカーを削除",
        const { &[Shortcut::plain_either(Key::F)] },
        false,
        UiAction::AnchorEdit(Some(SelectSubMode::DeleteAnchor)),
    ),
    keys_only("グループ化", const { &[Shortcut::command(Key::G)] }, false, UiAction::Doc(Action::Group)),
    keys_only(
        "グループ解除",
        const { &[Shortcut::command_shift(Key::G), Shortcut::command_either(Key::B)] },
        false,
        UiAction::Doc(Action::Ungroup),
    ),
    keys_only(
        "前面へ",
        const { &[Shortcut::command(Key::ArrowUp)] },
        false,
        UiAction::Doc(Action::Arrange { mode: ArrangeMode::Forward }),
    ),
    keys_only(
        "最前面へ",
        const { &[Shortcut::command_shift(Key::ArrowUp)] },
        false,
        UiAction::Doc(Action::Arrange { mode: ArrangeMode::ToFront }),
    ),
    keys_only(
        "背面へ",
        const { &[Shortcut::command(Key::ArrowDown)] },
        false,
        UiAction::Doc(Action::Arrange { mode: ArrangeMode::Backward }),
    ),
    keys_only(
        "最背面へ",
        const { &[Shortcut::command_shift(Key::ArrowDown)] },
        false,
        UiAction::Doc(Action::Arrange { mode: ArrangeMode::ToBack }),
    ),
];

/// `is` に当たる項目のキー（メニューに出すものと出さないもの）を「 / 」で並べた文字（ボタンの吹き出しに出す。キーの決まりは
/// 表の1か所）
pub(crate) fn keys_text(is: impl Fn(&UiAction) -> bool) -> String {
    COMMANDS
        .iter()
        .filter(|c| is(&c.action))
        .flat_map(|c| c.shortcut.into_iter().chain(c.aliases.iter().copied()))
        .map(Shortcut::text)
        .collect::<Vec<_>>()
        .join(" / ")
}

/// 押したキー `pressed` で動く項目の番号。OS のメニューがキーを受け取るとき（`os_menu_has_keys`）は、そのキーは含めない
pub(crate) fn command_for_key(pressed: Pressed, os_menu_has_keys: bool) -> Option<usize> {
    COMMANDS
        .iter()
        .position(|c| c.egui_keys(os_menu_has_keys).any(|k| k.matches(pressed)))
}

/// このコマに押したショートカットと、押しっぱなしの繰り返しか。egui-winit は `Cmd+C`・`Cmd+X`・`Cmd+V` を
/// キーの合図でなくコピー・カット・貼り付けの合図で送るので、それをキーに戻す（`Shift` はそのときの修飾キー）。
/// 捨てられた `Cmd+V`（`swallowed_paste`：`Clipboard::swallowed_paste`）も `Cmd+V` にする
pub(crate) fn pressed_shortcuts(
    input: &egui::InputState,
    swallowed_paste: bool,
) -> Vec<(Pressed, bool)> {
    let clipboard_key = |key| Pressed {
        key,
        position: None,
        command: true,
        shift: input.modifiers.shift,
    };
    let mut pressed: Vec<(Pressed, bool)> = input
        .events
        .iter()
        .filter_map(|ev| match ev {
            egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                repeat,
                modifiers,
            } => Some((
                Pressed {
                    key: *key,
                    position: *physical_key,
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
