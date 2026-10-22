//! Signing in, and the library: loading it, refreshing it, and finding what is already on disk.

use super::*;

impl App {
    /// Pair with the server, trying each host candidate in turn.
    ///
    /// A *transport* failure moves on to the next candidate (an https-first attempt against a LAN
    /// server with no certificate is expected to fail that way); an HTTP status does not — that
    /// means we reached the right server and it said no, so trying the same code over plaintext
    /// would only burn it.
    pub(super) fn connect(&mut self) -> Task<Message> {
        let Screen::Login(state) = &mut self.screen else {
            return Task::none();
        };
        if state.connecting || !state.complete() {
            return Task::none();
        }
        state.connecting = true;
        state.status = Some("Connecting…".into());

        let candidates = host_candidates(&state.url);
        let code = normalize_code(&state.code);
        let insecure = state.insecure;
        let client = self.client.clone();

        Task::perform(
            async move { pair(&client, candidates, code, insecure).await.map(Box::new) },
            Message::Paired,
        )
    }

    /// Reconcile the cached library with the server, re-reading only what moved.
    ///
    /// The counterpart to [`App::load_library`], and the reason a paired start is instant: the
    /// platform list is one request and carries a `rom_count` per system, so the systems whose
    /// count still matches the cache are carried over untouched and never fetched. On a library
    /// nobody has added to, that is the whole walk skipped.
    ///
    /// A rom edited in place — a rescrape, a rename, a file swapped one-for-one — leaves the count
    /// where it was, so RomM is also asked which roms it changed since the last read, and their
    /// systems are read again too.
    ///
    /// Deliberately silent. Home is already on screen and usable, so this reports no progress and
    /// takes over no screen; only a failure surfaces, through the banner every refresh failure
    /// uses.
    pub(super) fn sync_library(&self) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else {
            return Task::none();
        };
        let client = self.client.clone();
        let cached_counts = self.system_counts.clone();
        let since = self.library_read_at.clone();
        // The cached games, by system, so the ones that did not move can be handed straight back.
        let cached_games: HashMap<String, Vec<Game>> = self
            .library
            .systems
            .iter()
            .map(|s| (s.id.clone(), self.library.games_for(&s.id).to_vec()))
            .collect();

