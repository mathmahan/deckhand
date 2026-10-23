//! What art the page needs, and fetching it — see [`crate::covers`].

use super::*;

impl App {
    /// Every piece of art the page on screen, the hero and Home's shelves draw: its URL, the box it
    /// is drawn into, and how it is prepared.
    pub(super) fn wanted_art(&self) -> Vec<(String, covers::Target, covers::Kind)> {
        let mut wanted: Vec<(String, covers::Target, covers::Kind)> = Vec::new();
        // Every height below is in layout units, the way the views state them; this is what turns
        // them into the pixels the panel will actually draw.
        let px = |units: f32| units * self.layout_scale() * self.panel_density;

        // Whatever page is open asks for its own art first, so the thing being looked at resolves
        // before Home's shelves finish filling in behind it.
        match &self.screen {
            Screen::Browsing(Route::Browse { scope }) => {
                // Only what the viewport can reach. A grid is the one page in the app that is
                // genuinely unbounded — a system runs to thousands of games — and asking for all of
                // it decoded every cover in the library at once: ~750 KB of RGBA each, held for as
                // long as the app ran, against the ~30 tiles actually on screen.
                let (view, spec) = self.scope_view(scope);
                let packing = self.packing(&view, &spec);
                // A tile is its game's *system's* box — the one box in the app not shaped by the art
                // inside it — so a scan of a slightly different shape is cropped to fit, and needs
                // the rows that crop eats. Each tile's own box, since a grid mixes systems' shapes.
                for i in self.grid_window(&spec, &packing) {
                    if let Some(url) = &view.games[i].cover_url {
                        let tile = covers::Target {
                            height_px: px(spec.tile_h),
                            box_ratio: Some(packing.width(i) / spec.tile_h.max(1.0)),
                        };
                        wanted.push((url.clone(), tile, covers::Kind::Art));
                    }
                }
                // The spotlight's picture, and its neighbours' ahead of a step.
                for game in self.library_spotlit() {
                    wanted.extend(pictures_of(game, &px));
                }
                // The system list's icons: a few dozen, and small.
                if matches!(scope, Scope::System(_)) {
                    for system in &self.library.systems {
                        wanted.push((self.platform_logo(&system.id), covers::Target::tall(px(crate::ui::browse::LOGO_H)), covers::Kind::Art));
                    }
                }
            }
            Screen::Browsing(Route::Detail { rom_id, .. }) => {
                if let Some(game) = self.library.game(*rom_id)
                {
                    // A feature, held apart from the grid's copy: a Browse tile takes RomM's small
                    // cover, cut to the tile, and the page drawing that one half again as large is
                    // what made a cover opened from its system's grid soft.
                    if let Some(url) = &game.cover_url {
                        wanted.push((url.clone(), covers::Target::tall(px(crate::ui::detail::COVER_H)), covers::Kind::Feature));
                    }
                    // The picture and light behind the page, as Home's spotlight has them.
                    wanted.extend(pictures_of(game, &px));
                    for shot in game.screenshots.iter().take(crate::ui::detail::SHOTS_SHOWN) {
                        wanted.push((shot.clone(), covers::Target::tall(px(crate::ui::detail::SCREENSHOT_H)), covers::Kind::Art));
                    }
                    // The screenshot open over the window, fitted to it, and those either side
                    // ahead of a step.
                    if let Some(index) = self.detail_focus.viewing {
                        let shots = crate::ui::detail::shown_shots(game, &self.covers);
                        let (side, end) = crate::ui::detail::VIEWER_MARGIN;
                        let (w, h) = ((self.content_width - 2.0 * side).max(1.0), (self.content_height - 2.0 * end).max(1.0));
                        let fitted = covers::Target { height_px: px(h), box_ratio: Some(w / h) };
                        for i in [Some(index), index.checked_sub(1), Some(index + 1)].into_iter().flatten() {
                            if let Some(url) = shots.get(i) {
                                wanted.push(((*url).clone(), fitted, covers::Kind::Full));
                            }
                        }
                    }
                    // The badges only while the list showing them is open.
                    if self.achievements_open
                        && let Some(Ok(game)) = self.achievements.get(rom_id)
                    {
                        for achievement in game.list.iter().take(crate::ui::detail::ACHIEVEMENTS_SHOWN) {
                            if let Some(url) = crate::ui::detail::achievement_badge(achievement, game.locked(achievement), self.host()) {
                                wanted.push((url, covers::Target::tall(px(crate::ui::detail::BADGE)), covers::Kind::Art));
                            }
                        }
                    }
                }
            }
            // The covers at the Downloads rows' left, at a shelf's size, so the two share one copy: the
            // first few dozen, which is more than a window shows.
            Screen::Browsing(Route::Downloads) => {
                for url in self.download_rows().iter().filter_map(|row| row.cover).take(60) {
                    wanted.push((url.to_string(), covers::Target::tall(px(crate::ui::home::COVER_H)), covers::Kind::Art));
                }
            }
            // The covers beside the save sync page's games, at a shelf's size, so the two share one copy.
            Screen::Browsing(Route::Sync { .. }) => {
                for game in self.sync_covers() {
                    if let Some(url) = &game.cover_url {
                        wanted.push((url.clone(), covers::Target::tall(px(crate::ui::home::COVER_H)), covers::Kind::Art));
                    }
                }
            }
            Screen::Browsing(Route::Collections) => {
                // The current kind's covers, for the rows on screen and a couple either side — a
                // kind runs to hundreds of cards (one server here has 378 companies), and decoding
                // every one of them for a page showing fifteen is the waste the game grid avoids.
                let window = crate::ui::collections::cards_in_view(self.collections_scroll, self.content_height);
                let cards = self.collection_cards();
                let window = window.start.min(cards.len())..window.end.min(cards.len());
                for card in &cards[window] {
                    if let Some(url) = card.cover_url {
                        wanted.push((url.to_string(), covers::Target::tall(px(crate::ui::collections::COVER_H)), covers::Kind::Art));
                    }
                }
            }
            Screen::Browsing(Route::Saves { rom_id, .. }) => {
                // Every screenshot the game's records carry, at the preview's size; a row draws the
                // same picture small rather than asking for a second copy.
                if let Some(Ok(saves)) = self.game_saves.get(rom_id) {
                    for record in saves.states.iter().chain(&saves.saves).take(40) {
                        if let Some(url) = crate::ui::saves::screenshot_url(record, self.host()) {
                            wanted.push((url, covers::Target::tall(px(crate::ui::saves::PREVIEW_H)), covers::Kind::Art));
                        }
                    }
                }
            }
            _ => {}
        }

        if matches!(self.screen, Screen::Browsing(Route::Home)) {
            wanted.extend(self.spotlight_art(&px));
        }
        for shelf in &self.shelves {
            for game in &shelf.games {
                if let Some(url) = &game.cover_url {
                    wanted.push((url.clone(), covers::Target::tall(px(crate::ui::home::COVER_H)), covers::Kind::Art));
                }
            }
        }
        wanted
    }

