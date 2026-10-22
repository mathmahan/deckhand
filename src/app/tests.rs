use super::*;
use iced::Color;

/// Building the view is what exercises the layout: every gradient, every ratio division, every
/// truncation and every `Length` runs here. A panic in any of them is a black window at
/// runtime, and this is the cheapest place to catch it.
#[test]
fn every_screen_builds_its_view() {
    // Home, on a fixture library — the spotlight, the shelves, and the art-less card path.
    let (app, _) = App::new(Some(60));
    assert!(matches!(app.screen, Screen::Browsing(Route::Home)));
    assert!(app.focus.spotlit(&app.shelves).is_some(), "fixtures give Home a game to open on");
    let _ = app.view();

    // Login: no connection, no library.
    let (mut app, _) = App::new(Some(0));
    app.screen = Screen::Login(LoginState::new("https://".into()));
    let _ = app.view();

    // The gap between the two, which a slow server makes the longest-lived of the three.
    app.screen = Screen::Loading { status: "Reading 12 systems…".into() };
    let _ = app.view();

    // Downloads, empty and then with a row of each kind — the bar, the reason line and the
    // two section headers all draw off state nothing else on the page produces.
    let (mut app, _) = App::new(Some(12));
    let _ = app.update(Message::TabSelected(Tab::Downloads));
    assert!(matches!(app.screen, Screen::Browsing(Route::Downloads)));
    let _ = app.view();

    let game = app.library.all_games()[0].clone();
    app.downloads.enqueue(QueueItem {
        host: String::new(),
        rom_id: game.rom_id,
        system_id: game.system_id.clone(),
        title: game.title.clone(),
        ..Default::default()
    });
    app.downloads.start_next();
    app.downloads.progress(game.rom_id, 0.37);
    app.downloads.finish(-1, Err("the download stopped early".into()));
    app.downloads.finish(-2, Ok(installed(-2, "On disk", 4 << 20)));
    let _ = app.view();
}

/// A record of a game on disk, for a test that needs one without a server to fetch it from.
/// `files` is empty, so nothing on disk is claimed and nothing has to be cleaned up.
fn installed(rom_id: i64, title: &str, bytes: u64) -> crate::downloads::Installed {
    crate::downloads::Installed {
        // Stamped by `Downloads::finish`, which is the only thing that knows the server.
        host: String::new(),
        rom_id,
        title: title.into(),
        system_id: "snes".into(),
        platform_id: None,
        dir: std::env::temp_dir(),
        files: Vec::new(),
        bytes,
        ..Default::default()
    }
}

/// A paired fixture app. The host is unreachable on purpose: every assertion below is about the
/// state a press moves through, and the task that would do the fetching is dropped unexecuted.
fn paired_fixtures(count: usize) -> App {
    let (mut app, _) = App::new(Some(count));
    app.settings.connection = Some(Connection {
        host: "http://127.0.0.1:1".into(),
        token: "rmm_test".into(),
        insecure: false,
        token_id: None,
        token_name: None,
        device_id: None,
    });
    app
}

#[test]
fn a_download_press_shows_at_once_and_a_second_one_says_it_is_already_coming() {
    use crate::downloads::SyncState;
    let mut app = paired_fixtures(12);
    let rom_id = app.library.all_games()[0].rom_id;

    let _ = app.update(Message::Download(rom_id));
    assert!(
        matches!(app.downloads.state(rom_id), SyncState::Downloading(_)),
        "the badge is optimistic — before anything is asked of the server"
    );
    assert!(app.banner.as_deref().unwrap().starts_with("Downloading"), "{:?}", app.banner);

    let _ = app.update(Message::Download(rom_id));
    assert!(app.banner.as_deref().unwrap().contains("already downloading"));
    assert_eq!(app.downloads.in_progress().count(), 1, "not a second download of one rom");
}

#[test]
fn an_unpaired_press_says_why_rather_than_queueing_something_nothing_can_fetch() {
    let (mut app, _) = App::new(Some(12));
    let rom_id = app.library.all_games()[0].rom_id;
    let _ = app.update(Message::Download(rom_id));
    assert!(app.banner.as_deref().unwrap().contains("Not paired"), "{:?}", app.banner);
    assert_eq!(app.downloads.in_progress().count(), 0);
}

#[test]
fn a_finished_download_reaches_the_shelves_the_chips_and_the_filter() {
    let mut app = paired_fixtures(12);
    let game = app.library.all_games()[0].clone();
    let system = Scope::System(game.system_id.clone());
    assert_eq!(app.chip_counts(&system).installed, 0);
    assert!(!app.shelves.iter().any(|s| s.id == "installed"), "nothing to put on it yet");

    let _ = app.update(Message::DownloadEvent(crate::downloads::Event::Finished(
        game.rom_id,
        Ok(Box::new(installed(game.rom_id, &game.title, 8 << 20))),
    )));

    assert_eq!(app.chip_counts(&system).installed, 1, "the chip counts it");
    let shelf = app
        .shelves
        .iter()
        .find(|s| s.id == "installed")
        .expect("the shelf appears the first time it has an answer");
    assert_eq!(shelf.games.len(), 1);
    assert!(app.banner.as_deref().unwrap().contains("8 MB"), "{:?}", app.banner);

    // And the filter, which could only ever be empty before.
    let _ = app.update(Message::OpenSystem(game.system_id));
    let _ = app.update(Message::FilterSelected(crate::view_model::Filter::Installed));
    assert_eq!(app.scope_view(&system).0.len(), 1);
    let _ = app.view();
}

#[test]
fn an_install_asked_in_romm_is_downloaded_and_answered_and_one_called_off_there_stops() {
    use crate::downloads::SyncState;
    use crate::romm::models::InstallRequest;
    let mut app = paired_fixtures(12);
    let games = app.library.all_games().to_vec();
    let (first, second, here) = (games[0].clone(), games[1].clone(), games[2].clone());
    let request = |id: &str, rom_id: i64| InstallRequest { id: id.into(), rom_id };
    let _ = app.update(Message::DownloadEvent(crate::downloads::Event::Finished(
        here.rom_id,
        Ok(Box::new(installed(here.rom_id, &here.title, 1 << 20))),
    )));

    let taken = vec![request("a", first.rom_id), request("b", second.rom_id), request("c", here.rom_id)];
    let _ = app.update(Message::InstallsClaimed(Ok(taken.clone())));
    assert_eq!(app.installs, Installs::Taking, "RomM answered: this device takes installs");
    assert!(matches!(app.downloads.state(first.rom_id), SyncState::Downloading(_)));
    assert!(matches!(app.downloads.state(second.rom_id), SyncState::Downloading(_)));
    assert!(app.banner.as_deref().is_some_and(|b| b.starts_with("RomM sent")), "{:?}", app.banner);
    assert!(!app.install_requests.contains_key("c"), "one already here is answered at once");

    // RomM hands back what it holds taken until it is answered: none of it is taken twice.
    let _ = app.update(Message::InstallsClaimed(Ok(taken)));
    assert_eq!(app.downloads.in_progress().count(), 2);

    // Called off in RomM: its download stops here too.
    let _ = app.update(Message::InstallsClaimed(Ok(vec![request("a", first.rom_id)])));
    assert!(!matches!(app.downloads.state(second.rom_id), SyncState::Downloading(_)));
    assert!(app.banner.as_deref().is_some_and(|b| b.contains("called off in RomM")), "{:?}", app.banner);

    // Landed: answered, and held no longer.
    let _ = app.update(Message::DownloadEvent(crate::downloads::Event::Finished(
        first.rom_id,
        Ok(Box::new(installed(first.rom_id, &first.title, 1 << 20))),
    )));
    assert!(app.install_requests.is_empty());

    // A game added to RomM since the library was read: RomM is asked what it is, then it comes.
    let _ = app.update(Message::InstallsClaimed(Ok(vec![request("d", 9_999)])));
    assert!(app.install_requests.contains_key("d"), "held while RomM is asked, so it is not taken twice");
    let detail: crate::romm::models::RomDetail =
        serde_json::from_value(serde_json::json!({ "id": 9_999, "name": "New Arrival", "platform_slug": "snes", "files": [] })).unwrap();
    let _ = app.update(Message::InstallDetail(request("d", 9_999), Ok(Box::new(detail))));
    assert!(matches!(app.downloads.state(9_999), SyncState::Downloading(_)));
    assert!(app.banner.as_deref().is_some_and(|b| b.contains("New Arrival")), "{:?}", app.banner);
    let _ = app.view();
}

#[test]
fn a_token_romm_tied_to_no_device_takes_no_installs_and_settings_says_how_to() {
    let mut app = paired_fixtures(4);
    assert!(app.installs.taking(), "asked until RomM says otherwise");
    let refused = |status| ApiError { status, message: String::new(), detail: None };

    // Out of reach: asked again next time.
    let _ = app.update(Message::InstallsClaimed(Err(refused(-1))));
    assert!(app.installs.taking());

    // Paired by code: RomM will not hand this token any.
    let _ = app.update(Message::InstallsClaimed(Err(refused(403))));
    assert_eq!(app.installs, Installs::NotTied);
    assert!(!app.installs.taking(), "and is not asked again this run");

    // A RomM before 5.4, or with installs off.
    app.installs = Installs::Unknown;
    let _ = app.update(Message::InstallsClaimed(Err(refused(404))));
    assert_eq!(app.installs, Installs::Unavailable);
}

#[test]
fn the_downloads_page_opens_a_game_but_takes_delete_to_remove_one() {
    use crate::downloads::SyncState;
    let mut app = paired_fixtures(12);
    let game = app.library.all_games()[0].clone();
    app.downloads.finish(game.rom_id, Ok(installed(game.rom_id, &game.title, 1 << 20)));
    let _ = app.update(Message::TabSelected(Tab::Downloads));

    // Confirm opens the game, as it does from every other list in the app.
    let _ = app.update(Message::Activate);
    assert!(
        matches!(app.screen, Screen::Browsing(Route::Detail { rom_id, .. }) if rom_id == game.rom_id),
        "{}",
        app.screen_name()
    );

    // Delete is what removes it, and only on the page that says so.
    let _ = app.update(Message::TabSelected(Tab::Downloads));
    let _ = app.update(Message::RemoveFocusedDownload);
    assert_eq!(app.downloads.state(game.rom_id), SyncState::RemoteOnly);
    assert_eq!(app.downloads.installed_count(), 0);
    assert!(!app.shelves.iter().any(|s| s.id == "installed"), "and off the shelf with it");
}

/// A game on this device in a folder of its own, its page open, and RomM's word on what it holds:
/// the game, its update and a manual.
fn with_extras_on_romm(manual_here: bool) -> (App, Game) {
    use crate::romm::models::{RomDetail, RomFile};
    let mut app = paired_fixtures(12);
    let game = app.library.all_games()[0].clone();
    let mut record = installed(game.rom_id, &game.title, 1 << 20);
    record.files = vec!["Game/Game.sfc".into(), "Game/update/Game.ips".into(), "Game/update/noload.txt".into()];
    if manual_here {
        record.files.extend(["Game/manual/Game.pdf".to_string(), "Game/manual/noload.txt".to_string()]);
    }
    app.downloads.finish(game.rom_id, Ok(record));
    let _ = app.update(Message::OpenDetail(game.rom_id));
    let file = |id: i64, category: &str, name: &str| RomFile {
        id,
        file_name: name.into(),
        file_size_bytes: 10,
        file_path: if category == "game" { "roms/snes/Game".into() } else { format!("roms/snes/Game/{category}") },
        category: Some(category.into()),
        ..Default::default()
    };
    let detail = RomDetail {
        id: game.rom_id,
        fs_name: Some("Game".into()),
        fs_path: Some("roms/snes".into()),
        files: vec![file(1, "game", "Game.sfc"), file(2, "update", "Game.ips"), file(3, "manual", "Game.pdf")],
        ..Default::default()
    };
    let _ = app.update(Message::FilesLoaded(game.rom_id, Ok(Box::new(detail))));
    (app, game)
}

#[test]
fn a_game_s_extras_on_romm_get_a_button_that_comes_in_without_moving_the_cursor() {
    use crate::ui::detail::Action;
    let mut app = paired_fixtures(12);
    let game = app.library.all_games()[0].clone();
    let mut record = installed(game.rom_id, &game.title, 1 << 20);
    record.files = vec!["Game/Game.sfc".into()];
    app.downloads.finish(game.rom_id, Ok(record));
    let _ = app.update(Message::OpenDetail(game.rom_id));
    assert_eq!(app.detail_actions(), Action::shown(false), "nothing known of its extras yet");

    // On Favourite, past where Extras comes in, as RomM answers.
    let favourite = Action::shown(false).iter().position(|a| *a == Action::Favourite).unwrap();
    for _ in 0..favourite {
        let _ = app.update(Message::Nav(Direction::Right));
    }
    let (with, _) = with_extras_on_romm(false);
    let detail = with.rom_files.get(&game.rom_id).cloned().expect("the answer kept");
    let _ = app.update(Message::FilesLoaded(game.rom_id, Ok(Box::new((*detail).clone()))));
    assert!(app.detail_actions().contains(&Action::Extras));
    assert_eq!(app.detail_actions()[app.detail_focus.action], Action::Favourite, "the cursor stays on the action it was on");
}

#[test]
fn picking_an_extra_queues_it_alone_into_the_game_here() {
    let (mut app, game) = with_extras_on_romm(false);
    let _ = app.update(Message::DetailAction(crate::ui::detail::Action::Extras));
    let picker = &app.pick.as_ref().expect("the extras, to pick from").picker;
    let rows: Vec<(&str, &str, bool)> = picker.rows.iter().map(|r| (r.label.as_str(), r.detail.as_str(), r.mark.is_some())).collect();
    assert_eq!(rows, [("Manual", "Game.pdf", false)], "the update came with the game, so it is no extra");

    let _ = app.update(Message::Activate);
    let queued: Vec<QueueItem> = app.downloads.in_progress().map(|(item, _)| item.clone()).collect();
    assert_eq!(queued.len(), 1);
    assert_eq!((queued[0].rom_id, queued[0].files.clone(), queued[0].label.as_deref(), queued[0].extra), (game.rom_id, Some(vec![3]), Some("Manual"), true));
}

#[test]
fn an_extra_here_is_marked_and_taking_it_off_keeps_the_game() {
    let (mut app, game) = with_extras_on_romm(true);
    let _ = app.update(Message::DetailAction(crate::ui::detail::Action::Extras));
    let picker = &app.pick.as_ref().expect("the extras").picker;
    assert_eq!(picker.rows[0].mark.as_deref(), Some("on this device"));

    // What `downloads::shed` answers once the file is gone.
    let mut left = app.downloads.installed(game.rom_id).cloned().unwrap();
    left.files.retain(|f| !f.starts_with("Game/manual/"));
    let _ = app.update(Message::ExtraRemoved(game.rom_id, "Manual".into(), Ok(Box::new(left))));
    let record = app.downloads.installed(game.rom_id).expect("the game is still here");
    assert!(!record.files.iter().any(|f| f.contains("manual")));
    assert!(app.banner.as_deref().is_some_and(|b| b.contains("Manual is off this device")), "{:?}", app.banner);
}

#[test]
fn deleting_a_failed_row_of_a_game_here_dismisses_the_failure_and_keeps_the_game() {
    let (mut app, game) = with_extras_on_romm(false);
    app.downloads.enqueue(QueueItem {
        rom_id: game.rom_id,
        system_id: game.system_id.clone(),
        title: game.title.clone(),
        files: Some(vec![3]),
        label: Some("Manual".into()),
        extra: true,
        ..Default::default()
    });
    app.downloads.start_next();
    app.downloads.finish(game.rom_id, Err("the server went away".into()));
    let _ = app.update(Message::TabSelected(Tab::Downloads));
    assert!(matches!(app.download_rows()[0].state, crate::downloads::SyncState::Failed(_)), "the failure leads the page");

    let _ = app.update(Message::RemoveFocusedDownload);
    assert_eq!(app.downloads.failures().count(), 0, "the failure is dismissed");
    assert!(app.downloads.installed(game.rom_id).is_some(), "and the game it was for is still here");
}

#[test]
fn delete_outside_the_downloads_page_removes_nothing() {
    let mut app = paired_fixtures(12);
    let game = app.library.all_games()[0].clone();
    app.downloads.finish(game.rom_id, Ok(installed(game.rom_id, &game.title, 1 << 20)));

    // The key is bound app-wide, because one page owning a key binding is not something the
    // subscription can express — so the handler is what has to ignore it everywhere else.
    for tab in [Tab::Home, Tab::Browse, Tab::Collections, Tab::Settings] {
        let _ = app.update(Message::TabSelected(tab));
        let _ = app.update(Message::RemoveFocusedDownload);
    }
    assert_eq!(app.downloads.installed_count(), 1);
}

#[test]
fn a_failed_download_is_retried_by_confirming_it_and_dismissed_by_deleting_it() {
    use crate::downloads::SyncState;
    let mut app = paired_fixtures(12);
    let game = app.library.all_games()[0].clone();
    app.downloads.enqueue(QueueItem {
        host: String::new(),
        rom_id: game.rom_id,
        system_id: game.system_id.clone(),
        title: game.title.clone(),
        ..Default::default()
    });
    app.downloads.start_next();
    let _ = app.update(Message::DownloadEvent(crate::downloads::Event::Finished(
        game.rom_id,
        Err("the download stopped early".into()),
    )));
    assert!(matches!(app.downloads.state(game.rom_id), SyncState::Failed(_)));

    let _ = app.update(Message::TabSelected(Tab::Downloads));
    let _ = app.update(Message::Activate);
    assert!(
        matches!(app.downloads.state(game.rom_id), SyncState::Downloading(_)),
        "Confirm on a row that failed is the retry, not the detail page"
    );
    assert!(matches!(app.screen, Screen::Browsing(Route::Downloads)), "{}", app.screen_name());

    // Fail it again, then dismiss: the reason goes and the game is fetchable once more.
    let _ = app.update(Message::DownloadEvent(crate::downloads::Event::Finished(
        game.rom_id,
        Err("the download stopped early".into()),
    )));
    let _ = app.update(Message::RemoveFocusedDownload);
    assert_eq!(app.downloads.state(game.rom_id), SyncState::RemoteOnly);
}

#[test]
fn the_detail_pages_action_row_does_what_it_says_at_each_stage() {
    use crate::downloads::SyncState;
    use crate::ui::detail::Action;
    let mut app = paired_fixtures(12);
    let game = app.library.all_games()[0].clone();
    let _ = app.update(Message::OpenDetail(game.rom_id));

    // Not here yet: Download is live, and Remove has nothing to take.
    let _ = app.update(Message::DetailAction(Action::Remove));
    assert!(app.banner.as_deref().unwrap().contains("nothing to remove"), "{:?}", app.banner);
    let _ = app.update(Message::DetailAction(Action::Download));
    assert!(matches!(app.downloads.state(game.rom_id), SyncState::Downloading(_)));

    // Here: Download becomes Play, which starts it — once.
    let _ = app.update(Message::DownloadEvent(crate::downloads::Event::Finished(
        game.rom_id,
        Ok(Box::new(installed(game.rom_id, &game.title, 1 << 20))),
    )));
    let _ = app.update(Message::DetailAction(Action::Download));
    assert_eq!(app.playing, Some(game.rom_id));
    assert!(app.handover.is_some_and(|h| h.rom_id == game.rom_id && !h.closing), "the window says it is opening");
    let _ = app.view();
    let _ = app.update(Message::DetailAction(Action::Download));
    assert!(app.banner.as_deref().unwrap().contains("still running"), "{:?}", app.banner);
    // Keys pressed while it opens are for no page under it.
    let screen = app.screen_name();
    let _ = app.update(Message::Dismiss);
    let _ = app.update(Message::CycleTab(1));
    assert_eq!(app.screen_name(), screen);

    // It runs, then closes and its save goes up while the window says so; then it is over.
    let _ = app.update(Message::LaunchEvent(crate::launch::Event::Running(game.rom_id)));
    assert!(app.handover.is_none(), "the game has the screen");
    let _ = app.update(Message::LaunchEvent(crate::launch::Event::Saving(game.rom_id)));
    assert!(app.handover.is_some_and(|h| h.closing));
    let _ = app.view();

    // It ends, saying what went wrong and that its save went up.
    let exit = crate::launch::Exit { success: false, code: Some(1), last_line: Some("no BIOS".into()) };
    let session = crate::launch::Session {
        exit,
        played_at: Some("2999-01-01T00:00:00Z".into()),
        notes: vec!["saves synced".into()],
        report: None,
    };
    let _ = app.update(Message::LaunchEvent(crate::launch::Event::Ended(game.rom_id, Ok(session))));
    assert_eq!(app.playing, None);
    assert!(app.handover.is_none());
    let banner = app.banner.clone().unwrap();
    assert!(banner.contains("code 1: no BIOS") && banner.contains("saves synced"), "{banner}");
    assert_eq!(
        app.shelves.first().and_then(|s| s.games.first()).map(|g| g.rom_id),
        Some(game.rom_id),
        "the game just played leads Home"
    );

    let _ = app.update(Message::DetailAction(Action::Remove));
    assert_eq!(app.downloads.state(game.rom_id), SyncState::RemoteOnly);
}

/// Pages drawn headlessly to PNGs, for a person to look at — not asserted on, so ignored by
/// default. `DECKHAND_SNAPSHOTS=<dir> cargo test render_ -- --ignored` writes them there.
mod render {
    use super::*;
    use crate::game_saves::{GameSaves, SavesFocus};
    use crate::romm::models::SaveAsset;

    fn record(id: i64, name: &str, iso: &str, shot: bool, emulator: &str, bytes: i64) -> SaveAsset {
        let mut json = serde_json::json!({
            "id": id, "rom_id": 1, "file_name": name, "emulator": emulator, "updated_at": iso,
            "file_size_bytes": bytes, "slot": if name.ends_with(".srm") { "autosave" } else { "" },
        });
        if shot {
            json["screenshot"] = serde_json::json!({ "download_path": format!("/api/raw/assets/screenshots/{id}.png") });
        }
        serde_json::from_value(json).unwrap()
    }

    /// A stand-in screenshot: a diagonal wash in the given hue, with a band across it, so a
    /// thumbnail and its preview read as the same picture.
    fn picture(hue: [u8; 3]) -> iced::widget::image::Handle {
        picture_sized(320, 240, hue)
    }

