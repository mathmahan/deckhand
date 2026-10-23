//! One game's saves and states as RomM holds them — what the Saves page shows, and what the detail
//! page counts. A port of the data half of the Kotlin app's `SavesState`.
//!
//! The page lists the server's records rather than what is on disk: RomM is where every device's
//! progress meets, so it is the one place that can show all of it. Each record says whether this
//! device holds that very version, from the sync's own indexes ([`crate::sync::on_device`]) — so
//! "on this device" means the bytes, not a file of the same name.
//!
//! A game's saves can sit on more than its tile's rom: a multi-disc set's states land on whichever
//! disc's rom was running, and a sibling build has saves of its own. So they are gathered across the
//! tile's rom and every sibling the library knows.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::app::Direction;
use crate::config::Connection;
use crate::library::Stamp;
use crate::romm::RommClient;
use crate::romm::models::SaveAsset;
use crate::sync::{parse_timestamp, states};

/// The page's two tabs. States and in-game saves are different things — a snapshot an emulator
/// took, with a picture of the moment, against the game's own file — so they are not interleaved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    States,
    Saves,
}

/// A game's saves and states, newest first.
#[derive(Debug, Clone, Default)]
pub struct GameSaves {
    pub states: Vec<SaveAsset>,
    pub saves: Vec<SaveAsset>,
    /// The ids of the ones this device holds that version of.
    pub on_device: HashSet<i64>,
    /// The other devices holding each save's current version, by name, as RomM records them.
    pub elsewhere: HashMap<i64, Vec<String>>,
    /// The RetroArch slot each state is this device's, by id — see [`states::layout`].
    pub slots: HashMap<i64, states::Slot>,
    /// The core the game runs under here, by RetroArch's name for it, when that is known.
    pub core: Option<String>,
    /// The RetroArch states another core made, which this device's cannot load — see
    /// [`states::Core::loads`].
    pub other_core: HashSet<i64>,
}

impl GameSaves {
    pub fn list(&self, tab: Tab) -> &[SaveAsset] {
        match tab {
            Tab::States => &self.states,
            Tab::Saves => &self.saves,
        }
    }
}

/// Fetch them: the records from RomM, the "on this device" ids from the indexes in `store` — and from
/// RomM's own record of which devices hold which save, where it keeps one. `roms` are the game's,
/// each with its stamp, which an index entry has to follow to count ([`crate::sync::on_device`]).
/// `core` is the one the game runs under here, which decides the states that land in a slot.
pub async fn load(
    client: RommClient,
    connection: Connection,
    roms: Vec<(i64, Option<Stamp>)>,
    store: PathBuf,
    core: states::Core,
) -> Result<GameSaves, String> {
    let rom_ids: Vec<i64> = roms.iter().map(|(rom_id, _)| *rom_id).collect();
    let (mut saves, mut states) = client.fetch_game_saves(&connection, &rom_ids).await.map_err(|e| e.to_string())?;
    let newest_first = |list: &mut Vec<SaveAsset>| {
        list.sort_by_key(|s| std::cmp::Reverse((s.updated_at.as_deref().and_then(parse_timestamp), s.id)));
        list.dedup_by_key(|s| s.id);
    };
    newest_first(&mut saves);
    newest_first(&mut states);
    let mut on_device = tokio::task::spawn_blocking(move || crate::sync::on_device(&store, &roms)).await.unwrap_or_default();
    let mut elsewhere = HashMap::new();
    for save in &saves {
        for sync in save.device_syncs.iter().filter(|s| s.is_current && !s.is_untracked) {
            if connection.device_id.as_deref() == Some(sync.device_id.as_str()) {
                on_device.insert(save.id);
            } else {
                let name = sync.device_name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| "another device".into());
                elsewhere.entry(save.id).or_insert_with(Vec::new).push(name);
            }
        }
    }
    let slots = states::layout(&states, &core);
    let other_core = states.iter().filter(|s| is_retroarch_state(s) && !core.loads(s)).map(|s| s.id).collect();
    Ok(GameSaves { states, saves, on_device, elsewhere, slots, core: core.name().map(str::to_string), other_core })
}

/// A RetroArch state, as against another emulator's (a `.p2s`, a `.ppst`).
fn is_retroarch_state(state: &SaveAsset) -> bool {
    state.file_name.to_lowercase().ends_with(".state")
}

/// The page's cursor: a tab, and a row per tab, remembered while switching between them.
#[derive(Debug, Clone, Default)]
pub struct SavesFocus {
    pub tab: Tab,
    rows: [usize; 2],
}

impl SavesFocus {
    /// Open on a tab that has something in it, so a game with only battery saves does not land on an
    /// empty States tab and look broken.
    pub fn opening(saves: Option<&GameSaves>) -> Self {
        let tab = match saves {
            Some(s) if s.states.is_empty() && !s.saves.is_empty() => Tab::Saves,
            _ => Tab::States,
        };
        SavesFocus { tab, rows: [0, 0] }
    }

    /// The row the cursor is on, among `rows` many.
    pub fn row(&self, rows: usize) -> usize {
        self.rows[self.tab as usize].min(rows.saturating_sub(1))
    }

    /// Up and Down walk the list; Left and Right switch tab. Answers whether anything moved.
    pub fn step(&mut self, direction: Direction, rows: usize) -> bool {
        let before = (self.tab, self.row(rows));
        match direction {
            Direction::Up => self.rows[self.tab as usize] = self.row(rows).saturating_sub(1),
            Direction::Down => self.rows[self.tab as usize] = (self.row(rows) + 1).min(rows.saturating_sub(1)),
            Direction::Left => self.tab = Tab::States,
            Direction::Right => self.tab = Tab::Saves,
        }
        before != (self.tab, self.row(rows))
    }

    pub fn show(&mut self, tab: Tab) {
        self.tab = tab;
    }

    /// Put the cursor on a row of the current tab — where a click landed.
    pub fn focus(&mut self, row: usize) {
        self.rows[self.tab as usize] = row;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(id: i64, name: &str) -> SaveAsset {
        serde_json::from_value(serde_json::json!({ "id": id, "file_name": name })).unwrap()
    }

    #[test]
    fn the_cursor_keeps_its_row_per_tab_and_opens_where_there_is_something() {
        let only_saves = GameSaves { saves: vec![state(1, "a.srm")], ..Default::default() };
        assert_eq!(SavesFocus::opening(Some(&only_saves)).tab, Tab::Saves);
        assert_eq!(SavesFocus::opening(None).tab, Tab::States);

        let mut f = SavesFocus::default();
        assert!(f.step(Direction::Down, 3));
        assert!(f.step(Direction::Down, 3));
        assert!(!f.step(Direction::Down, 3), "clamped at the end");
        assert!(f.step(Direction::Right, 1));
        assert_eq!((f.tab, f.row(1)), (Tab::Saves, 0));
        f.step(Direction::Left, 3);
        assert_eq!(f.row(3), 2, "the States row was kept");
        assert_eq!(f.row(1), 0, "and clamped into a shorter list");
    }
}
