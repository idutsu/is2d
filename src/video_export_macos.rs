use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
    AVFileTypeMPEG4, AVMediaTypeAudio, AVMediaTypeVideo, AVVideoAverageBitRateKey, AVVideoCodecKey,
    AVVideoCodecTypeH264, AVVideoColorPrimariesKey, AVVideoColorPrimaries_ITU_R_709_2,
    AVVideoColorPropertiesKey, AVVideoCompressionPropertiesKey, AVVideoHeightKey,
    AVVideoMaxKeyFrameIntervalKey, AVVideoProfileLevelH264HighAutoLevel, AVVideoProfileLevelKey,
    AVVideoTransferFunctionKey, AVVideoTransferFunction_ITU_R_709_2, AVVideoWidthKey,
    AVVideoYCbCrMatrixKey, AVVideoYCbCrMatrix_ITU_R_709_2,
};
use objc2_core_audio_types::{
    kAudioFormatFlagsNativeFloatPacked, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC,
    AudioStreamBasicDescription,
};
use objc2_core_media::{
    kCMBlockBufferAssureMemoryNowFlag, kCMBlockBufferNoErr, CMAudioFormatDescription,
    CMAudioFormatDescriptionCreate, CMAudioSampleBufferCreateReadyWithPacketDescriptions,
    CMBlockBuffer, CMSampleBuffer, CMTime,
};
use objc2_core_video::{
    kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_32BGRA, kCVReturnSuccess, CVPixelBuffer, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool,
    CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

use crate::video_export::{
    key_frame_interval, rgba_to_bgra, video_bit_rate, AudioSource, VideoEncoder,
    EXPORT_AUDIO_BIT_RATE, EXPORT_AUDIO_CHANNELS, EXPORT_AUDIO_RATE,
};

// 音の入力の設定のキー（AVFAudio の AVAudioSettings.h）。objc2-av-foundation 0.3.2 の `AVAudioSettings` は中身が空なので、
// AVFAudio にある定数をここで宣言して使う
#[link(name = "AVFAudio", kind = "framework")]
extern "C" {
    static AVFormatIDKey: &'static NSString;
    static AVSampleRateKey: &'static NSString;
    static AVNumberOfChannelsKey: &'static NSString;
    static AVEncoderBitRateKey: &'static NSString;
}

pub(crate) struct MacosVideoEncoder<'a> {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    /// 音の入力（音なしで作ったときは無い）
    audio: Option<AudioTrack<'a>>,
    width: u32,
    height: u32,
    fps: u32,
    frame_index: i64,
    finished: bool,
}

/// 音の入力と、そこへ渡す音
struct AudioTrack<'a> {
    input: Retained<AVAssetWriterInput>,
    /// 渡す音の形：32bit の小数・左右を交互に並べたステレオ・`EXPORT_AUDIO_RATE`（`AudioSource` が返す形のまま）
    format: Retained<CMAudioFormatDescription>,
    /// 続きの音を、コマの順に返すもの
    source: AudioSource<'a>,
    /// 取ったが、音の入力がまだ受け取れずに残っている音の塊
    pending: Option<Retained<CMSampleBuffer>>,
    /// 音の塊にした音の、左右の組の数。次の塊の始まりの時刻（`EXPORT_AUDIO_RATE` 分の1秒の単位）で、コマ `f` の音を
    /// 塊にする前には `frame_start(f)` になっている（コマごとの音をすき間なくつなぐので）
    written: i64,
    /// `source` が音を全部返した
    ended: bool,
}

fn writer_error_message(writer: &AVAssetWriter, fallback: &str) -> String {
    // SAFETY: `writer` は有効な `AVAssetWriter` インスタンスであり、`error` の取得は安全。
    match unsafe { writer.error() } {
        Some(err) => format!("{fallback}: {}", err.localizedDescription()),
        None => fallback.to_owned(),
    }
}

/// 書き込みが止まっていないことを確かめて、入力が受け取れるようになるのを少し（1ms）待つ
fn wait_for_input(writer: &AVAssetWriter) -> Result<(), String> {
    // SAFETY: `writer` は有効な `AVAssetWriter` インスタンスであり、`status` の取得は安全。
    if unsafe { writer.status() } != AVAssetWriterStatus::Writing {
        return Err(writer_error_message(
            writer,
            "動画データの書き込み待機中にエラーが発生しました",
        ));
    }
    std::thread::sleep(Duration::from_millis(1));
    Ok(())
}

