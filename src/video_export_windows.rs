//! Windows の動画の書き出し：Media Foundation の書き込む係（`IMFSinkWriter`）で、H.264 の動画と AAC の音を MP4 に書く。
//!
//! - 絵の入口は 32bit の BGRA（`MFVideoFormat_RGB32`。メモリの並びは 青・緑・赤・アルファ）。1行のバイトの数（ストライド）は、
//!   Media Foundation の決まりどおり `MFGetStrideForBitmapInfoHeader` で決め、入口の形の `MF_MT_DEFAULT_STRIDE` に書く。
//!   Media Foundation の決まりでは、ストライドが正なら、メモリの先頭の行が絵のいちばん上の行（上から下へ）、負なら
//!   いちばん下の行（下から上へ）。RGB の形の最小のストライドは負（下から上へ並べる、Windows の画像のふつうの並び）なので、
//!   決まったストライドの符号で並べる向きを決め（`row_offset`）、受け取った RGBA の各行を、その位置に並べる。
//! - 時刻と長さは Media Foundation の単位（100ns）。絵のコマ `n` の時刻は `n`÷fps 秒、音の塊の時刻は、それまでに書いた
//!   左右の組の数÷`EXPORT_AUDIO_RATE` 秒（どちらも端数は切り捨て）。長さは次の時刻との差にする（端数がたまらない）。
//! - Media Foundation の DLL は遅延読み込み（build.rs）なので、Media Foundation の関数を初めて呼ぶ前に、DLL がみな読み込める
//!   ことを確かめる（`ensure_media_foundation_dlls`。無ければ、メディア機能パックが要ると文言を返す）。
//! - 書き込む係は、絵と音を内部でためて時刻の順に並べるので、音は絵の1コマを書くたびに、その時刻まで書けばよい（Mac の
//!   ような「音が進むまで絵を止める」待ちは要らない）。

use std::path::Path;

use windows::core::{GUID, HSTRING};
use windows::Win32::Foundation::{FreeLibrary, E_POINTER, HANDLE};
use windows::Win32::Media::MediaFoundation::{
    eAVEncH264VProfile_High, CODECAPI_AVEncMPVGOPSize, IMFAttributes, IMFByteStream,
    IMFMediaBuffer, IMFMediaType, IMFSinkWriter, MFAudioFormat_AAC, MFAudioFormat_PCM,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFGetStrideForBitmapInfoHeader, MFMediaType_Audio,
    MFMediaType_Video, MFShutdown, MFStartup, MFTranscodeContainerType_MPEG4, MFVideoFormat_H264,
    MFVideoFormat_RGB32, MFVideoInterlace_Progressive, MFSTARTUP_FULL,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

use crate::sound::frame_start;
use crate::video_export::{
    key_frame_interval, rgba_to_bgra, video_bit_rate, AudioSource, VideoEncoder,
    EXPORT_AUDIO_BIT_RATE, EXPORT_AUDIO_CHANNELS, EXPORT_AUDIO_RATE,
};

/// Media Foundation の時刻の単位（100ns）で数えた、1 秒の長さ
const HNS_PER_SECOND: u64 = 10_000_000;

/// 遅延読み込みにした Media Foundation の DLL の名前（セミコロン区切り。並びは build.rs で決める）
const DELAY_LOADED_DLLS: &str = env!("IS2D_DELAY_LOADED_DLLS");

pub(crate) struct WindowsVideoEncoder<'a> {
    writer: IMFSinkWriter,
    /// 絵の流れの番号（`AddStream` が返したもの）
    video_stream: u32,
    /// 音の流れ（音なしで作ったときは無い）
    audio: Option<AudioTrack<'a>>,
    width: u32,
    height: u32,
    fps: u32,
    /// 絵の入口の1行のバイトの数（入口の形の `MF_MT_DEFAULT_STRIDE` に書いたもの。負なら下の行から並べる）
    stride: i32,
    /// 次に書く絵のコマの番号
    frame_index: usize,
    /// Media Foundation と COM。いちばん後ろに置き、上の Media Foundation のもの（`writer`）を手放してから終える
    /// （構造体の中身は、書いた順に手放される）
    _media_foundation: MediaFoundation,
}

