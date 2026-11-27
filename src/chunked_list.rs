//! 文書の長い列（レイヤーのキーフレーム、シンボルのライブラリ）を、Undo の控えと共有して持つ列。
//!
//! 列を一定の数（`CHUNK`）ほどずつの塊に分け、要素も塊も塊の並びも `Arc` で共有して持つ。複製（Undo の控え）は
//! `Arc` 1つで済み、列の長さで重くならない。書き換えは、塊の並びと、触った塊1つだけを写す（Copy-on-Write）。
//! 要素そのものは、書く所が `Arc::make_mut` で写す。
//!
//! 要素が並びの順にそろった数（`Keyed`：キーフレームの始まりのコマ）を持つときは、要素には塊の基準からの差だけを持ち、
//! 塊ごとに基準を持つ。ある所から後ろの数をみなずらすのは、その塊の中の後ろの要素と、後ろの塊の基準だけを書き換えれば
//! 済む（`shift_keys_from`。書き換える量が列の長さによらない：レビュー f01d518 の R5）

use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct ChunkedList<T> {
    chunks: Arc<Chunks<T>>,
    len: usize,
}

/// 塊の並びと、各塊の先頭の番号と、各塊の数の基準（`Keyed` の要素の数は、基準＋要素の差。`Keyed` でない列では 0）
#[derive(Debug)]
struct Chunks<T> {
    chunks: Vec<Chunk<T>>,
    starts: Vec<usize>,
    bases: Vec<i64>,
}

/// 並びの順にそろった数を持つ要素（要素には塊の基準からの差だけを持つ：`ChunkedList::key`）
pub(crate) trait Keyed: Clone {
    fn rel(&self) -> i64;
    fn set_rel(&mut self, rel: i64);
}

/// 塊（取り消しの控えと共有する）
pub(crate) type Chunk<T> = Arc<ChunkItems<T>>;

/// 塊の中身：要素の並びと、要素の型が決める要約（`Summarize`。初めて要るときに作って持つ）。中身を書く道はみな
/// `DerefMut` を通るので、そこで要約を捨てる（書いた塊だけが要約を作り直す。書いていない塊は、控えと要約ごと共有する）
pub(crate) struct ChunkItems<T> {
    items: Vec<Arc<T>>,
    summary: std::sync::OnceLock<Box<dyn std::any::Any + Send + Sync>>,
}

/// 塊の要約を持つ要素：塊の要素の並びから要約を作る（要約の形は要素の型ごとに1つ）
pub(crate) trait Summarize: Sized {
    type Summary: Send + Sync + 'static;
    fn summarize(items: &[Arc<Self>]) -> Self::Summary;
}

impl<T> ChunkItems<T> {
    fn new(items: Vec<Arc<T>>) -> Self {
        Self { items, summary: std::sync::OnceLock::new() }
    }
}

impl<T: Summarize> ChunkItems<T> {
    /// この塊の要約（初めて要るときに作る）
    pub(crate) fn summary(&self) -> &T::Summary {
        self.summary
            .get_or_init(|| Box::new(T::summarize(&self.items)))
            .downcast_ref()
            .expect("要約の形は要素の型ごとに1つ")
    }
}

/// 写した塊は要約を持たない（写すのは書く前：書けば捨てるので、作り直しを先にしない）
impl<T> Clone for ChunkItems<T> {
    fn clone(&self) -> Self {
        Self::new(self.items.clone())
    }
}

impl<T> std::ops::Deref for ChunkItems<T> {
    type Target = Vec<Arc<T>>;
    fn deref(&self) -> &Vec<Arc<T>> {
        &self.items
    }
}

/// 中身を書く道：要約を捨てる
impl<T> std::ops::DerefMut for ChunkItems<T> {
    fn deref_mut(&mut self) -> &mut Vec<Arc<T>> {
        self.summary = std::sync::OnceLock::new();
        &mut self.items
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for ChunkItems<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.items.fmt(f)
    }
}

/// 1つの塊の要素の数の目安（足して2倍を超えたら2つに分ける）
const CHUNK: usize = 64;

impl<T> Clone for ChunkedList<T> {
    fn clone(&self) -> Self {
        Self {
            chunks: Arc::clone(&self.chunks),
            len: self.len,
        }
    }
}

