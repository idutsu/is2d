//! 音（読み込み・保存・鳴らす・書き出し）のテスト

use super::*;

/// テスト用：タイムライン `tl` の1枚目のレイヤーを `frames` コマにし、コマ 0 のキーフレームに音 1 の区間 `clip` を付ける
fn put_clip(tl: &mut timeline::Timeline, frames: usize, clip: crate::sound::SoundClip) {
    tl.layers[0].frame_count = frames;
    tl.layers[0].keyframe_mut(0).sound = Some(clip);
    tl.total_frames = tl.total_frames.max(frames);
}

fn flat_clip() -> crate::sound::SoundClip {
    crate::sound::SoundClip {
        sound: 1,
        start: 0,
        end: crate::sound::SOUND_RATE,
    }
}

/// テスト用：fps 24 で、ルートの1枚目のレイヤー 10 コマのコマ 0 に、のこぎりの形の 1 秒の音（位置ごとに値が違う）を
/// 付けた文書（音はレイヤーの終わりのコマ 10 で切れる）
fn doc_with_saw_sound() -> symbol::Document {
    let mut doc = symbol::Document::default();
    doc.fps = 24;
    let samples: Vec<i16> = (0..48_000).map(|i| ((i % 4000) * 8 - 16000) as i16).collect();
    let bytes = wav_bytes(48_000, 1, &samples);
    let s = crate::sound::Sound::new(1, "saw".to_owned(), bytes.into()).unwrap();
    doc.sounds.push_new(std::sync::Arc::new(s));
    doc.next_sound_id = crate::symbol::Counter::starting_at(2);
    put_clip(&mut doc.root, 10, flat_clip());
    doc
}

/// 出力の装置が無いとき（テストもこれ）は、画面の時刻で、装置と同じ速さで同じ数を数える。鳴らし始めてから最初に読んだ
/// 時刻から数え、鳴らし直すと 0 から数え直す
#[test]
fn silent_output_counts_on_the_screen_clock() {
    use crate::sound::frame_start;
    let doc = doc_with_saw_sound();
    let mut out = crate::audio_out::AudioOut::silent();
    assert_eq!(out.rate(), 48_000);
    assert_eq!(out.tick(), None);
    assert_eq!(out.position(3.0), None, "鳴らす中身が無い");
    let s2 = frame_start(2, 48_000, 24);
    out.play(&doc, &doc.root, 2, 10, false);
    assert_eq!(out.position(5.0), Some(s2));
    assert_eq!(out.position(5.25), Some(s2 + 12_000));
    // 時刻は2進で割り切れる値にする（0.375 秒 = 18,000 サンプル = 9 コマ）
    assert_eq!(out.position(5.375), None, "8 コマを鳴らし終えた");
    out.play(&doc, &doc.root, 2, 10, true);
    assert_eq!(out.position(9.0), Some(s2));
    assert_eq!(out.position(9.375), Some(frame_start(1, 48_000, 24)), "コマ 10 でコマ 0 へ戻った");
    out.stop();
    assert_eq!(out.position(10.0), None);
}

/// 再生のコマは、鳴らしたサンプルの数から決まる（設計の 5.1：始めたコマ ＋ ⌊鳴らした秒 × fps⌋）。くり返すときは
/// 終わりで 0 へ戻り、くり返さないときは最後のコマを鳴らし終えたら終わる。コマの境目はサンプル1つの所で変わる
#[test]
fn playback_frame_follows_the_played_samples() {
    use crate::audio_out::Span;
    use crate::sound::{frame_at, frame_start};
    // 44.1kHz・fps 24 は割り切れない（1コマ 1837.5 サンプル）
    let (rate, fps) = (44_100, 24);
    let frame = |span: &Span, played: u64| span.position(played).map(|p| frame_at(p, rate, fps));
    let looping = Span::frames(3, 10, true, rate, fps);
    let once = Span::frames(3, 10, false, rate, fps);
    for k in 0..30usize {
        // 始めてから k コマ目の半ば
        let played = (2 * k as u64 + 1) * u64::from(rate) / (2 * u64::from(fps));
        assert_eq!(frame(&looping, played), Some((3 + k) % 10));
        assert_eq!(frame(&once, played), (3 + k < 10).then_some(3 + k));
    }
    let to_end = frame_start(10, rate, fps) - frame_start(3, rate, fps);
    assert_eq!(frame(&once, to_end - 1), Some(9));
    assert_eq!(frame(&once, to_end), None);
    assert_eq!(looping.position(to_end), Some(0));
    let f2 = frame_start(2, rate, fps);
    assert_eq!(frame(&looping, to_end + f2 - 1), Some(1));
    assert_eq!(frame(&looping, to_end + f2), Some(2));
}

/// くり返す再生は、終わりで 0 へ戻る所を、戻る前（コマ 8 から）と後（コマ 0 から）の2つの `mix` に分けて混ぜ、続けて
/// 鳴らしたのと同じ波形になる。装置が細かく区切って求めても同じ。音は戻る前のコマの終わりでなめらかに止み、戻った後は
/// 頭から鳴り直す。くり返さなければ、終わりから後は無音
#[test]
fn looping_playback_mixes_across_the_wrap_like_two_mixes() {
    use crate::audio_out::Job;
    use crate::sound::{frame_start, mix};
    let doc = doc_with_saw_sound();
    let rate = 44_100;
    let (s8, end) = (frame_start(8, rate, 24), frame_start(10, rate, 24));
    let before = (end - s8) as usize;
    let n = before + 3000;
    let job = Job::new(&doc, &doc.root, 8, 10, true, rate);
    let mut whole = vec![0.0f32; 2 * n];
    assert_eq!(job.fill(0, rate, &mut whole), n);
    let mut expected = mix(&doc, &doc.root, 8..10, s8, before, rate);
    expected.extend(mix(&doc, &doc.root, 0..10, 0, n - before, rate));
    assert_eq!(whole, expected);

    let mut pieces = vec![0.0f32; 2 * n];
    let mut played = 0u64;
    for chunk in pieces.chunks_mut(2 * 333) {
        assert_eq!(job.fill(played, rate, chunk), chunk.len() / 2);
        played += (chunk.len() / 2) as u64;
    }
    assert_eq!(pieces, whole);

    assert!(whole[2 * (before - 1)].abs() < 0.01, "戻る直前はほぼ止んでいる");
    assert!(whole[2 * (before + 200)].abs() > 0.1, "戻った後は頭から鳴っている");

    let once = Job::new(&doc, &doc.root, 8, 10, false, rate);
    let mut out = vec![1.0f32; 2 * n];
    assert_eq!(once.fill(0, rate, &mut out), before);
    assert_eq!(out[..2 * before], whole[..2 * before]);
    assert!(out[2 * before..].iter().all(|&x| x == 0.0));
}

/// スクラブは、そのコマの1コマ分（1/fps 秒）だけを鳴らす。画面では、再生していないときに同じタイムラインのコマが
/// 変わったときだけ鳴らし（最初に見たとき・同じコマ・再生中は鳴らさない）、新しいスクラブは前のを打ち切る
#[test]
fn scrub_plays_one_frame_when_the_frame_changes() {
    use crate::audio_out::Job;
    use crate::sound::{frame_start, mix};
    let doc = doc_with_saw_sound();
    let rate = 44_100;
    let s3 = frame_start(3, rate, 24);
    let len = (frame_start(4, rate, 24) - s3) as usize;
    assert!(len == 1837 || len == 1838, "1/24 秒は 1837.5 サンプル");
    let mut out = vec![1.0f32; 2 * 4000];
    assert_eq!(Job::new(&doc, &doc.root, 3, 4, false, rate).fill(0, rate, &mut out), len);
    assert_eq!(out[..2 * len], mix(&doc, &doc.root, 3..4, s3, len, rate)[..]);
    assert!(out[2 * len..].iter().all(|&x| x == 0.0));

    let mut app = Is2dApp::default();
    app.doc.fps = 24;
    app.doc.root.layers[0].frame_count = 10;
    app.doc.root.sync_total_frames();
    let rate = app.timeline_ui.audio.rate();
    let start = |f: usize| frame_start(f, rate, 24);
    timeline_ui::scrub_on_frame_change(&mut app);
    assert_eq!(app.timeline_ui.audio.position(0.0), None, "最初に見たときは鳴らさない");
    timeline_ui::go_to_frame(&mut app, 3);
    timeline_ui::scrub_on_frame_change(&mut app);
    assert_eq!(app.timeline_ui.audio.position(0.0), Some(start(3)));
    timeline_ui::go_to_frame(&mut app, 5);
    timeline_ui::scrub_on_frame_change(&mut app);
    assert_eq!(app.timeline_ui.audio.position(1.0), Some(start(5)), "前のスクラブを打ち切る");
    // 時刻は2進で割り切れる値にする（1/64 秒 = 750 サンプル）
    assert_eq!(app.timeline_ui.audio.position(1.0 + 1.0 / 64.0), Some(start(5) + 750));
    assert_eq!(app.timeline_ui.audio.position(1.0 + 1.5 / 24.0), None, "1コマ分で止む");
    timeline_ui::scrub_on_frame_change(&mut app);
    assert_eq!(app.timeline_ui.audio.position(2.0), None, "同じコマでは鳴らさない");

    timeline_ui::toggle_playback(&mut app);
    assert_eq!(app.timeline_ui.audio.position(3.0), Some(start(5)));
    assert!(app.play_to_frame(7));
    timeline_ui::scrub_on_frame_change(&mut app);
    assert_eq!(app.timeline_ui.audio.position(3.0), Some(start(5)), "再生中はスクラブしない");
    timeline_ui::toggle_playback(&mut app);
    timeline_ui::scrub_on_frame_change(&mut app);
    assert_eq!(app.timeline_ui.audio.position(4.0), None, "止めたコマでは鳴らさない");
}

