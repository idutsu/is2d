use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::NSString;

/// macOS のメニューバーにアプリメニュー（「is2d について」「is2d を隠す」「is2d を終了」）を設定する
pub(crate) fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let menubar = NSMenu::new(mtm);
    let app_menu_item = NSMenuItem::new(mtm);
    menubar.addItem(&app_menu_item);

    let app_menu = NSMenu::new(mtm);
    let empty_key = NSString::new();

    // SAFETY: `MainThreadMarker` でメインスレッド実行が保証されており、
    // `orderFrontStandardAboutPanel:` / `hide:` / `performClose:` は `NSApplication`・`NSWindow` の標準セレクタであり、
    // ターゲットに設定する `window` も `app.windows()` から取得した有効な `NSWindow` インスタンスであるため安全。
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
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        let quit_item = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str("is2d を終了"),
            Some(sel!(performClose:)),
            &NSString::from_str("q"),
        );
        if let Some(window) = app.windows().firstObject() {
            quit_item.setTarget(Some(&window));
        }
        app_menu.addItem(&quit_item);
    }

    app_menu_item.setSubmenu(Some(&app_menu));
    app.setMainMenu(Some(&menubar));
}
