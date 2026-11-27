use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::SystemTime;

use crate::canvas_tool::Tab;
use crate::floating::Floating;
use crate::project_io::{open_document_in_new_tab, ProjectWorkspace};
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
            id: tab.id(),
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
            .map(|t| (t.id(), t.history().content_gen, t.file.as_deref(), t.untitled_number, t.history().is_dirty()));
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
    /// 書けなかったこと（どの控えか・わけ）の受け箱（書き手の失敗はみなここ1つ：内部の不具合で止まった・入出力の失敗・
    /// 始めるときの置き場・錠・スレッド。画面が毎コマ取り出して知らせる：`take_failures`。書いている間も待たずに取り出せる
    /// よう、書く係とは別に持つ：設計審査 b7208d9 の 1.）
    failed: Mutex<Vec<WriteFailure>>,
    thread: Mutex<Option<std::thread::JoinHandle<Option<()>>>>,
}

/// 書けなかったこと：どの控えか（タブの名前。置き場・錠・スレッドなら `None`）と、わけ
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WriteFailure {
    pub(crate) tab: Option<String>,
    pub(crate) why: String,
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
    journals: HashMap<u64, TabJournal>,
    /// 知らせた失敗（控えごと：タブの番号、置き場は `None`）のわけ。同じわけは、書けるようになるまで1回だけ知らせる
    reported: HashMap<Option<u64>, String>,
}

/// タブ1つの書き足すファイルと、最後に書いたときのタブ（中身の世代・保存先）。同じなら書かない：1回の操作で書くのは、
/// 触ったタブの変わった所だけ（前はどれか1つのタブが変わると、変わっていない未保存のタブもみな書き足し、浮いた片を
/// 置いたままのタブは、ほかのタブを触るたびに置いた文書を作り直して書いた：全体の点検 2026-11-27）
struct TabJournal {
    journal: Journal,
    content_gen: u64,
    file: Option<PathBuf>,
    /// 書き足しに失敗した（または書いている途中で止まった）：書き手の覚えた「書いたもの」がファイルと合わないかもしれない
    /// ので、次は全体を書き直す（一時ファイルから置き換えるので、書き終えるまで前の控えが残る）
    rewrite: bool,
}

impl RecoveryWriter {
    /// 復旧の置き場 `dir` に書く書き手を始める
    pub(crate) fn start(dir: PathBuf) -> Self {
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let token = format!("{}_{nanos}", std::process::id());
        let mut failed = Vec::new();
        // 錠が無いと、同時に動く別の is2d が、この控えを残り物と取り違える（知らせる）
        let lock = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::File::create(dir.join(format!("{token}.lock"))))
            .and_then(|f| f.try_lock().map(|()| f).map_err(std::io::Error::other))
            .map_err(|e| failed.push(WriteFailure { tab: None, why: format!("置き場の錠を作れません：{e}") }))
            .ok();
        let shared = Arc::new(WriterShared {
            queue: Mutex::new(WriterQueue::default()),
            queued: Condvar::new(),
            core: Mutex::new(WriterCore { dir, token, lock, written: 0, journals: HashMap::new(), reported: HashMap::new() }),
            failed: Mutex::new(Vec::new()),
            thread: Mutex::new(None),
        });
        let worker = shared.clone();
        let handle = crate::vector_engine::spawn_guarded("is2d-recovery", move || run_writer(&worker))
            .map_err(|e| failed.push(WriteFailure { tab: None, why: format!("裏で控えを書く係を始められません：{e}") }))
            .ok();
        *shared.thread.lock().unwrap_or_else(PoisonError::into_inner) = handle;
        shared.note_failures(failed);
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
            let failed = core.write(m, &items);
            self.0.note_failures(failed);
        }
    }

    /// 書けなかったことを取り出す（画面が毎コマ取り出して知らせる）
    pub(crate) fn take_failures(&self) -> Vec<WriteFailure> {
        std::mem::take(&mut self.0.failed.lock().unwrap_or_else(PoisonError::into_inner))
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
            let failed = core.write(u64::MAX, &[]);
            self.0.note_failures(failed);
        }
        if core.lock.take().is_some() {
            let _ = std::fs::remove_file(core.dir.join(format!("{}.lock", core.token)));
        }
    }
}