/// 音のライブラリ（`sound::SoundLibrary`）の番号で引く・バイトで引く・並びは、ふつうの並びを頭から探した答え（お手本）と
/// 同じ：でたらめに足す・消す・名前を変える（足す音には、前に足した音とバイトを共有するものも混ぜる。全体の点検 2026-11-27）
#[test]
fn sound_library_lookups_match_a_plain_scan() {
    use crate::sound::{Sound, SoundLibrary};
    use std::sync::Arc;
    let mut rng = OpRng(0x50ad_0001);
    let base = Sound::new(1, "s".to_owned(), short_wav().into()).unwrap();
    let mut lib = SoundLibrary::default();
    let mut plain: Vec<Arc<Sound>> = Vec::new();
    let mut next = 1u64;
    for step in 0..400 {
        match rng.range(4) {
            0 | 1 => {
                // バイトは、新しいものか、今ある音と共有するもの
                let mut s = match plain.get(rng.range(plain.len().max(1))).filter(|_| rng.coin()) {
                    Some(p) => Sound::clone(p),
                    None => Sound::new(next, format!("s{next}"), short_wav().into()).unwrap_or_else(|_| base.clone()),
                };
                s.id = next;
                next += 1;
                let s = Arc::new(s);
                lib.push_new(s.clone());
                plain.push(s);
            }
            2 if !plain.is_empty() => {
                let id = plain[rng.range(plain.len())].id;
                assert!(lib.remove(id));
                plain.retain(|s| s.id != id);
            }
            _ if !plain.is_empty() => {
                let i = rng.range(plain.len());
                let name = format!("n{}", rng.range(5));
                let changed = plain[i].name != name;
                assert_eq!(lib.rename(plain[i].id, name.clone()), changed);
                Arc::make_mut(&mut plain[i]).name = name;
            }
            _ => {}
        }
        assert_eq!(lib.iter().map(|s| (s.id, s.name.clone())).collect::<Vec<_>>(), plain.iter().map(|s| (s.id, s.name.clone())).collect::<Vec<_>>(), "step={step}");
        for p in &plain {
            assert_eq!(lib.get(p.id).map(|s| s.id), Some(p.id));
            let expected = plain.iter().find(|q| Arc::ptr_eq(&q.encoded, &p.encoded)).map(|q| q.id);
            assert_eq!(lib.with_bytes(p).map(|s| s.id), expected, "step={step}");
        }
        assert!(lib.get(next).is_none());
    }
}