pub(crate) fn create_macos_encoder<'a>(
    path: &Path,
    width: u32,
    height: u32,
    fps: u32,
    audio: Option<AudioSource<'a>>,
) -> Result<Box<dyn VideoEncoder + 'a>, String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| "書き出し先のパスに無効な文字が含まれています".to_owned())?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err("書き出し先のフォルダが存在しません".to_owned());
        }
    }

    let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));
    let num_width = NSNumber::new_u32(width);
    let num_height = NSNumber::new_u32(height);
    let num_bitrate = NSNumber::new_u64(video_bit_rate(width, height, fps));
    let num_keyframe = NSNumber::new_u32(key_frame_interval(fps));
    let num_bgra = NSNumber::new_u32(kCVPixelFormatType_32BGRA);

    // SAFETY: AVFoundation / CoreVideo の定数キーと有効な引数を用いて Objective-C API を呼び出し、
    // 生成されたオブジェクトの所有権はすべて `Retained` で安全に管理する。
    let (writer, input, adaptor, audio) = unsafe {
        let file_type = AVFileTypeMPEG4.ok_or("MP4 ファイル種別定数が利用できません")?;
        let media_type = AVMediaTypeVideo.ok_or("Video メディア種別定数が利用できません")?;
        let writer =
            AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type).map_err(|e| {
                format!(
                    "動画ライターの初期化に失敗しました: {}",
                    e.localizedDescription()
                )
            })?;

        let compression_dict = NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                AVVideoAverageBitRateKey.expect("AVVideoAverageBitRateKey"),
                AVVideoProfileLevelKey.expect("AVVideoProfileLevelKey"),
                AVVideoMaxKeyFrameIntervalKey.expect("AVVideoMaxKeyFrameIntervalKey"),
            ],
            &[
                num_bitrate.as_ref(),
                AVVideoProfileLevelH264HighAutoLevel
                    .expect("AVVideoProfileLevelH264HighAutoLevel")
                    .as_ref(),
                num_keyframe.as_ref(),
            ],
        );
        let color_dict = NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                AVVideoColorPrimariesKey.expect("AVVideoColorPrimariesKey"),
                AVVideoTransferFunctionKey.expect("AVVideoTransferFunctionKey"),
                AVVideoYCbCrMatrixKey.expect("AVVideoYCbCrMatrixKey"),
            ],
            &[
                AVVideoColorPrimaries_ITU_R_709_2
                    .expect("AVVideoColorPrimaries_ITU_R_709_2")
                    .as_ref(),
                AVVideoTransferFunction_ITU_R_709_2
                    .expect("AVVideoTransferFunction_ITU_R_709_2")
                    .as_ref(),
                AVVideoYCbCrMatrix_ITU_R_709_2
                    .expect("AVVideoYCbCrMatrix_ITU_R_709_2")
                    .as_ref(),
            ],
        );
        let output_settings = NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                AVVideoCodecKey.expect("AVVideoCodecKey"),
                AVVideoWidthKey.expect("AVVideoWidthKey"),
                AVVideoHeightKey.expect("AVVideoHeightKey"),
                AVVideoCompressionPropertiesKey.expect("AVVideoCompressionPropertiesKey"),
                AVVideoColorPropertiesKey.expect("AVVideoColorPropertiesKey"),
            ],
            &[
                AVVideoCodecTypeH264.expect("AVVideoCodecTypeH264").as_ref(),
                num_width.as_ref(),
                num_height.as_ref(),
                compression_dict.as_ref(),
                color_dict.as_ref(),
            ],
        );

        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            media_type,
            Some(&output_settings),
        );
        input.setExpectsMediaDataInRealTime(false);

        let source_attrs = NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                &*(kCVPixelBufferPixelFormatTypeKey as *const _ as *const NSString),
                &*(kCVPixelBufferWidthKey as *const _ as *const NSString),
                &*(kCVPixelBufferHeightKey as *const _ as *const NSString),
            ],
            &[num_bgra.as_ref(), num_width.as_ref(), num_height.as_ref()],
        );
        let adaptor = AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
            &input,
            Some(&source_attrs),
        );

        if !writer.canAddInput(&input) {
            return Err("動画エンコーダ入力を追加できませんでした".to_owned());
        }
        writer.addInput(&input);
        let audio = match audio {
            Some(source) => Some(add_audio_track(&writer, source)?),
            None => None,
        };
        if !writer.startWriting() {
            return Err(writer_error_message(
                &writer,
                "動画の書き込み開始に失敗しました",
            ));
        }
        writer.startSessionAtSourceTime(CMTime::new(0, fps as i32));
        (writer, input, adaptor, audio)
    };

    Ok(Box::new(MacosVideoEncoder {
        writer,
        input,
        adaptor,
        audio,
        width,
        height,
        fps,
        frame_index: 0,
        finished: false,
    }))
}

