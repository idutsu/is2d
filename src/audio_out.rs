//! 音の出力：再生とスクラブの音を鳴らし、鳴らしたサンプルの数を数える（音の時計）。設計：claude/オーディオの設計_9693dbf.md
//! の 5 章
//!
//! 鳴らす中身（`Job`：文書の写しと、タイムラインのどこからどこまで）を1つ持つ。出力の装置が次の分を求めるたびに
//! `sound::mix` で作って渡し、渡した数を共有の数（`Shared::played`）に足す。再生のコマは、画面がこの数から、まだ耳に
//! 届いていない数（`Shared::ahead`：渡した分と、出力の遅れ）を引いた、鳴った所から決める（`AudioOut::position` →
//! `sound::frame_at`。Bluetooth など遅れの大きい出力でも、絵が耳に届く音より先に進まない）。装置が使えないときは、同じ数を画面の時刻で同じ速さに進める
//! （`Output::Clock`。鳴らさない）ので、コマの決め方は装置の有る無しによらず1つ
//!
//! 装置は、鳴らし始めるときに決める（`AudioOut::follow_default_device`）：OS の既定の出力の装置が、開いた（開こうとした）
//! ものと違えば、その場で開き直し、遅れもそのとき聞く。開けなければ、その装置に替わるまで時計で数える。開いていた装置が
//! 使えなくなったら開いた記録を消すので、同じ装置をつなぎ直したときも、次に鳴らし始めるときに開き直す。起動のときに
//! 装置が無いときも、装置が使えなくなったときも、同じ決まりで、次に既定の装置が替わった（つないだ・つなぎ直した・OS が
//! 切り替えた）ときから鳴る

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use eframe::egui;

// 装置が受け取ってから耳に届くまでの遅れ：macOS は OS に聞く（OS の関数を呼ぶので、macOS の置き場所にある）
#[cfg(target_os = "macos")]
use crate::audio_latency_macos::output_latency;
use crate::sound::{frame_at, frame_start, mix, SOUND_RATE};
use crate::symbol::Document;
use crate::timeline::Timeline;

/// タイムラインの鳴らす範囲（出力のサンプルの番号。コマ 0 の始まりが 0：`sound::mix` と同じ数え方）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    /// 鳴らし始める所
    start: u64,
    /// 終わり（この手前まで）。`looping` なら、ここで 0 へ戻ってくり返す
    end: u64,
    looping: bool,
}

impl Span {
    /// コマ `first` の始まりから、コマ `end` の手前まで（`first < end`。出力の速さ `rate`・作品の fps `fps`）
    pub(crate) fn frames(first: usize, end: usize, looping: bool, rate: u32, fps: u32) -> Self {
        Self {
            start: frame_start(first, rate, fps),
            end: frame_start(end, rate, fps),
            looping,
        }
    }

    /// 鳴らし始めてから `played` 個のサンプルを鳴らした所の、タイムラインのサンプルの番号。くり返さず、終わりまで鳴らし
    /// 終えていれば `None`
    pub(crate) fn position(&self, played: u64) -> Option<u64> {
        let p = self.start + played;
        if self.looping {
            Some(p % self.end)
        } else {
            (p < self.end).then_some(p)
        }
    }

    /// 鳴らし始めてから `played` 個のサンプルを鳴らした所を含む、続けて鳴らすコマの範囲（出力の速さ `rate`・作品の fps
    /// `fps`）：初めは鳴らし始めたコマから、くり返して 0 へ戻った後はコマ 0 から、終わりのコマの手前まで
    fn run_frames(&self, played: u64, rate: u32, fps: u32) -> std::ops::Range<usize> {
        let start = if self.start + played < self.end {
            self.start
        } else {
            0
        };
        frame_at(start, rate, fps)..frame_at(self.end, rate, fps)
    }
}