impl<T> Clone for Chunks<T> {
    fn clone(&self) -> Self {
        Self {
            chunks: self.chunks.clone(),
            starts: self.starts.clone(),
            bases: self.bases.clone(),
        }
    }
}

impl<T> Default for ChunkedList<T> {
    fn default() -> Self {
        Self {
            chunks: Arc::new(Chunks {
                chunks: Vec::new(),
                starts: Vec::new(),
                bases: Vec::new(),
            }),
            len: 0,
        }
    }
}

impl<T> ChunkedList<T> {
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `i` 番目（`i` は列の長さより小さい）が入っている塊と、その中の位置
    fn locate(all: &Chunks<T>, i: usize) -> (usize, usize) {
        let c = all.starts.partition_point(|&s| s <= i) - 1;
        (c, i - all.starts[c])
    }

    pub(crate) fn get(&self, i: usize) -> Option<&Arc<T>> {
        (i < self.len).then(|| {
            let (c, k) = Self::locate(&self.chunks, i);
            &self.chunks.chunks[c][k]
        })
    }

    pub(crate) fn first(&self) -> Option<&Arc<T>> {
        self.get(0)
    }

    pub(crate) fn last(&self) -> Option<&Arc<T>> {
        self.len.checked_sub(1).and_then(|i| self.get(i))
    }

