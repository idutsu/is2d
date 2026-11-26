use std::path::PathBuf;
use std::sync::Arc;
#[cfg(any(target_os = "windows", test))]
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use crate::canvas_tool::Tab;
use crate::floating::Floating;
use crate::symbol::{Document, EditPath};
use crate::project_io::{
    open_document_in_new_tab, read_project_document, write_project_file_atomically,
    ProjectDocument, ProjectWorkspace,
};

/// 起動時に見つかった未保存作業1件分の一覧ファイルと復旧ファイルの情報
#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub(crate) entry_path: PathBuf,
    pub(crate) recovery_path: PathBuf,
    pub(crate) modified: SystemTime,
}

/// 確認なし終了時の復旧ファイル書き出し・次回起動時の復元管理状態
#[derive(Default)]
pub(crate) struct Recovery {
    /// アプリ専用のフォルダ。None なら何もしない（`Is2dApp::default()` とテストの既定では None）
    pub(crate) dir: Option<PathBuf>,
    /// 起動時に見つかって、ダイアログで聞いている最中のもの（一覧のファイルの場所と、復旧ファイルの場所）
    pub(crate) found: Vec<Found>,
}

/// 確認なしに終わるときに書く、未保存のタブ1つ分の控え（保存したことがあり、未保存の変更があるタブ）。文書と浮いた片は
/// 共有して持つので、控えるのは安い。浮いた片を置いた文書は、書くときに作る（`canvas_tool::committed_document`）
#[derive(Clone)]
pub(crate) struct UnsavedTab {
    id: u64,
    untitled_number: usize,
    file: PathBuf,
    /// 控えたときの文書の世代（控え直すかを決める：`UnsavedTabs::refresh`）
    #[cfg(any(target_os = "windows", test))]
    content_gen: u64,
    doc: Document,
    edit_path: EditPath,
    editing_group_path: Vec<u64>,
    floating: Option<Arc<Floating>>,
}

impl UnsavedTab {
    /// タブ `tab` の控え（保存したことがない・未保存の変更が無いタブは `None`）
    pub(crate) fn of(tab: &Tab) -> Option<Self> {
        let file = tab.file.clone().filter(|_| tab.history.is_dirty())?;
        Some(Self {
            id: tab.id,
            untitled_number: tab.untitled_number,
            file,
            #[cfg(any(target_os = "windows", test))]
            content_gen: tab.history.content_gen,
            doc: tab.doc.clone(),
            edit_path: tab.session.edit_path().clone(),
            editing_group_path: tab.session.editing_group_path().to_vec(),
            floating: tab.session.shared_floating(),
        })
    }
}

/// 確認なしに終わるときに書く、未保存のタブの控え（画面の毎コマの終わりに、タブの文書が変わったときだけ控え直す：
/// `refresh`）。OS がアプリを終わらせる知らせ（Windows のログアウト・再起動・シャットダウン）は、画面の処理の外から来て、
/// そのときにはタブを読めないので、書く中身をここに控えておく（Windows で使う。テストでは Mac でも同じものを確かめる）
#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Default)]
pub(crate) struct UnsavedTabs(Arc<Mutex<Vec<UnsavedTab>>>);

#[cfg(any(target_os = "windows", test))]
impl UnsavedTabs {
    /// タブ `tabs` の今の控えにする（控えるタブとその文書の世代が前と同じなら何もしない）
    pub(crate) fn refresh(&self, tabs: &[Tab]) {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let now = tabs
            .iter()
            .filter(|t| t.history.is_dirty() && t.file.is_some())
            .map(|t| (t.id, t.history.content_gen, t.file.as_deref()));
        if !now.eq(held.iter().map(|u| (u.id, u.content_gen, Some(u.file.as_path())))) {
            *held = tabs.iter().filter_map(UnsavedTab::of).collect();
        }
    }

    /// 今の控え
    pub(crate) fn get(&self) -> Vec<UnsavedTab> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// アプリ専用の復旧フォルダの場所を返す（環境変数が無ければ `None`）
pub(crate) fn default_recovery_dir() -> Option<PathBuf> {
    let env_dir = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    #[cfg(target_os = "macos")]
    let base = env_dir("HOME")?.join("Library/Application Support");
    #[cfg(target_os = "windows")]
    let base = env_dir("APPDATA")?;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base =
        env_dir("XDG_DATA_HOME").or_else(|| env_dir("HOME").map(|h| h.join(".local/share")))?;
    Some(base.join("is2d").join("recovery"))
}

impl Recovery {
    /// 確認なしにアプリが終了するとき、保存したことのある未保存タブの復旧ファイルと一覧を書き出す
    pub(crate) fn write_unsaved_tabs(&self, tabs: &[Tab]) {
        let items: Vec<UnsavedTab> = tabs.iter().filter_map(UnsavedTab::of).collect();
        self.write_unsaved(&items);
    }

