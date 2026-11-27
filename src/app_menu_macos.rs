use std::cell::RefCell;

use eframe::egui;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSMenu,
    NSMenuItem, NSPasteboard, NSPasteboardTypeString,
};
use objc2_foundation::{NSObject, NSString};

use crate::menu::{ItemState, COMMANDS, COMMAND_COUNT};

/// 項目を押したときに受け取る受け手の中身：押した項目の番号の受け箱と、描き直しを頼む egui
struct TargetIvars {
    /// 押した項目の番号（`menu::COMMANDS` の番号。アプリが毎コマ読む：`OsMenu::take_chosen`）
    chosen: RefCell<Vec<usize>>,
    ctx: egui::Context,
}

define_class!(
    // SAFETY: 親の `NSObject` には、継ぐときの決まりが無い。`MenuTarget` は `Drop` を持たない
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetIvars]
    struct MenuTarget;

    impl MenuTarget {
        // SAFETY: メニューの項目の動き（`action`）の形（受け取るのは押した項目）どおり
        #[unsafe(method(chooseCommand:))]
        fn choose_command(&self, sender: &NSMenuItem) {
            // OS から呼ばれる受け口（砦の中。OS をまたいで止まると強制終了になる。止まったら、その1回を捨てる：設計
            // claude/最後の砦を仕事の入口に置く設計.md）
            let _ = crate::guard::guarded(|| {
                self.ivars().chosen.borrow_mut().push(sender.tag() as usize);
                self.ivars().ctx.request_repaint();
            });
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, ctx: egui::Context) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            chosen: RefCell::default(),
            ctx,
        });
        // SAFETY: `NSObject` の `init` の形どおり
        unsafe { msg_send![super(this), init] }
    }
}

/// OS のメニューバーの「ファイル」「編集」「表示」（メニューの項目の表 `menu::COMMANDS` から作る）
pub(crate) struct OsMenu {
    /// 表の項目（`tag` が表の番号）
    items: Vec<Retained<NSMenuItem>>,
    /// 項目の受け手（項目は受け手を弱く指すので、ここで持つ）
    target: Retained<MenuTarget>,
}

impl OsMenu {
    /// 前のコマから押された項目の番号（押した順）。読んだ分は受け箱から出す
    pub(crate) fn take_chosen(&self) -> Vec<usize> {
        self.target.ivars().chosen.take()
    }

    /// 使えるかどうかとチェックの印を、表の決まりで決めた見せ方 `states`（`Is2dApp::menu_states`）にする
    pub(crate) fn show_states(&self, states: &[ItemState; COMMAND_COUNT]) {
        for item in &self.items {
            let state = states[item.tag() as usize];
            if item.isEnabled() != state.enabled {
                item.setEnabled(state.enabled);
            }
            if let Some(checked) = state.checked {
                let value = if checked {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                };
                if item.state() != value {
                    item.setState(value);
                }
            }
        }
    }
}

/// OS のクリップボードの文字（無ければ空。入力欄へ貼り付けを渡すとき：`Is2dApp::run_os_menu_choices`）
pub(crate) fn pasteboard_text() -> String {
    let pasteboard = NSPasteboard::generalPasteboard();
    // SAFETY: `NSPasteboardTypeString` は AppKit が持つ変わらない文字列
    let text = pasteboard.stringForType(unsafe { NSPasteboardTypeString });
    text.map(|t| t.to_string()).unwrap_or_default()
}

/// macOS のメニューバーに、アプリメニュー（「is2d について」「is2d を隠す」と、表のアプリのメニューの項目「終了」）と、
/// 表から作る「ファイル」「編集」「表示」を設定する。表の項目を押すと、その番号を受け箱に入れて egui `ctx` に描き直しを頼む
pub(crate) fn install(ctx: &egui::Context) -> Option<OsMenu> {
    let mtm = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    let menubar = NSMenu::new(mtm);
    let app_menu_item = NSMenuItem::new(mtm);
    menubar.addItem(&app_menu_item);

    let app_menu = NSMenu::new(mtm);
    let empty_key = NSString::new();

    // SAFETY: `MainThreadMarker` でメインスレッド実行が保証されており、
    // `orderFrontStandardAboutPanel:` / `hide:` は `NSApplication` の標準セレクタであるため安全。
    unsafe {
        let about_item = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str("is2d について"),
            Some(sel!(orderFrontStandardAboutPanel:)),
            &empty_key,
        );
        app_menu.addItem(&about_item);
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        let hide_item = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str("is2d を隠す"),
            Some(sel!(hide:)),
            &NSString::from_str("h"),
        );
        app_menu.addItem(&hide_item);
    }
    // 使えるかどうかは、毎コマ表の決まりで決めて写す（`OsMenu::show_states`）
    app_menu.setAutoenablesItems(false);
    let target = MenuTarget::new(mtm, ctx.clone());
    let mut items = Vec::with_capacity(COMMAND_COUNT);
    add_table_items(mtm, &app_menu, crate::menu::Menu::App, &target, &mut items);
    app_menu_item.setSubmenu(Some(&app_menu));

    for menu in crate::menu::Menu::ALL {
        let submenu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(menu.label()));
        submenu.setAutoenablesItems(false);
        add_table_items(mtm, &submenu, menu, &target, &mut items);
        let menu_item = NSMenuItem::new(mtm);
        menu_item.setSubmenu(Some(&submenu));
        menubar.addItem(&menu_item);
    }

    app.setMainMenu(Some(&menubar));
    Some(OsMenu { items, target })
}

/// 表（`menu::COMMANDS`）のメニュー `menu` の項目を、OS のメニュー `submenu` に足す（押すと受け手 `target` が番号を受け取る）。
/// 足した項目は `items` に入れる（使えるかどうかを毎コマ写す：`OsMenu::show_states`）
fn add_table_items(
    mtm: MainThreadMarker,
    submenu: &NSMenu,
    menu: crate::menu::Menu,
    target: &Retained<MenuTarget>,
    items: &mut Vec<Retained<NSMenuItem>>,
) {
    for (index, command) in COMMANDS.iter().enumerate() {
        if command.menu != Some(menu) {
            continue;
        }
        if command.separator_before {
            submenu.addItem(&NSMenuItem::separatorItem(mtm));
        }
        // `Cmd` の付くショートカットだけを OS のメニューに付ける（`MenuCommand::shortcut`）
        let shortcut = command.shortcut.filter(|s| s.command);
        let key = shortcut.map_or_else(String::new, |s| s.key.name().to_lowercase());
        // SAFETY: `chooseCommand:` は受け手 `MenuTarget` が持つ、押した項目を受け取るメソッド。受け手は
        // `OsMenu` が持ち続けるので、項目が指す間は無くならない
        let item = unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(command.label),
                Some(sel!(chooseCommand:)),
                &NSString::from_str(&key),
            );
            item.setTarget(Some(AsRef::<AnyObject>::as_ref(&**target)));
            item
        };
        if let Some(shortcut) = shortcut {
            let mut mask = NSEventModifierFlags::Command;
            if shortcut.shift == crate::menu::ShiftKey::On {
                mask |= NSEventModifierFlags::Shift;
            }
            item.setKeyEquivalentModifierMask(mask);
        }
        item.setTag(index as isize);
        submenu.addItem(&item);
        items.push(item);
    }
}
