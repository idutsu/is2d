use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use eframe::egui::Vec2;

use crate::canvas_tool::View;
use crate::planar_map::LayerBase;
use crate::sound::{frame_start, mix, SOUND_RATE};
use crate::symbol::Document;

/// OS ごとの H.264 MP4 動画エンコーダの共通インターフェース
pub(crate) trait VideoEncoder {
    /// 1コマ分の画像（幅×高さ×4 バイト、RGBA、上の行から順、アルファは 255）を渡す。音ありで作った入口は、絵を
    /// 受け取れるようになるのを待つ間、音の入力が受け取れるだけ、続きの音を `AudioSource` から取って書く
    fn push_frame(&mut self, rgba: &[u8]) -> Result<(), String>;
    /// 残りの音を書き、書き出しを終えて、ファイルを閉じる
    fn finish(self: Box<Self>) -> Result<(), String>;
}

/// 書き出す音を、コマ 0 から1コマ分ずつ順に作って返すもの（全部返したら `None`：`export_audio`）。音は絵と同じ入口で
/// コマごとに渡すのではなく、書き込む側が求めるときに取る。書き込む側（AVAssetWriter）は、絵と音の時刻が離れすぎない
/// よう、絵の入力を、音が先へ進むまで止めることがあり、AAC の圧縮は音をためてから出すので、音は渡した所より手前までしか
/// 進まない。そのとき今のコマより先の音を渡せないと、止まったままになる
pub(crate) type AudioSource<'a> = Box<dyn FnMut() -> Option<Vec<f32>> + 'a>;

/// `path` に、幅 `width`・高さ `height`（どちらも偶数）・`fps`（1〜60）の H.264 MP4 を書き出す入口を作る。`audio`
/// があれば、その音（AAC・`EXPORT_AUDIO_RATE`・ステレオ）も入れる。入力を整えて `path` を消しておくのは、呼び出し元
/// （`run_video_export`）
pub(crate) fn create_video_encoder<'a>(
    path: &Path,
    width: u32,
    height: u32,
    fps: u32,
    audio: Option<AudioSource<'a>>,
) -> Result<Box<dyn VideoEncoder + 'a>, String> {
    #[cfg(target_os = "macos")]
    {
        crate::video_export_macos::create_macos_encoder(path, width, height, fps, audio)
    }
    #[cfg(target_os = "windows")]
    {
        crate::video_export_windows::create_windows_encoder(path, width, height, fps, audio)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (path, width, height, fps, audio);
        Err("この OS ではまだ動画の書き出しに対応していません".to_owned())
    }
}

/// 書き出すコマの数（ルートのタイムラインのコマ 0 から最後のコマまで。コマが無くても1コマ）
fn export_frames(doc: &Document) -> usize {
    doc.root.total_frames.max(1)
}

/// 書き出す音の速さ（1 秒あたりのサンプルの数）。音の中の位置の単位 `SOUND_RATE`（48kHz）と同じにする
pub(crate) const EXPORT_AUDIO_RATE: u32 = SOUND_RATE as u32;
/// 書き出す音のチャンネルの数（ステレオ。`AudioSource` が返す音は、左・右・左・右…と交互に並ぶ）
pub(crate) const EXPORT_AUDIO_CHANNELS: u32 = 2;
/// 書き出す音（AAC）のビットレート（1 秒あたりのビットの数。ステレオで 192kbps）
pub(crate) const EXPORT_AUDIO_BIT_RATE: u32 = 192_000;

/// 書き出す動画（H.264）の、1画素・1コマあたりのビットの数（平均ビットレートの式：`video_bit_rate`）
const VIDEO_BITS_PER_PIXEL: f64 = 0.15;
/// 書き出す動画のキーフレームの、いちばん長い間隔（秒）
const KEY_FRAME_INTERVAL_SECONDS: u32 = 2;

/// 書き出す動画の平均ビットレート（1 秒あたりのビットの数）：幅×高さ×fps×`VIDEO_BITS_PER_PIXEL`。どの OS の入口も
/// これを使う（決め方は1つ）
pub(crate) fn video_bit_rate(width: u32, height: u32, fps: u32) -> u64 {
    (f64::from(width) * f64::from(height) * f64::from(fps) * VIDEO_BITS_PER_PIXEL).round() as u64
}

/// 書き出す動画のキーフレームの、いちばん長い間隔（コマの数）：fps×`KEY_FRAME_INTERVAL_SECONDS`。どの OS の入口も
/// これを使う
pub(crate) fn key_frame_interval(fps: u32) -> u32 {
    fps.saturating_mul(KEY_FRAME_INTERVAL_SECONDS)
}

