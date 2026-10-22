//! Where a running sync is: how many of the saves it took on it has been through, and whose it is
//! on — what the save sync page says while one runs.
//!
//! Held in one place rather than sent back as the sync goes: the window runs one sync of every save
//! at a time, and reading this is all the page needs, so the sync's callers do not change to carry
//! it. A sync after a game in the same window moves it too, which says only what is true.

use std::sync::{Mutex, MutexGuard};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Saves gone through.
    pub done: usize,
    /// Saves taken on.
    pub of: usize,
    /// The game the save it is on is of.
    pub rom_id: Option<i64>,
}

static NOW: Mutex<Option<Progress>> = Mutex::new(None);

fn now_mut() -> MutexGuard<'static, Option<Progress>> {
    NOW.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A sync taking on `of` saves.
pub fn begin(of: usize) {
    *now_mut() = Some(Progress { done: 0, of, rom_id: None });
}

/// On a save of `rom_id`'s game.
pub fn at(rom_id: i64) {
    if let Some(progress) = now_mut().as_mut() {
        progress.rom_id = Some(rom_id);
    }
}

/// One more save gone through. More than were taken on is one found on the way, and counts in both.
pub fn step() {
    if let Some(progress) = now_mut().as_mut() {
        progress.done += 1;
        progress.of = progress.of.max(progress.done);
    }
}

/// Where the sync running is; None when none is, or one has not got as far as saying.
pub fn now() -> Option<Progress> {
    *now_mut()
}

/// The sync is over.
pub fn end() {
    *now_mut() = None;
}
