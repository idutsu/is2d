//! シンボルのライブラリ（設計：claude/シンボルの目次と参照の表の設計.md）。本当の中身は、シンボルを番号の順に並べた列
//! （`defs`。番号で引くのは2分探索）。見せる順（`order`）・名前の表（`names`）・使われ方の表（`users`・`sound_users`）と、
//! シンボルが持つ子・音・高さ・動くか・深い版は、ライブラリを書く入口（このファイルの `insert`・`remove`・
//! `put_timeline`・`rename`）だけが、書いた所から作り直す。表はどれも塊ごとに取り消しの控えと共有する
//! （`ChunkedList`）ので、控えを取る・戻すはライブラリの入れ替えだけで、表も一緒に入れ替わる

use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::chunked_list::{Chunk, ChunkedList, Summarize};
use crate::timeline::Timeline;

use super::{SymbolDef, MAX_SYMBOL_NEST_DEPTH};

/// 見せる順の1つ（シンボルの番号）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot(pub(crate) u64);

/// 見せる順の塊の要約：塊の中のシンボルの番号（小さい順）。番号の場所を探すとき、塊を要約で選ぶ（`Library::position`）
impl Summarize for Slot {
    type Summary = Vec<u64>;
    fn summarize(items: &[Arc<Self>]) -> Vec<u64> {
        let mut ids: Vec<u64> = items.iter().map(|s| s.0).collect();
        ids.sort_unstable();
        ids
    }
}

/// 名前の表の1つ（名前の印の順、同じ印なら番号の順）
#[derive(Clone, Debug)]
struct NameEntry {
    mark: u64,
    id: u64,
    name: String,
}

/// 名前の番号の付け方（シンボルに名前を付ける所が使う形：`Document::numbered_name`）
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NumberForm {
    /// 「元 N」（シンボル化の「Symbol N」・複製の「〇〇のコピー N」）
    Space,
    /// 「元 (N)」（別のタブから移したシンボル）
    Paren,
}

impl NumberForm {
    /// 元 `stem` に番号 `n` を付けた名前
    pub(crate) fn name(self, stem: &str, n: u64) -> String {
        match self {
            Self::Space => format!("{stem} {n}"),
            Self::Paren => format!("{stem} ({n})"),
        }
    }

    /// 名前 `name` が、この付け方で番号を付けた名前なら（元, 番号）。番号は、付ける所が書く形（先頭の 0 なし）だけ
    fn parse(self, name: &str) -> Option<(&str, u64)> {
        let (stem, digits) = match self {
            Self::Space => name.rsplit_once(' ')?,
            Self::Paren => name.strip_suffix(')')?.rsplit_once(" (")?,
        };
        let canonical = !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'));
        canonical.then(|| digits.parse().ok().map(|n| (stem, n))).flatten()
    }
}

/// 番号の付いた名前の表の1つ（付け方・元・番号の順）。同じ名前のシンボルがいくつあるか（名前を変えて重なることがある）
#[derive(Clone, Debug)]
struct Numbered {
    form: NumberForm,
    stem: String,
    n: u64,
    count: usize,
}

fn numbered_key(e: &Numbered) -> (NumberForm, String, u64) {
    (e.form, e.stem.clone(), e.n)
}

/// 使われ方の表の1つ：`used`（子のシンボルか音の番号）を `user`（シンボルの番号）が使う（`used`・`user` の順）
#[derive(Clone, Copy, Debug)]
struct Use {
    used: u64,
    user: u64,
}

/// シンボルのライブラリ
#[derive(Clone, Debug, Default)]
pub(crate) struct Library {
    /// シンボル（番号の順。新しい番号はいつも今までより大きいので、足すのは列の終わり）
    defs: ChunkedList<SymbolDef>,
    /// 見せる順（ライブラリのタブ・保存の順。複製は元のすぐ下）
    order: ChunkedList<Slot>,
    names: ChunkedList<NameEntry>,
    /// 番号の付いた名前（空いている一番小さい番号を2分探索で探す：`first_free_number`）
    numbered: ChunkedList<Numbered>,
    users: ChunkedList<Use>,
    sound_users: ChunkedList<Use>,
    /// 高さが入れ子の上限を超えたシンボルの数
    too_deep: usize,
}

