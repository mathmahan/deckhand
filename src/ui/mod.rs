//! The view layer. Each module owns one screen and takes the state it renders by reference — no
//! module here reaches for the network or the disk.

pub mod browse;
pub mod collections;
pub mod detail;
pub mod downloads;
pub mod entry;
pub mod home;
pub mod login;
pub mod picker;
pub mod reveal;
pub mod save_sync;
pub mod saves;
pub mod search;
pub mod style;
pub mod settings;
pub mod slide;
pub mod tab_bar;