/// 鳴らす中身：文書の写し（鳴らし始めたときのもの。文書を変える操作は再生を止める：`apply`）と、鳴らすタイムライン
/// （ルートか、編集中のシンボルの：画面が再生するもの）と、その範囲
pub(crate) struct Job {
    doc: Document,
    timeline: Timeline,
    span: Span,
}

impl Job {
    /// 文書 `doc` のタイムライン `tl` の、コマ `first` の始まりからコマ `end` の手前まで（`first < end`）を、出力の速さ
    /// `rate` で鳴らす中身（`looping` なら、終わりでコマ 0 へ戻ってくり返す）
    pub(crate) fn new(
        doc: &Document,
        tl: &Timeline,
        first: usize,
        end: usize,
        looping: bool,
        rate: u32,
    ) -> Self {
        Self {
            doc: doc.clone(),
            timeline: tl.clone(),
            span: Span::frames(first, end, looping, rate, doc.fps),
        }
    }

    /// `played` 個鳴らした所から、出力の速さ `rate` で `out`（左・右・左・右…）を埋め、範囲の音で埋めた数（左右の組の数）
    /// を返す。範囲の終わりで 0 へ戻る所は、戻る前と後を、それぞれの続けて鳴らす範囲（`Span::run_frames`）で別々に混ぜる
    /// （`mix` は続いた区間を作り、範囲の端で鳴り始め・鳴り終わりをなめらかにするので）。鳴らし終えた後は無音
    pub(crate) fn fill(&self, played: u64, rate: u32, out: &mut [f32]) -> usize {
        let frames = out.len() / 2;
        let mut done = 0;
        while done < frames {
            let Some(pos) = self.span.position(played + done as u64) else {
                break;
            };
            let run = (frames - done).min((self.span.end - pos) as usize);
            out[2 * done..2 * (done + run)].copy_from_slice(&mix(
                &self.doc,
                &self.timeline,
                self.span
                    .run_frames(played + done as u64, rate, self.doc.fps),
                pos,
                run,
                rate,
            ));
            done += run;
        }
        out[2 * done..].fill(0.0);
        done
    }
}

/// 出力の装置の側（別のスレッド）と画面が共有するもの
struct Shared {
    /// 出力の速さ（1 秒あたりのサンプルの数）
    rate: u32,
    /// 鳴らす中身（無ければ無音）
    job: Mutex<Option<Job>>,
    /// 今の中身を鳴らし始めてから、装置に渡したサンプルの数（音の時計）
    played: AtomicU64,
    /// 渡した数のうち、最後に装置が求めたときにまだ耳に届いていなかった数：そのとき渡した分と、装置がその分を鳴らし
    /// 始めるまで（cpal が知らせる）と、装置が受け取ってから耳に届くまで（OS が知らせる：`output_latency`）
    ahead: AtomicU64,
    /// 中身を鳴らし終え、渡した分が装置で鳴り終わった（画面が見て装置を止める：止めないと、無音でも Mac がスリープ
    /// しない）
    idle: AtomicBool,
    /// 装置が使えない・使えなくなったわけ（画面が見て1回だけ知らせ、時計に切り替える）
    failed: Mutex<Option<String>>,
    /// 混ぜる途中で止まった（内部の不具合）か（画面が取り出して知らせる：`AudioOut::tick`）
    mix_failed: AtomicBool,
    /// 上の2つを、操作が無いときにも画面が見るよう、画面を起こす
    wake: Option<egui::Context>,
}

impl Shared {
    fn new(rate: u32, wake: Option<egui::Context>) -> Self {
        Self {
            rate,
            job: Mutex::new(None),
            played: AtomicU64::new(0),
            ahead: AtomicU64::new(0),
            idle: AtomicBool::new(false),
            failed: Mutex::new(None),
            mix_failed: AtomicBool::new(false),
            wake,
        }
    }

