//! 復旧の書き足すファイル（設計：審査4f8ef46の直しの設計.md の D-3）。保存していない作業の控えを、作品が変わるたびに
//! 書き直すが、作品全体は書かない：最初に1回だけ全体を書き、そのあとは、新しくできた地図の塊・書き換えたキーフレームの
//! 塊（`ChunkedList` の塊。64 コマほど）・新しい音と、目録の書き換えた段だけを書き足す（目録は入れ子：作品 → レイヤーの
//! キーフレームの並び・ライブラリの塊 → キーフレームの塊。書き換えていない段は、前に書いた記録を番号で指す）。書き足した量が、全体を書いた
//! 量を超えたら、全体を書き直す（ならすと、1回の書き込みは触った所の大きさで決まる）。読むときは、書き足した記録を順に
//! 読み、最後の目録から保存の形（`ProjectDocument`）を組み立てて、ふつうの読み込みの道（`into_document_noting`）に渡す。
//!
//! ファイルは、頭の印のあとに記録の列：記録ごとに（長さ・CRC32・postcard の中身）。書いている途中で止まると最後の記録
//! が欠けるが、欠けた記録から後は読まない（最後の完全な目録までで組み立てる）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::chunked_list::{Chunk, ChunkedList, Keyed};
use crate::project_io::{
    file_identity, file_kind, read_identity, ProjectDocument, SaveKind, SavedKeyframe, SavedLayer, SavedSound, SavedSymbolDef,
    SavedTimeline,
};
use crate::sound::{Sound, SoundLibrary};
use crate::stored_map::{ReadTiles, SavedStroke, SavedTile, WriteTiles};
use crate::symbol::{Document, Slot, SymbolDef};
use crate::timeline::{Keyframe, Timeline};


/// 記録1つ。目録は入れ子で、どの段も、書き換えていない所は前に書いた記録を番号で指す（番号は書いた順の通し番号）
#[derive(Serialize, Deserialize)]
enum Record {
    /// 塊の表の続き（色・塊・その塊の線。番号は前の記録の続き）
    Tiles {
        colors: Vec<[u8; 4]>,
        tiles: Vec<SavedTile>,
        strokes: Vec<SavedStroke>,
    },
    /// 音（音のライブラリの1つ）：番号・名前と、どのデータか（データは `SoundBytes` で1回だけ書く）
    Sound(SoundEntry),
    /// 音のライブラリ（番号 `id`）：音の番号の並び
    Sounds { id: u64, sounds: Vec<u64> },
    /// キーフレームの塊（番号 `id`）：塊の基準からの始まりのコマ `rels` と、キーフレーム（始まりのコマは使わない）
    Chunk {
        id: u64,
        rels: Vec<i64>,
        keyframes: Vec<SavedKeyframe>,
    },
    /// レイヤーのキーフレームの並び（番号 `id`）：キーフレームの塊の番号と、その塊の基準
    Keyframes { id: u64, chunks: Vec<(u64, i64)> },
    /// ライブラリの塊（番号 `id`）：シンボル（番号の順）
    Symbols { id: u64, symbols: Vec<JournalSymbol> },
    /// ライブラリの見せる順の塊（番号 `id`）：シンボルの番号
    Order { id: u64, ids: Vec<u64> },
    /// 目録（作品の一番上）。最後の目録が今の作品
    Manifest(Manifest),
    /// 保存した（最後の記録がこれなら、復元するものは無い：`is_saved`）
    Saved,
    /// 音のデータ（番号 `id`。WAV・MP3 のバイトのまま）。同じデータは1回だけ書く（名前を変えても書き直さない）
    SoundBytes { id: u64, bytes: Arc<[u8]> },
}

