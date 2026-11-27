//! 仕事の砦（Layer 1：何も参照しない部品。どの層の仕事の入口からも使うので、いちばん下の層に置く）：計算が止まったときの
//! 受け止め `guarded`・裏のスレッドの始め方 `spawn_guarded`・止まったスレッドの記録 `StoppedThreads`（設計：
//! claude/最後の砦を仕事の入口に置く設計.md。置き場所：claude/層の分け方の直しの設計.md の 1.）

/// 計算の途中で止まったら（panic：計算の前提が破れた）受け止めて `None` を返す（最後の砦。受け止めるのはこの1つ）。置くのは
/// 仕事の入口ごとに1つ：画面の毎コマ・終わるとき・アプリの始まりの手順・OS から呼ばれる受け口・スレッドの始まり
/// （`spawn_guarded`）と、その中の仕事の単位（作品を変える操作 `apply`・復旧の書き手のタブ1つ分・書き出し1回・音の装置の
/// 呼び出し1回・先読みと地図の確かめの地図1つ）。計算はかならずどれかの入口から始まるので、どこで止まっても砦の中にいる
/// （設計：claude/最後の砦を仕事の入口に置く設計.md。前は計算の場所ごとに置いたので、置き忘れが出続けた）。受け止めた所が
/// その仕事を無かったことにする（別のやり方へ逃げない）。どの層からも使えるよう、いちばん下の層に置く。何も止まらない
/// ときの手間は無い
pub(crate) fn guarded<R>(f: impl FnOnce() -> R) -> Option<R> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok()
}

/// 名前 `name` のスレッドで `f` を、砦の中で走らせる（スレッドを始めるのはこれ1つ。素のスレッドの始め方は clippy の
/// `disallowed-methods` で禁じてある：clippy.toml）。止まったら、そのスレッドは `None` で終わり、名前を止まったスレッドの
/// 記録に足す（画面が毎コマ `StoppedThreads::take_new` で読んで知らせる。スレッドの中の仕事の単位ごとの砦が先に受け止める
/// ので、ここへ来るのは、それでも漏れたとき）。スレッドを作れなければ `Err`
pub(crate) fn spawn_guarded<T: Send + 'static>(
    name: &str,
    f: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<Option<T>>> {
    let owned = name.to_owned();
    #[allow(clippy::disallowed_methods)] // スレッドを始める所はここだけ
    std::thread::Builder::new().name(owned.clone()).spawn(move || {
        let out = guarded(f);
        if out.is_none() {
            STOPPED_THREADS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(owned);
        }
        out
    })
}

/// 砦の外まで止まって終わったスレッドの名前の記録（`spawn_guarded` が足すだけで、消さない。スレッドはプロセスに1組なので
/// 記録も1つ。読む側はそれぞれ、どこまで読んだかを覚える：`StoppedThreads`。足すのは内部の不具合のときだけ）
static STOPPED_THREADS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// 止まったスレッドの記録を読む側（どこまで読んだか）。作ったときより前の記録は読まない
pub(crate) struct StoppedThreads {
    seen: usize,
}

impl Default for StoppedThreads {
    fn default() -> Self {
        Self { seen: STOPPED_THREADS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len() }
    }
}

impl StoppedThreads {
    /// まだ読んでいない、止まったスレッドの名前（画面が毎コマ読んで知らせる）
    pub(crate) fn take_new(&mut self) -> Vec<String> {
        let all = STOPPED_THREADS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let new = all.get(self.seen..).unwrap_or_default().to_vec();
        self.seen = all.len();
        new
    }
}