    pub(crate) fn iter(&self) -> ChunkedIter<'_, T> {
        self.into_iter()
    }

    pub(crate) fn insert(&mut self, i: usize, item: Arc<T>) {
        self.insert_at(i, item);
    }

    /// `i` 番目に入れて、入った塊の番号を返す（分けた塊は、元の塊と同じ基準を持つ）
    fn insert_at(&mut self, i: usize, item: Arc<T>) -> usize {
        assert!(i <= self.len, "番号 {i} が列の長さ {} を超える", self.len);
        let all = Arc::make_mut(&mut self.chunks);
        if all.chunks.is_empty() {
            all.chunks.push(Arc::new(ChunkItems::new(Vec::new())));
            all.bases.push(0);
        }
        let (c, k) = if i == self.len {
            let last = all.chunks.len() - 1;
            (last, all.chunks[last].len())
        } else {
            Self::locate(all, i)
        };
        let chunk = Arc::make_mut(&mut all.chunks[c]);
        chunk.insert(k, item);
        if chunk.len() > 2 * CHUNK {
            let half = chunk.len() / 2;
            let tail = chunk.split_off(half);
            all.chunks.insert(c + 1, Arc::new(ChunkItems::new(tail)));
            let base = all.bases[c];
            all.bases.insert(c + 1, base);
        }
        self.len += 1;
        Self::restart(all);
        Self::locate(all, i).0
    }

    pub(crate) fn push(&mut self, item: Arc<T>) {
        self.insert(self.len, item);
    }

    pub(crate) fn remove(&mut self, i: usize) -> Arc<T> {
        assert!(i < self.len, "番号 {i} が列の長さ {} を超える", self.len);
        let all = Arc::make_mut(&mut self.chunks);
        let (c, k) = Self::locate(all, i);
        let removed = Arc::make_mut(&mut all.chunks[c]).remove(k);
        if all.chunks[c].is_empty() {
            all.chunks.remove(c);
            all.bases.remove(c);
        }
        self.len -= 1;
        Self::restart(all);
        removed
    }

    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// 番号 `range` の要素を消す。かかる塊だけを書き（範囲がまるごと入る塊は外す）、ほかの塊は写さずにそのまま共有する
    /// （手間は塊の数と、かかる塊の中身の分。前は全部の要素で消すかを確かめた：全体の点検 2026-11-27）
    pub(crate) fn remove_range(&mut self, range: std::ops::Range<usize>) {
        assert!(range.end <= self.len, "範囲 {range:?} が列の長さ {} を超える", self.len);
        if range.is_empty() {
            return;
        }
        let all = Arc::make_mut(&mut self.chunks);
        let mut c = 0;
        while c < all.chunks.len() {
            let (start, len) = (all.starts[c], all.chunks[c].len());
            let (lo, hi) = (range.start.max(start), range.end.min(start + len));
            if lo < hi {
                Arc::make_mut(&mut all.chunks[c]).drain(lo - start..hi - start);
            }
            if all.chunks[c].is_empty() {
                all.chunks.remove(c);
                all.bases.remove(c);
                all.starts.remove(c);
            } else {
                c += 1;
            }
        }
        self.len -= range.len();
        Self::restart(all);
    }

    /// `other` と塊の並びを共有しているか（共有していれば中身は同じ。複製してから書いていないかの確かめと、中身が変わったか
    /// の決まり `Layer::same_content`）
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.chunks, &other.chunks)
    }

    /// 塊と、その塊の数の基準（`Keyed` でない列では 0）を並びの順に。塊は書き換えると新しい塊になり、書き換えていない塊は
    /// 写しどうしで同じ塊（復旧の書き足すファイルが、新しい塊だけを書く：`recovery_journal`）
    pub(crate) fn chunks_with_bases(&self) -> impl Iterator<Item = (&Chunk<T>, i64)> + '_ {
        self.chunks.chunks.iter().zip(self.chunks.bases.iter().copied())
    }

    /// 列の塊の並びの番地（塊の並び・基準を書き換えると変わる。写しどうしは同じ：`ptr_eq`。この列の写しを持っている間は、
    /// ほかの並びに使い回されない）
    pub(crate) fn list_address(&self) -> usize {
        Arc::as_ptr(&self.chunks) as *const () as usize
    }

    /// テスト用：塊ごとの要素の数
    #[cfg(test)]
    pub(crate) fn chunk_sizes(&self) -> Vec<usize> {
        self.chunks.chunks.iter().map(|c| c.len()).collect()
    }

    /// テスト用：`other` と共有している塊の数と、塊の数
    #[cfg(test)]
    pub(crate) fn shared_chunks_with(&self, other: &Self) -> (usize, usize) {
        let shared = self
            .chunks
            .chunks
            .iter()
            .filter(|c| other.chunks.chunks.iter().any(|o| Arc::ptr_eq(c, o)))
            .count();
        (shared, self.chunks.chunks.len())
    }

    /// 鍵の順の列（鍵 `key`）で、鍵が `k` 以上の最初の要素の番号（無ければ列の長さ。2分探索）
    pub(crate) fn lower_bound<K: Ord>(&self, k: &K, key: impl Fn(&T) -> K) -> usize {
        let all = &self.chunks;
        let c = all.chunks.partition_point(|c| key(&c[c.len() - 1]) < *k);
        all.chunks.get(c).map_or(self.len, |chunk| all.starts[c] + chunk.partition_point(|x| key(x) < *k))
    }

    /// 鍵 `key`（列は鍵の順で、鍵は1つずつ別）が `k` の要素の番号（2分探索）
    pub(crate) fn position_sorted<K: Ord>(&self, k: K, key: impl Fn(&T) -> K) -> Option<usize> {
        let all = &self.chunks;
        let c = all.chunks.partition_point(|c| key(&c[c.len() - 1]) < k);
        let chunk = all.chunks.get(c)?;
        let i = chunk.partition_point(|x| key(x) < k);
        chunk.get(i).filter(|x| key(x) == k).map(|_| all.starts[c] + i)
    }

    /// `may_have`（塊）が真の塊の中で、`is_it` が真の最初の要素の番号（塊を要約などで選んでから探す。手間は塊の数と、
    /// 選んだ塊の中身の分）
    pub(crate) fn position_where(
        &self,
        may_have: impl Fn(&ChunkItems<T>) -> bool,
        is_it: impl Fn(&T) -> bool,
    ) -> Option<usize> {
        let all = &self.chunks;
        all.chunks.iter().zip(&all.starts).filter(|(c, _)| may_have(c)).find_map(|(c, &start)| {
            c.iter().position(|x| is_it(x)).map(|i| start + i)
        })
    }

    /// `other` と同じ並びか（要素は同じ箱か `same`。`Keyed` の列は `same_keyed`）。手間は塊の数と、書いた所の分
    /// （`same_by_chunks`）
    pub(crate) fn same_items(&self, other: &Self, same: impl Fn(&T, &T) -> bool) -> bool {
        self.same_by_chunks(other, |_, _| true, |_, a, _, b| Arc::ptr_eq(a, b) || same(a, b))
    }

    /// 2つの列を、塊をそろえながら前から比べる：両方とも塊の始まりにいて、同じ塊（番地が同じで `chunk_same`（塊の番号）
    /// が真）なら中を見ずに進み、ほかは要素を `item_same`（塊の番号と要素）で比べる。途中に足した・消した所があって塊の
    /// 切れ目がずれても、その先の書いていない塊は両方の列で同じ塊なので、またそろって中を見ずに進む（比べる要素は、書いた
    /// 所の分だけ。前は塊の切れ目がずれると、残りの要素をみな1つずつ比べた：全体の点検 2026-11-27）
    fn same_by_chunks(
        &self,
        other: &Self,
        chunk_same: impl Fn(usize, usize) -> bool,
        item_same: impl Fn(usize, &Arc<T>, usize, &Arc<T>) -> bool,
    ) -> bool {
        if self.ptr_eq(other) {
            return true;
        }
        if self.len != other.len {
            return false;
        }
        let (x, y) = (&self.chunks.chunks, &other.chunks.chunks);
        let (mut ca, mut ka, mut cb, mut kb) = (0, 0, 0, 0);
        loop {
            while ca < x.len() && x[ca].is_empty() {
                ca += 1;
            }
            while cb < y.len() && y[cb].is_empty() {
                cb += 1;
            }
            if ca == x.len() || cb == y.len() {
                return ca == x.len() && cb == y.len();
            }
            if ka == 0 && kb == 0 && Arc::ptr_eq(&x[ca], &y[cb]) && chunk_same(ca, cb) {
                ca += 1;
                cb += 1;
                continue;
            }
            if !item_same(ca, &x[ca][ka], cb, &y[cb][kb]) {
                return false;
            }
            ka += 1;
            if ka == x[ca].len() {
                (ca, ka) = (ca + 1, 0);
            }
            kb += 1;
            if kb == y[cb].len() {
                (cb, kb) = (cb + 1, 0);
            }
        }
    }

    /// 鍵 `key`（列は鍵の順で、鍵は1つずつ別）が `k` の要素（`sorted_from` の最初の要素の鍵が `k` なら、それ）
    pub(crate) fn find_sorted<K: Ord>(&self, k: K, key: impl Fn(&T) -> K) -> Option<&Arc<T>> {
        self.sorted_from(&k, &key).next().filter(|x| key(x) == k)
    }

    /// 鍵の順の列（鍵 `key`）の、鍵が `k` 以上の要素を順に。まず塊を探し（塊のいちばん後ろの鍵で）、塊の中を引く（鍵で
    /// 引くのはこれ1つ）
    pub(crate) fn sorted_from<K: Ord>(&self, k: &K, key: impl Fn(&T) -> K) -> ChunkedIter<'_, T> {
        let all = &self.chunks;
        let c = all.chunks.partition_point(|c| key(&c[c.len() - 1]) < *k);
        let Some(chunk) = all.chunks.get(c) else {
            return ChunkedIter {
                chunks: all.chunks[..0].iter(),
                cur: [].iter(),
                remaining: 0,
            };
        };
        let i = chunk.partition_point(|x| key(x) < *k);
        ChunkedIter {
            chunks: all.chunks[c + 1..].iter(),
            cur: chunk[i..].iter(),
            remaining: self.len - (all.starts[c] + i),
        }
    }

    /// 鍵の順の列（鍵 `key` は1つずつ別。塊の基準を使わない列：`Keyed` の列には使わない）から、鍵が `gone`（鍵の順）の
    /// 要素を消し、`added`（鍵の順。消したあとの列に無い鍵）を足す。消す鍵か足す要素がかかる塊だけを作り直し、ほかの塊は
    /// 写さずにそのまま共有する。作り直す塊は、続けて作り直す塊を1つにまとめてから、`CHUNK` 個ほどずつにそろえて分ける
    /// （`CHUNK / 2` より小さくなれば、続く塊（無ければ前の塊）と合わせる）。手間は、塊の数と、作り直す塊の中身の分
    pub(crate) fn replace_sorted<K: Ord>(
        &mut self,
        gone: &[K],
        added: Vec<Arc<T>>,
        key: impl Fn(&T) -> K,
    ) {
        debug_assert!(gone.windows(2).all(|w| w[0] < w[1]));
        debug_assert!(added.windows(2).all(|w| key(&w[0]) < key(&w[1])));
        debug_assert!(self.chunks.bases.iter().all(|&b| b == 0));
        if gone.is_empty() && added.is_empty() {
            return;
        }
        let old = &self.chunks.chunks;
        let n = old.len();
        let mut out: Vec<Chunk<T>> = Vec::with_capacity(n + 1);
        // 作り直す塊の中身（続けて作り直す塊はまとめる）
        let mut run: Vec<Arc<T>> = Vec::new();
        let mut added = added.into_iter().peekable();
        let mut gi = 0;
        for (i, chunk) in old.iter().enumerate() {
            let last = i + 1 == n;
            let upper = key(&chunk[chunk.len() - 1]);
            // この塊にかかる消す鍵と足す要素（いちばん後ろの塊は、残りを全部受け取る）
            let g0 = gi;
            while gi < gone.len() && (last || gone[gi] <= upper) {
                gi += 1;
            }
            let mut mine: Vec<Arc<T>> = Vec::new();
            while let Some(a) = added.next_if(|a| last || key(a) <= upper) {
                mine.push(a);
            }
            let touched = gi > g0 || !mine.is_empty();
            if touched {
                merge_sorted(&mut run, chunk, &gone[g0..gi], mine, &key);
                if run.len() >= CHUNK {
                    split_even(std::mem::take(&mut run), &mut out);
                }
            } else if run.is_empty() {
                out.push(Arc::clone(chunk));
            } else if run.len() < CHUNK / 2 {
                // 作り直した中身が小さすぎれば、この塊と合わせる
                run.extend(chunk.iter().cloned());
                split_even(std::mem::take(&mut run), &mut out);
            } else {
                split_even(std::mem::take(&mut run), &mut out);
                out.push(Arc::clone(chunk));
            }
        }
        // 列が空だったとき（塊が無い）は、足す要素だけ
        run.extend(added);
        if !run.is_empty() {
            if run.len() < CHUNK / 2 {
                if let Some(prev) = out.pop() {
                    let mut joined: Vec<Arc<T>> = prev.iter().cloned().collect();
                    joined.append(&mut run);
                    run = joined;
                }
            }
            split_even(run, &mut out);
        }
        let all = Arc::make_mut(&mut self.chunks);
        all.bases = vec![0; out.len()];
        all.chunks = out;
        Self::restart(all);
        self.len = all.chunks.iter().map(|c| c.len()).sum();
    }

    /// 各塊の先頭の番号を数え直す
    fn restart(all: &mut Chunks<T>) {
        all.starts.clear();
        let mut n = 0;
        for c in &all.chunks {
            all.starts.push(n);
            n += c.len();
        }
    }
}