        Task::run(
            iced::stream::channel(64, move |mut output: Sender<Progress>| async move {
                let started = read_time();
                let platforms = match client.fetch_platforms(&connection).await {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = output.send(Progress::Failed(e.to_string())).await;
                        return;
                    }
                };
                let platforms: Vec<_> = platforms
                    .into_iter()
                    .filter(|p| p.rom_count > 0 && p.slug.is_some())
                    .collect();
                let mut counts = platform_counts(&platforms);
                let mut stale: std::collections::HashSet<String> =
                    cache::stale(&cached_counts, &counts).into_iter().collect();
                // What RomM changed without changing a count. Unanswered, the next sync asks from the
                // same time again rather than from this one.
                let mut read_at = since.clone();
                match &since {
                    Some(since) => {
                        if let Ok(changed) = client.changed_platforms(&connection, since).await {
                            stale.extend(platforms.iter().filter(|p| changed.contains(&p.id)).filter_map(|p| p.slug.clone()));
                            read_at = Some(started);
                        }
                    }
                    None => read_at = Some(started),
                }

                // Always re-read, whatever moved: play history is what *Continue playing* — and so
                // the game Home opens on — is built from, and it changes without any count changing.
                let played = played_at(&client, &connection).await;

                let to_read: Vec<_> = platforms
                    .iter()
                    .filter(|p| stale.contains(p.slug.as_deref().unwrap_or_default()))
                    .cloned()
                    .collect();

                let results: Vec<(String, Option<Vec<Game>>)> = futures::stream::iter(to_read)
                    .map(|platform| {
                        let (client, connection, played) = (&client, &connection, &played);
                        async move { (platform.slug.clone().unwrap_or_default(), read_platform(client, connection, &platform, played).await) }
                    })
                    .buffer_unordered(FETCH_PARALLELISM)
                    .collect()
                    .await;
                let mut fetched: HashMap<String, Vec<Game>> = HashMap::new();
                for (slug, games) in results {
                    match games {
                        Some(games) => {
                            fetched.insert(slug, games);
                        }
                        // A platform failing loses that platform's update, not the sync: it keeps the
                        // games it had, and no count, so the next sync reads it again.
                        None => {
                            counts.remove(&slug);
                        }
                    }
                }

                let mut systems: Vec<System> = Vec::with_capacity(platforms.len());
                let mut games: Vec<Game> = Vec::new();
                for platform in &platforms {
                    let slug = platform.slug.clone().unwrap_or_default();
                    systems.push(System {
                        id: slug.clone(),
                        name: systems::display_name(&slug, &platform.display_name()),
                        cover_ratio: systems::ratio_for(&slug),
                    });
                    match fetched.remove(&slug) {
                        // Freshly read: `from_rom` already resolved play history against `played`.
                        Some(fresh) => games.extend(fresh),
                        // Carried over, so its play history is as old as the cache file is.
                        None => games.extend(cached_games.get(&slug).into_iter().flatten().map(|g| {
                            let mut game = g.clone();
                            cache::restamp(&mut game, &played);
                            game
                        })),
                    }
                }
                // The same stable order the full load settles on: `buffer_unordered` yields by
                // completion, and a system's place must not depend on how fast it read.
                systems.sort_by(|a, b| a.name.cmp(&b.name));

                let collections = client.fetch_collections(&connection).await.unwrap_or_default();

                let _ = output
                    .send(Progress::Loaded(Arc::new(Loaded {
                        library: Library::new(systems, games),
                        collections,
                        counts,
                        read_at,
                    })))
                    .await;
            }),
            Message::LibraryProgress,
        )
    }

    pub(super) fn load_library(&self) -> Task<Message> {
        let Some(connection) = self.settings.connection.clone() else {
            return Task::none();
        };
        let client = self.client.clone();

        Task::run(
            iced::stream::channel(64, move |mut output: Sender<Progress>| async move {
                let read_at = Some(read_time());
                let platforms = match client.fetch_platforms(&connection).await {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = output.send(Progress::Failed(e.to_string())).await;
                        return;
                    }
                };
                // A platform with no roms has nothing to contribute to any shelf.
                let platforms: Vec<_> = platforms
                    .into_iter()
                    .filter(|p| p.rom_count > 0 && p.slug.is_some())
                    .collect();
                let mut counts = platform_counts(&platforms);
                let total = platforms.len();
                let _ = output.send(Progress::Started { platforms: total }).await;

                let played = played_at(&client, &connection).await;

                let done = std::sync::atomic::AtomicUsize::new(0);
                let results: Vec<(System, Option<Vec<Game>>)> = futures::stream::iter(platforms)
                    .map(|platform| {
                        let client = client.clone();
                        let connection = connection.clone();
                        let played = &played;
                        let done = &done;
                        let mut output = output.clone();
                        async move {
                            let slug = platform.slug.clone().unwrap_or_default();
                            let system = System {
                                id: slug.clone(),
                                name: systems::display_name(&slug, &platform.display_name()),
                                cover_ratio: systems::ratio_for(&slug),
                            };

                            let games = read_platform(&client, &connection, &platform, played).await;

                            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            let _ = output
                                .send(Progress::Platform { done: n, total, name: system.name.clone() })
                                .await;
                            (system, games)
                        }
                    })
                    .buffer_unordered(FETCH_PARALLELISM)
                    .collect()
                    .await;

                // Restore a stable order: `buffer_unordered` yields by completion, which would
                // shuffle the systems between runs.
                let mut results = results;
                results.sort_by(|a, b| a.0.name.cmp(&b.0.name));

                let systems: Vec<System> = results.iter().map(|(s, _)| s.clone()).collect();
                // One platform failing loses that platform, not the library — and has no count, so
                // the sync after this reads it again rather than taking it as read.
                for (system, games) in &results {
                    if games.is_none() {
                        counts.remove(&system.id);
                    }
                }
                let games: Vec<Game> = results.into_iter().flat_map(|(_, g)| g.unwrap_or_default()).collect();

                // Best effort: collections are extra shelves, so failing to read them costs those
                // shelves, not the library.
                let collections = client.fetch_collections(&connection).await.unwrap_or_default();

                let _ = output
                    .send(Progress::Loaded(Arc::new(Loaded {
                        library: Library::new(systems, games),
                        collections,
                        counts,
                        read_at,
                    })))
                    .await;
            }),
            Message::LibraryProgress,
        )
    }

    pub(super) fn on_progress(&mut self, progress: Progress) -> Task<Message> {
        match progress {
            Progress::Started { platforms } => {
                self.screen = Screen::Loading {
                    status: format!("Reading {platforms} systems…"),
                };
                Task::none()
            }
            Progress::Platform { done, total, name } => {
                if let Screen::Loading { status } = &mut self.screen {
                    *status = format!("{name} — {done} of {total} systems");
                }
                Task::none()
            }
            Progress::Failed(error) => {
                // A refresh that fails with a library already on screen keeps the library: the
                // banner says the data is stale, which is far better than throwing it away.
                self.say(error.clone());
                if !matches!(self.screen, Screen::Browsing(_)) {
                    let mut login = LoginState::new(
                        self.settings
                            .connection
                            .as_ref()
                            .map(|c| c.host.clone())
                            .unwrap_or_else(|| "https://".into()),
                    );
                    login.status = Some(error);
                    self.screen = Screen::Login(login);
                }
                Task::none()
            }
            Progress::Loaded(loaded) => {
                let loaded = Arc::try_unwrap(loaded).unwrap_or_else(|arc| Loaded {
                    library: arc.library.clone(),
                    collections: arc.collections.clone(),
                    counts: arc.counts.clone(),
                    read_at: arc.read_at.clone(),
                });
                self.library = Arc::new(loaded.library);
                self.match_downloads();
                self.favourites = favourites_of(&loaded.collections);
                self.collections = loaded.collections;
                self.rebuild_collections();
                self.system_counts = loaded.counts;
                self.library_read_at = loaded.read_at;
                // Written after the library is whole, not per system: a cache is only worth having
                // if reading it back is the same as not having fetched at all.
                // Off the UI thread: some 5 MB of JSON, which held up the first frame of the new library.
                let write_cache = match &self.settings.connection {
                    Some(connection) => {
                        let (host, library, collections, counts, read_at) = (
                            connection.host.clone(),
                            Arc::clone(&self.library),
                            self.collections.clone(),
                            self.system_counts.clone(),
                            self.library_read_at.clone(),
                        );
                        Task::future(off_thread(move || cache::save(&host, &library, &collections, &counts, read_at.as_deref()))).discard()
                    }
                    None => Task::none(),
                };
                self.rebuild_shelves();
                // Keep the route across a refresh: pressing Refresh from a system grid should
                // return to that grid, not throw you back to Home.
                if !matches!(self.screen, Screen::Browsing(_)) {
                    self.screen = Screen::Browsing(Route::Home);
                }
                // The first library after a pairing: the moment to offer RetroDECK, if it is not here.
                let offer = if std::mem::take(&mut self.offer_retrodeck) { self.look_for_retrodeck(Offering::Pairing) } else { Task::none() };
                // RomM's smart and generated collections move with the library: asked again with
                // every load, so the Collections page is current by the time it is opened — and kept
                // on screen until the new answer lands.
                let collections = self.fetch_collections();
                // The pages fetched, the old library and the cache written are done with: given back
                // rather than kept by the allocator — some 40 MB on a library of a few thousand games.
                crate::memory::release();
                // Registered as a RomM device here as well as on the cached path: a library loaded
                // from the server is the one run that has no cache to open on, and without the id
                // every sync would fall back to the engine from before RomM's sync sessions.
                Task::batch([
                    self.fetch_art(),
                    collections,
                    self.detect_games(),
                    write_cache,
                    self.load_awarded(),
                    self.ensure_device(),
                    self.claim_installs(),
                    offer,
                ])
            }
        }
    }

    /// Look for games already in the downloads folder that no record names — see
    /// [`downloads::detect`]. Off the UI thread: it reads every platform folder.
    pub(super) fn detect_games(&self) -> Task<Message> {
        if self.settings.connection.is_none() {
            return Task::none();
        }
        let here = self.downloads.installed_ids();
        let wanted: Vec<downloads::detect::Wanted> = self
            .library
            .all_games().iter()
            // A game being fetched, or whose fetch failed, has files of its own in the folder that
            // are not a game yet.
            .filter(|game| !game.builds().any(|id| here.contains(&id) || self.downloads.state(id) != downloads::SyncState::RemoteOnly))
            .flat_map(|game| {
                game.file_stems.iter().map(move |(rom_id, stem)| downloads::detect::Wanted {
                    rom_id: *rom_id,
                    title: game.title.clone(),
                    system_id: game.system_id.clone(),
                    stem: stem.clone(),
                })
            })
            .collect();
        if wanted.is_empty() {
            return Task::none();
        }
        let (root, taken) = (self.settings.roms_dir(), self.downloads.claimed());
        Task::perform(
            async move {
                off_thread(move || downloads::detect::detect(&root, &wanted, &taken)).await.unwrap_or_default()
            },
            Message::GamesFound,
        )
    }
}