    fn picture_sized(w: u32, h: u32, hue: [u8; 3]) -> iced::widget::image::Handle {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let t = (x + y) as f32 / (w + h) as f32;
                let band = (y as f32 / h as f32 - 0.6).abs() < 0.08;
                for c in hue {
                    let v = if band { 235.0 } else { c as f32 * (0.35 + 0.65 * t) };
                    rgba.push(v as u8);
                }
                rgba.push(255);
            }
        }
        iced::widget::image::Handle::from_rgba(w, h, rgba)
    }

    fn snapshot(app: &App, name: &str) {
        snapshot_sized(app, name, (1280.0, 800.0));
    }

    /// [`snapshot`] in a window of another size — a page the render cannot scroll, drawn tall
    /// enough to show all of it.
    fn snapshot_sized(app: &App, name: &str, size: (f32, f32)) {
        let dir = std::env::var("DECKHAND_SNAPSHOTS").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let path = dir.join(format!("{name}.png"));
        let fonts = crate::theme::FONT_FACES.iter().chain([&iced_fonts::LUCIDE_FONT_BYTES]).map(|f| std::borrow::Cow::Borrowed(*f)).collect();
        let settings = iced::Settings { fonts, default_font: crate::theme::REGULAR, ..Default::default() };
        let theme = app.theme();
        let mut ui = iced_test::Simulator::with_size(settings, size, app.view());
        // A picture over 2 MiB goes up to the GPU on a worker of iced's own and is drawn from the
        // frame after — the spotlight's, at the panel density these are drawn at. So the frame kept
        // is the second, as a window would show it once the worker asked for a redraw.
        let _ = ui.snapshot(&theme);
        std::thread::sleep(std::time::Duration::from_millis(250));
        let shot = ui.snapshot(&theme).expect("a headless frame");
        // Written fresh each time: `matches_image` only writes when there is no file yet.
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with(&format!("{name}-")) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        shot.matches_image(&path).expect("the PNG is written");
    }

    fn with_saves() -> (App, Game) {
        let mut app = paired_fixtures(30);
        let game = app.library.all_games()[3].clone();
        let states = vec![
            record(21, &format!("{}.slot2.retroarch.deckhand.state", game.title), "2026-09-30T21:14:00Z", true, "retroarch", 1_210_000),
            record(20, &format!("{}.auto.retroarch.deckhand.state", game.title), "2026-09-30T20:02:11Z", true, "retroarch", 1_208_000),
            record(19, &format!("{}.slot0.retroarch.deckhand.state", game.title), "2026-09-28T18:40:02Z", true, "retroarch", 1_207_000),
            record(18, &format!("{}.state", game.title), "2026-09-21T09:12:45Z", false, "snes9x", 1_201_000),
        ];
        let saves = vec![
            record(31, &format!("{} [2026-09-30_21-20-00].srm", game.title), "2026-09-30T21:20:00Z", false, "snes9x", 8_192),
            record(30, &format!("{} [2026-09-21_09-30-00].srm", game.title), "2026-09-21T09:30:00Z", false, "retroarch", 8_192),
        ];
        let host = app.host().to_string();
        for (record, hue) in states.iter().zip([[90, 140, 220], [220, 120, 70], [80, 190, 120], [0, 0, 0]]) {
            if let Some(url) = crate::ui::saves::screenshot_url(record, &host) {
                let meta = covers::CoverMeta { ratio: 4.0 / 3.0, dominant: Color::BLACK };
                app.covers.insert(url, covers::Cover::Ready(picture(hue), meta));
            }
        }
        let slots = crate::sync::states::layout(&states, &crate::sync::states::Core::default());
        let elsewhere = [(31, vec!["Living room PC".to_string()])].into_iter().collect();
        let data = GameSaves { states, saves, on_device: [20, 31].into_iter().collect(), elsewhere, slots, ..Default::default() };
        app.game_saves.insert(game.rom_id, Ok(Arc::new(data)));
        (app, game)
    }

    #[test]
    #[ignore = "draws pages to PNGs for a person to look at"]
    fn render_saves_page() {
        let (mut app, game) = with_saves();
        let detail = Route::Detail { rom_id: game.rom_id, from: Box::new(Route::Home) };
        app.screen = Screen::Browsing(Route::Saves { rom_id: game.rom_id, from: Box::new(detail) });
        app.saves_focus = SavesFocus::opening(None);
        let _ = app.update(Message::Nav(Direction::Down));
        snapshot(&app, "saves-states");
        let _ = app.update(Message::Nav(Direction::Right));
        snapshot(&app, "saves-saves");
    }

    #[test]
    #[ignore = "draws pages to PNGs for a person to look at"]
    fn render_collections() {
        let (mut app, ids) = with_collections(30);
        // Stand-in covers, one hue per game, so the cards show what they would.
        let mut games: Vec<Game> = app.library.all_games().to_vec();
        for (i, game) in games.iter_mut().enumerate() {
            game.cover_url = Some(format!("http://127.0.0.1:1/cover/{i}.png"));
            game.rating = Some((i % 5) as f32);
        }
        let hues = [[90, 140, 220], [220, 120, 70], [80, 190, 120], [190, 90, 200], [220, 200, 80]];
        for (i, game) in games.iter().enumerate() {
            let meta = covers::CoverMeta { ratio: 0.71, dominant: Color::BLACK };
            app.covers.insert(game.cover_url.clone().unwrap(), covers::Cover::Ready(picture(hues[i % hues.len()]), meta));
        }
        app.library = Arc::new(Library::new(app.library.systems.clone(), games));
        app.rebuild_collections();
        let _ = ids;
        let _ = app.update(Message::TabSelected(Tab::Collections));
        let _ = app.update(Message::Nav(Direction::Up));
        let _ = app.update(Message::Nav(Direction::Right));
        let _ = app.update(Message::Nav(Direction::Down));
        snapshot(&app, "collections");
        let _ = app.update(Message::Activate);
        snapshot(&app, "collection-grid");
    }

    /// A collection mixing two systems' cover shapes — PSP's tall boxes and PS1's wide jewel
    /// cases — which is the grid where a cover of the other shape has to stay inside its tile.
    #[test]
    #[ignore = "draws pages to PNGs for a person to look at"]
    fn render_mixed_grid() {
        use crate::romm::models::VirtualCollection;
        let mut app = paired_fixtures(60);
        let mut games: Vec<Game> = app.library.all_games().to_vec();
        let pick = |system: &str, n: usize, games: &[Game]| -> Vec<i64> {
            games.iter().filter(|g| g.system_id == system).take(n).map(|g| g.rom_id).collect()
        };
        let ids: Vec<i64> = pick("psp", 3, &games).into_iter().chain(pick("psx", 2, &games)).collect();
        let hues = [[90, 140, 220], [220, 120, 70], [80, 190, 120], [190, 90, 200], [220, 200, 80]];
        for (i, id) in ids.iter().enumerate() {
            let game = games.iter_mut().find(|g| g.rom_id == *id).unwrap();
            let url = format!("http://127.0.0.1:1/cover/{id}.png");
            game.cover_url = Some(url.clone());
            let (w, h, ratio) = if game.system_id == "psx" { (464, 400, 1.16) } else { (268, 400, 0.67) };
            let meta = covers::CoverMeta { ratio, dominant: Color::BLACK };
            app.covers.insert(url, covers::Cover::Ready(picture_sized(w, h, hues[i % hues.len()]), meta));
        }
        app.library = Arc::new(Library::new(app.library.systems.clone(), games));
        app.collection_sources = Some(Ok((
            Vec::new(),
            vec![VirtualCollection { id: "f-1".into(), name: Some("Ace Combat".into()), kind: "franchise".into(), rom_ids: ids }],
        )));
        app.rebuild_collections();
        let _ = app.update(Message::OpenCollection("virtual:f-1".into()));
        snapshot(&app, "mixed-grid");
    }

    /// Home's spotlight over a stand-in screenshot — the picture faded into the page, the light it
    /// casts, and the shelves under it — then the same with the cursor up on its buttons, and a
    /// game with no screenshot, whose cover stands in.
    #[test]
    #[ignore = "draws pages to PNGs for a person to look at"]
    fn render_home_spotlight() {
        let mut app = paired_fixtures(30);
        let shot = format!("{}/assets/romm/resources/roms/1/1/screenshots/0.jpg", app.host());
        let mut games = app.library.all_games().to_vec();
        let first = app.shelves[0].games[0].rom_id;
        let second = app.shelves[0].games[1].rom_id;
        for game in &mut games {
            if game.rom_id == first {
                game.screenshots = vec![shot.clone()];
                game.achievements = 40;
            }
            if game.rom_id == second {
                game.cover_url = Some(format!("{}/assets/cover.png", app.host()));
            }
        }
        app.library = Arc::new(Library::new(app.library.systems.clone(), games));
        app.rebuild_shelves();
        let bytes = {
            let iced::widget::image::Handle::Rgba { width, height, pixels, .. } = picture_sized(320, 240, [70, 130, 210]) else { unreachable!() };
            let mut png = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_raw(width, height, pixels.to_vec()).unwrap().write_to(&mut png, image::ImageFormat::Png).unwrap();
            png.into_inner()
        };
        for (kind, target) in [
            (covers::Kind::Spotlight, covers::Target { height_px: crate::ui::home::PICTURE.1, box_ratio: Some(crate::ui::home::PICTURE.0 / crate::ui::home::PICTURE.1) }),
            (covers::Kind::Ambient, covers::Target::tall(40.0)),
        ] {
            let (handle, meta) = covers::prepare(&bytes, target, kind).unwrap();
            app.covers.insert(covers::key(&shot, kind), covers::Cover::Ready(handle, meta));
        }
        let cover = format!("{}/assets/cover.png", app.host());
        for (kind, target) in [(covers::Kind::Feature, covers::Target::tall(crate::ui::home::CARD_H)), (covers::Kind::Ambient, covers::Target::tall(40.0)), (covers::Kind::Art, covers::Target::tall(crate::ui::home::COVER_H))] {
            let art = picture_sized(300, 420, [200, 90, 60]);
            let iced::widget::image::Handle::Rgba { width, height, pixels, .. } = art else { unreachable!() };
            let mut png = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_raw(width, height, pixels.to_vec()).unwrap().write_to(&mut png, image::ImageFormat::Png).unwrap();
            let (handle, meta) = covers::prepare(png.get_ref(), target, kind).unwrap();
            app.covers.insert(covers::key(&cover, kind), covers::Cover::Ready(handle, meta));
        }
        app.backdrop = Default::default();
        app.follow_spotlight();
        app.backdrop = settled(app.backdrop);
        app.screen = Screen::Browsing(Route::Home);
        snapshot(&app, "home-spotlight");

        let _ = app.update(Message::Nav(Direction::Up));
        app.fade = Default::default();
        snapshot(&app, "home-buttons");

        let _ = app.update(Message::Nav(Direction::Down));
        let _ = app.update(Message::Nav(Direction::Right));
        app.fade = Default::default();
        app.backdrop = settled(std::mem::take(&mut app.backdrop));
        snapshot(&app, "home-cover");

        // A desktop window too short for all of it: the description and the facts make way.
        let _ = app.update(Message::Nav(Direction::Left));
        app.fade = Default::default();
        app.backdrop = settled(std::mem::take(&mut app.backdrop));
        app.content_height = 600.0;
        app.banner = None;
        snapshot_sized(&app, "home-compact", (1280.0, 600.0));
    }

    /// The Library in Home's look: the spotlight over a stand-in screenshot, the system list on its
    /// panel, the pills and the grid — then with the cursor in the list, and up on the pills.
    #[test]
    #[ignore = "draws pages to PNGs for a person to look at"]
    fn render_library() {
        let mut app = paired_fixtures(90);
        app.banner = None;
        let host = app.host().to_string();
        let mut games = app.library.all_games().to_vec();
        let hues = [[90, 140, 220], [220, 120, 70], [80, 190, 120], [190, 90, 200], [220, 200, 80]];
        for (i, game) in games.iter_mut().enumerate() {
            let url = format!("{host}/cover/{}.png", game.rom_id);
            let ratio = app.library.cover_ratio(&game.system_id);
            let meta = covers::CoverMeta { ratio, dominant: Color::BLACK };
            app.covers.insert(url.clone(), covers::Cover::Ready(picture_sized((300.0 * ratio) as u32, 300, hues[i % hues.len()]), meta));
            game.cover_url = Some(url);
            game.screenshots = vec![format!("{host}/shot/{}.jpg", game.rom_id)];
            game.achievements = 40;
        }
        app.library = Arc::new(Library::new(app.library.systems.clone(), games));
        let _ = app.update(Message::OpenSystem("snes".into()));
        let shot = app.library_spotlit()[0].screenshots[0].clone();
        let bytes = {
            let iced::widget::image::Handle::Rgba { width, height, pixels, .. } = picture_sized(320, 240, [70, 130, 210]) else { unreachable!() };
            let mut png = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_raw(width, height, pixels.to_vec()).unwrap().write_to(&mut png, image::ImageFormat::Png).unwrap();
            png.into_inner()
        };
        for (kind, target) in [
            (covers::Kind::Spotlight, covers::Target { height_px: crate::ui::home::PICTURE.1, box_ratio: Some(crate::ui::home::PICTURE.0 / crate::ui::home::PICTURE.1) }),
            (covers::Kind::Ambient, covers::Target::tall(40.0)),
        ] {
            let (handle, meta) = covers::prepare(&bytes, target, kind).unwrap();
            app.covers.insert(covers::key(&shot, kind), covers::Cover::Ready(handle, meta));
        }
        app.backdrop = Default::default();
        app.follow_spotlight();
        app.backdrop = settled(std::mem::take(&mut app.backdrop));
        app.fade = Default::default();
        snapshot(&app, "library");

        let _ = app.update(Message::Nav(Direction::Left));
        let _ = app.update(Message::Nav(Direction::Down));
        app.fade = Default::default();
        snapshot(&app, "library-list");

        let _ = app.update(Message::Nav(Direction::Up));
        let _ = app.update(Message::Nav(Direction::Right));
        let _ = app.update(Message::Nav(Direction::Up));
        let _ = app.update(Message::Nav(Direction::Right));
        app.fade = Default::default();
        snapshot(&app, "library-chips");
    }

    /// The header's right with a download coming and the saves in each state, and on the Downloads
    /// page, whose icon is then filled in.
    #[test]
    #[ignore = "draws pages to PNGs for a person to look at"]
    fn render_header_icons() {
        let mut app = paired_fixtures(30);
        app.banner = None;
        let game = app.library.all_games()[0].clone();
        app.downloads.enqueue(QueueItem { rom_id: game.rom_id, title: game.title.clone(), system_id: game.system_id.clone(), ..Default::default() });
        let started = app.downloads.start_next().expect("a download to start");
        app.downloads.progress(started.rom_id, 0.4);
        app.screen = Screen::Browsing(Route::Home);
        for (name, sync) in [
            ("header-quiet", SaveSync::Idle),
            ("header-syncing", SaveSync::Running),
            ("header-in-sync", SaveSync::Done(Box::default())),
            ("header-failed", SaveSync::Failed("no RetroArch".into())),
        ] {
            app.save_sync = sync;
            snapshot(&app, name);
        }
        let _ = app.update(Message::TabSelected(Tab::Downloads));
        snapshot(&app, "header-downloads-page");
    }

    /// A backdrop as it is once its hand-off is over — a render is one frame, and the frame wanted
    /// is the one after the fade.
    fn settled(mut backdrop: crate::animation::Backdrop) -> crate::animation::Backdrop {
        let shown = backdrop.layers(std::time::Instant::now() + std::time::Duration::from_secs(60)).last().map(|(id, _)| id);
        let mut fresh = crate::animation::Backdrop::default();
        if let Some(id) = shown {
            fresh.show(id, std::time::Instant::now() - std::time::Duration::from_secs(60));
        }
        backdrop = fresh;
        backdrop
    }

    #[test]
    #[ignore = "draws pages to PNGs for a person to look at"]
    fn render_detail_page() {
        let (mut app, game) = with_saves();
        app.screen = Screen::Browsing(Route::Detail { rom_id: game.rom_id, from: Box::new(Route::Home) });
        snapshot(&app, "detail");
        app.favourites.insert(game.rom_id);
        snapshot(&app, "detail-favourited");
    }

    /// The game page with a wide box's cover, a row of screenshots and the achievements' heading;
    /// then the cursor on a screenshot, the list opened, and a screenshot open over the window.
    #[test]
    #[ignore = "draws pages to PNGs for a person to look at"]
    fn render_detail_rows() {
        use crate::romm::models::Achievement;
        let (mut app, game) = with_saves();
        let host = app.host().to_string();
        let cover = format!("{host}/assets/wide-cover.png");
        let shots: Vec<String> = (0..5).map(|i| format!("{host}/assets/screenshots/{i}.png")).collect();
        let mut games = app.library.all_games().to_vec();
        for each in &mut games {
            if each.rom_id == game.rom_id {
                each.cover_url = Some(cover.clone());
                each.screenshots = shots.clone();
                each.achievements = 3;
            }
        }
        app.library = Arc::new(Library::new(app.library.systems.clone(), games));
        let meta = |ratio: f32| covers::CoverMeta { ratio, dominant: Color::BLACK };
        app.covers.insert(cover, covers::Cover::Ready(picture_sized(548, 400, [200, 60, 50]), meta(1.37)));
        let hues = [[90, 140, 220], [220, 120, 70], [80, 190, 120], [200, 180, 60], [150, 90, 200]];
        for (i, (url, hue)) in shots.iter().zip(hues).enumerate() {
            app.covers.insert(url.clone(), covers::Cover::Ready(picture_sized(256, 224, hue), meta(256.0 / 224.0)));
            if i == 1 {
                app.covers.insert(covers::key(url, covers::Kind::Full), covers::Cover::Ready(picture_sized(768, 672, hue), meta(256.0 / 224.0)));
            }
        }
        let _ = app.update(Message::OpenDetail(game.rom_id));
        app.banner = None;
        // Its first screenshot behind the page, as Home's spotlight draws it.
        let png = {
            let iced::widget::image::Handle::Rgba { width, height, pixels, .. } = picture_sized(512, 448, [90, 140, 220]) else { unreachable!() };
            let mut png = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_raw(width, height, pixels.to_vec()).unwrap().write_to(&mut png, image::ImageFormat::Png).unwrap();
            png.into_inner()
        };
        let (w, h) = crate::ui::home::PICTURE;
        for (kind, target) in [
            (covers::Kind::Spotlight, covers::Target { height_px: (crate::ui::detail::PICTURE_ENDS + 70.0).min(h), box_ratio: Some(w / h) }),
            (covers::Kind::Ambient, covers::Target::tall(40.0)),
        ] {
            let (handle, meta) = covers::prepare(&png, target, kind).unwrap();
            app.covers.insert(covers::key(&shots[0], kind), covers::Cover::Ready(handle, meta));
        }
        app.follow_spotlight();
        app.backdrop = settled(std::mem::take(&mut app.backdrop));
        let list = vec![
            Achievement { title: "First steps".into(), description: Some("Clear the opening area".into()), points: Some(5), num_awarded: Some(19_293), ..Default::default() },
            Achievement { title: "Collector".into(), description: Some("Find every hidden item".into()), points: Some(25), num_awarded: Some(1_204), ..Default::default() },
            Achievement { title: "No damage".into(), description: Some("Beat the last boss without a hit".into()), points: Some(50), num_awarded: Some(87), ..Default::default() },
        ];
        app.achievements.insert(game.rom_id, Ok(Arc::new(crate::achievements::GameAchievements::new(list, None, None))));
        snapshot(&app, "detail-wide");
        let _ = app.update(Message::Nav(Direction::Down));
        let _ = app.update(Message::Nav(Direction::Right));
        snapshot(&app, "detail-shots");
        let _ = app.update(Message::Nav(Direction::Down));
        let _ = app.update(Message::Activate);
        snapshot_sized(&app, "detail-achievements", (1280.0, 1300.0));
        let _ = app.update(Message::ViewShot(1));
        snapshot(&app, "detail-viewer");
    }

    /// What Download offers for a three-disc game with another build — and the drive picker.
    #[test]
    #[ignore = "draws pages to PNGs for a person to look at"]
    fn render_download_pickers() {
        use crate::romm::models::{RomDetail, RomFile};
        let mut app = paired_fixtures(30);
        let mut games: Vec<Game> = app.library.all_games().to_vec();
        let tile = games[1].rom_id;
        games[1].sibling_ids = vec![9001];
        games[1].file_stems = vec![(tile, "Final Fantasy VII (USA)".into()), (9001, "Final Fantasy VII (Europe) (Rev 1)".into())];
        app.library = Arc::new(Library::new(app.library.systems.clone(), games));
        let disc = |id: i64, n: u32| RomFile {
            id,
            file_name: format!("Final Fantasy VII (USA) (Disc {n}).chd"),
            file_size_bytes: 700 * 1024 * 1024,
            file_path: "roms/psx".into(),
            category: Some("game".into()),
            ..Default::default()
        };
        let detail = RomDetail { id: tile, files: vec![disc(1, 1), disc(2, 2), disc(3, 3)], ..Default::default() };
        let _ = app.update(Message::OpenDetail(tile));
        let _ = app.offer_choices(tile, Ok(Box::new(detail)));
        snapshot(&app, "download-picker");
        app.pick = None;
        let _ = app.update(Message::TabSelected(Tab::Settings));
        let _ = app.pick_location();
        snapshot(&app, "location-picker");
    }

    /// A game on this device with an update waiting on RomM, pinned — and the Storage settings.
    #[test]
    #[ignore = "draws pages to PNGs for a person to look at"]
    fn render_storage() {
        use crate::library::Fingerprint;
        let mut app = paired_fixtures(30);
        let mut games: Vec<Game> = app.library.all_games().to_vec();
        games[2].fingerprint = Some(Fingerprint { bytes: 5, md5: Some("new".into()) });
        let rom_id = games[2].rom_id;
        app.library = Arc::new(Library::new(app.library.systems.clone(), games));
        app.downloads.enqueue(QueueItem { rom_id, title: "Game".into(), system_id: "gba".into(), ..Default::default() });
        app.downloads.start_next();
        let mut record = installed(rom_id, &app.title_of(rom_id), 3 * 1024 * 1024 * 1024);
        record.fingerprint = Some(Fingerprint { bytes: 5, md5: Some("old".into()) });
        record.pinned = true;
        app.downloads.finish(rom_id, Ok(record));
        app.downloads.check_updates(&app.library);
        app.settings.storage_budget_gb = Some(64);
        app.screen = Screen::Browsing(Route::Detail { rom_id, from: Box::new(Route::Home) });
        snapshot(&app, "detail-update");
        let _ = app.update(Message::TabSelected(Tab::Downloads));
        snapshot(&app, "downloads-notes");
        let _ = app.update(Message::TabSelected(Tab::Settings));
        for _ in 0..3 {
            let _ = app.update(Message::CycleSystem(1));
        }
        snapshot_sized(&app, "settings-storage", (1280.0, 1800.0));
    }

    /// Real images, from a folder `DECKHAND_ICONS` names: `<slug>.ico` platform icons as RomM
    /// serves them and a `badge.png`, decoded the way the art pipeline decodes them.
    fn put(app: &mut App, url: String, file: &std::path::Path, height: f32) {
        let bytes = std::fs::read(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let (handle, meta) = crate::covers::prepare(&bytes, crate::covers::Target::tall(height * 2.0), crate::covers::Kind::Art)
            .unwrap_or_else(|| panic!("{} did not decode", file.display()));
        app.covers.insert(url, Cover::Ready(handle, meta));
    }

    #[test]
    #[ignore = "writes PNGs; run explicitly"]
    fn render_login() {
        let (mut app, _) = App::new(Some(0));
        let mut state = LoginState::new("https://romm.example.com".into());
        state.focus = crate::ui::login::Row::Qr;
        app.screen = Screen::Login(state.clone());
        snapshot_sized(&app, "login", (1280.0, 900.0));
        state.qr = Some(Box::new(crate::qr_pairing::start_for_test("https://romm.example.com")));
        app.screen = Screen::Login(state);
        snapshot_sized(&app, "login-qr", (1280.0, 900.0));
    }

    #[test]
    #[ignore = "writes PNGs; run explicitly"]
    fn render_picks() {
        use crate::library::PlayStatus;
        use crate::romm::models::Achievement;
        let icons = std::path::PathBuf::from(std::env::var("DECKHAND_ICONS").expect("DECKHAND_ICONS"));
        let mut app = paired_fixtures(30);
        app.banner = None;
        for system in app.library.systems.clone() {
            let url = app.platform_logo(&system.id);
            put(&mut app, url, &icons.join(format!("{}.ico", system.id)), crate::ui::browse::LOGO_H);
        }
        let mut games: Vec<Game> = app.library.all_games().to_vec();
        games[0].now_playing = true;
        games[1].backlogged = true;
        games[2].backlogged = true;
        games[3].status = Some(PlayStatus::Finished);
        games[3].build = Some("USA · Rev 1".into());
        games[3].achievements = 3;
        let rom_id = games[3].rom_id;
        app.library = Arc::new(Library::new(app.library.systems.clone(), games));
        app.rebuild_shelves();
        snapshot(&app, "picks-home");

        let _ = app.update(Message::OpenSystem("snes".into()));
        snapshot(&app, "picks-browse");

        let badge = format!("{}/assets/romm/resources/roms/1/37/retroachievements/badges/60274.png", app.host());
        let list: Vec<Achievement> = ["Don't Spend It All in One Place", "The Fair", "End of Time"]
            .iter()
            .enumerate()
            .map(|(i, title)| Achievement {
                title: (*title).into(),
                description: Some("Get your allowance from Mom".into()),
                points: Some(2 + i as i64 * 5),
                num_awarded: Some(19_293 / (i as i64 + 1)),
                badge_path: Some("/assets/romm/resources/roms/1/37/retroachievements/badges/60274.png".into()),
                ..Default::default()
            })
            .collect();
        put(&mut app, badge, &icons.join("badge.png"), crate::ui::detail::BADGE);
        let _ = app.update(Message::OpenDetail(rom_id));
        let list: Vec<Achievement> = list.into_iter().enumerate().map(|(i, a)| Achievement { badge_id: Some(i.to_string()), ..a }).collect();
        let user: crate::romm::models::UserInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "ra_username": "player",
            "ra_progression": { "results": [{ "rom_ra_id": 319, "earned_achievements": [
                { "id": "0", "date": "2026-08-06 19:19:21" }, { "id": "1", "date": "2026-08-29 21:58:49", "date_hardcore": "2026-08-29 21:58:49" }
            ] }] }
        }))
        .unwrap();
        let game = crate::achievements::GameAchievements::new(list, Some(&user), Some(319));
        let _ = app.update(Message::AchievementsLoaded(rom_id, Ok(Arc::new(game))));
        snapshot_sized(&app, "picks-detail", (1280.0, 1400.0));
    }

    #[test]
    #[ignore = "writes PNGs; run explicitly"]
    fn render_tabs_and_rows() {
        let mut app = paired_fixtures(30);
        app.banner = None;
        snapshot(&app, "tweak-home");
        let games: Vec<Game> = app.library.all_games().to_vec();
        let half = games.iter().find(|g| g.beat_label().contains('½')).expect("a fixture with a half hour");
        app.screen = Screen::Browsing(Route::Detail { rom_id: half.rom_id, from: Box::new(Route::Home) });
        snapshot(&app, "tweak-detail");
        let _ = app.update(Message::TabSelected(Tab::Search));
        let word: String = games[0].title.split_whitespace().next().unwrap().chars().filter(|c| c.is_alphanumeric()).collect();
        let _ = app.update(Message::KeyChar(word[..2.min(word.len())].to_string(), false));
        snapshot(&app, "tweak-search");
        // One downloading, one failed, one here.
        for game in &games[..3] {
            app.downloads.enqueue(QueueItem { rom_id: game.rom_id, title: game.title.clone(), system_id: game.system_id.clone(), ..Default::default() });
        }
        app.downloads.start_next();
        app.downloads.progress(games[0].rom_id, 0.18);
        app.downloads.start_next();
        app.downloads.finish(games[1].rom_id, Err("the server answered 500".into()));
        app.downloads.start_next();
        let record = installed(games[2].rom_id, &games[2].title, 700 * 1024 * 1024);
        app.downloads.finish(games[2].rom_id, Ok(record));
        let _ = app.update(Message::TabSelected(Tab::Downloads));
        snapshot(&app, "tweak-downloads");
        let _ = app.update(Message::TabSelected(Tab::Settings));
        snapshot(&app, "tweak-settings");
    }

    /// The README's screenshots, of a real library: `DECKHAND_SHOT_CACHE=~/.cache/deckhand-rust`
    /// names a Deckhand cache directory, whose `library.json` and `covers/` are read and never
    /// written. Nothing is fetched: art not in that cache is drawn as it would be offline.
    #[test]
    #[ignore = "reads the cache named in DECKHAND_SHOT_CACHE; writes PNGs"]
    fn render_readme() {
        let Some(dir) = std::env::var_os("DECKHAND_SHOT_CACHE").map(std::path::PathBuf::from) else { return };
        let text = std::fs::read_to_string(dir.join("library.json")).expect("a cached library");
        let host = serde_json::from_str::<serde_json::Value>(&text).unwrap()["host"].as_str().unwrap().to_string();
        let cached: crate::cache::Cached = serde_json::from_str(&text).expect("a cache this build reads");
        let mut app = paired_fixtures(1);
        // Only so asset paths resolve to the URLs the cache holds them under.
        app.settings.connection.as_mut().unwrap().host = host;
        app.panel_density = 2.0;
        app.banner = None;
        app.library = Arc::new(cached.library());
        app.favourites = favourites_of(&cached.collections);
        app.collections = cached.collections;
        app.rebuild_collections();
        app.rebuild_shelves();
        // Lived in: the last game played and the rest of Continue playing are on this device.
        let here: Vec<Game> = app.shelves.first().map(|s| &s.games[..s.games.len().min(8)]).unwrap_or_default().to_vec();
        app.downloads.adopt(here.iter().map(|g| installed(g.rom_id, &g.title, 700 * 1024 * 1024)).collect());
        app.rebuild_shelves();
        app.screen = Screen::Browsing(Route::Home);
        let hydrate = |app: &mut App| {
            for (url, target, kind) in app.wanted_art() {
                // Whichever of RomM's two copies the cache holds: it was filled at the panel's
                // own density, which may have wanted the small one.
                let sources = [covers::source_url(&url, target), url.clone(), covers::source_url(&url, covers::Target::tall(1.0))];
                let Some(bytes) = sources.iter().find_map(|source| std::fs::read(dir.join("covers").join(covers::cache_name(source))).ok()) else {
                    continue;
                };
                if let Some((handle, meta)) = covers::prepare(&bytes, target, kind) {
                    app.covers.insert(covers::key(&url, kind), covers::Cover::Ready(handle, meta));
                }
            }
        };
        hydrate(&mut app);
        app.follow_spotlight();
        app.backdrop = settled(std::mem::take(&mut app.backdrop));
        snapshot(&app, "readme-home");
        for system in ["snes", "ps2", "psx"] {
            let _ = app.update(Message::OpenSystem(system.into()));
            let _ = app.update(Message::Nav(Direction::Down));
            app.fade = Default::default();
            hydrate(&mut app);
            snapshot(&app, &format!("readme-browse-{system}"));
        }
        if let Some(first) = app.shelves.first().and_then(|s| s.games.first()).map(|g| g.rom_id) {
            let _ = app.update(Message::OpenDetail(first));
            hydrate(&mut app);
            snapshot(&app, "readme-detail");
        }
    }

    #[test]
    #[ignore = "writes PNGs; run explicitly"]
    fn render_handover() {
        let mut app = paired_fixtures(12);
        let mut games: Vec<Game> = app.library.all_games().to_vec();
        let url = format!("http://127.0.0.1:1/cover/{}.png", games[0].rom_id);
        games[0].cover_url = Some(url.clone());
        app.covers.insert(url, covers::Cover::Ready(picture_sized(288, 400, [90, 140, 220]), covers::CoverMeta { ratio: 0.72, dominant: Color::BLACK }));
        app.library = Arc::new(Library::new(app.library.systems.clone(), games.clone()));
        let since = std::time::Instant::now() - std::time::Duration::from_millis(500);
        app.handover = Some(Handover { rom_id: games[0].rom_id, closing: false, since });
        snapshot(&app, "handover-opening");
        app.handover = Some(Handover { rom_id: games[0].rom_id, closing: true, since });
        snapshot(&app, "handover-saving");
        // A game with no cover on hand, its name long enough to wrap.
        let mut long = games[1].clone();
        long.title = "The Legend of Zelda: Ocarina of Time — Master Quest (Europe) (En,Fr,De)".into();
        games[1] = long;
        app.library = Arc::new(Library::new(app.library.systems.clone(), games.clone()));
        app.handover = Some(Handover { rom_id: games[1].rom_id, closing: false, since });
        snapshot(&app, "handover-no-cover");
    }

    #[test]
    #[ignore = "writes PNGs; run explicitly"]
    fn render_collections_favourites() {
        let (mut app, ids) = super::with_collections(30);
        let mut games: Vec<Game> = app.library.all_games().to_vec();
        let hues = [[90, 140, 220], [220, 120, 70], [80, 190, 120], [190, 90, 200], [220, 200, 80]];
        for (i, game) in games.iter_mut().enumerate() {
            let url = format!("http://127.0.0.1:1/cover/{}.png", game.rom_id);
            game.cover_url = Some(url.clone());
            app.covers.insert(url, covers::Cover::Ready(picture_sized(288, 400, hues[i % hues.len()]), covers::CoverMeta { ratio: 0.72, dominant: Color::BLACK }));
        }
        app.library = Arc::new(Library::new(app.library.systems.clone(), games));
        app.favourites.extend([ids[2], ids[7], ids[13], ids[21], ids[25], ids[28]]);
        app.rebuild_collections();
        // As a start with favourites opens it: on the first chip.
        app.collections_focus = Default::default();
        let _ = app.update(Message::TabSelected(Tab::Collections));
        snapshot(&app, "collections-favourites");
        let _ = app.update(Message::Nav(Direction::Up));
        let _ = app.update(Message::Nav(Direction::Right));
        let _ = app.update(Message::Nav(Direction::Down));
        snapshot(&app, "collections-yours");
    }

    #[test]
    #[ignore = "writes PNGs; run explicitly"]
    fn render_pad_focus() {
        use crate::gamepad::Pad;
        let mut app = paired_fixtures(30);
        // Covers drawn to the edge, as real ones are: the cursor's ring has to show over them.
        let mut games: Vec<Game> = app.library.all_games().to_vec();
        let hues = [[90, 140, 220], [220, 120, 70], [80, 190, 120], [190, 90, 200], [220, 200, 80]];
        for (i, game) in games.iter_mut().enumerate() {
            let url = format!("http://127.0.0.1:1/cover/{}.png", game.rom_id);
            game.cover_url = Some(url.clone());
            let meta = covers::CoverMeta { ratio: 0.72, dominant: Color::BLACK };
            app.covers.insert(url, covers::Cover::Ready(picture_sized(288, 400, hues[i % hues.len()]), meta));
        }
        app.library = Arc::new(Library::new(app.library.systems.clone(), games.clone()));
        app.rebuild_shelves();
        for game in &games[..2] {
            app.downloads.enqueue(QueueItem { rom_id: game.rom_id, title: game.title.clone(), system_id: game.system_id.clone(), ..Default::default() });
        }
        app.downloads.start_next();
        app.downloads.progress(games[0].rom_id, 0.4);
        app.banner = Some("Found 3 games already in the downloads folder: they're marked as on this device.".into());
        let pad = |app: &mut App, pad: Pad| {
            let _ = app.update(Message::Pad(pad));
        };
        for _ in 0..12 {
            pad(&mut app, Pad::Nav(Direction::Up));
        }
        snapshot(&app, "focus-banner");
        pad(&mut app, Pad::Nav(Direction::Down));
        snapshot(&app, "focus-tabs");
        app.banner = None;
        for (tab, name) in [(Tab::Home, "home"), (Tab::Browse, "browse"), (Tab::Collections, "collections"), (Tab::Search, "search"), (Tab::Downloads, "downloads"), (Tab::Settings, "settings")] {
            let _ = app.update(Message::TabSelected(tab));
            pad(&mut app, Pad::Nav(Direction::Down));
            snapshot(&app, &format!("focus-{name}"));
            if tab == Tab::Browse {
                // Up to the grid's first row and on: the chips, on the one that is chosen.
                while app.browse_focus.zone() != crate::focus::BrowseZone::Chips {
                    pad(&mut app, Pad::Nav(Direction::Up));
                }
                snapshot(&app, "focus-browse-chip");
            }
        }
        let _ = app.update(Message::OpenDetail(games[3].rom_id));
        snapshot(&app, "focus-detail");
        let _ = app.update(Message::Nav(Direction::Right));
        snapshot(&app, "focus-detail-second");
        let _ = app.update(Message::TabSelected(Tab::Home));
        for _ in 0..3 {
            pad(&mut app, Pad::Nav(Direction::Up));
        }
        pad(&mut app, Pad::Nav(Direction::Down));
        pad(&mut app, Pad::Nav(Direction::Down));
        app.fade = Default::default();
        snapshot(&app, "focus-home-shelf");
    }

    #[test]
    #[ignore = "writes PNGs; run explicitly"]
    fn render_system_emulators() {
        let mut app = paired_fixtures(30);
        app.banner = None;
        app.settings.system_commands.insert(
            "psp".into(),
            ["/var/lib/flatpak/exports/bin/org.ppsspp.PPSSPP", "{rom}"].map(String::from).to_vec(),
        );
        let _ = app.update(Message::TabSelected(Tab::Settings));
        let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::SystemEmulators)));
        app.pick = None;
        snapshot_sized(&app, "emulators-row", (1280.0, 1400.0));
        let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::SystemEmulators)));
        snapshot(&app, "emulators-systems");
        app.retrodeck = false;
        let _ = app.update(Message::Dismiss);
        let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::SystemEmulators)));
        snapshot(&app, "emulators-systems-none");
        app.retrodeck = true;
        let _ = app.update(Message::Dismiss);
        let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::SystemEmulators)));
        let _ = app.update(Message::Picked(1));
        snapshot(&app, "emulators-system");
        app.pick = None;
        let _ = app.picked_emulator("psx".into(), std::path::Path::new("/home/deck/Applications/duckstation-qt-x64.AppImage"));
        snapshot(&app, "emulators-entry");
        app.entry = None;
        let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Launcher)));
        snapshot(&app, "emulators-default");
    }

    #[test]
    #[ignore = "writes PNGs; run explicitly"]
    fn render_settings_editing() {
        let mut app = paired_fixtures(30);
        app.settings.launch_command = Some(["retroarch", "-L", "/usr/lib/libretro/snes9x_libretro.so", "{rom}"].map(String::from).to_vec());
        let _ = app.update(Message::TabSelected(Tab::Settings));
        let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Launcher)));
        let _ = app.update(Message::Picked(1));
        let _ = app.update(Message::Pad(crate::gamepad::Pad::Nav(Direction::Up)));
        let _ = app.update(Message::Pad(crate::gamepad::Pad::Nav(Direction::Up)));
        let _ = app.update(Message::Pad(crate::gamepad::Pad::Confirm));
        snapshot(&app, "entry-command");
        let _ = app.update(Message::EntryKey(crate::ui::entry::Key::Control(crate::ui::entry::Control::Symbols)));
        snapshot(&app, "entry-symbols");
        let _ = app.update(Message::Dismiss);
        let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::RetroArchConfig)));
        let last = app.pick.as_ref().unwrap().picker.rows.len() - 1;
        let _ = app.update(Message::Picked(last));
        snapshot(&app, "browse-retroarch");
    }

    /// Every section of Settings, one at a time as the page shows them — Saves after a sync that
    /// moved some saves, and again after one that could not send one.
    #[test]
    #[ignore = "writes PNGs; run explicitly"]
    fn render_settings_sections() {
        use crate::ui::settings::Section;
        let mut app = paired_fixtures(30);
        app.banner = None;
        let _ = app.update(Message::TabSelected(Tab::Settings));
        app.save_sync = SaveSync::Done(Box::new(crate::sync::Report { in_sync: 124, pushed: vec!["a".into(); 3], pulled: vec!["b".into()], ..Default::default() }));
        app.save_synced_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(130));
        for (i, section) in Section::ALL.into_iter().enumerate() {
            let _ = app.update(Message::SettingsSection(section));
            snapshot(&app, &format!("settings-{i}-{section:?}").to_lowercase());
        }
        let _ = app.update(Message::SettingsSection(Section::Saves));
        let _ = app.update(Message::Nav(Direction::Down));
        let _ = app.update(Message::Nav(Direction::Down));
        app.save_sync = SaveSync::Done(Box::new(crate::sync::Report {
            in_sync: 120,
            pushed: vec!["a".into(); 2],
            failed: vec!["An empty memory card on RomM has nothing to restore; the save on this device stays.".into()],
            ..Default::default()
        }));
        snapshot(&app, "settings-saves-failing");
        // A sync under way: the card as the last one left it, the icon turning.
        app.sync_before = std::mem::replace(&mut app.save_sync, SaveSync::Running);
        snapshot(&app, "settings-saves-syncing");
    }

    #[test]
    #[ignore = "writes PNGs; run explicitly"]
    fn render_app_update() {
        use crate::release::Status;
        let mut app = paired_fixtures(30);
        app.appimage = Some("/home/deck/Applications/Deckhand.AppImage".into());
        let mut release = super::release("0.6.0");
        release.headline = Some("New in 0.6.0: unpacked archives, a disc picker, and a space budget".into());
        app.release = Status::Out(Box::new(release.clone()));
        let _ = app.update(Message::TabSelected(Tab::Settings));
        for _ in 0..4 {
            let _ = app.update(Message::CycleSystem(1));
        }
        let _ = app.update(Message::Nav(Direction::Down));
        snapshot_sized(&app, "settings-update-out", (1280.0, 2100.0));
        app.release = Status::Installing(Box::new(release.clone()), 0.43);
        snapshot_sized(&app, "settings-update-installing", (1280.0, 2100.0));
        app.release = Status::Installed(Box::new(release));
        snapshot_sized(&app, "settings-update-installed", (1280.0, 2100.0));
    }
}