/// 復旧ファイルの音：名前を変えても、音のデータは書き直さず、名前の小さな記録だけを書き足す（「同じデータか」は
/// `Sound::bytes_address` の1つ。前は音の箱の場所で見分け、名前を1文字変えるたびにデータを丸ごと書き足した：設計審査
/// 6fb4faa の D-2）。読み戻した音は、名前もデータも今の作品と同じ
#[test]
fn renaming_a_sound_appends_only_its_name_to_the_recovery_file() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::recovery_journal::Journal;
    let dir = std::env::temp_dir().join(format!("is2d_journal_sound_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("work.is2dj");
    let mut app = Is2dApp::default();
    // 1 秒の音（約 96KB）
    assert!(import_sound(&mut app, wav_bytes(48_000, 1, &vec![1000i16; 48_000])));
    let sound_id = app.doc.sounds.iter().next().unwrap().id;
    let mut journal = Journal::create(&path, &app.tab().committed_document_copy().unwrap(), None).unwrap();
    for name in ["音a", "音ab", "音abc"] {
        let before = std::fs::metadata(&path).unwrap().len();
        assert!(apply(&mut app, Action::RenameSound { sound_id, new_name: name.to_owned() }));
        journal.write(&app.tab().committed_document_copy().unwrap(), None).unwrap();
        let appended = std::fs::metadata(&path).unwrap().len() - before;
        assert!(appended < 1_000, "名前を変えて書き足した量 {appended} バイトが、音のデータの大きさに比例している");
        let read = read_saved_file(&path).unwrap();
        assert_eq!(read.sounds.len(), 1);
        assert_eq!(read.sounds[0].name, name);
        assert_eq!(&*read.sounds[0].bytes, &*app.doc.sounds.iter().next().unwrap().encoded);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 波形にできない音（読めないバイト）は、作品を開かないのでなく「読めない音」として開いて知らせる。バイトは残して保存し、
/// 区間もそのまま。鳴らさず、区間の端は動かせない（長さが分からない）（審査 5616fcc の D-3、持ち主が決めた 2026-11-27。
/// 直す前は作品全体を開かなかった）
#[test]
fn unreadable_sound_opens_silently_and_keeps_its_bytes() {
    let mut app = Is2dApp::default();
    assert!(import_sound(&mut app, short_wav()));
    let mut saved = saved_of(&app.doc);
    saved.sounds[0].bytes = b"broken"[..].into();
    let clip = saved.timeline.layers[0].keyframes[0].sound;
    let doc = saved.clone().into_document().expect("読めない音があっても開く");
    assert!(!doc.sounds.at(0).unwrap().readable());
    assert_eq!(&*doc.sounds.at(0).unwrap().encoded, b"broken");
    assert_eq!(
        doc.load_notices.take(),
        vec![timeline::LoadNotice::UnreadableSound(saved.sounds[0].name.clone())]
    );
    assert!(crate::sound::sounding_at(&doc, &doc.root, 0).iter().all(|v| v.sound.pcm.samples.is_empty()), "鳴らす波形が無い");
    assert!(doc.root.layers[0].sound_trim_range(0, crate::sound::SoundEdge::End, &doc.sounds, doc.fps).is_none());
    // 保存すると、バイトと区間をそのまま書く
    let again = saved_of(&doc);
    assert_eq!(&*again.sounds[0].bytes, b"broken");
    assert_eq!(format!("{:?}", again.timeline.layers[0].keyframes[0].sound), format!("{clip:?}"));
}

/// WAV は、元のサンプルの速さのまま左右2つずつの波形になり（1 つの音は左右に同じ値）、長さは音の中の位置の単位
/// （`SOUND_RATE`）で数える。WAV でも MP3 でもないバイトは読めない
#[test]
fn sounds_decode_wav_and_refuse_other_bytes() {
    let bytes = short_wav();
    let pcm = crate::sound::decode(&bytes).expect("WAV は読める");
    assert_eq!(pcm.rate, 44_100);
    assert_eq!(pcm.frames(), 4410);
    assert!(pcm.samples.chunks(2).all(|lr| lr[0] == lr[1]));
    let s = crate::sound::Sound::new(1, "a".to_owned(), bytes.clone().into()).unwrap();
    assert_eq!(s.len(), crate::sound::SOUND_RATE / 10);
    let stereo = wav_bytes(48_000, 2, &[1, 2, 3, 4, 5, 6]);
    assert_eq!(crate::sound::decode(&stereo).unwrap().samples, vec![1, 2, 3, 4, 5, 6]);
    assert!(crate::sound::decode(b"not a sound file at all").is_err());
    assert!(crate::sound::decode(&bytes[..30]).is_err());
}

/// 空白のキーフレームに音を読み込むと、音のライブラリに入り、そのキーフレームに頭から終わりまで付く。取り消すと、
/// 音のライブラリからも消え、やり直すと戻る
#[test]
fn importing_a_sound_attaches_it_to_the_blank_keyframe_with_undo() {
    let mut app = Is2dApp::default();
    assert!(import_sound(&mut app, short_wav()));
    let sounds = app.doc.sounds.clone();
    assert_eq!(sounds.len(), 1);
    let kf_sound = app.doc.root.layers[0].keyframes[0].sound;
    assert_eq!(kf_sound, crate::sound::SoundClip::whole(sounds.at(0).unwrap()));
    assert!(doc_actions::apply(&mut app, doc_actions::Action::Undo));
    assert!(app.doc.sounds.is_empty());
    assert_eq!(app.doc.root.layers[0].keyframes[0].sound, None);
    assert!(doc_actions::apply(&mut app, doc_actions::Action::Redo));
    assert_eq!(app.doc.sounds.len(), 1);
    assert_eq!(app.doc.root.layers[0].keyframes[0].sound, kf_sound);
    // 読めないバイトは、文書を変えずに断る
    assert!(!import_sound(&mut app, b"broken".to_vec()));
    assert_eq!(app.doc.sounds.len(), 1);
}

/// 音の付いたキーフレームは音だけ：絵を描こうとしても断る。絵のあるキーフレームには音を付けられない
#[test]
fn sound_keyframes_hold_only_sound() {
    let stroke = || doc_actions::Action::PaintStroke {
        points: vec![Point::new(0.0, 0.0), Point::new(40.0, 10.0)],
        radius: 6.0,
        color: Color32::RED,
    };
    let mut app = Is2dApp::default();
    assert!(import_sound(&mut app, short_wav()));
    assert!(!doc_actions::apply(&mut app, stroke()));
    assert!(app.doc.root.layers[0].keyframes[0].cel.is_empty());

    let mut app = Is2dApp::default();
    assert!(doc_actions::apply(&mut app, stroke()));
    assert!(!import_sound(&mut app, short_wav()));
    assert!(app.doc.sounds.is_empty());
    assert_eq!(app.doc.root.layers[0].keyframes[0].sound, None);
}

/// 音だけのキーフレームも空白ではない：コマ 1 で `Shift+F6` を押すと白紙にクリアされ、1コマだけのレイヤーで
/// `Shift+F5` を押しても白紙になる（どちらも取り消せる）。先頭で `F7` を押すと、音を消さずに次のコマへ進んで白紙の
/// キーフレームを作る
#[test]
fn sound_only_keyframes_are_not_blank() {
    let mut app = Is2dApp::default();
    assert!(import_sound(&mut app, short_wav()));
    let clip = app.doc.root.layers[0].keyframes[0].sound;
    assert!(clip.is_some());
    for action in [doc_actions::Action::DeleteKeyframe, doc_actions::Action::RemoveFrame] {
        assert!(doc_actions::apply(&mut app, action), "音のキーフレームは消せる");
        assert_eq!(app.doc.root.layers[0].keyframes[0].sound, None);
        assert!(doc_actions::apply(&mut app, doc_actions::Action::Undo));
        assert_eq!(app.doc.root.layers[0].keyframes[0].sound, clip);
    }
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::InsertKeyframe { copy_prev: false }
    ));
    let layer = &app.doc.root.layers[0];
    assert_eq!(layer.keyframes.len(), 2);
    assert_eq!(layer.keyframes[0].sound, clip);
    assert!(layer.keyframes[1].cel.is_empty() && layer.keyframes[1].sound.is_none());
    assert_eq!(app.doc.root.current_frame, 1);
}

/// 音は、保存して開いても、読み込んだバイト・名前・番号・キーフレームの区間が同じ
#[test]
fn sounds_survive_save_and_open() {
    let mut app = Is2dApp::default();
    assert!(import_sound(&mut app, short_wav()));
    let bytes = saved_file_bytes(&app.doc);
    let doc = project_from_bytes(&bytes)
        .unwrap()
        .into_document()
        .unwrap();
    assert_eq!(doc.sounds.len(), 1);
    assert_eq!(doc.sounds.at(0).unwrap().id, app.doc.sounds.at(0).unwrap().id);
    assert_eq!(doc.sounds.at(0).unwrap().name, "テスト.wav");
    assert_eq!(doc.sounds.at(0).unwrap().encoded, app.doc.sounds.at(0).unwrap().encoded);
    assert_eq!(doc.sounds.at(0).unwrap().pcm, app.doc.sounds.at(0).unwrap().pcm);
    assert_eq!(
        doc.root.layers[0].keyframes[0].sound,
        app.doc.root.layers[0].keyframes[0].sound
    );
    assert!(doc.next_sound_id.next() > doc.sounds.at(0).unwrap().id);
}

/// 壊れた音のデータ（無い音を指す区間・始まりが終わり以降の区間・絵のあるキーフレームの音・重なる番号）は、開くときに
/// 壊れたファイルとして断る（黙って音を外さない）。音の長さを越える区間は断らない。読めないバイトの音は断らず、「読めない音」として開く
/// （`unreadable_sound_opens_silently_and_keeps_its_bytes`）
#[test]
fn broken_sound_data_is_refused_when_opening() {
    let mut app = Is2dApp::default();
    assert!(import_sound(&mut app, short_wav()));
    let good = saved_of(&app.doc);
    assert!(good.clone().into_document().is_ok());
    let refused = |f: &dyn Fn(&mut save_format::ProjectDocument)| {
        let mut d = good.clone();
        f(&mut d);
        d.into_document().is_err()
    };
    fn kf(d: &mut save_format::ProjectDocument) -> &mut save_format::SavedKeyframe {
        &mut d.timeline.layers[0].keyframes[0]
    }
    assert!(!refused(&|d| d.sounds[0].bytes = b"broken"[..].into()), "読めない音は開く");
    assert!(refused(&|d| kf(d).sound.as_mut().unwrap().sound = 99));
    // 音の長さを越える区間は断らない（`a_clip_longer_than_its_sound_opens_with_a_notice`）
    assert!(!refused(&|d| kf(d).sound.as_mut().unwrap().end += 1), "音の長さを越える区間は開く");
    assert!(refused(&|d| {
        let c = kf(d).sound.as_mut().unwrap();
        c.start = c.end;
    }));
    assert!(refused(&|d| {
        let s = d.sounds[0].clone();
        d.sounds.push(s);
    }));
    // 絵のあるキーフレームの音
    let mut drawn = Is2dApp::default();
    assert!(doc_actions::apply(
        &mut drawn,
        doc_actions::Action::PaintStroke {
            points: vec![Point::new(0.0, 0.0), Point::new(40.0, 10.0)],
            radius: 6.0,
            color: Color32::RED,
        }
    ));
    let mut d = saved_of(&drawn.doc);
    d.sounds = good.sounds.clone();
    d.timeline.layers[0].keyframes[0].sound = good.timeline.layers[0].keyframes[0].sound;
    write_map_tiles(&mut d);
    assert!(d.into_document().is_err());
}

/// 音の区間が、開くときに読み直した音の長さより長くても（音を読む部品・読み方が変わると、同じ音の長さが変わる）、作品は
/// 開き、区間はそのまま残して「音が区間より短い」と知らせる（保存したデータを、作り直せる値で拒まない：設計審査 392e9af の
/// D-2。前は作品全体を「音が壊れています」と断った）。区間の始まりが音の終わりより後ろでも開く。開いた作品で、画面の描画・
/// 再生の音（`mix`）・端のドラッグ・F6 の分けが落ちず、音の終わりから先は鳴らない
#[test]
fn a_clip_longer_than_its_sound_opens_with_a_notice() {
    for past_end in [false, true] {
        let mut app = Is2dApp::default();
        assert!(import_sound(&mut app, short_wav()));
        let len = app.doc.sounds.at(0).unwrap().len();
        let mut d = saved_of(&app.doc);
        {
            let kf = &mut d.timeline.layers[0].keyframes[0];
            let c = kf.sound.as_mut().unwrap();
            if past_end {
                c.start = len + 10;
            }
            c.end = len + 2_000;
        }
        d.timeline.layers[0].frame_count = 30;
        d.timeline.total_frames = 30;
        let doc = d.into_document().expect("区間が音より長くても開く");
        let clip = doc.root.layers[0].keyframes[0].sound.expect("区間を残す");
        assert_eq!(clip.end, len + 2_000, "区間を変えた");
        assert!(
            doc.load_notices.take().iter().any(|n| matches!(n, crate::timeline::LoadNotice::ShortSound(name) if name == "テスト.wav")),
            "音が区間より短いことを知らせない"
        );
        // 鳴らす：音の終わりから先は無音
        let m = crate::sound::mix(&doc, &doc.root, 0..30, 0, 48_000, 48_000);
        assert!(m.iter().all(|v| v.is_finite()));
        // 開いた作品で、画面の描画・端のドラッグ・F6 の分け
        let mut app = Is2dApp::default();
        let id = crate::project_io::ProjectWorkspace::alloc_doc_id(&mut app);
        app.tab_mut().install(id, doc);
        let ctx = egui::Context::default();
        let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
        let _ = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.update_ui(ctx));
        for edge in [crate::sound::SoundEdge::End, crate::sound::SoundEdge::Start] {
            for frame in [0usize, 1, 2, 5] {
                doc_actions::apply(&mut app, doc_actions::Action::TrimSound { layer: 0, kf_start: 0, edge, frame });
                if let Some(c) = app.doc.root.layers[0].keyframes[0].sound {
                    assert!(c.start < c.end, "区間が壊れた：{c:?}");
                }
            }
        }
        doc_actions::apply(&mut app, doc_actions::Action::ScrubRulerToFrame { frame: 3 });
        doc_actions::apply(&mut app, doc_actions::Action::InsertKeyframe { copy_prev: true });
        let _ = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.update_ui(ctx));
    }
}