/// When each rom the user has played was last played, by rom id.
///
/// Every rom's own, ungrouped. The library is fetched *grouped*, so a game's tile is one
/// representative rom and its siblings' play history is invisible there — a hack or regional build
/// the user actually played would never light up "Continue playing".
///
/// Best effort: without it the shelves fall back to each tile's own last_played, which is the
/// pre-sibling behaviour, not an empty Home.
async fn played_at(client: &RommClient, connection: &Connection) -> HashMap<i64, String> {
    let played = client.fetch_played(connection).await.unwrap_or_default();
    played.into_iter().filter_map(|r| r.rom_user.and_then(|u| u.last_played).map(|lp| (r.id, lp))).collect()
}

/// How far a library read's time is set back from this device's clock: a read is what the next sync
/// asks RomM for the changes since, and a clock running ahead of the server's would otherwise skip
/// the changes made in between. Too early only reads a system again; too late misses an edit.
pub(super) const SKEW: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Now, as a library read's time — see [`SKEW`].
fn read_time() -> String {
    crate::sync::format_timestamp(std::time::SystemTime::now() - SKEW)
}

/// Every game of one platform, page by page — or None when a page could not be read, so a platform
/// read halfway is never taken for the whole of it.
async fn read_platform(
    client: &RommClient,
    connection: &Connection,
    platform: &crate::romm::models::Platform,
    played: &HashMap<i64, String>,
) -> Option<Vec<Game>> {
    let slug = platform.slug.clone().unwrap_or_default();
    let mut games = Vec::new();
    let mut offset = 0usize;
    loop {
        let page = client.fetch_roms(connection, &RomQuery::platform_page(platform.id, offset)).await.ok()?;
        let count = page.items.len();
        games.extend(crate::library::games_from(&page.items, &slug, &connection.host, played));
        offset += count;
        if count == 0 || offset >= page.total {
            return Some(games);
        }
    }
}