impl WriterShared {
    /// 書けなかったこと `failed` を、画面が取り出す所に足す
    fn note_failures(&self, failed: Vec<WriteFailure>) {
        if !failed.is_empty() {
            self.failed.lock().unwrap_or_else(PoisonError::into_inner).extend(failed);
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
        let failed = shared.core.lock().unwrap_or_else(PoisonError::into_inner).write(n, &items);
        shared.note_failures(failed);
    }
}

impl WriterCore {
    /// 番号 `n` の控え `items` を書く（書いた控えより古ければ何もしない）。未保存のタブは、タブごとの書き足すファイルに、
    /// 変わった所だけを書き足す。名前が変わった（名前を付けて保存した）タブは、新しい名前で全体を書き終えてから、一覧の札を
    /// 替えて前のファイルを消す。保存したタブは「保存した」の印を書き足す。控えに無いタブ（閉じた）の復旧ファイルは消す。
    /// 書けなかったら、前に書けた控えと札はそのまま残し、わけを返す（同じ控えの同じわけは、書けるようになるまで1回だけ：
    /// 設計審査 b7208d9 の 1.。前は書けなかったことを黙って捨て、前の控えを先に消すことがあった）
    fn write(&mut self, n: u64, items: &[TabCopy]) -> Vec<WriteFailure> {
        let mut failed = Vec::new();
        if n <= self.written {
            return failed;
        }
        self.written = n;
        let gone: Vec<u64> = self.journals.keys().copied().filter(|id| !items.iter().any(|i| i.id == *id)).collect();
        for id in gone {
            if let Some(j) = self.journals.remove(&id) {
                self.remove(id, &j.journal);
            }
            self.reported.remove(&Some(id));
        }
        if items.is_empty() {
            return failed;
        }
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            self.report(None, Err(format!("置き場を作れません：{e}")), &mut failed);
            return failed;
        }
        self.report(None, Ok(()), &mut failed);
        for item in items {
            // タブ1つ分を砦の中で書く（途中で止まったら、そのタブだけを飛ばし（前に書けた控えは残る。次は全体を書き直す）、
            // ほかのタブは続けて書き、止まったことは画面に知らせる：設計審査 392e9af の D-1）
            let res = crate::vector_engine::guarded(|| self.write_tab(item)).unwrap_or_else(|| {
                if let Some(j) = self.journals.get_mut(&item.id) {
                    j.rewrite = true;
                }
                Err("内部の不具合で止まりました".to_owned())
            });
            self.report(Some(item), res, &mut failed);
        }
        failed
    }

    /// 控え（タブ `item`。置き場なら `None`）を書いた結果 `res` を覚え、書けなかったわけが前に知らせたものと違えば `failed`
    /// に足す（知らせる決まりはここ1つ）。書けたら、覚えたわけを忘れる
    fn report(&mut self, item: Option<&TabCopy>, res: Result<(), String>, failed: &mut Vec<WriteFailure>) {
        let key = item.map(|i| i.id);
        match res {
            Ok(()) => {
                self.reported.remove(&key);
            }
            Err(why) => {
                if self.reported.get(&key) != Some(&why) {
                    failed.push(WriteFailure { tab: item.map(TabCopy::name), why: why.clone() });
                    self.reported.insert(key, why);
                }
            }
        }
    }