    fn job(&self) -> std::sync::MutexGuard<'_, Option<Job>> {
        self.job.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 装置が次の `out`（左・右…）を求めたとき：今の中身で埋め、渡した数を進める。`delay` は、渡す分を装置が鳴らし始めて
    /// から耳に届くまでのサンプルの数（まだ耳に届いていない数は、これと渡す分の和）。`idle_after` は範囲の音で埋めた数を
    /// 受けて、もう鳴り終わったかを返す。中身を取り替える所（`AudioOut::play`）と食い違わないよう、ここまでを中身の鍵の
    /// 中で行う
    fn render(&self, out: &mut [f32], delay: u64, idle_after: impl FnOnce(usize) -> bool) {
        let job = self.job();
        let played = self.played.load(Ordering::Relaxed);
        let covered = job.as_ref().map_or(0, |j| j.fill(played, self.rate, out));
        if job.is_none() {
            out[2 * covered..].fill(0.0);
        }
        self.ahead
            .store(delay + (out.len() / 2) as u64, Ordering::Relaxed);
        self.played
            .store(played + (out.len() / 2) as u64, Ordering::Relaxed);
        if idle_after(covered) && !self.idle.swap(true, Ordering::Relaxed) {
            self.wake_screen();
        }
    }

    /// 装置の呼び出しの途中で止まったとき（panic：装置の呼び出しの砦が受け止める。呼び出し1回が仕事の単位）：鳴らしている
    /// 中身を外し（同じ所で止まり続けない）、渡した `frames` 個の数を進め、画面に知らせる。装置は使い続ける
    fn stopped(&self, frames: usize) {
        *self.job() = None;
        self.played.fetch_add(frames as u64, Ordering::Relaxed);
        self.mix_failed.store(true, Ordering::Relaxed);
        self.wake_screen();
    }

    fn fail(&self, why: String) {
        *self.failed.lock().unwrap_or_else(PoisonError::into_inner) = Some(why);
        self.wake_screen();
    }

    fn wake_screen(&self) {
        if let Some(ctx) = &self.wake {
            ctx.request_repaint();
        }
    }
}

/// 鳴らす先
enum Output {
    /// 出力の装置
    Device(cpal::Stream),
    /// 装置が使えないとき：画面の時刻（秒）`since` から、`base` 個に、装置と同じ速さで数を足していく（鳴らさない）。
    /// `since` は、鳴らし始めてから最初に数を読んだときの時刻
    Clock { since: Option<f64>, base: u64 },
}

/// 音の出力（再生とスクラブで鳴らす中身を1つ持つ）
pub(crate) struct AudioOut {
    shared: Arc<Shared>,
    output: Output,
    /// 今の中身の範囲（画面がコマを決めるための写し。中身が無ければ `None`）
    span: Option<Span>,
    /// 装置を動かしているか
    running: bool,
    /// 装置を開く出力（`open`）なら、画面（装置の側が画面を起こす先）。装置を使わない出力（`silent`）は `None`（開かない）
    wake: Option<egui::Context>,
    /// 最後に開いた（開こうとした）ときの、既定の出力の装置の名前（装置が無ければ `None`）。まだ開こうとしていなければ
    /// `None`（外側）
    tried: Option<Option<String>>,
}

impl AudioOut {
    /// 装置を使わない出力（時計だけで数える。速さは音の中の位置の単位 `SOUND_RATE`）。テストと、装置が使えないとき
    pub(crate) fn silent() -> Self {
        Self {
            shared: Arc::new(Shared::new(SOUND_RATE as u32, None)),
            output: Output::Clock {
                since: None,
                base: 0,
            },
            span: None,
            running: false,
            wake: None,
            tried: None,
        }
    }

    /// 既定の出力の装置を開く出力。開けなければ時計で数え、わけを `tick` が1回だけ返す（画面が知らせる）
    pub(crate) fn open(ctx: &egui::Context) -> Self {
        let mut out = Self::silent();
        out.wake = Some(ctx.clone());
        out.follow_default_device();
        out
    }

