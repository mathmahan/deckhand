//! The C API of argosy-sigil, as `third_party/argosy-sigil/include/sigil.h` declares it at the
//! pinned commit — only the parts this app calls, and every struct those parts pass by pointer laid
//! out field for field, since sigil reads and writes them in place. `sizes.c` holds the layouts to
//! the header's own (see the tests in `super`).

#![allow(non_camel_case_types, dead_code)]

use std::os::raw::{c_char, c_int, c_void};

pub const SIGIL_RESULT_V4: u32 = 4;
pub const SIGIL_SAVE_REQUEST_V1: u32 = 1;
pub const SIGIL_SYNC_REQUEST_V1: u32 = 1;

pub const SIGIL_OK: c_int = 0;
pub const SIGIL_ERR_UNSUPPORTED_FORMAT: c_int = -4;
pub const SIGIL_ERR_NOT_FOUND: c_int = -5;
pub const SIGIL_ERR_CONFLICT: c_int = -9;

pub const SIGIL_SAVE_SHAPE_NONE: c_int = 0;
pub const SIGIL_SAVE_SHAPE_SINGLE: c_int = 1;
pub const SIGIL_SAVE_SHAPE_MULTI: c_int = 2;
pub const SIGIL_SAVE_SHAPE_FOLDER: c_int = 3;

pub const SIGIL_SYNC_UNMANAGED: c_int = 1;

pub const SIGIL_SOURCE_BINARY: c_int = 0;

pub const SIGIL_SAVE_PATH_MAX: usize = 512;
pub const SIGIL_SAVE_ENTRY_MAX: usize = 256;
pub const SIGIL_PROFILE_ID_MAX: usize = 64;
pub const SIGIL_PROFILE_NAME_MAX: usize = 64;
pub const SIGIL_CARD_NAME_MAX: usize = 64;
pub const SIGIL_LAYOUT_OPTION_VALUES: usize = 8;
pub const SIGIL_LAYOUT_OPTIONS: usize = 16;

/// `sigil_platform`. An enum in C, so an `int`.
pub type sigil_platform = c_int;
pub const SIGIL_PLATFORM_AUTO: sigil_platform = 0;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct sigil_result {
    pub struct_version: u32,
    pub title_id: [c_char; 32],
    pub raw_serial: [c_char; 32],
    pub save_id: [c_char; 32],
    pub platform: sigil_platform,
    pub source: c_int,
    pub usage: c_int,
    pub experimental: c_int,
    pub switch_content_type: c_int,
    pub title_version: u32,
    pub features: u32,
    pub n64_header: [c_char; 24],
    pub n64_md5: [c_char; 33],
    pub n64_md5_n64: [c_char; 33],
}