/// 塊 `chunk` から鍵が `gone` の要素を除き、`mine` を鍵の順に混ぜて、`run` の後ろに足す（`replace_sorted`）
fn merge_sorted<T, K: Ord>(
    run: &mut Vec<Arc<T>>,
    chunk: &[Arc<T>],
    gone: &[K],
    mine: Vec<Arc<T>>,
    key: &impl Fn(&T) -> K,
) {
    let mut gone = gone.iter().peekable();
    let mut mine = mine.into_iter().peekable();
    for x in chunk {
        let k = key(x);
        while gone.next_if(|g| **g < k).is_some() {}
        while let Some(a) = mine.next_if(|a| key(a) < k) {
            run.push(a);
        }
        if gone.next_if(|g| **g == k).is_none() {
            run.push(Arc::clone(x));
        }
    }
    run.extend(mine);
}

/// `items` を、大きさのそろった塊（`CHUNK` 個以下）に分けて `out` に足す（`replace_sorted`）
fn split_even<T>(items: Vec<Arc<T>>, out: &mut Vec<Chunk<T>>) {
    if items.is_empty() {
        return;
    }
    let n = items.len().div_ceil(CHUNK);
    let (base, extra) = (items.len() / n, items.len() % n);
    let mut it = items.into_iter();
    for p in 0..n {
        let size = base + usize::from(p < extra);
        out.push(Arc::new(ChunkItems::new(it.by_ref().take(size).collect())));
    }
}

