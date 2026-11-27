//! 復旧の書き足すファイル（1つのタブの控え。設計：審査4f8ef46の直しの設計.md の D-3）。保存していない作業の控えを、作品が
//! 変わるたびに書き直すが、作品全体は書かない：最初に1回だけ全体を書き、そのあとは、変わった所の記録と目録だけを書き足す
//! （記録の形・書き手・読み手は作品ファイルと同じ1つ：`save_format::RecordWriter`・`save_format::read_file`。設計審査 01e4283 の
//! D-3）。書き足した量が、全体を書いた量を超えたら、全体を書き直す（ならすと、1回の書き込みは触った所の大きさで決まる）。
//! ここが持つのは、ファイルの場所と、書いた量と、保存の印だけ

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::save_format::{file_identity, file_kind, saved_mark_bytes, write_atomically, RecordWriter, SaveKind};
use crate::symbol::Document;

/// 1つのタブの書き足すファイル
pub(crate) struct Journal {
    path: PathBuf,
    /// 書き手（書いたものを覚えていて、変わった所だけを書き足す）
    writer: RecordWriter,
    /// 全体を書いた量と、そのあと書き足した量（バイト）
    snapshot: u64,
    appended: u64,
    /// 最後に書いたのが「保存した」の印か
    saved: bool,
}

impl Journal {
    /// `path` に、文書 `doc`（元のファイル `source`）の全体を書いたファイルを作る（一時ファイルに書いてから置き換える：
    /// 作品ファイルと同じ頭と置き換え方 `file_identity`・`write_atomically`）
    pub(crate) fn create(path: &Path, doc: &Document, source: Option<&Path>) -> std::io::Result<Self> {
        let mut writer = RecordWriter::default();
        let mut snapshot = 0;
        write_atomically(path, |file| {
            let mut out = std::io::BufWriter::new(file);
            out.write_all(&file_identity(SaveKind::Journal))?;
            snapshot = writer.write(&mut out, doc, source)?;
            out.into_inner().map_err(|e| e.into_error())
        })?;
        Ok(Self { path: path.to_path_buf(), writer, snapshot, appended: 0, saved: false })
    }

    /// 文書 `doc` の今を書く：前に書いてから変わった所だけを書き足す。書き足した量が全体を書いた量を超えたら、全体を
    /// 書き直す（一時ファイルに書いてから置き換える。覚えている書いたものも手放す）
    pub(crate) fn write(&mut self, doc: &Document, source: Option<&Path>) -> std::io::Result<()> {
        if self.appended > self.snapshot {
            *self = Self::create(&self.path, doc, source)?;
            return Ok(());
        }
        self.append(|writer, out| writer.write(out, doc, source))?;
        self.saved = false;
        Ok(())
    }

    /// 「保存した」の印を書き足す（もう書いてあれば何もしない）
    pub(crate) fn mark_saved(&mut self) -> std::io::Result<()> {
        if !self.saved {
            self.append(|_, out| RecordWriter::write_saved_mark(out))?;
            self.saved = true;
        }
        Ok(())
    }

    /// ファイルの終わりに、`records` が書く記録を書き足す
    fn append(
        &mut self,
        records: impl FnOnce(&mut RecordWriter, &mut std::io::BufWriter<std::fs::File>) -> std::io::Result<u64>,
    ) -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new().append(true).open(&self.path)?;
        let mut out = std::io::BufWriter::new(file);
        let n = records(&mut self.writer, &mut out)?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_data()?;
        self.appended += n;
        Ok(())
    }

    /// ファイルの場所
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// 書き足すファイル `path` の最後の記録が「保存した」の印か（ファイルの終わりの数バイトだけを見る）
pub(crate) fn is_saved(path: &Path) -> bool {
    use std::io::{Seek, SeekFrom};
    let tail = saved_mark_bytes();
    if file_kind(path) != Ok(SaveKind::Journal) {
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
