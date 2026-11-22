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

/// 音のライブラリの1つの音（写すのは名前だけ：バイトと波形は写しと共有する）
#[derive(Clone, Debug)]
pub(crate) struct Sound {
    pub(crate) id: u64,
    /// 読み込んだファイルの名前（ライブラリに出す）
    pub(crate) name: String,
    /// 読み込んだ WAV・MP3 のバイトのまま（保存するのはこれ）
    pub(crate) encoded: Arc<[u8]>,
    /// 鳴らすための波形（`encoded` から作った写し。保存しない）
    pub(crate) pcm: Arc<Pcm>,
    /// 波形の絵のための、音の中の位置 `PEAK_UNITS` ごとの、左右の値のいちばん小さいもの・大きいもの（`pcm` から1回だけ
    /// 作る写し。保存しない）。サンプルが1つも入らない所は、小さいほうが大きいほうより大きい（値が無い）
    peaks: Arc<Vec<(i16, i16)>>,
}

/// 波形の絵の値の幅を持つ長さ（`SOUND_RATE` の単位）：タイムラインの画面の 1px に当たる音のいちばん短い長さ。1コマの
/// 幅 `CELL_W` に、いちばん短いコマ（fps の上限 `MAX_FPS` の1コマ）が入るので、1px は 1 / (`MAX_FPS` × `CELL_W`) 秒
/// （60fps・16px で 50）。これより細かく持っても画面に出ない
const PEAK_UNITS: u64 =
    SOUND_RATE / (crate::symbol::MAX_FPS as u64 * crate::timeline_ui::CELL_W as u64);

impl Sound {
    /// 番号 `id`・名前 `name` の、バイト `encoded`（WAV か MP3）の音。波形にできなければ `Err`（壊れた音・読めない形式）
    pub(crate) fn new(id: u64, name: String, encoded: Arc<[u8]>) -> Result<Self, String> {
        let pcm = decode(&encoded)?;
        let mut peaks: Vec<(i16, i16)> = Vec::new();
        for (i, lr) in pcm.samples.chunks_exact(2).enumerate() {
            let b = (i as u64 * SOUND_RATE / u64::from(pcm.rate) / PEAK_UNITS) as usize;
            if b >= peaks.len() {
                peaks.resize(b + 1, (i16::MAX, i16::MIN));
            }
            let p = &mut peaks[b];
            *p = (p.0.min(lr[0]).min(lr[1]), p.1.max(lr[0]).max(lr[1]));
        }
        Ok(Self {
            id,
            name,
            encoded,
            pcm: Arc::new(pcm),
            peaks: Arc::new(peaks),
        })
    }

    /// 音の中の位置 `a` から `b` の前まで（`SOUND_RATE` の単位。`a < b`）の、左右の値のいちばん小さいもの・大きいもの
    /// （-1〜1）。波形の絵に使う（`PEAK_UNITS` ごとに持った値から：範囲にかかる所を全部見る）。値が無い所は `None`
    pub(crate) fn peak(&self, a: u64, b: u64) -> Option<(f32, f32)> {
        let first = (a / PEAK_UNITS) as usize;
        let last = (b.div_ceil(PEAK_UNITS) as usize).min(self.peaks.len());
        let (lo, hi) = self
            .peaks
            .get(first..last)?
            .iter()
            .fold((i16::MAX, i16::MIN), |(lo, hi), &(l, h)| {
                (lo.min(l), hi.max(h))
            });
        (lo <= hi).then(|| (f32::from(lo) / 32768.0, f32::from(hi) / 32768.0))
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

/// コマ数 `frames` を、作品の fps `fps` で、音の中の位置の単位（`SOUND_RATE`）にしたもの（いちばん近い整数）。コマと音の
/// 位置を結ぶ決まりはこれ1つ（鳴らす位置 `SoundClip::pos_at`・分割 `SoundClip::advanced`・端のドラッグ）
pub(crate) fn frames_to_sound(frames: usize, fps: u32) -> u64 {
    let fps = u64::from(fps.max(1));
    (frames as u64 * SOUND_RATE * 2 + fps) / (2 * fps)
}

/// 音の中の長さ `units`（`SOUND_RATE` の単位）に届く、いちばん少ないコマ数：`frames_to_sound(n, fps) >= units` になる
/// いちばん小さい `n`（`frames_to_sound` の逆）。R を `SOUND_RATE`、f を fps、L を `units` とすると、
/// `frames_to_sound(n) = ⌊(2nR + f) / 2f⌋ ≥ L` は `2nR + f ≥ 2fL`、つまり `n ≥ f(2L − 1) / 2R` と同じなので、その切り上げ
/// （L が 0 なら 0 コマ）
pub(crate) fn frames_to_reach(units: u64, fps: u32) -> usize {
    let fps = u64::from(fps.max(1));
    units.checked_sub(1).map_or(0, |l1| {
        (fps * (2 * l1 + 1)).div_ceil(2 * SOUND_RATE) as usize
    })
}

/// 音の区間の端（タイムラインの波形の左端・右端のドラッグで動かす所）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SoundEdge {
    /// 始まり（左端）
    Start,
    /// 終わり（右端）
    End,
}

impl SoundClip {
    /// この区間が、キーフレームの始まりから鳴るコマの数（区間の終わりに届くまで：`frames_to_reach`）
    pub(crate) fn frames(&self, fps: u32) -> usize {
        frames_to_reach(self.end - self.start, fps)
    }