    /// 装置を開く出力なら、OS の既定の出力の装置が、最後に開いた（開こうとした）ものと違うときだけ、開き直す（装置の
    /// 遅れも聞き直す：`open_device`）。開けなければ時計で数え、わけを `tick` が1回だけ返す。開き直すのは既定の装置が
    /// 替わったときだけなので、スクラブ（コマごとに鳴らし始める）で毎回開かない
    fn follow_default_device(&mut self) {
        let Some(ctx) = self.wake.clone() else {
            return;
        };
        let now = default_output_name();
        if !needs_reopen(self.tried.as_ref(), &now) {
            return;
        }
        self.tried = Some(now);
        self.running = false;
        (self.shared, self.output) = match open_device(&ctx) {
            Ok((shared, stream)) => (shared, Output::Device(stream)),
            Err(why) => {
                let shared = Arc::new(Shared::new(SOUND_RATE as u32, Some(ctx)));
                shared.fail(why);
                (shared, Output::Clock { since: None, base: 0 })
            }
        };
    }

    /// 出力の速さ（1 秒あたりのサンプルの数）
    pub(crate) fn rate(&self) -> u32 {
        self.shared.rate
    }

    /// 文書 `doc` のタイムライン `tl` の、コマ `first` の始まりからコマ `end` の手前まで（`first < end`）を鳴らす
    /// （`looping` なら、終わりでコマ 0 へ戻ってくり返す）。鳴らしている中身は打ち切り、数は 0 から数え直す。鳴らす装置は
    /// ここで決める（`follow_default_device`）
    pub(crate) fn play(
        &mut self,
        doc: &Document,
        tl: &Timeline,
        first: usize,
        end: usize,
        looping: bool,
    ) {
        self.follow_default_device();
        let job = Job::new(doc, tl, first, end, looping, self.rate());
        self.span = Some(job.span);
        {
            // 中身・数・鳴り終わりを、装置の側（`Shared::render`）と食い違わないよう、中身の鍵の中でそろえて変える
            let mut current = self.shared.job();
            *current = Some(job);
            self.shared.played.store(0, Ordering::Relaxed);
            self.shared.idle.store(false, Ordering::Relaxed);
        }
        match &mut self.output {
            Output::Device(stream) => {
                if !self.running {
                    use cpal::traits::StreamTrait;
                    match stream.play() {
                        Ok(()) => self.running = true,
                        Err(e) => self.shared.fail(e.to_string()),
                    }
                }
            }
            Output::Clock { since, base } => (*since, *base) = (None, 0),
        }
    }

    /// 鳴らしている中身をやめる（装置に渡した分は鳴り終わるまで鳴り、装置は `tick` が止める）
    pub(crate) fn stop(&mut self) {
        self.span = None;
        *self.shared.job() = None;
    }

    /// 今の中身の、耳に届いた所の、タイムラインのサンプルの番号（再生のコマはここから：`sound::frame_at`）：渡した数から、
    /// まだ耳に届いていない数を引いた所（鳴らし始めてすぐ、まだ何も届いていない間は、鳴らし始めた所）。中身が無いか、
    /// くり返さずに鳴らし終えていれば `None`。`now` は画面の時刻（秒。時計で数えるときだけ使う。時計には遅れが無い）
    pub(crate) fn position(&mut self, now: f64) -> Option<u64> {
        let rate = f64::from(self.rate());
        if let Output::Clock { since, base } = &mut self.output {
            let elapsed = (now - *since.get_or_insert(now)).max(0.0);
            let n = *base + (elapsed * rate).floor() as u64;
            self.shared.played.store(n, Ordering::Relaxed);
        }
        let ahead = self.shared.ahead.load(Ordering::Relaxed);
        self.span?
            .position(self.shared.played.load(Ordering::Relaxed).saturating_sub(ahead))
    }