#[test]
fn a_game_romm_has_no_file_for_is_not_downloaded_and_its_button_says_why() {
    use crate::ui::detail::Action;
    let mut app = paired_fixtures(12);
    let mut games: Vec<Game> = app.library.all_games().to_vec();
    games[0].unavailable = true;
    let rom_id = games[0].rom_id;
    app.library = Arc::new(Library::new(app.library.systems.clone(), games));

    let _ = app.update(Message::Download(rom_id));
    assert!(app.banner.as_deref().unwrap().contains("isn't on RomM's disk"), "{:?}", app.banner);
    assert_eq!(app.downloads.in_progress().count(), 0, "nothing queued");

    let _ = app.update(Message::OpenDetail(rom_id));
    app.banner = None;
    let _ = app.update(Message::DetailAction(Action::Download));
    assert!(app.banner.as_deref().unwrap().contains("rescan its platform"), "{:?}", app.banner);
    let _ = app.view();
}

#[test]
fn the_detail_page_asks_for_a_game_s_achievements_once_and_scrolls_down_to_them() {
    use crate::romm::models::Achievement;
    let mut app = paired_fixtures(12);
    let mut games: Vec<Game> = app.library.all_games().to_vec();
    games[0].achievements = 2;
    let (with, without) = (games[0].rom_id, games[1].rom_id);
    app.library = Arc::new(Library::new(app.library.systems.clone(), games));

    assert_eq!(app.load_achievements(without).units(), 0, "a game with none asks for nothing");
    assert!(app.load_achievements(with).units() > 0);
    let _ = app.update(Message::OpenDetail(with));
    let list = vec![
        Achievement { title: "First".into(), points: Some(5), num_awarded: Some(19_293), display_order: Some(1), badge_id: Some("1".into()), ..Default::default() },
        Achievement { title: "Second".into(), points: Some(10), badge_id: Some("2".into()), ..Default::default() },
    ];
    let user: crate::romm::models::UserInfo = serde_json::from_value(serde_json::json!({
        "id": 1, "ra_username": "player",
        "ra_progression": { "results": [{ "rom_ra_id": 7, "earned_achievements": [{ "id": "2", "date": "2026-08-06 19:19:21" }] }] }
    }))
    .unwrap();
    let game = crate::achievements::GameAchievements::new(list, Some(&user), Some(7));
    assert_eq!(game.earned_count(), 1);
    let _ = app.update(Message::AchievementsLoaded(with, Ok(Arc::new(game))));
    assert!(app.load_achievements(with).units() > 0, "asked again on every visit: earning happens in between");
    let _ = app.view();
    assert!(app.update(Message::Nav(Direction::Down)).units() > 0, "Down scrolls the page");
    assert_eq!(app.detail_focus.zone, crate::ui::detail::Zone::Achievements, "to their heading");
}