/// 名前の印（同じ名前は同じ印。印が同じでも名前は比べる：`Library::name_taken`）
fn name_mark(name: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}

fn use_key(u: &Use) -> (u64, u64) {
    (u.used, u.user)
}

/// 小さい順の `a` にあって `b` に無いもの
fn minus(a: &[u64], b: &[u64]) -> Vec<u64> {
    a.iter().copied().filter(|x| b.binary_search(x).is_err()).collect()
}

impl Library {
    pub(crate) fn len(&self) -> usize {
        self.defs.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// 番号 `id` のシンボル（2分探索）
    pub(crate) fn get(&self, id: u64) -> Option<&Arc<SymbolDef>> {
        self.defs.find_sorted(id, |s| s.id)
    }

    /// シンボルを番号の順に（順のいらない所：全部を見る・確かめる）
    pub(crate) fn iter_by_id(&self) -> impl Iterator<Item = &Arc<SymbolDef>> + '_ {
        self.defs.iter()
    }

    /// 見せる順の `i` 番目のシンボル
    pub(crate) fn at(&self, i: usize) -> Option<&Arc<SymbolDef>> {
        self.order.get(i).and_then(|s| self.get(s.0))
    }

    /// 番号 `id` のシンボルの見せる順の場所（塊を要約で選んでから、塊の中を探す。手間は塊の数）
    pub(crate) fn position(&self, id: u64) -> Option<usize> {
        self.order.position_where(|c| c.summary().binary_search(&id).is_ok(), |s| s.0 == id)
    }

    /// どれかのシンボルが名前 `name` か
    pub(crate) fn name_taken(&self, name: &str) -> bool {
        let mark = name_mark(name);
        self.names.sorted_from(&(mark, 0), |e| (e.mark, e.id)).take_while(|e| e.mark == mark).any(|e| e.name == name)
    }

    /// 付け方 `form`・元 `stem` で、番号が `from` 以上の、どのシンボルも使っていない一番小さい番号（無ければ `None`）。
    /// 表の、その元の番号の並びで、番号が「`from` から1つずつ」でなくなる最初の所を2分探索で探す（手間は log。前は候補の
    /// 名前を1つずつ試したので、同じ名前の写しの数だけかかった：全体の点検 2026-11-27）
    pub(crate) fn first_free_number(&self, form: NumberForm, stem: &str, from: u64) -> Option<u64> {
        let key = |e: &Numbered| numbered_key(e);
        let i0 = self.numbered.lower_bound(&(form, stem.to_owned(), from), key);
        let in_family = |i: usize| self.numbered.get(i).is_some_and(|e| e.form == form && e.stem == stem);
        // 元が同じ所の終わり
        let (mut lo, mut hi) = (i0, self.numbered.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if in_family(mid) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let end = lo;
        // 番号が `from + (i - i0)` のままの所の終わり（番号は1つずつ別で、小さい順なので、一度ずれたらずれたまま）
        let (mut lo, mut hi) = (i0, end);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let n = self.numbered.get(mid).map_or(u64::MAX, |e| e.n);
            if Some(n) == from.checked_add((mid - i0) as u64) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        from.checked_add((lo - i0) as u64)
    }

    /// シンボル `id` を中で使っているシンボルの番号（番号の順）
    pub(crate) fn users_of(&self, id: u64) -> impl Iterator<Item = u64> + '_ {
        self.users.sorted_from(&(id, 0), use_key).take_while(move |u| u.used == id).map(|u| u.user)
    }