/// 音の流れと、そこへ渡す音
struct AudioTrack<'a> {
    /// 音の流れの番号（`AddStream` が返したもの）
    stream: u32,
    /// 続きの音を、コマの順に返すもの
    source: AudioSource<'a>,
    /// 書いた音の、左右の組の数（次に書く音の始まりの時刻。`EXPORT_AUDIO_RATE` 分の1秒の単位）。コマごとの音を
    /// すき間なくつなぐので、コマ `f` までの音を書き終えると `frame_start(f + 1)` になっている
    written: u64,
    /// `source` が音を全部返した
    ended: bool,
}

/// 書き出しのスレッドで始めた COM（`Drop` で `CoUninitialize` を呼んで終える）
struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, String> {
        // SAFETY: 書き出しのスレッドで、COM をマルチスレッドの形で始める。成功したとき（もう始まっていたときの S_FALSE も
        // 含む）だけ `Self` を返し、その `Drop` で1回だけ `CoUninitialize` を呼ぶので、始まりと終わりが対になる
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(failed("COM を始められませんでした"))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: `initialize` が成功したスレッドと同じスレッドで（入口は書き出しのスレッドの中だけで作って使い、
        // 手放す）、1回だけ終える
        unsafe { CoUninitialize() };
    }
}

/// 書き出しのスレッドで始めた Media Foundation（`Drop` で `MFShutdown` を呼んで終える）。COM は、その後で終える
/// （`_com` は、`drop` を呼んだ後に手放される）
struct MediaFoundation {
    _com: ComApartment,
}

impl MediaFoundation {
    fn start() -> Result<Self, String> {
        let com = ComApartment::initialize()?;
        ensure_media_foundation_dlls()?;
        // SAFETY: COM を始めたスレッドで、DLL があると確かめてから Media Foundation を始める。成功したときだけ `Self` を
        // 返し、その `Drop` で1回だけ `MFShutdown` を呼ぶので、始まりと終わりが対になる
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(failed(
            "Windows の動画の機能（Media Foundation）を始められませんでした",
        ))?;
        Ok(Self { _com: com })
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        // SAFETY: `start` で始めた Media Foundation を、同じスレッドで1回だけ終える。Media Foundation のものはみな、
        // これより先に手放している（入口の中身の順）。終えるのに失敗しても、できることは無い
        let _ = unsafe { MFShutdown() };
    }
}