/// `writer` に、`source` の音を書く音の入力（AAC・`EXPORT_AUDIO_RATE`・ステレオ）を足す（書き込みを始める前に呼ぶ）
fn add_audio_track<'a>(
    writer: &AVAssetWriter,
    source: AudioSource<'a>,
) -> Result<AudioTrack<'a>, String> {
    let num_format = NSNumber::new_u32(kAudioFormatMPEG4AAC);
    let num_rate = NSNumber::new_u32(EXPORT_AUDIO_RATE);
    let num_channels = NSNumber::new_u32(EXPORT_AUDIO_CHANNELS);
    let num_bitrate = NSNumber::new_u32(EXPORT_AUDIO_BIT_RATE);
    // 左右の組1つ（1サンプルの時刻の分）のバイトの数
    let bytes_per_frame = EXPORT_AUDIO_CHANNELS * size_of::<f32>() as u32;
    let asbd = AudioStreamBasicDescription {
        mSampleRate: f64::from(EXPORT_AUDIO_RATE),
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagsNativeFloatPacked,
        mBytesPerPacket: bytes_per_frame,
        mFramesPerPacket: 1,
        mBytesPerFrame: bytes_per_frame,
        mChannelsPerFrame: EXPORT_AUDIO_CHANNELS,
        mBitsPerChannel: size_of::<f32>() as u32 * u8::BITS,
        mReserved: 0,
    };
    // SAFETY: AVFAudio の定数キーと有効な引数で Objective-C / CoreMedia の API を呼び、作ったものの所有権は
    // `Retained` で持つ。音の形を作る関数は `asbd` を写すので、呼んだ後に `asbd` が無くなってよい
    unsafe {
        let media_type = AVMediaTypeAudio.ok_or("Audio メディア種別定数が利用できません")?;
        let settings = NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                AVFormatIDKey,
                AVSampleRateKey,
                AVNumberOfChannelsKey,
                AVEncoderBitRateKey,
            ],
            &[
                num_format.as_ref(),
                num_rate.as_ref(),
                num_channels.as_ref(),
                num_bitrate.as_ref(),
            ],
        );
        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            media_type,
            Some(&settings),
        );
        input.setExpectsMediaDataInRealTime(false);
        if !writer.canAddInput(&input) {
            return Err("音のエンコーダ入力を追加できませんでした".to_owned());
        }
        writer.addInput(&input);

        let mut format_ptr: *const CMAudioFormatDescription = std::ptr::null();
        let status = CMAudioFormatDescriptionCreate(
            None,
            NonNull::from(&asbd),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            None,
            NonNull::from(&mut format_ptr),
        );
        if status != 0 {
            return Err(format!("音の形を作れませんでした (OSStatus: {status})"));
        }
        let format = Retained::from_raw(format_ptr as *mut CMAudioFormatDescription)
            .ok_or("音の形の保持に失敗しました")?;
        Ok(AudioTrack {
            input,
            format,
            source,
            pending: None,
            written: 0,
            ended: false,
        })
    }
}