#[test]
fn the_game_page_stands_on_its_own_games_picture() {
    let (app, game) = detail_with(2, 0);
    assert_eq!(app.spotlit_id(), Some(game.rom_id), "the backdrop follows the page's game");
    let wanted = app.wanted_art();
    for kind in [covers::Kind::Spotlight, covers::Kind::Ambient] {
        assert!(wanted.iter().any(|(url, _, k)| *k == kind && *url == game.screenshots[0]), "its first screenshot, as {kind:?}");
    }
}

/// A game's page with `shots` screenshots arrived, and `achievements` to earn.
fn detail_with(shots: usize, achievements: u32) -> (App, Game) {
    let mut app = paired_fixtures(12);
    let mut games: Vec<Game> = app.library.all_games().to_vec();
    let host = app.host().to_string();
    games[0].screenshots = (0..shots).map(|i| format!("{host}/assets/screenshots/{i}.png")).collect();
    games[0].achievements = achievements;
    let game = games[0].clone();
    app.library = Arc::new(Library::new(app.library.systems.clone(), games));
    let meta = covers::CoverMeta { ratio: 4.0 / 3.0, dominant: Color::BLACK };
    for url in &game.screenshots {
        app.covers.insert(url.clone(), covers::Cover::Ready(iced::widget::image::Handle::from_rgba(4, 3, vec![0; 48]), meta));
    }
    let _ = app.update(Message::OpenDetail(game.rom_id));
    (app, game)
}

#[test]
fn the_game_pages_cursor_walks_the_actions_the_screenshots_and_the_achievements() {
    use crate::ui::detail::Zone;
    let (mut app, _) = detail_with(3, 12);
    assert_eq!(app.detail_zones(), [Zone::Actions, Zone::Shots, Zone::Achievements]);

    let _ = app.update(Message::Nav(Direction::Down));
    assert_eq!(app.detail_focus.zone, Zone::Shots);
    for _ in 0..4 {
        let _ = app.update(Message::Nav(Direction::Right));
    }
    assert_eq!(app.detail_focus.shot, 2, "held at the last");
    let _ = app.update(Message::Nav(Direction::Down));
    assert_eq!(app.detail_focus.zone, Zone::Achievements);
    assert!(!app.achievements_open, "folded until opened");
    let _ = app.update(Message::Activate);
    assert!(app.achievements_open);
    let _ = app.view();
    let _ = app.update(Message::Activate);
    assert!(!app.achievements_open, "and folded again");

    // Up reads an opened list back up before the cursor leaves it: the page says which.
    let _ = app.update(Message::DetailClimbed(Some(300.0)));
    assert_eq!(app.detail_focus.zone, Zone::Achievements, "a step up the list keeps the cursor in it");
    let _ = app.update(Message::DetailClimbed(None));
    assert_eq!(app.detail_focus.zone, Zone::Shots, "the heading in view, on to the row above");
    let _ = app.update(Message::DetailClimbed(None));
    assert_eq!(app.detail_focus.zone, Zone::Actions);
    assert_eq!(app.detail_focus.shot, 2, "the screenshots keep their place for the way back");
    let _ = app.update(Message::Nav(Direction::Up));
    assert_eq!(app.chrome, None, "scrolled down the page, Up goes up it first");
    let _ = app.update(Message::DetailScrolled(0.0));
    let _ = app.update(Message::Nav(Direction::Up));
    assert_eq!(app.chrome, Some(Chrome::Tabs), "from the top of the page, up into the bar");

    // A game with neither has its actions alone, and Down reads on down the page.
    let (mut app, _) = detail_with(0, 0);
    assert_eq!(app.detail_zones(), [Zone::Actions]);
    assert!(app.update(Message::Nav(Direction::Down)).units() > 0);
    assert_eq!(app.detail_focus.zone, Zone::Actions);
}

#[test]
fn a_screenshot_opens_over_the_window_steps_along_and_closes_back_to_the_page() {
    use crate::ui::detail::Zone;
    let (mut app, game) = detail_with(3, 0);
    let _ = app.update(Message::Nav(Direction::Down));
    let _ = app.update(Message::Nav(Direction::Right));
    let _ = app.update(Message::Activate);
    assert_eq!(app.detail_focus.viewing, Some(1));
    let full: Vec<String> = app.wanted_art().into_iter().filter(|(_, _, kind)| *kind == covers::Kind::Full).map(|(url, _, _)| url).collect();
    assert_eq!(full, [game.screenshots[1].clone(), game.screenshots[0].clone(), game.screenshots[2].clone()], "the one open first, then either side");
    let _ = app.view();

    for _ in 0..2 {
        let _ = app.update(Message::Nav(Direction::Right));
    }
    assert_eq!(app.detail_focus.viewing, Some(2), "held at the last");
    // Keys that would act on the page under it wait until it is closed.
    let _ = app.update(Message::CycleTab(1));
    let _ = app.update(Message::Nav(Direction::Down));
    assert!(matches!(app.screen, Screen::Browsing(Route::Detail { .. })));
    assert_eq!(app.detail_focus.viewing, Some(2));

    let _ = app.update(Message::Pad(crate::gamepad::Pad::Back));
    assert_eq!(app.detail_focus.viewing, None);
    assert!(matches!(app.screen, Screen::Browsing(Route::Detail { .. })), "Back closes the picture, not the page");
    assert_eq!((app.detail_focus.zone, app.detail_focus.shot), (Zone::Shots, 2), "the cursor on the last one looked at");

    // A click on one opens it too, and a click on the backdrop closes it.
    let _ = app.update(Message::ViewShot(0));
    assert_eq!(app.detail_focus.viewing, Some(0));
    let _ = app.update(Message::CloseShot);
    assert_eq!(app.detail_focus.viewing, None);
    assert!(app.wanted_art().iter().all(|(_, _, kind)| *kind != covers::Kind::Full), "nothing full-size kept asking once closed");
}

#[test]
fn a_download_over_the_space_budget_first_removes_the_least_recently_used_game() {
    let mut app = paired_fixtures(12);
    app.settings.storage_budget_gb = Some(16);
    let games: Vec<Game> = app.library.all_games().to_vec();
    let gib = 1024 * 1024 * 1024u64;
    // Three games of 5 GB: one pinned, one played last week, one played years ago.
    for (game, played, pinned) in [(&games[0], "2020-01-01T00:00:00Z", true), (&games[1], "2026-09-25T00:00:00Z", false), (&games[2], "2021-01-01T00:00:00Z", false)] {
        app.downloads.enqueue(QueueItem { rom_id: game.rom_id, title: game.title.clone(), system_id: game.system_id.clone(), ..Default::default() });
        app.downloads.start_next();
        let mut record = installed(game.rom_id, &game.title, 5 * gib);
        record.played_at = Some(played.into());
        record.pinned = pinned;
        app.downloads.finish(game.rom_id, Ok(record));
    }

    // 4 GB more would make 19 of 16: the oldest unpinned one goes, the pinned oldest of all stays.
    let item = QueueItem { rom_id: games[3].rom_id, title: games[3].title.clone(), system_id: games[3].system_id.clone(), bytes: Some(4 * gib), ..Default::default() };
    app.downloads.enqueue(item.clone());
    app.downloads.start_next();
    let _ = app.make_room(&item);
    assert!(app.downloads.installed(games[2].rom_id).is_none(), "played longest ago, and not pinned");
    assert!(app.downloads.installed(games[0].rom_id).is_some(), "pinned");
    assert!(app.downloads.installed(games[1].rom_id).is_some(), "recent");
    assert!(app.banner.as_deref().is_some_and(|b| b.contains(&games[2].title) && b.contains("stay in your library")), "{:?}", app.banner);
}

#[test]
fn without_a_budget_nothing_is_ever_removed_to_make_room() {
    let mut app = paired_fixtures(4);
    let games: Vec<Game> = app.library.all_games().to_vec();
    app.downloads.enqueue(QueueItem { rom_id: games[0].rom_id, title: "A".into(), system_id: games[0].system_id.clone(), ..Default::default() });
    app.downloads.start_next();
    app.downloads.finish(games[0].rom_id, Ok(installed(games[0].rom_id, "A", u64::MAX / 4)));
    let item = QueueItem { rom_id: games[1].rom_id, bytes: Some(u64::MAX / 4), ..Default::default() };
    let _ = app.make_room(&item);
    assert!(app.downloads.installed(games[0].rom_id).is_some());
}

#[test]
fn download_asks_which_build_and_queues_the_one_picked() {
    let mut app = paired_fixtures(6);
    let mut games: Vec<Game> = app.library.all_games().to_vec();
    let tile = games[0].rom_id;
    games[0].sibling_ids = vec![777];
    games[0].file_stems = vec![(tile, "Game (USA)".into()), (777, "Game (Japan)".into())];
    app.library = Arc::new(Library::new(app.library.systems.clone(), games));

    let _ = app.offer_choices(tile, Err("no detail".into()));
    let pick = app.pick.as_ref().expect("a choice to make");
    let labels: Vec<&str> = pick.picker.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["USA", "Japan"]);
    let _ = app.update(Message::Nav(Direction::Down));
    let _ = app.update(Message::Activate);
    assert!(app.pick.is_none());
    let queued: Vec<i64> = app.downloads.in_progress().map(|(i, _)| i.rom_id).collect();
    assert_eq!(queued, [777], "the build picked, under the game's title");
    assert_eq!(app.game_state(tile), crate::downloads::SyncState::Downloading(0.0), "and the tile says so");
}

/// A fixture app with collections of two kinds: one of the player's, two genres.
fn with_collections(count: usize) -> (App, Vec<i64>) {
    use crate::romm::models::{Collection, VirtualCollection};
    let mut app = paired_fixtures(count);
    let ids: Vec<i64> = app.library.all_games().iter().map(|g| g.rom_id).collect();
    let genre = |id: &str, name: &str, rom_ids: Vec<i64>| VirtualCollection {
        id: id.into(),
        name: Some(name.into()),
        kind: "genre".into(),
        rom_ids,
    };
    app.collection_sources = Some(Ok((
        Vec::new(),
        vec![genre("g-1", "Action", ids[..6].to_vec()), genre("g-2", "Puzzle", vec![ids[6], ids[7]])],
    )));
    app.collections = vec![Collection {
        id: 5,
        name: Some("Couch co-op".into()),
        rom_count: 3,
        rom_ids: vec![ids[8], ids[9], ids[10]],
        is_favorite: false,
        is_virtual: false,
        is_smart: false,
    }];
    app.rebuild_collections();
    (app, ids)
}

#[test]
fn the_triggers_walk_the_collection_kinds_and_leave_the_cursor_in_its_band() {
    use crate::collections::{Kind, Zone};
    let (mut app, _) = with_collections(30);
    let _ = app.update(Message::TabSelected(Tab::Collections));
    assert_eq!(app.all_collections.kinds(), [Kind::Yours, Kind::Genre]);
    let _ = app.update(Message::Nav(Direction::Right));
    let _ = app.update(Message::CycleSystem(1));
    assert_eq!((app.collections_focus.kind, app.collections_focus.zone, app.collections_focus.cards.index()), (1, Zone::Cards, 0), "R2: genres, from its first card");
    let _ = app.update(Message::CycleSystem(1));
    assert_eq!(app.collections_focus.kind, 1, "nothing past the last kind");
    let _ = app.view();

    // Up on the strip, the triggers still walk it, and the cursor stays up there.
    let _ = app.update(Message::Nav(Direction::Up));
    let _ = app.update(Message::CycleSystem(-1));
    assert_eq!((app.collections_focus.kind, app.collections_focus.zone), (0, Zone::Kinds));
    let _ = app.view();
}

#[test]
fn a_collection_opens_as_a_grid_that_steps_through_its_kind_and_leads_back_to_the_page() {
    use crate::collections::Kind;
    let (mut app, ids) = with_collections(30);
    let _ = app.update(Message::TabSelected(Tab::Collections));
    assert!(matches!(app.screen, Screen::Browsing(Route::Collections)));
    assert_eq!(app.all_collections.kinds(), [Kind::Yours, Kind::Genre], "the player's own first");
    let _ = app.view();

    // Up off the cards to the chips, over to genres, down into its cards, and open the first.
    let _ = app.update(Message::Nav(Direction::Up));
    let _ = app.update(Message::Nav(Direction::Right));
    let _ = app.update(Message::Nav(Direction::Down));
    let _ = app.update(Message::Activate);
    let open = |app: &App| match &app.screen {
        Screen::Browsing(route @ Route::Browse { scope: Scope::Collection(key) }) => (key.clone(), route.tab()),
        _ => panic!("not on a collection's grid"),
    };
    assert_eq!(open(&app), ("virtual:g-1".to_string(), Tab::Collections), "Action, under the Collections tab");
    assert_eq!(app.scope_view(&Scope::Collection("virtual:g-1".into())).0.len(), 6, "every system it touches");
    let names: Vec<String> = app.places(&Scope::Collection("virtual:g-1".into())).into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["Action", "Puzzle"], "beside it, the other genres — not the player's own");
    let _ = app.view();

    // A game opened from it says where it is in the collection, and back returns to it.
    let first = app.scope_view(&Scope::Collection("virtual:g-1".into())).0.games[0].rom_id;
    assert!(ids[..6].contains(&first));
    let _ = app.update(Message::Activate);
    assert!(matches!(&app.screen, Screen::Browsing(Route::Detail { rom_id, .. }) if *rom_id == first));
    let _ = app.view();
    let _ = app.update(Message::Back);
    assert_eq!(open(&app).0, "virtual:g-1");

    // ] steps to the next genre, and Esc goes back to the page of collections.
    let _ = app.update(Message::CycleSystem(1));
    assert_eq!(open(&app).0, "virtual:g-2");
    let _ = app.update(Message::Dismiss);
    assert!(matches!(app.screen, Screen::Browsing(Route::Collections)));
    assert_eq!(app.collections_focus.kind, 1, "where it was left");
}

fn favourites_answer(rom_ids: Vec<i64>) -> Collection {
    Collection {
        id: 9,
        name: Some("Favourites".into()),
        rom_count: rom_ids.len() as i64,
        rom_ids,
        is_favorite: true,
        is_virtual: false,
        is_smart: false,
    }
}

#[test]
fn favourites_lead_the_collections_page_as_their_games_and_follow_a_heart() {
    use crate::collections::Kind;
    let (mut app, ids) = with_collections(30);
    let title = |app: &App, id: i64| app.library.game(id).unwrap().title.clone();
    let _ = app.update(Message::TabSelected(Tab::Collections));
    assert_eq!(app.all_collections.kinds(), [Kind::Yours, Kind::Genre], "no favourites, no chip for them");

    // Over to the genres, then two hearts pressed: the chip comes in ahead, and the cursor stays on
    // the genres it was on.
    let _ = app.update(Message::Nav(Direction::Up));
    let _ = app.update(Message::Nav(Direction::Right));
    let _ = app.toggle_favourite(ids[20]);
    let _ = app.toggle_favourite(ids[4]);
    assert_eq!(app.all_collections.kinds(), [Kind::Favourites, Kind::Yours, Kind::Genre]);
    assert_eq!(app.collections_focus.kind, 2, "still on the genres");
    for tile in [ids[20], ids[4]] {
        let result = Ok(favourites_answer(vec![ids[4], ids[20]]));
        let _ = app.update(Message::FavouriteChanged { tile, adding: true, ids: vec![tile], result });
    }

    // Over to favourites: its cards are the games themselves, in library order, each with its system.
    let _ = app.update(Message::Nav(Direction::Left));
    let _ = app.update(Message::Nav(Direction::Left));
    let cards = app.collection_cards();
    let names: Vec<String> = cards.iter().map(|c| c.name.to_string()).collect();
    assert_eq!(names, [title(&app, ids[4]), title(&app, ids[20])]);
    let system = &app.library.game(ids[4]).unwrap().system_id;
    assert_eq!(cards[0].note, app.library.system_name(system));
    drop(cards);
    let _ = app.view();

    // A card opens its game's page, still under the Collections tab.
    let _ = app.update(Message::Nav(Direction::Down));
    let _ = app.update(Message::Nav(Direction::Right));
    let _ = app.update(Message::Activate);
    assert!(matches!(&app.screen, Screen::Browsing(route @ Route::Detail { rom_id, .. }) if *rom_id == ids[20] && route.tab() == Tab::Collections));

    // Taken out of favourites there: back on the page, the cursor is on the card that is left.
    let _ = app.toggle_favourite(ids[20]);
    let result = Ok(favourites_answer(vec![ids[4]]));
    let _ = app.update(Message::FavouriteChanged { tile: ids[20], adding: false, ids: vec![ids[20]], result });
    let _ = app.update(Message::Back);
    assert!(matches!(app.screen, Screen::Browsing(Route::Collections)));
    assert_eq!(app.collection_cards().len(), 1);
    assert_eq!(app.collections_focus.cards.index(), 0);
    let _ = app.view();

    // The last one out takes the chip with it, and the cursor to the kind now in its place.
    let _ = app.toggle_favourite(ids[4]);
    assert_eq!(app.all_collections.kinds(), [Kind::Yours, Kind::Genre]);
    assert_eq!(app.collections_focus.kind, 0);
    let _ = app.view();
}

#[test]
fn a_collection_made_on_romm_while_the_app_is_open_is_on_the_page_the_next_time_it_opens() {
    use crate::collections::Kind;
    let (mut app, ids) = with_collections(30);
    let testing = Collection {
        id: 2,
        name: Some("Testing".into()),
        rom_count: 1,
        rom_ids: vec![ids[12]],
        is_favorite: false,
        is_virtual: false,
        is_smart: false,
    };
    let mut own = app.collections.clone();
    own.push(testing);
    let _ = app.update(Message::TabSelected(Tab::Collections));
    let edits = app.favourite_edits;
    let _ = app.update(Message::OwnCollectionsLoaded { edits, result: Ok((own.clone(), None)) });
    let yours: Vec<&str> = app.all_collections.of_kind(Kind::Yours).map(|c| c.name.as_str()).collect();
    assert_eq!(yours, ["Couch co-op", "Testing"]);
    assert!(app.shelves.iter().any(|s| s.title == "Testing"), "and on Home, as a shelf of its own");

    // Favourites made on RomM's web app come with them.
    own.push(favourites_answer(vec![ids[0]]));
    let _ = app.update(Message::OwnCollectionsLoaded { edits, result: Ok((own.clone(), None)) });
    assert!(app.favourites.contains(&ids[0]));
    assert_eq!(app.all_collections.favourites(), [ids[0]]);

    // An answer asked for before a heart was pressed here does not undo it.
    let _ = app.toggle_favourite(ids[1]);
    let _ = app.update(Message::OwnCollectionsLoaded { edits, result: Ok((own.clone(), None)) });
    assert!(app.favourites.contains(&ids[1]));
    assert_eq!(app.all_collections.favourites(), [ids[0], ids[1]]);

    // One that fails leaves the page as it was.
    let edits = app.favourite_edits;
    let _ = app.update(Message::OwnCollectionsLoaded { edits, result: Err("timed out".into()) });
    assert!(app.all_collections.get("manual:2").is_some());
    let _ = app.view();
}

#[test]
fn a_games_saves_are_counted_on_its_page_and_walked_on_their_own() {
    use crate::game_saves::{GameSaves, Tab};
    let mut app = paired_fixtures(12);
    let game = app.library.all_games()[2].clone();
    let _ = app.update(Message::OpenDetail(game.rom_id));
    let record = |id: i64, name: &str| -> crate::romm::models::SaveAsset {
        serde_json::from_value(serde_json::json!({ "id": id, "rom_id": game.rom_id, "file_name": name })).unwrap()
    };
    let saves = GameSaves {
        states: vec![record(3, "Game.slot1.deckhand.state"), record(2, "Game.state"), record(1, "Game [x].deckhand.state")],
        saves: vec![record(9, "Game.srm")],
        ..Default::default()
    };
    let _ = app.update(Message::SavesLoaded(game.rom_id, Ok(Arc::new(saves))));
    let _ = app.view();

    let _ = app.update(Message::DetailAction(crate::ui::detail::Action::Saves));
    assert!(matches!(&app.screen, Screen::Browsing(Route::Saves { rom_id, .. }) if *rom_id == game.rom_id));
    assert_eq!(app.saves_focus.tab, Tab::States);
    let _ = app.update(Message::Nav(Direction::Down));
    let _ = app.update(Message::Nav(Direction::Down));
    let _ = app.update(Message::Nav(Direction::Down));
    assert_eq!(app.saves_focus.row(3), 2, "clamped at the last state");
    let _ = app.view();
    let _ = app.update(Message::Nav(Direction::Right));
    assert_eq!(app.saves_focus.tab, Tab::Saves);
    let _ = app.update(Message::SavesTab(Tab::States));
    assert_eq!(app.saves_focus.row(3), 2, "each tab keeps its own row");
    let _ = app.view();

    // Esc steps back out to the game, not past it.
    let _ = app.update(Message::Dismiss);
    assert!(matches!(&app.screen, Screen::Browsing(Route::Detail { rom_id, .. }) if *rom_id == game.rom_id));
}

#[test]
fn an_older_in_game_save_is_restored_only_after_asking_and_only_for_a_game_that_is_here() {
    use crate::game_saves::{GameSaves, Tab};
    let mut app = paired_fixtures(12);
    let game = app.library.all_games()[2].clone();
    let record = |id: i64, stamp: &str| -> crate::romm::models::SaveAsset {
        serde_json::from_value(serde_json::json!({ "id": id, "rom_id": game.rom_id, "file_name": "Game.srm", "updated_at": stamp })).unwrap()
    };
    let saves = GameSaves {
        saves: vec![record(9, "2026-10-01T12:00:00Z"), record(8, "2026-09-01T12:00:00Z")],
        on_device: [9].into_iter().collect(),
        ..Default::default()
    };
    let _ = app.update(Message::OpenDetail(game.rom_id));
    let _ = app.update(Message::SavesLoaded(game.rom_id, Ok(Arc::new(saves))));
    let _ = app.update(Message::DetailAction(crate::ui::detail::Action::Saves));
    let _ = app.update(Message::SavesTab(Tab::Saves));

    // The newest is the one here: nothing to restore.
    let _ = app.update(Message::Activate);
    assert!(app.banner.as_deref().unwrap().contains("already"), "{:?}", app.banner);
    assert!(app.pick.is_none());

    // The older one, for a game not downloaded here: download it first.
    let _ = app.update(Message::Nav(Direction::Down));
    let _ = app.update(Message::Activate);
    assert!(app.banner.as_deref().unwrap().contains("download it first"), "{:?}", app.banner);

    // Downloaded, it asks — on the safe answer — and keeping it changes nothing.
    app.downloads.enqueue(QueueItem { rom_id: game.rom_id, title: game.title.clone(), system_id: game.system_id.clone(), ..Default::default() });
    app.downloads.start_next();
    let record_here = installed(game.rom_id, &game.title, 1024);
    app.downloads.finish(game.rom_id, Ok(record_here));
    let _ = app.update(Message::Activate);
    let picker = &app.pick.as_ref().expect("asks first").picker;
    assert_eq!((picker.title.as_str(), picker.cursor), ("Restore this save?", 1));
    let _ = app.view();
    assert_eq!(app.update(Message::Picked(1)).units(), 0, "kept: nothing happens");
    let _ = app.update(Message::Activate);
    assert!(app.update(Message::Picked(0)).units() > 0, "restore it: under way");
    assert!(app.banner.as_deref().unwrap().starts_with("Restoring"), "{:?}", app.banner);
}