/// A stream sigil reads through. Only ever made by sigil (`sigil_io_open_file`) and handed back to
/// it, so its fields are never touched here.
#[repr(C)]
pub struct sigil_io {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct sigil_save_member {
    pub path: [c_char; SIGIL_SAVE_PATH_MAX],
    pub entry: [c_char; SIGIL_SAVE_ENTRY_MAX],
    pub role: c_int,
    pub present: c_int,
    pub area: c_int,
}

#[repr(C)]
pub struct sigil_save_profile {
    pub id: [c_char; SIGIL_PROFILE_ID_MAX],
    pub name: [c_char; SIGIL_PROFILE_NAME_MAX],
}

#[repr(C)]
pub struct sigil_save_option {
    pub key: *const c_char,
    pub value: *const c_char,
}

#[repr(C)]
pub struct sigil_save_alternate {
    pub path: [c_char; SIGIL_SAVE_PATH_MAX],
    pub shared: c_int,
    pub options: [sigil_save_option; 2],
    pub option_count: usize,
}

pub type sigil_save_open_fn = Option<unsafe extern "C" fn(ctx: *mut c_void, relative_path: *const c_char) -> *mut sigil_io>;

#[repr(C)]
pub struct sigil_save_request {
    pub struct_version: u32,
    pub layout: *const c_char,
    pub platform: *const c_char,
    pub content_path: *const c_char,
    pub result: *const sigil_result,
    pub features: u32,
    pub options: *const sigil_save_option,
    pub option_count: usize,
    pub listing: *const *const c_char,
    pub listing_count: usize,
    pub open: sigil_save_open_fn,
    pub open_ctx: *mut c_void,
    pub root_path: *const c_char,
    pub profile: *const c_char,
}

#[repr(C)]
pub struct sigil_save_unit {
    pub struct_version: u32,
    pub key: [c_char; SIGIL_SAVE_ENTRY_MAX],
    pub shape: c_int,
    pub members: *mut sigil_save_member,
    pub member_count: usize,
    pub expected: *mut sigil_save_member,
    pub expected_count: usize,
    pub unkeyed: *mut [c_char; SIGIL_SAVE_PATH_MAX],
    pub unkeyed_count: usize,
    pub artifact: [c_char; SIGIL_SAVE_ENTRY_MAX],
    pub content_hash: [c_char; 33],
    pub identity_hash: [c_char; 33],
    pub alternates: *mut sigil_save_alternate,
    pub alternate_count: usize,
}

#[repr(C)]
pub struct sigil_layout_option {
    pub key: [c_char; 64],
    pub values: [[c_char; 32]; SIGIL_LAYOUT_OPTION_VALUES],
    pub value_count: usize,
    pub default_value: [c_char; 32],
}

#[repr(C)]
pub struct sigil_layout_info {
    pub id: [c_char; 64],
    pub platform: [c_char; 16],
    pub options: [sigil_layout_option; SIGIL_LAYOUT_OPTIONS],
    pub option_count: usize,
    pub region_option: [c_char; 64],
    pub profiles: c_int,
    pub needs_existing: c_int,
}

pub type sigil_save_write_fn =
    Option<unsafe extern "C" fn(ctx: *mut c_void, relative_path: *const c_char, data: *const u8, len: usize) -> c_int>;
pub type sigil_save_remove_fn = Option<unsafe extern "C" fn(ctx: *mut c_void, relative_path: *const c_char) -> c_int>;

#[repr(C)]
pub struct sigil_sync_companion {
    pub game_ids: *const *const c_char,
    pub game_id_count: usize,
    pub unit: *const u8,
    pub unit_len: usize,
}

#[repr(C)]
pub struct sigil_sync_companion_result {
    pub data: *mut u8,
    pub len: usize,
    pub content_hash: [c_char; 33],
    pub identity_hash: [c_char; 33],
    pub changed: c_int,
}

#[repr(C)]
pub struct sigil_sync_request {
    pub struct_version: u32,
    pub save: sigil_save_request,
    pub game_ids: *const *const c_char,
    pub game_id_count: usize,
    pub mode: c_int,
    pub state: *const u8,
    pub state_len: usize,
    pub overwrite_local: c_int,
    pub write: sigil_save_write_fn,
    pub write_ctx: *mut c_void,
    pub claimed: *const *const c_char,
    pub claimed_count: usize,
    pub remove: sigil_save_remove_fn,
    pub companions: *const sigil_sync_companion,
    pub companion_count: usize,
    pub repair: c_int,
}

#[repr(C)]
pub struct sigil_sync_result {
    pub struct_version: u32,
    pub artifact: [c_char; SIGIL_SAVE_ENTRY_MAX],
    pub shape: c_int,
    pub data: *mut u8,
    pub len: usize,
    pub content_hash: [c_char; 33],
    pub identity_hash: [c_char; 33],
    pub changed: c_int,
    pub conflict: c_int,
    pub state: *mut u8,
    pub state_len: usize,
    pub holding: *mut u8,
    pub holding_len: usize,
    pub unowned: *mut [c_char; SIGIL_CARD_NAME_MAX],
    pub unowned_count: usize,
    pub restore_again: c_int,
    pub companions: *mut sigil_sync_companion_result,
    pub companion_count: usize,
    pub problem: [c_char; SIGIL_SAVE_PATH_MAX],
    pub blocks_short: u32,
    pub profiles: *mut sigil_save_profile,
    pub profile_count: usize,
    pub profile: [c_char; SIGIL_PROFILE_ID_MAX],
    pub alternates: *mut sigil_save_alternate,
    pub alternate_count: usize,
    pub hardcore_marker: c_int,
    pub unowned_changed: usize,
}

unsafe extern "C" {
    pub fn sigil_extract_from_path(path: *const c_char, hint: sigil_platform, opts: *const c_void, out: *mut sigil_result) -> c_int;
    pub fn sigil_platform_from_slug(slug: *const c_char) -> sigil_platform;
    pub fn sigil_platform_to_slug(platform: sigil_platform) -> *const c_char;
    pub fn sigil_io_open_file(path: *const c_char) -> *mut sigil_io;

    pub fn sigil_save_resolve(req: *const sigil_save_request, out: *mut *mut sigil_save_unit) -> c_int;
    pub fn sigil_save_unit_free(unit: *mut sigil_save_unit);
    pub fn sigil_save_layout_subdirs(layout: *const c_char, out: *mut *const c_char, cap: usize) -> usize;
    pub fn sigil_layouts(platform: *const c_char, out: *mut *mut sigil_layout_info, count: *mut usize) -> c_int;
    pub fn sigil_layouts_free(layouts: *mut sigil_layout_info);

    pub fn sigil_collect(req: *const sigil_sync_request, out: *mut *mut sigil_sync_result) -> c_int;
    pub fn sigil_restore(req: *const sigil_sync_request, unit: *const u8, unit_len: usize, out: *mut *mut sigil_sync_result) -> c_int;
    pub fn sigil_sync_result_free(result: *mut sigil_sync_result);

    pub fn sigil_strerror(code: c_int) -> *const c_char;
    pub fn sigil_version() -> *const c_char;
}

#[cfg(test)]
unsafe extern "C" {
    pub fn deckhand_sigil_sizeof_result() -> usize;
    pub fn deckhand_sigil_sizeof_save_member() -> usize;
    pub fn deckhand_sigil_sizeof_save_request() -> usize;
    pub fn deckhand_sigil_sizeof_save_unit() -> usize;
    pub fn deckhand_sigil_sizeof_layout_info() -> usize;
    pub fn deckhand_sigil_sizeof_sync_request() -> usize;
    pub fn deckhand_sigil_sizeof_sync_result() -> usize;
    pub fn deckhand_sigil_offsetof_sync_result_unowned_changed() -> usize;
    pub fn deckhand_sigil_offsetof_sync_request_repair() -> usize;
    pub fn deckhand_sigil_offsetof_save_unit_alternate_count() -> usize;
    pub fn deckhand_sigil_offsetof_result_n64_md5_n64() -> usize;
}