    /// 画面の毎コマ：装置が鳴り終わっていれば止め、装置が使えなくなっていれば時計に切り替えて、わけを返す（1回だけ）
    pub(crate) fn tick(&mut self) -> Option<String> {
        if self.shared.mix_failed.swap(false, Ordering::Relaxed) {
            return Some("内部の不具合で、音を鳴らせなかった所があります（作品は変わっていません）".to_owned());
        }
        let failed = self
            .shared
            .failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(why) = failed {
            if matches!(self.output, Output::Device(_)) {
                // 時計は耳に届いた所から続ける（時計には遅れが無い）
                let ahead = self.shared.ahead.swap(0, Ordering::Relaxed);
                self.output = Output::Clock {
                    since: None,
                    base: self.shared.played.load(Ordering::Relaxed).saturating_sub(ahead),
                };
                self.running = false;
                // 開いていた装置が使えなくなったので、開いた記録を消す：次に鳴らし始めるとき、既定の装置の名前が同じでも
                // （同じイヤホンを外して付け直した）開き直す（`follow_default_device`。レビュー d91cd4c の N-4001 の残り：
                // 41766f9 の N-4001）。開こうとして開けなかったとき（`Output::Clock` のまま知らせを受けたとき）は消さない
                // （同じ装置で鳴らすたびに開こうとしない）
                self.tried = None;
            }
            return Some(format!(
                "音の出力が使えません（{why}）。再生はできますが、音は鳴りません（出力の装置をつなぎ直すか、ほかの装置に切り替えると、次に鳴らすときから鳴ります）"
            ));
        }
        if let Output::Device(stream) = &self.output {
            if self.running && self.shared.idle.load(Ordering::Relaxed) {
                use cpal::traits::StreamTrait;
                self.running = false;
                if let Err(e) = stream.pause() {
                    self.shared.fail(e.to_string());
                }
            }
        }
        None
    }
}

/// 鳴らし始めるときに装置を開き直すか：最後に開いた（開こうとした）ときの既定の出力の装置の名前 `tried`（まだ開こうと
/// していなければ `None`）と、今の既定の出力の装置の名前 `now`（装置が無ければ `None`）が違うとき。同じ装置で開けな
/// かった・使えなくなったときは、既定の装置が替わるまで開き直さない（開くのは時間がかかることがあるので、鳴らすたびに
/// 試さない）
fn needs_reopen(tried: Option<&Option<String>>, now: &Option<String>) -> bool {
    tried != Some(now)
}

/// 今の既定の出力の装置の名前（装置を見分ける：`needs_reopen`。名前を聞けなければ空。装置が無ければ `None`）
fn default_output_name() -> Option<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    cpal::default_host()
        .default_output_device()
        .map(|d| d.name().unwrap_or_default())
}

/// 既定の出力の装置を、その装置の既定の形で開く（止めたまま）。サンプルの形は装置の既定に合わせる
fn open_device(ctx: &egui::Context) -> Result<(Arc<Shared>, cpal::Stream), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("出力の装置がありません")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let shared = Arc::new(Shared::new(config.sample_rate.0, Some(ctx.clone())));
    let latency = output_latency();
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, &shared, latency),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, &shared, latency),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, &shared, latency),
        f => return Err(format!("サンプルの形 {f} に対応していません")),
    }
    .map_err(|e| e.to_string())?;
    stream.pause().map_err(|e| e.to_string())?;
    Ok((shared, stream))
}