impl<T> From<Vec<Arc<T>>> for ChunkedList<T> {
    fn from(list: Vec<Arc<T>>) -> Self {
        list.into_iter().collect()
    }
}

impl<T> FromIterator<Arc<T>> for ChunkedList<T> {
    fn from_iter<I: IntoIterator<Item = Arc<T>>>(iter: I) -> Self {
        let mut all = Chunks {
            chunks: Vec::new(),
            starts: Vec::new(),
            bases: Vec::new(),
        };
        let mut len = 0;
        let mut cur: Vec<Arc<T>> = Vec::new();
        for item in iter {
            cur.push(item);
            len += 1;
            if cur.len() == CHUNK {
                all.chunks.push(Arc::new(ChunkItems::new(std::mem::take(&mut cur))));
            }
        }
        if !cur.is_empty() {
            all.chunks.push(Arc::new(ChunkItems::new(cur)));
        }
        all.bases = vec![0; all.chunks.len()];
        Self::restart(&mut all);
        Self {
            chunks: Arc::new(all),
            len,
        }
    }
}

impl<T: Keyed> ChunkedList<T> {
    /// `i` 番目の数（塊の基準＋要素の差）
    pub(crate) fn key(&self, i: usize) -> usize {
        let (c, k) = Self::locate(&self.chunks, i);
        (self.chunks.bases[c] + self.chunks.chunks[c][k].rel()) as usize
    }

