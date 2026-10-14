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
    /// The RetroArch slot each state is laid into, by id — see [`slots`].
    pub slots: HashMap<i64, u32>,
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
/// RomM's own record of which devices hold which save, where it keeps one. `core` is the one the game
/// runs under here, which decides the states that land in a slot.
pub async fn load(
    client: RommClient,
    connection: Connection,
    rom_ids: Vec<i64>,
    store: PathBuf,
    core: states::Core,
) -> Result<GameSaves, String> {
    let (mut saves, mut states) = client.fetch_game_saves(&connection, &rom_ids).await.map_err(|e| e.to_string())?;
    let newest_first = |list: &mut Vec<SaveAsset>| {
        list.sort_by_key(|s| std::cmp::Reverse((s.updated_at.as_deref().and_then(parse_timestamp), s.id)));
        list.dedup_by_key(|s| s.id);
    };
    newest_first(&mut saves);
    newest_first(&mut states);
    let mut on_device = tokio::task::spawn_blocking(move || crate::sync::on_device(&store)).await.unwrap_or_default();
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
    let slots = slots(&states, &core);
    let other_core = states.iter().filter(|s| is_retroarch_state(s) && !core.loads(s)).map(|s| s.id).collect();
    Ok(GameSaves { states, saves, on_device, elsewhere, slots, core: core.name().map(str::to_string), other_core })
}

/// A RetroArch state, as against another emulator's (a `.p2s`, a `.ppst`).
fn is_retroarch_state(state: &SaveAsset) -> bool {
    state.file_name.to_lowercase().ends_with(".state")
}

/// Where each RetroArch state lands among RetroArch's slots — the layout the sync writes, so the
/// page can say which slot to load: the player's own `.slotN` states in their slot, the newest of
/// the rest in slot 0, and the history after it from [`states::FIRST_HISTORY`] up. `states` must be
/// newest first. Another emulator's states (a `.p2s`, a `.ppst`) have no RetroArch slot, and nor do
/// another core's, which `core` cannot load.
pub fn slots(states: &[SaveAsset], core: &states::Core) -> HashMap<i64, u32> {
    let mut out = HashMap::new();
    let mut claimed = HashSet::new();
    let mut rank = 0u32;
    for state in states.iter().filter(|s| is_retroarch_state(s) && core.loads(s)) {
        match states::slot_of(&state.file_name) {
            // A slot saved twice: the newer owns it, the older is not anywhere on disk.
            Some(slot) => {
                if claimed.insert(slot) {
                    out.insert(state.id, slot);
                }
            }
            None => {
                let slot = if rank == 0 { 0 } else { states::FIRST_HISTORY + rank - 1 };
                if rank < states::HISTORY as u32 {
                    out.insert(state.id, slot);
                }
                rank += 1;
            }
        }
    }
    out
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
    fn states_land_in_the_slots_the_sync_lays_them_into() {
        // Newest first, as the page holds them.
        let states = vec![
            state(9, "Game.slot2.deckhand.state"),
            state(8, "Game [2026-07-03 10-00-00-000].deckhand.state"),
            state(7, "Game.slot2.deckhand.state"),
            state(6, "Game [2026-07-02 10-00-00-000].deckhand.state"),
            state(5, "Game.state"),
            state(4, "SLUS-20946 (2C6BE434).01.p2s"),
        ];
        let slots = slots(&states, &states::Core::default());
        assert_eq!(slots.get(&9), Some(&2), "the player's slot");
        assert_eq!(slots.get(&7), None, "superseded by a newer save of the same slot");
        assert_eq!(slots.get(&8), Some(&0), "the newest of the rest is slot 0");
        assert_eq!(slots.get(&6), Some(&states::FIRST_HISTORY));
        assert_eq!(slots.get(&5), Some(&(states::FIRST_HISTORY + 1)));
        assert_eq!(slots.get(&4), None, "not RetroArch's");
    }

    #[test]
    fn another_cores_states_land_in_no_slot() {
        let by = |id: i64, name: &str, core: &str| -> SaveAsset {
            serde_json::from_value(serde_json::json!({ "id": id, "file_name": name, "emulator": core })).unwrap()
        };
        let states = vec![
            by(9, "Game [2026-07-03 10-00-00-000].deckhand.state", "pcsx_rearmed"),
            by(8, "Game.slot2.deckhand.state", "pcsx_rearmed"),
            by(7, "Game [2026-07-02 10-00-00-000].deckhand.state", "swanstation"),
        ];
        let dir = std::env::temp_dir().join(format!("deckhand-game-saves-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("retroarch.cfg"), "").unwrap();
        let install = crate::sync::retroarch::read(&dir.join("retroarch.cfg")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let slots = slots(&states, &states::Core::of(Some("swanstation"), &install));
        assert_eq!(slots.get(&7), Some(&0), "this core's newest is slot 0");
        assert_eq!((slots.get(&9), slots.get(&8)), (None, None), "{slots:?}");
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