impl AudioTrack<'_> {
    /// 音の入力が受け取れる間、続きの音を書く。1つでも書いたら `true`。音を全部書いたら、音の入力を終える（AAC の
    /// 圧縮は音を少しためてから出すので、終えないと最後の分が出ず、書き込む側がそれを待って絵の入力を止めたままにする）
    fn write_while_ready(&mut self, writer: &AVAssetWriter) -> Result<bool, String> {
        let mut wrote = false;
        while !self.ended {
            let buffer = match self.pending.take() {
                Some(buffer) => buffer,
                None => match (self.source)() {
                    Some(samples) => self.sample_buffer(&samples)?,
                    None => {
                        // SAFETY: 有効な音の入力を、音を全部渡した後に1回だけ終える。
                        unsafe { self.input.markAsFinished() };
                        self.ended = true;
                        break;
                    }
                },
            };
            // SAFETY: 有効な音の入力に、その入力の形（`format`）で作った音の塊を、受け取れるときだけ渡す。
            unsafe {
                if !self.input.isReadyForMoreMediaData() {
                    self.pending = Some(buffer);
                    break;
                }
                if !self.input.appendSampleBuffer(&buffer) {
                    return Err(writer_error_message(writer, "音の書き込みに失敗しました"));
                }
            }
            wrote = true;
        }
        Ok(wrote)
    }

    /// 1コマ分の音 `samples`（左・右・左・右…）を、続きの時刻（`written`）に置く音の塊にする
    fn sample_buffer(&mut self, samples: &[f32]) -> Result<Retained<CMSampleBuffer>, String> {
        let bytes = std::mem::size_of_val(samples);
        let frames = (samples.len() / EXPORT_AUDIO_CHANNELS as usize) as i64;
        // SAFETY: 長さ `bytes` の領域をすぐに確保した塊を作り（音が空なら、長さ 0 は断られて Err になる）、`samples` の
        // `bytes` バイトをそこへ写してから、音の塊に包む。作ったものの所有権は `Retained` で持つ
        unsafe {
            let pts = CMTime::new(self.written, EXPORT_AUDIO_RATE as i32);
            let mut block_ptr: *mut CMBlockBuffer = std::ptr::null_mut();
            let status = CMBlockBuffer::create_with_memory_block(
                None,
                std::ptr::null_mut(),
                bytes,
                None,
                std::ptr::null(),
                0,
                bytes,
                kCMBlockBufferAssureMemoryNowFlag,
                NonNull::from(&mut block_ptr),
            );
            if status != kCMBlockBufferNoErr {
                return Err(format!(
                    "音の領域を確保できませんでした (OSStatus: {status})"
                ));
            }
            let block = Retained::from_raw(block_ptr).ok_or("音の領域の保持に失敗しました")?;
            let status = CMBlockBuffer::replace_data_bytes(
                NonNull::from(samples).cast::<c_void>(),
                &block,
                0,
                bytes,
            );
            if status != kCMBlockBufferNoErr {
                return Err(format!("音を写せませんでした (OSStatus: {status})"));
            }
            let mut buffer_ptr: *mut CMSampleBuffer = std::ptr::null_mut();
            let status = CMAudioSampleBufferCreateReadyWithPacketDescriptions(
                None,
                &block,
                &self.format,
                frames as _,
                pts,
                std::ptr::null(),
                NonNull::from(&mut buffer_ptr),
            );
            if status != 0 {
                return Err(format!("音の塊を作れませんでした (OSStatus: {status})"));
            }
            self.written += frames;
            Retained::from_raw(buffer_ptr).ok_or_else(|| "音の塊の保持に失敗しました".to_owned())
        }
    }
}