    /// 数と要素を、前から順に
    /// `other` と同じ並びか：数と要素が順に同じ（要素は同じ箱か `same`）。同じ塊（番地と基準が同じ）は中を見ない
    /// （`same_by_chunks`。手間は塊の数と、書いた所の分）
    pub(crate) fn same_keyed(&self, other: &Self, same: impl Fn(&T, &T) -> bool) -> bool {
        let (xb, yb) = (&self.chunks.bases, &other.chunks.bases);
        self.same_by_chunks(
            other,
            |ca, cb| xb[ca] == yb[cb],
            |ca, a, cb, b| xb[ca] + a.rel() == yb[cb] + b.rel() && (Arc::ptr_eq(a, b) || same(a, b)),
        )
    }

    pub(crate) fn iter_keyed(&self) -> impl Iterator<Item = (usize, &Arc<T>)> + '_ {
        self.chunks
            .chunks
            .iter()
            .zip(&self.chunks.bases)
            .flat_map(|(c, &b)| c.iter().map(move |x| ((b + x.rel()) as usize, x)))
    }

    /// `pred` が数について真から偽に変わる番号（`slice::partition_point` と同じ。列は数の順）
    pub(crate) fn partition_point_key(&self, mut pred: impl FnMut(usize) -> bool) -> usize {
        let (mut lo, mut hi) = (0, self.len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if pred(self.key(mid)) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// 数 `key` の要素 `item` を `i` 番目に入れる（列が数の順のままになる所に）
    pub(crate) fn insert_keyed(&mut self, i: usize, key: usize, item: Arc<T>) {
        let c = self.insert_at(i, item);
        let base = self.chunks.bases[c];
        let k = i - self.chunks.starts[c];
        let all = Arc::make_mut(&mut self.chunks);
        Arc::make_mut(&mut Arc::make_mut(&mut all.chunks[c])[k]).set_rel(key as i64 - base);
    }

    /// `i` 番目の数を `key` にする（列が数の順のままになる値だけ）
    pub(crate) fn set_key(&mut self, i: usize, key: usize) {
        let (c, k) = Self::locate(&self.chunks, i);
        let all = Arc::make_mut(&mut self.chunks);
        let base = all.bases[c];
        Arc::make_mut(&mut Arc::make_mut(&mut all.chunks[c])[k]).set_rel(key as i64 - base);
    }

    /// `i` 番目から後ろの数を、みな `delta` だけずらす（列が数の順のままになる量だけ）。書き換えるのは、`i` 番目の塊の中の
    /// 後ろの要素と、後ろの塊の基準だけ
    pub(crate) fn shift_keys_from(&mut self, i: usize, delta: i64) {
        if i >= self.len || delta == 0 {
            return;
        }
        let (c, k) = Self::locate(&self.chunks, i);
        let all = Arc::make_mut(&mut self.chunks);
        if k == 0 {
            all.bases[c] += delta;
        } else {
            for item in &mut Arc::make_mut(&mut all.chunks[c])[k..] {
                let it = Arc::make_mut(item);
                it.set_rel(it.rel() + delta);
            }
        }
        for b in &mut all.bases[c + 1..] {
            *b += delta;
        }
    }

    /// 数と要素の並び（数の順）から作る（塊の基準は、みな 0 から始める）
    pub(crate) fn from_keyed(items: impl IntoIterator<Item = (usize, Arc<T>)>) -> Self {
        items
            .into_iter()
            .map(|(key, mut item)| {
                Arc::make_mut(&mut item).set_rel(key as i64);
                item
            })
            .collect()
    }
}

impl<T> std::ops::Index<usize> for ChunkedList<T> {
    type Output = Arc<T>;
    fn index(&self, i: usize) -> &Arc<T> {
        self.get(i)
            .unwrap_or_else(|| panic!("番号 {i} が列の長さ {} を超える", self.len))
    }
}

impl<T> std::ops::IndexMut<usize> for ChunkedList<T> {
    /// 触った塊と塊の並びだけを写す（Undo の控えと共有していれば）
    fn index_mut(&mut self, i: usize) -> &mut Arc<T> {
        assert!(i < self.len, "番号 {i} が列の長さ {} を超える", self.len);
        let (c, k) = Self::locate(&self.chunks, i);
        let all = Arc::make_mut(&mut self.chunks);
        &mut Arc::make_mut(&mut all.chunks[c])[k]
    }
}

/// 列を前から順にたどる（残りの数がいつも分かる：受け取る側が入れ物を1回で用意できる）
pub(crate) struct ChunkedIter<'a, T> {
    chunks: std::slice::Iter<'a, Chunk<T>>,
    cur: std::slice::Iter<'a, Arc<T>>,
    remaining: usize,
}

