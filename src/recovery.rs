use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::SystemTime;

use crate::canvas_tool::Tab;
use crate::floating::Floating;
use crate::project_io::{open_document_in_new_tab, read_project_document, ProjectWorkspace};
use crate::recovery_journal::Journal;
use crate::symbol::{Document, EditPath};

/// 起動時に見つかった未保存作業1件分の一覧ファイルと復旧ファイルの情報
#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub(crate) entry_path: PathBuf,
    pub(crate) recovery_path: PathBuf,
    pub(crate) modified: SystemTime,
}

/// 復旧ファイルの名前の、作品の名前の前に置く印（名前を付けるのは `WriterCore::write`、読むのは `Found::work_name`。
/// 前の版のアプリは `RECOVER_{作品のファイル名}` と書いた）
const RECOVER_MARK: &str = "RECOVER_";

impl Found {
    /// 復元のダイアログに出す作品の名前（復旧ファイルの名前の印の後ろ。拡張子は出さない）
    pub(crate) fn work_name(&self) -> String {
        let stem = self.recovery_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        stem.split_once(RECOVER_MARK).map_or(stem.clone(), |(_, n)| n.to_owned())
    }
}

/// 次回起動時の復元管理状態（復旧ファイルを書くのは裏の書き手：`RecoveryWriter`）
#[derive(Default)]
pub(crate) struct Recovery {
    /// アプリ専用のフォルダ。None なら何もしない（`Is2dApp::default()` とテストの既定では None）
    pub(crate) dir: Option<PathBuf>,
    /// 起動時に見つかって、ダイアログで聞いている最中のもの（一覧のファイルの場所と、復旧ファイルの場所）
    pub(crate) found: Vec<Found>,
}

/// 開いているタブ1つ分の控え（保存したことのない無題の作品も控える：2026-11-27）。文書と浮いた片は共有して持つので、
/// 控えるのは安い。浮いた片を置いた文書は、裏の書き手が書くときに作る（`canvas_tool::committed_document`）
#[derive(Clone)]
pub(crate) struct TabCopy {
    id: u64,
    untitled_number: usize,
    /// 未保存の変更があるか（無ければ、復旧ファイルには「保存した」の印だけを書き足す：`WriterCore::write`）
    dirty: bool,
    /// 保存先（無題なら `None`）
    file: Option<PathBuf>,
    /// 控えたときの文書の世代（控え直すかを決める：`TabCopies::refresh`）
    content_gen: u64,
    doc: Document,
    edit_path: EditPath,
    editing_group_path: Vec<u64>,
    floating: Option<Arc<Floating>>,
}

impl TabCopy {
    /// タブ `tab` の控え
    pub(crate) fn of(tab: &Tab) -> Self {
        Self {
            id: tab.id,
            untitled_number: tab.untitled_number,
            dirty: tab.history().is_dirty(),
            file: tab.file.clone(),
            content_gen: tab.history().content_gen,
            doc: tab.doc().clone(),
            edit_path: tab.session.edit_path().clone(),
            editing_group_path: tab.session.editing_group_path().to_vec(),
            floating: tab.session.shared_floating(),
        }
    }

    /// 浮いた片を置いた文書（置けなければ、片を持ち上げる前の文書：復旧は、何も書かないより前の絵を残す）
    fn document(&self) -> Document {
        crate::canvas_tool::committed_document(
            (self.id, self.untitled_number),
            &self.doc,
            &self.edit_path,
            &self.editing_group_path,
            self.floating.as_deref(),
        )
        .unwrap_or_else(|| self.doc.clone())
    }

    /// 復旧ファイルの名前（保存先の名前か、無題の名前）
    fn name(&self) -> String {
        self.file
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|n| n.to_str())
            .map_or_else(|| format!("無題-{}", self.untitled_number), str::to_owned)
    }
}