/// テスト用：1 秒の、値がいつも 0.5 の音（48kHz・1 つの音）を音のライブラリに入れた文書（fps 24）。音の番号は 1
fn doc_with_flat_sound() -> symbol::Document {
    let mut doc = symbol::Document::default();
    doc.fps = 24;
    let bytes = wav_bytes(48_000, 1, &vec![16384i16; 48_000]);
    let s = crate::sound::Sound::new(1, "flat".to_owned(), bytes.into()).unwrap();
    doc.sounds.push_new(std::sync::Arc::new(s));
    doc.next_sound_id = crate::symbol::Counter::starting_at(2);
    doc
}

/// ルートのキーフレームの音は、キーフレームの始まりから進んだコマの分の位置で鳴り、区間の終わりから後は鳴らない。
/// 非表示のレイヤーの音は鳴らない
#[test]
fn sounding_at_follows_the_frame_on_the_root() {
    let mut doc = doc_with_flat_sound();
    put_clip(&mut doc.root, 30, flat_clip());
    let pos = |doc: &symbol::Document, f: usize| -> Vec<u64> {
        crate::sound::sounding_at(doc, &doc.root, f).iter().map(|v| v.pos).collect()
    };
    assert_eq!(pos(&doc, 0), vec![0]);
    assert_eq!(pos(&doc, 12), vec![24_000]);
    assert_eq!(pos(&doc, 23), vec![46_000]);
    assert!(pos(&doc, 24).is_empty(), "1 秒の音は 24 コマ目から鳴らない");
    doc.root.layers[0].visible = false;
    assert!(pos(&doc, 0).is_empty());
}

/// F6 で音の区間の途中を分けると、後ろのキーフレームは続きから鳴り、前の区間は分けた所で終わる。鳴る音は分ける前と
/// 同じ（混ぜた波形もビットまで同じ：fps 24 は 48,000 を割り切る）
#[test]
fn splitting_a_sound_with_f6_keeps_the_same_sound() {
    let mut doc = doc_with_flat_sound();
    put_clip(&mut doc.root, 30, flat_clip());
    let before: Vec<Vec<u64>> = (0..30)
        .map(|f| crate::sound::sounding_at(&doc, &doc.root, f).iter().map(|v| v.pos).collect())
        .collect();
    let mixed_before = crate::sound::mix(&doc, &doc.root, 0..30, 0, 48_000, 48_000);
    assert!(insert_keyframe_at(&mut doc.root, 0, 12, true, doc.fps));
    let kfs: Vec<_> = doc.root.layers[0].keyframes.iter().map(|k| k.sound).collect();
    assert_eq!(
        kfs,
        vec![
            Some(crate::sound::SoundClip { end: 24_000, ..flat_clip() }),
            Some(crate::sound::SoundClip { start: 24_000, ..flat_clip() }),
        ]
    );
    let after: Vec<Vec<u64>> = (0..30)
        .map(|f| crate::sound::sounding_at(&doc, &doc.root, f).iter().map(|v| v.pos).collect())
        .collect();
    assert_eq!(before, after);
    assert_eq!(crate::sound::mix(&doc, &doc.root, 0..30, 0, 48_000, 48_000), mixed_before);

    // 48,000 を割り切らない fps（7）でも、分けた所で音が途切れない（位置の丸めのずれは 1 まで）
    let mut doc = doc_with_flat_sound();
    doc.fps = 7;
    put_clip(&mut doc.root, 30, flat_clip());
    assert!(insert_keyframe_at(&mut doc.root, 0, 3, true, doc.fps));
    let m = crate::sound::mix(&doc, &doc.root, 0..30, 0, 48_000, 48_000);
    let split = 3 * 48_000 / 7;
    assert!(
        (split - 200..split + 200).all(|i| (m[2 * i] - 0.5).abs() < 1e-6),
        "分けた所の前後で小さくならない"
    );
}

/// 音のキーフレームの始まりをまたいでコマを消す（範囲の `Shift+F5`）と、消したコマのあとの音は、消す前と同じ所から
/// 続けて鳴る（絵より遅れない）
#[test]
fn removing_frames_across_a_sound_keyframe_keeps_the_sound_in_place() {
    let mut doc = doc_with_flat_sound();
    doc.root.layers[0].frame_count = 30;
    doc.root.total_frames = 30;
    assert!(insert_keyframe_at(&mut doc.root, 0, 10, false, doc.fps));
    doc.root.layers[0].keyframe_mut(1).sound = Some(flat_clip());
    let before = doc.clone();
    let pos = |doc: &symbol::Document, f: usize| -> Vec<u64> {
        crate::sound::sounding_at(doc, &doc.root, f).iter().map(|v| v.pos).collect()
    };
    assert!(remove_frames_at(&mut doc.root, 0, 5, 10, doc.fps));
    assert_eq!(doc.root.layers[0].frame_count, 20);
    assert_eq!(doc.root.layers[0].start_of(1), 5);
    for f in 5..20 {
        assert_eq!(pos(&doc, f), pos(&before, f + 10), "コマ {f}");
    }
}

/// シンボルの中の音は、中のコマで鳴る：ループは頭から鳴り直し、1回再生は最後のコマを過ぎたら止まり、コマ固定は鳴らない。
/// 2つ置けば、別々の音として2つ鳴る
#[test]
fn sounds_inside_symbols_follow_the_internal_frame() {
    use crate::symbol_binding::SymbolLoopMode;
    let build = |modes: &[SymbolLoopMode]| -> symbol::Document {
        let mut doc = doc_with_flat_sound();
        let sym_id = doc.register_new_symbol(Vec::new());
        {
            write_symbol(&mut doc, sym_id, |tl| put_clip(tl, 3, flat_clip()));
        }
        doc.root.layers[0].frame_count = 6;
        doc.root.total_frames = 6;
        for (k, &mode) in modes.iter().enumerate() {
            let mut b = crate::symbol_binding::SymbolInstanceBinding::new(
                100 + k as u64,
                sym_id,
                kurbo::Affine::IDENTITY,
            );
            b.loop_mode = mode;
            let lid = doc.root.layers[0].id;
            doc.root.layers[0].keyframe_mut(0).cel.instances_mut().push(
                timeline::SymbolInstance {
                    binding: b,
                    layer_id: lid,
                    parent_scope: Vec::new(),
                    order: 0,
                },
            );
        }
        doc
    };
    let pos = |doc: &symbol::Document, f: usize| -> Vec<u64> {
        crate::sound::sounding_at(doc, &doc.root, f).iter().map(|v| v.pos).collect()
    };
    let looped = build(&[SymbolLoopMode::Loop]);
    let got: Vec<Vec<u64>> = (0..6).map(|f| pos(&looped, f)).collect();
    assert_eq!(
        got,
        vec![vec![0], vec![2000], vec![4000], vec![0], vec![2000], vec![4000]]
    );
    let once = build(&[SymbolLoopMode::PlayOnce]);
    let got: Vec<Vec<u64>> = (0..6).map(|f| pos(&once, f)).collect();
    assert_eq!(got, vec![vec![0], vec![2000], vec![4000], vec![], vec![], vec![]]);
    let fixed = build(&[SymbolLoopMode::SingleFrame]);
    assert!((0..6).all(|f| pos(&fixed, f).is_empty()));
    let two = build(&[SymbolLoopMode::Loop, SymbolLoopMode::Loop]);
    let voices = crate::sound::sounding_at(&two, &two.root, 1);
    assert_eq!(voices.len(), 2);
    assert_ne!(voices[0].key, voices[1].key);
}

