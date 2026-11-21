//! 音：文書の音のライブラリ（`Document::sounds`）の1つの音と、キーフレームに付ける音の区間（`SoundClip`）。
//! 設計：claude/オーディオの設計_9693dbf.md
//!
//! 音は、読み込んだ WAV・MP3 のバイトをそのまま持ち（保存するのはこれ）、鳴らすための波形はそのバイトから1回だけ作る
//! （`Pcm`。保存しない）。音の中の位置は、どの音でも同じ単位 `SOUND_RATE`（1 秒を 48,000 に分けた番号）で数える。
//! 音の元のサンプルの速さによらず、作品の fps を変えても、切った位置がずれない

use std::sync::Arc;

/// 音の中の位置の単位：1 秒あたりの数（`SoundClip` の `start`・`end`、`Sound::len`）
pub(crate) const SOUND_RATE: u64 = 48_000;

/// 鳴らすための波形：元のサンプルの速さ `rate`（1 秒あたりの数）の、左右2つずつ並べた 16bit の値。読み込むたびに同じ
/// バイトから同じ値を作る（`decode`）
#[derive(Debug, PartialEq)]
pub(crate) struct Pcm {
    pub(crate) rate: u32,
    /// 左・右・左・右…（1つの時点に2つ）
    pub(crate) samples: Vec<i16>,
}

impl Pcm {
    /// 時点の数（左右の組の数）
    pub(crate) fn frames(&self) -> u64 {
        self.samples.len() as u64 / 2
    }
}

/// 音のライブラリの1つの音
#[derive(Debug)]
pub(crate) struct Sound {
    pub(crate) id: u64,
    /// 読み込んだファイルの名前（ライブラリに出す）
    pub(crate) name: String,
    /// 読み込んだ WAV・MP3 のバイトのまま（保存するのはこれ）
    pub(crate) encoded: Arc<[u8]>,
    /// 鳴らすための波形（`encoded` から作った写し。保存しない）
    pub(crate) pcm: Pcm,
}

impl Sound {
    /// 番号 `id`・名前 `name` の、バイト `encoded`（WAV か MP3）の音。波形にできなければ `Err`（壊れた音・読めない形式）
    pub(crate) fn new(id: u64, name: String, encoded: Arc<[u8]>) -> Result<Self, String> {
        let pcm = decode(&encoded)?;
        Ok(Self {
            id,
            name,
            encoded,
            pcm,
        })
    }

    /// 音の長さ（`SOUND_RATE` の単位。終わりの端数は切り捨て）
    pub(crate) fn len(&self) -> u64 {
        self.pcm.frames() * SOUND_RATE / u64::from(self.pcm.rate)
    }
}

/// 文書の音のライブラリ（番号の順。Undo の控えと共有する：書く所だけ `Arc::make_mut` で写す）
pub(crate) type SoundLibrary = Arc<Vec<Arc<Sound>>>;

/// キーフレームに付けた音の区間：音のライブラリの番号 `sound` の音の、`start` から `end` の前まで（`SOUND_RATE` の単位。
/// `start < end ≤ 音の長さ`）を、キーフレームの始まりから鳴らす
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SoundClip {
    pub(crate) sound: u64,
    pub(crate) start: u64,
    pub(crate) end: u64,
}

impl SoundClip {
    /// 音 `s` の頭から終わりまで（読み込んだ・付けたときの区間）。長さが 0 の音は付けられないので `None`
    pub(crate) fn whole(s: &Sound) -> Option<Self> {
        let end = s.len();
        (end > 0).then_some(Self {
            sound: s.id,
            start: 0,
            end,
        })
    }

    /// 音のライブラリ `sounds` に照らして、正しい区間か（番号の音があり、`start < end ≤ 音の長さ`）。区間を受け入れる
    /// 判定はこれ1つ（読み込み・付ける所）
    pub(crate) fn fits(&self, sounds: &[Arc<Sound>]) -> bool {
        sounds
            .iter()
            .find(|s| s.id == self.sound)
            .is_some_and(|s| self.start < self.end && self.end <= s.len())
    }
}

/// WAV・MP3 のバイト `bytes` を波形にする（左右2つずつの 16bit。1つしか無い音は左右に同じ値、3つ以上の音は前の2つ）。
/// 読めない形式・壊れたバイトは `Err`
pub(crate) fn decode(bytes: &[u8]) -> Result<Pcm, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let bad = |what: &str| format!("音を読めません（{what}）");
    let source = std::io::Cursor::new(bytes.to_vec());
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|_| bad("WAV か MP3 ではありません"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| bad("音のトラックがありません"))?;
    let track_id = track.id;
    let rate = track
        .codec_params
        .sample_rate
        .filter(|&r| r > 0)
        .ok_or_else(|| bad("サンプルの速さが分かりません"))?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| bad("対応していない形式です"))?;
    let mut samples: Vec<i16> = Vec::new();
    let mut buf: Option<SampleBuffer<i16>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // 終わりまで読んだ
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(_) => return Err(bad("壊れています")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = decoder.decode(&packet).map_err(|_| bad("壊れています"))?;
        let spec = *decoded.spec();
        if spec.rate != rate {
            return Err(bad("途中でサンプルの速さが変わります"));
        }
        let channels = spec.channels.count();
        if channels == 0 {
            return Err(bad("音の数が 0 です"));
        }
        let b = buf.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if b.capacity() < decoded.frames() * channels {
            *b = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        b.copy_interleaved_ref(decoded);
        for frame in b.samples().chunks_exact(channels) {
            let (l, r) = (frame[0], frame[if channels > 1 { 1 } else { 0 }]);
            samples.push(l);
            samples.push(r);
        }
    }
    if samples.is_empty() {
        return Err(bad("音が入っていません"));
    }
    Ok(Pcm { rate, samples })
}