#[test]
fn a_game_a_rebuilt_romm_keeps_under_a_new_id_takes_its_steam_shortcut_along_and_the_window_says_what_happened() {
    use crate::library::Stamp;
    let mut app = paired_fixtures(4);
    let stamp = |stem: &str| Some(Stamp { stem: stem.into(), born: Some("2026-08-01T22:26:48+00:00".into()) });
    for (rom_id, title, stem) in [(1, "Secret of Mana", "Secret of Mana (USA)"), (2, "Banjo-Kazooie", "Banjo-Kazooie (USA)")] {
        app.downloads.enqueue(QueueItem { rom_id, title: title.into(), system_id: "snes".into(), ..Default::default() });
        app.downloads.start_next();
        app.downloads.finish(rom_id, Ok(crate::downloads::Installed { stamp: stamp(stem), ..installed(rom_id, title, 1024) }));
    }
    let host = app.host().to_string();
    app.steam_bindings.bind(&host, 1, 0x8000_0001);

    // Rebuilt: Zipang was scanned into Secret of Mana's place, Secret of Mana is 900 now, and
    // Banjo-Kazooie is not in RomM at all.
    let game = |rom_id: i64, title: &str, stem: &str| Game {
        rom_id,
        title: title.into(),
        system_id: "snes".into(),
        file_stems: vec![(rom_id, stem.into())],
        created_at: Some("2026-10-12T19:00:00+00:00".into()),
        ..Default::default()
    };
    let games = vec![game(1, "Zipang", "Zipang (Japan)"), game(2, "Pilotwings", "Pilotwings (USA)"), game(900, "Secret of Mana", "Secret of Mana (USA)")];
    app.library = Arc::new(Library::new(app.library.systems.clone(), games));
    app.match_downloads();

    assert!(app.downloads.installed(900).is_some() && app.downloads.installed(1).is_none() && app.downloads.installed(2).is_none());
    assert_eq!(app.steam_bindings.get(&host, 900), Some(0x8000_0001), "its shortcut goes with it");
    assert_eq!(app.steam_bindings.get(&host, 1), None, "and is not Zipang's");
    let banner = app.banner.clone().unwrap_or_default();
    assert!(banner.contains("under a new id") && banner.contains("\"Banjo-Kazooie\""), "{banner}");
}

#[test]
fn a_game_goes_into_steam_only_from_this_device_and_says_so_once_it_is_in() {
    use crate::ui::detail::Action;
    let mut app = paired_fixtures(12);
    let game = app.library.all_games()[3].clone();
    let _ = app.update(Message::OpenDetail(game.rom_id));
    let _ = app.update(Message::DetailAction(Action::Steam));
    assert!(app.banner.as_deref().unwrap().contains("nothing for Steam to start"), "{:?}", app.banner);

    app.downloads.enqueue(QueueItem { rom_id: game.rom_id, title: game.title.clone(), system_id: game.system_id.clone(), ..Default::default() });
    app.downloads.start_next();
    app.downloads.finish(game.rom_id, Ok(installed(game.rom_id, &game.title, 1024)));
    let _ = app.update(Message::DetailAction(Action::Steam));
    assert!(app.banner.as_deref().unwrap().contains("No Steam found"), "{:?}", app.banner);

    // Added — as a worker answers once Steam's file is written.
    let _ = app.update(Message::SteamChanged { add: true, games: vec![(game.rom_id, 0x8000_0007)], restarted: true, result: Ok(crate::steam::Gaps::default()) });
    assert!(app.banner.as_deref().unwrap().contains("Steam is starting again"), "{:?}", app.banner);
    assert_eq!(app.steam_bindings.get(app.host(), game.rom_id), Some(0x8000_0007));
    assert_eq!(app.steam_bindings.get(app.host(), game.rom_id), Some(0x8000_0007), "kept, to take it out again");
    let _ = app.view();
    let _ = app.update(Message::SteamChanged { add: false, games: vec![(game.rom_id, 0x8000_0007)], restarted: false, result: Ok(crate::steam::Gaps::default()) });
    assert_eq!(app.steam_bindings.get(app.host(), game.rom_id), None);
}

#[test]
fn the_save_folders_are_not_looked_over_while_a_game_from_here_runs_or_a_sync_does() {
    let mut app = paired_fixtures(12);
    app.playing = Some(app.library.all_games()[0].rom_id);
    assert_eq!(app.update(Message::WatchTick).units(), 0, "the launch syncs when it ends");
    app.playing = None;
    app.save_sync = SaveSync::Running;
    assert_eq!(app.update(Message::WatchTick).units(), 0, "one sync at a time");
    app.save_sync = SaveSync::Idle;
    assert!(app.update(Message::WatchTick).units() > 0, "looked over off the UI thread");
    assert_eq!(app.update(Message::WatchTick).units(), 0, "and not twice at once");
    let _ = app.update(Message::WatchScanned(None, crate::watch::Snapshot::default()));
    assert!(!app.looking);
    // A setting written by default stays out of the file, and off is kept.
    let off = Settings { watch_saves: false, ..Settings::default() };
    assert!(serde_json::to_string(&Settings::default()).unwrap().find("watch_saves").is_none());
    let read: Settings = serde_json::from_str(&serde_json::to_string(&off).unwrap()).unwrap();
    assert!(!read.watch_saves);
    let fresh: Settings = serde_json::from_str("{}").unwrap();
    assert!(fresh.watch_saves, "on unless turned off");
}

#[test]
fn a_conflict_takes_the_keys_until_it_is_answered_and_the_answer_reaches_the_sync() {
    use crate::sync::{Choice, Conflict};
    let mut app = paired_fixtures(12);
    let rom_id = app.library.all_games()[0].rom_id;
    app.playing = Some(rom_id);
    let (reply, mut answer) = futures::channel::oneshot::channel();
    let conflict = Conflict { name: "Game.srm".into(), local_time: None, remote_time: None, local_bytes: 8192, newer: Choice::Remote };
    let _ = app.update(Message::LaunchEvent(crate::launch::Event::Conflict(rom_id, conflict, crate::launch::Reply::new(reply))));
    let _ = app.view();
    assert_eq!(app.conflict.as_ref().map(|c| c.keep), Some(Choice::Remote), "the cursor starts on the newer");

    // Played-game pad input normally goes nowhere; the chooser is the exception.
    let _ = app.update(Message::Pad(crate::gamepad::Pad::Nav(Direction::Left)));
    assert_eq!(app.conflict.as_ref().map(|c| c.keep), Some(Choice::Local));
    let screen = matches!(app.screen, Screen::Browsing(Route::Home));
    let _ = app.update(Message::Nav(Direction::Down));
    assert!(app.conflict.is_some() && screen == matches!(app.screen, Screen::Browsing(Route::Home)), "other keys do nothing to it");

    let _ = app.update(Message::Pad(crate::gamepad::Pad::Confirm));
    assert!(app.conflict.is_none());
    assert_eq!(answer.try_recv().unwrap(), Some(Choice::Local));
    assert!(app.banner.as_deref().unwrap().contains("keeping this device's Game.srm"), "{:?}", app.banner);
}

#[test]
fn the_pad_drives_the_app_except_while_a_game_has_it() {
    use crate::gamepad::Pad;
    let mut app = paired_fixtures(12);
    let _ = app.update(Message::Pad(Pad::Start));
    assert!(matches!(app.screen, Screen::Browsing(Route::Settings)));
    let _ = app.update(Message::Pad(Pad::Search));
    assert!(matches!(app.screen, Screen::Browsing(Route::Search { .. })));
    let _ = app.update(Message::Pad(Pad::Confirm));
    assert_eq!(app.search_focus.query, "q", "A types the key under the cursor");
    let _ = app.update(Message::Pad(Pad::Erase));
    assert_eq!(app.search_focus.query, "");
    let _ = app.update(Message::Pad(Pad::Back));
    assert!(matches!(app.screen, Screen::Browsing(Route::Settings)));

    app.playing = Some(1);
    let _ = app.update(Message::Pad(Pad::Select));
    assert!(matches!(app.screen, Screen::Browsing(Route::Settings)), "the game's buttons are not the app's");
}

#[test]
fn search_is_typed_walked_into_and_left_back_where_it_was_opened() {
    let mut app = paired_fixtures(30);
    let game = app.library.all_games()[7].clone();
    let _ = app.update(Message::KeyChar("/".into(), false));
    assert!(matches!(&app.screen, Screen::Browsing(Route::Search { from }) if **from == Route::Home));

    // Typed on a real keyboard, capitals and all; the shortcut keys type too while here.
    for c in game.title.chars() {
        let _ = app.update(Message::KeyChar(c.to_string(), false));
    }
    assert_eq!(app.search_focus.query, game.title.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == ' ').collect::<String>());
    let results = app.search_results();
    let at = results.iter().position(|g| g.rom_id == game.rom_id).expect("the game finds itself");
    let _ = app.view();

    for _ in 0..4 {
        let _ = app.update(Message::Nav(Direction::Down));
    }
    assert!(app.search_focus.in_results);
    let _ = app.update(Message::SearchResultClicked(at));
    assert!(matches!(&app.screen, Screen::Browsing(Route::Detail { rom_id, .. }) if *rom_id == game.rom_id));

    let _ = app.update(Message::Dismiss);
    assert!(matches!(app.screen, Screen::Browsing(Route::Search { .. })), "back to the search, query intact");
    assert!(!app.search_focus.query.is_empty());
    let _ = app.update(Message::Dismiss);
    assert!(matches!(app.screen, Screen::Browsing(Route::Home)));
}

#[test]
fn the_library_spotlight_follows_the_grid_cursor_and_makes_way_in_a_short_window() {
    let mut app = paired_fixtures(30);
    let _ = app.update(Message::OpenSystem("snes".into()));
    let grid: Vec<i64> = app.scope_view(&Scope::System("snes".into())).0.games.iter().map(|g| g.rom_id).collect();
    assert_eq!(app.spotlit_id(), Some(grid[0]), "on the first tile");
    let _ = app.update(Message::Nav(Direction::Right));
    assert_eq!(app.spotlit_id(), Some(grid[1]), "and on the next");
    assert_eq!(app.backdrop.waiting().or_else(|| app.backdrop.layers(std::time::Instant::now()).last().map(|(id, _)| id)), Some(grid[1]), "its picture with it");
    let _ = app.view();

    // Up on the chips, it stays on the game the grid's cursor rests on.
    while app.browse_focus.zone() != crate::focus::BrowseZone::Chips {
        let _ = app.update(Message::Nav(Direction::Up));
    }
    assert_eq!(app.spotlit_id(), Some(grid[1]));

    app.content_height = crate::ui::tab_bar::TAB_BAR_HEIGHT + crate::ui::browse::HERO_BELOW - 1.0;
    assert_eq!(app.spotlit_id(), None, "a short window gives the grid the room");
    let _ = app.view();
}

#[test]
fn an_empty_library_still_builds_home_rather_than_dividing_by_nothing() {
    let (mut app, _) = App::new(Some(0));
    app.screen = Screen::Browsing(Route::Home);
    assert!(app.shelves.is_empty());
    assert!(app.focus.spotlit(&app.shelves).is_none());
    let _ = app.view();
    // And the pad on it goes nowhere rather than off the end of nothing.
    for direction in [Direction::Up, Direction::Down, Direction::Left, Direction::Right] {
        let _ = app.update(Message::Nav(direction));
    }
    let _ = app.update(Message::Activate);
    let _ = app.update(Message::Pad(crate::gamepad::Pad::Erase));
    let _ = app.view();
}

#[test]
fn arrow_keys_walk_the_shelves_without_leaving_the_cursor_off_a_card() {
    let (mut app, _) = App::new(Some(60));
    assert!(app.focus.is_focused(&app.shelves, 0, 0), "Home opens on its first card");
    let first = app.shelves[0].games[0].rom_id;
    assert_eq!(app.spotlit_id(), Some(first), "with the spotlight on it");

    // Two steps along the first shelf: the spotlight follows the cursor.
    for direction in [Direction::Right, Direction::Right] {
        let _ = app.update(Message::Nav(direction));
        let _ = app.view();
    }
    let third = app.shelves[0].games[2].rom_id;
    assert_eq!(app.spotlit_id(), Some(third), "two rights from the first card lands on the third");

    // Down to the next shelf and back up: the first shelf is where it was left, not reset.
    let _ = app.update(Message::Nav(Direction::Down));
    assert_eq!(app.focus.shelf_pos(), 1);
    let _ = app.view();
    let _ = app.update(Message::Nav(Direction::Up));
    assert_eq!(app.spotlit_id(), Some(third), "coming back up restores the column");

    // Up once more is the tab bar, the spotlight still on that card.
    let _ = app.update(Message::Nav(Direction::Up));
    assert_eq!(app.chrome, Some(Chrome::Tabs));
    assert_eq!(app.spotlit_id(), Some(third));
    let _ = app.view();
}

#[test]
fn the_spotlight_follows_the_cursor_and_the_shelves_slide_to_it() {
    let (mut app, _) = App::new(Some(60));
    let now = std::time::Instant::now();
    let _ = app.update(Message::Nav(Direction::Right));
    assert_eq!(app.backdrop.layers(now + std::time::Duration::from_secs(5)).last().map(|(id, _)| id), app.spotlit_id(),
        "a fixture has no picture to wait for, so its spotlight is up at once");
    let _ = app.update(Message::Nav(Direction::Down));
    assert!(app.glide.is_animating(std::time::Instant::now()), "the column slides down a shelf");
    assert_eq!(app.glide.window(now + std::time::Duration::from_secs(5)), (1, 0.0), "and lands on it");
}

#[test]
fn a_spotlight_picture_still_on_its_way_is_shown_when_it_lands() {
    let (mut app, _) = App::new(Some(60));
    let mut games = app.library.all_games().to_vec();
    let next = app.shelves[0].games[1].rom_id;
    let shot = "https://romm.lan/assets/romm/resources/roms/1/1/screenshots/0.jpg".to_string();
    for game in &mut games {
        if game.rom_id == next {
            game.screenshots = vec![shot.clone()];
        }
    }
    app.library = Arc::new(Library::new(app.library.systems.clone(), games));
    app.rebuild_shelves();

    let _ = app.update(Message::Nav(Direction::Right));
    assert_eq!(app.backdrop.waiting(), Some(next), "the picture is not in, so the old one stays up");
    let picture = Arc::new((iced::widget::image::Handle::from_rgba(1, 1, vec![0; 4]), covers::CoverMeta { ratio: 1.5, dominant: Color::BLACK }));
    let _ = app.update(Message::CoverLoaded(app.art_generation, covers::key(&shot, covers::Kind::Spotlight), Some(picture)));
    assert_eq!(app.backdrop.waiting(), None, "it landed");
    let later = std::time::Instant::now() + std::time::Duration::from_secs(5);
    assert_eq!(app.backdrop.layers(later).last().map(|(id, _)| id), Some(next));
}

#[test]
fn confirm_on_a_card_opens_its_page_on_any_shelf_whether_or_not_the_game_is_here() {
    use crate::gamepad::Pad;
    let (mut app, _) = App::new(Some(60));
    let opened = |app: &App, card: i64| matches!(&app.screen, Screen::Browsing(Route::Detail { rom_id, .. }) if *rom_id == card);

    // Not on this device: its page, where fetching it is a press of its own.
    let card = app.spotlit_id().unwrap();
    let _ = app.update(Message::Activate);
    assert!(opened(&app, card), "the card's page opened, got {:?}", app.screen_name());
    assert!(app.downloads.in_progress().next().is_none(), "and nothing started downloading");
    let _ = app.update(Message::Back);
    assert_eq!(app.spotlit_id(), Some(card), "back on Home, the cursor is still on it");

    // On this device, on a lower shelf: its page too, where playing it is.
    let _ = app.update(Message::Nav(Direction::Down));
    let card = app.spotlit_id().unwrap();
    let game = app.library.game(card).unwrap().clone();
    app.downloads.adopt(vec![installed(card, &game.title, 1 << 20)]);
    app.rebuild_shelves();
    let _ = app.view();
    let _ = app.update(Message::Activate);
    assert!(opened(&app, card), "got {:?}", app.screen_name());
    assert_eq!(app.playing, None, "nothing was started");
    let _ = app.update(Message::Back);

    // X is nothing on Home: the saves are on the game's page.
    let _ = app.update(Message::Pad(Pad::Erase));
    assert!(matches!(app.screen, Screen::Browsing(Route::Home)), "got {:?}", app.screen_name());
}

#[test]
fn a_click_puts_the_spotlight_on_a_card_and_a_second_click_presses_it() {
    let (mut app, _) = App::new(Some(60));
    let target = app.shelves[1].games[2].rom_id;
    let _ = app.update(Message::HomeCardClicked(1, 2));
    assert_eq!(app.spotlit_id(), Some(target));
    assert_eq!(app.focus.shelf_pos(), 1, "the shelf it is on comes up to the top");
    assert!(matches!(app.screen, Screen::Browsing(Route::Home)), "one click only moves the spotlight");
    let _ = app.update(Message::HomeCardClicked(1, 2));
    assert!(matches!(&app.screen, Screen::Browsing(Route::Detail { rom_id, .. }) if *rom_id == target), "got {:?}", app.screen_name());
}

#[test]
fn the_wheel_walks_the_shelves_and_stops_at_the_first() {
    use iced::mouse::ScrollDelta;
    let (mut app, _) = App::new(Some(60));
    let _ = app.update(Message::HomeWheel(ScrollDelta::Lines { x: 0.0, y: -1.0 }));
    assert_eq!(app.focus.shelf_pos(), 1, "a notch down is a shelf down");
    // A touchpad's pixels add up to a step.
    for _ in 0..3 {
        let _ = app.update(Message::HomeWheel(ScrollDelta::Pixels { x: 0.0, y: 25.0 }));
    }
    assert_eq!(app.focus.shelf_pos(), 0, "75 px up is one shelf");
    let _ = app.update(Message::HomeWheel(ScrollDelta::Lines { x: 0.0, y: 3.0 }));
    assert_eq!(app.focus.shelf_pos(), 0, "and stops at the first shelf");
    assert_eq!(app.chrome, None, "never climbing into the bar");
}

#[test]
fn the_header_says_what_is_coming_and_where_the_saves_stand() {
    use crate::ui::tab_bar::Saves;
    let (mut app, _) = App::new(Some(12));
    assert_eq!(app.header_status().downloads, None, "nothing coming: the chip names the page");
    assert_eq!(app.header_status().saves, Saves::Quiet, "nothing synced yet, so nothing to say");

    let games: Vec<Game> = app.library.all_games()[..2].to_vec();
    for game in &games {
        app.downloads.enqueue(QueueItem { rom_id: game.rom_id, title: game.title.clone(), system_id: game.system_id.clone(), ..Default::default() });
    }
    let first = app.downloads.start_next().expect("a download to start");
    app.downloads.progress(first.rom_id, 0.5);
    let (count, done) = app.header_status().downloads.expect("two coming");
    assert_eq!(count, 2);
    assert!((done - 0.25).abs() < 1e-4, "half of one of two: {done}");

    app.save_sync = SaveSync::Running;
    assert_eq!(app.header_status().saves, Saves::Syncing);
    app.save_sync = SaveSync::Failed("no RetroArch".into());
    assert_eq!(app.header_status().saves, Saves::Failed);
    let _ = app.view();
}

#[test]
fn a_cover_opened_from_its_grid_is_a_copy_of_its_own_not_the_tiles() {
    let (mut app, _) = App::new(Some(60));
    let mut games = app.library.all_games().to_vec();
    let url = format!("https://romm.lan/assets/romm/resources/roms/{}/cover/big.png", games[0].rom_id);
    games[0].cover_url = Some(url.clone());
    app.library = Arc::new(Library::new(app.library.systems.clone(), games.clone()));
    // What the grid holds: RomM's small copy, cut to the tile.
    let pixel = |v: u8| iced::widget::image::Handle::from_rgba(1, 1, vec![v; 4]);
    let (tile, page) = (pixel(1), pixel(2));
    let meta = covers::CoverMeta { ratio: 0.58, dominant: Color::BLACK };
    app.covers.insert(url.clone(), covers::Cover::Ready(tile.clone(), meta));

    let _ = app.update(Message::OpenDetail(games[0].rom_id));
    let wanted = app.wanted_art();
    let (_, target, kind) = wanted.iter().find(|(u, _, _)| *u == url).expect("the cover is asked for");
    assert_eq!(*kind, covers::Kind::Feature, "under a key of its own, so the grid's copy does not answer for it");
    // Drawn as tall as the facts beside it: RomM's small copy has the rows for that on a 1x panel,
    // and only its big copy on a 2x one.
    assert_eq!(covers::source_url(&url, *target), url.replace("big.png", "small.png"), "from RomM's small copy at 1x");
    app.panel_density = 2.0;
    let wanted = app.wanted_art();
    let (_, target, _) = wanted.iter().find(|(u, _, _)| *u == url).expect("the cover is asked for");
    assert_eq!(covers::source_url(&url, *target), url, "from RomM's big copy at 2x");

    // The tile's copy until the page's own has come, then that.
    assert_eq!(app.covers.feature(&url).map(|(h, _)| h), Some(tile));
    app.covers.insert(covers::key(&url, covers::Kind::Feature), covers::Cover::Ready(page.clone(), meta));
    assert_eq!(app.covers.feature(&url).map(|(h, _)| h), Some(page));
    let _ = app.view();
}