/// サンプルの形 `T` の出力を作る。装置が求めるたびに `Shared::render` で左右の波形を作り、装置の音の数に合わせて書く
/// （1つなら左右の平均、2つ以上なら前の2つに左右、残りは無音）。鳴り終わりは、範囲の音の後ろの無音が装置で鳴り始める
/// 時刻（装置が知らせる、渡した分を鳴らす時刻から）を、装置が次を求める時刻が過ぎたときとする。`latency` は、装置が
/// 受け取ってから耳に届くまでのサンプルの数（`output_latency`）
fn build<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: &Arc<Shared>,
    latency: u64,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    use cpal::traits::DeviceTrait;
    let channels = usize::from(config.channels.max(1));
    let rate = f64::from(config.sample_rate.0);
    let (render, fail) = (Arc::clone(shared), Arc::clone(shared));
    let mut stereo: Vec<f32> = Vec::new();
    let mut quiet_from: Option<cpal::StreamInstant> = None;
    device.build_output_stream(
        config,
        // 装置の呼び出しの入口（呼び出し1回が仕事の単位。途中で止まったら、その分を無音にして `Shared::stopped`：設計
        // claude/最後の砦を仕事の入口に置く設計.md。前は止まるとアプリごと落ちた）
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let frames = data.len() / channels;
            if crate::vector_engine::guarded(|| {
                stereo.resize(2 * frames, 0.0);
                let ts = info.timestamp();
                // 渡す分を装置が鳴らし始めるまで（cpal が知らせる）と、耳に届くまで
                let start = ts.playback.duration_since(&ts.callback).unwrap_or_default();
                let delay = (start.as_secs_f64() * rate).round() as u64 + latency;
                render.render(&mut stereo, delay, |covered| {
                    if covered > 0 || quiet_from.is_none() {
                        quiet_from = ts
                            .playback
                            .add(Duration::from_secs_f64(covered as f64 / rate));
                    }
                    covered < frames && quiet_from.is_some_and(|q| ts.callback >= q)
                });
                for (o, lr) in data.chunks_exact_mut(channels).zip(stereo.chunks_exact(2)) {
                    for (c, x) in o.iter_mut().enumerate() {
                        let v = match (channels, c) {
                            (1, _) => (lr[0] + lr[1]) * 0.5,
                            (_, 0 | 1) => lr[c],
                            _ => 0.0,
                        };
                        *x = T::from_sample(v);
                    }
                }
            })
            .is_none()
            {
                data.fill(T::from_sample(0.0f32));
                render.stopped(frames);
            }
        },
        // 装置が使えなくなった知らせの入口（止まったら、その知らせを捨てる）
        move |e| {
            let _ = crate::vector_engine::guarded(|| fail.fail(e.to_string()));
        },
        None,
    )
}

/// macOS のほかでは、装置が受け取ってから耳に届くまでの遅れを OS に聞かない（0。cpal が知らせる、装置が鳴らし始める
/// までだけを数える）
#[cfg(not(target_os = "macos"))]
fn output_latency() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::needs_reopen;

    /// レビュー d91cd4c の N-4001：装置は鳴らし始めるときに決め、既定の出力の装置が替わったときだけ開き直す（前は起動の
    /// ときに1回だけ開き、使えなくなったら、ほかの装置があってもアプリを終えるまで鳴らなかった）
    #[test]
    fn the_output_follows_the_default_device_when_it_changes() {
        let (earphones, speakers) = (Some("イヤホン".to_owned()), Some("内蔵スピーカー".to_owned()));
        // 起動：まだ開こうとしていない
        assert!(needs_reopen(None, &earphones));
        assert!(needs_reopen(None, &None), "起動のときに装置が無いことも知らせる");
        // 同じ装置のまま：鳴らすたびに開き直さない（使えなくなっていても、替わるまで試さない）
        assert!(!needs_reopen(Some(&earphones), &earphones));
        // イヤホンが切れて、OS が内蔵スピーカーに切り替えた：開き直す（遅れも聞き直す）
        assert!(needs_reopen(Some(&earphones), &speakers));
        // 装置が無くなった：開き直す（開けないので時計で数える）。装置がつながったら開き直す
        assert!(needs_reopen(Some(&speakers), &None));
        assert!(!needs_reopen(Some(&None), &None));
        assert!(needs_reopen(Some(&None), &earphones));
        // 開いていたイヤホンが使えなくなった（開いた記録を消す：`AudioOut::tick`）あと、同じイヤホンをつなぎ直した：名前は同じ
        // でも開き直す（41766f9 の N-4001）
        assert!(needs_reopen(None, &earphones));
    }
}