/// 1行分の RGBA の画素 `rgba` を、BGRA に並べ替えて `bgra` に書く（どの OS の入口も、絵をこの並びで渡す）
pub(crate) fn rgba_to_bgra(rgba: &[u8], bgra: &mut [u8]) {
    for (dst, src) in bgra.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
}

/// 書き出す音：書き出すコマ（`export_frames`）の `frame_audio` を、コマ 0 から順に1つずつ返す `AudioSource`。全部
/// つなぐと、絵と同じ長さ（`frame_start(コマ数)` 個）の音になる
pub(crate) fn export_audio(doc: &Document) -> AudioSource<'_> {
    let mut frames = 0..export_frames(doc);
    Box::new(move || frames.next().map(|f| frame_audio(doc, f)))
}

/// 書き出しの、ルートのタイムラインのコマ `frame` の間に鳴る音（左・右・左・右…）：コマの最初のサンプル
/// （`frame_start(frame)`）から次のコマの最初のサンプルの手前までを、再生と同じ `sound::mix` で、書き出す全部のコマを
/// 続けて鳴らす範囲として混ぜたもの（聞こえる音と書き出す音は同じ）。`mix` はどのサンプルの値もその番号と範囲だけで
/// 決めるので、コマごとに作ってつないだ音は、続けて混ぜた音と同じになる
fn frame_audio(doc: &Document, frame: usize) -> Vec<f32> {
    let start = frame_start(frame, EXPORT_AUDIO_RATE, doc.fps);
    let end = frame_start(frame + 1, EXPORT_AUDIO_RATE, doc.fps);
    mix(
        doc,
        &doc.root,
        0..export_frames(doc),
        start,
        (end - start) as usize,
        EXPORT_AUDIO_RATE,
    )
}

/// ステージの画素の大きさ。書き出す画素の大きさと原点は、みなこれから決める（ステージの大きさは、書く入口と読み込みで
/// `symbol::stage_size_of` で範囲にそろえてある：レビュー f01d518 の N-4007）
fn stage_pixel_size(stage_size: Vec2) -> (u32, u32) {
    (stage_size.x.round() as u32, stage_size.y.round() as u32)
}

/// ステージの大きさから書き出し動画の偶数サイズ `(width, height)` を求める
pub(crate) fn export_dimensions(stage_size: Vec2) -> (u32, u32) {
    let (w, h) = stage_pixel_size(stage_size);
    (w + (w & 1), h + (h & 1))
}

/// ルートのタイムラインの指定コマ `frame` を RGBA 画像（`width * height * 4` バイト、白背景、アルファ 255）に塗る
#[cfg(test)]
pub(crate) fn render_frame(doc: &Document, frame: usize, width: u32, height: u32) -> Vec<u8> {
    render_frame_holding(doc, frame, width, height)
        .expect("画像を作れる大きさ")
        .0
}

/// 塗った1コマ（RGBA）と、塗るときに展開したセル画の地図の計算に使う写し（`render_frame_holding`）
type Rendered = (Vec<u8>, Vec<Arc<LayerBase>>);

/// `render_frame` と、塗るときに展開したセル画の地図の計算に使う写し。写しを次のコマを塗る間まで持てば、同じキーフレームの
/// 地図を作り直さない（写しは、それを使う所が持つ：レビュー「軽量化・高速化」の★1）。`None`：その大きさの画像を作れない
fn render_frame_holding(doc: &Document, frame: usize, width: u32, height: u32) -> Option<Rendered> {
    let ctx = crate::symbol::SymbolEvalCtx::new(&doc.library).without_mesh();
    let rgba = paint_frame(doc, frame, width, height, &ctx)?;
    Some((rgba, ctx.into_state().take_held()))
}

fn paint_frame(
    doc: &Document,
    frame: usize,
    width: u32,
    height: u32,
    ctx: &crate::symbol::SymbolEvalCtx<'_>,
) -> Option<Vec<u8>> {
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    pixmap.fill(tiny_skia::Color::WHITE);

    let (stage_w, stage_h) = stage_pixel_size(doc.stage_size);
    let origin_x = (View::DEFAULT_STAGE_CENTER.x - stage_w as f64 * 0.5) as f32;
    let origin_y = (View::DEFAULT_STAGE_CENTER.y - stage_h as f64 * 0.5) as f32;

    for layer in &doc.root.layers {
        if !layer.visible {
            continue;
        }
        let shapes = layer.evaluate_shapes_at(frame, layer.id, ctx.clone());
        for shape in &shapes {
            if shape.is_empty_symbol_marker() {
                continue;
            }
            // 塗り、続けて輪郭に乗せた線（画面のメッシュと同じ順）
            fill_contours(&mut pixmap, shape.contours(), shape.color, (origin_x, origin_y));
            for o in shape.outline() {
                fill_contours(&mut pixmap, o.rings.iter(), o.color, (origin_x, origin_y));
            }
        }
    }

    let mut data = pixmap.take();
    let w_usize = width as usize;
    let h_usize = height as usize;
    let stage_w_usize = (stage_w as usize).min(w_usize);
    let stage_h_usize = (stage_h as usize).min(h_usize);
    if stage_w_usize < w_usize {
        for y in 0..stage_h_usize {
            let row_start = (y * w_usize + stage_w_usize) * 4;
            let row_end = (y + 1) * w_usize * 4;
            data[row_start..row_end].fill(255);
        }
    }
    if stage_h_usize < h_usize {
        data[stage_h_usize * w_usize * 4..].fill(255);
    }
    Some(data)
}