/// 混ぜた波形：音の無い文書は無音。鳴り始めはなめらかに大きくなり、コマの境目では途切れず、ループで頭に戻る所は
/// なめらかにつなぎ直す
#[test]
fn mixing_fades_only_where_the_sound_jumps() {
    let empty = symbol::Document::default();
    assert!(crate::sound::mix(&empty, &empty.root, 0..3, 0, 4800, 48_000).iter().all(|&x| x == 0.0));
    let mut doc = doc_with_flat_sound();
    put_clip(&mut doc.root, 30, flat_clip());
    let m = crate::sound::mix(&doc, &doc.root, 0..30, 0, 48_000, 48_000);
    let left = |i: usize| m[2 * i];
    assert_eq!(left(0), 0.0, "鳴り始めは 0 から");
    assert!(left(10) > 0.0 && left(10) < 0.5);
    assert!((left(1000) - 0.5).abs() < 1e-6);
    // コマの境目（2000 サンプルごと）でも途切れない
    assert!((left(2000) - 0.5).abs() < 1e-6 && (left(1999) - 0.5).abs() < 1e-6);
    // 区間の終わり（1 秒）の手前でなめらかに小さくなる
    assert!(left(47_999) < 0.05);

    // 3 コマのシンボルをループで置くと、3 コマごとに頭へ戻る所でつなぎ直す
    let mut doc = doc_with_flat_sound();
    let sym_id = doc.register_new_symbol(Vec::new());
    {
        write_symbol(&mut doc, sym_id, |tl| put_clip(tl, 3, flat_clip()));
    }
    doc.root.layers[0].frame_count = 6;
    doc.root.total_frames = 6;
    let b = crate::symbol_binding::SymbolInstanceBinding::new(7, sym_id, kurbo::Affine::IDENTITY);
    let lid = doc.root.layers[0].id;
    doc.root.layers[0].keyframe_mut(0).cel.instances_mut().push(timeline::SymbolInstance {
        binding: b,
        layer_id: lid,
        parent_scope: Vec::new(),
        order: 0,
    });
    let m = crate::sound::mix(&doc, &doc.root, 0..6, 0, 12_000, 48_000);
    let left = |i: usize| m[2 * i];
    assert!((left(5000) - 0.5).abs() < 1e-6, "3 コマの中は途切れない");
    assert!(left(5999) < 0.05 && left(6000) == 0.0, "頭へ戻る所でつなぎ直す");
    assert!((left(7000) - 0.5).abs() < 1e-6);
}

fn set_keyframe_sound(app: &mut Is2dApp, sound: Option<u64>) -> bool {
    doc_actions::apply(app, doc_actions::Action::SetKeyframeSound { sound })
}

/// 「サウンド」の欄で音を選ぶと、頭から終わりまで付く。付け替えると、始まり・終わりは新しい音の頭から終わりに戻る。
/// 「なし」で外すと空白のキーフレームに戻り、また絵を描ける。どれも取り消せる。付けられる所が無い・無い番号・
/// 変わらないなら断る
#[test]
fn choosing_a_keyframe_sound_attaches_replaces_and_removes_with_undo() {
    let mut app = Is2dApp::default();
    assert!(import_sound(&mut app, short_wav()));
    let long: Vec<i16> = vec![1000; 9600];
    assert!(import_sound(&mut app, wav_bytes(48_000, 1, &long)));
    let sounds = app.doc.sounds.clone();
    let whole = |i: usize| crate::sound::SoundClip::whole(sounds.at(i).unwrap());
    let kf_sound = |app: &Is2dApp| app.doc.root.layers[0].keyframes[0].sound;
    assert_eq!(kf_sound(&app), whole(1));

    // 区間を切ってから付け替えると、新しい音の頭から終わりに戻る
    app.doc.root.layers[0].keyframe_mut(0).sound.as_mut().unwrap().end = 100;
    assert!(set_keyframe_sound(&mut app, Some(sounds.at(0).unwrap().id)));
    assert_eq!(kf_sound(&app), whole(0));
    assert!(!set_keyframe_sound(&mut app, Some(sounds.at(0).unwrap().id)), "変わらない");
    assert!(!set_keyframe_sound(&mut app, Some(99)), "無い番号");

    assert!(set_keyframe_sound(&mut app, None));
    assert_eq!(kf_sound(&app), None);
    assert!(!set_keyframe_sound(&mut app, None), "変わらない");
    assert!(doc_actions::apply(&mut app, doc_actions::Action::Undo));
    assert_eq!(kf_sound(&app), whole(0));
    assert!(doc_actions::apply(&mut app, doc_actions::Action::Redo));
    assert_eq!(kf_sound(&app), None);
    // 外したら絵を描ける。絵のあるキーフレームには付けられない
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::PaintStroke {
            points: vec![Point::new(0.0, 0.0), Point::new(40.0, 10.0)],
            radius: 6.0,
            color: Color32::RED,
        }
    ));
    assert!(!set_keyframe_sound(&mut app, Some(sounds.at(0).unwrap().id)));
    assert_eq!(kf_sound(&app), None);
    // ロックしたレイヤーにも付けられない
    let mut app = Is2dApp::default();
    assert!(import_sound(&mut app, short_wav()));
    let id = app.doc.sounds.at(0).unwrap().id;
    assert!(set_keyframe_sound(&mut app, None));
    app.doc.root.layers[0].locked = true;
    assert!(!set_keyframe_sound(&mut app, Some(id)));
}

/// ライブラリで音を消すと、その音を付けたキーフレーム（ルートとシンボルの中）は空白に戻り、無い音を指す区間は残らない
/// （保存して開ける）。ほかの音を付けたキーフレームはそのまま。取り消すと音もキーフレームの音も戻る。名前の変更は、
/// 続けて打った分を1つの取り消しにまとめる
#[test]
fn deleting_and_renaming_library_sounds_with_undo() {
    let mut app = Is2dApp::default();
    assert!(import_sound(&mut app, short_wav()));
    let (a, b) = {
        let id = app.doc.sounds.at(0).unwrap().id;
        assert!(set_keyframe_sound(&mut app, None));
        assert!(import_sound(&mut app, short_wav()));
        (id, app.doc.sounds.at(1).unwrap().id)
    };
    // ルートのレイヤー 0 は音 b、シンボルの中は音 a。ルートに2枚目のレイヤーを足して音 a を付ける
    let clip_a = crate::sound::SoundClip::whole(app.doc.sounds.at(0).unwrap()).unwrap();
    let sym_id = app.doc.register_new_symbol(Vec::new());
    write_symbol(&mut app.doc, sym_id, |tl| put_clip(tl, 1, clip_a));
    assert!(doc_actions::apply(&mut app, doc_actions::Action::AddLayer));
    let top = app.doc.root.layers.len() - 1;
    app.doc.root.layers[top].keyframe_mut(0).sound = Some(clip_a);
    let snapshot = |app: &Is2dApp| {
        (
            app.doc.sounds.iter().map(|s| s.id).collect::<Vec<_>>(),
            app.doc.root.layers.iter().map(|l| l.keyframes[0].sound).collect::<Vec<_>>(),
            app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0].sound,
        )
    };
    let before = snapshot(&app);

    assert!(doc_actions::apply(&mut app, doc_actions::Action::DeleteSound { sound_id: a }));
    let (ids, root, inner) = snapshot(&app);
    assert_eq!(ids, vec![b]);
    assert_eq!(root[top], None);
    assert_eq!(root[0].map(|c| c.sound), Some(b), "ほかの音はそのまま");
    assert_eq!(inner, None);
    let saved = saved_of(&app.doc);
    assert!(saved.into_document().is_ok(), "無い音を指す区間は残らない");
    assert!(!doc_actions::apply(&mut app, doc_actions::Action::DeleteSound { sound_id: a }));

    assert!(doc_actions::apply(&mut app, doc_actions::Action::Undo));
    assert_eq!(snapshot(&app), before);
    assert!(doc_actions::apply(&mut app, doc_actions::Action::Redo));
    assert_eq!(snapshot(&app).0, vec![b]);

    // 名前の変更：続けて打った分は1つの取り消し
    let undo_len = app.history.undo_stack.len();
    for name in ["x", "xy", "xyz"] {
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::RenameSound {
                sound_id: b,
                new_name: name.to_owned(),
            }
        ));
    }
    assert!(!doc_actions::apply(
        &mut app,
        doc_actions::Action::RenameSound {
            sound_id: b,
            new_name: "xyz".to_owned(),
        }
    ));
    assert_eq!(app.doc.sounds.at(0).unwrap().name, "xyz");
    assert_eq!(app.history.undo_stack.len(), undo_len + 1);
    assert!(doc_actions::apply(&mut app, doc_actions::Action::Undo));
    assert_eq!(app.doc.sounds.at(0).unwrap().name, "テスト.wav");
}

