//! アプリ全体の通し番号（Layer 1：何も参照しない部品。図形の番号・絵の版の番号・キーフレームの番号。取り消しでも戻さない
//! アプリ全体の番号なので、別の中身が同じ番号になることは無い：設計審査 01e4283 の「守るべき良い所」）。配る決まりは型
//! `Serial` の1つ（前は形の計算の部屋とタイムラインの部屋に同じ作りが2つあった。置き場所：claude/層の分け方の直しの設計.md の 2.）

use std::sync::atomic::{AtomicU64, Ordering};

/// 通し番号の数え札（スレッドの間で共有する）
pub(crate) struct Serial(AtomicU64);

impl Serial {
    /// 次に配る番号が `next` の数え札
    pub(crate) const fn starting_at(next: u64) -> Self {
        Self(AtomicU64::new(next))
    }

    /// 番号を1つ配る
    pub(crate) fn take(&self) -> u64 {
        self.take_many(1)
    }

    /// 続けて使える番号を `n` 個配り、その最初の番号を返す（使い切ったら計算を止める：`advanced`）
    pub(crate) fn take_many(&self, n: u64) -> u64 {
        match self.0.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| Some(advanced(next, n))) {
            Ok(first) | Err(first) => first,
        }
    }
}

/// 番号 `id` から `n` 進めた番号（番号を配る決まりはこれ1つ：通し番号 `Serial` と作品の数え札 `symbol::Counter`）。数の上限を
/// 越えるなら計算を止める：止まった計算は入口の砦が受け止めて、その仕事を無かったことにするので、同じ番号を2回配らない
/// （2^64 個で、1秒に10億個配っても約580年。持ち主の了解：2026-11-27）
pub(crate) fn advanced(id: u64, n: u64) -> u64 {
    id.checked_add(n).expect("番号を使い切った")
}

static SHAPE_IDS: Serial = Serial::starting_at(1);
static MESH_REVISIONS: Serial = Serial::starting_at(1);
static KEYFRAME_IDS: Serial = Serial::starting_at(1);

/// 新しい図形の番号
pub(crate) fn next_shape_id() -> u64 {
    SHAPE_IDS.take()
}

/// 続けて使える新しい図形の番号を `n` 個取り、その最初の番号を返す（図形の番号を振るのはこれと `next_shape_id` だけ。
/// ファイルから読んだ地図の印も、これで取った番号に替える：`TileReader::new`）
pub(crate) fn new_shape_ids(n: u64) -> u64 {
    SHAPE_IDS.take_many(n)
}

/// 新しい絵の版の番号（セル画の版・シンボルの版・ライブラリの世代）
pub(crate) fn next_mesh_revision() -> u64 {
    MESH_REVISIONS.take()
}

/// 新しいキーフレームの番号
pub(crate) fn next_keyframe_id() -> u64 {
    KEYFRAME_IDS.take()
}