    /// 未保存のタブの控え `items` の復旧ファイルと一覧を書き出す（書き方はこれ1つ：アプリの終わり `on_exit` と、OS の
    /// セッションの終わり `session_end_windows`）
    pub(crate) fn write_unsaved(&self, items: &[UnsavedTab]) {
        let Some(dir) = &self.dir else {
            return;
        };
        if items.is_empty() || std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let pid = std::process::id();
        for item in items {
            let Some((parent, name)) = Some(item.file.as_path())
                .and_then(|p| Some((p.parent()?, p.file_name()?.to_str()?)))
            else {
                continue;
            };
            // 浮いた片を置けなければ、片を持ち上げる前の文書を書く（復旧は、何も書かないより前の絵を残す）
            let doc = crate::canvas_tool::committed_document(
                (item.id, item.untitled_number),
                &item.doc,
                &item.edit_path,
                &item.editing_group_path,
                item.floating.as_deref(),
            )
            .unwrap_or_else(|| item.doc.clone());
            let mut proj_doc = ProjectDocument::of(&doc);
            proj_doc.recovery_source = Some(item.file.clone());

            let entry_path = dir.join(format!("{pid}-{}.entry", item.id));
            let rec_name = format!("RECOVER_{name}");
            let primary_path = parent.join(&rec_name);
            let fallback_path = dir.join(format!("{pid}-{}-{rec_name}", item.id));

            if std::fs::write(&entry_path, primary_path.to_string_lossy().as_bytes()).is_err() {
                continue;
            }
            if write_project_file_atomically(&primary_path, &proj_doc).is_err()
                && std::fs::write(&entry_path, fallback_path.to_string_lossy().as_bytes()).is_ok()
            {
                let _ = write_project_file_atomically(&fallback_path, &proj_doc);
            }
        }
    }

    /// 起動時にアプリ専用のフォルダから一覧ファイル（`*.entry`）を読み込み、復元対象を `found` に集める
    pub(crate) fn find_leftovers(&mut self) {
        self.found.clear();
        let Some(dir) = &self.dir else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') && name.ends_with(".tmp") {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if !name.ends_with(".entry") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                let _ = std::fs::remove_file(&path);
                continue;
            };
            let rec = PathBuf::from(content.trim());
            if let Some(meta) = std::fs::metadata(&rec).ok().filter(|m| m.is_file()) {
                self.found.push(Found {
                    entry_path: path,
                    recovery_path: rec,
                    modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                });
            } else if rec.parent().is_some_and(|p| p.is_dir()) {
                let _ = std::fs::remove_file(&path);
            }
        }
        self.found.sort_by(|a, b| {
            a.modified
                .cmp(&b.modified)
                .then_with(|| a.entry_path.cmp(&b.entry_path))
        });
    }

    /// 見つかった復旧ファイル（`take_found` で取り出した一覧 `found`）を古い順に未保存タブとして開き、全部開き終えてから
    /// 復旧ファイルと一覧を消す。係（`Recovery`）は取り出さないので、開いている途中で止まっても、落ちる前の控え書きは
    /// 置き場を知っていて、開いたタブを書ける（前は係を丸ごと取り出して開き、1件開けるごとに復旧ファイルを消したので、
    /// 途中で止まると開いた作業が消えた：約束の棚卸し 5616fcc の C の C5）
    pub(crate) fn restore_found(found: Vec<Found>, ws: &mut impl ProjectWorkspace, ctx: &eframe::egui::Context) {
        let mut restored = 0usize;
        let mut broken_paths = Vec::new();
        let mut opened = Vec::new();

        for item in found {
            let res = read_project_document(&item.recovery_path).and_then(|mut proj_doc| {
                let src = proj_doc
                    .recovery_source
                    .take()
                    .ok_or_else(|| "復旧ファイルに元のファイル場所がありません".to_owned())?;
                open_document_in_new_tab(ws, proj_doc.into_document()?, Some(ctx))?;
                ws.tab_mut().file = Some(src);
                ws.tab_mut().history.mark_unsaved();
                Ok(())
            });

            match res {
                Ok(()) => {
                    opened.push(item);
                    restored += 1;
                }
                Err(_) => {
                    let broken_path = item.recovery_path.with_extension("is2d.broken");
                    let _ = std::fs::rename(&item.recovery_path, &broken_path);
                    let _ = std::fs::remove_file(&item.entry_path);
                    broken_paths.push(broken_path.display().to_string());
                }
            }
        }

        for item in opened {
            let _ = std::fs::remove_file(&item.recovery_path);
            let _ = std::fs::remove_file(&item.entry_path);
        }

        if !broken_paths.is_empty() {
            ws.notify_error(format!(
                "一部の復旧ファイルを読み込めませんでした（{}）",
                broken_paths.join("、")
            ));
        } else if restored > 0 {
            ws.notify_info(format!(
                "前回保存されなかった作業を {restored} 件復元しました"
            ));
        }
    }

    /// このアプリ（今のプロセス）が書いた復旧ファイルと一覧を消す（OS がセッションを終わらせるのをやめたとき：書いた
    /// ものが残ると、次の起動で、もう要らない復元を聞いてしまう）
    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn discard_written(&self) {
        let Some(dir) = &self.dir else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let own = format!("{}-", std::process::id());
        for path in entries.flatten().map(|e| e.path()) {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with(&own) && name.ends_with(".entry") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let _ = std::fs::remove_file(PathBuf::from(content.trim()));
                }
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// 見つかった復旧ファイルと一覧をすべて破棄（削除）する
    pub(crate) fn discard_found(&mut self) {
        for item in self.found.drain(..) {
            let _ = std::fs::remove_file(&item.recovery_path);
            let _ = std::fs::remove_file(&item.entry_path);
        }
    }
}
