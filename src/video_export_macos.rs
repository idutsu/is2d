use std::path::Path;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
    AVFileTypeMPEG4, AVMediaTypeVideo, AVVideoAverageBitRateKey, AVVideoCodecKey,
    AVVideoCodecTypeH264, AVVideoColorPrimariesKey, AVVideoColorPrimaries_ITU_R_709_2,
    AVVideoColorPropertiesKey, AVVideoCompressionPropertiesKey, AVVideoHeightKey,
    AVVideoMaxKeyFrameIntervalKey, AVVideoProfileLevelH264HighAutoLevel, AVVideoProfileLevelKey,
    AVVideoTransferFunctionKey, AVVideoTransferFunction_ITU_R_709_2, AVVideoWidthKey,
    AVVideoYCbCrMatrixKey, AVVideoYCbCrMatrix_ITU_R_709_2,
};
use objc2_core_media::CMTime;
use objc2_core_video::{
    kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_32BGRA, kCVReturnSuccess, CVPixelBuffer, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferPool, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

use crate::video_export::VideoEncoder;

pub(crate) struct MacosVideoEncoder {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    width: u32,
    height: u32,
    fps: u32,
    frame_index: i64,
    finished: bool,
}

fn writer_error_message(writer: &AVAssetWriter, fallback: &str) -> String {
    // SAFETY: `writer` は有効な `AVAssetWriter` インスタンスであり、`error` の取得は安全。
    match unsafe { writer.error() } {
        Some(err) => format!("{fallback}: {}", err.localizedDescription()),
        None => fallback.to_owned(),
    }
}

pub(crate) fn create_macos_encoder(
    path: &Path,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<Box<dyn VideoEncoder>, String> {
    if width == 0 || height == 0 || (width & 1) != 0 || (height & 1) != 0 {
        return Err("動画の幅と高さは正の偶数である必要があります".to_owned());
    }
    let fps = fps.clamp(1, 60);
    let path_str = path
        .to_str()
        .ok_or_else(|| "書き出し先のパスに無効な文字が含まれています".to_owned())?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err("書き出し先のフォルダが存在しません".to_owned());
        }
    }
    let _ = std::fs::remove_file(path);

    let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));
    let bitrate = ((width as f64) * (height as f64) * (fps as f64) * 0.15).round() as u64;
    let num_width = NSNumber::new_u32(width);
    let num_height = NSNumber::new_u32(height);
    let num_bitrate = NSNumber::new_u64(bitrate);
    let num_keyframe = NSNumber::new_u32(fps.saturating_mul(2));
    let num_bgra = NSNumber::new_u32(kCVPixelFormatType_32BGRA);

    // SAFETY: AVFoundation / CoreVideo の定数キーと有効な引数を用いて Objective-C API を呼び出し、
    // 生成されたオブジェクトの所有権はすべて `Retained` で安全に管理する。
    let (writer, input, adaptor) = unsafe {
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
        if !writer.startWriting() {
            return Err(writer_error_message(
                &writer,
                "動画の書き込み開始に失敗しました",
            ));
        }
        writer.startSessionAtSourceTime(CMTime::new(0, fps as i32));
        (writer, input, adaptor)
    };

    Ok(Box::new(MacosVideoEncoder {
        writer,
        input,
        adaptor,
        width,
        height,
        fps,
        frame_index: 0,
        finished: false,
    }))
}

impl VideoEncoder for MacosVideoEncoder {
    fn push_frame(&mut self, rgba: &[u8]) -> Result<(), String> {
        let w = self.width as usize;
        let h = self.height as usize;
        let row_bytes = w * 4;
        if rgba.len() != row_bytes * h {
            return Err("フレーム画像のバイト数がサイズと一致しません".to_owned());
        }

        autoreleasepool(|_| {
            // SAFETY: アダプタのプールから取得した `CVPixelBuffer` をロック中のみ `bytes_per_row` と `height` の範囲内で書き込み、
            // アンロック後に `appendPixelBuffer_withPresentationTime` へ渡す。
            unsafe {
                while !self.input.isReadyForMoreMediaData() {
                    if self.writer.status() != AVAssetWriterStatus::Writing {
                        return Err(writer_error_message(
                            &self.writer,
                            "動画データの書き込み待機中にエラーが発生しました",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(1));
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
                if base_ptr.is_null() || bytes_per_row < row_bytes {
                    let _ =
                        CVPixelBufferUnlockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags(0));
                    return Err("画素バッファのメモリアドレスまたは行幅が不正です".to_owned());
                }

                for y in 0..h {
                    let src_row = &rgba[y * row_bytes..(y + 1) * row_bytes];
                    let dst_row =
                        std::slice::from_raw_parts_mut(base_ptr.add(y * bytes_per_row), row_bytes);
                    for (dst_px, src_px) in dst_row.chunks_exact_mut(4).zip(src_row.chunks_exact(4))
                    {
                        dst_px[0] = src_px[2];
                        dst_px[1] = src_px[1];
                        dst_px[2] = src_px[0];
                        dst_px[3] = src_px[3];
                    }
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
        self.finished = true;
        let (tx, rx) = mpsc::sync_channel::<()>(1);
        let block = RcBlock::new(move || {
            let _ = tx.send(());
        });

        // SAFETY: 書き込み中の `input` / `writer` に対して完了処理を呼び出し、
        // 完了ハンドラ `block` の通知を待ってから最終ステータスを読み取る。
        unsafe {
            self.input.markAsFinished();
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

impl Drop for MacosVideoEncoder {
    fn drop(&mut self) {
        if !self.finished {
            // SAFETY: `finish` 完了前に破棄される場合のみ、有効な `writer` に対して `cancelWriting` を呼ぶ。
            unsafe {
                self.writer.cancelWriting();
            }
        }
    }
}