    /// What Home's spotlight draws: for the game the cursor is on and the cards either side of it,
    /// its picture, the light it casts and its logo — the neighbours' so a step along the shelf
    /// finds its picture already here, and the crossfade starts on the press rather than a
    /// download later.
    ///
    /// The game under the cursor first, which is the order the fetches are started in.
    fn spotlight_art(&self, px: &dyn Fn(f32) -> f32) -> Vec<(String, covers::Target, covers::Kind)> {
        let mut wanted = Vec::new();
        let Some(shelf) = self.shelves.get(self.focus.shelf_pos()) else { return wanted };
        let at = self.focus.index_of(shelf);
        let around = [Some(at), at.checked_sub(1), Some(at + 1)];
        for game in around.into_iter().flatten().filter_map(|i| shelf.games.get(i)) {
            wanted.extend(pictures_of(game, px));
            if let Some(path) = &game.logo_path {
                wanted.push((self.asset_url(path), covers::Target::tall(px(100.0)), covers::Kind::Art));
            }
        }
        wanted
    }

    /// Queue every piece of art Home will draw.
    ///
    /// Fetched up front rather than lazily as cards scroll into view: iced's `view` cannot start
    /// work, so a lazy pipeline would need the view to report visibility back through a message,
    /// and Home is a bounded set — two shelves plus a shelf per collection — not an infinite grid.
    pub(super) fn fetch_art(&mut self) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else {
            return Task::none();
        };

        let wanted = self.wanted_art();

