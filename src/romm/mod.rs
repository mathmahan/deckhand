//! The RomM client: HTTP surface plus the serde models it decodes into.

pub mod client;

// The models are the decoded shape of RomM's API, not an internal model. A field nothing reads yet
// still documents what the server sends and what a later view can rely on, so unused ones are kept
// deliberately rather than trimmed to whatever this build happens to touch.
#[allow(dead_code)]
pub mod models;

pub use client::{host_candidates, normalize_code, ApiError, Outcome, Push, Pushed, RommClient, RomQuery, FETCH_PARALLELISM, HEARTBEAT_LASTS};
