//! Deckhand — a gamepad-first desktop companion for the RomM ROM manager, in Rust and iced.
//!
//! It pairs with a RomM server, browses it, downloads games from it, launches them, and keeps their
//! saves and states in step with it — from the window, or around a game another launcher starts
//! (`deckhand pre|post`, in `cli`).

mod achievements;
mod animation;
mod bios;
mod app;
mod cache;
mod cli;
mod config;
mod collections;
mod covers;
mod downloads;
mod fixtures;
mod focus;
mod folders;
mod frames;
mod game_saves;
mod gamepad;
mod grid;
mod hooks;
mod launch;
mod library;
mod memory;
mod qr_pairing;
mod release;
mod search;
mod sigil;
mod steam;
mod romm;
#[cfg(test)]
mod fake_romm;
#[cfg(test)]
mod stub_server;
mod sync;
mod systems;
mod theme;
mod ui;
mod view_model;
mod watch;

use app::App;

/// The Deck's panel, which is what the layout is designed against: the hero band, the shelf heights
/// and the 16 px type floor are all sized for 1280x800 read at couch distance.
pub const PANEL: (f32, f32) = (1280.0, 800.0);

/// The window gives [`PANEL`] worth of *layout space* at every UI scale.
///
/// This matters because iced's `scale_factor` scales the whole layout, where Deckhand's UI-scale
/// ladder is meant to scale type and poster size against a fixed panel. Opening a 1280x800 window
/// at 1.15 would hand the layout 1113x696 to work with, and a 420-unit hero would then take three
/// fifths of the height instead of half — pushing the first shelf off the bottom. So the window is
/// 1280x800 *times* the scale in the system's pixels — which is what iced makes of a window asked
/// for at [`PANEL`], since it sizes a new window in layout units and applies the scale itself. Its
/// minimum is the exception: iced takes that in the system's pixels, so it is multiplied here.
fn window_size(scale: f32) -> iced::Size {
    iced::Size::new(PANEL.0 * scale, PANEL.1 * scale)
}

/// `--fixtures=N` — run the UI on N synthetic games with no RomM server, so the layout can be seen
/// and changed without a pairing. Ported from the Kotlin app's flag of the same name.
fn fixture_count() -> Option<usize> {
    std::env::args()
        .find_map(|a| a.strip_prefix("--fixtures=").map(str::to_string))
        .and_then(|n| n.parse().ok())
}

fn main() -> iced::Result {
    // Before any thread exists — see `memory`.
    memory::gpu_backends();
    memory::tune();

    // `deckhand pre|post|sync` is the save-sync wrapper a launcher calls around a game, and it must
    // be done before anything starts drawing — see `cli`.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }

    let fixtures = fixture_count();
    let scale = config::Settings::load().ui_scale;
    // Built here rather than in the app, so the one thing that can fail before a window exists —
    // the TLS setup underneath — is said in words, not as a panic.
    let client = match romm::RommClient::new() {
        Ok(client) => client,
        Err(e) => {
            eprintln!("Deckhand can't start: it could not set up its connection to RomM ({e}).");
            std::process::exit(1);
        }
    };
    // In Game Mode the window is the screen, whatever shape the screen is: gamescope scales a window
    // of any other size to fit, so a 16:10 one on a 16:9 TV came out with bars down its sides.
    // `DECKHAND_FULLSCREEN=1` asks for the same anywhere else.
    let fullscreen = steam::game_mode() || std::env::var_os("DECKHAND_FULLSCREEN").is_some_and(|v| v != "0");
    // 0.14 takes the boot function first and moves the title to a builder.
    let boot = move || {
        let (mut app, task) = App::with_client(fixtures, client.clone());
        app.set_fullscreen(fullscreen);
        (app, task)
    };
    let mut app = iced::application(boot, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .style(App::style)
        .scale_factor(App::scale_factor)
        .subscription(App::subscription)
        .window(iced::window::Settings {
            fullscreen,
            size: iced::Size::new(PANEL.0, PANEL.1),
            // Below this the shelves stop being shelves — a card plus its label needs the height,
            // and the hero needs the width to keep its art panel beside the text.
            min_size: Some(window_size(scale) * 0.7),
            // The logo in the taskbar and Alt-Tab, where the system takes the window's own icon.
            icon: theme::window_icon(),
            // And where it takes the icon from the desktop entry instead — Wayland's desktops — the
            // id that finds it: `deckhand.desktop`'s name, which the AppImage carries.
            #[cfg(target_os = "linux")]
            platform_specific: iced::window::settings::PlatformSpecific { application_id: "deckhand".into(), ..Default::default() },
            ..Default::default()
        })
        // The tab bar's icons. Loaded up front rather than through a `font::load` task, because a
        // font that arrives a frame late draws the whole bar as tofu boxes on the way past.
        .font(iced_fonts::LUCIDE_FONT_BYTES)
        // DM Sans is the app's face, so nothing that does not name a font falls back to the host's
        // sans — the type scale in `theme` is set against this one.
        .default_font(theme::REGULAR)
        .antialiasing(true);

    // Every weight the UI asks for, registered by hand. iced matches a `Font` by family *and*
    // weight, and an unregistered weight silently resolves to the nearest one that is there — so a
    // missing SemiBold does not fail, it just quietly draws headings at body weight.
    for face in theme::FONT_FACES {
        app = app.font(face);
    }

    app.run()
}