    /// 音 `sound` を中で使っているシンボルの番号（番号の順）
    pub(crate) fn sound_users_of(&self, sound: u64) -> impl Iterator<Item = u64> + '_ {
        self.sound_users.sorted_from(&(sound, 0), use_key).take_while(move |u| u.used == sound).map(|u| u.user)
    }

    /// 高さが入れ子の上限を超えたシンボルがあるか
    pub(crate) fn nest_exceeded(&self) -> bool {
        self.too_deep > 0
    }

    /// シンボル `from` から、使っているシンボルを上へたどって（`from` 自身を含む）、シンボル `target` に行き着くか
    /// （`target` を `from` の中に置くと輪になるか。手間は `from` の先祖の数）
    pub(crate) fn reaches_up(&self, from: u64, target: u64) -> bool {
        super::reaches(from, |id| id == target, |id| self.users_of(id).collect())
    }

    /// テスト用：`other` と同じ列を共有しているか（取り消しの控え）
    #[cfg(test)]
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        self.defs.ptr_eq(&other.defs) && self.order.ptr_eq(&other.order)
    }

    /// シンボルの列と見せる順の、塊と番地（復旧の書き足すファイル：書いた塊は番地で見分ける）
    pub(crate) fn def_chunks(&self) -> impl Iterator<Item = &Chunk<SymbolDef>> + '_ {
        self.defs.chunks_with_bases().map(|(c, _)| c)
    }

    pub(crate) fn order_chunks(&self) -> impl Iterator<Item = &Chunk<Slot>> + '_ {
        self.order.chunks_with_bases().map(|(c, _)| c)
    }

    /// 中身が同じか（取り消しの1手にするかの比べ）：見せる順と、シンボルの中身（`SymbolDef::same_content`）。同じ塊は
    /// 中を見ない（手間は書いた塊の分）
    pub(crate) fn same_content(&self, other: &Self) -> bool {
        self.order.same_items(&other.order, |a, b| a == b)
            && self.defs.same_items(&other.defs, SymbolDef::same_content)
    }

    // ------------------------------------------------------------------------
    // 書く入口（文書 `Document` だけが呼ぶ）
    // ------------------------------------------------------------------------

    /// シンボル `def` を見せる順の `at` 番目（`None` なら最後）に足す。番号は、ライブラリのどのシンボルとも違うこと
    pub(super) fn insert(&mut self, at: Option<usize>, def: SymbolDef) {
        let id = def.id;
        debug_assert!(self.get(id).is_none(), "シンボルの番号 {id} が重なる");
        let refs = def.timeline.refs();
        let mut def = def;
        def.children.clone_from(&refs.symbols);
        def.sounds.clone_from(&refs.sounds);
        def.height = 0;
        let name = def.name.clone();
        self.defs.replace_sorted(&[], vec![Arc::new(def)], |s| s.id);
        match at {
            Some(i) => self.order.insert(i.min(self.order.len()), Arc::new(Slot(id))),
            None => self.order.push(Arc::new(Slot(id))),
        }
        self.add_name(id, &name);
        self.relink(id, &[], &refs.symbols, &[], &refs.sounds);
        // 足したシンボル自身の版は、渡されたまま（別のタブから移した写しは、元と同じ版で、元と同じ中身だと分かる）
        self.propagate(id, false);
    }

    /// シンボル `id` を消す（中で使っているシンボルの表からも外す。使っている親は、高さ・動くか・版を作り直す）
    pub(super) fn remove(&mut self, id: u64) -> Option<Arc<SymbolDef>> {
        let def = self.get(id)?.clone();
        let parents: Vec<u64> = self.users_of(id).collect();
        self.relink(id, &def.children, &[], &def.sounds, &[]);
        if def.height > MAX_SYMBOL_NEST_DEPTH {
            self.too_deep -= 1;
        }
        self.defs.replace_sorted(&[id], Vec::new(), |s| s.id);
        if let Some(i) = self.position(id) {
            self.order.remove(i);
        }
        self.remove_name(id, &def.name);
        for p in parents {
            self.propagate(p, true);
        }
        Some(def)
    }

    /// シンボル `id` のタイムラインを `tl` にする（書いた写しを置く）。書く前の写し `before` と中身が違えば、子・音・高さ・動くかを作り直し、
    /// 自分と先祖の版を上げる。中身が変わったら `true`
    pub(super) fn put_timeline(&mut self, id: u64, tl: Timeline, before: &Timeline) -> bool {
        let Some(i) = self.defs.position_sorted(id, |s| s.id) else {
            return false;
        };
        let changed = !tl.same_content(before);
        let def = Arc::make_mut(&mut self.defs[i]);
        def.timeline = tl;
        if !changed {
            return false;
        }
        let refs = def.timeline.refs();
        let (old_children, old_sounds) = (std::mem::take(&mut def.children), std::mem::take(&mut def.sounds));
        def.children.clone_from(&refs.symbols);
        def.sounds.clone_from(&refs.sounds);
        self.relink(id, &old_children, &refs.symbols, &old_sounds, &refs.sounds);
        self.propagate(id, true);
        true
    }

    /// シンボル `id` の、見ているコマ・選んでいるレイヤーなど中身でない状態だけを書く窓（中身は書けないので、何も作り直さ
    /// ない：`TimelineView`）
    pub(super) fn view_mut(&mut self, id: u64) -> Option<crate::timeline::TimelineView<'_>> {
        let i = self.defs.position_sorted(id, |s| s.id)?;
        Some(Arc::make_mut(&mut self.defs[i]).timeline.view_mut())
    }

    /// シンボル `id` の名前を `name` にする（名前の表も直す）。変わったら `true`
    pub(super) fn rename(&mut self, id: u64, name: String) -> bool {
        let Some(i) = self.defs.position_sorted(id, |s| s.id) else {
            return false;
        };
        if self.defs[i].name == name {
            return false;
        }
        let old = std::mem::replace(&mut Arc::make_mut(&mut self.defs[i]).name, name.clone());
        self.remove_name(id, &old);
        self.add_name(id, &name);
        true
    }

    /// 名前の表と番号の付いた名前の表に、シンボル `id` の名前 `name` を足す
    fn add_name(&mut self, id: u64, name: &str) {
        self.names.replace_sorted(&[], vec![Arc::new(NameEntry { mark: name_mark(name), id, name: name.to_owned() })], |e| {
            (e.mark, e.id)
        });
        for form in [NumberForm::Space, NumberForm::Paren] {
            if let Some((stem, n)) = form.parse(name) {
                let k = (form, stem.to_owned(), n);
                let count = self.numbered.find_sorted(k.clone(), numbered_key).map_or(0, |e| e.count);
                let gone: Vec<_> = (count > 0).then(|| k.clone()).into_iter().collect();
                let e = Numbered { form, stem: stem.to_owned(), n, count: count + 1 };
                self.numbered.replace_sorted(&gone, vec![Arc::new(e)], numbered_key);
            }
        }
    }

    /// 名前の表と番号の付いた名前の表から、シンボル `id` の名前 `name` を外す
    fn remove_name(&mut self, id: u64, name: &str) {
        self.names.replace_sorted(&[(name_mark(name), id)], Vec::new(), |e| (e.mark, e.id));
        for form in [NumberForm::Space, NumberForm::Paren] {
            if let Some((stem, n)) = form.parse(name) {
                let k = (form, stem.to_owned(), n);
                let count = self.numbered.find_sorted(k.clone(), numbered_key).map_or(0, |e| e.count);
                let added: Vec<Arc<Numbered>> =
                    (count > 1).then(|| Arc::new(Numbered { form, stem: stem.to_owned(), n, count: count - 1 })).into_iter().collect();
                self.numbered.replace_sorted(&[k], added, numbered_key);
            }
        }
    }

    /// 使われ方の表で、シンボル `user` の子を `old_children` から `children` に、音を `old_sounds` から `sounds` にする
    /// （違いだけを書く）
    fn relink(&mut self, user: u64, old_children: &[u64], children: &[u64], old_sounds: &[u64], sounds: &[u64]) {
        let edit = |table: &mut ChunkedList<Use>, old: &[u64], new: &[u64]| {
            let gone: Vec<(u64, u64)> = minus(old, new).into_iter().map(|u| (u, user)).collect();
            let added: Vec<Arc<Use>> = minus(new, old).into_iter().map(|used| Arc::new(Use { used, user })).collect();
            // 消す鍵と足す鍵は重ならないので、1回で書ける
            table.replace_sorted(&gone, added, use_key);
        };
        edit(&mut self.users, old_children, children);
        edit(&mut self.sound_users, old_sounds, sounds);
    }

    /// シンボル `start` の高さ・動くかを子から作り直し（`bump_start` なら版も上げ）、使っている親へ順に伝える（親の高さ・
    /// 動くかは子から作り直し、版は1回ずつ上げる）。高さは入れ子の上限＋1で止める（輪があっても終わる）。手間は先祖の数
    /// ×子の数×log
    fn propagate(&mut self, start: u64, bump_start: bool) {
        let mut queue = VecDeque::from([start]);
        let mut bumped: HashSet<u64> = HashSet::new();
        while let Some(id) = queue.pop_front() {
            let Some(i) = self.defs.position_sorted(id, |s| s.id) else {
                continue;
            };
            let (height, dynamic) = self.computed(&self.defs[i]);
            let first = bumped.insert(id);
            let def = &self.defs[i];
            let changed = def.height != height || def.dynamic != dynamic;
            if !changed && !first {
                continue;
            }
            let was_deep = def.height > MAX_SYMBOL_NEST_DEPTH;
            let def = Arc::make_mut(&mut self.defs[i]);
            def.height = height;
            def.dynamic = dynamic;
            if first && (bump_start || id != start) {
                def.revision = crate::vector_engine::next_mesh_revision();
            }
            let is_deep = height > MAX_SYMBOL_NEST_DEPTH;
            if was_deep != is_deep {
                if is_deep {
                    self.too_deep += 1;
                } else {
                    self.too_deep -= 1;
                }
            }
            queue.extend(self.users_of(id));
        }
    }

    /// シンボル `def` の高さ（1＋子の高さの最大。無い子は 0。上限＋1で止める）と、動くか（2コマ以上か、動く子を持つ）
    fn computed(&self, def: &SymbolDef) -> (usize, bool) {
        let mut height = 1;
        let mut dynamic = def.timeline.total_frames > 1;
        for &c in &def.children {
            if let Some(child) = self.get(c) {
                height = height.max(child.height + 1);
                dynamic |= child.dynamic;
            }
        }
        (height.min(MAX_SYMBOL_NEST_DEPTH + 1), dynamic)
    }

    /// テスト用：表と、シンボルが持つもの（子・音・高さ・動くか）が、ライブラリの全部から作り直したもの（お手本）と同じか
    #[cfg(test)]
    pub(crate) fn check_tables(&self) -> Result<(), String> {
        let rebuilt: Library = self.order.iter().filter_map(|s| self.get(s.0)).map(|s| Arc::new(SymbolDef::clone(s))).collect();
        let ids = |l: &ChunkedList<Slot>| l.iter().map(|s| s.0).collect::<Vec<_>>();
        if ids(&self.order) != ids(&rebuilt.order) {
            return Err("見せる順が違う".into());
        }
        let names = |l: &Library| l.names.iter().map(|e| (e.mark, e.id, e.name.clone())).collect::<Vec<_>>();
        if names(self) != names(&rebuilt) {
            return Err("名前の表が違う".into());
        }
        let numbered = |l: &Library| l.numbered.iter().map(|e| (numbered_key(e), e.count)).collect::<Vec<_>>();
        if numbered(self) != numbered(&rebuilt) {
            return Err("番号の付いた名前の表が違う".into());
        }
        let uses = |l: &ChunkedList<Use>| l.iter().map(|u| use_key(u)).collect::<Vec<_>>();
        if uses(&self.users) != uses(&rebuilt.users) {
            return Err("シンボルの使われ方の表が違う".into());
        }
        if uses(&self.sound_users) != uses(&rebuilt.sound_users) {
            return Err("音の使われ方の表が違う".into());
        }
        if self.too_deep != rebuilt.too_deep {
            return Err(format!("上限を超えた数が違う：{} と {}", self.too_deep, rebuilt.too_deep));
        }
        // 塊の要約から作った子・音を、タイムラインの全部のキーフレームを1つずつ見た答えとも比べる（要約が古いままでないか）
        for d in self.defs.iter() {
            let mut children: Vec<u64> = d.timeline.instances().map(|i| i.binding.symbol_id).collect();
            children.sort_unstable();
            children.dedup();
            let mut sounds: Vec<u64> = d.timeline.sound_clips().map(|c| c.sound).collect();
            sounds.sort_unstable();
            sounds.dedup();
            if (children, sounds) != (d.children.clone(), d.sounds.clone()) {
                return Err(format!("シンボル {} の子・音が、全部のキーフレームを見た答えと違う", d.id));
            }
        }
        for (a, b) in self.defs.iter().zip(rebuilt.defs.iter()) {
            if (a.id, &a.children, &a.sounds, a.height, a.dynamic) != (b.id, &b.children, &b.sounds, b.height, b.dynamic) {
                return Err(format!("シンボル {} の子・音・高さ・動くかが違う", a.id));
            }
        }
        Ok(())
    }
}