impl<'a, T> Iterator for ChunkedIter<'a, T> {
    type Item = &'a Arc<T>;
    fn next(&mut self) -> Option<&'a Arc<T>> {
        loop {
            if let Some(x) = self.cur.next() {
                self.remaining -= 1;
                return Some(x);
            }
            self.cur = self.chunks.next()?.iter();
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<T> ExactSizeIterator for ChunkedIter<'_, T> {}

impl<'a, T> IntoIterator for &'a ChunkedList<T> {
    type Item = &'a Arc<T>;
    type IntoIter = ChunkedIter<'a, T>;
    fn into_iter(self) -> ChunkedIter<'a, T> {
        ChunkedIter {
            chunks: self.chunks.chunks.iter(),
            cur: [].iter(),
            remaining: self.len,
        }
    }
}

/// 足していくだけの、長さの前もって分からない列（保存・読み込みで、作品の大きさに比例する数の塊を並べる：`WriteTiles`・
/// `ReadTiles`）。1つの並びを押し足しで伸ばすと、伸ばすたびに取り直す（realloc）ので、Mac では捨てたあとも、その広さが
/// アプリのメモリとして残る（保存や開閉のたびに増えたまま戻らない：レビュー de92ceb の N-5001。測った：130 万個の並びを
/// 押し足しで作って捨てると1回ごと +10MB 残り、大きさを決めて1回で取った並びでは残らない）。そこで、k 番目の塊を
/// ちょうど 2^k 個の大きさで1回だけ取り、取り直さない（塊の数は長さの対数だけ。要素は写さない）
pub(crate) struct GrowingList<T> {
    segs: Vec<Vec<T>>,
    len: usize,
}

impl<T> Default for GrowingList<T> {
    fn default() -> Self {
        Self {
            segs: Vec::new(),
            len: 0,
        }
    }
}

impl<T: Clone> Clone for GrowingList<T> {
    fn clone(&self) -> Self {
        Self {
            segs: self.segs.clone(),
            len: self.len,
        }
    }
}

impl<T> GrowingList<T> {
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn push(&mut self, item: T) {
        let k = self.segs.len();
        if self.segs.last().is_none_or(|s| s.len() == 1 << (k - 1)) {
            self.segs.push(Vec::with_capacity(1 << k));
        }
        self.segs.last_mut().expect("塊を足した").push(item);
        self.len += 1;
    }

    /// 番号 `i` の要素の、塊と塊の中の番号（k 番目の塊は、番号 2^k − 1 から 2^k 個）
    fn at(i: usize) -> (usize, usize) {
        let k = (i + 1).ilog2() as usize;
        (k, i + 1 - (1 << k))
    }

    pub(crate) fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        let (k, j) = Self::at(i);
        self.segs.get_mut(k)?.get_mut(j)
    }

    /// 前から順に（数は `len`）
    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.segs.iter().flatten()
    }

    /// 前から順に、要素を取り出して
    pub(crate) fn into_iter(self) -> impl Iterator<Item = T> {
        self.segs.into_iter().flatten()
    }
}