    /// キーフレームの始まりから `frames` コマ進んだコマの始まりでの、音の中の位置。鳴るコマ（`frames`）を過ぎていれば
    /// `None`。鳴るかどうかの決まりはこれ1つ（鳴らす `sounding_at`・分割 `advanced`・波形の絵）
    pub(crate) fn pos_at(&self, frames: usize, fps: u32) -> Option<u64> {
        (frames < self.frames(fps)).then(|| self.start + frames_to_sound(frames, fps))
    }

    /// この区間を、キーフレームの始まりから `frames` コマ進んだ所で分けた後ろ（同じ音の続きから鳴らす：F6 の分割）。
    /// 進んだ所が終わり以降なら `None`（後ろに鳴らす音は無い）
    pub(crate) fn advanced(self, frames: usize, fps: u32) -> Option<Self> {
        self.pos_at(frames, fps).map(|start| Self { start, ..self })
    }

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

// ============================================================================
// いま、どの音のどこを鳴らすか（再生・スクラブ・書き出しのどれもが使う決まり：設計の 4 章）
// ============================================================================

/// コマ1つの間に鳴っている音1つ：どこから来た音か `key`（レイヤーの番号と、たどったインスタンスの番号の並び。
/// 同じシンボルを2つ置けば別の音）、音、そのコマの始まりでの音の中の位置 `pos` と、音の終わり `end`（`SOUND_RATE` の単位）
#[derive(Clone, Debug)]
pub(crate) struct Voice {
    pub(crate) key: Vec<u64>,
    pub(crate) sound: Arc<Sound>,
    pub(crate) pos: u64,
    pub(crate) end: u64,
}

/// 文書 `doc` のタイムライン `tl`（ルートか、編集中のシンボルの：画面が再生するもの）のコマ `frame` で鳴っている音
/// （`Voice`）。表示中のレイヤーの、そのコマを含むキーフレームの音と、そのコマに置かれたシンボルの中の音（中のコマは絵と
/// 同じ `resolve_internal_frame` で決める。入れ子は描くのと同じく `MAX_SYMBOL_NEST_DEPTH` 段まで）。位置は「区間の始まり ＋
/// キーフレームの始まりから進んだコマ」（`frames_to_sound`）で、区間の終わりから後は入れない
pub(crate) fn sounding_at(
    doc: &crate::symbol::Document,
    tl: &crate::timeline::Timeline,
    frame: usize,
) -> Vec<Voice> {
    let mut out = Vec::new();
    collect_voices(doc, tl, frame, &mut Vec::new(), 0, &mut out);
    out
}

fn collect_voices(
    doc: &crate::symbol::Document,
    tl: &crate::timeline::Timeline,
    frame: usize,
    path: &mut Vec<u64>,
    depth: usize,
    out: &mut Vec<Voice>,
) {
    for layer in tl.layers.iter().filter(|l| l.visible) {
        let Some(k) = layer.keyframe_index_at(frame) else {
            continue;
        };
        let kf_start = layer.start_of(k);
        let kf = &layer.keyframes[k];
        path.push(layer.id);
        if let Some(c) = kf.sound {
            let pos = c.pos_at(frame - kf_start, doc.fps);
            let sound = doc.sounds.iter().find(|s| s.id == c.sound);
            if let (Some(pos), Some(sound)) = (pos, sound) {
                out.push(Voice {
                    key: path.clone(),
                    sound: Arc::clone(sound),
                    pos,
                    end: c.end,
                });
            }
        }
        if depth < crate::symbol::MAX_SYMBOL_NEST_DEPTH {
            // トゥイーンの途中は、絵と同じく、そのコマの結び付けのインスタンスだけ
            let bindings: Vec<crate::vector_engine::SymbolInstanceBinding> =
                match layer.interpolated_tween_binding_at(frame) {
                    Some(b) => vec![b],
                    None => kf.cel.instances.iter().map(|i| i.binding).collect(),
                };
            for b in bindings {
                let Some(sym) = doc.library.iter().find(|s| s.id == b.symbol_id) else {
                    continue;
                };
                if let Some(inner) = sym.playing_internal_frame(b, kf_start, frame) {
                    path.push(b.instance_gid);
                    collect_voices(doc, &sym.timeline, inner, path, depth + 1, out);
                    path.pop();
                }
            }
        }
        path.pop();
    }
}

/// 音を切り替える所（鳴り始め・鳴り終わり・位置が飛んだ所）を、なめらかにつなぐ長さ（秒）。波形が急に途切れると
/// 「ぷつっ」と鳴るのを防ぐ。耳にそれと分からない短さで、どの fps の1コマ（60fps で約 17ms）よりも十分短い
const FADE_SECONDS: f64 = 0.002;

/// 前のコマの音 `prev` から、このコマの音 `cur` へ、途切れずに続くか：同じ所から来た同じ音で、位置が1コマ分だけ進んで
/// いる。位置はコマごとに `frames_to_sound` でいちばん近い整数に丸めるので、1コマ分の進みは、ちょうどの値から 1 まで
/// ずれうる（丸めのずれは1つにつき 0.5 まで）。F6 で分けた所も、分ける前と同じく続く
fn continues(prev: &Voice, cur: &Voice, fps: u32) -> bool {
    let step = SOUND_RATE as f64 / f64::from(fps.max(1));
    prev.key == cur.key
        && prev.sound.id == cur.sound.id
        && (cur.pos as f64 - (prev.pos as f64 + step)).abs() <= 1.0
}

/// 音 `s` の、音の中の位置 `p`（`SOUND_RATE` の単位。小数も）の左右の値（-1〜1）。元のサンプルの速さに合わせて、
/// となりの2つのサンプルの間を直線でつなぐ（速さを合わせる決まりはここ1つ）。音の外は 0
fn sample_at(s: &Sound, p: f64) -> (f32, f32) {
    let x = p * f64::from(s.pcm.rate) / SOUND_RATE as f64;
    if x < 0.0 {
        return (0.0, 0.0);
    }
    let i = x.floor() as usize;
    let f = (x - i as f64) as f32;
    let at = |k: usize| -> (f32, f32) {
        match s.pcm.samples.get(2 * k..2 * k + 2) {
            Some(lr) => (f32::from(lr[0]) / 32768.0, f32::from(lr[1]) / 32768.0),
            None => (0.0, 0.0),
        }
    };
    let (a, b) = (at(i), at(i + 1));
    (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)
}

/// 出力の速さ `rate`（1 秒あたりのサンプルの数）で、タイムラインのコマ 0 の始まりから数えて `sample` 番目のサンプルが
/// 入るコマ（作品の fps `fps`）：⌊`sample` × fps / `rate`⌋。サンプルとコマを結ぶ決まりはこれと `frame_start` の2つだけ
/// （混ぜる `mix`・再生のコマ・鳴らす範囲）
pub(crate) fn frame_at(sample: u64, rate: u32, fps: u32) -> usize {
    (sample * u64::from(fps.max(1)) / u64::from(rate.max(1))) as usize
}

/// コマ `frame` の最初のサンプル：`frame_at` がそのコマになる、いちばん小さいサンプルの番号（⌈`frame` × `rate` / fps⌉）
pub(crate) fn frame_start(frame: usize, rate: u32, fps: u32) -> u64 {
    (frame as u64 * u64::from(rate.max(1))).div_ceil(u64::from(fps.max(1)))
}

/// 文書 `doc` のタイムライン `tl`（`sounding_at` と同じ）の、続けて鳴らすコマの範囲 `frames` の音を、出力の速さ `rate`
/// （1 秒あたりのサンプルの数）で、コマ 0 の始まりから数えて `start` 番目のサンプルから `count` 個分混ぜた波形（左・右・
/// 左・右…）。どのサンプルの値も、その番号と `frames` だけで決まる（区間をどう分けて作っても同じ波形になる）。
/// 再生・スクラブ・途中からの再生・くり返しの戻り・書き出しのどれもが使う、混ぜる決まりの1つ：コマごとに
/// 鳴っている音（`sounding_at`）を足し、前のコマから続かない音は鳴り始めを、次のコマへ続かない音は鳴り終わり（区間の
/// 終わりか、コマの終わりの早いほう）を、`FADE_SECONDS` でなめらかにする（F6 で分けた所は続くので、なめらかにしない）。
/// `frames` の最初のコマの前と、最後のコマの後は鳴らさないので、そこの音は「無い」とする。足した結果が -1〜1 を越える所は
/// そこで切る
pub(crate) fn mix(
    doc: &crate::symbol::Document,
    tl: &crate::timeline::Timeline,
    frames: std::ops::Range<usize>,
    start: u64,
    count: usize,
    rate: u32,
) -> Vec<f32> {
    let mut out = vec![0.0f32; 2 * count];
    let fps = doc.fps.max(1);
    let rate_f = f64::from(rate.max(1));
    let mut cache: std::collections::HashMap<usize, Vec<Voice>> = Default::default();
    let mut voices = |f: usize| -> Vec<Voice> {
        cache
            .entry(f)
            .or_insert_with(|| sounding_at(doc, tl, f))
            .clone()
    };
    let fade_units = FADE_SECONDS * SOUND_RATE as f64;
    let mut i = 0usize;
    while i < count {
        let f = frame_at(start + i as u64, rate, fps);
        // このコマに入るサンプルの範囲（次のコマの最初のサンプルの手前まで）
        let j = (frame_start(f + 1, rate, fps) - start).min(count as u64) as usize;
        let cur = voices(f);
        let prev = if f > frames.start {
            voices(f - 1)
        } else {
            Vec::new()
        };
        let next = if f + 1 < frames.end {
            voices(f + 1)
        } else {
            Vec::new()
        };
        let frame_t0 = f as f64 / f64::from(fps);
        let frame_t1 = (f + 1) as f64 / f64::from(fps);
        for v in &cur {
            let fade_in = !prev.iter().any(|p| continues(p, v, fps));
            let fade_out = !next.iter().any(|n| continues(v, n, fps));
            for (k, o) in out.chunks_exact_mut(2).enumerate().take(j).skip(i) {
                let t = (start + k as u64) as f64 / rate_f;
                let p = v.pos as f64 + (t - frame_t0) * SOUND_RATE as f64;
                if p >= v.end as f64 {
                    break;
                }
                let mut g = 1.0f64;
                if fade_in {
                    g = g.min((t - frame_t0) * SOUND_RATE as f64 / fade_units);
                }
                if fade_out {
                    // 次のコマへ続かない：区間の終わりか、このコマの終わりの早いほうでなめらかに止める
                    g = g
                        .min((v.end as f64 - p) / fade_units)
                        .min((frame_t1 - t) * SOUND_RATE as f64 / fade_units);
                }
                let g = g.clamp(0.0, 1.0) as f32;
                let (l, r) = sample_at(&v.sound, p);
                o[0] += l * g;
                o[1] += r * g;
            }
        }
        i = j;
    }
    for x in &mut out {
        *x = x.clamp(-1.0, 1.0);
    }
    out
}