/// 見せる順に並んだシンボルから作る（読み込み・テスト）。表とシンボルが持つものは、みな全部から作る
impl FromIterator<Arc<SymbolDef>> for Library {
    fn from_iter<I: IntoIterator<Item = Arc<SymbolDef>>>(iter: I) -> Self {
        let mut defs: Vec<Arc<SymbolDef>> = iter.into_iter().collect();
        let order: ChunkedList<Slot> = defs.iter().map(|d| Arc::new(Slot(d.id))).collect();
        for d in &mut defs {
            let refs = d.timeline.refs();
            let d = Arc::make_mut(d);
            d.children = refs.symbols;
            d.sounds = refs.sounds;
            d.height = 0;
        }
        defs.sort_by_key(|d| d.id);
        defs.dedup_by_key(|d| d.id);
        let mut names: Vec<Arc<NameEntry>> =
            defs.iter().map(|d| Arc::new(NameEntry { mark: name_mark(&d.name), id: d.id, name: d.name.clone() })).collect();
        names.sort_by_key(|e| (e.mark, e.id));
        let mut numbered: Vec<Numbered> = Vec::new();
        for d in &defs {
            for form in [NumberForm::Space, NumberForm::Paren] {
                if let Some((stem, n)) = form.parse(&d.name) {
                    numbered.push(Numbered { form, stem: stem.to_owned(), n, count: 1 });
                }
            }
        }
        numbered.sort_by_key(numbered_key);
        let mut merged: Vec<Arc<Numbered>> = Vec::with_capacity(numbered.len());
        for e in numbered {
            match merged.last_mut() {
                Some(last) if numbered_key(last) == numbered_key(&e) => Arc::make_mut(last).count += 1,
                _ => merged.push(Arc::new(e)),
            }
        }
        let table = |of: &dyn Fn(&SymbolDef) -> &Vec<u64>| {
            let mut all: Vec<Arc<Use>> =
                defs.iter().flat_map(|d| of(d).iter().map(|&used| Arc::new(Use { used, user: d.id }))).collect();
            all.sort_by_key(|u| use_key(u));
            all.into_iter().collect::<ChunkedList<Use>>()
        };
        let users = table(&|d| &d.children);
        let sound_users = table(&|d| &d.sounds);
        let mut lib = Library {
            defs: defs.into_iter().collect(),
            order,
            names: names.into_iter().collect(),
            numbered: merged.into_iter().collect(),
            users,
            sound_users,
            too_deep: 0,
        };
        // 高さ・動くか：子から先に作る（深さ優先。たどっている途中の子に戻ったら輪：上限＋1にする）
        let ids: Vec<u64> = lib.defs.iter().map(|d| d.id).collect();
        let mut done: HashSet<u64> = HashSet::new();
        for id in ids {
            lib.settle(id, &mut done, &mut Vec::new());
        }
        lib.too_deep = lib.defs.iter().filter(|d| d.height > MAX_SYMBOL_NEST_DEPTH).count();
        lib
    }
}

impl Library {
    /// 読み込みで、シンボル `id` の高さ・動くかを、子から先に作る（`path` はたどっている途中のシンボル）
    fn settle(&mut self, id: u64, done: &mut HashSet<u64>, path: &mut Vec<u64>) {
        if done.contains(&id) {
            return;
        }
        let Some(i) = self.defs.position_sorted(id, |s| s.id) else {
            return;
        };
        if path.contains(&id) {
            // 輪：上限を超えたものにする
            Arc::make_mut(&mut self.defs[i]).height = MAX_SYMBOL_NEST_DEPTH + 1;
            return;
        }
        path.push(id);
        for c in self.defs[i].children.clone() {
            self.settle(c, done, path);
        }
        path.pop();
        let (height, dynamic) = self.computed(&self.defs[i]);
        let def = Arc::make_mut(&mut self.defs[i]);
        def.height = def.height.max(height);
        def.dynamic = dynamic;
        done.insert(id);
    }
}