    /// タブ `item` を書く（`write` のタブ1つ分）。前に書けた控えと一覧の札は、新しい控えを書き終えるまで消さない
    fn write_tab(&mut self, item: &TabCopy) -> Result<(), String> {
        let failed = |e: std::io::Error| e.to_string();
        if !item.dirty {
            // 保存した：印を書き足す。書けなければ控えを消す（作品は保存してあるので、控えが無くても作業は消えない）
            if let Some(j) = self.journals.get_mut(&item.id) {
                if j.journal.mark_saved().is_err() {
                    if let Some(j) = self.journals.remove(&item.id) {
                        self.remove(item.id, &j.journal);
                    }
                }
            }
            return Ok(());
        }
        let name = format!("{}-{}-{RECOVER_MARK}{}.is2dj", self.token, item.id, item.name());
        let path = self.dir.join(&name);
        let source = item.file.as_deref();
        match self.journals.get_mut(&item.id) {
            // 前に書いたときから変わっていない（中身の世代も保存先も同じ）：書かない
            Some(j) if j.journal.path() == path && !j.rewrite && j.content_gen == item.content_gen && j.file == item.file => {
                Ok(())
            }
            // 同じ名前のファイルに、変わった所を書き足す（前に失敗していれば全体を書き直す）
            Some(j) if j.journal.path() == path => {
                let doc = item.document();
                let res = if j.rewrite {
                    Journal::create(&path, &doc, source).map(|journal| j.journal = journal)
                } else {
                    j.journal.write(&doc, source)
                };
                j.rewrite = res.is_err();
                res.map_err(failed)?;
                j.content_gen = item.content_gen;
                j.file.clone_from(&item.file);
                Ok(())
            }
            // 名前が変わった：新しい名前で全体を書き終えてから、札を替え、前のファイルを消す
            Some(_) => {
                let journal = Journal::create(&path, &item.document(), source).map_err(failed)?;
                if let Err(e) = self.write_entry(item.id, &path) {
                    let _ = std::fs::remove_file(&path);
                    return Err(failed(e));
                }
                let old = self.journals.insert(
                    item.id,
                    TabJournal { journal, content_gen: item.content_gen, file: item.file.clone(), rewrite: false },
                );
                if let Some(old) = old {
                    let _ = std::fs::remove_file(old.journal.path());
                }
                Ok(())
            }
            // 初めて書く：札を先に書き、それから控えを作る（間で止まっても、指す先の無い札は起動したときに消える）
            None => {
                self.write_entry(item.id, &path).map_err(failed)?;
                match Journal::create(&path, &item.document(), source) {
                    Ok(journal) => {
                        self.journals.insert(
                            item.id,
                            TabJournal { journal, content_gen: item.content_gen, file: item.file.clone(), rewrite: false },
                        );
                        Ok(())
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(self.entry_path(item.id));
                        Err(failed(e))
                    }
                }
            }
        }
    }

    /// タブ `id` の一覧の札を、控え `path` を指すように書く（書いて置き換える：`write_atomically`）
    fn write_entry(&self, id: u64, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        crate::save_format::write_atomically(&self.entry_path(id), |mut f| {
            f.write_all(path.to_string_lossy().as_bytes())?;
            Ok(f)
        })
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
        // 札が指す控えと、置き場の控え（動いているアプリのものを除く）。指す札の無い控え（名前が変わったタブの新しい控えを
        // 書き終えて、札を替える前に落ちた）は消す（札の指す前の控えが残っているので、作業は消えない：設計審査 b7208d9 の 1.）
        let mut referenced = HashSet::new();
        let mut journals = Vec::new();
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
            if name.ends_with(".is2dj") {
                journals.push(path);
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
            referenced.insert(rec.clone());
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
        for j in journals.into_iter().filter(|j| !referenced.contains(j)) {
            let _ = std::fs::remove_file(j);
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
        let mut trimmed_works = Vec::new();

        for item in found {
            // 復旧ファイルは、作品ファイルと同じ1つの読み手で読む（`save_format::read_file`。頭の版が違えば読まずに断る）
            let read = crate::save_format::read_file(&item.recovery_path).map(|(_, doc)| doc);
            let mut trimmed = false;
            let res = read.and_then(|mut proj_doc| {
                let src = proj_doc.recovery_source.take();
                // 読み込みで外したものがあれば、作品を開くときと同じく知らせる（設計審査 b7208d9 の 5.）
                open_document_in_new_tab(ws, proj_doc.into_document_noting(&mut trimmed)?, Some(ctx))?;
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
                    if trimmed {
                        trimmed_works.push(ws.tab().name_stem());
                    }
                    opened.push(item);
                    restored += 1;
                }
                // 読めない（壊れている・このアプリと違う版）：中身は消さずに名前を替えて残し、わけと一緒に知らせる
                Err(why) => {
                    let broken_path = item.recovery_path.with_extension("is2d.broken");
                    let _ = std::fs::rename(&item.recovery_path, &broken_path);
                    let _ = std::fs::remove_file(&item.entry_path);
                    broken_paths.push(format!("{}：{why}", broken_path.display()));
                }
            }
        }

        for item in opened {
            let _ = std::fs::remove_file(&item.recovery_path);
            let _ = std::fs::remove_file(&item.entry_path);
        }

        for name in trimmed_works {
            ws.notify_warning(crate::project_io::trimmed_notice_text(&name));
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
