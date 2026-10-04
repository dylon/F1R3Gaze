//! `gaze-shell` — the F1R3Gaze application (spec §3, WP C1, K1).
//!
//! * [`engine`]: what a profile's tabs share (HTTP, blobs, the shard bridge,
//!   the broker) and each tab's services.
//! * [`tab`]: the load pipeline — fetch, parse, verify scripts, plan grants,
//!   prompt, run.
//! * `chrome` (feature `window`): the window's own document — tab strip,
//!   address bar, prompt bar, grants and console panels — hosting each tab's
//!   `RhoDocument` as a sub-document.
//! * `cursor` (feature `window`): the window's mouse cursor and the pointer's
//!   hover inside pages, decided in one place by the chrome.
//! * [`headless`]: the same pipeline without a window.

pub mod display;
pub mod engine;
pub mod headless;
pub mod pages;
pub mod profile;
pub mod tab;
pub mod text_fit;
pub mod theme;
pub mod ui_state;

#[cfg(feature = "window")]
pub mod chrome;
#[cfg(feature = "window")]
pub mod cursor;

#[cfg(test)]
mod test_support;

pub use engine::Engine;