/// 文書 `doc` のシンボル `sym_id` のタイムラインの音の区間が指す音のバイト（区間がライブラリの音に収まること）
fn symbol_sound_bytes(doc: &symbol::Document, sym_id: u64) -> Vec<Vec<u8>> {
    doc.find_symbol(sym_id)
        .unwrap()
        .timeline
        .sound_clips()
        .map(|c| {
            assert!(c.is_well_formed(&doc.sounds), "区間は音のライブラリの音を指す");
            let s = doc.sounds.iter().find(|s| s.id == c.sound).unwrap();
            s.encoded.to_vec()
        })
        .collect()
}

/// 中に音を付けたシンボルを別のタブへ貼ると、音も貼り先の音のライブラリに入る（貼り先に同じ番号の別の音があっても、
/// 新しい番号で入れて区間を付け替える）。保存して開け、取り消すと足した音も消える（レビュー e90f27c の N-4001）
#[test]
fn pasting_a_symbol_with_sound_into_another_tab_brings_the_sound() {
    use doc_actions::{apply, Action};
    let ctx = egui::Context::default();
    let (mut app, _) = copied_symbol_with_sound();
    let src_bytes = app.doc.sounds.at(0).unwrap().encoded.to_vec();
    project_io::new_project(&mut app, &ctx);
    let other = wav_bytes(48_000, 1, &vec![1000i16; 4800]);
    assert!(import_sound(&mut app, other.clone()));
    assert_eq!(app.doc.sounds.at(0).unwrap().id, 1, "コピー元の音と同じ番号");
    assert!(apply(&mut app, Action::AddLayer));

    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.sounds.len(), 2);
    let pasted = app.doc.library().at(0).unwrap().id;
    assert_eq!(
        symbol_sound_bytes(&app.doc, pasted),
        vec![src_bytes.clone()]
    );
    assert_eq!(*app.doc.sounds.at(0).unwrap().encoded, *other, "貼り先の音はそのまま");
    let doc = reopened(&app.doc).expect("保存したファイルを開ける");
    assert_eq!(symbol_sound_bytes(&doc, pasted), vec![src_bytes]);

    assert!(apply(&mut app, Action::Undo));
    assert_eq!(app.doc.sounds.len(), 1);
    assert!(app.doc.library().is_empty());
}

/// 別のタブから貼ったシンボルの音を貼り先で消してから、もう一度貼ると、消した後のシンボルを使い回さず、コピーしたときの
/// 中身（音入り）が新しいシンボルとして取り込まれる（音を消すのもシンボルの中身を書くことなので、版が上がる）
#[test]
fn pasting_again_after_deleting_the_pasted_symbols_sound_brings_the_copied_contents() {
    use doc_actions::{apply, Action};
    let ctx = egui::Context::default();
    let (mut app, _) = copied_symbol_with_sound();
    let src_bytes = app.doc.sounds.at(0).unwrap().encoded.to_vec();
    project_io::new_project(&mut app, &ctx);
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    let first = app.doc.library().at(0).unwrap().id;
    let sound_id = app.doc.sounds.at(0).unwrap().id;
    assert!(apply(&mut app, Action::DeleteSound { sound_id }));
    assert!(symbol_sound_bytes(&app.doc, first).is_empty());

    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library().len(), 2, "新しいシンボルとして取り込む");
    let second = app.doc.library().at(1).unwrap().id;
    assert_eq!(symbol_sound_bytes(&app.doc, second), vec![src_bytes]);
    assert!(symbol_sound_bytes(&app.doc, first).is_empty());
}

/// 同じタブでも、コピーのあと音とシンボルを消してから貼ると、シンボルと一緒に音もライブラリに戻る（新しい番号で）。
/// 消していなければ、今の音をそのまま使い、音を増やさない
#[test]
fn pasting_a_symbol_after_deleting_its_sound_restores_the_sound() {
    use doc_actions::{apply, Action};
    let (mut app, sym_id) = copied_symbol_with_sound();
    let src_bytes = app.doc.sounds.at(0).unwrap().encoded.to_vec();
    assert!(apply(&mut app, Action::Paste { in_place: false }));
    assert_eq!(app.doc.sounds.len(), 1);
    assert_eq!(app.doc.library().len(), 1);
    assert!(apply(&mut app, Action::Undo));

    let sound_id = app.doc.sounds.at(0).unwrap().id;
    assert!(apply(&mut app, Action::DeleteSound { sound_id }));
    assert!(apply(&mut app, Action::DeleteSymbol { sym_id }));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.sounds.len(), 1);
    assert_ne!(app.doc.sounds.at(0).unwrap().id, sound_id, "消した音の番号は使わない");
    assert_eq!(
        symbol_sound_bytes(&app.doc, sym_id),
        vec![src_bytes.clone()]
    );
    let doc = reopened(&app.doc).expect("保存したファイルを開ける");
    assert_eq!(symbol_sound_bytes(&doc, sym_id), vec![src_bytes]);

    assert!(apply(&mut app, Action::Undo));
    assert!(app.doc.sounds.is_empty());
    assert!(app.doc.library().is_empty());
}

/// コマ数に届く決まり `frames_to_reach` は `frames_to_sound` の逆（届くいちばん少ないコマ数）で、区間が鳴るかどうか
/// （`SoundClip::pos_at`）は「区間の始まり＋進んだコマの位置が終わりより前」と同じ
#[test]
fn frames_to_reach_is_the_inverse_of_frames_to_sound() {
    use crate::sound::{frames_to_reach, frames_to_sound, SoundClip};
    for fps in [1, 7, 24, 25, 30, 59, 60] {
        for units in (0..30_000u64).step_by(37).chain([47_999, 48_000, 48_001]) {
            let n = frames_to_reach(units, fps);
            assert!(frames_to_sound(n, fps) >= units, "fps {fps} units {units}");
            assert!(n == 0 || frames_to_sound(n - 1, fps) < units, "fps {fps} units {units}");
        }
        let clip = SoundClip {
            sound: 1,
            start: 1234,
            end: 20_000,
        };
        for d in 0..100 {
            let pos = clip.start + frames_to_sound(d, fps);
            assert_eq!(clip.pos_at(d, fps), (pos < clip.end).then_some(pos));
        }
    }
}

/// 波形の絵の値の幅：一定の値の音はどこでもその値。音の外は値が無い
#[test]
fn sound_peaks_follow_the_samples() {
    let doc = doc_with_flat_sound();
    let s = &doc.sounds.at(0).unwrap();
    let unit = crate::timeline_ui::WAVE_UNIT;
    assert_eq!(s.peak(0, 125, unit), Some((0.5, 0.5)));
    assert_eq!(s.peak(47_900, 48_000, unit), Some((0.5, 0.5)));
    assert_eq!(s.peak(48_000, 48_100, unit), None);
    // 細かさを変えると作り直す（同じ音の値）
    assert_eq!(s.peak(0, 125, unit * 2), Some((0.5, 0.5)));
    assert_eq!(s.peak(48_000, 48_100, unit * 2), None);
}

/// 波形の山の表を、区切りごとにまとめて作っても（`peak_table`）、時点ごとに区切りを求めて入れた表（お手本）と同じ。いろいろな
/// rate・細かさ・長さで
#[test]
fn peak_table_matches_per_sample_binning() {
    let mut rng = fastrand::Rng::with_seed(0x9ea6);
    for &(rate, unit) in &[(48_000u32, 50u64), (44_100, 50), (22_050, 50), (8_000, 7), (96_000, 13), (44_100, 1)] {
        for &frames in &[0usize, 1, 2, 3, 49, 50, 51, 1000, 4411] {
            let samples: Vec<i16> = (0..frames * 2).map(|_| rng.i16(..)).collect();
            let pcm = crate::sound::Pcm { rate, samples };
            let mut want: Vec<(i16, i16)> = Vec::new();
            for (i, lr) in pcm.samples.chunks_exact(2).enumerate() {
                let b = (i as u64 * crate::sound::SOUND_RATE / u64::from(rate) / unit) as usize;
                if b >= want.len() {
                    want.resize(b + 1, (i16::MAX, i16::MIN));
                }
                let p = &mut want[b];
                *p = (p.0.min(lr[0]).min(lr[1]), p.1.max(lr[0]).max(lr[1]));
            }
            assert_eq!(crate::sound::peak_table(&pcm, unit), want, "rate {rate}・細かさ {unit}・長さ {frames}");
        }
    }
}