/// Media Foundation の DLL が、みな読み込めるかを確かめる。DLL は遅延読み込みにしてあり（build.rs）、無いまま中の関数を
/// 呼ぶとアプリが落ちるので、呼ぶ前に確かめる（Windows の N エディションなどで「メディア機能パック」が入っていないと無い）
fn ensure_media_foundation_dlls() -> Result<(), String> {
    for name in DELAY_LOADED_DLLS.split(';') {
        // SAFETY: Windows の System32 のフォルダの中からだけ DLL を読み込み、読み込めたらすぐに手放す（中の関数は呼ばない。
        // 遅延読み込みは、関数を初めて呼ぶときに同じ DLL を読み込み直す）
        unsafe {
            let module = LoadLibraryExW(
                &HSTRING::from(name),
                HANDLE::default(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .map_err(|err| {
                format!(
                    "この Windows には動画の書き出しに要る機能（Media Foundation の {name}）がありません。Windows の \
                     N エディションなどでは、Windows の設定の「オプション機能」から「メディア機能パック」を追加してください: \
                     {err}"
                )
            })?;
            FreeLibrary(module).map_err(failed("Media Foundation の DLL を手放せませんでした"))?;
        }
    }
    Ok(())
}

/// Windows の API の失敗を、書き出しの画面に出す文言（`{していたこと}: {Windows の説明} (HRESULT)`）にする関数
fn failed(context: &'static str) -> impl FnOnce(windows::core::Error) -> String {
    move |err| format!("{context}: {err}")
}

/// Media Foundation の形の決まり（`MFSetAttributeSize`・`MFSetAttributeRatio`）どおり、2つの数を1つの 64bit の数に
/// 詰める（上の 32bit が `high`、下の 32bit が `low`。幅と高さ、分子と分母）
fn pair(high: u32, low: u32) -> u64 {
    (u64::from(high) << u32::BITS) | u64::from(low)
}

/// `count` 個を 1 秒に `per_second` 個の速さで並べたときの時刻（Media Foundation の単位：100ns。端数は切り捨て）
fn hns(count: u64, per_second: u32) -> i64 {
    (count * HNS_PER_SECOND / u64::from(per_second)) as i64
}

/// 形や設定に書く値
enum Value {
    U32(u32),
    U64(u64),
    Guid(GUID),
}

/// `target` に、`values` の値をみな書く
fn set_all(target: &IMFAttributes, values: &[(GUID, Value)]) -> windows::core::Result<()> {
    for (key, value) in values {
        // SAFETY: 有効な属性の入れ物に、生きている GUID と値への参照を渡す（入れ物は値を写す）
        unsafe {
            match value {
                Value::U32(v) => target.SetUINT32(key, *v)?,
                Value::U64(v) => target.SetUINT64(key, *v)?,
                Value::Guid(v) => target.SetGUID(key, v)?,
            }
        }
    }
    Ok(())
}

/// `values` の値を書いた、メディアの形（流れの出口・入口の形）
fn media_type(values: &[(GUID, Value)]) -> windows::core::Result<IMFMediaType> {
    // SAFETY: 空の形を作る。作ったものの所有権は `media_type` が持つ
    let media_type = unsafe { MFCreateMediaType() }?;
    set_all(&media_type, values)?;
    Ok(media_type)
}

/// `values` の値を書いた、属性の入れ物（書き込む係・符号化の係の設定）
fn attributes(values: &[(GUID, Value)]) -> windows::core::Result<IMFAttributes> {
    let mut attributes = None;
    // SAFETY: 書き込み先の `attributes` は生きている。作ったものの所有権は `attributes` が持つ
    unsafe { MFCreateAttributes(&mut attributes, values.len() as u32) }?;
    // 成功したのに入れ物が無いのは、ポインタの誤り
    let attributes = attributes.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
    set_all(&attributes, values)?;
    Ok(attributes)
}

/// 高さ `height` の絵の上から `y` 番目の行を置く、バッファの先頭からのバイトの位置。ストライド `stride` が正なら上の行から、
/// 負なら下の行から並べる（Media Foundation の決まり：このファイルの始めの説明）
fn row_offset(stride: i32, y: usize, height: usize) -> usize {
    let pitch = stride.unsigned_abs() as usize;
    let row = if stride >= 0 { y } else { height - 1 - y };
    row * pitch
}

/// 長さ `len` バイトの領域を作り、`fill` で中身を書いたもの
fn media_buffer(len: usize, fill: impl FnOnce(&mut [u8])) -> Result<IMFMediaBuffer, String> {
    let len_u32 =
        u32::try_from(len).map_err(|_| format!("書き出す領域（{len} バイト）が大きすぎます"))?;
    // SAFETY: 作った領域をロックし、ロックで得た先頭から、領域の大きさ（`max_len`）が `len` 以上だと確かめた上で、
    // ロックしている間だけ `len` バイトを書く。書き終えたらアンロックしてから、書いた長さを設定する
    unsafe {
        let buffer =
            MFCreateMemoryBuffer(len_u32).map_err(failed("書き出す領域を確保できませんでした"))?;
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut max_len = 0u32;
        buffer
            .Lock(&mut data, Some(&mut max_len), None)
            .map_err(failed("書き出す領域をロックできませんでした"))?;
        if data.is_null() || max_len < len_u32 {
            let _ = buffer.Unlock();
            return Err("書き出す領域のメモリアドレスまたはサイズが不正です".to_owned());
        }
        fill(std::slice::from_raw_parts_mut(data, len));
        buffer
            .Unlock()
            .map_err(failed("書き出す領域のロックを外せませんでした"))?;
        buffer
            .SetCurrentLength(len_u32)
            .map_err(failed("書き出す領域の長さを設定できませんでした"))?;
        Ok(buffer)
    }
}

/// `buffer` を、時刻 `start` から `end` まで（100ns の単位）の1つの塊にして、流れ `stream` に書く
fn write_sample(
    writer: &IMFSinkWriter,
    stream: u32,
    buffer: &IMFMediaBuffer,
    start: i64,
    end: i64,
) -> windows::core::Result<()> {
    // SAFETY: 有効な書き込む係と、中身を書き終えた領域で、塊を作って書く（塊は領域の参照を持つ）
    unsafe {
        let sample = MFCreateSample()?;
        sample.AddBuffer(buffer)?;
        sample.SetSampleTime(start)?;
        sample.SetSampleDuration(end - start)?;
        writer.WriteSample(stream, &sample)
    }
}

/// 32bit の小数の音（左・右・左・右…。-1〜1）を、16bit の整数の PCM（リトルエンディアン）にして `pcm` に書く（音の形を
/// 変える所はここだけ）。-1〜1 の外は、-1・1 にそろえてから直す（混ぜた音は 1 を超えることがある）
fn write_pcm16(samples: &[f32], pcm: &mut [u8]) {
    for (dst, &sample) in pcm.chunks_exact_mut(size_of::<i16>()).zip(samples) {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        dst.copy_from_slice(&value.to_le_bytes());
    }
}

pub(crate) fn create_windows_encoder<'a>(
    path: &Path,
    width: u32,
    height: u32,
    fps: u32,
    audio: Option<AudioSource<'a>>,
) -> Result<Box<dyn VideoEncoder + 'a>, String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err("書き出し先のフォルダが存在しません".to_owned());
        }
    }
    // 中身を作る前に始め、最後に手放す（入口の中身の順）
    let media_foundation = MediaFoundation::start()?;

    // 書き出し先は一時ファイル（`.mp4.tmp`）で、拡張子から入れ物の種類を決められないので、MP4 だと書く
    let settings = attributes(&[(
        MF_TRANSCODE_CONTAINERTYPE,
        Value::Guid(MFTranscodeContainerType_MPEG4),
    )])
    .map_err(failed("動画ライターの設定を作れませんでした"))?;
    // SAFETY: 生きているパスの文字と設定を渡して書き込む係を作る（バイトの流れは渡さず、パスへ書かせる）。作ったものの
    // 所有権は `writer` が持つ
    let writer = unsafe {
        MFCreateSinkWriterFromURL(&HSTRING::from(path), None::<&IMFByteStream>, &settings)
    }
    .map_err(failed("動画ライターの初期化に失敗しました"))?;

    let (video_stream, stride) = add_video_stream(&writer, width, height, fps)?;
    let audio = match audio {
        Some(source) => Some(AudioTrack {
            stream: add_audio_stream(&writer)?,
            source,
            written: 0,
            ended: false,
        }),
        None => None,
    };
    // SAFETY: 流れをみな足した、有効な書き込む係で、書き込みを始める
    unsafe { writer.BeginWriting() }.map_err(failed("動画の書き込み開始に失敗しました"))?;

    Ok(Box::new(WindowsVideoEncoder {
        writer,
        video_stream,
        audio,
        width,
        height,
        fps,
        stride,
        frame_index: 0,
        _media_foundation: media_foundation,
    }))
}