/// 音の記録の形：音の番号・名前・データの記録の番号
#[derive(Serialize, Deserialize)]
struct SoundEntry {
    id: u64,
    name: String,
    bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    stage_size: [f32; 2],
    fps: u32,
    next_symbol_id: u64,
    next_group_id: u64,
    next_sound_id: u64,
    next_layer_id: u64,
    recovery_source: Option<PathBuf>,
    timeline: JournalTimeline,
    /// ライブラリの塊の番号の並びと、見せる順の塊の番号の並び
    library: Vec<u64>,
    order: Vec<u64>,
    /// 音のライブラリの番号
    sounds: u64,
}

#[derive(Serialize, Deserialize)]
struct JournalSymbol {
    id: u64,
    name: String,
    timeline: JournalTimeline,
}

/// タイムラインの目録の形：レイヤーを除いた保存の形（`SavedTimeline::header`）と、レイヤー
#[derive(Serialize, Deserialize)]
struct JournalTimeline {
    header: SavedTimeline,
    layers: Vec<JournalLayer>,
}

/// レイヤーの目録の形：キーフレームを除いた保存の形（`SavedLayer::header`）と、キーフレームの並びの番号
#[derive(Serialize, Deserialize)]
struct JournalLayer {
    header: SavedLayer,
    keyframes: u64,
}

/// 書いたもの（番地 → 番号）。番地の持ち主の写しを一緒に持つので、持っている間は、番地がほかのものに使い回されない
struct Written<T> {
    ids: HashMap<usize, (u64, T)>,
}

impl<T> Default for Written<T> {
    fn default() -> Self {
        Self { ids: HashMap::new() }
    }
}

/// 1つのタブの書き足すファイル
pub(crate) struct Journal {
    path: PathBuf,
    /// 書いた地図の塊の表（番号はファイルの中で続く）と、書いた塊の数・色の数
    tiles: WriteTiles,
    done: (usize, usize),
    /// 書いた音のデータ（番地は `Sound::bytes_address`）・音・音のライブラリ・キーフレームの塊・キーフレームの並び・
    /// ライブラリの塊
    sound_bytes: Written<Arc<[u8]>>,
    sounds: Written<Arc<Sound>>,
    sound_lists: Written<SoundLibrary>,
    chunks: Written<Chunk<Keyframe>>,
    keyframe_lists: Written<ChunkedList<Keyframe>>,
    symbol_chunks: Written<Chunk<SymbolDef>>,
    order_chunks: Written<Chunk<Slot>>,
    /// 次に振る番号
    next_id: u64,
    /// 全体を書いた量と、そのあと書き足した量（バイト）
    snapshot: u64,
    appended: u64,
    /// 最後に書いたのが「保存した」の印か
    saved: bool,
}

impl Journal {
    /// `path` に、文書 `doc`（元のファイル `source`）の全体を書いたファイルを作る（同じフォルダの一時ファイル `tmp` に
    /// 書いてから置き換える）
    pub(crate) fn create(path: &Path, doc: &Document, source: Option<&Path>) -> std::io::Result<Self> {
        let mut journal = Self {
            path: path.to_path_buf(),
            tiles: WriteTiles::default(),
            done: (0, 0),
            sound_bytes: Written::default(),
            sounds: Written::default(),
            sound_lists: Written::default(),
            chunks: Written::default(),
            keyframe_lists: Written::default(),
            symbol_chunks: Written::default(),
            order_chunks: Written::default(),
            next_id: 1,
            snapshot: 0,
            appended: 0,
            saved: false,
        };
        // 頭（種類と版）と全体を一時ファイルに書いてから置き換える（作品ファイルと同じ頭と置き換え方：`file_identity`・
        // `write_atomically`）
        crate::project_io::write_atomically(path, |file| {
            let mut out = std::io::BufWriter::new(file);
            out.write_all(&file_identity(SaveKind::Journal))?;
            journal.snapshot = journal.write_records(&mut out, doc, source)?;
            out.into_inner().map_err(|e| e.into_error())
        })?;
        Ok(journal)
    }