/// テスト用：40 コマのレイヤーの、コマ 10 から 29 のキーフレームに音 1（1 秒）を頭から付け、コマ 0・30 は空白のキーフレーム
/// にした文書（fps `fps`）
fn doc_with_trim_layer(fps: u32) -> symbol::Document {
    let mut doc = doc_with_flat_sound();
    doc.fps = fps;
    doc.root.layers[0].frame_count = 40;
    doc.root.sync_total_frames();
    assert!(insert_keyframe_at(&mut doc.root, 0, 10, false, fps));
    assert!(insert_keyframe_at(&mut doc.root, 0, 30, false, fps));
    let layer = &mut doc.root.layers[0];
    let k = layer.keyframe_index_starting_at(10).unwrap();
    layer.keyframe_mut(k).sound = Some(flat_clip());
    doc.root.sync_total_frames();
    doc
}

fn positions(doc: &symbol::Document) -> Vec<Option<u64>> {
    (0..40)
        .map(|f| crate::sound::sounding_at(doc, &doc.root, f).first().map(|v| v.pos))
        .collect()
}

/// 波形の右端：終わりを、コマの境目に当たる位置にする。始まりより1コマ以上後ろから、音の本当の終わりとキーフレームの
/// 区間の終わりの早いほうまで。残ったコマで鳴る位置は変わらない
#[test]
fn trimming_the_sound_end_respects_bounds_and_keeps_timing() {
    use crate::sound::SoundEdge::End;
    let mut doc = doc_with_trim_layer(24);
    let range = |doc: &symbol::Document| {
        let layer = &doc.root.layers[0];
        let k = layer.keyframe_index_starting_at(10).unwrap();
        layer.sound_trim_range(k, End, &doc.sounds, doc.fps).unwrap()
    };
    assert_eq!(range(&doc), 11..=30, "1 秒は 24 コマ分あるが、区間はコマ 29 まで");
    let before = positions(&doc);
    assert!(!trim(&mut doc, 10, End, 10), "始まりと同じ所");
    assert!(!trim(&mut doc, 10, End, 31), "区間の外");
    assert!(trim(&mut doc, 10, End, 20));
    assert_eq!(doc.root.layers[0].keyframes[1].sound.unwrap().end, 20_000);
    let after = positions(&doc);
    assert_eq!(after[..20], before[..20]);
    assert!(after[20..].iter().all(Option::is_none));
    assert!(!trim(&mut doc, 10, End, 20), "変わらない");

    // 区間より短い音：音の本当の終わりに届くコマまで。届くコマの境目では、終わりは音の本当の終わり
    let mut doc = doc_with_trim_layer(24);
    doc.root.layers[0].keyframe_mut(1).sound.as_mut().unwrap().start = 24_100;
    assert_eq!(range(&doc), 11..=22, "残り 23,900 は 11.95 コマ");
    assert!(trim(&mut doc, 10, End, 15));
    assert!(trim(&mut doc, 10, End, 22));
    assert_eq!(doc.root.layers[0].keyframes[1].sound.unwrap().end, 48_000);
}

/// 波形の左端：キーフレームは動かさず、区間の始まりを、キーフレームからそのコマまでの時間だけ進める（戻す）。残りの音は
/// キーフレームのコマから鳴る。鳴り終わるコマの1つ手前まで削れ、削った頭は音の頭まで（キーフレームより前のコマへも）戻せる
#[test]
fn trimming_the_sound_start_keeps_the_keyframe_and_plays_the_rest_from_it() {
    use crate::sound::{frames_to_sound, SoundEdge::Start};
    for fps in [24, 7] {
        let mut doc = doc_with_trim_layer(fps);
        let before = positions(&doc);
        assert!(!trim(&mut doc, 10, Start, 9), "音の頭より前には戻せない");
        assert!(!trim(&mut doc, 10, Start, 30), "鳴り終わるコマの1つ手前まで");
        assert!(trim(&mut doc, 10, Start, 15));
        let layer = &doc.root.layers[0];
        let starts: Vec<usize> = (0..layer.keyframes.len()).map(|k| layer.start_of(k)).collect();
        assert_eq!(starts, vec![0, 10, 30], "キーフレームは動かない");
        let start = layer.keyframes[1].sound.unwrap().start;
        assert_eq!(start, frames_to_sound(5, fps));
        let after = positions(&doc);
        assert_eq!(after[..10], before[..10]);
        assert_eq!(after[10], Some(start), "残りの音がキーフレームのコマから鳴る（fps {fps}）");
        // 戻す：音の頭まで。キーフレームより前のコマへも
        assert!(!trim(&mut doc, 10, Start, 4));
        assert!(trim(&mut doc, 10, Start, 5));
        assert_eq!(doc.root.layers[0].keyframes[1].sound.unwrap().start, 0);
        assert_eq!(positions(&doc), before, "fps {fps}");
    }

    // コマ 0 のキーフレームも動かない
    let mut doc = doc_with_flat_sound();
    put_clip(&mut doc.root, 30, flat_clip());
    let keyframes = doc.root.layers[0].keyframes.len();
    assert!(doc.root.layers[0].trim_sound(0, Start, 3, &doc.sounds.clone(), 24));
    let layer = &doc.root.layers[0];
    assert_eq!(layer.keyframes.len(), keyframes);
    assert_eq!(layer.start_of(0), 0);
    assert_eq!(layer.keyframes[0].sound.unwrap().start, 6000);
}

/// 波形の端のドラッグを離したときの操作（`Action::TrimSound`）は1回の取り消しで戻る。ロックしたレイヤー・音の無い
/// キーフレーム・動かせない所は断る
#[test]
fn trim_sound_action_with_undo_and_refusals() {
    use crate::sound::SoundEdge::{End, Start};
    let trim_action = |app: &mut Is2dApp, kf_start, edge, frame| {
        doc_actions::apply(
            app,
            doc_actions::Action::TrimSound {
                layer: 0,
                kf_start,
                edge,
                frame,
            },
        )
    };
    let mut app = Is2dApp::default();
    app.doc.fps = 24;
    app.doc.root.layers[0].frame_count = 10;
    app.doc.root.sync_total_frames();
    assert!(import_sound(&mut app, short_wav()));
    let clip = app.doc.root.layers[0].keyframes[0].sound.unwrap();
    assert_eq!(clip.end, 4800, "0.1 秒は 2.4 コマ");
    assert!(trim_action(&mut app, 0, End, 2));
    assert_eq!(app.doc.root.layers[0].keyframes[0].sound.unwrap().end, 4000);
    assert!(doc_actions::apply(&mut app, doc_actions::Action::Undo));
    assert_eq!(app.doc.root.layers[0].keyframes[0].sound, Some(clip));

    assert!(!trim_action(&mut app, 0, End, 4), "音の本当の終わりを越える");
    assert!(!trim_action(&mut app, 5, End, 6), "そこに始まるキーフレームが無い");
    assert!(!trim_action(&mut app, 0, Start, 3), "鳴り終わるコマ（3 コマ目）の1つ手前まで");
    app.doc.root.layers[0].locked = true;
    assert!(!trim_action(&mut app, 0, End, 2), "ロックしたレイヤー");
}

/// 鳴らす範囲の端では、前後のコマに音が続いていても、鳴り始め・鳴り終わりをなめらかにする：音の途中のコマのスクラブ
/// （1コマ分）は 0 から鳴り始めて 0 近くで止み、途中のコマから始めた再生も 0 から鳴り始める
#[test]
fn playing_from_the_middle_of_a_sound_fades_at_the_range_ends() {
    use crate::audio_out::Job;
    use crate::sound::frame_start;
    let mut doc = doc_with_flat_sound();
    put_clip(&mut doc.root, 30, flat_clip());
    let rate = 48_000;
    let len = (frame_start(6, rate, 24) - frame_start(5, rate, 24)) as usize;
    let mut out = vec![1.0f32; 2 * len];
    assert_eq!(Job::new(&doc, &doc.root, 5, 6, false, rate).fill(0, rate, &mut out), len);
    assert_eq!(out[0], 0.0, "スクラブの頭は 0 から");
    assert!(out[2 * (len - 1)] < 0.05, "スクラブの終わりは止んでいる");
    assert!((out[len] - 0.5).abs() < 1e-6, "コマの中ほどは元の大きさ");
    let mut out = vec![1.0f32; 2 * len];
    assert_eq!(Job::new(&doc, &doc.root, 5, 30, false, rate).fill(0, rate, &mut out), len);
    assert_eq!(out[0], 0.0, "途中からの再生の頭は 0 から");
    assert!((out[2 * (len - 1)] - 0.5).abs() < 1e-6, "次のコマへ続く所は小さくしない");
}

