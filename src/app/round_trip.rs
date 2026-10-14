//! End-to-end tests against a stub RomM server.
//!
//! These are the two paths this build exists for — pair, then load — and both are mostly
//! *decoding*. The response shapes below are the ones a real RomM sends: `raw_token` rather than
//! `token`, `{items: [...]}` rather than a bare array for roms but a bare array for platforms,
//! `ss_metadata` paths relative to the resource root, and `rom_user.last_played` on a sibling
//! rather than on the tile.

use super::*;
use crate::stub_server;

fn exchange_ok() -> (&'static str, u16, String) {
    (
        "POST /api/client-tokens/exchange",
        200,
        serde_json::json!({
            "id": 7,
            "name": "Steam Deck",
            "raw_token": "rmm_0123456789abcdef",
        })
        .to_string(),
    )
}

fn user_ok() -> (&'static str, u16, String) {
    ("GET /api/users/me", 200, serde_json::json!({ "id": 1, "username": "mattias" }).to_string())
}

#[tokio::test]
async fn pairing_stores_the_token_and_the_identity_that_makes_it_revocable() {
    let host = stub_server::start(vec![exchange_ok(), user_ok()]).await;
    let client = RommClient::new().unwrap();

    let connection = pair(&client, vec![host.clone()], "ABCDEFGH".into(), false)
        .await
        .expect("pairing succeeds");

    assert_eq!(connection.host, host);
    assert_eq!(connection.token, "rmm_0123456789abcdef");
    // Without the id there is no way to tell this device's token from another's later.
    assert_eq!(connection.token_id, Some(7));
    assert_eq!(connection.token_name.as_deref(), Some("Steam Deck"));
}

#[tokio::test]
async fn a_token_that_pairs_but_cannot_read_is_refused_rather_than_stored() {
    // The exchange succeeds and `/users/me` then 401s — a revoked or scope-less token. Storing
    // it would open the app on an empty library instead of back on the login screen.
    let host = stub_server::start(vec![
        exchange_ok(),
        ("GET /api/users/me", 401, "{\"detail\":\"unauthorized\"}".into()),
    ])
    .await;
    let client = RommClient::new().unwrap();

    let error = pair(&client, vec![host], "ABCDEFGH".into(), false)
        .await
        .expect_err("a token that cannot read is not a pairing");
    assert!(error.contains("Authentication failed"), "got: {error}");
}

#[tokio::test]
async fn an_expired_code_reports_what_to_do_about_it() {
    let host = stub_server::start(vec![(
        "POST /api/client-tokens/exchange",
        400,
        "{\"detail\":\"expired\"}".into(),
    )])
    .await;
    let client = RommClient::new().unwrap();

    let error = pair(&client, vec![host], "ABCDEFGH".into(), false).await.unwrap_err();
    assert!(error.contains("Generate a new one in RomM"), "got: {error}");
}

#[tokio::test]
async fn an_unreachable_candidate_falls_through_to_the_next_one() {
    let host = stub_server::start(vec![exchange_ok(), user_ok()]).await;
    let client = RommClient::new().unwrap();

    // The https attempt a bare LAN address makes first cannot connect; the http one behind it
    // is the real server. This is the everyday case for a RomM with no certificate.
    let dead = "https://127.0.0.1:1".to_string();
    let connection = pair(&client, vec![dead, host.clone()], "ABCDEFGH".into(), false)
        .await
        .expect("the reachable candidate is used");
    assert_eq!(connection.host, host);
}

#[tokio::test]
async fn an_http_rejection_does_not_burn_the_code_on_the_next_candidate() {
    // The first candidate answers 400. Trying the second would spend a code valid once, so the
    // failure must be reported instead — even though a working server sits behind it.
    let good = stub_server::start(vec![exchange_ok(), user_ok()]).await;
    let bad = stub_server::start(vec![(
        "POST /api/client-tokens/exchange",
        400,
        "{\"detail\":\"expired\"}".into(),
    )])
    .await;
    let client = RommClient::new().unwrap();

    let error = pair(&client, vec![bad, good], "ABCDEFGH".into(), false)
        .await
        .expect_err("a server that said no is answered, not retried");
    assert!(error.contains("Generate a new one"), "got: {error}");
}