#[test]
fn the_spotlight_asks_for_its_picture_and_its_neighbours() {
    let (mut app, _) = App::new(Some(60));
    let mut games = app.library.all_games().to_vec();
    let around: Vec<i64> = app.shelves[0].games[..3].iter().map(|g| g.rom_id).collect();
    for game in &mut games {
        game.screenshots = vec![format!("https://romm.lan/shots/{}.jpg", game.rom_id)];
    }
    app.library = Arc::new(Library::new(app.library.systems.clone(), games));
    app.rebuild_shelves();
    let _ = app.update(Message::Nav(Direction::Right));
    let wanted = app.wanted_art();
    let pictures: Vec<&str> = wanted.iter().filter(|(_, _, kind)| *kind == covers::Kind::Spotlight).map(|(url, _, _)| url.as_str()).collect();
    assert_eq!(pictures.len(), 3, "the card the cursor is on and one either side: {pictures:?}");
    assert!(pictures[0].ends_with(&format!("{}.jpg", around[1])), "the one under the cursor first: {pictures:?}");
    assert!(wanted.iter().any(|(url, _, kind)| *kind == covers::Kind::Ambient && url.ends_with(&format!("{}.jpg", around[1]))));

    // Off Home there is no spotlight to ask for.
    let _ = app.update(Message::TabSelected(Tab::Settings));
    assert!(app.wanted_art().iter().all(|(_, _, kind)| *kind != covers::Kind::Spotlight));
}

#[test]
fn pressing_the_current_tab_returns_to_the_top() {
    let (mut app, _) = App::new(Some(60));
    for direction in [Direction::Down, Direction::Right] {
        let _ = app.update(Message::Nav(direction));
    }
    assert_eq!(app.focus.shelf_pos(), 1);

    let _ = app.update(Message::TabSelected(Tab::Home));
    assert_eq!(app.focus.shelf_pos(), 0, "back to the top of the page");
    assert!(app.focus.is_focused(&app.shelves, 0, 0), "on the game played last");
}

#[test]
fn the_login_screen_is_walked_and_typed_into_with_the_pad_alone() {
    use crate::gamepad::Pad;
    use crate::ui::login::Row;
    let (mut app, _) = App::new(Some(0));
    app.screen = Screen::Login(LoginState::new("https://".into()));
    let focus = |app: &App| match &app.screen {
        Screen::Login(state) => state.focus,
        _ => panic!("left the login screen"),
    };
    assert_eq!(focus(&app), Row::Server);

    // A on the address opens the on-screen keyboard; what it saves lands in the field, and the
    // cursor moves on to the QR button.
    let _ = app.update(Message::Pad(Pad::Confirm));
    assert!(app.entry.is_some());
    let _ = app.update(Message::KeyChar("romm.lan".into(), false));
    let _ = app.update(Message::Pad(Pad::Start));
    let Screen::Login(state) = &app.screen else { panic!() };
    assert_eq!(state.url, "https://romm.lan");
    assert_eq!(state.focus, Row::Qr);

    // The QR button asks RomM; a code shown waits on the phone, and B gives it up.
    assert!(app.update(Message::Pad(Pad::Confirm)).units() > 0, "asks RomM for a code");
    let generation = match &app.screen { Screen::Login(state) => state.qr_generation, _ => 0 };
    let pairing = crate::qr_pairing::start_for_test("https://romm.lan");
    assert!(app.update(Message::QrStarted(generation, Ok(pairing))).units() > 0, "and starts asking whether it is approved");
    let _ = app.view();
    let _ = app.update(Message::Pad(Pad::Back));
    let Screen::Login(state) = &app.screen else { panic!() };
    assert!(state.qr.is_none());
    let stale = state.qr_generation;
    assert_eq!(app.update(Message::QrPolled(stale - 1, crate::qr_pairing::Poll::Waiting)).units(), 0, "an answer to a request given up is dropped");

    // Down to the code, typed with the pad too.
    let _ = app.update(Message::Pad(Pad::Nav(Direction::Down)));
    assert_eq!(focus(&app), Row::Code);
    let _ = app.update(Message::Pad(Pad::Confirm));
    let _ = app.update(Message::KeyChar("abcdefgh".into(), false));
    let _ = app.update(Message::Pad(Pad::Start));
    let Screen::Login(state) = &app.screen else { panic!() };
    assert_eq!(state.code, "ABCDEFGH");
    assert_eq!(state.focus, Row::Connect);
    assert!(state.complete());
}

#[test]
fn an_approved_qr_code_pairs_with_the_device_it_was_approved_as() {
    let (mut app, _) = App::new(Some(0));
    app.writes_settings = false;
    app.screen = Screen::Login(LoginState::new("https://romm.lan".into()));
    let _ = app.update(Message::StartQr);
    let generation = match &app.screen { Screen::Login(state) => state.qr_generation, _ => 0 };
    let _ = app.update(Message::QrStarted(generation, Ok(crate::qr_pairing::start_for_test("https://romm.lan"))));
    let connection = Connection {
        host: "https://romm.lan".into(),
        token: "rmm_approved".into(),
        insecure: false,
        token_id: None,
        token_name: Some("Deckhand".into()),
        device_id: Some("dev-9".into()),
    };
    let _ = app.update(Message::QrPolled(generation, crate::qr_pairing::Poll::Approved(Box::new(connection))));
    assert!(matches!(app.screen, Screen::Loading { .. }), "on to loading the library");
    assert_eq!(app.settings.connection.as_ref().and_then(|c| c.device_id.as_deref()), Some("dev-9"));
}

#[test]
fn a_typed_pairing_code_is_capped_and_uppercased() {
    let (mut app, _) = App::new(Some(0));
    app.screen = Screen::Login(LoginState::new("https://romm.local".into()));
    let _ = app.update(Message::CodeChanged("abcd-efgh".into()));
    let Screen::Login(state) = &app.screen else { panic!("still on login") };
    assert_eq!(state.code, "ABCD-EFGH");
    assert!(state.complete(), "a grouped code of the right length is ready to send");

    // One character too many is refused rather than silently truncated mid-entry.
    let _ = app.update(Message::CodeChanged("ABCD-EFGHI".into()));
    let Screen::Login(state) = &app.screen else { panic!("still on login") };
    assert_eq!(state.code, "ABCD-EFGH", "the extra character is not accepted");
}

#[test]
fn the_system_list_opens_what_its_cursor_is_on_and_gives_back_the_grid_it_left() {
    use crate::focus::BrowseZone;
    let (mut app, _) = App::new(Some(60));
    let _ = app.update(Message::TabSelected(Tab::Browse));
    let open = |app: &App| match &app.screen {
        Screen::Browsing(Route::Browse { scope: Scope::System(system) }) => system.clone(),
        _ => panic!("not on Browse"),
    };
    let first = open(&app);
    let first_index = app.library.systems.iter().position(|s| s.id == first).unwrap();

    // Down a row, then off its left edge into the list.
    let _ = app.update(Message::Nav(Direction::Down));
    let tile = app.browse_focus.index();
    let _ = app.update(Message::Nav(Direction::Left));
    assert_eq!(app.browse_focus.zone(), BrowseZone::SystemList);
    assert_eq!(app.browse_focus.system, first_index, "the list opens on the system you are in");
    let _ = app.view();

    // Right straight back undoes it: same system, same tile, not reopened at the top.
    let _ = app.update(Message::Nav(Direction::Right));
    assert_eq!(app.browse_focus.zone(), BrowseZone::Grid);
    assert_eq!(app.browse_focus.index(), tile);
    assert_eq!(open(&app), first);

    // Walking the list moves only its cursor; Enter opens the system it is on.
    let _ = app.update(Message::Nav(Direction::Left));
    let _ = app.update(Message::Nav(Direction::Down));
    assert_eq!(open(&app), first, "walking the list opens nothing");
    let _ = app.update(Message::Activate);
    assert_eq!(open(&app), app.library.systems[first_index + 1].id);
    assert_eq!(app.browse_focus.zone(), BrowseZone::Grid, "and lands in its grid");
}

#[test]
fn a_hover_is_only_cleared_by_leaving_the_card_it_is_on() {
    let (mut app, _) = App::new(Some(60));
    let _ = app.update(Message::TabSelected(Tab::Browse));
    let (a, b) = (Cursor::BrowseTile(0), Cursor::BrowseTile(1));
    let _ = app.update(Message::Hovered(a));
    // Straight from one card to the next: the new card's enter can land before the old exit.
    let _ = app.update(Message::Hovered(b));
    let _ = app.update(Message::Unhovered(a));
    assert_eq!(app.hovered, Some(b));
    let _ = app.view();

    let _ = app.update(Message::Unhovered(b));
    assert_eq!(app.hovered, None);
}

#[test]
fn changing_page_drops_the_hover_rather_than_lighting_whatever_sits_there_next() {
    let (mut app, _) = App::new(Some(60));
    let _ = app.update(Message::Hovered(Cursor::HomeCard(0, 2)));
    let _ = app.view();
    let _ = app.update(Message::TabSelected(Tab::Browse));
    assert_eq!(app.hovered, None);
}

#[test]
fn every_scroll_changes_the_background_by_less_than_a_display_can_show() {
    // A background unchanged across a scroll is what drops tiny-skia back to drawing the frame
    // in pieces — see `App::style`. One changed by a visible amount would flicker.
    let (mut app, _) = App::new(Some(60));
    let theme = app.theme();
    let mut last = app.style(&theme).background_color;
    for message in [Message::Scrolled, Message::GridScrolled(40.0)].into_iter().cycle().take(128) {
        let _ = app.update(message);
        let now = app.style(&theme).background_color;
        assert_ne!(now, last, "a scroll left the background as it was");
        assert_eq!(now.into_rgba8(), theme::BACKGROUND.into_rgba8());
        last = now;
    }
}

#[test]
fn a_cursor_step_leaves_the_grid_to_the_layout_but_a_letter_still_goes_to_the_top() {
    let (mut app, _) = App::new(Some(60));
    let _ = app.update(Message::TabSelected(Tab::Browse));
    assert_eq!(app.browse_focus.zone(), crate::focus::BrowseZone::Grid);

    // A step no longer pins its row to the top. Whether the page moves at all is measured
    // against the real viewport, which only the runtime has, so the mirror waits for that.
    let _ = app.update(Message::Nav(Direction::Down));
    let _ = app.update(Message::Nav(Direction::Down));
    assert_eq!(app.grid_scroll, 0.0);

    // A letter is a jump to a heading, and still puts its row at the top.
    let Screen::Browsing(Route::Browse { scope }) = &app.screen else { panic!("on Browse") };
    let (spec, packing) = app.layout(scope);
    let target = packing.rows()[3].start;
    let _ = app.update(Message::JumpToLetter(target));
    assert_eq!(app.grid_scroll, crate::ui::browse::row_top(&spec, 3));
}

#[test]
fn the_settings_page_builds_whatever_it_has_to_report() {
    let (mut app, _) = App::new(Some(60));
    let _ = app.update(Message::TabSelected(Tab::Settings));
    assert!(matches!(app.screen, Screen::Browsing(Route::Settings)));

    // Unpaired, untested, still measuring — the page as it first opens in fixture mode.
    app.settings.connection = None;
    let _ = app.view();

    // Paired over plaintext, with every connection-test outcome and a measured cache.
    app.settings.connection = Some(Connection {
        host: "http://romm.lan".into(),
        token: "rmm_test".into(),
        insecure: false,
        token_id: Some(7),
        token_name: Some("Steam Deck".into()),
        device_id: None,
    });
    app.image_cache_bytes = Some(412 * 1024 * 1024);
    for check in [
        Check::Testing,
        Check::Reachable("mattias".into()),
        Check::Failed("Could not reach the server.".into()),
    ] {
        app.connection_check = check;
        let _ = app.view();
    }

    // Every state the Saves section can be in, including a run that failed on one save.
    for (config, folder) in [
        (Some("/home/deck/retrodeck/retroarch.cfg".into()), Ok("/home/deck/retrodeck/saves".into())),
        (None, Err("No RetroArch install found.".into())),
    ] {
        app.retroarch_found = Some(config);
        app.save_folder = Some(folder);
        let _ = app.view();
    }
    let report = crate::sync::Report {
        in_sync: 4,
        failed: vec!["Chrono Trigger (USA).srm: RomM refused it (HTTP 404)".into()],
        ..Default::default()
    };
    for state in [
        SaveSync::Running,
        SaveSync::Done(Box::new(report)),
        SaveSync::Done(Default::default()),
        SaveSync::Failed("Could not read the saves on RomM.".into()),
    ] {
        app.save_sync = state;
        let _ = app.view();
    }
}

fn settings_row(row: SettingRow) -> usize {
    SettingRow::ALL.iter().position(|r| *r == row).expect("a row on the page")
}

#[test]
fn settings_shows_a_section_at_a_time_walked_with_the_triggers() {
    use crate::ui::settings::{Row, Section};
    let (mut app, _) = App::new(Some(60));
    app.banner = None;
    let _ = app.update(Message::TabSelected(Tab::Settings));
    assert_eq!(app.settings_focus.section(), Section::Server);
    let _ = app.view();

    // Down stops at the section's last row: the next section is the triggers'.
    for _ in 0..10 {
        let _ = app.update(Message::Nav(Direction::Down));
    }
    assert_eq!(app.settings_focus.row(), Row::SignOut);
    let _ = app.update(Message::CycleSystem(1));
    assert_eq!((app.settings_focus.section(), app.settings_focus.row()), (Section::Playing, Row::Launcher), "R2: the next section, at its top");
    let _ = app.update(Message::CycleSystem(-1));
    assert_eq!(app.settings_focus.row(), Row::ServerAddress);

    // A click on the strip shows a section too; Up off its first row is Up into the bar.
    let _ = app.update(Message::SettingsSection(Section::Storage));
    assert_eq!(app.settings_focus.row(), Row::RomsFolder);
    let _ = app.view();
    let _ = app.update(Message::Nav(Direction::Up));
    assert_eq!(app.chrome, Some(Chrome::Tabs));
}

#[test]
fn x_syncs_the_saves_from_the_saves_section_and_nowhere_else() {
    use crate::gamepad::Pad;
    use crate::ui::settings::Section;
    let (mut app, _) = App::new(Some(60));
    app.settings.connection = None;
    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.update(Message::Pad(Pad::Erase));
    assert!(matches!(app.save_sync, SaveSync::Idle), "not from the server's section");
    let _ = app.update(Message::SettingsSection(Section::Saves));
    let _ = app.update(Message::Pad(Pad::Erase));
    assert!(matches!(&app.save_sync, SaveSync::Failed(why) if why.contains("not paired")), "X is the card's Sync now");
    let _ = app.view();
}

#[test]
fn syncing_saves_unpaired_says_why_instead_of_starting() {
    let (mut app, _) = App::new(Some(60));
    app.settings.connection = None;
    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.update(Message::SyncNow);
    assert!(matches!(&app.save_sync, SaveSync::Failed(why) if why.contains("not paired")));
}

#[test]
fn a_save_sync_runs_once_at_a_time_and_its_answer_lands_on_the_page() {
    let (mut app, _) = App::new(Some(60));
    app.settings.connection = Some(Connection {
        host: "https://romm.lan".into(),
        token: "rmm_test".into(),
        insecure: false,
        token_id: None,
        token_name: None,
        device_id: None,
    });
    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.update(Message::SyncNow);
    assert!(matches!(app.save_sync, SaveSync::Running));

    // Leaving and coming back while it runs does not lose it, or start a second.
    let _ = app.update(Message::TabSelected(Tab::Home));
    let _ = app.update(Message::TabSelected(Tab::Settings));
    assert!(matches!(app.save_sync, SaveSync::Running));

    let folder = std::path::PathBuf::from("/saves");
    let report = crate::sync::Report { folder: folder.clone(), in_sync: 2, ..Default::default() };
    assert!(app.save_synced_at.is_none(), "nothing has finished yet to count from");
    let _ = app.update(Message::SavesSynced(Ok(report)));
    assert!(matches!(&app.save_sync, SaveSync::Done(r) if r.in_sync == 2));
    assert!(app.save_synced_at.is_some_and(|at| at.elapsed().as_secs() < 5), "the card counts its \"ago\" from now");
    assert!(matches!(&app.save_folder, Some(Ok(f)) if *f == folder));

    // The next run keeps that one's outcome for the card to go on showing while it runs.
    let _ = app.update(Message::SyncNow);
    assert!(matches!(app.save_sync, SaveSync::Running));
    assert!(matches!(&app.sync_before, SaveSync::Done(r) if r.in_sync == 2));
    let _ = app.view();
}

fn release(version: &str) -> crate::release::Release {
    crate::release::Release {
        version: version.into(),
        headline: Some("New in this one".into()),
        appimage: Some(crate::release::Asset {
            name: "Deckhand.AppImage".into(),
            url: "https://repo.example/x".into(),
            size: 1,
            sha256: None,
        }),
        preview: false,
    }
}

#[test]
fn new_versions_are_looked_for_with_no_token_and_the_updates_row_asks_again() {
    use crate::release::Status;
    let (mut app, _) = App::new(Some(0));
    assert_eq!(app.release, Status::Unasked);
    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.view();
    assert_eq!(app.update(Message::CheckRelease).units(), 1, "asked");
    assert_eq!(app.release, Status::Checking);

    // The Updates row asks again after a failed ask, and before any.
    let _ = app.update(Message::ReleaseChecked(Err("could not reach api.github.com".into())));
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Updates)));
    assert!(app.entry.is_none());
    assert_eq!(app.release, Status::Checking);
    app.release = Status::Unasked;
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Updates)));
    assert_eq!(app.release, Status::Checking);
    let _ = app.view();
}

#[test]
fn a_field_takes_keys_the_pad_and_a_paste_and_is_cancelled_or_saved() {
    use crate::gamepad::Pad;
    let (mut app, _) = App::new(Some(0));
    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.type_launch_command();
    assert_eq!(app.entry.as_ref().unwrap().entry.text, "", "RetroDECK, so empty");

    // A keyboard types, the pad presses keys, and neither reaches the page underneath.
    let _ = app.update(Message::KeyChar("ab[".into(), false));
    let _ = app.update(Message::KeyChar(crate::search::BACKSPACE.to_string(), false));
    let _ = app.update(Message::CycleTab(1));
    assert!(matches!(app.screen, Screen::Browsing(Route::Settings)), "Tab does not leave the field");
    let _ = app.update(Message::Pad(Pad::Nav(Direction::Up)));
    let _ = app.update(Message::Pad(Pad::Confirm));
    assert_eq!(app.entry.as_ref().unwrap().entry.text, "ab/", "the key above Save");
    let _ = app.update(Message::Pad(Pad::Erase));
    let _ = app.update(Message::Pasted(Some(" cd\n".into())));
    let _ = app.view();
    let _ = app.update(Message::Activate);
    assert!(app.entry.is_none());
    assert_eq!(app.settings.launch_command, Some(vec!["abcd".to_string()]));

    // Cancelled, nothing changes; saved empty, the setting goes.
    let _ = app.type_launch_command();
    assert_eq!(app.entry.as_ref().unwrap().entry.text, "abcd", "opens on what is set");
    let _ = app.update(Message::KeyChar("zz".into(), false));
    let _ = app.update(Message::Pad(Pad::Back));
    assert_eq!(app.settings.launch_command, Some(vec!["abcd".to_string()]));
    let _ = app.type_launch_command();
    for _ in 0..4 {
        let _ = app.update(Message::Pad(Pad::Erase));
    }
    let _ = app.update(Message::Pad(Pad::Start));
    assert_eq!(app.settings.launch_command, None);
}

#[test]
fn a_launch_command_is_typed_in_and_a_broken_one_stays_open_saying_why() {
    let (mut app, _) = App::new(Some(0));
    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Launcher)));
    assert_eq!(app.pick.as_ref().unwrap().picker.cursor, 0, "on RetroDECK, the one in use");
    let _ = app.update(Message::Picked(1));
    assert!(app.entry.is_some(), "a command of one's own is typed");

    let _ = app.update(Message::KeyChar("retroarch -L '/my cores/snes9x.so".into(), false));
    let _ = app.update(Message::Activate);
    let open = &app.entry.as_ref().expect("still open").entry;
    assert!(open.error.as_deref().unwrap().contains("never closed"), "{:?}", open.error);
    let _ = app.update(Message::KeyChar("' {rom}".into(), false));
    let _ = app.update(Message::Activate);
    assert!(app.entry.is_none());
    let words = ["retroarch", "-L", "/my cores/snes9x.so", "{rom}"].map(String::from).to_vec();
    assert_eq!(app.settings.launch_command.as_ref(), Some(&words));
    let _ = app.view();

    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Launcher)));
    assert_eq!(app.pick.as_ref().unwrap().picker.cursor, 1, "on the command in use");
    let _ = app.update(Message::Picked(0));
    assert_eq!(app.settings.launch_command, None, "back to RetroDECK");
}

#[test]
fn a_system_gets_an_emulator_of_its_own_walked_to_typed_or_given_back() {
    let (mut app, _) = App::new(Some(60));
    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::SystemEmulators)));
    let Some(PendingPick { then: Then::SystemEmulators { systems }, .. }) = &app.pick else { panic!("the systems") };
    let psx = systems.iter().position(|s| s == "psx").expect("the library's PlayStation");
    assert_eq!(app.pick.as_ref().unwrap().picker.rows[psx].label, "PlayStation");
    let _ = app.update(Message::Picked(psx));
    let Some(PendingPick { then: Then::SystemEmulator { system, choices }, .. }) = &app.pick else { panic!("its emulator") };
    assert_eq!(system, "psx");
    assert!(matches!(choices[0], EmulatorChoice::Pick(_)), "walking to the file comes first");

    // Walked to: the options ES-DE starts it with, offered to check, then kept.
    let dir = crate::config::test_root().join("emulators");
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join(if cfg!(windows) { "duckstation-qt.exe" } else { "duckstation-qt" });
    std::fs::write(&program, "").unwrap();
    // Written as any download is, unable to run until it is made so.
    crate::folders::make_runnable(&program).unwrap();
    app.pick = None;
    let _ = app.browse(Browsing::Emulator("psx".into()), dir, None);
    let at = app.pick.as_ref().unwrap().picker.rows.iter().position(|r| r.label.starts_with("duckstation-qt")).expect("a program to pick");
    let _ = app.update(Message::Picked(at));
    let words = vec![program.display().to_string(), "-batch".into(), "{rom}".into()];
    assert_eq!(app.entry.as_ref().expect("the command, to check").entry.text, crate::launch::join_command(&words));
    let _ = app.update(Message::Activate);
    assert_eq!(app.settings.system_commands.get("psx"), Some(&words));
    assert!(app.banner.as_deref().unwrap().starts_with("PlayStation games now start with:"));
    let _ = app.view();

    // Typed instead, and given back.
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::SystemEmulators)));
    let _ = app.update(Message::Picked(psx));
    let choices = match &app.pick.as_ref().unwrap().then {
        Then::SystemEmulator { choices, .. } => choices.clone(),
        _ => panic!(),
    };
    let typed = choices.iter().position(|c| *c == EmulatorChoice::Type).unwrap();
    let _ = app.update(Message::Picked(typed));
    assert_eq!(app.entry.as_ref().unwrap().entry.text, crate::launch::join_command(&words), "starts from the one in use");
    let shared = choices.iter().position(|c| *c == EmulatorChoice::Shared).unwrap();
    app.entry = None;
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::SystemEmulators)));
    let _ = app.update(Message::Picked(psx));
    let _ = app.update(Message::Picked(shared));
    assert!(app.settings.system_commands.is_empty(), "back to the default emulator");
}