/// `writer` に、絵の流れ（出口は H.264、入口は 32bit の BGRA）を足し、`(流れの番号, 入口の1行のバイトの数（符号付き。負なら下の行から並べる）)` を返す
fn add_video_stream(
    writer: &IMFSinkWriter,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(u32, i32), String> {
    let bit_rate = u32::try_from(video_bit_rate(width, height, fps))
        .map_err(|_| "動画のビットレートが大きすぎます".to_owned())?;
    let frame_size = pair(width, height);
    // 1 秒に fps コマ（fps / 1）
    let frame_rate = pair(fps, 1);
    // 画素は正方形（横:縦 = 1:1）
    let square_pixel = pair(1, 1);
    let progressive = MFVideoInterlace_Progressive.0 as u32;

    // 入口の1行のバイトの数：Media Foundation が決める、その形の最小のストライド（RGB の形の番号は、GUID の先頭の
    // 32bit：`MFGetStrideForBitmapInfoHeader` の決まり）。符号が並べる向き（このファイルの始めの説明）
    // SAFETY: 引数は数だけ
    let stride = unsafe { MFGetStrideForBitmapInfoHeader(MFVideoFormat_RGB32.data1, width) }
        .map_err(failed("絵の1行のバイトの数を決められませんでした"))?;
    if (stride.unsigned_abs() as usize) < width as usize * 4 {
        return Err(format!("絵の1行のバイトの数（{stride}）が幅に合いません"));
    }

    let output = media_type(&[
        (MF_MT_MAJOR_TYPE, Value::Guid(MFMediaType_Video)),
        (MF_MT_SUBTYPE, Value::Guid(MFVideoFormat_H264)),
        (MF_MT_AVG_BITRATE, Value::U32(bit_rate)),
        // Mac の書き出しと同じ High プロファイル
        (
            MF_MT_MPEG2_PROFILE,
            Value::U32(eAVEncH264VProfile_High.0 as u32),
        ),
        (MF_MT_INTERLACE_MODE, Value::U32(progressive)),
        (MF_MT_FRAME_SIZE, Value::U64(frame_size)),
        (MF_MT_FRAME_RATE, Value::U64(frame_rate)),
        (MF_MT_PIXEL_ASPECT_RATIO, Value::U64(square_pixel)),
    ])
    .map_err(failed("動画の出口の形を作れませんでした"))?;
    let input = media_type(&[
        (MF_MT_MAJOR_TYPE, Value::Guid(MFMediaType_Video)),
        (MF_MT_SUBTYPE, Value::Guid(MFVideoFormat_RGB32)),
        (MF_MT_INTERLACE_MODE, Value::U32(progressive)),
        (MF_MT_FRAME_SIZE, Value::U64(frame_size)),
        (MF_MT_FRAME_RATE, Value::U64(frame_rate)),
        (MF_MT_PIXEL_ASPECT_RATIO, Value::U64(square_pixel)),
        // 符号付きのストライドを、そのまま 32bit の数として書く（`MF_MT_DEFAULT_STRIDE` の決まり）
        // 負のストライドも、Media Foundation の決まりどおり 32bit の符号付きの値のビットのまま書く
        (MF_MT_DEFAULT_STRIDE, Value::U32(stride as u32)),
    ])
    .map_err(failed("動画の入口の形を作れませんでした"))?;
    // 符号化の係（H.264）の設定：キーフレームの間隔（GOP の長さ。コマの数）
    let encoding = attributes(&[(
        CODECAPI_AVEncMPVGOPSize,
        Value::U32(key_frame_interval(fps)),
    )])
    .map_err(failed("動画の符号化の設定を作れませんでした"))?;

    // SAFETY: 有効な書き込む係に、作った形と設定を渡す（係はそれぞれの参照を持つか、写す）
    let stream = unsafe { writer.AddStream(&output) }
        .map_err(failed("動画エンコーダ入力を追加できませんでした"))?;
    // SAFETY: 同上。流れの番号は `AddStream` が返したもの
    unsafe { writer.SetInputMediaType(stream, &input, &encoding) }
        .map_err(failed("動画エンコーダの入力の形を設定できませんでした"))?;
    Ok((stream, stride))
}

/// `writer` に、音の流れ（出口は AAC・`EXPORT_AUDIO_RATE`・ステレオ、入口は 16bit の整数の PCM）を足し、流れの番号を返す
fn add_audio_stream(writer: &IMFSinkWriter) -> Result<u32, String> {
    let bits_per_sample = i16::BITS;
    // 左右の組1つ（1サンプルの時刻の分）のバイトの数
    let block_align = EXPORT_AUDIO_CHANNELS * size_of::<i16>() as u32;
    let output = media_type(&[
        (MF_MT_MAJOR_TYPE, Value::Guid(MFMediaType_Audio)),
        (MF_MT_SUBTYPE, Value::Guid(MFAudioFormat_AAC)),
        (MF_MT_AUDIO_BITS_PER_SAMPLE, Value::U32(bits_per_sample)),
        (
            MF_MT_AUDIO_SAMPLES_PER_SECOND,
            Value::U32(EXPORT_AUDIO_RATE),
        ),
        (MF_MT_AUDIO_NUM_CHANNELS, Value::U32(EXPORT_AUDIO_CHANNELS)),
        // AAC の係は、1 秒あたりのバイトの数で決める
        (
            MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
            Value::U32(EXPORT_AUDIO_BIT_RATE / u8::BITS),
        ),
    ])
    .map_err(failed("音の出口の形を作れませんでした"))?;
    let input = media_type(&[
        (MF_MT_MAJOR_TYPE, Value::Guid(MFMediaType_Audio)),
        (MF_MT_SUBTYPE, Value::Guid(MFAudioFormat_PCM)),
        (MF_MT_AUDIO_BITS_PER_SAMPLE, Value::U32(bits_per_sample)),
        (
            MF_MT_AUDIO_SAMPLES_PER_SECOND,
            Value::U32(EXPORT_AUDIO_RATE),
        ),
        (MF_MT_AUDIO_NUM_CHANNELS, Value::U32(EXPORT_AUDIO_CHANNELS)),
        (MF_MT_AUDIO_BLOCK_ALIGNMENT, Value::U32(block_align)),
        (
            MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
            Value::U32(EXPORT_AUDIO_RATE * block_align),
        ),
    ])
    .map_err(failed("音の入口の形を作れませんでした"))?;
    // SAFETY: 有効な書き込む係に、作った形を渡す（係はそれぞれの参照を持つか、写す）
    let stream = unsafe { writer.AddStream(&output) }
        .map_err(failed("音のエンコーダ入力を追加できませんでした"))?;
    // SAFETY: 同上。流れの番号は `AddStream` が返したもの。符号化の設定は渡さない（出口の形で決まる）
    unsafe { writer.SetInputMediaType(stream, &input, None::<&IMFAttributes>) }
        .map_err(failed("音のエンコーダの入力の形を設定できませんでした"))?;
    Ok(stream)
}

impl AudioTrack<'_> {
    /// 続きの1コマ分の音を `source` から取って書く。音を全部書き終えていたら `false`
    fn write_next(&mut self, writer: &IMFSinkWriter) -> Result<bool, String> {
        if self.ended {
            return Ok(false);
        }
        let Some(samples) = (self.source)() else {
            self.ended = true;
            return Ok(false);
        };
        let frames = (samples.len() / EXPORT_AUDIO_CHANNELS as usize) as u64;
        let buffer = media_buffer(size_of::<i16>() * samples.len(), |pcm| {
            write_pcm16(&samples, pcm)
        })?;
        write_sample(
            writer,
            self.stream,
            &buffer,
            hns(self.written, EXPORT_AUDIO_RATE),
            hns(self.written + frames, EXPORT_AUDIO_RATE),
        )
        .map_err(failed("音の書き込みに失敗しました"))?;
        self.written += frames;
        Ok(true)
    }
}

