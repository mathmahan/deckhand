//! RomM's collections, as the Collections tab browses them — a port of the Kotlin app's
//! `BrowseCollection` and `CollectionCards`.
//!
//! Three sources, one list. The player's own collections (`/api/collections`), their saved-filter
//! "smart" ones (`/api/collections/smart`), and the ones RomM generates from its metadata
//! (`/api/collections/virtual`): genres, franchises, series, companies and game modes. Each is a set
//! of rom ids, which is all browsing one needs — its games are the library's tiles those ids belong
//! to, siblings included, since a collection names whichever build was matched and a tile stands for
//! all of its builds.
//!
//! Favourites leads, from the favourites the app holds rather than RomM's copy of that collection,
//! so a heart pressed here shows on the page at once. The page lists its games themselves rather
//! than one card for it: a card that only opens onto a grid is a press spent getting to them.

use std::collections::{HashMap, HashSet};

use crate::app::Direction;
use crate::config::Connection;
use crate::focus::{CARD_COLS, CardFocus};
use crate::library::{Game, Library};
use crate::romm::RommClient;
use crate::romm::models::{Collection, VirtualCollection};

/// What a [`crate::ui::browse`] grid is showing: one system, or one collection, by its key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    System(String),
    Collection(String),
}

/// A collection's kind — one chip on the Collections page, in this order. The player's own come
/// first: they are the ones made by hand, favourites the one made most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// The player's favourites — one collection, [`FAVOURITES`], whose games the page lists.
    Favourites,
    Yours,
    Smart,
    Genre,
    Franchise,
    Series,
    Company,
    Mode,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Favourites => "favourites",
            Kind::Yours => "yours",
            Kind::Smart => "smart",
            Kind::Genre => "genres",
            Kind::Franchise => "franchises",
            Kind::Series => "series",
            Kind::Company => "companies",
            Kind::Mode => "modes",
        }
    }

    /// RomM's virtual collection `type`, or None for one this page does not list.
    fn of_virtual(kind: &str) -> Option<Kind> {
        Some(match kind.to_lowercase().as_str() {
            "genre" => Kind::Genre,
            "franchise" => Kind::Franchise,
            // RomM calls IGDB's series "collection", which would read as a second "yours" here.
            "collection" => Kind::Series,
            "company" => Kind::Company,
            "mode" => Kind::Mode,
            _ => return None,
        })
    }
}

/// The key of the favourites collection.
pub const FAVOURITES: &str = "favourites";

/// One collection, reduced to what browsing it needs.
#[derive(Debug, Clone, PartialEq)]
pub struct GameCollection {
    /// Unique across the three sources: manual and smart ids are separate sequences, and a virtual
    /// id is a string.
    pub key: String,
    pub name: String,
    pub kind: Kind,
    /// The library tiles it holds, by tile rom id, in library order. A collection with none of them
    /// is not listed — RomM builds its lists from the whole server's metadata, and one that would
    /// open onto an empty grid is a dead end.
    pub members: Vec<i64>,
    /// The cover it is shown with: its best-rated game that has one.
    pub cover_url: Option<String>,
}

/// Every collection, by kind, each kind alphabetical.
#[derive(Debug, Clone, Default)]
pub struct Collections {
    all: Vec<GameCollection>,
    by_key: HashMap<String, usize>,
}