#[test]
fn downloads_can_go_to_any_folder_walked_to() {
    let root = std::env::temp_dir().join(format!("deckhand-walk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("games/roms")).unwrap();
    let (mut app, _) = App::new(Some(0));
    app.settings.roms_dir = Some(root.join("games"));
    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::RomsFolder)));
    let rows = &app.pick.as_ref().unwrap().picker.rows;
    assert_eq!(rows.last().unwrap().label, "Another folder…");
    let _ = app.update(Message::Picked(rows.len() - 1));

    let labels = |app: &App| app.pick.as_ref().unwrap().picker.rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
    assert_eq!(labels(&app), ["Use this folder", "Up", "roms/"], "starting in the folder in use");
    let _ = app.update(Message::Picked(2));
    assert_eq!(labels(&app), ["Use this folder", "Up"]);
    let _ = app.update(Message::Picked(1));
    assert_eq!(app.pick.as_ref().unwrap().picker.cursor, 2, "Up lands on the folder it came out of");
    let _ = app.update(Message::Picked(2));
    let _ = app.update(Message::Picked(0));
    assert!(app.pick.is_none());
    // What making the folder answers.
    let _ = app.update(Message::LocationSet(Ok(root.join("games/roms"))));
    assert_eq!(app.settings.roms_dir, Some(root.join("games/roms")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_retroarch_config_is_walked_to_and_read_before_it_is_kept() {
    let root = std::env::temp_dir().join(format!("deckhand-ra-walk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("ra")).unwrap();
    std::fs::write(root.join("ra/retroarch.cfg"), format!("savefile_directory = \"{}\"\n", root.join("saves").display())).unwrap();
    std::fs::write(root.join("ra/binary.cfg"), [0xffu8, 0xfe, 0x00]).unwrap();
    let (mut app, _) = App::new(Some(0));
    app.settings.retroarch_config = Some(root.join("ra/missing.cfg"));
    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::RetroArchConfig)));
    let rows = &app.pick.as_ref().unwrap().picker.rows;
    assert_eq!(rows[0].label, "Find it automatically");
    assert_eq!(rows.last().unwrap().label, "Another file…");
    assert!(rows.iter().any(|r| r.mark.is_some() && r.detail.ends_with("missing.cfg")), "the one set now is offered, marked");

    // Walked to from the folder the one set now is in.
    let _ = app.update(Message::Picked(rows.len() - 1));
    let labels: Vec<String> = app.pick.as_ref().unwrap().picker.rows.iter().map(|r| r.label.clone()).collect();
    assert_eq!(labels, ["Up", "binary.cfg", "retroarch.cfg"]);
    let _ = app.update(Message::Picked(1));
    assert!(app.banner.as_deref().unwrap().contains("can't be used"), "{:?}", app.banner);
    assert_eq!(app.settings.retroarch_config, Some(root.join("ra/missing.cfg")), "not kept");

    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::RetroArchConfig)));
    let last = app.pick.as_ref().unwrap().picker.rows.len() - 1;
    let _ = app.update(Message::Picked(last));
    let _ = app.update(Message::Picked(2));
    assert_eq!(app.settings.retroarch_config, Some(root.join("ra/retroarch.cfg")));
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::RetroArchConfig)));
    let _ = app.update(Message::Picked(0));
    assert_eq!(app.settings.retroarch_config, None, "back to finding it");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_new_version_is_announced_once_and_stays_offered_through_a_failed_ask() {
    use crate::release::Status;
    let (mut app, _) = App::new(Some(0));
    app.banner = None;

    let _ = app.update(Message::ReleaseChecked(Ok(release("99.0.0"))));
    assert!(matches!(&app.release, Status::Out(r) if r.version == "99.0.0"));
    let banner = app.banner.take().expect("announced");
    assert!(banner.contains("99.0.0") && banner.contains("/releases"), "not an AppImage, so where to get it: {banner}");

    // Six hours on: still offered while it is asked again, not announced a second time, and
    // still offered when the asking fails.
    let _ = app.update(Message::CheckRelease);
    assert!(matches!(app.release, Status::Out(_)));
    let _ = app.update(Message::ReleaseChecked(Ok(release("99.0.0"))));
    let _ = app.update(Message::ReleaseChecked(Err("could not reach repo.example".into())));
    assert!(matches!(&app.release, Status::Out(r) if r.version == "99.0.0"));
    assert_eq!(app.banner, None);

    // Not an AppImage, so pressing the row says where to get it rather than installing.
    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Updates)));
    assert!(matches!(app.release, Status::Out(_)));
    assert!(app.banner.as_deref().unwrap().contains("updated by hand"), "{:?}", app.banner);
    let _ = app.view();

    let _ = app.update(Message::ReleaseChecked(Ok(release("0.0.1"))));
    assert_eq!(app.release, Status::Current, "an older release is no news");
}

#[test]
fn an_install_reports_its_progress_and_a_restart_waits_for_the_game_being_played() {
    use crate::release::{Event, Status};
    let (mut app, _) = App::new(Some(60));
    app.banner = None;
    app.appimage = Some("/nowhere/Deckhand.AppImage".into());
    let _ = app.update(Message::ReleaseChecked(Ok(release("99.0.0"))));
    assert!(app.banner.as_deref().unwrap().contains("Settings ▸ About ▸ Updates installs it"), "{:?}", app.banner);

    let _ = app.update(Message::TabSelected(Tab::Settings));
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Updates)));
    assert!(matches!(app.release, Status::Installing(_, f) if f == 0.0));
    let _ = app.update(Message::ReleaseChecked(Ok(release("99.0.0"))));
    assert!(matches!(app.release, Status::Installing(..)), "an answer in flight does not undo it");
    let _ = app.update(Message::ReleaseEvent(Event::Progress(0.5)));
    assert!(matches!(app.release, Status::Installing(_, f) if f == 0.5));
    let _ = app.view();

    let _ = app.update(Message::ReleaseEvent(Event::Done(Err("the download answered 404".into()))));
    assert!(matches!(&app.release, Status::Failed(why, Some(_)) if why.contains("404")));
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Updates)));
    assert!(matches!(app.release, Status::Installing(..)), "a retry installs again");
    let _ = app.update(Message::ReleaseEvent(Event::Done(Ok(()))));
    assert!(matches!(&app.release, Status::Installed(r) if r.version == "99.0.0"));
    assert!(app.banner.as_deref().unwrap().contains("in place"), "{:?}", app.banner);

    // A game being played would lose its save sync to a restart.
    let rom_id = app.library.all_games()[0].rom_id;
    app.playing = Some(rom_id);
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Updates)));
    assert!(app.banner.as_deref().unwrap().contains("still running"), "{:?}", app.banner);
    assert!(matches!(app.release, Status::Installed(_)));
}

#[test]
fn arrows_walk_the_settings_rows_and_step_the_one_under_the_cursor() {
    let (mut app, _) = App::new(Some(60));
    app.settings.grid_density = 0;
    let _ = app.update(Message::TabSelected(Tab::Settings));
    assert_eq!(app.settings_focus.row(), SettingRow::ServerAddress, "opens on the first row");

    // `]` jumps a section at a time — past Server, Playing and Saves — to Display; Down lands on
    // grid density.
    for _ in 0..3 {
        let _ = app.update(Message::CycleSystem(1));
    }
    let _ = app.update(Message::Nav(Direction::Down));
    assert_eq!(app.settings_focus.row(), SettingRow::GridDensity);

    let _ = app.update(Message::Nav(Direction::Right));
    assert_eq!(app.settings.grid_density, 1, "Right steps to More");
    for _ in 0..5 {
        let _ = app.update(Message::Nav(Direction::Right));
    }
    assert_eq!(app.settings.grid_density, 2, "and stops at Most rather than wrapping");
    let _ = app.update(Message::Activate);
    assert_eq!(app.settings.grid_density, -1, "Confirm cycles, so it does wrap");
    let _ = app.view();

    // Left/Right on a row with nothing to step is a no-op, not a cursor move.
    let _ = app.update(Message::CycleSystem(1));
    assert_eq!(app.settings_focus.row(), SettingRow::RomsFolder, "the first row of Storage");
    let _ = app.update(Message::Nav(Direction::Left));
    assert_eq!(app.settings_focus.row(), SettingRow::RomsFolder);
}

#[test]
fn every_destination_is_reached_with_the_shoulders_and_with_the_d_pad_alone() {
    use crate::gamepad::Pad;
    let (mut app, _) = App::new(Some(60));
    let tab = |app: &App| match &app.screen {
        Screen::Browsing(route) => route.tab(),
        _ => panic!("not browsing"),
    };
    // The shoulders walk all six, wrapping.
    let mut seen = vec![tab(&app)];
    for _ in 0..6 {
        let _ = app.update(Message::Pad(Pad::NextTab));
        seen.push(tab(&app));
    }
    assert_eq!(seen, [Tab::Home, Tab::Browse, Tab::Collections, Tab::Search, Tab::Downloads, Tab::Settings, Tab::Home]);
    let _ = app.update(Message::Pad(Pad::PrevTab));
    assert_eq!(tab(&app), Tab::Settings);

    // The d-pad: up off the top of a page is the tab bar; along it, each destination opens.
    app.banner = None;
    let _ = app.update(Message::TabSelected(Tab::Home));
    for _ in 0..12 {
        let _ = app.update(Message::Pad(Pad::Nav(Direction::Up)));
    }
    assert_eq!(app.chrome, Some(Chrome::Tabs), "up off the top of Home is the bar");
    for _ in 0..5 {
        let _ = app.update(Message::Pad(Pad::Nav(Direction::Right)));
    }
    assert_eq!(tab(&app), Tab::Settings, "Downloads and Settings are on the way");
    assert_eq!(app.chrome, Some(Chrome::Tabs), "the cursor stays on the bar while walking it");
    let _ = app.view();
    let _ = app.update(Message::Pad(Pad::Nav(Direction::Down)));
    assert_eq!(app.chrome, None, "down is into the page");
    assert_eq!(app.settings_focus.row(), SettingRow::ServerAddress);
    let _ = app.update(Message::Pad(Pad::Nav(Direction::Up)));
    assert_eq!(app.chrome, Some(Chrome::Tabs), "and up from its first row is the bar again");
    let _ = app.update(Message::Pad(Pad::Back));
    assert_eq!(app.chrome, None, "B goes back down into the page");

    // The banner's Dismiss is above the bar, and dismisses — nothing else.
    app.banner = Some("Something to read.".into());
    let _ = app.update(Message::Pad(Pad::Nav(Direction::Up)));
    let _ = app.update(Message::Pad(Pad::Nav(Direction::Up)));
    assert_eq!(app.chrome, Some(Chrome::Banner));
    let _ = app.view();
    let _ = app.update(Message::Pad(Pad::Confirm));
    assert_eq!(app.banner, None);
    assert_eq!(app.chrome, Some(Chrome::Tabs), "back on the bar");
    assert_eq!(tab(&app), Tab::Settings, "still where it was");
}

#[test]
fn games_found_in_the_folder_count_as_here_unless_a_download_of_theirs_is_under_way() {
    let (mut app, _) = App::new(Some(60));
    app.banner = None;
    let remote: Vec<Game> =
        app.library.all_games().iter().filter(|g| app.downloads.state(g.rom_id) == crate::downloads::SyncState::RemoteOnly).take(2).cloned().collect();
    let (left, fetching) = (&remote[0], &remote[1]);
    app.downloads.enqueue(QueueItem { rom_id: fetching.rom_id, title: fetching.title.clone(), system_id: fetching.system_id.clone(), ..Default::default() });
    let _ = app.update(Message::GamesFound(vec![installed(left.rom_id, &left.title, 1024), installed(fetching.rom_id, &fetching.title, 512)]));
    assert_eq!(app.downloads.state(left.rom_id), crate::downloads::SyncState::Installed);
    assert_eq!(app.downloads.state(fetching.rom_id), crate::downloads::SyncState::Downloading(0.0), "its download records it when done, not the look");
    assert_eq!(app.banner.as_deref(), Some("Found a game already in the downloads folder: it's marked as on this device."));
    // Found again on the next look: already recorded, so nothing new and nothing said.
    app.banner = None;
    let _ = app.update(Message::GamesFound(vec![installed(left.rom_id, &left.title, 1024)]));
    assert_eq!(app.banner, None);
}

#[test]
fn up_from_anywhere_on_browse_reaches_the_bar_with_the_pointer_resting_on_a_tile() {
    use crate::gamepad::Pad;
    // Random walks over Browse — the grid, the chips, the system list, the rail, filters, other
    // systems — each followed by Up until the bar. A pointer resting on the grid makes a tile
    // report it entering after every press, as the page is rebuilt under it; that once pulled
    // the cursor straight back off the bar, so it is in every walk.
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for round in 0..150 {
        let (mut app, _) = App::new(Some(60));
        app.banner = None;
        let _ = app.update(Message::TabSelected(Tab::Browse));
        let mut walk = Vec::new();
        let resting = Message::Hovered(Cursor::BrowseTile(next(8) as usize));
        for _ in 0..(5 + next(40)) {
            let pad = match next(9) {
                0 => Pad::Nav(Direction::Up),
                1 | 2 => Pad::Nav(Direction::Down),
                3 | 4 => Pad::Nav(Direction::Left),
                5 | 6 => Pad::Nav(Direction::Right),
                7 => Pad::NextSystem,
                _ => Pad::Confirm,
            };
            walk.push(format!("{pad:?}"));
            let _ = app.update(Message::Pad(pad));
            let _ = app.update(resting.clone());
            if !matches!(app.screen, Screen::Browsing(Route::Browse { .. })) || app.chrome.is_some() {
                break;
            }
        }
        if !matches!(app.screen, Screen::Browsing(Route::Browse { .. })) || app.chrome.is_some() {
            continue;
        }
        for _ in 0..100 {
            let _ = app.update(Message::Pad(Pad::Nav(Direction::Up)));
            let _ = app.update(resting.clone());
            if app.chrome.is_some() {
                break;
            }
        }
        assert_eq!(app.chrome, Some(Chrome::Tabs), "round {round}: stuck on the page after {walk:?}: {:?}", app.browse_focus);
        let _ = app.update(Message::PointerUsed);
        assert_eq!(app.chrome, None, "the mouse in use is the page in use");
    }
}

#[test]
fn cancel_stops_a_running_download_and_takes_a_queued_one_off_the_queue() {
    use crate::downloads::SyncState;
    let (mut app, _) = App::new(Some(12));
    let games: Vec<Game> = app.library.all_games().to_vec();
    for g in &games[..2] {
        app.downloads.enqueue(QueueItem { rom_id: g.rom_id, title: g.title.clone(), system_id: g.system_id.clone(), ..Default::default() });
    }
    app.downloads.start_next();
    let _ = app.update(Message::RemoveDownload(games[0].rom_id));
    let _ = app.update(Message::RemoveDownload(games[1].rom_id));
    assert_eq!(app.downloads.state(games[0].rom_id), SyncState::RemoteOnly, "the running one stopped");
    assert_eq!(app.downloads.state(games[1].rom_id), SyncState::RemoteOnly, "the queued one is gone");
}

#[test]
fn search_results_are_kept_per_query_and_follow_a_new_library() {
    let (mut app, _) = App::new(Some(12));
    let _ = app.update(Message::TabSelected(Tab::Search));
    let title = app.library.all_games()[0].title.clone();
    let word: String = title.split_whitespace().next().unwrap().chars().filter(|c| c.is_alphanumeric()).collect();
    let _ = app.update(Message::KeyChar(word, false));
    let first: Vec<i64> = app.search_results().iter().map(|g| g.rom_id).collect();
    assert!(!first.is_empty());
    assert_eq!(app.search_results().iter().map(|g| g.rom_id).collect::<Vec<_>>(), first, "asked again, the same");
    app.library = Arc::new(Library::new(app.library.systems.clone(), Vec::new()));
    assert!(app.search_results().is_empty(), "a new library is searched, not the old one's index");
}

#[test]
fn browse_follows_its_filter_the_favourites_and_a_play_without_rebuilding_every_frame() {
    use crate::view_model::{Filter, Sort};
    let (mut app, _) = App::new(Some(30));
    let id = app.library.systems[0].id.clone();
    let scope = Scope::System(id.clone());
    let _ = app.update(Message::OpenSystem(id.clone()));
    let all = app.scope_view(&scope).0.len();
    assert_eq!(app.chip_counts(&scope).all, all);

    let _ = app.update(Message::FilterSelected(Filter::Favourites));
    let before = app.chip_counts(&scope).favourites;
    let first = app.library.games_for(&id)[0].rom_id;
    app.favourites.insert(first);
    assert_eq!(app.chip_counts(&scope).favourites, before + 1, "a new favourite is counted");
    assert!(app.scope_view(&scope).0.position_of(first).is_some(), "and shown");

    let _ = app.update(Message::FilterSelected(Filter::All));
    let _ = app.update(Message::SortSelected(Sort::Played));
    let last = app.library.games_for(&id).last().unwrap().rom_id;
    Arc::make_mut(&mut app.library).mark_played(last, "2099-01-01T00:00:00Z".into());
    assert_eq!(app.scope_view(&scope).0.games[0].rom_id, last, "a game just played leads the played sort");
    assert_eq!(app.scope_view(&scope).0.len(), all);
}

/// A window's size as iced 0.14 reports it: the physical size over the system's scale *and* over
/// the scale the app last handed iced — asked for here, the way iced asks at the end of an update.
fn reported(app: &App, physical: (f32, f32), system: f32) -> Message {
    let applied = app.scale_factor();
    Message::Resized(physical.0 / (system * applied), physical.1 / (system * applied))
}

#[test]
fn filling_the_screen_fits_the_design_whatever_scale_the_system_reports() {
    let (mut app, _) = App::new(Some(12));
    let _ = app.set_ui_scale(crate::theme::ui_scale::DEFAULT);
    app.set_fullscreen(true);
    let units = |app: &App| (app.content_width.round(), app.content_height.round());
    // How big the page is drawn: the system's scale times this app's.
    let drawn = |app: &App, system: f32| system * app.scale_factor();

    // A Deck in Game Mode: its 1280x800 panel, at a system scale of 2. The page it was designed for.
    let _ = app.update(reported(&app, (1280.0, 800.0), 2.0));
    assert_eq!(units(&app), (1280.0, 800.0));
    assert!((drawn(&app, 2.0) - 1.0).abs() < 1e-3, "drawn at {}", drawn(&app, 2.0));
    // And it stays there however often the screen says so. Fitted against iced's sizes as they
    // came, each report was divided by the scale the last one set, and the page swung between 87%
    // and twice its size with every resize.
    for _ in 0..5 {
        let _ = app.update(reported(&app, (1280.0, 800.0), 2.0));
        assert_eq!(units(&app), (1280.0, 800.0));
    }

    // A 4K TV at a system scale of 1: the same 800 units of height a little wider, drawn 2.7 times
    // over — the Deck's page at the TV's size.
    let _ = app.update(reported(&app, (3840.0, 2160.0), 1.0));
    assert_eq!(units(&app), (1422.0, 800.0));
    assert!((drawn(&app, 1.0) - 2.7).abs() < 1e-3, "drawn at {}", drawn(&app, 1.0));
    let _ = app.update(reported(&app, (3840.0, 2160.0), 1.0));
    assert_eq!(units(&app), (1422.0, 800.0), "steady on the next report too");

    // A larger step is larger type on the same screen, not a window resized under the screen.
    let _ = app.set_ui_scale(1.5);
    let _ = app.update(reported(&app, (3840.0, 2160.0), 1.0));
    assert_eq!(app.content_height.round(), (800.0 * crate::theme::ui_scale::DEFAULT / 1.5_f32).round());
    // A window of its own is sized to the design, and the UI scale alone scales it.
    app.set_fullscreen(false);
    assert!((app.scale_factor() - 1.5).abs() < 1e-4);
}

#[test]
fn a_size_reported_under_an_old_scale_is_asked_again_rather_than_kept() {
    let (mut app, _) = App::new(Some(12));
    app.set_fullscreen(true);
    // Two reports of a Deck's panel, both taken under the scale the window opened at — the size
    // asked for at launch and the screen's own resize — and read one update apart.
    let first = reported(&app, (1280.0, 800.0), 2.0);
    let second = reported(&app, (1280.0, 800.0), 2.0);
    let _ = app.update(first);
    let _ = app.scale_factor(); // iced applies the new scale at the end of the update
    let _ = app.update(second); // and the second is read under it: wrong, for now
    // The scale moved, so the size is asked again; the answers settle it.
    for _ in 0..3 {
        let _ = app.update(reported(&app, (1280.0, 800.0), 2.0));
    }
    assert_eq!((app.content_width.round(), app.content_height.round()), (1280.0, 800.0));
    assert!((2.0 * app.scale_factor() - 1.0).abs() < 1e-3);
}