    /// 文書 `doc` の今を書く：前に書いてから変わった所だけを書き足す。書き足した量が全体を書いた量を超えたら、全体を
    /// 書き直す（一時ファイル `tmp` に書いてから置き換える。覚えている書いたものも手放す）
    pub(crate) fn write(&mut self, doc: &Document, source: Option<&Path>) -> std::io::Result<()> {
        if self.appended > self.snapshot {
            *self = Self::create(&self.path, doc, source)?;
            return Ok(());
        }
        self.append(|j, out| j.write_records(out, doc, source))?;
        self.saved = false;
        Ok(())
    }

    /// 「保存した」の印を書き足す（もう書いてあれば何もしない）
    pub(crate) fn mark_saved(&mut self) -> std::io::Result<()> {
        if !self.saved {
            self.append(|_, out| write_record(out, &Record::Saved))?;
            self.saved = true;
        }
        Ok(())
    }

    /// ファイルの終わりに、`records` が書く記録を書き足す
    fn append(&mut self, records: impl FnOnce(&mut Self, &mut std::io::BufWriter<std::fs::File>) -> std::io::Result<u64>) -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new().append(true).open(&self.path)?;
        let mut out = std::io::BufWriter::new(file);
        let n = records(self, &mut out)?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_data()?;
        self.appended += n;
        Ok(())
    }

    /// ファイルの場所
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// 新しく振る番号
    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }

    /// 前に書いてから変わった所の記録と目録を `out` に書く。書いたバイト数。手間は、変わった所（新しい塊・書き換えた
    /// 並び）と、作品の一番上の段（タイムラインのレイヤー・ライブラリの塊の数）で決まる
    fn write_records(&mut self, out: &mut impl Write, doc: &Document, source: Option<&Path>) -> std::io::Result<u64> {
        let mut written = 0;
        let sounds = self.sound_list(out, &doc.sounds, &mut written)?;
        let timeline = self.timeline(out, &doc.root, &mut written)?;
        let mut library = Vec::new();
        for c in doc.library().def_chunks() {
            library.push(self.symbol_chunk(out, c, &mut written)?);
        }
        let mut order = Vec::new();
        for c in doc.library().order_chunks() {
            order.push(self.order_chunk(out, c, &mut written)?);
        }
        let manifest = Manifest {
            stage_size: [doc.stage_size.x, doc.stage_size.y],
            fps: crate::project_io::saved_fps(doc),
            next_symbol_id: doc.next_symbol_id,
            next_group_id: doc.next_group_id,
            next_sound_id: doc.next_sound_id,
            next_layer_id: doc.next_layer_id,
            recovery_source: source.map(Path::to_path_buf),
            timeline,
            library,
            order,
            sounds,
        };
        written += write_record(out, &Record::Manifest(manifest))?;
        Ok(written)
    }

    /// 音のライブラリ `lib` の番号（まだ書いていなければ、新しい音と並びを書く）
    fn sound_list(&mut self, out: &mut impl Write, lib: &SoundLibrary, written: &mut u64) -> std::io::Result<u64> {
        let key = lib.address();
        if let Some((id, _)) = self.sound_lists.ids.get(&key) {
            return Ok(*id);
        }
        for s in lib.iter() {
            let k = Arc::as_ptr(s) as usize;
            if self.sounds.ids.contains_key(&k) {
                continue;
            }
            let bytes = match self.sound_bytes.ids.get(&s.bytes_address()) {
                Some((id, _)) => *id,
                None => {
                    let id = self.new_id();
                    *written += write_record(out, &Record::SoundBytes { id, bytes: s.encoded.clone() })?;
                    self.sound_bytes.ids.insert(s.bytes_address(), (id, s.encoded.clone()));
                    id
                }
            };
            *written += write_record(out, &Record::Sound(SoundEntry { id: s.id, name: s.name.clone(), bytes }))?;
            self.sounds.ids.insert(k, (s.id, s.clone()));
        }
        let id = self.new_id();
        *written += write_record(out, &Record::Sounds { id, sounds: lib.iter().map(|s| s.id).collect() })?;
        self.sound_lists.ids.insert(key, (id, lib.clone()));
        Ok(id)
    }

    /// ライブラリの塊 `c` の番号（まだ書いていなければ、シンボルのタイムラインの変わった所と、塊を書く）
    fn symbol_chunk(&mut self, out: &mut impl Write, c: &Chunk<SymbolDef>, written: &mut u64) -> std::io::Result<u64> {
        let key = Arc::as_ptr(c) as usize;
        if let Some((id, _)) = self.symbol_chunks.ids.get(&key) {
            return Ok(*id);
        }
        let mut symbols = Vec::with_capacity(c.len());
        for sym in c.iter() {
            let timeline = self.timeline(out, &sym.timeline, written)?;
            symbols.push(JournalSymbol { id: sym.id, name: sym.name.clone(), timeline });
        }
        let id = self.new_id();
        *written += write_record(out, &Record::Symbols { id, symbols })?;
        self.symbol_chunks.ids.insert(key, (id, c.clone()));
        Ok(id)
    }

    /// ライブラリの見せる順の塊 `c` の番号（まだ書いていなければ書く）
    fn order_chunk(&mut self, out: &mut impl Write, c: &Chunk<Slot>, written: &mut u64) -> std::io::Result<u64> {
        let key = Arc::as_ptr(c) as usize;
        if let Some((id, _)) = self.order_chunks.ids.get(&key) {
            return Ok(*id);
        }
        let id = self.new_id();
        *written += write_record(out, &Record::Order { id, ids: c.iter().map(|s| s.0).collect() })?;
        self.order_chunks.ids.insert(key, (id, c.clone()));
        Ok(id)
    }

    /// タイムライン `tl` の目録の形（レイヤーのキーフレームの並びのうち、まだ書いていないものを書く）
    fn timeline(&mut self, out: &mut impl Write, tl: &Timeline, written: &mut u64) -> std::io::Result<JournalTimeline> {
        let mut layers = Vec::with_capacity(tl.layers.len());
        for l in &tl.layers {
            let keyframes = self.keyframe_list(out, &l.keyframes, written)?;
            layers.push(JournalLayer { header: SavedLayer::header(l), keyframes });
        }
        Ok(JournalTimeline { header: SavedTimeline::header(tl), layers })
    }

    /// キーフレームの並び `list` の番号（まだ書いていなければ、新しい塊と並びを書く）
    fn keyframe_list(&mut self, out: &mut impl Write, list: &ChunkedList<Keyframe>, written: &mut u64) -> std::io::Result<u64> {
        let key = list.list_address();
        if let Some((id, _)) = self.keyframe_lists.ids.get(&key) {
            return Ok(*id);
        }
        let mut chunks = Vec::new();
        for (c, base) in list.chunks_with_bases() {
            chunks.push((self.chunk(out, c, written)?, base));
        }
        let id = self.new_id();
        *written += write_record(out, &Record::Keyframes { id, chunks })?;
        self.keyframe_lists.ids.insert(key, (id, list.clone()));
        Ok(id)
    }

    /// キーフレームの塊 `c` の番号（まだ書いていなければ、その塊と、塊が使う新しい地図の塊を書く。地図の塊は塊ごとに
    /// 書くので、全体を書くときも、一時に持つのは1つの塊が使う分だけ）
    fn chunk(&mut self, out: &mut impl Write, c: &Chunk<Keyframe>, written: &mut u64) -> std::io::Result<u64> {
        let key = Arc::as_ptr(c) as usize;
        if let Some((id, _)) = self.chunks.ids.get(&key) {
            return Ok(*id);
        }
        let rels: Vec<i64> = c.iter().map(|kf| kf.rel()).collect();
        let keyframes: Vec<SavedKeyframe> = c
            .iter()
            .map(|kf| {
                let mut s = SavedKeyframe::new(0, kf);
                for m in &mut s.maps {
                    m.write_tiles(&mut self.tiles);
                }
                s
            })
            .collect();
        let (colors, tiles, strokes) = self.tiles.appended(self.done.0, self.done.1);
        self.done = self.tiles.counts();
        if !tiles.is_empty() || !colors.is_empty() {
            *written += write_record(out, &Record::Tiles { colors, tiles, strokes })?;
        }
        let id = self.new_id();
        *written += write_record(out, &Record::Chunk { id, rels, keyframes })?;
        self.chunks.ids.insert(key, (id, c.clone()));
        Ok(id)
    }
}

