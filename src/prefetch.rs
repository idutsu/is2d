//! 先読み：再生・スクラブ・コマ送りで次に見るセル画の、地図の計算に使う写し（面と線の図形の表）を、
//! 見る前に裏のスレッドで作って持っておく（設計：claude/先読みの設計_31d78a6.md）。
//!
//! 写しは、しまった地図が「今誰かが持っている写し」を覚えているので（`LayerBase::of_stored`）、裏の係が持っていれば、
//! 見る瞬間に画面のスレッドが求めたときに、作り終えたその写しが返る。作っている途中なら、写しの中の表は鍵を掛けて
//! 作るので、画面のスレッドは作り終わるのを待って同じものを使う。描き変えると地図が新しくなるので、古い写しが返ること
//! は無い。係が止まっても、写しが来なくなるだけ（画面のスレッドが見る瞬間に作る）

use std::sync::mpsc;
use std::sync::Arc;

use crate::planar_map::LayerBase;
use crate::timeline::{Layer, ScopeMap, Timeline};

/// 先読みの係への窓口（アプリに1つ）。係のスレッドは、初めて一覧を渡すときに作り、窓口が無くなると終わる
#[derive(Default)]
pub(crate) struct Prefetch {
    /// 直近に渡した一覧（同じなら渡さない）
    sent: Vec<ScopeMap>,
    /// 係への通り道（係を作れなかった・係が止まったら `None` のまま、作り直さない）
    tx: Option<mpsc::Sender<Vec<ScopeMap>>>,
    /// 係を作ろうとしたか
    started: bool,
}

impl Prefetch {
    /// 先読みして持つ地図を `maps` にする（前と同じなら何もしない）。一覧から外れた地図の写しは、係が捨てる
    pub(crate) fn want(&mut self, maps: Vec<ScopeMap>) {
        if maps.len() == self.sent.len() && maps.iter().zip(&self.sent).all(|(a, b)| a.same(b)) {
            return;
        }
        if !self.started {
            self.started = true;
            let (tx, rx) = mpsc::channel();
            self.tx = std::thread::Builder::new()
                .name("is2d-prefetch".to_owned())
                .spawn(move || work(&rx))
                .ok()
                .map(|_| tx);
        }
        if let Some(tx) = &self.tx {
            if tx.send(maps.clone()).is_err() {
                self.tx = None;
            }
        }
        self.sent = maps;
    }
}

/// 係の仕事：渡された一覧のうち、まだ作っていない地図の写しを順に作って持つ。新しい一覧が来たら、それに替える
fn work(rx: &mpsc::Receiver<Vec<ScopeMap>>) {
    let mut want: Vec<ScopeMap> = Vec::new();
    let mut held: Vec<(ScopeMap, Arc<LayerBase>)> = Vec::new();
    loop {
        let todo = want.iter().position(|m| !held.iter().any(|(h, _)| h.same(m)));
        let next = match todo {
            None => rx.recv().ok(),
            Some(_) => match rx.try_recv() {
                Ok(list) => Some(list),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => return,
            },
        };
        match (next, todo) {
            (Some(mut list), _) => {
                while let Ok(newer) = rx.try_recv() {
                    list = newer;
                }
                held.retain(|(h, _)| list.iter().any(|m| m.same(h)));
                want = list;
            }
            (None, Some(i)) => {
                let base = prefetched(&want[i]);
                held.push((want[i].clone(), base));
            }
            (None, None) => return,
        }
    }
}

/// 地図 `map` の計算に使う写しを手に入れ、描く図形（面と線）の表を作っておく（見る瞬間に作るのと同じ関数：
/// `ScopeMap::base`・`LayerBase::drawn_shapes`。表は写しの中に残る）。図形のメッシュは作らない（写しの図形はメッシュを
/// 持たない決まり：メッシュを持つのは、今のコマの土台・上乗せ・表示中のオニオンスキンだけ）
pub(crate) fn prefetched(map: &ScopeMap) -> Arc<LayerBase> {
    let base = map.base();
    base.drawn_shapes(&map.group_path);
    base
}

/// タイムライン `tl` の、表示しているレイヤーごとに、今描いているコマの範囲 `first..=last` のすぐ外の、次に見るセル画の
/// 地図（進む向き。`both` なら戻る向きも。`wrap` なら、端の先はもう一方の端から）
pub(crate) fn wanted_maps(tl: &Timeline, first: usize, last: usize, both: bool, wrap: bool) -> Vec<ScopeMap> {
    let total = tl.total_frames;
    let mut out = Vec::new();
    for layer in tl.layers.iter().filter(|l| l.visible) {
        let ahead = next_cel(layer, last).or_else(|| wrap.then(|| layer.keyframe_index_at(0)).flatten());
        let behind = if both {
            prev_cel(layer, first)
                .or_else(|| wrap.then(|| layer.keyframe_index_at(total.saturating_sub(1))).flatten())
        } else {
            None
        };
        for kf in ahead.into_iter().chain(behind) {
            out.extend(layer.keyframes[kf].cel.maps().iter().cloned());
        }
    }
    out
}

/// レイヤー `layer` の、コマ `frame` のあとで初めて替わるセル画（キーフレームの番号）
fn next_cel(layer: &Layer, frame: usize) -> Option<usize> {
    let next = match layer.keyframe_index_at(frame) {
        Some(k) => layer.span_end_for_keyframe(k) + 1,
        None if frame < layer.frame_count && !layer.keyframes.is_empty() => layer.start_of(0),
        None => return None,
    };
    layer.keyframe_index_at(next)
}

/// レイヤー `layer` の、コマ `frame` の前で初めて替わるセル画（キーフレームの番号）
fn prev_cel(layer: &Layer, frame: usize) -> Option<usize> {
    let prev = match layer.keyframe_index_at(frame) {
        Some(k) => layer.start_of(k).checked_sub(1),
        None => frame.min(layer.frame_count).checked_sub(1),
    };
    prev.and_then(|f| layer.keyframe_index_at(f))
}