impl VideoEncoder for WindowsVideoEncoder<'_> {
    fn push_frame(&mut self, rgba: &[u8]) -> Result<(), String> {
        let row_bytes = self.width as usize * 4;
        let height = self.height as usize;
        if rgba.len() != row_bytes * height {
            return Err("フレーム画像のバイト数がサイズと一致しません".to_owned());
        }
        // 絵の各行を、ストライドの符号で決まる位置に並べる（このファイルの始めの説明）
        let pitch = self.stride.unsigned_abs() as usize;
        let buffer = media_buffer(pitch * height, |bgra| {
            for (y, src) in rgba.chunks_exact(row_bytes).enumerate() {
                let at = row_offset(self.stride, y, height);
                rgba_to_bgra(src, &mut bgra[at..at + row_bytes]);
            }
        })?;
        let frame = self.frame_index as u64;
        write_sample(
            &self.writer,
            self.video_stream,
            &buffer,
            hns(frame, self.fps),
            hns(frame + 1, self.fps),
        )
        .map_err(failed("動画フレームの書き込みに失敗しました"))?;
        self.frame_index += 1;

        // 音を、書いた絵の終わりの時刻（次のコマの始まり）まで書く
        if let Some(track) = &mut self.audio {
            let until = frame_start(self.frame_index, EXPORT_AUDIO_RATE, self.fps);
            while track.written < until && track.write_next(&self.writer)? {}
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), String> {
        if let Some(track) = &mut self.audio {
            while track.write_next(&self.writer)? {}
        }
        // SAFETY: 絵と音をみな書いた、有効な書き込む係を閉じる
        unsafe { self.writer.Finalize() }.map_err(failed("動画ファイルの完了処理に失敗しました"))
    }
}

