//! スレッドの間で共有する写しの箱（Layer 1：何も参照しない部品。地図・しまう地図・シンボル・画面の写しが使う。置き場所：
//! claude/層の分け方の直しの設計.md の 3.）

use std::sync::{Mutex, MutexGuard};

/// `Arc` と共存可能なスレッドセーフ（`Send + Sync`）かつ `Clone` 対応の内部可変キャッシュラッパー
#[derive(Debug, Default)]
pub(crate) struct SyncCache<T>(Mutex<T>);

impl<T> SyncCache<T> {
    #[inline]
    pub(crate) const fn new(val: T) -> Self {
        Self(Mutex::new(val))
    }

    #[inline]
    pub(crate) fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<T: Clone> Clone for SyncCache<T> {
    fn clone(&self) -> Self {
        Self::new(self.lock().clone())
    }
}