/// 書き出しの音（`export_audio`）は、1コマ分ずつ（そのコマの最初のサンプルから次のコマの最初のサンプルの手前まで）
/// 返し、全部つなぐと、続けて混ぜた1つの `mix` と同じで、長さは `frame_start(全コマ数)`。fps が 48,000 を割り切らない
/// とき（1コマのサンプルの数がコマごとに違う）も同じ
#[test]
fn export_audio_per_frame_joins_into_one_mix() {
    use crate::sound::{frame_start, mix};
    use crate::video_export::{export_audio, EXPORT_AUDIO_RATE};
    for fps in [24, 7, 60] {
        let mut doc = doc_with_saw_sound();
        doc.fps = fps;
        let total = doc.root.total_frames;
        let pieces: Vec<Vec<f32>> = std::iter::from_fn(export_audio(&doc)).collect();
        assert_eq!(pieces.len(), total, "fps {fps}");
        for (f, p) in pieces.iter().enumerate() {
            let len = frame_start(f + 1, EXPORT_AUDIO_RATE, fps) - frame_start(f, EXPORT_AUDIO_RATE, fps);
            assert_eq!(p.len() as u64, 2 * len, "fps {fps} コマ {f}");
        }
        let joined = pieces.concat();
        let n = frame_start(total, EXPORT_AUDIO_RATE, fps);
        assert_eq!(joined.len() as u64, 2 * n, "fps {fps}");
        assert_eq!(joined, mix(&doc, &doc.root, 0..total, 0, n as usize, EXPORT_AUDIO_RATE));
        assert!(joined.iter().any(|&x| x != 0.0), "音が入っている");
    }
}

/// テスト用：MP4 のバイト `b` の `range` の中に並ぶ箱の、種類と中身の範囲
fn mp4_boxes(b: &[u8], range: std::ops::Range<usize>) -> Vec<([u8; 4], std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut i = range.start;
    while i + 8 <= range.end {
        // 大きさが 1 なら、種類の後の 64bit が本当の大きさ
        let (size, head) = match u32::from_be_bytes(b[i..i + 4].try_into().unwrap()) {
            1 => (u64::from_be_bytes(b[i + 8..i + 16].try_into().unwrap()) as usize, 16),
            n => (n as usize, 8),
        };
        assert!(size >= head && i + size <= range.end, "箱の大きさ {size}");
        out.push((b[i + 4..i + 8].try_into().unwrap(), i + head..i + size));
        i += size;
    }
    out
}

/// テスト用：MP4 の `range` の中に並ぶ箱のうち、種類 `kind` の1つ目の中身の範囲
fn mp4_child(b: &[u8], range: std::ops::Range<usize>, kind: &[u8; 4]) -> std::ops::Range<usize> {
    mp4_boxes(b, range)
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, r)| r)
        .unwrap_or_else(|| panic!("{} の箱がある", String::from_utf8_lossy(kind)))
}

/// テスト用：mvhd・mdhd（時間の単位と長さ）・tkhd（長さ。単位は mvhd の）の中身から `(単位, 長さ)`。tkhd の単位は 0
fn mp4_time(b: &[u8], r: &std::ops::Range<usize>, tkhd: bool) -> (u64, u64) {
    let be32 = |i: usize| u64::from(u32::from_be_bytes(b[i..i + 4].try_into().unwrap()));
    let be64 = |i: usize| u64::from_be_bytes(b[i..i + 8].try_into().unwrap());
    let s = r.start;
    match (b[s], tkhd) {
        (0, false) => (be32(s + 12), be32(s + 16)),
        (1, false) => (be32(s + 20), be64(s + 24)),
        (0, true) => (0, be32(s + 20)),
        (1, true) => (0, be64(s + 28)),
        (v, _) => panic!("知らない版 {v}"),
    }
}

/// 音のある作品の書き出しの MP4 には、絵の入力と、音の入力（AAC・48kHz・ステレオ）が1つずつあり、絵の長さはコマ数 ÷ fps
/// （止まらずに書き終える）。音の長さ（と、いちばん長い入力で決まる作品の長さ）は、コマ数 ÷ fps から、AAC の塊1つ分に
/// 満たない分まで長い（AAC は音を 1024 個ずつの塊で詰め、最後の塊の残りは無音で埋まる。Mac の書き込む係は編集リストで
/// 隠すが、Windows の Media Foundation は隠さない）。音のライブラリが空の作品は、今までどおり音の入力の無い MP4 になる
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn video_export_includes_the_mixed_audio_track() {
    use crate::video_export::{VideoExportJob, VideoExportOutcome};
    let tmp_dir = std::env::temp_dir().join(format!("is2d_audio_export_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // 3 秒（音は始めの 1 秒）。AAC の圧縮は音をためてから出すので、1 秒に満たない作品では、絵と音を書く順の誤りで
    // 書き込みが止まるのが見えない
    let mut doc = doc_with_saw_sound();
    doc.stage_size = Vec2::new(160.0, 90.0);
    doc.root.layers[0].frame_count = 72;
    doc.root.total_frames = 72;
    let frames = doc.root.total_frames;
    let export = |doc: &symbol::Document, name: &str| -> Vec<u8> {
        let path = tmp_dir.join(name);
        let mut job = VideoExportJob::start(doc.clone(), path.clone()).expect("スレッドを作れる");
        assert_eq!(job.wait(), VideoExportOutcome::Completed);
        std::fs::read(&path).unwrap()
    };
    // 作品の長さ（秒）・各入力の `(種類, 長さ（秒）)`・音の入力の形（音の種類・左右の数・速さ）
    type Summary = (f64, Vec<([u8; 4], f64)>, Option<([u8; 4], u16, u32)>);
    let inspect = |b: &[u8]| -> Summary {
        let moov = mp4_child(b, 0..b.len(), b"moov");
        let (scale, len) = mp4_time(b, &mp4_child(b, moov.clone(), b"mvhd"), false);
        let mut tracks = Vec::new();
        let mut audio = None;
        for (_, trak) in mp4_boxes(b, moov).into_iter().filter(|(k, _)| k == b"trak") {
            let (_, tk_len) = mp4_time(b, &mp4_child(b, trak.clone(), b"tkhd"), true);
            let mdia = mp4_child(b, trak, b"mdia");
            let hdlr = mp4_child(b, mdia.clone(), b"hdlr");
            let kind: [u8; 4] = b[hdlr.start + 8..hdlr.start + 12].try_into().unwrap();
            tracks.push((kind, tk_len as f64 / scale as f64));
            if &kind == b"soun" {
                let stbl = mp4_child(b, mp4_child(b, mdia, b"minf"), b"stbl");
                let stsd = mp4_child(b, stbl, b"stsd");
                // stsd：版と印（4）・数（4）の後に、音の形の箱（大きさ（4）・種類（4）・予約（6）・番号（2）・予約（8）・
                // 左右の数（2）・ビット（2）・予約（4）・速さ（16.16 の 4））
                let e = stsd.start + 8;
                let format: [u8; 4] = b[e + 4..e + 8].try_into().unwrap();
                let channels = u16::from_be_bytes(b[e + 24..e + 26].try_into().unwrap());
                let rate = u32::from_be_bytes(b[e + 32..e + 36].try_into().unwrap()) >> 16;
                audio = Some((format, channels, rate));
            }
        }
        (len as f64 / scale as f64, tracks, audio)
    };
    let expected = frames as f64 / f64::from(doc.fps);
    // AAC（AAC-LC）の1つの塊の音の数（決まりで 1024）。音の長さは、その分に満たないだけ長くなりうる
    const AAC_FRAME_SAMPLES: f64 = 1024.0;
    let aac_frame = AAC_FRAME_SAMPLES / f64::from(crate::video_export::EXPORT_AUDIO_RATE);
    let audio_len_ok = |t: f64| t > expected - 1e-3 && t < expected + aac_frame;

    let (len, tracks, audio) = inspect(&export(&doc, "with_sound.mp4"));
    assert_eq!(audio, Some((*b"mp4a", 2, 48_000)));
    let kinds: Vec<[u8; 4]> = tracks.iter().map(|t| t.0).collect();
    assert_eq!(kinds.iter().filter(|k| *k == b"vide").count(), 1);
    assert_eq!(kinds.iter().filter(|k| *k == b"soun").count(), 1);
    assert!(audio_len_ok(len), "作品の長さ {len} 秒");
    for (kind, t) in &tracks {
        let ok = match kind {
            b"soun" => audio_len_ok(*t),
            _ => (t - expected).abs() < 1e-3,
        };
        assert!(ok, "{} の長さ {t} 秒", String::from_utf8_lossy(kind));
    }

    doc.sounds = crate::sound::SoundLibrary::default();
    let (len, tracks, audio) = inspect(&export(&doc, "without_sound.mp4"));
    assert_eq!(audio, None);
    assert_eq!(tracks.len(), 1);
    assert_eq!(&tracks[0].0, b"vide");
    assert!((len - expected).abs() < 1e-3, "作品の長さ {len} 秒");

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