/// 輪郭 `contours`（文書の座標）を、偶奇の規則で色 `color` に塗る（`origin` は書き出す絵の左上の文書の座標）
fn fill_contours<'a>(
    pixmap: &mut tiny_skia::Pixmap,
    contours: impl Iterator<Item = &'a crate::vector_engine::Contour>,
    color: eframe::egui::Color32,
    (origin_x, origin_y): (f32, f32),
) {
    let mut pb = tiny_skia::PathBuilder::new();
    for contour in contours {
        let Some(first) = contour.segments.first() else {
            continue;
        };
        pb.move_to(first.p0.x as f32 - origin_x, first.p0.y as f32 - origin_y);
        for seg in contour.segments.iter() {
            pb.quad_to(
                seg.p1.x as f32 - origin_x,
                seg.p1.y as f32 - origin_y,
                seg.p2.x as f32 - origin_x,
                seg.p2.y as f32 - origin_y,
            );
        }
        pb.close();
    }
    let Some(path) = pb.finish() else {
        return;
    };
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        tiny_skia::FillRule::EvenOdd,
        tiny_skia::Transform::identity(),
        None,
    );
}

/// 書き出し先 `target_path` と同じフォルダに置く一時ファイルパス（`.<名前>.<プロセスID>.mp4.tmp`）を返す
pub(crate) fn tmp_export_path(target_path: &Path) -> PathBuf {
    let file_name = crate::project_io::file_name_or(target_path, "video.mp4");
    let stem = file_name.strip_suffix(".mp4").unwrap_or(file_name);
    target_path.with_file_name(format!(".{stem}.{}.mp4.tmp", std::process::id()))
}

/// 動画書き出しジョブの終了結果
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum VideoExportOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

/// バックグラウンドスレッドで動画（MP4）を書き出す進行状態
pub(crate) struct VideoExportJob {
    pub(crate) target_path: PathBuf,
    pub(crate) completed_frames: Arc<AtomicUsize>,
    pub(crate) total_frames: usize,
    pub(crate) cancel_flag: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<VideoExportOutcome>>,
}

impl VideoExportJob {
    /// 文書の写し `doc` を受け取り、`target_path` へのバックグラウンド書き出しを開始する
    pub(crate) fn start(doc: Document, target_path: PathBuf) -> Self {
        let target_path = crate::project_io::resolve_target_path(&target_path);
        let tmp_path = tmp_export_path(&target_path);
        let total_frames = export_frames(&doc);
        let completed_frames = Arc::new(AtomicUsize::new(0));
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let thread_target = target_path.clone();
        let thread_done = Arc::clone(&completed_frames);
        let thread_cancel = Arc::clone(&cancel_flag);
        let handle = std::thread::spawn(move || {
            let res = run_video_export(
                &doc,
                &thread_target,
                &tmp_path,
                &thread_done,
                &thread_cancel,
            );
            if res != VideoExportOutcome::Completed {
                let _ = std::fs::remove_file(&tmp_path);
            }
            res
        });

        Self {
            target_path,
            completed_frames,
            total_frames,
            cancel_flag,
            handle: Some(handle),
        }
    }

    /// 現在の進捗 `(完了したコマ数, 全コマ数)` を返す
    #[inline]
    pub(crate) fn progress(&self) -> (usize, usize) {
        (
            self.completed_frames.load(Ordering::Acquire),
            self.total_frames,
        )
    }

    /// 裏の処理が終わっていれば結果を取り出す（まだ動いていれば `None`）
    pub(crate) fn try_take_outcome(&mut self) -> Option<VideoExportOutcome> {
        if !self.handle.as_ref().is_some_and(|h| h.is_finished()) {
            return None;
        }
        Some(self.wait())
    }

