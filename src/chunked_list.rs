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

/// 塊
type Chunk<T> = Arc<Vec<Arc<T>>>;

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

    #[cfg(test)]
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
            all.chunks.push(Arc::new(Vec::new()));
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
            let tail = chunk.split_off(chunk.len() / 2);
            all.chunks.insert(c + 1, Arc::new(tail));
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

    /// `keep` が偽の要素を消す。消すものの無い塊は、写さずにそのまま共有する
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&Arc<T>) -> bool) {
        self.retain_in_chunks(|_, x| keep(x));
    }

    /// `keep`（塊の基準と要素）が偽の要素を消す。消すものの無い塊は、写さずにそのまま共有する
    fn retain_in_chunks(&mut self, mut keep: impl FnMut(i64, &Arc<T>) -> bool) {
        let all = Arc::make_mut(&mut self.chunks);
        for (chunk, &b) in all.chunks.iter_mut().zip(&all.bases) {
            if !chunk.iter().all(|x| keep(b, x)) {
                Arc::make_mut(chunk).retain(|x| keep(b, x));
            }
        }
        let mut bases = all.bases.iter();
        all.bases = all
            .chunks
            .iter()
            .filter_map(|c| bases.next().copied().filter(|_| !c.is_empty()))
            .collect();
        all.chunks.retain(|c| !c.is_empty());
        self.len = all.chunks.iter().map(|c| c.len()).sum();
        Self::restart(all);
    }

    /// テスト用：`other` と塊の並びを共有しているか（複製してから書いていないか）
    #[cfg(test)]
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.chunks, &other.chunks)
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
                all.chunks.push(Arc::new(std::mem::take(&mut cur)));
            }
        }
        if !cur.is_empty() {
            all.chunks.push(Arc::new(cur));
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

    /// `keep`（数と要素）が偽の要素を消す。消すものの無い塊は、写さずにそのまま共有する
    pub(crate) fn retain_keyed(&mut self, mut keep: impl FnMut(usize, &Arc<T>) -> bool) {
        self.retain_in_chunks(|b, x| keep((b + x.rel()) as usize, x));
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

/// 塊1つの要素をたどる
type ChunkIterFn<'a, T> = fn(&'a Chunk<T>) -> std::slice::Iter<'a, Arc<T>>;

/// 列を前から順にたどる
pub(crate) type ChunkedIter<'a, T> = std::iter::FlatMap<
    std::slice::Iter<'a, Chunk<T>>,
    std::slice::Iter<'a, Arc<T>>,
    ChunkIterFn<'a, T>,
>;

impl<'a, T> IntoIterator for &'a ChunkedList<T> {
    type Item = &'a Arc<T>;
    type IntoIter = ChunkedIter<'a, T>;
    fn into_iter(self) -> ChunkedIter<'a, T> {
        let chunk_iter: ChunkIterFn<'a, T> = |c| c.iter();
        self.chunks.chunks.iter().flat_map(chunk_iter)
    }
}