impl VideoEncoder for MacosVideoEncoder<'_> {
    fn push_frame(&mut self, rgba: &[u8]) -> Result<(), String> {
        let w = self.width as usize;
        let h = self.height as usize;
        let row_bytes = w * 4;
        if rgba.len() != row_bytes * h {
            return Err("フレーム画像のバイト数がサイズと一致しません".to_owned());
        }

        autoreleasepool(|_| {
            // SAFETY: プールから取得した `CVPixelBuffer` をロックし、幅・高さ・`bytes_per_row` が書き込み範囲以上であることを確認した上で
            // ロック中のみ書き込み、アンロック後に `appendPixelBuffer_withPresentationTime` へ渡す。
            unsafe {
                // 絵の入力が受け取れるようになるまで、音の入力が受け取れるだけ続きの音を書き、どちらも受け取れない間は
                // 待つ（書き込む側は、絵と音の時刻が離れすぎないよう、絵の入力を、音が先へ進むまで止めることがある）
                while !self.input.isReadyForMoreMediaData() {
                    let wrote = match &mut self.audio {
                        Some(track) => track.write_while_ready(&self.writer)?,
                        None => false,
                    };
                    if !wrote {
                        wait_for_input(&self.writer)?;
                    }
                }

                let pool = self.adaptor.pixelBufferPool().ok_or_else(|| {
                    writer_error_message(&self.writer, "画素バッファプールを取得できません")
                })?;
                let mut px_buf_ptr: *mut CVPixelBuffer = std::ptr::null_mut();
                let status = CVPixelBufferPool::create_pixel_buffer(
                    None,
                    &pool,
                    NonNull::from(&mut px_buf_ptr),
                );
                if status != kCVReturnSuccess || px_buf_ptr.is_null() {
                    return Err(format!(
                        "画素バッファの確保に失敗しました (CVReturn: {status})"
                    ));
                }
                let pixel_buffer =
                    Retained::from_raw(px_buf_ptr).ok_or("画素バッファの保持に失敗しました")?;

                let lock_res =
                    CVPixelBufferLockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags(0));
                if lock_res != kCVReturnSuccess {
                    return Err(format!(
                        "画素バッファのロックに失敗しました (CVReturn: {lock_res})"
                    ));
                }

                let base_ptr = CVPixelBufferGetBaseAddress(&pixel_buffer) as *mut u8;
                let bytes_per_row = CVPixelBufferGetBytesPerRow(&pixel_buffer);
                let buf_w = CVPixelBufferGetWidth(&pixel_buffer);
                let buf_h = CVPixelBufferGetHeight(&pixel_buffer);
                if base_ptr.is_null() || bytes_per_row < row_bytes || buf_w < w || buf_h < h {
                    let _ =
                        CVPixelBufferUnlockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags(0));
                    return Err("画素バッファのメモリアドレスまたはサイズが不正です".to_owned());
                }

                for y in 0..h {
                    let src_row = &rgba[y * row_bytes..(y + 1) * row_bytes];
                    let dst_row =
                        std::slice::from_raw_parts_mut(base_ptr.add(y * bytes_per_row), row_bytes);
                    rgba_to_bgra(src_row, dst_row);
                }
                let _ = CVPixelBufferUnlockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags(0));

                let pts = CMTime::new(self.frame_index, self.fps as i32);
                self.frame_index += 1;
                if !self
                    .adaptor
                    .appendPixelBuffer_withPresentationTime(&pixel_buffer, pts)
                {
                    return Err(writer_error_message(
                        &self.writer,
                        "動画フレームの書き込みに失敗しました",
                    ));
                }
                Ok(())
            }
        })
    }

    fn finish(mut self: Box<Self>) -> Result<(), String> {
        let (tx, rx) = mpsc::sync_channel::<()>(1);
        // OS から呼ばれる受け口（書き終えた知らせ。砦の中：設計 claude/最後の砦を仕事の入口に置く設計.md）
        let block = RcBlock::new(move || {
            let _ = crate::vector_engine::guarded(|| {
                let _ = tx.send(());
            });
        });

        // SAFETY: `writer` が `Writing` 状態であることを確認した上で `markAsFinished` / `finishWritingWithCompletionHandler` を呼び、
        // 完了ハンドラ `block` の通知を待ってから最終ステータスを読み取る。
        unsafe {
            if self.writer.status() != AVAssetWriterStatus::Writing {
                return Err(writer_error_message(
                    &self.writer,
                    "動画ファイルの完了処理を開始できませんでした",
                ));
            }
            // 絵を終えてから、残りの音を書く（絵の入力を終えれば、音は絵を待たずに受け取られる。全部書けば音の入力も終える）
            self.input.markAsFinished();
            if let Some(track) = &mut self.audio {
                while !track.ended {
                    if !track.write_while_ready(&self.writer)? {
                        wait_for_input(&self.writer)?;
                    }
                }
            }
            self.finished = true;
            self.writer.finishWritingWithCompletionHandler(&block);
            let _ = rx.recv();
            if self.writer.status() == AVAssetWriterStatus::Completed {
                Ok(())
            } else {
                Err(writer_error_message(
                    &self.writer,
                    "動画ファイルの完了処理に失敗しました",
                ))
            }
        }
    }
}

impl Drop for MacosVideoEncoder<'_> {
    fn drop(&mut self) {
        if !self.finished {
            // SAFETY: `finish` 完了前に破棄される場合のみ、有効な `writer` に対して `cancelWriting` を呼ぶ。
            unsafe {
                self.writer.cancelWriting();
            }
        }
    }
}