/// 開いているタブの控え（画面の毎コマの終わりに、タブの文書が変わったときだけ控え直す：`refresh`）。OS がアプリを終わら
/// せる知らせ（Windows のログアウト・再起動・シャットダウン）は、画面の処理の外から来て、そのときにはタブを読めないので、
/// 書く中身をここに控えておく
#[derive(Clone, Default)]
pub(crate) struct TabCopies(Arc<Mutex<Vec<TabCopy>>>);

impl TabCopies {
    /// タブ `tabs` の今の控えにする（タブとその文書の世代・保存先・未保存かが前と同じなら何もしない）。控え直したら `true`
    pub(crate) fn refresh(&self, tabs: &[Tab]) -> bool {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let now = tabs
            .iter()
            .map(|t| (t.id, t.history().content_gen, t.file.as_deref(), t.untitled_number, t.history().is_dirty()));
        if now.eq(held.iter().map(|u| (u.id, u.content_gen, u.file.as_deref(), u.untitled_number, u.dirty))) {
            return false;
        }
        *held = tabs.iter().map(TabCopy::of).collect();
        true
    }

    /// 今の控え
    pub(crate) fn get(&self) -> Vec<TabCopy> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// 復旧ファイルの裏の書き手：渡された控え（開いているタブの今）を、タブごとの書き足すファイル（`recovery_journal::Journal`：
/// 変わった所だけを書き足す）に書く。書くのは「控えが変わったとき・前の書き込みが終わってから」だけ（書いている間に
/// 変わった分は、終わってから最後の控えだけを書く。決め打ちの間隔は使わない）。保存したタブは「保存した」の印だけを
/// 書き足し（ファイルは消さないので、続きの変更も書き足しで済む。消すと、保存のたびに次の変更で全体を書き直す）、
/// 控えに無くなったタブ（閉じた）の復旧ファイルは消す。書く関数は `WriterCore::write` の1つで、裏のスレッドも、待って書く所（確認なしに終わるとき・画面の
/// 処理で止まったとき・Windows のセッションの終わり：`flush`）も、これを呼ぶ（設計審査 4f8ef46 の D-3。前は終わるとき
/// にだけ書いたので、強制終了・電源断で作業が消えた）
#[derive(Clone)]
pub(crate) struct RecoveryWriter(Arc<WriterShared>);

struct WriterShared {
    /// まだ書いていない控え（番号つき）と、止める合図
    queue: Mutex<WriterQueue>,
    queued: Condvar,
    /// 書く係（書くのは同時に1つだけ）
    core: Mutex<WriterCore>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[derive(Default)]
struct WriterQueue {
    pending: Option<(u64, Vec<TabCopy>)>,
    /// 渡した控えの番号
    submitted: u64,
    stop: bool,
}

/// 書く係：書いた控えの番号と、タブごとの書き足すファイル
struct WriterCore {
    dir: PathBuf,
    /// このアプリ（今のプロセス）のしるし。書くファイルの名前はみなこれで始まり、しるしの錠のファイル（`{しるし}.lock`）を
    /// 終わるまで錠をかけて持つ。起動したときに探す所（`find_leftovers`）は、錠がかかっている（動いているアプリの）
    /// ファイルを見ない（同時に開いた別の is2d の作業を、復元して消さない）
    token: String,
    lock: Option<std::fs::File>,
    written: u64,
    journals: HashMap<u64, Journal>,
}

impl RecoveryWriter {
    /// 復旧の置き場 `dir` に書く書き手を始める
    pub(crate) fn start(dir: PathBuf) -> Self {
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let token = format!("{}_{nanos}", std::process::id());
        let lock = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::File::create(dir.join(format!("{token}.lock"))))
            .ok()
            .filter(|f| f.try_lock().is_ok());
        let shared = Arc::new(WriterShared {
            queue: Mutex::new(WriterQueue::default()),
            queued: Condvar::new(),
            core: Mutex::new(WriterCore { dir, token, lock, written: 0, journals: HashMap::new() }),
            thread: Mutex::new(None),
        });
        let worker = shared.clone();
        let handle = std::thread::Builder::new()
            .name("is2d-recovery".to_owned())
            .spawn(move || run_writer(&worker))
            .ok();
        *shared.thread.lock().unwrap_or_else(PoisonError::into_inner) = handle;
        Self(shared)
    }