/// 記録 `rec` を（長さ・CRC32・中身）で書く。書いたバイト数
fn write_record(out: &mut impl Write, rec: &Record) -> std::io::Result<u64> {
    let body = postcard::to_allocvec(rec).map_err(std::io::Error::other)?;
    let len = u32::try_from(body.len()).map_err(std::io::Error::other)?;
    out.write_all(&len.to_le_bytes())?;
    out.write_all(&crc32fast::hash(&body).to_le_bytes())?;
    out.write_all(&body)?;
    Ok(8 + u64::from(len))
}

/// 書き足すファイル `path` の最後の記録が「保存した」の印か（ファイルの終わりの数バイトだけを見る）
pub(crate) fn is_saved(path: &Path) -> bool {
    use std::io::{Seek, SeekFrom};
    let mut tail = Vec::new();
    if write_record(&mut tail, &Record::Saved).is_err() || file_kind(path) != Ok(SaveKind::Journal) {
        return false;
    }
    let mut got = vec![0u8; tail.len()];
    std::fs::File::open(path)
        .and_then(|mut f| {
            f.seek(SeekFrom::End(-(tail.len() as i64)))?;
            f.read_exact(&mut got)
        })
        .is_ok_and(|()| got == tail)
}

/// 大きな記録（塊の表・キーフレームの塊・音のデータ）の場所：中身の始まり・長さ・CRC32・記録の並びの番号・番号（塊の表は 0）
struct BigRecord {
    at: u64,
    len: u64,
    crc: u32,
    kind: u8,
    id: u64,
}