/// The library load, through the real response shapes, all the way to the Home shelves.
#[tokio::test]
async fn a_library_load_builds_the_home_shelves_from_romm_shaped_json() {
    let platforms = serde_json::json!([
        { "id": 1, "name": "Super Nintendo", "slug": "snes", "rom_count": 2 },
        // Filtered out before any rom fetch: nothing to contribute to a shelf.
        { "id": 2, "name": "Empty Console", "slug": "empty", "rom_count": 0 }
    ])
    .to_string();

    let roms = serde_json::json!({
        "total": 2,
        "items": [
            {
                "id": 10,
                "name": "Chrono Trigger",
                "platform_id": 1,
                "platform_slug": "snes",
                "summary": "Time travel, and a very good soundtrack.",
                "path_cover_large": "/assets/romm/resources/roms/1/10/cover/big.png",
                "created_at": "2026-02-01T10:00:00",
                "metadatum": { "average_rating": 95.0, "first_release_date": 762_048_000_000i64 },
                // The play is on a *sibling* build, which is exactly the case the ungrouped
                // played-map exists to catch.
                "sibling_roms": [{ "id": 11 }],
                "ss_metadata": {
                    "logo_path": "roms/1/10/logo/logo.png",
                    "marquee_path": "roms/1/10/marquee/m.png",
                    "miximage_v2_path": "roms/1/10/mix/mix.png"
                }
            },
            {
                "id": 12,
                "name": "The Lost City",
                "platform_id": 1,
                "platform_slug": "snes",
                "created_at": "2026-03-01T10:00:00"
            },
            // Hidden by this user in RomM, so not in the library here either.
            {
                "id": 13,
                "name": "Hidden Away",
                "platform_id": 1,
                "platform_slug": "snes",
                "rom_user": { "hidden": true }
            }
        ]
    })
    .to_string();

    // The ungrouped, last_played-ordered call: rom 11 is the sibling that was actually played.
    let played = serde_json::json!({
        "total": 1,
        "items": [{ "id": 11, "name": "Chrono Trigger (JP)", "rom_user": { "last_played": "2026-08-01T21:00:00" } }]
    })
    .to_string();

    let collections = serde_json::json!([
        { "id": 3, "name": "RPGs", "rom_count": 1, "rom_ids": [10], "is_favorite": false },
        // Skipped: favourites has its own affordance elsewhere.
        { "id": 4, "name": "Favourites", "rom_count": 1, "rom_ids": [12], "is_favorite": true }
    ])
    .to_string();

    let (host, requests) = stub_server::start_recording(vec![
        ("GET /api/platforms", 200, platforms),
        // Order matters: the played-map call is the one *without* a platform filter, so it must
        // be matched before the general roms route.
        ("GET /api/roms?limit=250&offset=0&order_by=last_played", 200, played),
        ("GET /api/roms", 200, roms),
        ("GET /api/collections", 200, collections),
    ])
    .await;

    let client = RommClient::new().unwrap();
    let connection = Connection {
        host: host.clone(),
        token: "rmm_test".into(),
        insecure: false,
        token_id: None,
        token_name: None,
        device_id: None,
    };

    // The same sequence `load_library` runs, without the iced stream around it.
    let platforms: Vec<_> = client
        .fetch_platforms(&connection)
        .await
        .unwrap()
        .into_iter()
        .filter(|p| p.rom_count > 0 && p.slug.is_some())
        .collect();
    assert_eq!(platforms.len(), 1, "the empty console is dropped before any rom fetch");

    let played_roms = client.fetch_played(&connection).await.unwrap();
    assert!(
        requests.lock().unwrap().iter().any(|r| r.contains("order_by=last_played") && r.contains("last_played=true")),
        "played roms are asked for by filter: ordered alone, RomM answers with the whole library"
    );
    let played: HashMap<i64, String> = played_roms
        .iter()
        .filter_map(|r| r.rom_user.as_ref().and_then(|u| u.last_played.clone()).map(|lp| (r.id, lp)))
        .collect();
    assert_eq!(played.get(&11).map(String::as_str), Some("2026-08-01T21:00:00"));

    let page = client
        .fetch_roms(&connection, &RomQuery::platform_page(1, 0))
        .await
        .unwrap();
    let games = crate::library::games_from(&page.items, "snes", &connection.host, &played);

    let library = Library::new(
        vec![System { id: "snes".into(), name: "Super Nintendo".into(), cover_ratio: 1.37 }],
        games,
    );
    assert_eq!(library.total_games, 2, "the hidden one is left out");

    // "The Lost City" sorts under L, not T — so it falls *after* Chrono Trigger, which is the
    // whole point of stripping the article.
    assert_eq!(
        library.games_for("snes").iter().map(|g| g.title.as_str()).collect::<Vec<_>>(),
        vec!["Chrono Trigger", "The Lost City"],
    );

    let hero = library.hero().cloned().expect("the sibling's play makes Chrono Trigger the hero");
    assert_eq!(hero.rom_id, 10);
    assert_eq!(hero.rating, Some(4.75), "95/100 becomes 4.75 stars");
    // The cover is absolute against the connected host; the ss_metadata paths are prefixed with
    // the resource root, without which RomM serves its SPA shell instead of the image.
    assert_eq!(
        hero.cover_url.as_deref(),
        Some(format!("{host}/assets/romm/resources/roms/1/10/cover/big.png").as_str())
    );
    assert_eq!(hero.logo_path.as_deref(), Some("/assets/romm/resources/roms/1/10/logo/logo.png"));
    assert_eq!(hero.marquee_path.as_deref(), Some("/assets/romm/resources/roms/1/10/marquee/m.png"));

    let collections = client.fetch_collections(&connection).await.unwrap();
    let shelves = home_shelves(&library, &collections, &Default::default());
    let titles: Vec<&str> = shelves.iter().map(|s| s.title.as_str()).collect();
    // Continue playing leads with the one game played. Favourites is skipped; RPGs becomes a shelf.
    assert_eq!(titles, vec!["Continue playing", "Recently added", "RPGs"]);
    assert_eq!(shelves[0].games[0].rom_id, hero.rom_id, "and Home opens on it");

    // And the whole thing renders.
    let (mut app, _) = App::new(Some(0));
    app.library = Arc::new(library);
    app.collections = collections;
    app.shelves = shelves;
    app.focus.reconcile(&app.shelves);
    app.screen = Screen::Browsing(Route::Home);
    let _ = app.view();
}