        // Claim before spawning, so the same cover on two shelves is fetched once. Claimed under the
        // key the art will be held by — one screenshot is held as three images — and fetched by
        // its URL.
        let jobs: Vec<_> = wanted
            .into_iter()
            .filter(|(url, _, kind)| self.covers.claim(&covers::key(url, *kind)))
            .collect();
        if jobs.is_empty() {
            return Task::none();
        }

        let client = self.client.clone();
        let generation = self.art_generation;
        Task::run(
            iced::stream::channel(256, move |mut output: Sender<ArtResult>| async move {
                let mut stream = futures::stream::iter(jobs)
                    .map(|(url, target, kind)| {
                        let client = client.clone();
                        let connection = connection.clone();
                        async move {
                            // Already prepared, from an earlier run or from before the memory
                            // budget dropped it? Then nothing else is needed - not the original on
                            // disk, and certainly not the network. This is the ordinary case every
                            // time art comes back on screen, and asking first is what makes
                            // returning to a system you have already looked at cost nothing.
                            //
                            // Decoding is CPU work either way: off the UI thread, or a shelf's
                            // worth at once is a visible hitch.
                            let for_cache = url.clone();
                            let hit = off_thread(move || {
                                covers::prepared_art(&for_cache, target, kind)
                            })
                            .await
                            .ok()
                            .flatten();

                            let prepared = match hit {
                                Some(art) => Some(art),
                                None => {
                                    // Which of RomM's two copies is worth the bytes, given the size
                                    // this is drawn at. Picked from the target that won the claim
                                    // above, which is the largest use of this cover on the page -
                                    // `wanted` is built current-page first, then hero, then
                                    // shelves, for that reason.
                                    let source = covers::source_url(&url, target);
                                    let mut bytes = original(&client, &connection, &source).await;
                                    // A `small` shorter than the reference server's — a PSP case is
                                    // 200 rows — would be stretched, so `big` it is after all.
                                    if source != url
                                        && bytes.as_deref().is_some_and(|b| covers::falls_short(b, target))
                                        && let Some(big) = original(&client, &connection, &url).await
                                    {
                                        bytes = Some(big);
                                    }
                                    match bytes {
                                        Some(bytes) => {
                                            let for_cache = url.clone();
                                            off_thread(move || {
                                                covers::prepare_cached(&for_cache, &bytes, target, kind)
                                            })
                                            .await
                                            .ok()
                                            .flatten()
                                        }
                                        None => None,
                                    }
                                }
                            };
                            (generation, covers::key(&url, kind), prepared.map(Arc::new))
                        }
                    })
                    .buffer_unordered(COVER_PARALLELISM);

                while let Some(result) = stream.next().await {
                    let _ = output.send(result).await;
                }
                // The full-size decodes the batch went through are done with — see `memory`.
                crate::memory::release();
            }),
            |(generation, url, art)| Message::CoverLoaded(generation, url, art),
        )
    }
}

/// The bytes RomM sent for `source`: off the disk when it has been fetched before — a cold start
/// should show real art in the first frame rather than fetch what it already has — or fetched, and
/// kept. Keyed by the URL actually fetched, so RomM's two copies of a cover cannot be mistaken for
/// one another on disk.
async fn original(client: &RommClient, connection: &Connection, source: &str) -> Option<Vec<u8>> {
    if let Some(bytes) = covers::read_cached(source) {
        return Some(bytes);
    }
    let bytes = client.fetch_asset(connection, source).await.ok()?;
    covers::write_cached(source, &bytes);
    Some(bytes)
}

/// What a spotlight draws of `game` behind it: its picture — a screenshot, or its cover as a card —
/// and the light that casts.
fn pictures_of(game: &Game, px: &dyn Fn(f32) -> f32) -> Vec<(String, covers::Target, covers::Kind)> {
    use crate::ui::home::{self, Picture};
    match home::picture_of(game) {
        Some(Picture::Shot(url)) => {
            let (w, h) = home::PICTURE;
            vec![
                (url.to_string(), covers::Target { height_px: px(h), box_ratio: Some(w / h) }, covers::Kind::Spotlight),
                (url.to_string(), covers::Target::tall(px(40.0)), covers::Kind::Ambient),
            ]
        }
        Some(Picture::Cover(url)) => vec![
            (url.to_string(), covers::Target::tall(px(home::CARD_H)), covers::Kind::Feature),
            (url.to_string(), covers::Target::tall(px(40.0)), covers::Kind::Ambient),
        ],
        None => Vec::new(),
    }
}
