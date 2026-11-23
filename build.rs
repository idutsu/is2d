//! Windows 向けにビルドするときだけ、Media Foundation の DLL を「使うときに初めて読み込む」（遅延読み込み）ようにする。
//!
//! ふつうに結び付けると、DLL が無い Windows（N エディションなどで「メディア機能パック」が入っていないもの）では、アプリが
//! 起動すらしない。遅延読み込みにすれば、起動はでき、動画の書き出しを始めるときに DLL があるかを確かめて、無ければ書き出しの
//! 画面に文言を出せる（`video_export_windows`）。確かめる DLL は、ここで決めた並びを `IS2D_DELAY_LOADED_DLLS` で渡す
//! （並びを決める所は1つ）。

/// 遅延読み込みにする DLL：書き出しで呼ぶ Media Foundation の関数の入っているもの（`MFStartup` などは mfplat.dll、
/// `MFCreateSinkWriterFromURL` は mfreadwrite.dll）
const DELAY_LOADED_DLLS: [&str; 2] = ["mfplat.dll", "mfreadwrite.dll"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // 遅延読み込みは MSVC のリンカーの機能（/DELAYLOAD と delayimp.lib）
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        panic!("Windows 向けには MSVC のツールチェーン（x86_64-pc-windows-msvc など）でビルドしてください");
    }
    for dll in DELAY_LOADED_DLLS {
        println!("cargo:rustc-link-arg=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg=delayimp.lib");
    println!(
        "cargo:rustc-env=IS2D_DELAY_LOADED_DLLS={}",
        DELAY_LOADED_DLLS.join(";")
    );
}