#[test]
fn a_banner_goes_by_itself_once_it_has_been_up_long_enough() {
    use crate::gamepad::Pad;
    use std::time::Instant;
    let (mut app, _) = App::new(Some(60));
    app.say("Something to read.".into());
    let _ = app.update(Message::BannerTick);
    assert!(app.banner.is_some(), "not yet");
    app.banner_since = Some(Instant::now() - BANNER_LASTS);
    let _ = app.update(Message::BannerTick);
    assert_eq!(app.banner, None);

    // The same words said again are up for as long again.
    app.say("Something to read.".into());
    app.banner_since = Some(Instant::now() - BANNER_LASTS / 2);
    app.say("Something to read.".into());
    app.banner_since = app.banner_since.map(|since| since - BANNER_LASTS / 2);
    let _ = app.update(Message::BannerTick);
    assert!(app.banner.is_some(), "its time started over");

    // One set without `say` has its time start at the first look.
    app.banner = Some("Set by hand.".into());
    app.banner_since = None;
    let _ = app.update(Message::BannerTick);
    assert!(app.banner.is_some() && app.banner_since.is_some());

    // Going with the pad's cursor on its Dismiss leaves the cursor on the bar.
    let _ = app.update(Message::TabSelected(crate::ui::tab_bar::Tab::Settings));
    let _ = app.update(Message::Pad(Pad::Nav(Direction::Up)));
    let _ = app.update(Message::Pad(Pad::Nav(Direction::Up)));
    assert_eq!(app.chrome, Some(Chrome::Banner));
    app.banner_since = Some(Instant::now() - BANNER_LASTS);
    let _ = app.update(Message::BannerTick);
    assert_eq!((app.banner.as_deref(), app.chrome), (None, Some(Chrome::Tabs)));
}

#[test]
fn the_banners_dismiss_hides_it_without_leaving_the_page() {
    let (mut app, _) = App::new(Some(60));
    let rom_id = app.library.all_games()[0].rom_id;
    let _ = app.update(Message::OpenDetail(rom_id));
    app.banner = Some("Something to read.".into());
    let _ = app.update(Message::DismissBanner);
    assert_eq!(app.banner, None);
    assert!(matches!(&app.screen, Screen::Browsing(Route::Detail { rom_id: open, .. }) if *open == rom_id), "still on the game");
    // On a detail page scrolled down, Up reads back up the page before it reaches the bar.
    let _ = app.update(Message::DetailScrolled(320.0));
    assert!(app.update(Message::Nav(Direction::Up)).units() > 0);
    assert_eq!(app.chrome, None);
    let _ = app.update(Message::DetailScrolled(0.0));
    let _ = app.update(Message::Nav(Direction::Up));
    assert_eq!(app.chrome, Some(Chrome::Tabs));
}

#[test]
fn search_is_a_tab_and_goes_back_to_the_search_a_game_was_opened_from() {
    let (mut app, _) = App::new(Some(60));
    let tab = |app: &App| match &app.screen {
        Screen::Browsing(route) => Some(route.tab()),
        _ => None,
    };
    let _ = app.update(Message::TabSelected(Tab::Search));
    assert!(matches!(&app.screen, Screen::Browsing(Route::Search { from }) if **from == Route::Home));
    assert_eq!(tab(&app), Some(Tab::Search), "lit in the bar");

    let title = app.library.all_games()[0].title.clone();
    let word: String = title.split_whitespace().next().unwrap().chars().filter(|c| c.is_alphanumeric()).collect();
    let _ = app.update(Message::KeyChar(word.clone(), false));
    let _ = app.update(Message::SearchResultClicked(0));
    assert!(matches!(app.screen, Screen::Browsing(Route::Detail { .. })));
    assert_eq!(tab(&app), Some(Tab::Search), "a game opened from it stays under it");

    let _ = app.update(Message::TabSelected(Tab::Search));
    assert!(matches!(app.screen, Screen::Browsing(Route::Search { .. })), "back to the results");
    assert_eq!(app.search_focus.query, word.to_lowercase(), "query intact");
    let _ = app.update(Message::Dismiss);
    assert!(matches!(app.screen, Screen::Browsing(Route::Home)), "Esc goes where it was opened from");
}

#[test]
fn a_ui_scale_change_keeps_the_layout_the_size_it_was_once_the_window_follows() {
    let (mut app, _) = App::new(Some(60));
    app.settings.ui_scale = 1.15;
    // A window of its own at the design: 1280x800 layout units at 1.15, on a system scale of 1.
    let _ = app.update(reported(&app, (1280.0 * 1.15, 800.0 * 1.15), 1.0));
    assert!((app.content_width - 1280.0).abs() < 0.01, "{}", app.content_width);
    let _ = app.update(Message::TabSelected(Tab::Settings));

    let _ = app.update(Message::SettingsPick(SettingRow::UiScale, 3));
    assert_eq!(app.settings.ui_scale, 1.5);
    // Until the window grows, the layout is what the old window holds at the new scale — and
    // on a window that cannot grow, that is all it will ever hold.
    assert!((app.content_width - 1280.0 * 1.15 / 1.5).abs() < 0.01);

    // The resize lands: iced made the window 1280x800 layout units at the new scale.
    let _ = app.update(reported(&app, (1280.0 * 1.5, 800.0 * 1.5), 1.0));
    assert!((app.content_width - 1280.0).abs() < 0.01);
    assert!((app.content_height - 800.0).abs() < 0.01);
}

#[test]
fn a_favourite_shows_at_once_and_goes_back_if_romm_says_no() {
    use crate::romm::models::Collection;
    use crate::ui::detail::Action;
    let (mut app, _) = App::new(Some(30));
    let game = app.library.all_games()[2].clone();
    let other = app.library.all_games()[5].clone();
    let _ = app.update(Message::OpenDetail(game.rom_id));
    app.banner = None;

    // In fixture mode there is no RomM to tell: the press is simply so, and says no reason why not.
    let _ = app.update(Message::DetailAction(Action::Favourite));
    assert!(app.favourites.contains(&game.rom_id));
    assert_eq!(app.banner, None, "favourites are written now, not explained away");
    let _ = app.view();
    let _ = app.update(Message::DetailAction(Action::Favourite));
    assert!(!app.favourites.contains(&game.rom_id), "and a second press takes it back out");

    // A press while RomM has the last one still in hand waits for it.
    app.favouriting.insert(game.rom_id);
    let _ = app.update(Message::DetailAction(Action::Favourite));
    assert!(!app.favourites.contains(&game.rom_id));

    // RomM refuses: back the way it was, and the banner says why.
    app.favourites.insert(game.rom_id);
    let _ = app.update(Message::FavouriteChanged { tile: game.rom_id, adding: true, ids: vec![game.rom_id], result: Err("HTTP 403".into()) });
    assert!(!app.favourites.contains(&game.rom_id));
    assert!(app.banner.as_deref().is_some_and(|b| b.contains("could not be added to favourites") && b.contains("HTTP 403")), "{:?}", app.banner);
    assert!(!app.favouriting.contains(&game.rom_id), "and the next press is free to go");

    // RomM agrees: its collection as it now is, with what another device favourited meanwhile.
    let collection = Collection {
        id: 9,
        name: Some("Favourites".into()),
        rom_count: 2,
        rom_ids: vec![game.rom_id, other.rom_id],
        is_favorite: true,
        is_virtual: false,
        is_smart: false,
    };
    let _ = app.update(Message::FavouriteChanged { tile: game.rom_id, adding: true, ids: vec![game.rom_id], result: Ok(collection) });
    assert!(app.favourites.contains(&game.rom_id) && app.favourites.contains(&other.rom_id));
    assert!(app.collections.iter().any(|c| c.is_favorite && c.id == 9), "kept, so the next press goes to it");
    let _ = app.view();
}

#[test]
fn a_failed_collections_refresh_keeps_the_ones_on_screen() {
    use crate::romm::models::VirtualCollection;
    let (mut app, _) = App::new(Some(30));
    let ids: Vec<i64> = app.library.all_games()[..3].iter().map(|g| g.rom_id).collect();
    let genre = VirtualCollection { id: "genre-action".into(), name: Some("Action".into()), kind: "genre".into(), rom_ids: ids };
    let _ = app.update(Message::CollectionsLoaded(Ok((Vec::new(), vec![genre]))));
    let kinds = app.all_collections.kinds().len();
    assert!(kinds > 0, "the genre is a kind on the page");

    // The refresh after a sync fails: what was there stays, rather than an error in its place.
    let _ = app.update(Message::CollectionsLoaded(Err("timed out".into())));
    assert!(matches!(app.collection_sources, Some(Ok(_))));
    assert_eq!(app.all_collections.kinds().len(), kinds);

    // With nothing to show, the page says why.
    let (mut fresh, _) = App::new(Some(30));
    let _ = fresh.update(Message::CollectionsLoaded(Err("timed out".into())));
    assert!(matches!(fresh.collection_sources, Some(Err(_))));
}

/// Where a page stays scrolled to as the window changes around it — what a person sees between two
/// frames, and so what only a window kept from one frame to the next can show. iced keeps each
/// scrollable's offset in the widget tree, by the scrollable's place in it, and a view that moves
/// the page to another place starts it over at the top.
mod page_place {
    use super::*;
    use iced::widget::scrollable::AbsoluteOffset;
    use iced_test::core::widget::{operation, Id, Operation};
    use iced_test::core::{Rectangle, Size, Vector};
    use iced_test::runtime::user_interface::{Cache, UserInterface};

    /// The window as iced keeps it between frames: each view built over the state the last one
    /// left, the way the running app's is.
    struct Window {
        renderer: iced::Renderer,
        cache: Cache,
    }

    impl Window {
        fn new() -> Self {
            use iced_test::core::renderer::Headless;
            let renderer = iced_test::futures::futures::executor::block_on(<iced::Renderer as Headless>::new(
                crate::theme::REGULAR,
                iced::Pixels(16.0),
                None,
            ))
            .expect("a headless renderer");
            Self { renderer, cache: Cache::default() }
        }

        /// Draw `app` as the next frame, and run `operation` on it.
        fn frame(&mut self, app: &App, operation: &mut dyn Operation) {
            let size = Size::new(1280.0, 800.0);
            let mut ui = UserInterface::build(app.view(), size, std::mem::take(&mut self.cache), &mut self.renderer);
            ui.operate(&self.renderer, operation);
            self.cache = ui.into_cache();
        }

        /// Scroll the scrollable `id` to `y`, as a wheel or the cursor would.
        fn scroll(&mut self, app: &App, id: Id, y: f32) {
            self.frame(app, &mut operation::scrollable::scroll_to(id, AbsoluteOffset { x: None, y: Some(y) }));
        }

        /// How far down the scrollable `id` is, drawing `app` as the next frame.
        fn offset(&mut self, app: &App, id: Id) -> f32 {
            struct Offset {
                id: Id,
                y: Option<f32>,
            }
            impl Operation for Offset {
                fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                    operate(self);
                }
                fn scrollable(&mut self, id: Option<&Id>, _: Rectangle, _: Rectangle, at: Vector, _: &mut dyn operation::Scrollable) {
                    if id == Some(&self.id) {
                        self.y = Some(at.y);
                    }
                }
            }
            let mut offset = Offset { id, y: None };
            self.frame(app, &mut offset);
            offset.y.expect("the page is on screen")
        }
    }

    /// The Downloads page with thirty games on it: a list that runs well past the window.
    fn long_page() -> (App, Id) {
        let mut app = paired_fixtures(40);
        app.banner = None;
        let games: Vec<Game> = app.library.all_games()[..30].to_vec();
        for game in &games {
            app.downloads.finish(game.rom_id, Ok(installed(game.rom_id, &game.title, 1 << 20)));
        }
        let _ = app.update(Message::TabSelected(Tab::Downloads));
        (app, crate::ui::downloads::downloads_id())
    }

    #[test]
    fn a_banner_coming_or_going_leaves_the_page_where_it_was() {
        let (mut app, page) = long_page();
        let mut window = Window::new();
        window.scroll(&app, page.clone(), 600.0);

        // Each a step a person sees: an update found and its banner timing out, a second one.
        app.say("Deckhand 9.9.9 is out".into());
        assert_eq!(window.offset(&app, page.clone()), 600.0, "as the banner comes");
        app.banner = None;
        assert_eq!(window.offset(&app, page.clone()), 600.0, "as it goes");
        app.say("Deckhand 9.9.9 is in place".into());
        assert_eq!(window.offset(&app, page), 600.0, "and as another comes");
    }

    #[test]
    fn removing_the_last_game_on_downloads_leaves_the_list_at_its_end() {
        let mut app = paired_fixtures(40);
        app.banner = None;
        let games: Vec<Game> = app.library.all_games()[..30].to_vec();
        for (i, game) in games.iter().enumerate() {
            app.downloads.finish(game.rom_id, Ok(installed(game.rom_id, &game.title, (100 - i as u64) << 20)));
        }
        let _ = app.update(Message::TabSelected(Tab::Downloads));
        for _ in 1..games.len() {
            let _ = app.update(Message::Nav(Direction::Down));
        }
        let list = crate::ui::downloads::downloads_id();
        let mut window = Window::new();
        window.scroll(&app, list.clone(), 10_000.0);
        let end = window.offset(&app, list.clone());
        assert!(end > 0.0, "thirty games run past the window");

        let _ = app.update(Message::RemoveFocusedDownload);
        let _ = app.update(Message::DownloadRemoved(games[29].title.clone(), Ok(1)));
        let shorter = window.offset(&app, list.clone());
        assert!(shorter > end - 100.0, "a row shorter, still at the end, not at the top: {shorter}");
        app.banner = None;
        // The list has the banner's height back, so its end is that much further up.
        let after = window.offset(&app, list);
        assert!(after > shorter - 100.0, "still at the end once the banner goes, not at the top: {after}");
    }

    #[test]
    fn closing_a_picker_leaves_the_page_where_it_was() {
        let (mut app, page) = long_page();
        let mut window = Window::new();
        window.scroll(&app, page.clone(), 600.0);

        app.pick = Some(PendingPick {
            picker: crate::ui::picker::Picker { title: "Emulator".into(), note: String::new(), rows: Vec::new(), cursor: 0 },
            then: Then::Remove { builds: Vec::new() },
        });
        window.frame(&app, &mut operation::scrollable::scroll_by(Id::new("nothing"), AbsoluteOffset::default()));
        let _ = app.update(Message::Dismiss);
        assert!(app.pick.is_none());
        assert_eq!(window.offset(&app, page), 600.0);
    }
}

// ---- RetroDECK, offered and installed ----------------------------------------------------------

fn retrodeck_look(presence: crate::retrodeck::Presence, scope: Option<crate::retrodeck::Scope>) -> crate::retrodeck::Look {
    crate::retrodeck::Look { presence, scope, free: Some(40_000_000_000) }
}

#[test]
fn retrodeck_is_only_offered_once_paired() {
    use crate::retrodeck::{Presence, Scope};
    // Signed out: nothing is offered, wherever it is asked.
    let (mut app, _) = App::new(Some(12));
    assert!(app.settings.connection.is_none());
    let _ = app.update(Message::RetroDeckLooked(Offering::Pairing, retrodeck_look(Presence::Missing, Some(Scope::System))));
    let _ = app.update(Message::RetroDeckLooked(Offering::NoEmulator, retrodeck_look(Presence::Missing, Some(Scope::System))));
    assert!(app.pick.is_none(), "no RomM, nothing for RetroDECK to play");
    let _ = app.pick_launcher();
    assert_eq!(app.pick.as_ref().unwrap().picker.rows.len(), 2, "and Default emulator has no install");

    // Paired: an install to say yes to, and what it costs.
    let mut app = paired_fixtures(12);
    let _ = app.update(Message::RetroDeckLooked(Offering::Pairing, retrodeck_look(Presence::Missing, Some(Scope::System))));
    let pick = app.pick.as_ref().expect("the offer");
    assert_eq!(pick.picker.title, "Install RetroDECK?");
    let download = format!("{} to download", downloads::human_bytes(crate::retrodeck::DOWNLOAD_BYTES));
    assert!(pick.picker.note.contains(&download), "{}", pick.picker.note);
    assert_eq!(pick.picker.rows[0].label, "Install RetroDECK");
    assert!(matches!(pick.then, Then::RetroDeck(Some(Scope::System))));

    // Installed but never set up: its setup is what is offered.
    app.pick = None;
    let _ = app.update(Message::RetroDeckLooked(Offering::Asked, retrodeck_look(Presence::NotSetUp, None)));
    assert_eq!(app.pick.as_ref().unwrap().picker.title, "Set up RetroDECK?");
    assert!(matches!(app.pick.as_ref().unwrap().then, Then::RetroDeck(None)));

    // Nowhere to install it from: quiet after a pairing, said when asked.
    app.pick = None;
    app.banner = None;
    let _ = app.update(Message::RetroDeckLooked(Offering::Pairing, retrodeck_look(Presence::Missing, None)));
    assert!(app.pick.is_none() && app.banner.is_none(), "{:?}", app.banner);
    let _ = app.update(Message::RetroDeckLooked(Offering::Asked, retrodeck_look(Presence::Missing, None)));
    assert!(app.banner.as_deref().is_some_and(|b| b.contains("Flatpak")), "{:?}", app.banner);

    // And Default emulator offers it.
    let _ = app.pick_launcher();
    let rows = &app.pick.as_ref().unwrap().picker.rows;
    assert_eq!(rows.last().map(|r| r.label.as_str()), Some("Install RetroDECK…"));
}

#[test]
fn pairing_offers_retrodeck_once_the_library_is_in() {
    let (mut app, _) = App::new(Some(12));
    let connection = Connection { host: "http://127.0.0.1:1".into(), token: "rmm_test".into(), insecure: false, token_id: None, token_name: None, device_id: None };
    let _ = app.update(Message::Paired(Ok(Box::new(connection))));
    assert!(app.offer_retrodeck, "to be offered once the library is in");
}

#[test]
fn retrodeck_s_install_shows_in_settings_and_its_setup_has_the_pad() {
    use crate::gamepad::Pad;
    use crate::retrodeck::{Event, Presence, Progress, Stop};
    let mut app = paired_fixtures(12);
    app.screen = Screen::Browsing(Route::Settings);
    app.retrodeck_setup = RetroDeckSetup::Installing(None);
    let _ = app.update(Message::RetroDeckEvent(Event::Progress(Progress { step: 2, of: 2, percent: 45 })));
    assert_eq!(app.retrodeck_progress().as_deref(), Some("RetroDECK · installing 2 of 2 · 45%"));
    let _ = app.view();

    // Its row stops it — asked first, since a press there is easily one too many.
    let _ = app.update(Message::SettingsClicked(settings_row(SettingRow::Launcher)));
    assert_eq!(app.pick.as_ref().unwrap().picker.title, "Stop installing RetroDECK?");
    app.pick = None;

    // Open for its setup: the pad is RetroDECK's.
    let _ = app.update(Message::RetroDeckEvent(Event::SettingUp));
    let _ = app.update(Message::Pad(Pad::Select));
    assert!(matches!(app.screen, Screen::Browsing(Route::Settings)), "Select would have opened Downloads");

    // Set up: games start through it, and ES-DE's hooks are offered next.
    let _ = app.update(Message::RetroDeckEvent(Event::Done(Ok(Presence::Ready))));
    assert!(app.retrodeck);
    assert_eq!(app.retrodeck_setup, RetroDeckSetup::Idle);
    assert!(matches!(app.pick.as_ref().map(|p| &p.then), Some(Then::EsDeHooks)));
    app.pick = None;

    // Stopped part way: said, and back to where it was.
    app.retrodeck_setup = RetroDeckSetup::Installing(None);
    let _ = app.update(Message::RetroDeckEvent(Event::Done(Err(Stop::Cancelled))));
    assert_eq!(app.retrodeck_setup, RetroDeckSetup::Idle);
    assert!(app.banner.as_deref().is_some_and(|b| b.contains("stopped")), "{:?}", app.banner);
}

// ---- RetroArch's state pictures --------------------------------------------------------------

#[test]
fn state_pictures_are_offered_once_and_a_no_is_remembered() {
    use crate::sync::retroarch::StatePictures;
    let off = || Ok(StatePictures { on: false, off_in: Vec::new() });
    let mut app = paired_fixtures(12);

    // A game's sync sent a state up without a picture: offered, once this session.
    let session = crate::launch::Session {
        exit: crate::launch::Exit { success: true, code: Some(0), last_line: None },
        played_at: None,
        notes: Vec::new(),
        report: Some(Box::new(crate::sync::Report { pictureless: 1, ..Default::default() })),
    };
    let game = app.library.all_games()[0].rom_id;
    app.playing = Some(game);
    let _ = app.update(Message::LaunchEvent(crate::launch::Event::Ended(game, Ok(session))));
    assert!(app.state_pictures_offered, "a sync without a picture asks where they stand");
    let _ = app.update(Message::StatePicturesFound(off()));
    let pick = app.pick.as_ref().expect("the offer");
    assert_eq!(pick.picker.title, "Keep a picture with each save state?");
    assert!(pick.picker.note.contains("next time they're saved"), "{}", pick.picker.note);

    // No: remembered, and not asked again — the Settings row still turns them on.
    let _ = app.update(Message::Picked(1));
    assert!(app.settings.state_pictures_declined);
    app.state_pictures_offered = false;
    let _ = app.offer_state_pictures();
    assert!(!app.state_pictures_offered, "declined for good");

    // Already on: nothing to offer.
    let mut app = paired_fixtures(12);
    let _ = app.update(Message::StatePicturesFound(Ok(StatePictures { on: true, off_in: Vec::new() })));
    assert!(app.pick.is_none());
}

#[test]
fn the_state_pictures_row_turns_them_on_or_names_the_override_that_turns_them_off() {
    use crate::sync::retroarch::StatePictures;
    let mut app = paired_fixtures(12);
    app.screen = Screen::Browsing(Route::Settings);
    app.state_pictures = Some(Ok(StatePictures { on: false, off_in: Vec::new() }));
    let _ = app.view();
    assert!(app.update(Message::SettingsClicked(settings_row(SettingRow::StatePictures))).units() > 0, "turning them on is under way");

    let override_file = std::path::PathBuf::from("/ra/config/Snes9x/Chrono Trigger (USA).cfg");
    let _ = app.update(Message::StatePicturesChanged(Ok(()), Ok(StatePictures { on: true, off_in: vec![override_file.clone()] })));
    assert!(app.banner.as_deref().is_some_and(|b| b.contains("Chrono Trigger (USA).cfg")), "{:?}", app.banner);
    let _ = app.view();

    // Refused while RetroArch runs: said, and still off.
    let _ = app.update(Message::StatePicturesChanged(Err("RetroArch is running".into()), Ok(StatePictures { on: false, off_in: Vec::new() })));
    assert!(app.banner.as_deref().is_some_and(|b| b.contains("left off: RetroArch is running")), "{:?}", app.banner);
}