/// 記録の並びの番号（postcard は列挙の何番目かを先頭に書く。`Record` の並びと同じ）
const TILES: u8 = 0;
const CHUNK: u8 = 3;
const SOUND_BYTES: u8 = 9;

/// 読み手 `r` から、postcard の数（可変長）を1つ読む
fn read_varint(r: &mut impl Read) -> std::io::Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let mut b = [0u8; 1];
        r.read_exact(&mut b)?;
        value |= u64::from(b[0] & 0x7f) << shift;
        if b[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(std::io::Error::other("数が長すぎます"))
}

/// 書き足すファイル `path` を読み、最後の完全な目録の作品の保存の形にする。欠けた記録（書いている途中で止まった）から
/// 後は読まない。目録が1つも無い・目録が無い塊を指すなら `Err`。ファイル全体をメモリに持たない：1回目は記録を順に流して
/// CRC32 を確かめ、小さな記録は読み、大きな記録（塊の表・キーフレームの塊・音のデータ）は場所だけ覚える。2回目に、塊の
/// 表と、最後の目録の作品が指すキーフレームの塊・音のデータだけを、作品ファイルと同じ読み手（`read_streamed`）で読む
/// （置き換わった古い記録は持たない：設計審査 b7208d9 の 6.。前はファイル全体を読み、古い記録まで全部持った）
pub(crate) fn read(path: &Path) -> Result<ProjectDocument<ReadTiles>, String> {
    use std::io::{Seek, SeekFrom};
    let broken = || "復旧ファイルが壊れています".to_owned();
    let failed = |e: std::io::Error| format!("復旧ファイルを読めませんでした: {e}");
    let file = std::fs::File::open(path).map_err(failed)?;
    let size = file.metadata().map_err(failed)?.len();
    let mut file = std::io::BufReader::with_capacity(crate::project_io::READ_CHUNK_LEN, file);
    let id_len = SaveKind::Journal.identity_len();
    let mut head = Vec::with_capacity(id_len);
    (&mut file).take(id_len as u64).read_to_end(&mut head).map_err(failed)?;
    if read_identity(&head)? != SaveKind::Journal {
        return Err(broken());
    }
    let mut sounds: HashMap<u64, SoundEntry> = HashMap::new();
    let mut sound_lists: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut keyframe_lists: HashMap<u64, Vec<(u64, i64)>> = HashMap::new();
    let mut symbol_chunks: HashMap<u64, Vec<JournalSymbol>> = HashMap::new();
    let mut order_chunks: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut big: Vec<BigRecord> = Vec::new();
    let mut manifest = None;
    // 1回目：記録を順に流す（記録の長さは、ファイルの残りより長ければ欠けた記録として止まる：書いてある長さで取らない）
    let mut at = id_len as u64;
    while size.saturating_sub(at) >= 8 {
        let mut frame = [0u8; 8];
        if file.read_exact(&mut frame).is_err() {
            break;
        }
        let len = u64::from(u32::from_le_bytes(frame[..4].try_into().expect("4 バイト")));
        let crc = u32::from_le_bytes(frame[4..8].try_into().expect("4 バイト"));
        if len == 0 || len > size - at - 8 {
            break;
        }
        let mut body = crate::project_io::BodyReader::new((&mut file).take(len));
        let mut kind = [0u8; 1];
        let mut small = Vec::new();
        let read = body.read_exact(&mut kind).and_then(|()| match kind[0] {
            TILES => std::io::copy(&mut body, &mut std::io::sink()).map(|_| 0),
            CHUNK | SOUND_BYTES => {
                read_varint(&mut body).and_then(|id| std::io::copy(&mut body, &mut std::io::sink()).map(|_| id))
            }
            _ => body.read_to_end(&mut small).map(|_| 0),
        });
        let (Ok(id), Ok((got, sum))) = (read, body.finish()) else {
            break;
        };
        if got != len || sum != crc {
            break;
        }
        match kind[0] {
            TILES | CHUNK | SOUND_BYTES => big.push(BigRecord { at: at + 8, len, crc, kind: kind[0], id }),
            _ => {
                let Ok(rec) = postcard::from_bytes::<Record>(&[&kind[..], &small].concat()) else {
                    break;
                };
                match rec {
                    Record::Sound(s) => {
                        sounds.insert(s.id, s);
                    }
                    Record::Sounds { id, sounds: list } => {
                        sound_lists.insert(id, list);
                    }
                    Record::Keyframes { id, chunks: list } => {
                        keyframe_lists.insert(id, list);
                    }
                    Record::Symbols { id, symbols } => {
                        symbol_chunks.insert(id, symbols);
                    }
                    Record::Order { id, ids } => {
                        order_chunks.insert(id, ids);
                    }
                    Record::Manifest(m) => manifest = Some((m, at)),
                    Record::Saved | Record::Tiles { .. } | Record::Chunk { .. } | Record::SoundBytes { .. } => {}
                }
            }
        }
        at += 8 + len;
    }
    let (m, manifest_at) = manifest.ok_or_else(broken)?;
    // 最後の目録の作品が指すキーフレームの塊と音のデータ
    let timelines = std::iter::once(&m.timeline).chain(
        m.library.iter().filter_map(|id| symbol_chunks.get(id)).flatten().map(|s| &s.timeline),
    );
    let wanted_chunks: std::collections::HashSet<u64> = timelines
        .flat_map(|t| t.layers.iter())
        .filter_map(|l| keyframe_lists.get(&l.keyframes))
        .flatten()
        .map(|&(id, _)| id)
        .collect();
    let wanted_bytes: std::collections::HashSet<u64> = sound_lists
        .get(&m.sounds)
        .into_iter()
        .flatten()
        .filter_map(|id| sounds.get(id))
        .map(|s| s.bytes)
        .collect();
    // 2回目：塊の表（番号が続くので、目録までのものを全部）と、指されている大きな記録だけを読む
    let mut tiles = ReadTiles::default();
    let mut chunks: HashMap<u64, (Vec<i64>, Vec<SavedKeyframe>)> = HashMap::new();
    let mut sound_bytes: HashMap<u64, Arc<[u8]>> = HashMap::new();
    for r in big.iter().filter(|r| r.at < manifest_at) {
        let wanted = match r.kind {
            TILES => true,
            CHUNK => wanted_chunks.contains(&r.id),
            _ => wanted_bytes.contains(&r.id),
        };
        if !wanted {
            continue;
        }
        file.seek(SeekFrom::Start(r.at)).map_err(failed)?;
        let streamed = crate::project_io::read_streamed::<Record>((&mut file).take(r.len))?;
        // 1回目に確かめた中身と同じか（2回のあいだにファイルが変わっていても、違う中身を読まない）
        if streamed.len != r.len || streamed.crc != r.crc || streamed.left != 0 {
            return Err(broken());
        }
        let rec = streamed.value.map_err(|_| broken())?;
        match rec {
            Record::Tiles { colors, tiles: t, strokes } => tiles.add(colors, t, &strokes),
            Record::Chunk { id, rels, keyframes } => {
                chunks.insert(id, (rels, keyframes));
            }
            Record::SoundBytes { id, bytes } => {
                sound_bytes.insert(id, bytes);
            }
            _ => return Err(broken()),
        }
    }
    let timeline = |jt: &JournalTimeline| -> Option<SavedTimeline> {
        let mut layers = Vec::with_capacity(jt.layers.len());
        for l in &jt.layers {
            let mut keyframes = Vec::new();
            for (id, base) in keyframe_lists.get(&l.keyframes)? {
                let (rels, kfs) = chunks.get(id)?;
                for (rel, kf) in rels.iter().zip(kfs) {
                    let mut kf = kf.clone();
                    kf.start_frame = usize::try_from(base + rel).ok()?;
                    keyframes.push(kf);
                }
            }
            layers.push(SavedLayer { keyframes, ..l.header.clone() });
        }
        Some(SavedTimeline { layers, ..jt.header.clone() })
    };
    let root = SavedTimeline { fps: m.fps, ..timeline(&m.timeline).ok_or_else(broken)? };
    // シンボルを番号で引けるようにしてから、見せる順に並べる
    let mut by_id: HashMap<u64, &JournalSymbol> = HashMap::new();
    for id in &m.library {
        for s in symbol_chunks.get(id).ok_or_else(broken)? {
            by_id.insert(s.id, s);
        }
    }
    let mut library = Vec::with_capacity(by_id.len());
    for id in &m.order {
        for sym_id in order_chunks.get(id).ok_or_else(broken)? {
            let s = by_id.get(sym_id).ok_or_else(broken)?;
            let timeline = timeline(&s.timeline).ok_or_else(broken)?;
            library.push(SavedSymbolDef { id: s.id, name: s.name.clone(), timeline });
        }
    }
    let sounds = sound_lists
        .get(&m.sounds)
        .ok_or_else(broken)?
        .iter()
        .map(|id| {
            let s = sounds.get(id)?;
            Some(SavedSound { id: s.id, name: s.name.clone(), bytes: sound_bytes.get(&s.bytes)?.clone() })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(broken)?;
    Ok(ProjectDocument {
        stage_size: m.stage_size,
        timeline: root,
        library,
        next_symbol_id: m.next_symbol_id,
        next_group_id: m.next_group_id,
        recovery_source: m.recovery_source,
        map_tiles: tiles,
        sounds,
        next_sound_id: m.next_sound_id,
        next_layer_id: m.next_layer_id,
    })
}