#[cfg(test)]
mod tests {
    /// ストライドが正なら上の行がバッファの先頭、負なら下の行がバッファの先頭（どちらも1行はストライドの大きさおき）
    #[test]
    fn rows_follow_the_sign_of_the_stride() {
        assert_eq!(super::row_offset(3200, 0, 3), 0);
        assert_eq!(super::row_offset(3200, 2, 3), 6400);
        assert_eq!(super::row_offset(-3200, 0, 3), 6400);
        assert_eq!(super::row_offset(-3200, 2, 3), 0);
    }

    use super::*;

    #[test]
    fn pcm16_scales_and_clamps() {
        let samples = [0.0, 1.0, -1.0, 0.5, 2.0, -2.0];
        let mut pcm = vec![0u8; samples.len() * size_of::<i16>()];
        write_pcm16(&samples, &mut pcm);
        let values: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(values, [0, i16::MAX, -i16::MAX, 16384, i16::MAX, -i16::MAX]);
    }

    #[test]
    fn time_stamps_do_not_drift() {
        // 30fps のコマの時刻と長さ：長さを足すと、いつも次のコマの時刻になる
        let fps = 30;
        let total: i64 = (0..u64::from(fps))
            .map(|n| hns(n + 1, fps) - hns(n, fps))
            .sum();
        assert_eq!(total, HNS_PER_SECOND as i64);
        assert_eq!(pair(1920, 1080), (1920u64 << 32) | 1080);
    }
}
