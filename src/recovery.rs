use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui;

use crate::canvas_tool::Tab;
use crate::project_io::{
    open_document_in_new_tab, read_project_document, update_window_title,
    write_project_file_atomically, ProjectDocument, ProjectWorkspace,
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
    /// 確認なしにアプリが終了するとき、未保存のタブすべての復旧ファイルと一覧を書き出す
    pub(crate) fn write_unsaved_tabs(&self, tabs: &[Tab]) {
        let Some(dir) = &self.dir else {
            return;
        };
        if !tabs.iter().any(|t| t.history.is_dirty()) || std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let pid = std::process::id();
        for tab in tabs.iter().filter(|t| t.history.is_dirty()) {
            let mut proj_doc = ProjectDocument::from_app(tab);
            proj_doc.recovery_source = tab.file.clone();
            proj_doc.remove_non_finite();

            let entry_path = dir.join(format!("{pid}-{}.entry", tab.id));
            let untitled_name = format!("RECOVER_無題-{pid}-{}.is2d", tab.id);
            let (primary_path, fallback_path) = match tab
                .file
                .as_deref()
                .and_then(|p| Some((p.parent()?, p.file_name()?.to_str()?)))
            {
                Some((parent, name)) => {
                    let rec_name = format!("RECOVER_{name}");
                    (parent.join(&rec_name), Some(dir.join(rec_name)))
                }
                None => (dir.join(untitled_name), None),
            };

            if std::fs::write(&entry_path, primary_path.to_string_lossy().as_bytes()).is_err() {
                continue;
            }
            if write_project_file_atomically(&primary_path, &proj_doc).is_err() {
                if let Some(fallback) = fallback_path {
                    if std::fs::write(&entry_path, fallback.to_string_lossy().as_bytes()).is_ok() {
                        let _ = write_project_file_atomically(&fallback, &proj_doc);
                    }
                }
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
            let target = std::fs::read_to_string(&path)
                .ok()
                .map(|s| PathBuf::from(s.trim()))
                .and_then(|rec| {
                    let meta = std::fs::metadata(&rec).ok().filter(|m| m.is_file())?;
                    Some((rec, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
                });
            match target {
                Some((recovery_path, modified)) => self.found.push(Found {
                    entry_path: path,
                    recovery_path,
                    modified,
                }),
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        self.found.sort_by(|a, b| {
            a.modified
                .cmp(&b.modified)
                .then_with(|| a.entry_path.cmp(&b.entry_path))
        });
    }

    /// 見つかった復旧ファイルを古い順に未保存タブとして開き、復旧ファイルと一覧を消す
    pub(crate) fn restore_found(
        &mut self,
        ws: &mut impl ProjectWorkspace,
        ctx: Option<&egui::Context>,
    ) {
        let mut restored = 0usize;
        let mut broken_paths = Vec::new();

        for item in self.found.drain(..) {
            let res = read_project_document(&item.recovery_path).and_then(|proj_doc| {
                let source = proj_doc.recovery_source.clone();
                open_document_in_new_tab(ws, proj_doc.into_document())?;
                if let Some(src) = source {
                    ws.tab_mut().file = Some(src);
                } else {
                    ws.tab_mut().untitled_number = ws.alloc_untitled_number();
                    ws.tab_mut().file = None;
                }
                ws.tab_mut().history.mark_unsaved();
                Ok(())
            });

            match res {
                Ok(()) => {
                    let _ = std::fs::remove_file(&item.recovery_path);
                    let _ = std::fs::remove_file(&item.entry_path);
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

        if let Some(c) = ctx {
            update_window_title(ws, c);
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

    /// 見つかった復旧ファイルと一覧をすべて破棄（削除）する
    pub(crate) fn discard_found(&mut self) {
        for item in self.found.drain(..) {
            let _ = std::fs::remove_file(&item.recovery_path);
            let _ = std::fs::remove_file(&item.entry_path);
        }
    }
}