#[tokio::test]
async fn a_favourite_goes_into_romms_favourites_collection_and_comes_back_out() {
    let favourites = |ids: &[i64]| {
        serde_json::json!({ "id": 9, "name": "Favourites", "rom_ids": ids, "rom_count": ids.len(), "is_favorite": true }).to_string()
    };
    let (host, requests) = stub_server::start_recording(vec![
        ("POST /api/collections/9/roms", 200, favourites(&[1, 37])),
        ("DELETE /api/collections/9/roms", 200, favourites(&[1])),
        ("POST /api/collections?is_favorite=true", 200, favourites(&[])),
    ])
    .await;
    let client = RommClient::new().unwrap();
    let connection = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };

    let added = client.add_to_collection(&connection, 9, &[37]).await.unwrap();
    assert!(added.is_favorite && added.rom_ids.contains(&37), "RomM's answer is the collection as it now is");
    let removed = client.remove_from_collection(&connection, 9, &[37]).await.unwrap();
    assert_eq!(removed.rom_ids, [1]);
    // An account with none yet gets one, as RomM's web app makes it.
    let made = client.create_favourites(&connection).await.unwrap();
    assert!(made.is_favorite);

    let lines = requests.lock().unwrap().clone();
    assert!(lines[0].starts_with("POST /api/collections/9/roms"), "{lines:?}");
    assert!(lines[1].starts_with("DELETE /api/collections/9/roms"), "{lines:?}");
    assert!(lines[2].starts_with("POST /api/collections?is_favorite=true"), "{lines:?}");
}