    /// 控え `items` に番号を振って、まだ書いていない控えにする（前の控えをまだ書いていなければ、それは書かない）
    fn enqueue(&self, items: Vec<TabCopy>) -> u64 {
        let mut q = self.0.queue.lock().unwrap_or_else(PoisonError::into_inner);
        q.submitted += 1;
        let n = q.submitted;
        q.pending = Some((n, items));
        self.0.queued.notify_all();
        n
    }

    /// 控え `items` を裏で書いてもらう（待たない）
    pub(crate) fn submit(&self, items: Vec<TabCopy>) {
        self.enqueue(items);
    }

    /// 控え `items` を、この場で書き終える（確認なしに終わるとき・画面の処理で止まったとき・Windows のセッションの終わり）。
    /// 裏で書いている途中なら、それが終わってから書く
    pub(crate) fn flush(&self, items: Vec<TabCopy>) {
        let n = self.enqueue(items);
        let job = self.0.queue.lock().unwrap_or_else(PoisonError::into_inner).pending.take();
        let mut core = self.0.core.lock().unwrap_or_else(PoisonError::into_inner);
        // 裏のスレッドが先に取って書いていれば、書き終わっている（`core.written` が進んでいる）
        if let Some((m, items)) = job.filter(|(m, _)| *m == n) {
            core.write(m, &items);
        }
    }

