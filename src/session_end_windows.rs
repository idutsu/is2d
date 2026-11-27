//! Windows のセッションの終わり（ログアウト・再起動・シャットダウン・Windows Update の再起動）を受け取り、アプリが
//! 確認なしに止められる前に、未保存のタブの復旧ファイルを書く（Mac・Linux のアプリの終わり `on_exit` と同じ書き方：
//! 裏の書き手 `RecoveryWriter::flush`）。Windows はセッションを終わらせるとき、窓に `WM_QUERYENDSESSION`（終わってよいか）と
//! `WM_ENDSESSION`（終わるか・やめたか）を送るが、winit はどちらも受け取らないので、窓の受け口（ウィンドウプロシージャ）を
//! 包んで受ける。書いている間は、Windows に「保存していない作業を書いています」と伝えて待ってもらう
//! （`ShutdownBlockReasonCreate`）。セッションを終わらせるのをやめたら（ほかのアプリが止めたなど）、次の終わりに備えるだけ
//! （復旧ファイルは裏の書き手が作品の変わるたびに書き直しているので、消さない）。OS の機能を呼ぶ `unsafe` は、このファイルの中だけ

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{WM_ENDSESSION, WM_QUERYENDSESSION};

use crate::recovery::{RecoveryWriter, TabCopies};

/// 包んだ受け口が持つもの（アプリが終わるまで生きる）
struct SessionEnd {
    writer: RecoveryWriter,
    unsaved: TabCopies,
    /// このセッションの終わりで、もう書いたか
    written: AtomicBool,
}

/// 窓 `hwnd` の受け口を包んで、セッションの終わりに、控え `unsaved` を裏の書き手 `writer` に書き終えてもらうようにする。
/// 包めなければ何もしない（裏の書き手は、作品の変わるたびに書いている）
pub(crate) fn install(hwnd: isize, writer: RecoveryWriter, unsaved: TabCopies) {
    let state = Box::new(SessionEnd {
        writer,
        unsaved,
        written: AtomicBool::new(false),
    });
    // 受け口が使う間（アプリが終わるまで）生かすので、手放さない
    let data = Box::into_raw(state) as usize;
    // SAFETY: `hwnd` はこのアプリの窓（winit が作り、アプリが終わるまで生きる）。`data` は上で手放さないことにした
    // `SessionEnd` を指し、受け口 `subclass_proc` だけが、窓の受け口と同じスレッドで読む
    let ok = unsafe { SetWindowSubclass(HWND(hwnd as *mut _), Some(subclass_proc), 1, data) };
    if !ok.as_bool() {
        // 包めなかった：手放さないことにした状態を返す
        // SAFETY: `data` は上で `Box::into_raw` したもので、受け口には渡っていない
        drop(unsafe { Box::from_raw(data as *mut SessionEnd) });
    }
}

/// 包んだ窓の受け口：セッションの終わりの2つの知らせだけを受け、ほかは元の受け口へ渡す
unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: `data` は `install` で手放さないことにした `SessionEnd`（アプリが終わるまで生きる）
    let state = unsafe { &*(data as *const SessionEnd) };
    match msg {
        // 終わってよいか：書いてから、よいと答える
        WM_QUERYENDSESSION => {
            write_once(hwnd, state);
            LRESULT(1)
        }
        // 終わる（wparam が 0 でない）なら、まだ書いていなければ書く。やめたなら、次の終わりに備える
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                write_once(hwnd, state);
            } else {
                state.written.store(false, Ordering::Relaxed);
            }
            LRESULT(0)
        }
        // SAFETY: 受け取らない知らせは、元の受け口へそのまま渡す
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}

/// このセッションの終わりで、まだ書いていなければ、控えの復旧ファイルを書く。書いている間は、Windows に待ってもらう
fn write_once(hwnd: HWND, state: &SessionEnd) {
    if state.written.swap(true, Ordering::Relaxed) {
        return;
    }
    // SAFETY: `hwnd` はこのアプリの窓。理由の文字列は静的な UTF-16
    let _ = unsafe { ShutdownBlockReasonCreate(hwnd, w!("保存していない作業を書いています")) };
    state.writer.flush(state.unsaved.get());
    // SAFETY: 同じ窓の、上で作った理由を消す
    let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
}