    /// 裏の処理が終わるまで待って結果を返す
    pub(crate) fn wait(&mut self) -> VideoExportOutcome {
        match self.handle.take() {
            Some(handle) => handle.join().unwrap_or_else(|_| {
                VideoExportOutcome::Failed("書き出し処理が異常終了しました".to_owned())
            }),
            None => VideoExportOutcome::Cancelled,
        }
    }

    /// 書き出しに中止の合図を送る（待たない。結果は `try_take_outcome` で受け取る）
    pub(crate) fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::Release);
    }

    /// 中止の合図を送ったか
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::Acquire)
    }

    /// 書き出しに中止の合図を送り、裏の処理が終わるまで待って結果を返す
    pub(crate) fn cancel_and_wait(&mut self) -> VideoExportOutcome {
        self.cancel();
        self.wait()
    }
}

impl Drop for VideoExportJob {
    fn drop(&mut self) {
        if self.handle.is_some() {
            let _ = self.cancel_and_wait();
        }
    }
}

/// 書き出しでコマを塗るスレッドの数の上限
const MAX_RENDER_THREADS: usize = 4;

/// コマ 0 から最後のコマまでを、`threads` コマずつ別々のスレッドで同時に塗り、コマの順に `push` へ渡す
pub(crate) fn render_frames_in_order(
    doc: &Document,
    (width, height): (u32, u32),
    threads: usize,
    cancel_flag: &AtomicBool,
    mut push: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> VideoExportOutcome {
    let total_frames = export_frames(doc);
    // 前の組のコマで展開した地図の写し（次の組を塗る間持つので、続くコマの同じキーフレームの地図を作り直さない）
    let mut held: Vec<Arc<LayerBase>> = Vec::new();
    for first in (0..total_frames).step_by(threads) {
        if cancel_flag.load(Ordering::Acquire) {
            return VideoExportOutcome::Cancelled;
        }
        let rendered: Option<Vec<Rendered>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (first..(first + threads).min(total_frames))
                .map(|frame| scope.spawn(move || render_frame_holding(doc, frame, width, height)))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let Some(rendered) = rendered else {
            return VideoExportOutcome::Failed(format!(
                "{width}×{height} の画像を作れませんでした"
            ));
        };
        let (frames, bases): (Vec<Vec<u8>>, Vec<Vec<Arc<LayerBase>>>) =
            rendered.into_iter().unzip();
        // 前の組の写しは、この組を塗り終えてから手放す
        drop(std::mem::replace(
            &mut held,
            bases.into_iter().flatten().collect(),
        ));
        if cancel_flag.load(Ordering::Acquire) {
            return VideoExportOutcome::Cancelled;
        }
        for (frame, rgba) in (first..).zip(&frames) {
            if let Err(err) = push(frame, rgba) {
                return VideoExportOutcome::Failed(err);
            }
        }
    }
    VideoExportOutcome::Completed
}

fn run_video_export(
    doc: &Document,
    target_path: &Path,
    tmp_path: &Path,
    completed_frames: &AtomicUsize,
    cancel_flag: &AtomicBool,
) -> VideoExportOutcome {
    if target_path.file_name().is_none() {
        return VideoExportOutcome::Failed("書き出し先のファイル名が正しくありません".to_owned());
    }
    let _ = std::fs::remove_file(tmp_path);
    if cancel_flag.load(Ordering::Acquire) {
        return VideoExportOutcome::Cancelled;
    }

    let (width, height) = export_dimensions(doc.stage_size);
    let fps = doc.fps.clamp(1, crate::symbol::MAX_FPS);

    // 音のライブラリが空なら、音を入れない（今までどおり音声なしの MP4）。音があれば、鳴らない所は無音で、いつも入れる
    let audio = (!doc.sounds.is_empty()).then(|| export_audio(doc));
    let mut encoder = match create_video_encoder(tmp_path, width, height, fps, audio) {
        Ok(enc) => enc,
        Err(err) => return VideoExportOutcome::Failed(err),
    };

    let outcome = render_frames_in_order(
        doc,
        (width, height),
        std::thread::available_parallelism().map_or(1, |n| n.get().min(MAX_RENDER_THREADS)),
        cancel_flag,
        |frame, rgba| {
            encoder.push_frame(rgba)?;
            completed_frames.store(frame + 1, Ordering::Release);
            Ok(())
        },
    );
    if outcome != VideoExportOutcome::Completed {
        return outcome;
    }

    if cancel_flag.load(Ordering::Acquire) {
        return VideoExportOutcome::Cancelled;
    }
    if let Err(err) = encoder.finish() {
        return VideoExportOutcome::Failed(err);
    }
    if let Err(err) = std::fs::rename(tmp_path, target_path) {
        return VideoExportOutcome::Failed(format!("動画ファイルの保存に失敗しました: {err}"));
    }
    VideoExportOutcome::Completed
}