    /// 書き手を止める（いま書いている分を書き終えてから）。`discard` なら、書いた復旧ファイルをみな消す（確認ありで終わる
    /// とき：保存するか捨てるかを決めたあと）。錠のファイルは消す
    pub(crate) fn finish(&self, discard: bool) {
        {
            let mut q = self.0.queue.lock().unwrap_or_else(PoisonError::into_inner);
            q.stop = true;
            self.0.queued.notify_all();
        }
        let handle = self.0.thread.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(h) = handle {
            let _ = h.join();
        }
        let mut core = self.0.core.lock().unwrap_or_else(PoisonError::into_inner);
        if discard {
            core.write(u64::MAX, &[]);
        }
        if core.lock.take().is_some() {
            let _ = std::fs::remove_file(core.dir.join(format!("{}.lock", core.token)));
        }
    }
}

/// 裏の書き手の仕事：控えが来たら書き、止める合図が来たら終わる
fn run_writer(shared: &WriterShared) {
    loop {
        let job = {
            let mut q = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(job) = q.pending.take() {
                    break Some(job);
                }
                if q.stop {
                    break None;
                }
                q = shared.queued.wait(q).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let Some((n, items)) = job else {
            break;
        };
        shared.core.lock().unwrap_or_else(PoisonError::into_inner).write(n, &items);
    }
}

impl WriterCore {
    /// 番号 `n` の控え `items` を書く（書いた控えより古ければ何もしない）。未保存のタブは、タブごとの書き足すファイルに、
    /// 変わった所だけを書き足す。名前が変わった（名前を付けて保存した）タブは、前のファイルを消して新しく作る。保存した
    /// タブは「保存した」の印を書き足す。控えに無いタブ（閉じた）の復旧ファイルは消す。書けなかったタブは忘れて、次の
    /// 控えで作り直す
    fn write(&mut self, n: u64, items: &[TabCopy]) {
        if n <= self.written {
            return;
        }
        self.written = n;
        let gone: Vec<u64> = self.journals.keys().copied().filter(|id| !items.iter().any(|i| i.id == *id)).collect();
        for id in gone {
            if let Some(j) = self.journals.remove(&id) {
                self.remove(id, &j);
            }
        }
        if items.is_empty() || std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        for item in items {
            if !item.dirty {
                if let Some(j) = self.journals.get_mut(&item.id) {
                    if j.mark_saved().is_err() {
                        if let Some(j) = self.journals.remove(&item.id) {
                            self.remove(item.id, &j);
                        }
                    }
                }
                continue;
            }
            let name = format!("{}-{}-{RECOVER_MARK}{}.is2dj", self.token, item.id, item.name());
            let path = self.dir.join(&name);
            let tmp = self.dir.join(format!(".{name}.tmp"));
            let doc = item.document();
            let source = item.file.as_deref();
            match self.journals.get_mut(&item.id) {
                // 同じ名前のファイルに書き足す
                Some(j) if j.path() == path => {
                    if j.write(&tmp, &doc, source).is_err() {
                        self.journals.remove(&item.id);
                    }
                    continue;
                }
                // 名前が変わった：前のファイルを消して、新しく作る
                Some(_) => {
                    if let Some(old) = self.journals.remove(&item.id) {
                        self.remove(item.id, &old);
                    }
                }
                None => {}
            }
            // 一覧を先に書き、それから復旧ファイルを作る（間で止まっても、指す先の無い一覧は起動したときに消える）
            let entry = self.entry_path(item.id);
            if std::fs::write(&entry, path.to_string_lossy().as_bytes()).is_err() {
                continue;
            }
            match Journal::create(&path, &tmp, &doc, source) {
                Ok(j) => {
                    self.journals.insert(item.id, j);
                }
                Err(_) => {
                    let _ = std::fs::remove_file(&entry);
                }
            }
        }
    }

    /// タブ `id` の一覧ファイルの場所
    fn entry_path(&self, id: u64) -> PathBuf {
        self.dir.join(format!("{}-{id}.entry", self.token))
    }

    /// タブ `id` の書き足すファイル `j` と一覧ファイルを消す
    fn remove(&self, id: u64, j: &Journal) {
        let _ = std::fs::remove_file(self.entry_path(id));
        let _ = std::fs::remove_file(j.path());
    }
}

/// 名前 `name` のファイル（一覧・復旧ファイル・一時ファイル・錠のファイル）を書いたアプリが、いま動いているか（名前の頭の
/// しるしの錠に、錠がかかっているか。しるしは `-` も `.` も含まない：`RecoveryWriter::start`）
fn written_by_running_app(dir: &Path, name: &str) -> bool {
    let token = name.trim_start_matches('.').split(['-', '.']).next().unwrap_or_default();
    std::fs::File::open(dir.join(format!("{token}.lock")))
        .is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
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
            // いま動いている別の is2d が書いているファイルは見ない
            if written_by_running_app(dir, name) {
                continue;
            }
            // 錠のかかっていない錠のファイル（終わったアプリの）は消す
            if name.ends_with(".lock") {
                let _ = std::fs::remove_file(&path);
                continue;
            }
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
            // 最後に「保存した」の印がある復旧ファイル（保存したあと、変えずに終わった作品）は、聞かずに消す
            if crate::recovery_journal::is_saved(&rec) {
                let _ = std::fs::remove_file(&rec);
                let _ = std::fs::remove_file(&path);
                continue;
            }
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
            // 書き足すファイル（今の書き方）か、前の版のアプリが書いた `.is2d` の復旧ファイルか（ファイルの頭の印で見る）
            let read = if crate::recovery_journal::is_journal(&item.recovery_path) {
                crate::recovery_journal::read(&item.recovery_path)
            } else {
                read_project_document(&item.recovery_path)
            };
            let res = read.and_then(|mut proj_doc| {
                let src = proj_doc.recovery_source.take();
                open_document_in_new_tab(ws, proj_doc.into_document()?, Some(ctx))?;
                // 保存先のある作品はその保存先で、無題の作品は無題のまま開く（どちらも未保存）
                match src {
                    Some(src) => ws.tab_mut().file = Some(src),
                    None => {
                        let n = ws.alloc_untitled_number();
                        ws.tab_mut().untitled_number = n;
                    }
                }
                ws.tab_mut().mark_unsaved();
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

    /// 見つかった復旧ファイルと一覧をすべて破棄（削除）する
    pub(crate) fn discard_found(&mut self) {
        for item in self.found.drain(..) {
            let _ = std::fs::remove_file(&item.recovery_path);
            let _ = std::fs::remove_file(&item.entry_path);
        }
    }
}