impl Collections {
    /// Build from the three sources and the player's favourites (rom ids, any build) against
    /// `library`.
    pub fn build(
        library: &Library,
        favourites: &HashSet<i64>,
        manual: &[Collection],
        smart: &[Collection],
        virtual_: &[VirtualCollection],
    ) -> Self {
        // Every rom id a tile stands for — its own, its siblings', every build's — to the tile.
        let mut tile_of: HashMap<i64, i64> = HashMap::new();
        let mut order: HashMap<i64, usize> = HashMap::new();
        let mut tiles: HashMap<i64, &Game> = HashMap::new();
        for (i, game) in library.all_games().iter().enumerate() {
            order.insert(game.rom_id, i);
            tiles.insert(game.rom_id, game);
            let ids = std::iter::once(game.rom_id)
                .chain(game.sibling_ids.iter().copied())
                .chain(game.file_stems.iter().map(|(id, _)| *id));
            for id in ids {
                tile_of.entry(id).or_insert(game.rom_id);
            }
        }
        let members = |rom_ids: &[i64]| -> Vec<i64> {
            let mut tiles: Vec<i64> = rom_ids.iter().filter_map(|id| tile_of.get(id).copied()).collect::<HashSet<_>>().into_iter().collect();
            tiles.sort_by_key(|id| order.get(id).copied().unwrap_or(usize::MAX));
            tiles
        };

        let collection = |key: String, name: String, kind: Kind, rom_ids: &[i64]| {
            let members = members(rom_ids);
            let games: Vec<&Game> = members.iter().filter_map(|id| tiles.get(id).copied()).collect();
            let cover_url = cover_game(&games).and_then(|g| g.cover_url.clone());
            GameCollection { key, name, kind, members, cover_url }
        };
        let favourites: Vec<i64> = favourites.iter().copied().collect();
        let mut all: Vec<GameCollection> = std::iter::once(collection(FAVOURITES.into(), "Favourites".into(), Kind::Favourites, &favourites))
            .chain(manual.iter().filter(|c| !c.is_favorite).map(|c| collection(format!("manual:{}", c.id), c.display_name(), Kind::Yours, &c.rom_ids)))
            .chain(smart.iter().map(|c| collection(format!("smart:{}", c.id), c.display_name(), Kind::Smart, &c.rom_ids)))
            .chain(virtual_.iter().filter_map(|v| {
                let name = v.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| v.id.clone());
                Some(collection(format!("virtual:{}", v.id), name, Kind::of_virtual(&v.kind)?, &v.rom_ids))
            }))
            .filter(|c| !c.members.is_empty())
            .collect();
        all.sort_by_key(|c| (c.kind, c.name.to_lowercase()));
        let by_key = all.iter().enumerate().map(|(i, c)| (c.key.clone(), i)).collect();
        Collections { all, by_key }
    }

    pub fn get(&self, key: &str) -> Option<&GameCollection> {
        self.by_key.get(key).map(|i| &self.all[*i])
    }

    /// The kinds that have at least one collection, in chip order.
    pub fn kinds(&self) -> Vec<Kind> {
        let mut kinds: Vec<Kind> = self.all.iter().map(|c| c.kind).collect();
        kinds.dedup();
        kinds
    }

    /// The favourite games' tiles, in library order.
    pub fn favourites(&self) -> &[i64] {
        self.get(FAVOURITES).map_or(&[], |c| c.members.as_slice())
    }

    /// One kind's collections, alphabetical.
    pub fn of_kind(&self, kind: Kind) -> impl Iterator<Item = &GameCollection> {
        self.all.iter().filter(move |c| c.kind == kind)
    }

    /// A collection's games out of `library`, in library order.
    pub fn games<'a>(&self, key: &str, library: &'a Library) -> Vec<&'a Game> {
        let Some(collection) = self.get(key) else { return Vec::new() };
        let mut seen = HashSet::new();
        collection.members.iter().filter(|id| seen.insert(**id)).filter_map(|id| library.game(*id)).collect()
    }
}

/// The card a collection is shown with: its best-rated game that has a cover.
pub fn cover_game<'a>(games: &[&'a Game]) -> Option<&'a Game> {
    games
        .iter()
        .copied()
        .filter(|g| g.cover_url.is_some())
        .max_by(|a, b| a.rating.unwrap_or(0.0).total_cmp(&b.rating.unwrap_or(0.0)))
}

/// RomM's smart collections and the ones it generates, as fetched.
pub type Sources = (Vec<Collection>, Vec<VirtualCollection>);

/// The smart and generated collections, fetched; the player's own already arrive with the library.
pub async fn load(client: RommClient, connection: Connection) -> Result<Sources, String> {
    let smart = client.fetch_smart_collections(&connection).await.map_err(|e| e.to_string())?;
    let generated = client.fetch_virtual_collections(&connection).await.map_err(|e| e.to_string())?;
    Ok((smart, generated))
}

/// The player's own collections, and their smart ones when RomM answers for those too.
///
/// The two made by hand — in RomM's web app as often as here — so they are asked for again each time
/// the Collections page opens. Otherwise one made while the app is open stays missing until the
/// library loads again, which on a paired start is the next launch.
pub async fn load_own(client: RommClient, connection: Connection) -> Result<(Vec<Collection>, Option<Vec<Collection>>), String> {
    let (own, smart) = futures::join!(client.fetch_collections(&connection), client.fetch_smart_collections(&connection));
    Ok((own.map_err(|e| e.to_string())?, smart.ok()))
}

/// Which band of the Collections page has the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zone {
    Kinds,
    #[default]
    Cards,
}

/// The Collections page's cursor: a row of kind chips over a card grid. The grid walks like any
/// card grid ([`CardFocus`]) and starts over whenever the kind changes, since each kind is a
/// different set of cards.
#[derive(Debug, Default)]
pub struct CollectionsFocus {
    pub kind: usize,
    pub zone: Zone,
    pub cards: CardFocus,
}

impl CollectionsFocus {
    /// Move. Up off the grid's top row reaches the chips, Down from the chips the grid; on the chips
    /// Left and Right change kind. `cards` is how many cards the current kind has.
    pub fn step(&mut self, direction: Direction, kinds: usize, cards: usize) -> bool {
        match (self.zone, direction) {
            (Zone::Kinds, Direction::Left) if self.kind > 0 => self.show(self.kind - 1),
            (Zone::Kinds, Direction::Right) if self.kind + 1 < kinds => self.show(self.kind + 1),
            (Zone::Kinds, Direction::Down) if cards > 0 => self.zone = Zone::Cards,
            (Zone::Cards, Direction::Up) if self.cards.index() < CARD_COLS => self.zone = Zone::Kinds,
            (Zone::Cards, direction) => return self.cards.step(direction, cards),
            _ => return false,
        }
        true
    }

