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
//! * `renderer` (feature `window`): Vello's window renderer with surface
//!   resizes coalesced into the next frame; `frame_stats` times the render
//!   loop in `frame-times` builds.
//! * `application` (feature `window`): Blitz's application handler, polling
//!   the chrome as soon as the window changes size, so a resize is painted
//!   once and already fitted; making the window where it was left, keeping
//!   `state/window.json`, showing the chrome's scheme in its pages and
//!   decorations, and carrying out full screen.
//! * [`window_state`]: the window's size, place, full screen and zoom in
//!   `state/window.json` (pure): where to make the window, and when to save.
//! * [`headless`]: the same pipeline without a window.

// Tests write their fixtures directly (clippy.toml's disallowed-methods
// apply to the code they test).
#![cfg_attr(test, allow(clippy::disallowed_methods))]

pub mod display;
pub mod engine;
pub mod grants;
pub mod headless;
pub mod pages;
pub mod profile;
pub mod site_index;
pub mod tab;
pub mod text_fit;
pub mod theme;
pub mod ui_state;
pub mod window_state;

#[cfg(feature = "window")]
pub mod application;
#[cfg(feature = "window")]
pub mod chrome;
#[cfg(feature = "window")]
pub mod cursor;
#[cfg(feature = "window")]
pub mod system_theme;
#[cfg(feature = "window")]
pub mod frame_stats;
#[cfg(feature = "window")]
pub mod renderer;
#[cfg(all(feature = "window", target_os = "macos"))]
mod macos_url;

#[cfg(test)]
mod test_support;

pub use engine::Engine;