    /// Show kind `index`, from its first card.
    pub fn show(&mut self, index: usize) {
        self.kind = index;
        self.cards = CardFocus::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        crate::fixtures::library(12)
    }

    fn collection(id: i64, name: &str, rom_ids: Vec<i64>, favourite: bool) -> Collection {
        Collection { id, name: Some(name.into()), rom_count: rom_ids.len() as i64, rom_ids, is_favorite: favourite, is_virtual: false, is_smart: false }
    }

    #[test]
    fn three_sources_become_one_list_by_kind_without_favourites_or_empty_ones() {
        let library = library();
        let games: Vec<i64> = library.all_games().iter().map(|g| g.rom_id).collect();
        let manual = vec![collection(1, "Couch co-op", vec![games[0], games[1]], false), collection(2, "Favourites", vec![games[2]], true)];
        let smart = vec![collection(1, "Unplayed RPGs", vec![games[3]], false)];
        let generated = vec![
            VirtualCollection { id: "g-1".into(), name: Some("Action".into()), kind: "genre".into(), rom_ids: vec![games[4], games[0]] },
            VirtualCollection { id: "s-1".into(), name: Some("Mario".into()), kind: "collection".into(), rom_ids: vec![games[5]] },
            VirtualCollection { id: "g-2".into(), name: Some("Nowhere".into()), kind: "genre".into(), rom_ids: vec![999_999] },
            VirtualCollection { id: "x".into(), name: Some("Unknown".into()), kind: "age_rating".into(), rom_ids: vec![games[6]] },
        ];
        let all = Collections::build(&library, &HashSet::new(), &manual, &smart, &generated);

        assert_eq!(all.kinds(), [Kind::Yours, Kind::Smart, Kind::Genre, Kind::Series], "no favourites, so no chip for them");
        assert!(all.get("manual:2").is_none(), "RomM's copy of favourites is not one of the player's own");
        assert!(all.get("virtual:g-2").is_none(), "none of its games are in the library");
        assert_eq!(all.get("smart:1").unwrap().kind, Kind::Smart, "smart ids do not collide with manual ones");
        let action = all.games("virtual:g-1", &library);
        assert_eq!(action.iter().map(|g| g.rom_id).collect::<Vec<_>>(), [games[0], games[4]], "in library order");
    }

    #[test]
    fn a_sibling_builds_rom_id_finds_its_tile() {
        let mut library = library();
        let tile = library.all_games()[0].rom_id;
        let mut games: Vec<Game> = library.all_games().to_vec();
        games[0].sibling_ids = vec![7_000];
        library = Library::new(library.systems.clone(), games);
        let all = Collections::build(&library, &HashSet::new(), &[collection(1, "Europe", vec![7_000], false)], &[], &[]);
        assert_eq!(all.get("manual:1").unwrap().members, [tile]);
    }

    #[test]
    fn favourites_lead_as_their_games_by_tile_in_library_order() {
        let mut library = library();
        let mut games: Vec<Game> = library.all_games().to_vec();
        games[0].sibling_ids = vec![7_000];
        library = Library::new(library.systems.clone(), games);
        let tiles: Vec<i64> = library.all_games().iter().map(|g| g.rom_id).collect();
        // A sibling build favourited on RomM's web app, a tile from here, and a rom this library lacks.
        let favourites = HashSet::from([tiles[5], 7_000, 999_999]);
        let manual = vec![collection(1, "Couch co-op", vec![tiles[1]], false)];
        let all = Collections::build(&library, &favourites, &manual, &[], &[]);

        assert_eq!(all.kinds(), [Kind::Favourites, Kind::Yours]);
        assert_eq!(all.favourites(), [tiles[0], tiles[5]], "each by its tile, in library order");
        assert!(Collections::build(&library, &HashSet::new(), &manual, &[], &[]).favourites().is_empty());
    }

    #[test]
    fn the_cursor_walks_kinds_on_the_chips_and_cards_below() {
        let mut f = CollectionsFocus::default();
        assert!(f.step(Direction::Right, 3, 12));
        assert_eq!(f.cards.index(), 1);
        assert!(f.step(Direction::Up, 3, 12), "the top row reaches the chips");
        assert_eq!(f.zone, Zone::Kinds);
        assert!(f.step(Direction::Right, 3, 12));
        assert_eq!((f.kind, f.cards.index()), (1, 0), "a new kind starts at its first card");
        assert!(!f.step(Direction::Up, 3, 12));
        assert!(f.step(Direction::Down, 3, 4));
        assert_eq!(f.zone, Zone::Cards);
    }
}
