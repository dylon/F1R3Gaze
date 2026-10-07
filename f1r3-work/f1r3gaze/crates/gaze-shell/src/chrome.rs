//! The window: a Blitz document of its own (the chrome) that hosts each
//! tab's `RhoDocument` as a sub-document of a view element. The chrome is
//! plain HTML driven from Rust; making it a f1r3lang page with a privileged
//! capability is work package K1 (P4).
//!
//! How the chrome updates (the design is documented in `docs/ui/README.md`):
//!
//! * Static regions (toolbar, rail, find box, wallet form) are parsed once
//!   from the templates below. Their state (pressed, disabled, focused,
//!   hidden) is set by attribute on stable ids with
//!   `ChromeDocument::sync_attr`, so a field being typed into is never
//!   rebuilt.
//! * Dynamic regions are rebuilt as HTML strings from plain data by the free
//!   `*_html` builders, and inserted only when their markup changed
//!   (`ChromeDocument::set_html`).
//! * Labels are shortened to the exact width they are given
//!   ([`TextFitter`]), because Blitz has no `text-overflow`.
//! * Labels are elements, never bare text in a flex box: Blitz never
//!   restyles the anonymous box such text gets (`docs/ui/ledger.md`, L2).

use crate::application::{ChromeApplication, PendingWindow};
use crate::cursor::CursorArbiter;
use crate::frame_stats;
use crate::renderer::CoalescingRenderer;
use crate::display::{self, LogLevel, Recency, SchemeKind, Tone};
use crate::engine::{Engine, NavRequest, escape};
use crate::tab::{Stage, Tab};
use crate::text_fit::{ELLIPSIS, Face, Font, Marked, TextFitter};
use crate::profile::report::{Event, EventKind, Severity};
use crate::system_theme::{Known, OVERRIDE_VAR, STARTUP_WAIT, Source, SystemScheme};
use crate::window_state::WindowState;
use crate::theme::{self, Scheme, ThemeChoice, ThemeFile};
// Was `UiState`: one `workspace.json` held the session, the history and the
// theme. They are `state/session.json`, `state/history.json` and
// `[appearance] theme` in `settings.toml` now (ledger S14).
use crate::ui_state::{History, SavedTab, SessionState, Visit, match_span, search_score};
use anyrender_vello::VelloWindowRenderer;
use blitz_dom::{
    BaseDocument, DocGuard, DocGuardMut, Document, DocumentConfig, EventDriver, EventHandler,
    LocalName, NodeId, QualName, ns,
};
// `WindowConfig` is made in `application.rs` now, where the window is made
// at the size and place it was left (storage ledger S13, part 2).
use blitz_shell::{BlitzApplication, BlitzShellProxy, ControlFlow, EventLoop};
use blitz_traits::events::{DomEvent, DomEventData, EventState, MouseEventButton, UiEvent};
use blitz_traits::net::NetWaker;
use blitz_traits::shell::ColorScheme;
use gaze_fs::Perm;
use gaze_dom_blitz::{FindHit, Pacer, RhoDocument, WakeHandle};
use keyboard_types::{Key, Modifiers};
use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::Context as TaskContext;
use std::time::{Duration, Instant};

/// Append formatted text to a `String` (formatting into a `String` cannot
/// fail).
macro_rules! put {
    ($out:expr, $($arg:tt)*) => {
        $out.write_fmt(format_args!($($arg)*))
            .expect("formatting into a String cannot fail")
    };
}

/// At most this many find hits are collected per page.
const FIND_LIMIT: usize = 2_000;
/// A revealed hit stays below the floating find box (10 px offset, 44 px
/// box, 8 px gap).
const FIND_REVEAL_TOP_MARGIN: f64 = 62.0;
/// While a newly attached page has no layout yet, find looks again after
/// this long.
const FIND_RESCAN_DELAY: Duration = Duration::from_millis(30);
/// Closed tabs remembered for reopening (the newest last).
const CLOSED_LIMIT: usize = 20;
/// Recently closed tabs listed in the Tabs panel.
const CLOSED_SHOWN: usize = 5;
/// History rows rendered at once; "Show more" adds this many.
const HISTORY_PAGE: usize = 200;
/// Fuzzy (typo-tolerant) history matching looks at this many recent visits;
/// older visits match by exact substring only.
const HISTORY_FUZZY_WINDOW: usize = 200;
/// Console lines kept per tab, and shown in the panel (newest first).
const CONSOLE_LIMIT: usize = 1_000;
const CONSOLE_SHOWN: usize = 200;
/// How long a status message stays: confirmations briefly, problems longer.
const FLASH_BRIEF: Duration = Duration::from_secs(4);
const FLASH_LONG: Duration = Duration::from_secs(8);

/// How long a visit waits before the history is written: the visits of a
/// burst of page loads are saved once, and a tab switch never rewrites the
/// history (`ChromeDocument::history_changed`).
const HISTORY_SAVE_DELAY: Duration = Duration::from_secs(2);

// Disabled in step 10 (ledger S12, part 3): Custom was the interim name of
// `themes/custom.css` (step 9). Every theme file is a row of the Appearance
// panel's Themes list now.
// /// The theme file the Appearance panel's Custom segment uses:
// /// `themes/custom.css` in the settings folder, where the move of an old
// /// profile puts its `palette.css`.
// const CUSTOM_THEME: &str = "custom";

/// How many names New theme tries when one turns out to be taken by
/// something the Themes list does not show (a folder; a `.CSS` file on a
/// file system that ignores case).
const NEW_THEME_TRIES: usize = 8;

/// What the colour scheme colours: the end of the sentence under the
/// scheme control (`scheme_note`). Pages that offer light and dark styles
/// follow it (ledger L13).
const SCHEME_NOTE: &str = "Colors the browser controls and built-in pages, and sites that offer light and dark styles.";

/// The sidebar panels in rail order: `(panel, rail button id, title)`.
const PANELS: [(&str, &str, &str); 7] = [
    ("tabs", "rail-tabs", "Tabs"),
    ("history", "rail-history", "History"),
    ("sites", "rail-sites", "Site data"),
    ("grants", "rail-grants", "Permissions"),
    ("wallet", "rail-wallet", "Wallet"),
    ("console", "rail-console", "Console"),
    ("appearance", "rail-appearance", "Appearance"),
];

// ── Text styles and box geometry ────────────────────────────────────────
//
// These mirror `CSS` so that labels can be fitted to the exact width the
// stylesheet gives them. The test `text_budgets_match_laid_out_boxes` lays
// the chrome out with Blitz and checks every derived width.

/// 13 px Noto Sans: body text, row labels, tab titles, suggestion titles.
const LABEL: Font = Font::new(Face::Sans, 130);
/// 13 px Noto Sans SemiBold: card titles.
const LABEL_STRONG: Font = Font::new(Face::SansSemibold, 130);
/// 11 px Fira Code: `.row-detail` (addresses under row labels).
const DETAIL: Font = Font::new(Face::Mono, 110);
/// 11.5 px Fira Code: `.mono` (wallet addresses) and suggestion URLs.
const MONO: Font = Font::new(Face::Mono, 115);
/// 11.5 px Noto Sans: the status bar.
const SMALL: Font = Font::new(Face::Sans, 115);
/// 11 px Noto Sans: `.row-meta` (relative times, balances), and `.btn-xs`.
const META: Font = Font::new(Face::Sans, 110);
/// 10.5 px Noto Sans SemiBold: `.tag`.
const TAG: Font = Font::new(Face::SansSemibold, 105);

mod geometry {
    //! Box geometry from `CSS`, in CSS px.

    /// `#sidebar` width less its 1 px right border.
    pub const SIDEBAR_INNER: f32 = 299.0;
    /// The sidebar gutter (R7): horizontal padding of `#panel`, `#wallet`,
    /// and `#sidecontrols`.
    pub const PANEL_PAD_X: f32 = 6.0;
    // The wallet once had its own, wider padding; it was dropped so every
    // panel shares one gutter (docs/ui/ledger.md, L5 R8).
    // pub const WALLET_PAD_X: f32 = 10.0;
    /// `.row`: horizontal padding, gap between children, icon width.
    pub const ROW_PAD_X: f32 = 8.0;
    pub const ROW_GAP: f32 = 8.0;
    pub const ROW_ICON: f32 = 16.0;
    /// Extra left padding per tree level in the Tabs panel.
    pub const TREE_INDENT: f32 = 12.0;
    /// Room kept for the always-visible actions of the active row.
    pub const ROW_ACTIONS: f32 = 56.0;
    /// `.card`: horizontal margin (none: cards span the gutter), border,
    /// and padding.
    pub const CARD_MARGIN_X: f32 = 0.0;
    pub const CARD_BORDER: f32 = 1.0;
    pub const CARD_PAD_X: f32 = 12.0;
    /// `.kv-key` width and the gap after it.
    pub const KV_KEY: f32 = 62.0;
    pub const KV_GAP: f32 = 10.0;
    /// `.tag`: horizontal padding (2 × 7).
    pub const TAG_PAD: f32 = 14.0;
    /// `#tabs`: horizontal padding and the gap before `#newtab`; `#tablist`
    /// gap between tabs; the new-tab button and the "+N" chip.
    pub const STRIP_PAD_X: f32 = 8.0;
    pub const STRIP_GAP: f32 = 4.0;
    pub const TAB_GAP: f32 = 2.0;
    pub const NEWTAB: f32 = 30.0;
    pub const TABMORE: f32 = 44.0;
    /// Tab widths: tabs share the room up to `TAB_MAX`; the active tab keeps
    /// at least `TAB_ACTIVE_MIN` and the others shrink to `TAB_MIN`, where
    /// only their icon shows.
    pub const TAB_MAX: f32 = 200.0;
    pub const TAB_ACTIVE_MIN: f32 = 128.0;
    pub const TAB_MIN: f32 = 44.0;
    /// What the active tab spends on everything but its title: padding
    /// 10 + 6, borders 2 × 1, icon 14, two 7 px gaps, close button 20.
    pub const TAB_CHROME: f32 = 66.0;
    /// The same for an inactive tab, whose close button overlays the title
    /// on hover instead of taking room: padding 10 + 6, borders 2 × 1, icon
    /// 14, one 7 px gap.
    pub const TAB_CHROME_INACTIVE: f32 = 39.0;
    /// Everything in the toolbar but the address box: padding 2 × 8, five
    /// 30 px buttons, five 2 px gaps, and the box's 2 × 6 px margins.
    pub const TOOLBAR_FIXED: f32 = 188.0;
    /// `#suggestions` border and padding, 2 × 1 + 2 × 4 (its border box is
    /// exactly as wide as `#urlwrap`'s).
    pub const DROPDOWN_INSET: f32 = 10.0;
    /// `.suggestion`: padding 2 × 10, icon 16, gap 10.
    pub const SUGGESTION_FIXED: f32 = 46.0;
    /// Gap between a suggestion's columns, and the title's share of them.
    pub const SUGGESTION_GAP: f32 = 10.0;
    pub const SUGGESTION_TITLE_SHARE: f32 = 0.4;
    /// `#status`: horizontal padding and gap; a chip's icon and its gap.
    pub const STATUS_PAD_X: f32 = 10.0;
    pub const STATUS_GAP: f32 = 12.0;
    pub const STATUS_ICON: f32 = 12.0;
    pub const STATUS_ICON_GAP: f32 = 5.0;
    /// Kept free at the end of every fitted label: Blitz rounds box sizes
    /// to whole pixels.
    pub const ROUNDING: f32 = 1.0;

    /// The text column of a sidebar `.row` (before indentation, meta, or
    /// actions are taken off).
    pub const fn row_text() -> f32 {
        SIDEBAR_INNER - 2.0 * PANEL_PAD_X - 2.0 * ROW_PAD_X - ROW_ICON - ROW_GAP
    }

    /// The content box of a `.card` inside `#panel` or `#wallet` (both have
    /// the gutter as their padding).
    pub const fn card_inner() -> f32 {
        SIDEBAR_INNER - 2.0 * PANEL_PAD_X - 2.0 * CARD_MARGIN_X - 2.0 * CARD_BORDER - 2.0 * CARD_PAD_X
    }

    /// The width of `#urlwrap` in a window `window` CSS px wide.
    pub fn address_box(window: f32) -> f32 {
        (window - TOOLBAR_FIXED).max(140.0)
    }

    /// The width a `.suggestion` row gives its title, URL, and tag.
    pub fn suggestion_row(window: f32) -> f32 {
        address_box(window) - DROPDOWN_INSET - SUGGESTION_FIXED
    }
}

// ── Stylesheet ──────────────────────────────────────────────────────────

/// The chrome stylesheet. Theme colours (`--gaze-*`) come from
/// [`theme::variables`]; icons and fonts from [`theme::font_css`].
const CSS: &str = r#"
/* Conventions (docs/ui/README.md):
   R1 text runs: labels are elements (<span>), icons <i>; never bare text
      directly in a flex or grid box (Blitz never restyles its anonymous box).
   R2 overlays: every overlay has a positive z-index, which hoists it above
      the page; its ancestors get no opacity, transform, or filter.
   R3 toggles: frequent show/hide uses visibility (.off, :hover). display:none
      is only for #sidebar, #panel, #wallet and the Send form with no wallet
      (.hidden), regions with nothing in them (:empty), and the parts of a
      narrow tab (set when the strip is rebuilt).
   R4 stable inputs: inputs live in constant markup; their state is set by
      attribute on stable ids.
   R5 budgets: labels are fitted to their exact width in Rust; overflow is a
      backstop.
   R6 accent: accent colours icons, bars, and rings; accent text appears only
      on --gaze-surface.
   R7 gutter: sidebar boxes (rows, cards, notices, the controls bar) span the
      6 px panel gutter; headings, labels, notes, and in-flow form controls
      start 8 px further in.
   R8 bars: an edge bar is a solid image the size of the bar,
      linear-gradient(c,c) 0 0/3px 100% no-repeat, over the background
      colour. Never an inset box-shadow (Blitz leaves a hairline along the
      other edges) nor a hard stop across the element (Vello quantizes it to
      1/511 of the element's length). Bordered elements put the bar in the
      border box. */
:root{--sans:'Noto Sans',system-ui,sans-serif;--mono:'Fira Code',monospace;--radius-sm:5px;--radius-md:7px;--radius-lg:10px}
*{box-sizing:border-box}
html,body{margin:0;height:100%;font:13px/1.35 var(--sans);color:var(--gaze-text);background:var(--gaze-bg)}
body{display:flex;flex-direction:column}
button,input{font:inherit;color:inherit}
button{display:inline-flex;align-items:center;justify-content:center;gap:6px;margin:0;padding:0;border:1px solid transparent;border-radius:var(--radius-md);background:transparent;cursor:pointer;white-space:nowrap}
button:disabled{cursor:default}
input{height:30px;min-width:0;padding:0 9px;border:1px solid var(--gaze-border);border-radius:var(--radius-md);background:var(--gaze-bg)}
button:focus,input:focus{outline:2px solid var(--gaze-accent);outline-offset:1px}
.hidden{display:none!important}.grow{flex:1;min-width:0}.muted{color:var(--gaze-muted)}
.mono{font-family:var(--mono);font-size:11.5px}
/* Buttons */
.btn{position:relative;height:28px;padding:0 10px;font-size:12px;border-color:var(--gaze-border);background:var(--gaze-raised)}
.btn:hover{background:var(--gaze-hover)}
.btn-sm{height:24px;padding:0 8px;font-size:11.5px;gap:5px}
.btn-xs{height:20px;padding:0 7px;font-size:11px;gap:4px}
.btn-ghost{background:transparent;border-color:transparent}
.btn-primary{background:var(--gaze-accent);border-color:var(--gaze-accent);color:var(--gaze-accent-text);font-weight:600}
.btn-primary:hover{background:var(--gaze-accent);box-shadow:inset 0 0 0 40px #00000022}
.btn-danger{color:var(--gaze-danger)}
.btn-primary.btn-danger{background:var(--gaze-danger);border-color:var(--gaze-danger);color:var(--gaze-bg)}
.btn:disabled,.btn:disabled:hover{background:transparent;border-color:var(--gaze-border);color:var(--gaze-border);box-shadow:none}
.btn-ghost:disabled,.btn-ghost:disabled:hover{border-color:transparent}
.icon-btn{position:relative;width:30px;height:30px;font-size:13px;color:var(--gaze-muted)}
.icon-btn:hover{background:var(--gaze-hover);color:var(--gaze-text)}
.icon-btn.on{color:var(--gaze-accent)}
.icon-btn-sm{width:24px;height:24px;border-radius:var(--radius-sm);font-size:11.5px}
.icon-btn:disabled,.icon-btn:disabled:hover{background:transparent;color:var(--gaze-border)}
.chip{position:relative;height:26px;padding:0 10px;gap:6px;border-color:var(--gaze-border);border-radius:13px;font-size:11.5px;color:var(--gaze-muted)}
.chip:hover{background:var(--gaze-hover);color:var(--gaze-text)}
.chip.on{background:var(--gaze-hover);border-color:var(--gaze-accent);color:var(--gaze-text)}
.chip.on>i{color:var(--gaze-accent)}
.segmented{display:flex;gap:2px;padding:2px;margin:0 8px;border:1px solid var(--gaze-border);border-radius:var(--radius-md);background:var(--gaze-bg)}
.segment{flex:1;height:30px;border-radius:var(--radius-sm);font-size:12px;color:var(--gaze-muted)}
.segment:hover{background:var(--gaze-hover);color:var(--gaze-text)}
.segment.on{background:var(--gaze-raised);color:var(--gaze-text);box-shadow:inset 0 0 0 1px var(--gaze-accent)}
.segment.on>i{color:var(--gaze-accent)}
.segment:disabled,.segment:disabled:hover{background:transparent;color:var(--gaze-border)}
.check{display:flex;align-items:center;gap:7px;cursor:pointer;font-size:12px;color:var(--gaze-muted);white-space:nowrap}
.check-box{display:flex;align-items:center;justify-content:center;width:16px;height:16px;border:1.5px solid var(--gaze-muted);border-radius:4px;font-size:10px;color:transparent}
.check.on{color:var(--gaze-text)}
.check.on .check-box{background:var(--gaze-accent);border-color:var(--gaze-accent);color:var(--gaze-accent-text)}
/* Badges hold bare text, so they are never flex boxes (R1). */
.tag{display:inline-block;height:18px;line-height:18px;padding:0 7px;border-radius:9px;font-size:10.5px;font-weight:600;background:var(--gaze-raised);color:var(--gaze-muted);white-space:nowrap}
.tag.ok{color:var(--gaze-success)}.tag.warn{color:var(--gaze-warning)}.tag.err{color:var(--gaze-danger)}
.count{display:inline-block;min-width:18px;height:18px;line-height:18px;padding:0 6px;border-radius:9px;text-align:center;font-size:11px;font-weight:600;background:var(--gaze-raised);color:var(--gaze-muted)}
/* Hover bubbles: Blitz shows no title tooltips. */
.tip{position:absolute;z-index:40;visibility:hidden;pointer-events:none;white-space:nowrap;padding:4px 8px;border-radius:var(--radius-sm);background:var(--gaze-text);color:var(--gaze-bg);font-size:12px;font-weight:400;line-height:1.3;box-shadow:0 4px 14px #00000055}
:hover>.tip{visibility:visible}
:disabled>.tip{visibility:hidden}
.tip-right{left:100%;top:5px;margin-left:8px}
.tip-below{top:100%;left:0;margin-top:6px}
.tip-below-end{top:100%;right:0;margin-top:6px}
/* Fields: visible labels, and ghost hints in place of placeholders. */
.field{position:relative;display:flex;align-items:center;min-width:0}
.field input{flex:1;width:100%}
.field-icon{position:absolute;left:10px;top:9px;font-size:11.5px;color:var(--gaze-muted);pointer-events:none}
.field-icon~input{padding-left:29px}
.ghost-hint{position:absolute;left:10px;right:8px;top:0;line-height:30px;overflow:hidden;white-space:nowrap;color:var(--gaze-muted);pointer-events:none}
.field-icon~.ghost-hint{left:29px}
.ghost-hint.off{visibility:hidden}
.form{padding:0 8px}
.field-label{display:block;margin:10px 0 4px;font-size:12px;font-weight:600}
.form-cols{display:flex;gap:8px}
.form-cols>div{flex:1;min-width:0}
.form-row{display:flex;align-items:center;gap:6px}
.form-actions{display:flex;justify-content:flex-end;gap:6px;margin-top:10px}
.form-actions.start{justify-content:flex-start}
/* Sections and rows */
.section-head{display:flex;align-items:center;gap:6px;padding:14px 8px 6px;font-size:11.5px;font-weight:600;color:var(--gaze-muted)}
.section-head:first-child{padding-top:4px}
.row{position:relative;display:flex;align-items:center;gap:8px;min-height:30px;padding:5px 8px;border-radius:var(--radius-md);cursor:pointer}
.row:hover{background:var(--gaze-hover)}
.row.static{cursor:default}
.row.static:hover{background:transparent}
.row.on{background:linear-gradient(var(--gaze-accent),var(--gaze-accent)) 0 0/3px 100% no-repeat,var(--gaze-hover)}
.row-icon{width:16px;flex:none;text-align:center;font-size:12px;color:var(--gaze-muted)}
.row.on .row-icon{color:var(--gaze-accent)}
.row-icon.warn{color:var(--gaze-warning)}.row-icon.err{color:var(--gaze-danger)}
.row-text{flex:1;min-width:0;display:flex;flex-direction:column;overflow:hidden}
.row-label{white-space:nowrap;overflow:hidden}
.row-detail{font:11px/1.45 var(--mono);color:var(--gaze-muted);white-space:nowrap;overflow:hidden}
.row-detail.hit{font-family:var(--sans);font-weight:600;color:var(--gaze-text)}
/* Search matches: colour and an underline only, so fitted widths hold. */
mark{background:transparent;color:inherit}
.row-label mark,.row-detail mark,.suggestion-title mark,.suggestion-url mark{color:var(--gaze-text);text-decoration-line:underline;text-decoration-color:var(--gaze-accent);text-decoration-thickness:2px;text-underline-offset:2px}
.row-desc{font-size:12px;line-height:1.4;color:var(--gaze-muted)}
.row-desc.why{overflow-wrap:anywhere}
.row-meta{flex:none;font-size:11px;color:var(--gaze-muted);white-space:nowrap}
.row-side{flex:none;display:flex;flex-direction:column;align-items:flex-end;gap:4px}
.row-actions{position:absolute;top:0;right:4px;bottom:0;display:flex;align-items:center;gap:2px;padding-left:18px;visibility:hidden;background:linear-gradient(to right,transparent,var(--gaze-hover) 16px)}
.row:hover>.row-actions,.row.on>.row-actions,.row.menu-open>.row-actions{visibility:visible}
.row-menu{display:flex;flex-direction:column;gap:1px;margin:2px 4px 8px 28px;padding:4px;border:1px solid var(--gaze-border);border-radius:var(--radius-lg);background:var(--gaze-raised)}
.menu-item{justify-content:flex-start;width:100%;height:28px;padding:0 8px;gap:9px;border-radius:var(--radius-sm);font-size:12.5px}
.menu-item>i{width:14px;color:var(--gaze-muted)}
.menu-item:hover{background:var(--gaze-hover)}
.menu-item:disabled,.menu-item:disabled>i{color:var(--gaze-border)}
.menu-item:disabled:hover{background:transparent}
.menu-item.danger,.menu-item.danger>i{color:var(--gaze-danger)}
.menu-sep{height:1px;margin:3px 6px;background:var(--gaze-border)}
/* Cards, notices, empty states */
.card{margin:6px 0 8px;padding:10px 12px;border:1px solid var(--gaze-border);border-radius:var(--radius-lg);background:var(--gaze-bg)}
.card-head{display:flex;align-items:center;gap:8px;margin-bottom:6px}
.card-title{flex:1;min-width:0;font-weight:600;white-space:nowrap;overflow:hidden}
.card-text{margin:4px 0;font-size:12px;color:var(--gaze-muted)}
.card-actions{display:flex;flex-wrap:wrap;gap:6px;margin-top:10px}
.card-actions.end{justify-content:flex-end}
.facts{display:flex;flex-wrap:wrap;gap:4px 12px;font-size:12px;color:var(--gaze-muted)}
.kv{display:flex;flex-direction:column;gap:5px;margin:6px 0 2px}
.kv-row{display:flex;gap:10px}
.kv-key{width:62px;flex:none;font-size:12px;color:var(--gaze-muted)}
.kv-value{flex:1;min-width:0;overflow:hidden;white-space:nowrap}
.notice{--bar:var(--gaze-border);--fill:var(--gaze-bg);display:flex;align-items:flex-start;gap:9px;margin:6px 0;padding:8px 10px;border-radius:var(--radius-md);background:linear-gradient(var(--bar),var(--bar)) 0 0/3px 100% no-repeat,var(--fill);font-size:12px}
.card .notice{--fill:var(--gaze-surface)}
.notice>i{margin-top:2px;color:var(--bar)}
.notice-text{flex:1;min-width:0;overflow-wrap:anywhere}
.notice.info{--bar:var(--gaze-accent)}.notice.ok{--bar:var(--gaze-success)}
.notice.warn{--bar:var(--gaze-warning)}.notice.err{--bar:var(--gaze-danger)}
.empty-state{display:flex;flex-direction:column;align-items:center;gap:8px;padding:32px 16px;text-align:center;color:var(--gaze-muted)}
.empty-state>i{font-size:22px}
.footnote{display:block;padding:12px 8px 4px;font-size:11.5px;color:var(--gaze-muted)}
.wallet-address{margin:2px 0 6px;white-space:nowrap;overflow:hidden}
/* Tab strip */
#tabs{flex:none;display:flex;align-items:flex-end;gap:4px;height:38px;padding:4px 8px 0;background:var(--gaze-bg);border-bottom:1px solid var(--gaze-border)}
#tablist{display:flex;align-items:flex-end;gap:2px;min-width:0;height:100%;overflow:hidden}
.tab{position:relative;display:flex;align-items:center;gap:7px;flex:none;height:33px;padding:0 6px 0 10px;border:1px solid transparent;border-bottom:0;border-radius:8px 8px 0 0;color:var(--gaze-muted);cursor:pointer;white-space:nowrap;overflow:hidden}
.tab:hover{background:var(--gaze-raised);color:var(--gaze-text)}
.tab.on{background:linear-gradient(var(--gaze-accent),var(--gaze-accent)) 0 0/100% 2px no-repeat,var(--gaze-surface);border-color:var(--gaze-border);color:var(--gaze-text)}
.tab.narrow{justify-content:center;padding:0;gap:0}
.tab.narrow .tab-title,.tab.narrow .tab-close{display:none}
.tab-icon{flex:none;width:14px;text-align:center;font-size:11px}
.tab-icon.warn{color:var(--gaze-warning)}.tab-icon.err{color:var(--gaze-danger)}
.tab-title{flex:1;min-width:0;overflow:hidden}
.tab-close{flex:none;display:flex;align-items:center;justify-content:center;width:20px;height:20px;border-radius:5px;font-size:11px;visibility:hidden}
.tab:hover .tab-close,.tab.on .tab-close{visibility:visible}
/* An inactive tab's close button overlays the end of its title on hover,
   so the title has the room when it is not. */
.tab:not(.on) .tab-close{position:absolute;right:6px;top:6px;background:var(--gaze-raised);box-shadow:-6px 0 6px 2px var(--gaze-raised)}
.tab-close:hover,.tab:not(.on) .tab-close:hover{background:var(--gaze-hover)}
#newtab{flex:none;margin-bottom:2px}
#tabmore{flex:none;width:44px;margin-bottom:5px}
/* Toolbar and address box */
#toolbar{flex:none;display:flex;align-items:center;gap:2px;height:46px;padding:0 8px;background:var(--gaze-surface);border-bottom:1px solid var(--gaze-border)}
#urlwrap{position:relative;flex:1;min-width:140px;display:flex;align-items:center;height:32px;margin:0 6px;background:var(--gaze-bg);border:1px solid var(--gaze-border);border-radius:9px}
#urlwrap.focus{border-color:var(--gaze-accent);box-shadow:0 0 0 1px var(--gaze-accent)}
#scheme{flex:none;display:flex;align-items:center;padding-left:3px}
.scheme-badge{position:relative;display:flex;align-items:center;gap:5px;height:24px;padding:0 7px;border-radius:6px;font-size:12px;color:var(--gaze-muted);cursor:pointer}
.scheme-badge:hover{background:var(--gaze-hover)}
.scheme-badge.warn{color:var(--gaze-warning)}
.scheme-badge.acc>i{color:var(--gaze-accent)}
.inbox{position:relative;flex:1;min-width:0;display:flex;align-items:center;height:100%}
#url{flex:1;width:100%;height:100%;padding:0 10px 0 5px;border:0;background:transparent}
#url:focus{outline:none}
.inbox .ghost-hint{left:5px}
#suggestions{position:absolute;left:-1px;right:-1px;top:100%;margin-top:6px;z-index:30;padding:4px;background:var(--gaze-surface);border:1px solid var(--gaze-border);border-radius:var(--radius-lg);box-shadow:0 12px 32px #00000066}
#suggestions:empty{display:none}
.suggestion{display:flex;align-items:center;gap:10px;height:32px;padding:0 10px;border-radius:var(--radius-md);cursor:pointer}
.suggestion:hover,.suggestion.on{background:var(--gaze-hover)}
.suggestion>i{width:16px;flex:none;text-align:center;color:var(--gaze-muted)}
.suggestion-title{flex:none;overflow:hidden;white-space:nowrap}
.suggestion-url{flex:1;min-width:0;overflow:hidden;white-space:nowrap;font:11.5px var(--mono);color:var(--gaze-muted)}
/* Prompt bar */
#prompt:empty{display:none}
.prompt-bar{display:flex;align-items:center;gap:10px;min-height:44px;padding:6px 12px;background:linear-gradient(var(--gaze-warning),var(--gaze-warning)) 0 0/3px 100% no-repeat,var(--gaze-raised);border-bottom:1px solid var(--gaze-border)}
.prompt-icon{flex:none;font-size:15px;color:var(--gaze-warning)}
.prompt-text{flex:1;min-width:0}
.prompt-site{position:relative;font-weight:600}
/* Workspace, rail, sidebar */
#workspace{display:flex;flex:1;min-height:0}
#rail{width:42px;flex:none;display:flex;flex-direction:column;align-items:center;gap:2px;padding:6px 0;background:var(--gaze-bg);border-right:1px solid var(--gaze-border)}
.rail-btn{position:relative;width:34px;height:34px;font-size:15px;color:var(--gaze-muted)}
.rail-btn:hover{background:var(--gaze-hover);color:var(--gaze-text)}
.rail-btn.on{background:linear-gradient(var(--gaze-accent),var(--gaze-accent)) 0 0/3px 100% no-repeat border-box,var(--gaze-hover);color:var(--gaze-accent)}
#sidebar{width:300px;flex:none;display:flex;flex-direction:column;min-height:0;background:var(--gaze-surface);border-right:1px solid var(--gaze-border)}
#sidehead{flex:none;display:flex;align-items:center;gap:8px;height:44px;padding:0 6px 0 14px}
.side-title{font-size:14px;font-weight:600}
#sidecontrols{flex:none;display:flex;align-items:center;gap:6px;padding:0 6px 10px}
#sidecontrols:empty{display:none}
#sidecontrols .field{flex:1}
#panel{flex:1;min-height:0;overflow:auto;padding:2px 6px 16px}
#wallet{flex:1;min-height:0;overflow:auto;padding:2px 6px 16px}
/* Page area and find */
#main{flex:1;position:relative;display:flex;min-width:0;min-height:0;background:white}
.view{flex:1;width:100%;height:100%;overflow:hidden}
#findoverlay{position:absolute;inset:0;overflow:hidden;pointer-events:none;z-index:2}
.findhit{position:absolute;border-radius:2px;background:#ffd54f59}
.findhit.active{background:transparent;box-shadow:0 0 0 2px #e8710a}
#findbar{position:absolute;top:10px;right:18px;z-index:6;display:flex;align-items:center;gap:2px;width:360px;padding:4px;background:var(--gaze-surface);border:1px solid var(--gaze-border);border-radius:var(--radius-lg);box-shadow:0 8px 24px #00000055}
#findbar.off{visibility:hidden}
#find{padding-right:70px}
#findcount{position:absolute;right:9px;top:0;line-height:30px;font-size:11.5px;color:var(--gaze-muted);pointer-events:none}
#findcount.none{color:var(--gaze-danger)}
/* Status bar */
#status{flex:none;display:flex;align-items:center;gap:12px;height:24px;padding:0 10px;background:var(--gaze-bg);border-top:1px solid var(--gaze-border);color:var(--gaze-muted);font-size:11.5px;white-space:nowrap;overflow:hidden}
.stage-chip{flex:none;display:flex;align-items:center;gap:5px}
.stage-chip.ok>i{color:var(--gaze-success)}.stage-chip.info>i{color:var(--gaze-accent)}
.stage-chip.warn{color:var(--gaze-warning)}.stage-chip.err{color:var(--gaze-danger)}
.status-text{min-width:0;overflow:hidden}
.status-warn{min-width:0;overflow:hidden;color:var(--gaze-warning)}
.flash{flex:none;display:flex;align-items:center;gap:5px;color:var(--gaze-text)}
.flash.ok>i{color:var(--gaze-success)}.flash.info>i{color:var(--gaze-accent)}
.flash.warn>i{color:var(--gaze-warning)}.flash.err{color:var(--gaze-danger)}
/* Console and appearance */
.log-line{--bar:var(--gaze-border);display:flex;gap:8px;margin:1px 0;padding:5px 8px;border-radius:var(--radius-sm);background:linear-gradient(var(--bar),var(--bar)) 0 0/3px 100% no-repeat;font:11.5px/1.45 var(--mono)}
.log-level{flex:none;width:40px;color:var(--gaze-muted)}
.log-text{flex:1;min-width:0;white-space:pre-wrap;overflow-wrap:anywhere}
.log-line.warn{--bar:var(--gaze-warning)}.log-line.warn .log-level{color:var(--gaze-warning)}
.log-line.err{--bar:var(--gaze-danger)}.log-line.err .log-level{color:var(--gaze-danger)}
.log-line.debug .log-text{color:var(--gaze-muted)}
.swatches{display:flex;flex-wrap:wrap;gap:6px 10px;padding:2px 8px}
.swatch{display:flex;align-items:center;gap:6px;width:126px}
.swatch-color{width:16px;height:16px;flex:none;border-radius:4px;border:1px solid var(--gaze-border)}
.swatch-name{font:11px var(--mono);color:var(--gaze-muted)}
"#;

// The stylesheet before the UIX overhaul (2026-10-04). It was disabled
// because every region now uses the component classes in `CSS` above
// (docs/ui/README.md, "Component vocabulary"); kept for reference.
//
// const LEGACY_CSS: &str = r#"
// html,body{margin:0;height:100%;font:13px 'Noto Sans',system-ui,sans-serif;color:var(--gaze-text);background:var(--gaze-bg)}
// body{display:flex;flex-direction:column}*{box-sizing:border-box}
// button,input{font:inherit;color:inherit}button{border:1px solid var(--gaze-border);border-radius:6px;background:var(--gaze-raised);padding:6px 10px;cursor:pointer}button:hover,.tab:hover,.railbtn:hover,.item:hover{background:var(--gaze-hover)}button:focus,input:focus{outline:2px solid var(--gaze-accent);outline-offset:1px}
// .ico{display:inline-block;min-width:15px;text-align:center}.muted{color:var(--gaze-muted)}.warn{color:var(--gaze-warning)}.hidden{display:none!important}
// #tabs{display:flex;align-items:end;gap:2px;padding:5px 8px 0;background:var(--gaze-bg);border-bottom:1px solid var(--gaze-border);min-height:38px;overflow:hidden}
// .tab{display:flex;align-items:center;gap:8px;max-width:220px;min-width:100px;padding:7px 8px;background:var(--gaze-raised);border:1px solid var(--gaze-border);border-bottom:0;border-radius:7px 7px 0 0;cursor:pointer;white-space:nowrap;overflow:hidden}
// .tab.on{background:var(--gaze-surface);border-top:2px solid var(--gaze-accent);padding-top:6px}.tabtitle{flex:1;overflow:hidden;white-space:nowrap}.tab .x{color:var(--gaze-muted);padding:0 3px}#newtab{margin:0 4px 4px}
// #toolbar{display:flex;align-items:center;gap:6px;padding:7px 10px;background:var(--gaze-surface);border-bottom:1px solid var(--gaze-border);flex:none}
// #toolbar button{min-width:30px}#url{flex:1;min-width:90px;padding:7px 10px;background:var(--gaze-bg);border:1px solid var(--gaze-border);border-radius:7px}
// #suggestions{background:var(--gaze-surface);border-bottom:1px solid var(--gaze-border)}#suggestions:empty{display:none}
// .suggest{padding:5px 14px;display:flex;gap:12px;cursor:pointer}.suggest .muted{overflow:hidden;white-space:nowrap}
// .bar{display:flex;align-items:center;gap:8px;padding:8px 12px;background:var(--gaze-raised);border-bottom:1px solid var(--gaze-warning)}.bar .text{flex:1}
// #workspace{display:flex;flex:1;min-height:0}#rail{width:42px;flex:none;background:var(--gaze-bg);border-right:1px solid var(--gaze-border);display:flex;flex-direction:column;align-items:center;padding:7px 3px;gap:5px}
// .railbtn{width:34px;height:34px;text-align:center;padding:7px;border:0;background:transparent;color:var(--gaze-muted);border-radius:6px;cursor:pointer}.railbtn.on{background:var(--gaze-hover);color:var(--gaze-accent)}
// #sidebar{width:290px;flex:none;display:flex;flex-direction:column;min-height:0;background:var(--gaze-surface);border-right:1px solid var(--gaze-border)}#sidebar.hidden{display:none}
// #sidehead{display:flex;align-items:center;justify-content:space-between;padding:12px 12px 8px;font-weight:600}#sidecontrols{padding:0 10px 10px;display:flex;gap:5px;flex-wrap:wrap}#sidecontrols:empty{display:none}
// #sidecontrols input{flex:1;min-width:110px;padding:7px 8px;background:var(--gaze-bg);border:1px solid var(--gaze-border);border-radius:6px}#sidecontrols button{padding:5px 7px}.search-label{color:var(--gaze-muted);align-self:center;padding:0 2px}
// #panel{flex:1;overflow:auto;padding:6px 8px 14px}.item{padding:8px;border-radius:6px;cursor:pointer;display:flex;align-items:center;gap:7px}.item.on{background:var(--gaze-hover);color:var(--gaze-accent)}.item .label{flex:1;overflow:hidden;white-space:nowrap}.sub{font:11px 'Gaze Fira Code',monospace;color:var(--gaze-muted);overflow:hidden;white-space:nowrap}.itemwrap{border-bottom:1px solid var(--gaze-border)}.itemwrap:last-child{border:0}.itemactions{padding:0 8px 7px;display:flex;gap:4px;flex-wrap:wrap}.itemactions button{font-size:11px;padding:3px 5px}
// #pagestack{flex:1;display:flex;flex-direction:column;min-width:0;min-height:0}#findbar{display:flex;align-items:center;gap:6px;padding:6px 10px;background:var(--gaze-surface);border-bottom:1px solid var(--gaze-border)}#findbar input{flex:1;padding:5px 8px;background:var(--gaze-bg);border:1px solid var(--gaze-border);border-radius:6px}#findbar button{padding:4px 8px}#findcount{color:var(--gaze-muted);min-width:45px;text-align:center}
// #main{flex:1;position:relative;display:flex;min-height:0;background:white}.view{flex:1;width:100%;height:100%;overflow:hidden}#findoverlay{position:absolute;inset:0;overflow:hidden;pointer-events:none;z-index:2}.findhit{position:absolute;background:#ffe27988;border-radius:2px}.findhit.active{background:#70d5cfaa;outline:1px solid #186e70}
// #wallet{overflow:auto;padding:4px}#wallet .row{display:flex;gap:5px;align-items:center;margin:5px 0;flex-wrap:wrap}#wallet input{min-width:0;padding:4px 6px;border:1px solid var(--gaze-border);border-radius:5px;background:var(--gaze-bg)}#wallet .addr{font:11px 'Fira Code',monospace;overflow:hidden}#wallet .active{font-weight:600}#walletmsg{color:var(--gaze-muted);margin-top:5px}
// #status{flex:none;padding:5px 11px;background:var(--gaze-bg);border-top:1px solid var(--gaze-border);color:var(--gaze-muted);font-size:11px;display:flex;gap:10px;align-items:center}#status button{font-size:11px;padding:1px 6px}
// "#;

// ── Markup templates ────────────────────────────────────────────────────

/// The window skeleton; [`shell_html`] fills in the placeholders.
const SHELL: &str = r#"<html><head><title>F1R3Gaze</title><style>__CSS__</style></head><body>
<div id="tabs">__TAB_STRIP__</div>
<div id="toolbar">__TOOLBAR__</div>
<div id="prompt"></div>
<div id="workspace"><div id="rail">__RAIL__</div><div id="sidebar" class="__SIDEBAR_CLASS__"><div id="sidehead"></div><div id="sidecontrols"></div><div id="panel"></div><div id="wallet" class="hidden">__WALLET_FORM__</div></div><div id="main"><div id="findoverlay"></div><div id="findbar" class="off">__FIND_BOX__</div></div></div>
<div id="status"></div>
</body></html>"#;

/// The tab list (filled per render) and the new-tab button, which stays
/// outside the list so that it is never clipped.
const TAB_STRIP_HTML: &str = r#"<div id="tablist"></div><button id="newtab" class="icon-btn" data-action="newtab" aria-label="New tab"><i class="fa-solid fa-plus"></i><span class="tip tip-below">New tab · {key:newtab}</span></button>"#;

const TOOLBAR_HTML: &str = r#"<button id="sidetoggle" class="icon-btn" data-action="sidebar:toggle" aria-label="Sidebar"><i class="fa-solid fa-table-columns"></i><span class="tip tip-below">Show or hide the sidebar · {key:sidebar}</span></button>
<button id="back" class="icon-btn" data-action="back" aria-label="Back"><i class="fa-solid fa-arrow-left"></i><span class="tip tip-below">Back</span></button>
<button id="fwd" class="icon-btn" data-action="fwd" aria-label="Forward"><i class="fa-solid fa-arrow-right"></i><span class="tip tip-below">Forward</span></button>
<button id="reload" class="icon-btn" data-action="reload" aria-label="Reload"><i class="fa-solid fa-rotate-right"></i><span class="tip tip-below">Reload · {key:reload}</span></button>
<div id="urlwrap"><span id="scheme"></span><span class="inbox"><input id="url" type="text" value="" aria-label="Address"><span id="url-ph" class="ghost-hint off">Enter an address, or search tabs and history</span></span><div id="suggestions"></div></div>
<!-- Go was removed: Enter opens the address, and its arrow icon was the same as Forward's.
<button data-action="go" title="Open address"><i class="fa-solid fa-arrow-right"></i></button> -->
<button id="findbtn" class="icon-btn" data-action="find:show" aria-label="Find in page"><i class="fa-solid fa-magnifying-glass"></i><span class="tip tip-below-end">Find in page · {key:find}</span></button>
<!-- The menu button was removed: it did the same as the rail's Tabs button.
<button data-action="panel:tabs" title="Toggle tab sidebar"><i class="fa-solid fa-bars"></i></button> -->
<!-- Scheme cycling was removed: Appearance shows and sets the color scheme (theme:next no longer parses).
<button data-action="theme:next" title="Change color scheme"><i class="fa-solid fa-palette"></i></button> -->"#;

const RAIL_HTML: &str = r#"<button id="rail-tabs" class="rail-btn" data-action="panel:tabs" aria-label="Tabs"><i class="fa-solid fa-layer-group"></i><span class="tip tip-right">Tabs</span></button>
<button id="rail-history" class="rail-btn" data-action="panel:history" aria-label="History"><i class="fa-solid fa-clock-rotate-left"></i><span class="tip tip-right">History · {key:history}</span></button>
<button id="rail-sites" class="rail-btn" data-action="panel:sites" aria-label="Site data"><i class="fa-solid fa-database"></i><span class="tip tip-right">Site data</span></button>
<button id="rail-grants" class="rail-btn" data-action="panel:grants" aria-label="Permissions"><i class="fa-solid fa-shield-halved"></i><span class="tip tip-right">Permissions</span></button>
<button id="rail-wallet" class="rail-btn" data-action="panel:wallet" aria-label="Wallet"><i class="fa-solid fa-wallet"></i><span class="tip tip-right">Wallet</span></button>
<button id="rail-console" class="rail-btn" data-action="panel:console" aria-label="Console"><i class="fa-solid fa-terminal"></i><span class="tip tip-right">Console</span></button>
<span class="grow"></span>
<button id="rail-appearance" class="rail-btn" data-action="panel:appearance" aria-label="Appearance"><i class="fa-solid fa-gear"></i><span class="tip tip-right">Appearance</span></button>"#;

const FIND_BOX_HTML: &str = r#"<div class="field grow"><i class="fa-solid fa-magnifying-glass field-icon"></i><input id="find" type="text" aria-label="Find in page"><span id="find-ph" class="ghost-hint">Find in page</span><span id="findcount"></span></div><button class="icon-btn icon-btn-sm" data-action="find:prev" aria-label="Previous match"><i class="fa-solid fa-chevron-up"></i><span class="tip tip-below-end">Previous · Shift+Enter</span></button><button class="icon-btn icon-btn-sm" data-action="find:next" aria-label="Next match"><i class="fa-solid fa-chevron-down"></i><span class="tip tip-below-end">Next · Enter</span></button><button class="icon-btn icon-btn-sm" data-action="find:hide" aria-label="Close find"><i class="fa-solid fa-xmark"></i><span class="tip tip-below-end">Close · Esc</span></button>"#;

/// The wallet panel's form. Its fields keep what is typed into them across
/// renders, so they are never rebuilt; `#walletmsg`, `#walletcard` and
/// `#walletlist` are rendered.
const WALLET_FORM_HTML: &str = r#"<div id="walletmsg"></div><div id="walletcard"></div>
<section id="wallet-send-form"><div class="section-head"><span>Send</span></div><div class="form">
<label class="field-label" for="wallet-to"><span>Recipient address</span></label><div class="field"><input id="wallet-to" type="text" aria-label="Recipient address"></div>
<div class="form-cols"><div><label class="field-label" for="wallet-amount"><span>Amount</span></label><div class="field"><input id="wallet-amount" type="text" aria-label="Amount"></div></div>
<div><label class="field-label" for="wallet-desc"><span>Note (optional)</span></label><div class="field"><input id="wallet-desc" type="text" aria-label="Note"></div></div></div>
<div class="form-actions"><button id="wallet-send" class="btn btn-primary" data-action="wallet:send"><i class="fa-solid fa-paper-plane"></i><span>Review transfer</span></button></div></div></section>
<div id="walletlist"></div>
<div class="section-head"><span>Add a wallet</span></div><div class="form">
<div class="form-actions start"><button class="btn" data-action="wallet:new"><i class="fa-solid fa-plus"></i><span>Create new wallet</span></button></div>
<label class="field-label" for="wallet-import"><span>Import an F1R3Sky wallet file or hex key</span></label>
<div class="form-row"><div class="field grow"><input id="wallet-import" type="text" aria-label="Wallet file or hex key"></div><button class="btn" data-action="wallet:import"><i class="fa-solid fa-file-import"></i><span>Import</span></button></div></div>"#;

/// Per-panel controls. They are constant, so `#sidecontrols` stays
/// byte-identical while the user types and the field keeps focus (R4).
const TABS_CONTROLS_HTML: &str = r#"<div class="field"><i class="fa-solid fa-magnifying-glass field-icon"></i><input id="side-search" type="text" aria-label="Search tabs"><span id="side-ph" class="ghost-hint">Search tabs</span></div><button id="chip-tree" class="chip" data-action="tree:toggle" aria-pressed="false"><i class="fa-solid fa-sitemap"></i><span>Tree</span><span class="tip tip-below-end">Nest tabs under the tab that opened them</span></button><button id="chip-pages" class="chip" data-action="side:pages" aria-pressed="false"><i class="fa-solid fa-file-lines"></i><span>Page text</span><span class="tip tip-below-end">Also search the text of loaded pages</span></button>"#;
const HISTORY_CONTROLS_HTML: &str = r#"<div class="field"><i class="fa-solid fa-magnifying-glass field-icon"></i><input id="side-search" type="text" aria-label="Search history"><span id="side-ph" class="ghost-hint">Search history</span></div><button id="history-clear" class="btn btn-sm btn-ghost btn-danger" data-action="history:clear-ask:0"><i class="fa-solid fa-trash-can"></i><span>Clear…</span></button>"#;
const CONSOLE_CONTROLS_HTML: &str = r#"<button class="btn btn-sm" data-action="savelog"><i class="fa-solid fa-floppy-disk"></i><span>Save replay log</span></button><button class="btn btn-sm btn-ghost" data-action="console:clear"><i class="fa-solid fa-eraser"></i><span>Clear</span></button>"#;

fn side_controls_html(panel: &str) -> &'static str {
    match panel {
        "tabs" => TABS_CONTROLS_HTML,
        "history" => HISTORY_CONTROLS_HTML,
        "console" => CONSOLE_CONTROLS_HTML,
        _ => "",
    }
}

/// The chrome stylesheet for a scheme: fonts and icons, colour variables,
/// then [`CSS`].
// The window resolves its colours (`chrome_css_of`); the tests still name
// the old schemes.
#[cfg(test)]
fn chrome_css(scheme: &str, custom: Option<&BTreeMap<String, String>>) -> String {
    chrome_css_of(&theme::palette(scheme, custom))
}

/// The chrome's style sheet for resolved colours (`theme::resolve`).
fn chrome_css_of(colours: &BTreeMap<String, String>) -> String {
    format!("{}{}{}", theme::font_css(), theme::variables_of(colours), CSS)
}

/// The keys the chrome's hover tips and tags name, on each platform:
/// `(token in the markup, elsewhere, on macOS)`. The command key is Cmd on
/// macOS (ledger L12), and History is Cmd+Y there (L16); the tips named Ctrl
/// on every platform (L17).
const KEY_LABELS: [(&str, &str, &str); 6] = [
    ("{key:newtab}", "Ctrl+T", "Cmd+T"),
    ("{key:sidebar}", "Ctrl+B", "Cmd+B"),
    ("{key:reload}", "Ctrl+R", "Cmd+R"),
    ("{key:find}", "Ctrl+F", "Cmd+F"),
    ("{key:history}", "Ctrl+H", "Cmd+Y"),
    ("{key:reopen}", "Ctrl+Shift+T", "Cmd+Shift+T"),
];

/// The label of the key `token` names, on `platform`.
fn key_label(token: &str, platform: KeyPlatform) -> &'static str {
    let (_, other, mac) = KEY_LABELS
        .iter()
        .find(|(name, _, _)| *name == token)
        .expect("every key token is in KEY_LABELS");
    match platform {
        KeyPlatform::Other => other,
        KeyPlatform::MacOs => mac,
    }
}

/// `html` with every key token replaced by its label on `platform`.
fn key_labels(html: &str, platform: KeyPlatform) -> String {
    KEY_LABELS
        .iter()
        .fold(html.to_string(), |html, (token, _, _)| html.replace(token, key_label(token, platform)))
}

/// The window's initial markup.
fn shell_html(css: &str, sidebar_open: bool) -> String {
    // Was the templates as they are: their tips named Ctrl on every
    // platform (ledger L17).
    let html = SHELL
        .replace("__CSS__", css)
        .replace("__TAB_STRIP__", TAB_STRIP_HTML)
        .replace("__TOOLBAR__", TOOLBAR_HTML)
        .replace("__RAIL__", RAIL_HTML)
        .replace("__WALLET_FORM__", WALLET_FORM_HTML)
        .replace("__FIND_BOX__", FIND_BOX_HTML)
        .replace(
            "__SIDEBAR_CLASS__",
            if sidebar_open { "" } else { "hidden" },
        );
    key_labels(&html, KeyPlatform::CURRENT)
}

// ── Actions ─────────────────────────────────────────────────────────────

/// Everything the chrome can be asked to do. Markup names actions as
/// `data-action="verb:argument"` strings ([`Action::parse`]).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Back,
    Fwd,
    Reload,
    /// Open the address in the active tab (the typed text when `None`).
    Go(Option<String>),
    NewTab,
    Close(u64),
    Select(u64),
    Answer(u64, u64, bool),
    Remember,
    Revoke(String),
    /// Show a panel, or hide the sidebar if that panel is already showing.
    Panel(String),
    /// Show a panel; never hides the sidebar.
    ShowPanel(String),
    SidebarToggle,
    LetItRun,
    SaveLog,
    Wallet(String),
    FocusUrl,
    Input(String, String),
    Theme(ThemeOp),
    Find(String),
    FindNext(i32),
    FindHide,
    TabOp(String, u64),
    HistoryOp(String, usize),
    /// `open` or `forget` a page of history, by address.
    VisitOp(String, String),
    ToggleTree,
    ToggleSidePages,
    SideClear,
    SiteOp(String, String),
    ConsoleClear,
    /// Enter in the address box: the highlighted suggestion, or the text.
    AddressSubmit,
    /// Arrow keys in the address box: move through the suggestions.
    SuggestMove(i32),
    /// Escape in the address box, or leaving it: drop the suggestions and
    /// show the page's address again.
    AddressDismiss,
    /// Escape elsewhere: close whatever is open (dropdown, menu, a
    /// confirmation).
    Dismiss,
    /// F11, or Ctrl+Cmd+F on macOS: enter or leave full screen (the window
    /// does it: `WindowRequest::ToggleFullScreen`). No markup emits it.
    FullScreen,
}

impl Action {
    fn parse(s: &str) -> Option<Action> {
        let (head, rest) = s.split_once(':').unwrap_or((s, ""));
        let num = |x: &str| x.parse::<u64>().ok();
        Some(match head {
            "back" => Action::Back,
            "fwd" => Action::Fwd,
            "reload" => Action::Reload,
            "go" => Action::Go(None),
            "suggest" => Action::Go(Some(rest.into())),
            "newtab" => Action::NewTab,
            "close" => Action::Close(num(rest)?),
            "select" => Action::Select(num(rest)?),
            "allow" | "deny" => {
                let (t, p) = rest.split_once(':')?;
                Action::Answer(num(t)?, num(p)?, head == "allow")
            }
            "remember" => Action::Remember,
            "revoke" => Action::Revoke(rest.to_string()),
            "panel" => Action::Panel(rest.to_string()),
            "show" => Action::ShowPanel(rest.to_string()),
            "sidebar" => match rest {
                "toggle" => Action::SidebarToggle,
                _ => return None,
            },
            "letitrun" => Action::LetItRun,
            "savelog" => Action::SaveLog,
            "wallet" => Action::Wallet(rest.to_string()),
            "theme" => Action::Theme(ThemeOp::parse(rest)?),
            "find" => match rest {
                "show" => Action::Find(String::new()),
                "next" => Action::FindNext(1),
                "prev" => Action::FindNext(-1),
                "hide" => Action::FindHide,
                _ => return None,
            },
            "tab" => {
                let (verb, id) = rest.split_once(':')?;
                Action::TabOp(verb.into(), num(id)?)
            }
            "history" => {
                let (verb, id) = rest.split_once(':')?;
                Action::HistoryOp(verb.into(), id.parse().ok()?)
            }
            "visit" => {
                let (verb, url) = rest.split_once(':')?;
                Action::VisitOp(verb.into(), url.into())
            }
            "tree" => Action::ToggleTree,
            "side" => match rest {
                "pages" => Action::ToggleSidePages,
                "clear" => Action::SideClear,
                _ => return None,
            },
            "site" => {
                let (verb, site) = rest.split_once(':')?;
                Action::SiteOp(verb.into(), site.into())
            }
            "console" => match rest {
                "clear" => Action::ConsoleClear,
                _ => return None,
            },
            "dismiss" => Action::Dismiss,
            _ => return None,
        })
    }
}

/// What the Appearance panel's controls ask (`data-action="theme:<op>"`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum ThemeOp {
    /// Follow the system's light or dark preference (`theme = "system"`).
    System,
    /// The built-in schemes.
    Dark,
    Light,
    /// A theme file, `themes/<name>.css`: `theme:use:<name>`.
    Use(String),
    /// The colours shown, saved as a new theme file, which is then chosen.
    New,
    /// The theme files, and the chosen theme, read again.
    Reload,
}

impl ThemeOp {
    /// `system`, `dark`, `light`, `use:<name>`, `new` or `reload`; anything
    /// else is refused. The step-9 strings `next`, `template` and `custom`
    /// no longer parse, and a name must be a theme name
    /// (`theme::check_theme_name`).
    fn parse(argument: &str) -> Option<ThemeOp> {
        match argument.split_once(':') {
            None => match argument {
                "system" => Some(ThemeOp::System),
                "dark" => Some(ThemeOp::Dark),
                "light" => Some(ThemeOp::Light),
                "new" => Some(ThemeOp::New),
                "reload" => Some(ThemeOp::Reload),
                _ => None,
            },
            Some(("use", name)) if theme::check_theme_name(name).is_ok() => Some(ThemeOp::Use(name.to_string())),
            Some(_) => None,
        }
    }
}

fn attr_of(doc: &BaseDocument, id: NodeId, k: &str) -> Option<String> {
    doc.get_node(id)?
        .element_data()?
        .attrs()
        .iter()
        .find(|a| a.name.local.as_ref() == k)
        .map(|a| a.value.clone())
}

struct ChromeHandler<'a> {
    actions: &'a mut Vec<Action>,
    url_node: Option<NodeId>,
    find_node: Option<NodeId>,
    side_node: Option<NodeId>,
    drag: &'a mut Option<u64>,
}

struct ChromeNetWake(WakeHandle);
impl NetWaker for ChromeNetWake {
    fn wake(&self, _doc: usize) {
        self.0.wake();
    }
}

impl EventHandler for ChromeHandler<'_> {
    fn handle_event(
        &mut self,
        chain: &[NodeId],
        event: &mut DomEvent,
        doc: &mut dyn Document,
        state: &mut EventState,
    ) {
        let d = doc.inner();
        match &event.data {
            DomEventData::Click(_) => {
                for id in chain {
                    // Blitz delivers clicks on disabled controls to handlers
                    // too (only the default action is suppressed).
                    if attr_of(&d, *id, "disabled").is_some() {
                        break;
                    }
                    if let Some(a) = attr_of(&d, *id, "data-action").and_then(|s| Action::parse(&s))
                    {
                        self.actions.push(a);
                        state.request_redraw();
                        break;
                    }
                }
            }
            DomEventData::Input(_) => {
                let name = if Some(event.target) == self.url_node {
                    "url"
                } else if Some(event.target) == self.find_node {
                    "find"
                } else if Some(event.target) == self.side_node {
                    "side-search"
                } else {
                    ""
                };
                if !name.is_empty() {
                    let value = d
                        .get_node(event.target)
                        .and_then(|n| n.element_data())
                        .and_then(|e| e.text_input_data())
                        .map(|t| t.editor.raw_text().to_string())
                        .unwrap_or_default();
                    self.actions.push(Action::Input(name.into(), value));
                }
            }
            // Leaving the address box drops its suggestions. A click on a
            // suggestion is handled first: Blitz moves focus in the click's
            // default action, after handlers have seen the click.
            DomEventData::Blur(_) if Some(event.target) == self.url_node => {
                self.actions.push(Action::AddressDismiss);
            }
            DomEventData::PointerDown(p) if p.button == MouseEventButton::Main => {
                *self.drag = chain
                    .iter()
                    .find_map(|id| attr_of(&d, *id, "data-tab-id").and_then(|s| s.parse().ok()));
            }
            DomEventData::PointerUp(p) => {
                let target = chain.iter().find_map(|id| {
                    attr_of(&d, *id, "data-tab-id").and_then(|s| s.parse::<u64>().ok())
                });
                if p.button == MouseEventButton::Auxiliary {
                    if let Some(id) = target {
                        self.actions.push(Action::Close(id));
                    }
                } else if p.button == MouseEventButton::Main
                    && let (Some(from), Some(to)) = (self.drag.take(), target)
                    && from != to
                {
                    self.actions
                        .push(Action::TabOp("move-to".into(), (from << 32) | to));
                }
            }
            DomEventData::ContextMenu(_) => {
                if let Some(id) = chain
                    .iter()
                    .find_map(|id| attr_of(&d, *id, "data-tab-id").and_then(|s| s.parse().ok()))
                {
                    self.actions.push(Action::TabOp("menu-open".into(), id));
                    state.prevent_default();
                }
            }
            // Enter and Escape in the address and find fields moved to
            // `chrome_key`, which runs before the event driver: on macOS,
            // editing keys in a text field are routed through standard key
            // bindings and never reach this handler.
            // DomEventData::KeyDown(k) => {
            //     if k.key == Key::Enter && Some(event.target) == self.url_node {
            //         let v = d
            //             .get_node(event.target)
            //             .and_then(|n| n.element_data())
            //             .and_then(|e| e.text_input_data())
            //             .map(|t| t.editor.raw_text().to_string());
            //         self.actions.push(Action::Go(v));
            //         state.prevent_default();
            //     } else if k.key == Key::Enter && Some(event.target) == self.find_node {
            //         self.actions.push(Action::FindNext(
            //             if k.modifiers.contains(Modifiers::SHIFT) { -1 } else { 1 },
            //         ));
            //         state.prevent_default();
            //     } else if k.key == Key::Escape && Some(event.target) == self.find_node {
            //         self.actions.push(Action::FindHide);
            //         state.prevent_default();
            //     }
            // }
            _ => {}
        }
    }
}

/// The platform whose key bindings apply. The window uses the one it runs on;
/// tests choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyPlatform {
    MacOs,
    Other,
}

impl KeyPlatform {
    const CURRENT: KeyPlatform = match cfg!(target_os = "macos") {
        true => KeyPlatform::MacOs,
        false => KeyPlatform::Other,
    };

    /// Whether `modifiers` hold the platform's command key: Cmd on macOS and
    /// Ctrl elsewhere. Blitz reports winit's meta key, which is Cmd on macOS,
    /// as `SUPER` (blitz-shell `winit_modifiers_to_kbt_modifiers`), never as
    /// `META`, so a binding that only tested `META` never fired (ledger L12).
    fn command(self, modifiers: Modifiers) -> bool {
        match self {
            KeyPlatform::MacOs => modifiers.intersects(Modifiers::SUPER | Modifiers::META),
            KeyPlatform::Other => modifiers.intersects(Modifiers::CONTROL | Modifiers::META),
        }
    }
}

fn global_shortcut(
    key: &Key,
    modifiers: Modifiers,
    tabs: &[u64],
    active: usize,
    platform: KeyPlatform,
) -> Option<Action> {
    // Ctrl+Tab cycles tabs on every platform: on macOS, Cmd+Tab is the
    // system's application switcher, so browsers bind Ctrl+Tab there too.
    let cycles = modifiers.contains(Modifiers::CONTROL) || platform.command(modifiers);
    if *key == Key::Tab && cycles && !tabs.is_empty() {
        let next = if modifiers.contains(Modifiers::SHIFT) {
            (active + tabs.len() - 1) % tabs.len()
        } else {
            (active + 1) % tabs.len()
        };
        return Some(Action::Select(tabs[next]));
    }
    if !platform.command(modifiers) {
        return None;
    }
    let Key::Character(c) = key else { return None };
    if let Some(n) = c
        .chars()
        .next()
        .filter(|_| c.chars().count() == 1)
        .and_then(|c| c.to_digit(10))
        && n > 0
        && !tabs.is_empty()
    {
        let index = if n == 9 {
            tabs.len() - 1
        } else {
            (n as usize - 1).min(tabs.len() - 1)
        };
        return Some(Action::Select(tabs[index]));
    }
    match c.to_ascii_lowercase().as_str() {
        "t" if modifiers.contains(Modifiers::SHIFT) => Some(Action::TabOp("restore".into(), 0)),
        "t" => Some(Action::NewTab),
        "r" => Some(Action::Reload),
        "l" => Some(Action::FocusUrl),
        "w" => Some(Action::Close(u64::MAX)),
        "f" => Some(Action::Find(String::new())),
        // L16: on macOS, winit's default menu takes Cmd+H for Hide before any
        // view sees the key (winit-appkit `menu.rs:38-45`), so History is
        // Cmd+Y there, as in Safari and Chrome.
        // "h" => Some(Action::Panel("history".into())),
        "h" if platform == KeyPlatform::Other => Some(Action::Panel("history".into())),
        "y" if platform == KeyPlatform::MacOs => Some(Action::Panel("history".into())),
        "b" => Some(Action::SidebarToggle),
        _ => None,
    }
}

/// The full-screen chord: F11 alone, or on macOS Ctrl+Cmd+F (Blitz reports
/// Cmd as `SUPER`, ledger L12; `META` is taken too), as other browsers bind
/// it on each.
fn full_screen_chord(key: &Key, modifiers: Modifiers, platform: KeyPlatform) -> bool {
    let others = Modifiers::ALT | Modifiers::SHIFT;
    match platform {
        KeyPlatform::Other => {
            *key == Key::F11 && !modifiers.intersects(Modifiers::CONTROL | Modifiers::SUPER | Modifiers::META | others)
        }
        KeyPlatform::MacOs => {
            matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("f"))
                && modifiers.contains(Modifiers::CONTROL)
                && modifiers.intersects(Modifiers::SUPER | Modifiers::META)
                && !modifiers.intersects(others)
        }
    }
}

/// What a key does before Blitz routes it: the full-screen chord first (on
/// macOS Cmd+F with Ctrl held is not Find), then the browser's shortcuts. A
/// chord held down repeats nothing: `Some(None)` consumes the key. `None`:
/// not the browser's key.
fn browser_shortcut(
    key: &Key,
    modifiers: Modifiers,
    repeating: bool,
    tabs: &[u64],
    active: usize,
    platform: KeyPlatform,
) -> Option<Option<Action>> {
    if full_screen_chord(key, modifiers, platform) {
        return Some((!repeating).then_some(Action::FullScreen));
    }
    global_shortcut(key, modifiers, tabs, active, platform).map(Some)
}

/// Which chrome field has keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyFocus {
    Address,
    Find,
    SideSearch,
    Elsewhere,
}

/// Keys the chrome's own fields handle, checked before the event driver.
/// Returns the action and whether the key is consumed (a consumed key does
/// not reach the field or the page).
fn chrome_key(
    key: &Key,
    modifiers: Modifiers,
    focus: KeyFocus,
    suggestions_open: bool,
) -> Option<(Action, bool)> {
    let shift = modifiers.contains(Modifiers::SHIFT);
    match (key, focus) {
        (Key::Enter, KeyFocus::Address) => Some((Action::AddressSubmit, true)),
        (Key::ArrowDown, KeyFocus::Address) if suggestions_open => {
            Some((Action::SuggestMove(1), true))
        }
        (Key::ArrowUp, KeyFocus::Address) if suggestions_open => {
            Some((Action::SuggestMove(-1), true))
        }
        (Key::Escape, KeyFocus::Address) => Some((Action::AddressDismiss, true)),
        (Key::Enter, KeyFocus::Find) => {
            Some((Action::FindNext(if shift { -1 } else { 1 }), true))
        }
        (Key::Escape, KeyFocus::Find) => Some((Action::FindHide, true)),
        (Key::Escape, KeyFocus::SideSearch) => Some((Action::SideClear, true)),
        // Pages may handle Escape too, so it is not consumed.
        (Key::Escape, KeyFocus::Elsewhere) => Some((Action::Dismiss, false)),
        _ => None,
    }
}

/// The next highlighted suggestion for an arrow key: down from the typed
/// text goes to the first, up to the last, and moving past either end
/// returns to the typed text (`None`).
fn next_suggestion(current: Option<usize>, delta: i32, len: usize) -> Option<usize> {
    match (current, delta.signum(), len) {
        (_, _, 0) => None,
        (None, 1, _) => Some(0),
        (None, -1, n) => Some(n - 1),
        (Some(i), 1, n) if i + 1 < n => Some(i + 1),
        (Some(_), 1, _) => None,
        (Some(0), -1, _) => None,
        (Some(i), -1, _) => Some(i - 1),
        (current, _, _) => current,
    }
}

// ── Requests to the window ──────────────────────────────────────────────

/// What the chrome asks of the window that shows it. Only
/// `ChromeApplication` holds the winit window, so it carries these out
/// (`take_window_requests`): after Blitz makes the window, after each of the
/// window's events, and once per turn of the event loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowRequest {
    /// Show `effective`: to the pages, through Blitz's theme override, which
    /// keeps the window's own theme from replacing the chrome's scheme
    /// (ledger L13); and in the window's decorations
    /// (`application::decoration_theme`). `follows_system`: the scheme is
    /// the system's.
    Scheme { effective: Scheme, follows_system: bool },
    /// The theme is System again: read the system's scheme now. macOS
    /// reports no change while the window has a theme of its own.
    FollowSystem,
    /// Full screen on the monitor the window is on, or back (F11; Ctrl+Cmd+F
    /// on macOS). Each press is one request.
    ToggleFullScreen,
}

// ── Plain data for the builders ─────────────────────────────────────────

/// A transient status-bar message.
#[derive(Clone, Debug)]
struct Flash {
    tone: Tone,
    text: String,
    until: Instant,
}

/// One sidebar panel's content and the count shown beside its title.
struct PanelView {
    html: String,
    count: Option<String>,
}

/// What a tab needs from the user, as shown by its icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabAttention {
    Idle,
    Loading,
    NeedsAnswer,
    Failed,
}

impl TabAttention {
    fn of(stage: &Stage, has_prompts: bool) -> TabAttention {
        match (stage, has_prompts) {
            (Stage::Failed(_), _) => TabAttention::Failed,
            (Stage::Grants, _) | (_, true) => TabAttention::NeedsAnswer,
            (Stage::Fetching | Stage::Scripts, false) => TabAttention::Loading,
            (Stage::Running | Stage::Static, false) => TabAttention::Idle,
        }
    }

    /// The icon and its tone class, or `None` for an idle tab (which shows
    /// its address's scheme instead).
    fn icon(self) -> Option<(&'static str, &'static str)> {
        match self {
            TabAttention::Idle => None,
            TabAttention::Loading => Some(("fa-hourglass-half", "")),
            TabAttention::NeedsAnswer => Some(("fa-shield-halved", "warn")),
            TabAttention::Failed => Some(("fa-triangle-exclamation", "err")),
        }
    }
}

/// The icon (and tone class) of a tab or row for `url` in `attention`.
fn tab_icon(url: &str, attention: TabAttention) -> (&'static str, &'static str) {
    attention
        .icon()
        .unwrap_or_else(|| (SchemeKind::of(url).icon(), ""))
}

struct TabRow<'a> {
    id: u64,
    title: &'a str,
    url: &'a str,
    depth: usize,
    active: bool,
    attention: TabAttention,
    page_matches: usize,
    menu_open: bool,
    has_children: bool,
    first: bool,
    last: bool,
}

struct ClosedRow<'a> {
    index: usize,
    title: &'a str,
    url: &'a str,
}

struct TabChip<'a> {
    id: u64,
    title: &'a str,
    url: &'a str,
    attention: TabAttention,
    active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Suggestion {
    title: String,
    url: String,
    /// An open tab with this address: choosing it switches to that tab.
    open_tab: Option<u64>,
}

struct SessionRow {
    tab: u64,
    label: String,
    uri: String,
}

struct SiteUsage {
    site: String,
    has_store: bool,
    stored: u64,
    remembered: usize,
    sessions: Vec<SessionRow>,
}

struct RememberedChoice {
    urn: String,
    allow: bool,
    classes: Vec<&'static str>,
}

struct PermissionsView {
    site: Option<String>,
    stage: Stage,
    grants: Vec<gaze_exec::Grant>,
    remembered: Vec<RememberedChoice>,
}

// Disabled in step 10 (ledger S12, part 3): the Appearance panel's card
// showed the status of one file, `themes/custom.css`; each theme file's row
// shows its own now.
// #[derive(Clone, Debug, PartialEq, Eq)]
// enum PaletteStatus {
//     Valid,
//     Missing,
//     Invalid(String),
// }

#[derive(Clone, Debug, PartialEq, Eq)]
struct WalletRow {
    address: String,
    label: String,
    balance: Option<String>,
}

/// A wallet action waiting for its confirming click.
#[derive(Clone, Debug, PartialEq, Eq)]
enum WalletConfirm {
    Send {
        from: String,
        to: String,
        amount: i64,
        note: Option<String>,
    },
    Remove {
        address: String,
        label: String,
    },
}

/// What the wallet panel shows; filled by background work.
#[derive(Default)]
struct WalletView {
    balances: BTreeMap<String, String>,
    notice: Option<(Tone, String)>,
    confirm: Option<WalletConfirm>,
}

// ── Builders: markup from plain data ────────────────────────────────────

fn plural(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

fn side_head_html(title: &str, count: Option<&str>) -> String {
    let mut html = String::with_capacity(320);
    put!(html, r#"<span class="side-title">{}</span>"#, escape(title));
    if let Some(count) = count {
        put!(html, r#"<span class="count">{}</span>"#, escape(count));
    }
    put!(
        html,
        r#"<span class="grow"></span><button class="icon-btn" data-action="sidebar:toggle" aria-label="Hide sidebar"><i class="fa-solid fa-angles-left"></i><span class="tip tip-below-end">Hide sidebar · {}</span></button>"#,
        key_label("{key:sidebar}", KeyPlatform::CURRENT)
    );
    html
}

fn empty_state_html(icon: &str, text: &str, action: Option<(&str, &str)>) -> String {
    let mut html = String::with_capacity(256);
    put!(
        html,
        r#"<div class="empty-state"><i class="fa-solid {icon}"></i><span>{}</span>"#,
        escape(text)
    );
    if let Some((data_action, label)) = action {
        put!(
            html,
            r#"<button class="btn btn-sm" data-action="{data_action}"><span>{}</span></button>"#,
            escape(label)
        );
    }
    html.push_str("</div>");
    html
}

fn notice_html(tone: Tone, text: &str, dismiss: Option<&str>) -> String {
    let mut html = String::with_capacity(160 + text.len());
    put!(
        html,
        r#"<div class="notice {}"><i class="fa-solid {}"></i><div class="notice-text">{}</div>"#,
        tone.class(),
        tone.icon(),
        escape(text)
    );
    if let Some(action) = dismiss {
        put!(
            html,
            r#"<button class="icon-btn icon-btn-sm" data-action="{action}" aria-label="Dismiss"><i class="fa-solid fa-xmark"></i></button>"#
        );
    }
    html.push_str("</div>");
    html
}

/// The Tabs panel: one row per matching tab (with the inline menu of the
/// row whose menu is open), then recently closed tabs.
/// The address shown under a fitted `label` while searching for `query`.
/// When the label already shows why the row matched, the address keeps its
/// usual form (origin and path tail), its own match highlighted only if
/// that form shows it; otherwise the address moves to its match.
fn fit_detail(
    fit: &mut TextFitter,
    font: Font,
    url: &str,
    query: &str,
    label: &Marked,
    max_px: f32,
) -> Marked {
    let mark = match_span(query, url);
    match label.mark {
        Some(_) => fit.url_marked_in_place(font, url, mark, max_px),
        None => fit.url_marked(font, url, mark, max_px),
    }
}

/// A fitted label as HTML, with the search match it shows (if any) in a
/// `<mark>`.
fn marked_html(marked: &Marked) -> String {
    match &marked.mark {
        Some(mark) => {
            let text = &marked.text;
            let mut html = String::with_capacity(text.len() + 24);
            html.push_str(&escape(&text[..mark.start]));
            html.push_str("<mark>");
            html.push_str(&escape(&text[mark.clone()]));
            html.push_str("</mark>");
            html.push_str(&escape(&text[mark.end..]));
            html
        }
        None => escape(&marked.text),
    }
}

fn tab_panel_html(
    fit: &mut TextFitter,
    rows: &[TabRow<'_>],
    closed: &[ClosedRow<'_>],
    query: &str,
    total: usize,
) -> PanelView {
    use geometry::*;
    let mut html = String::with_capacity(560 * rows.len() + 400 * closed.len() + 256);
    for row in rows {
        let depth = row.depth as f32;
        let reserve = match row.active || row.menu_open {
            true => ROW_ACTIONS,
            false => 0.0,
        };
        let width = row_text() - depth * TREE_INDENT - reserve - ROUNDING;
        let label = fit.end_marked(LABEL, row.title, match_span(query, row.title), width);
        let (icon, tone) = tab_icon(row.url, row.attention);
        let classes = match (row.active, row.menu_open) {
            (true, true) => "row on menu-open",
            (true, false) => "row on",
            (false, true) => "row menu-open",
            (false, false) => "row",
        };
        put!(
            html,
            r#"<div class="{classes}" data-tab-id="{id}" data-action="select:{id}" style="padding-left:{pad}px"><i class="fa-solid {icon} row-icon {tone}"></i><div class="row-text"><span class="row-label">{label}</span>"#,
            id = row.id,
            pad = ROW_PAD_X + depth * TREE_INDENT,
            label = marked_html(&label),
        );
        match row.page_matches {
            0 => put!(
                html,
                r#"<span class="row-detail">{}</span>"#,
                marked_html(&fit_detail(fit, DETAIL, row.url, query, &label, width))
            ),
            n => put!(
                html,
                r#"<span class="row-detail hit">{} in page text</span>"#,
                plural(n, "match", "matches")
            ),
        }
        put!(
            html,
            r#"</div><div class="row-actions"><button class="icon-btn icon-btn-sm" data-action="tab:menu:{id}" aria-label="Tab actions"><i class="fa-solid fa-ellipsis"></i><span class="tip tip-below-end">More actions</span></button><button class="icon-btn icon-btn-sm" data-action="close:{id}" aria-label="Close tab"><i class="fa-solid fa-xmark"></i><span class="tip tip-below-end">Close tab</span></button></div></div>"#,
            id = row.id
        );
        if row.menu_open {
            tab_menu_html(&mut html, row, total);
        }
    }
    if rows.is_empty() {
        let text = format!("No tabs match “{}”", query.trim());
        html.push_str(&empty_state_html(
            "fa-magnifying-glass",
            &text,
            Some(("side:clear", "Clear search")),
        ));
    }
    if !closed.is_empty() {
        put!(
            html,
            r#"<div class="section-head"><span>Recently closed</span><span class="count">{}</span><span class="grow"></span><span class="tag">{}</span></div>"#,
            closed.len(),
            key_label("{key:reopen}", KeyPlatform::CURRENT)
        );
        let width = row_text() - ROUNDING;
        for row in closed {
            let title = if row.title.is_empty() { row.url } else { row.title };
            let label = fit.end_marked(LABEL, title, match_span(query, title), width);
            let url = fit_detail(fit, DETAIL, row.url, query, &label, width);
            put!(
                html,
                r#"<div class="row" data-action="tab:restore-at:{index}"><i class="fa-solid fa-rotate-left row-icon"></i><div class="row-text"><span class="row-label">{label}</span><span class="row-detail">{url}</span></div></div>"#,
                index = row.index,
                label = marked_html(&label),
                url = marked_html(&url),
            );
        }
    }
    let count = match query.trim().is_empty() {
        true => total.to_string(),
        false => format!("{} of {total}", rows.len()),
    };
    PanelView {
        html,
        count: Some(count),
    }
}

/// The inline menu under a tab row. Items that do not apply are disabled;
/// the menu closes after any of them.
fn tab_menu_html(html: &mut String, row: &TabRow<'_>, total: usize) {
    let disabled = |off: bool| if off { " disabled" } else { "" };
    put!(
        html,
        r#"<div class="row-menu" style="margin-left:{left}px"><button class="menu-item" data-action="tab:duplicate:{id}"><i class="fa-solid fa-clone"></i><span>Duplicate</span></button><button class="menu-item" data-action="tab:copy:{id}"><i class="fa-solid fa-copy"></i><span>Copy address</span></button><div class="menu-sep"></div><button class="menu-item" data-action="tab:left:{id}"{up}><i class="fa-solid fa-arrow-up"></i><span>Move up</span></button><button class="menu-item" data-action="tab:right:{id}"{down}><i class="fa-solid fa-arrow-down"></i><span>Move down</span></button><div class="menu-sep"></div><button class="menu-item" data-action="tab:others:{id}"{others}><i class="fa-solid fa-broom"></i><span>Close other tabs</span></button><button class="menu-item" data-action="tab:close-right:{id}"{below}><i class="fa-solid fa-angles-down"></i><span>Close tabs below</span></button>"#,
        left = 28.0 + row.depth as f32 * geometry::TREE_INDENT,
        id = row.id,
        up = disabled(row.first),
        down = disabled(row.last),
        others = disabled(total < 2),
        below = disabled(row.last),
    );
    if row.has_children {
        put!(
            html,
            r#"<button class="menu-item" data-action="tab:branch:{}"><i class="fa-solid fa-sitemap"></i><span>Close tab and its children</span></button>"#,
            row.id
        );
    }
    put!(
        html,
        r#"<button class="menu-item danger" data-action="close:{}"><i class="fa-solid fa-xmark"></i><span>Close tab</span></button></div>"#,
        row.id
    );
}

/// Whether a visit matches a history search: by substring anywhere, or by a
/// small typo among the most recent visits.
fn visit_matches(query: &str, index: usize, visit: &Visit) -> bool {
    query.is_empty()
        || visit.title.to_lowercase().contains(query)
        || visit.url.to_lowercase().contains(query)
        || (index < HISTORY_FUZZY_WINDOW && search_score(query, &visit.title, &visit.url).is_some())
}

/// The History panel: matching visits, newest first, grouped by how long ago
/// they were, each address once per group.
fn history_panel_html(
    fit: &mut TextFitter,
    visits: &[Visit],
    query: &str,
    now: u64,
    limit: usize,
    confirm: bool,
) -> PanelView {
    use geometry::*;
    let query = query.trim().to_lowercase();
    // Keep the newest visit of each address within each recency group.
    let mut seen: HashSet<(Recency, &str)> = HashSet::with_capacity(visits.len().min(4_096));
    let kept: Vec<(Recency, &Visit)> = visits
        .iter()
        .enumerate()
        .filter(|(index, visit)| visit_matches(&query, *index, visit))
        .map(|(_, visit)| (Recency::of(now, visit.at), visit))
        .filter(|(recency, visit)| seen.insert((*recency, visit.url.as_str())))
        .collect();
    // Entries are what the list shows: one per address per group. The
    // header and the clear prompt count them, not the stored visits.
    let total = match query.is_empty() {
        true => kept.len(),
        false => {
            let mut all: HashSet<(Recency, &str)> = HashSet::with_capacity(visits.len().min(4_096));
            visits
                .iter()
                .filter(|visit| all.insert((Recency::of(now, visit.at), visit.url.as_str())))
                .count()
        }
    };
    let mut html = String::with_capacity(520 * kept.len().min(limit) + 512);
    if confirm {
        put!(
            html,
            r#"<div class="notice warn"><i class="fa-solid fa-triangle-exclamation"></i><div class="notice-text"><div>Clear all {} from history? This can't be undone.</div><div class="card-actions"><button class="btn btn-sm" data-action="history:clear-cancel:0"><span>Cancel</span></button><button class="btn btn-sm btn-primary btn-danger" data-action="history:clear:0"><i class="fa-solid fa-trash-can"></i><span>Clear history</span></button></div></div></div>"#,
            plural(total, "entry", "entries")
        );
    }
    let mut current: Option<Recency> = None;
    for (recency, visit) in kept.iter().take(limit) {
        if current != Some(*recency) {
            current = Some(*recency);
            let in_group = kept.iter().filter(|(r, _)| r == recency).count();
            put!(
                html,
                r#"<div class="section-head"><span>{}</span><span class="count">{in_group}</span></div>"#,
                recency.label()
            );
        }
        let when = display::relative_time(now, visit.at);
        let width = row_text() - fit.width(META, &when) - ROW_GAP - ROUNDING;
        let title = if visit.title.is_empty() {
            visit.url.as_str()
        } else {
            visit.title.as_str()
        };
        let url_attr = escape(&visit.url);
        let label = fit.end_marked(LABEL, title, match_span(&query, title), width);
        let detail = fit_detail(fit, DETAIL, &visit.url, &query, &label, width);
        put!(
            html,
            r#"<div class="row" data-action="visit:open:{url_attr}"><i class="fa-solid {icon} row-icon"></i><div class="row-text"><span class="row-label">{label}</span><span class="row-detail">{detail}</span></div><span class="row-meta">{when}</span><div class="row-actions"><button class="icon-btn icon-btn-sm" data-action="visit:forget:{url_attr}" aria-label="Remove from history"><i class="fa-solid fa-xmark"></i><span class="tip tip-below-end">Remove from history</span></button></div></div>"#,
            icon = SchemeKind::of(&visit.url).icon(),
            label = marked_html(&label),
            detail = marked_html(&detail),
            when = escape(&when),
        );
    }
    if kept.len() > limit {
        put!(
            html,
            r#"<div class="form-actions start"><button class="btn btn-sm btn-ghost" data-action="history:more:0"><span>Show {} more</span></button></div>"#,
            (kept.len() - limit).min(HISTORY_PAGE)
        );
    }
    match (kept.is_empty(), query.is_empty()) {
        (true, true) => html.push_str(&empty_state_html(
            "fa-clock-rotate-left",
            "No history yet. Pages you visit appear here.",
            None,
        )),
        (true, false) => html.push_str(&empty_state_html(
            "fa-magnifying-glass",
            &format!("No visits match “{query}”"),
            Some(("side:clear", "Clear search")),
        )),
        (false, _) => {}
    }
    let count = match query.is_empty() {
        true => total.to_string(),
        false => format!("{} of {total}", kept.len()),
    };
    PanelView {
        html,
        count: Some(count),
    }
}

/// The Site data panel: sites that keep something, then the content cache.
fn site_data_html(
    fit: &mut TextFitter,
    sites: &[SiteUsage],
    cache: &gaze_blob::CacheStats,
) -> PanelView {
    use geometry::*;
    let mut html = String::with_capacity(900 * sites.len() + 900);
    let head_width = card_inner() - ROW_ICON - ROW_GAP - ROUNDING;
    if sites.is_empty() {
        html.push_str(&empty_state_html(
            "fa-database",
            "No site keeps any data yet.",
            None,
        ));
    } else {
        put!(
            html,
            r#"<div class="section-head"><span>Sites with data</span><span class="count">{}</span></div>"#,
            sites.len()
        );
    }
    let end_width = fit.width(META, "End") + 2.0 * 7.0 + 2.0;
    let session_width =
        card_inner() - 2.0 * ROW_PAD_X - ROW_ICON - 2.0 * ROW_GAP - end_width - ROUNDING;
    for usage in sites {
        let site_attr = escape(&usage.site);
        put!(
            html,
            r#"<div class="card"><div class="card-head"><i class="fa-solid {icon} row-icon"></i><div class="row-text"><span class="row-label">{label}</span><span class="row-detail">{detail}</span></div></div><div class="facts">"#,
            icon = SchemeKind::of(&usage.site).icon(),
            label = escape(&fit.end(LABEL, &display::site_label(&usage.site), head_width)),
            detail = escape(&fit.url(DETAIL, &usage.site, head_width)),
        );
        if usage.stored > 0 {
            put!(
                html,
                "<span>{} stored</span>",
                display::human_bytes(usage.stored)
            );
        }
        if usage.remembered > 0 {
            put!(
                html,
                "<span>{}</span>",
                plural(usage.remembered, "remembered permission", "remembered permissions")
            );
        }
        if !usage.sessions.is_empty() {
            put!(
                html,
                "<span>{}</span>",
                plural(usage.sessions.len(), "live session", "live sessions")
            );
        }
        html.push_str("</div>");
        for session in &usage.sessions {
            put!(
                html,
                r#"<div class="row static"><i class="fa-solid fa-link row-icon"></i><div class="row-text"><span class="row-label mono">{label}</span><span class="row-detail">{uri}</span></div><button class="btn btn-xs btn-ghost" data-action="site:session:{tab}:{label_attr}"><span>End</span></button></div>"#,
                label = escape(&fit.end(MONO, &session.label, session_width)),
                uri = escape(&fit.end(DETAIL, &session.uri, session_width)),
                tab = session.tab,
                label_attr = escape(&session.label),
            );
        }
        if usage.has_store || usage.remembered > 0 {
            html.push_str(r#"<div class="card-actions">"#);
            if usage.has_store {
                put!(
                    html,
                    r#"<button class="btn btn-sm btn-ghost btn-danger" data-action="site:store:{site_attr}"><i class="fa-solid fa-trash-can"></i><span>Clear stored data</span></button>"#
                );
            }
            if usage.remembered > 0 {
                put!(
                    html,
                    r#"<button class="btn btn-sm btn-ghost" data-action="site:grants:{site_attr}"><i class="fa-solid fa-shield-halved"></i><span>Forget permissions</span></button>"#
                );
            }
            html.push_str("</div>");
        }
        html.push_str("</div>");
    }
    html.push_str(r#"<div class="section-head"><span>Content cache</span></div><div class="card"><div class="facts">"#);
    match (cache.entries, cache.partial_entries) {
        (0, 0) => html.push_str("<span>Empty</span>"),
        (entries, partial) => {
            put!(
                html,
                "<span>{} · {}</span>",
                plural(entries, "verified blob", "verified blobs"),
                display::human_bytes(cache.bytes)
            );
            if partial > 0 {
                put!(
                    html,
                    "<span>{} · {}</span>",
                    plural(partial, "partial download", "partial downloads"),
                    display::human_bytes(cache.partial_bytes)
                );
            }
        }
    }
    let empty = cache.entries == 0 && cache.partial_entries == 0;
    put!(
        html,
        r#"</div><div class="card-actions"><button class="btn btn-sm btn-ghost" data-action="site:cache:global"{}><i class="fa-solid fa-box-archive"></i><span>Clear cache</span></button></div></div><span class="footnote">Pages keep data only in their capability store. There are no cookies or localStorage, and JavaScript never runs.</span>"#,
        if empty { " disabled" } else { "" }
    );
    PanelView {
        html,
        count: Some(sites.len().to_string()),
    }
}

/// The Permissions panel for the active page.
fn permissions_html(fit: &mut TextFitter, view: &PermissionsView) -> PanelView {
    use geometry::*;
    let mut html = String::with_capacity(640 * (view.grants.len() + view.remembered.len()) + 1_024);
    let Some(site) = view.site.as_deref() else {
        html.push_str(&empty_state_html(
            "fa-shield-halved",
            "This page has no site, so it holds no capabilities.",
            None,
        ));
        return PanelView { html, count: None };
    };
    let head_width = card_inner() - ROW_ICON - ROW_GAP - ROUNDING;
    put!(
        html,
        r#"<div class="card"><div class="card-head"><i class="fa-solid {icon} row-icon"></i><div class="row-text"><span class="row-label">{label}</span><span class="row-detail">{detail}</span></div></div></div>"#,
        icon = SchemeKind::of(site).icon(),
        label = escape(&fit.end(LABEL, &display::site_label(site), head_width)),
        detail = escape(&fit.url(DETAIL, site, head_width)),
    );
    let granted = view.grants.iter().filter(|g| g.granted).count();
    match (&view.stage, view.grants.is_empty()) {
        (Stage::Grants, _) => html.push_str(&notice_html(
            Tone::Warn,
            "This page is waiting for your answer in the prompt bar before it runs.",
            None,
        )),
        (Stage::Failed(_), true) => html.push_str(&empty_state_html(
            "fa-triangle-exclamation",
            "This page did not run, so it holds no capabilities.",
            None,
        )),
        (Stage::Fetching | Stage::Scripts, true) => html.push_str(&empty_state_html(
            "fa-hourglass-half",
            "The page is still loading.",
            None,
        )),
        (_, true) => html.push_str(&empty_state_html(
            "fa-file-lines",
            "This page runs no f1r3lang, so it holds no capabilities.",
            None,
        )),
        (_, false) => {}
    }
    if !view.grants.is_empty() {
        put!(
            html,
            r#"<div class="section-head"><span>This page can use</span><span class="count">{granted}</span></div>"#
        );
    }
    for grant in &view.grants {
        let info = display::capability_info(&grant.urn);
        put!(
            html,
            r#"<div class="row static"><i class="fa-solid {icon} row-icon"></i><div class="row-text"><span class="row-label">{name}</span><span class="row-desc">{desc}</span></div><div class="row-side">"#,
            icon = info.icon,
            name = escape(info.name),
            desc = escape(info.description),
        );
        match grant.granted {
            true => put!(
                html,
                r#"<span class="tag ok">Allowed</span><button class="btn btn-xs btn-ghost btn-danger" data-action="revoke:{}"><span>Revoke</span></button>"#,
                escape(&grant.urn)
            ),
            false => html.push_str(r#"<span class="tag">Off</span>"#),
        }
        html.push_str("</div></div>");
    }
    if !view.remembered.is_empty() {
        put!(
            html,
            r#"<div class="section-head"><span>Remembered for this site</span><span class="count">{}</span></div>"#,
            view.remembered.len()
        );
        for choice in &view.remembered {
            let info = display::capability_info(&choice.urn);
            let mut detail = String::from(if choice.allow {
                "Always allow"
            } else {
                "Always deny"
            });
            if !choice.classes.is_empty() {
                put!(detail, " · {}", choice.classes.join(", "));
            }
            put!(
                html,
                r#"<div class="row static"><i class="fa-solid {icon} row-icon"></i><div class="row-text"><span class="row-label">{name}</span><span class="row-desc">{detail}</span></div></div>"#,
                icon = info.icon,
                name = escape(info.name),
                detail = escape(&detail),
            );
        }
        put!(
            html,
            r#"<div class="card-actions"><button class="btn btn-sm btn-ghost" data-action="site:grants:{}"><i class="fa-solid fa-rotate-left"></i><span>Forget remembered choices</span></button></div>"#,
            escape(site)
        );
    }
    PanelView {
        html,
        count: (!view.grants.is_empty()).then(|| granted.to_string()),
    }
}

/// The Console panel: the active tab's messages, newest first.
fn console_html(lines: &[(String, String)]) -> PanelView {
    let shown = lines.len().min(CONSOLE_SHOWN);
    let mut html = String::with_capacity(160 * shown + 256);
    if lines.is_empty() {
        html.push_str(&empty_state_html(
            "fa-terminal",
            "No console messages from this page. Pages write here with the log capability.",
            None,
        ));
        return PanelView {
            html,
            count: Some("0".into()),
        };
    }
    put!(
        html,
        r#"<div class="section-head"><span>Messages</span><span class="count">{}</span><span class="grow"></span><span class="tag">Newest first</span></div>"#,
        lines.len()
    );
    for (level, text) in lines.iter().rev().take(CONSOLE_SHOWN) {
        let level = LogLevel::of(level);
        put!(
            html,
            r#"<div class="log-line {}"><span class="log-level">{}</span><span class="log-text">{}</span></div>"#,
            level.class(),
            level.label(),
            escape(&display::console_text(text))
        );
    }
    PanelView {
        html,
        count: Some(lines.len().to_string()),
    }
}

// Disabled in step 10 (ledger S12, part 3): Dark, Light and Custom, and
// the card of the one custom file, `themes/custom.css`. Replaced by System,
// Dark and Light, the Themes list and the Theme files card below.
// /// The Appearance panel: the scheme, its colours, and the custom palette.
// fn appearance_html(
//     fit: &mut TextFitter,
//     scheme: &str,
//     colors: &BTreeMap<String, String>,
//     palette_path: &str,
//     status: &PaletteStatus,
// ) -> PanelView {
//     let mut html = String::with_capacity(4_096);
//     let segment = |value: &str, icon: &str, label: &str, disabled: bool| {
//         format!(
//             r#"<button class="segment{on}" data-action="theme:{value}"{off}><i class="fa-solid {icon}"></i><span>{label}</span></button>"#,
//             on = if scheme == value { " on" } else { "" },
//             off = if disabled { " disabled" } else { "" },
//         )
//     };
//     put!(
//         html,
//         r#"<div class="section-head"><span>Color scheme</span></div><div class="segmented">{}{}{}</div><span class="footnote">Colors the browser controls and built-in pages. Sites keep their own styles.</span>"#,
//         segment("dark", "fa-moon", "Dark", false),
//         segment("light", "fa-sun", "Light", false),
//         segment("custom", "fa-swatchbook", "Custom", *status != PaletteStatus::Valid),
//     );
//     html.push_str(r#"<div class="section-head"><span>Current palette</span></div><div class="swatches">"#);
//     for name in theme::token_names() {
//         let color = colors.get(*name).map(String::as_str).unwrap_or("transparent");
//         put!(
//             html,
//             r#"<div class="swatch"><span class="swatch-color" style="background:{}"></span><span class="swatch-name">{}</span></div>"#,
//             escape(color),
//             escape(name.trim_start_matches("--gaze-"))
//         );
//     }
//     let path_width = geometry::card_inner() - geometry::ROUNDING;
//     put!(
//         html,
//         r#"</div><div class="section-head"><span>Custom palette file</span></div><div class="card"><div class="mono wallet-address">{}</div>"#,
//         escape(&fit.middle(MONO, palette_path, path_width))
//     );
//     let (notice, button) = match status {
//         PaletteStatus::Valid => (
//             notice_html(
//                 Tone::Ok,
//                 "The palette is valid. Edit the file, then reload it to apply the changes.",
//                 None,
//             ),
//             r#"<button class="btn btn-sm" data-action="theme:custom"><i class="fa-solid fa-rotate-right"></i><span>Reload palette</span></button>"#,
//         ),
//         PaletteStatus::Missing => (
//             notice_html(
//                 Tone::Info,
//                 "Create a palette file to choose your own colors. It lists every --gaze-* color as #RRGGBB.",
//                 None,
//             ),
//             r#"<button class="btn btn-sm" data-action="theme:template"><i class="fa-solid fa-plus"></i><span>Create palette file</span></button>"#,
//         ),
//         PaletteStatus::Invalid(why) => (
//             notice_html(Tone::Err, &format!("The palette cannot be used: {why}"), None),
//             r#"<button class="btn btn-sm" data-action="theme:custom"><i class="fa-solid fa-rotate-right"></i><span>Reload palette</span></button>"#,
//         ),
//     };
//     put!(
//         html,
//         r#"{notice}<div class="card-actions">{button}</div></div>"#
//     );
//     PanelView { html, count: None }
// }

/// What the Appearance panel shows (`appearance_html`).
struct AppearanceView<'a> {
    /// The theme chosen.
    choice: &'a ThemeChoice,
    /// What is known of the system's preference (the sentence, with System).
    system: &'a Known,
    /// The colours shown (the swatches).
    colours: &'a BTreeMap<String, String>,
    /// The scheme shown.
    shown: Scheme,
    /// Why the chosen theme cannot be used, if it cannot
    /// (`theme::Resolved::problem`).
    problem: Option<&'a str>,
    /// The theme files: the user's, and those installed for everyone.
    themes: &'a [ThemeFile],
    /// The user's themes folder.
    folder: &'a str,
}

/// A scheme's name in a sentence.
fn scheme_word(scheme: Scheme) -> &'static str {
    match scheme {
        Scheme::Dark => "dark",
        Scheme::Light => "light",
    }
}

/// The sentence under the scheme control: with System, what the system
/// prefers; then what the scheme colours.
fn scheme_note(choice: &ThemeChoice, system: &Known) -> String {
    let follows = match (choice, system) {
        (ThemeChoice::System, Known::Answered(Some(Scheme::Dark))) => "Follows your system, which prefers dark. ",
        (ThemeChoice::System, Known::Answered(Some(Scheme::Light))) => "Follows your system, which prefers light. ",
        (ThemeChoice::System, Known::Answered(None)) => "Your system states no preference, so the dark scheme is used. ",
        (ThemeChoice::System, Known::Unknown(_)) => {
            "Your system's preference could not be read, so the dark scheme is used. "
        }
        _ => "",
    };
    format!("{follows}{SCHEME_NOTE}")
}

/// The Appearance panel: a chosen theme that cannot be used, the scheme
/// control and its sentence, the theme files, the colours shown, and the
/// themes folder with New theme and Reload.
fn appearance_html(fit: &mut TextFitter, view: &AppearanceView<'_>) -> PanelView {
    use geometry::*;
    let mut html = String::with_capacity(6_144 + 768 * view.themes.len());
    if let (ThemeChoice::Named(name), Some(problem)) = (view.choice, view.problem) {
        let text = format!(
            "“{name}” cannot be used: {problem}. The {} scheme is shown instead.",
            scheme_word(view.shown)
        );
        html.push_str(&notice_html(Tone::Err, &text, None));
    }
    let lit = match view.choice {
        ThemeChoice::System => "system",
        ThemeChoice::BuiltIn(Scheme::Dark) => "dark",
        ThemeChoice::BuiltIn(Scheme::Light) => "light",
        ThemeChoice::Named(_) => "",
    };
    let segment = |value: &str, icon: &str, label: &str| {
        format!(
            r#"<button class="segment{on}" data-action="theme:{value}" aria-pressed="{pressed}"><i class="fa-solid {icon}"></i><span>{label}</span></button>"#,
            on = if lit == value { " on" } else { "" },
            pressed = lit == value,
        )
    };
    put!(
        html,
        r#"<div class="section-head"><span>Color scheme</span></div><div class="segmented">{}{}{}</div><span class="footnote">{}</span>"#,
        segment("system", "fa-circle-half-stroke", "System"),
        segment("dark", "fa-moon", "Dark"),
        segment("light", "fa-sun", "Light"),
        escape(&scheme_note(view.choice, view.system)),
    );
    put!(
        html,
        r#"<div class="section-head"><span>Themes</span><span class="count">{}</span></div>"#,
        view.themes.len()
    );
    if view.themes.is_empty() {
        html.push_str(&empty_state_html("fa-palette", "No theme files yet.", None));
    }
    for file in view.themes {
        let tag = match &file.colours {
            Ok(declared) => match theme::scheme_of(&theme::palette_with(declared)) {
                Scheme::Dark => "Dark",
                Scheme::Light => "Light",
            },
            Err(_) => "Unusable",
        };
        // The row's text column, less the tag beside it (R5).
        let width = row_text() - (fit.width(TAG, tag) + TAG_PAD) - ROW_GAP - ROUNDING;
        let label = escape(&fit.end(LABEL, &file.name, width));
        match &file.colours {
            Ok(_) => {
                let chosen = matches!(view.choice, ThemeChoice::Named(name) if *name == file.name);
                put!(
                    html,
                    r#"<div class="row{on}" data-action="theme:use:{name}"><i class="fa-solid fa-palette row-icon"></i><div class="row-text"><span class="row-label">{label}</span><span class="row-detail">{path}</span></div><span class="tag">{tag}</span></div>"#,
                    on = if chosen { " on" } else { "" },
                    name = escape(&file.name),
                    path = escape(&fit.middle(DETAIL, &file.path.display().to_string(), width)),
                );
            }
            // Shown with why, and never chosen: it has no action.
            Err(why) => put!(
                html,
                r#"<div class="row static"><i class="fa-solid fa-triangle-exclamation row-icon err"></i><div class="row-text"><span class="row-label">{label}</span><span class="row-desc why">{why}</span></div><span class="tag err">{tag}</span></div>"#,
                why = escape(why),
            ),
        }
    }
    html.push_str(r#"<div class="section-head"><span>Current palette</span></div><div class="swatches">"#);
    for name in theme::token_names() {
        let color = view.colours.get(*name).map(String::as_str).unwrap_or("transparent");
        put!(
            html,
            r#"<div class="swatch"><span class="swatch-color" style="background:{}"></span><span class="swatch-name">{}</span></div>"#,
            escape(color),
            escape(name.trim_start_matches("--gaze-"))
        );
    }
    put!(
        html,
        r#"</div><div class="section-head"><span>Theme files</span></div><div class="card"><div class="mono wallet-address">{}</div>{}<div class="card-actions"><button class="btn btn-sm" data-action="theme:new"><i class="fa-solid fa-plus"></i><span>New theme</span></button><button class="btn btn-sm btn-ghost" data-action="theme:reload"><i class="fa-solid fa-rotate-right"></i><span>Reload</span></button></div></div>"#,
        escape(&fit.middle(MONO, view.folder, card_inner() - ROUNDING)),
        notice_html(
            Tone::Info,
            "A theme is a .css file of --gaze-* colors in this folder. New theme starts one from the current colors; edit it, then reload.",
            None
        ),
    );
    PanelView { html, count: None }
}

/// The paying wallet's card (or what to do without one).
fn wallet_card_html(
    fit: &mut TextFitter,
    active: Option<&WalletRow>,
    wallets: usize,
    embers: bool,
) -> String {
    use geometry::*;
    let mut html = String::with_capacity(1_536);
    match (active, wallets) {
        (_, 0) => html.push_str(&empty_state_html(
            "fa-wallet",
            "No wallets yet. Create one, or import the file F1R3Sky saved.",
            None,
        )),
        (None, _) => html.push_str(&notice_html(
            Tone::Info,
            "No wallet pays for deploys yet. Choose one below with “Use for payments”.",
            None,
        )),
        (Some(wallet), _) => {
            let label = if wallet.label.is_empty() {
                "Unnamed wallet"
            } else {
                wallet.label.as_str()
            };
            let inner = card_inner() - ROUNDING;
            let tag_width = fit.width(TAG, "Pays for deploys") + TAG_PAD;
            let title_width = inner - ROW_ICON - 2.0 * ROW_GAP - tag_width;
            let title = fit.end(LABEL_STRONG, label, title_width).into_owned();
            let address = fit.middle(MONO, &wallet.address, inner).into_owned();
            put!(
                html,
                r#"<div class="card"><div class="card-head"><i class="fa-solid fa-wallet row-icon"></i><span class="card-title">{title}</span><span class="tag ok">Pays for deploys</span></div><div class="mono wallet-address">{address}</div>"#,
                title = escape(&title),
                address = escape(&address),
            );
            if embers {
                let balance = wallet
                    .balance
                    .as_deref()
                    .map(|b| display::group_digits(b).into_owned())
                    .unwrap_or_else(|| "…".into());
                put!(
                    html,
                    r#"<div class="facts"><span>Balance {}</span></div>"#,
                    escape(&balance)
                );
            }
            put!(
                html,
                r#"<div class="card-actions"><button class="btn btn-sm" data-action="wallet:copy:{a}"><i class="fa-solid fa-copy"></i><span>Copy</span><span class="tip tip-below">Copy the address to receive funds</span></button><button class="btn btn-sm" data-action="wallet:export:{a}"><i class="fa-solid fa-file-export"></i><span>Export</span><span class="tip tip-below">Write the wallet file (F1R3Sky can import it)</span></button><button class="btn btn-sm btn-ghost btn-danger" data-action="wallet:remove:{a}"><i class="fa-solid fa-trash-can"></i><span>Remove</span></button></div></div>"#,
                a = escape(&wallet.address)
            );
        }
    }
    if !embers && wallets > 0 {
        html.push_str(&notice_html(
            Tone::Info,
            "Balances and transfers need an Embers service: set embers_api under [wallet] in settings.toml.",
            None,
        ));
    }
    html
}

/// Wallets other than the paying one.
fn wallet_list_html(fit: &mut TextFitter, others: &[WalletRow], embers: bool) -> String {
    use geometry::*;
    if others.is_empty() {
        return String::new();
    }
    let inner = card_inner() - ROUNDING;
    let mut html = String::with_capacity(1_200 * others.len() + 128);
    put!(
        html,
        r#"<div class="section-head"><span>Other wallets</span><span class="count">{}</span></div>"#,
        others.len()
    );
    for wallet in others {
        let label = if wallet.label.is_empty() {
            "Unnamed wallet"
        } else {
            wallet.label.as_str()
        };
        let balance = match (embers, wallet.balance.as_deref()) {
            (false, _) => String::new(),
            (true, Some(b)) => display::group_digits(b).into_owned(),
            (true, None) => "…".into(),
        };
        let balance_width = match balance.is_empty() {
            true => 0.0,
            false => fit.width(META, &balance) + ROW_GAP,
        };
        let title = fit
            .end(LABEL_STRONG, label, inner - ROW_ICON - ROW_GAP - balance_width)
            .into_owned();
        let address = fit.middle(MONO, &wallet.address, inner).into_owned();
        put!(
            html,
            r#"<div class="card"><div class="card-head"><i class="fa-solid fa-wallet row-icon"></i><span class="card-title">{title}</span><span class="row-meta">{balance}</span></div><div class="mono wallet-address">{address}</div><div class="card-actions"><button class="btn btn-sm" data-action="wallet:use:{a}"><i class="fa-solid fa-check"></i><span>Use for payments</span></button><button class="icon-btn icon-btn-sm" data-action="wallet:copy:{a}" aria-label="Copy address"><i class="fa-solid fa-copy"></i><span class="tip tip-below-end">Copy address</span></button><button class="icon-btn icon-btn-sm" data-action="wallet:export:{a}" aria-label="Export"><i class="fa-solid fa-file-export"></i><span class="tip tip-below-end">Export the wallet file</span></button><button class="icon-btn icon-btn-sm" data-action="wallet:remove:{a}" aria-label="Remove"><i class="fa-solid fa-trash-can"></i><span class="tip tip-below-end">Remove this wallet</span></button></div></div>"#,
            title = escape(&title),
            balance = escape(&balance),
            address = escape(&address),
            a = escape(&wallet.address),
        );
    }
    html
}

/// The wallet's message area: a notice, and a confirmation waiting for its
/// click.
fn wallet_message_html(
    fit: &mut TextFitter,
    notice: Option<&(Tone, String)>,
    confirm: Option<&WalletConfirm>,
    labels: &BTreeMap<String, String>,
) -> String {
    use geometry::*;
    let mut html = String::with_capacity(1_536);
    if let Some((tone, text)) = notice {
        html.push_str(&notice_html(*tone, text, Some("wallet:dismiss")));
    }
    let value_width = card_inner() - KV_KEY - KV_GAP - ROUNDING;
    match confirm {
        Some(WalletConfirm::Send {
            from,
            to,
            amount,
            note,
        }) => {
            let amount_text = display::group_digits(&amount.to_string()).into_owned();
            let from = fit.middle(MONO, from, value_width).into_owned();
            let to = fit.middle(MONO, to, value_width).into_owned();
            put!(
                html,
                r#"<div class="card"><div class="card-head"><i class="fa-solid fa-paper-plane row-icon"></i><span class="card-title">Review transfer</span></div><div class="kv"><div class="kv-row"><span class="kv-key">From</span><span class="kv-value mono">{from}</span></div><div class="kv-row"><span class="kv-key">To</span><span class="kv-value mono">{to}</span></div><div class="kv-row"><span class="kv-key">Amount</span><span class="kv-value">{amount}</span></div>"#,
                from = escape(&from),
                to = escape(&to),
                amount = escape(&amount_text),
            );
            if let Some(note) = note {
                let note = fit.end(LABEL, note, value_width).into_owned();
                put!(
                    html,
                    r#"<div class="kv-row"><span class="kv-key">Note</span><span class="kv-value">{}</span></div>"#,
                    escape(&note)
                );
            }
            put!(
                html,
                r#"</div><div class="card-actions end"><button class="btn btn-sm" data-action="wallet:cancel"><span>Cancel</span></button><button class="btn btn-sm btn-primary" data-action="wallet:confirm"><i class="fa-solid fa-paper-plane"></i><span>Send {}</span></button></div></div>"#,
                escape(&amount_text)
            );
        }
        Some(WalletConfirm::Remove { address, label }) => {
            let name = match (label.is_empty(), labels.get(address)) {
                (false, _) => label.clone(),
                (true, Some(known)) if !known.is_empty() => known.clone(),
                _ => display::abbreviate(address, 8, 6).into_owned(),
            };
            let title_width =
                card_inner() - ROW_ICON - ROW_GAP - fit.width(LABEL_STRONG, "Remove ?") - ROUNDING;
            let name = fit.end(LABEL_STRONG, &name, title_width).into_owned();
            put!(
                html,
                r#"<div class="card"><div class="card-head"><i class="fa-solid fa-triangle-exclamation row-icon warn"></i><span class="card-title">Remove {name}?</span></div><div class="card-text">Its key is deleted from this profile. Export it first if it holds funds.</div><div class="card-actions end"><button class="btn btn-sm" data-action="wallet:cancel"><span>Cancel</span></button><button class="btn btn-sm" data-action="wallet:export:{a}"><i class="fa-solid fa-file-export"></i><span>Export first</span></button><button class="btn btn-sm btn-primary btn-danger" data-action="wallet:confirm"><i class="fa-solid fa-trash-can"></i><span>Remove wallet</span></button></div></div>"#,
                name = escape(&name),
                a = escape(address),
            );
        }
        None => {}
    }
    html
}

/// The address box's identity badge: what kind of address the page has, or
/// a magnifier while the box holds a search or another address.
fn scheme_badge_html(url: &str, searching: bool) -> String {
    if searching {
        return r#"<span class="scheme-badge"><i class="fa-solid fa-magnifying-glass"></i><span class="tip tip-below">Matching open tabs and history</span></span>"#.into();
    }
    let kind = SchemeKind::of(url);
    let mut html = String::with_capacity(256);
    put!(
        html,
        r#"<span class="scheme-badge {tone}" data-action="show:grants"><i class="fa-solid {icon}"></i>"#,
        tone = kind.tone(),
        icon = kind.icon()
    );
    if let Some(label) = kind.label() {
        put!(html, "<span>{label}</span>");
    }
    put!(
        html,
        r#"<span class="tip tip-below">{} · click for this page's permissions</span></span>"#,
        kind.description()
    );
    html
}

/// The address box's dropdown.
fn suggestions_html(
    fit: &mut TextFitter,
    items: &[Suggestion],
    query: &str,
    highlighted: Option<usize>,
    window_width: f32,
) -> String {
    use geometry::*;
    let row = suggestion_row(window_width);
    let tag = "Switch to tab";
    let tag_width = fit.width(TAG, tag) + TAG_PAD + SUGGESTION_GAP;
    let mut html = String::with_capacity(560 * items.len());
    for (index, item) in items.iter().enumerate() {
        let title_room = (row - SUGGESTION_GAP) * SUGGESTION_TITLE_SHARE - ROUNDING;
        let title = if item.title.is_empty() {
            item.url.as_str()
        } else {
            item.title.as_str()
        };
        let title = fit.end_marked(LABEL, title, match_span(query, title), title_room);
        let used = fit.width(LABEL, &title.text);
        let url_room = row
            - used
            - SUGGESTION_GAP
            - if item.open_tab.is_some() { tag_width } else { 0.0 }
            - ROUNDING;
        let url = fit_detail(fit, MONO, &item.url, query, &title, url_room);
        let (action, icon) = match item.open_tab {
            Some(id) => (format!("select:{id}"), "fa-layer-group"),
            None => (format!("suggest:{}", escape(&item.url)), "fa-clock-rotate-left"),
        };
        put!(
            html,
            r#"<div class="suggestion{on}" data-action="{action}"><i class="fa-solid {icon}"></i><span class="suggestion-title">{title}</span><span class="suggestion-url">{url}</span>"#,
            on = if highlighted == Some(index) { " on" } else { "" },
            title = marked_html(&title),
            url = marked_html(&url),
        );
        if item.open_tab.is_some() {
            put!(html, r#"<span class="tag">{tag}</span>"#);
        }
        html.push_str("</div>");
    }
    html
}

/// The prompt bar for a pending question. The page's site is shown short and
/// bold (its full address in a hover bubble); the question itself is never
/// shortened, because consent prompts may quote costs.
fn prompt_bar_html(
    prompt_site: Option<&str>,
    text: &str,
    tab: u64,
    prompt: u64,
    total: usize,
    remember: bool,
) -> String {
    let mut html = String::with_capacity(900 + text.len());
    html.push_str(r#"<div class="prompt-bar"><i class="fa-solid fa-shield-halved prompt-icon"></i><div class="prompt-text">"#);
    match prompt_site.and_then(|site| text.strip_prefix(site).map(|rest| (site, rest))) {
        Some((site, rest)) => put!(
            html,
            r#"<span class="prompt-site">{label}<span class="tip tip-below">{site}</span></span><span>{rest}</span>"#,
            label = escape(&display::site_label(site)),
            site = escape(site),
            rest = escape(&display::without_default_ports(rest)),
        ),
        None => put!(
            html,
            "<span>{}</span>",
            escape(&display::without_default_ports(text))
        ),
    }
    html.push_str("</div>");
    if total > 1 {
        put!(html, r#"<span class="tag">1 of {total}</span>"#);
    }
    put!(
        html,
        r#"<span class="check{on}" data-action="remember"><span class="check-box"><i class="fa-solid fa-check"></i></span><span>Remember for this site</span></span><button class="btn btn-sm" data-action="deny:{tab}:{prompt}"><span>Deny</span></button><button class="btn btn-sm btn-primary" data-action="allow:{tab}:{prompt}"><span>Allow</span></button></div>"#,
        on = if remember { " on" } else { "" },
    );
    html
}

/// The stage a tab is in, as the status bar's chip shows it:
/// `(icon, label, tone class)`.
fn stage_chip(stage: &Stage) -> (&'static str, &'static str, &'static str) {
    match stage {
        Stage::Fetching => ("fa-hourglass-half", "Loading…", "info"),
        Stage::Scripts => ("fa-hourglass-half", "Verifying f1r3lang…", "info"),
        Stage::Grants => ("fa-shield-halved", "Waiting for your answer", "warn"),
        Stage::Running => ("fa-bolt", "Running f1r3lang", "ok"),
        Stage::Static => ("fa-file-lines", "Static page", ""),
        Stage::Failed(_) => ("fa-triangle-exclamation", "Couldn't load", "err"),
    }
}

/// The status bar, on one line: the stage, the reason a load failed or the
/// page's notice, the step-budget control, and a transient message.
fn status_html(
    fit: &mut TextFitter,
    window_width: f32,
    stage: &Stage,
    url: &str,
    notice: Option<&str>,
    stalled: bool,
    flash: Option<&Flash>,
) -> String {
    use geometry::*;
    let (icon, label, tone) = stage_chip(stage);
    let mut html = String::with_capacity(1_024);
    put!(
        html,
        r#"<span class="stage-chip {tone}"><i class="fa-solid {icon}"></i><span>{label}</span></span>"#
    );
    // Every other item is measured, so the free text gets exactly the rest.
    let mut used = 2.0 * STATUS_PAD_X + STATUS_ICON + STATUS_ICON_GAP + fit.width(SMALL, label);
    let mut tail = String::with_capacity(512);
    if stalled {
        tail.push_str(r#"<span class="tag warn">Step budget reached</span><button class="btn btn-xs" data-action="letitrun"><span>Let it run</span></button>"#);
        let tag = fit.width(TAG, "Step budget reached") + TAG_PAD;
        let button = fit.width(META, "Let it run") + 2.0 * 7.0 + 2.0;
        used += 2.0 * STATUS_GAP + tag + button;
    }
    if let Some(flash) = flash {
        let text = fit
            .end(SMALL, &flash.text, (window_width * 0.4).max(120.0))
            .into_owned();
        let text_width = fit.width(SMALL, &text);
        put!(
            tail,
            r#"<span class="grow"></span><span class="flash {}"><i class="fa-solid {}"></i><span>{}</span></span>"#,
            flash.tone.class(),
            flash.tone.icon(),
            escape(&text)
        );
        used += 2.0 * STATUS_GAP + STATUS_ICON + STATUS_ICON_GAP + text_width;
    }
    let free = window_width - used - STATUS_GAP - ROUNDING;
    match (stage, notice) {
        (Stage::Failed(why), _) => put!(
            html,
            r#"<span class="status-text">{}</span>"#,
            escape(&fit.end(SMALL, &display::failure_reason(url, why), free))
        ),
        (_, Some(notice)) => put!(
            html,
            r#"<span class="status-warn">{}</span>"#,
            escape(&fit.end(SMALL, notice, free))
        ),
        _ => {}
    }
    html.push_str(&tail);
    html
}

/// Which tabs fit in the strip: all of them, or a window of `capacity`
/// tabs that keeps the active one in view.
fn visible_window(len: usize, active: usize, capacity: usize) -> (usize, usize) {
    match capacity >= len {
        true => (0, len),
        false => {
            let capacity = capacity.max(1);
            let start = active.saturating_sub(capacity / 2).min(len - capacity);
            (start, start + capacity)
        }
    }
}

/// How the tab strip spends its room: the tabs `start..end` show, the
/// active one `active` px wide and each other one `other` px.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StripLayout {
    start: usize,
    end: usize,
    active: f32,
    other: f32,
}

/// Tabs share `room` equally, up to `TAB_MAX` each. Once that share drops
/// below `TAB_ACTIVE_MIN`, the active tab keeps `TAB_ACTIVE_MIN` (its title
/// stays readable) and the others share the rest. When the others would be
/// narrower than `TAB_MIN`, the room left after a "+N" chip holds a window
/// of as many tabs as fit at those minimums, around the active one.
fn strip_layout(count: usize, active: usize, room: f32) -> StripLayout {
    use geometry::*;
    // The active tab's width and the others' when `n` tabs share `room`.
    let split = |n: usize, room: f32| -> (f32, f32) {
        let equal = ((room - TAB_GAP * n.saturating_sub(1) as f32) / n.max(1) as f32).min(TAB_MAX);
        match (equal >= TAB_ACTIVE_MIN, n) {
            (true, _) => (equal, equal),
            (false, 0 | 1) => (equal.max(0.0), equal.max(0.0)),
            (false, _) => {
                let first = TAB_ACTIVE_MIN.min(room);
                let rest = (room - first - TAB_GAP * (n - 1) as f32) / (n - 1) as f32;
                (first, rest)
            }
        }
    };
    let (first, rest) = split(count, room);
    if count <= 1 || rest >= TAB_MIN {
        return StripLayout {
            start: 0,
            end: count,
            active: first,
            other: rest,
        };
    }
    let room = room - TABMORE - TAB_GAP;
    let others = ((room - TAB_ACTIVE_MIN) / (TAB_MIN + TAB_GAP)).floor().max(0.0) as usize;
    let (start, end) = visible_window(count, active, others + 1);
    let (first, rest) = split(end - start, room);
    StripLayout {
        start,
        end,
        active: first,
        other: rest,
    }
}

/// The tab strip's list (see [`strip_layout`]). A tab too narrow for any
/// of its title shows only its icon; an inactive tab's close button
/// appears over its title on hover. Hidden tabs are reached through the
/// "+N" chip, which opens the Tabs panel.
fn tab_strip_html(fit: &mut TextFitter, chips: &[TabChip<'_>], window_width: f32) -> String {
    use geometry::*;
    let count = chips.len();
    let active = chips.iter().position(|c| c.active).unwrap_or(0);
    let room = window_width - 2.0 * STRIP_PAD_X - STRIP_GAP - NEWTAB;
    let layout = match window_width > 0.0 {
        true => strip_layout(count, active, room),
        // Before the window has a size, show every tab at full width; the
        // next render, after the first resize, fits them.
        false => StripLayout {
            start: 0,
            end: count,
            active: TAB_MAX,
            other: TAB_MAX,
        },
    };
    let mut html = String::with_capacity(440 * (layout.end - layout.start) + 256);
    for chip in &chips[layout.start..layout.end] {
        let (icon, tone) = tab_icon(chip.url, chip.attention);
        let title = if chip.title.is_empty() { chip.url } else { chip.title };
        let (width, chrome) = match chip.active {
            true => (layout.active, TAB_CHROME),
            false => (layout.other, TAB_CHROME_INACTIVE),
        };
        let title = fit.end(LABEL, title, width - chrome - ROUNDING).into_owned();
        let classes = match (chip.active, title.is_empty() || title == ELLIPSIS) {
            (true, _) => "tab on",
            (false, true) => "tab narrow",
            (false, false) => "tab",
        };
        put!(
            html,
            r#"<div class="{classes}" data-tab-id="{id}" data-action="select:{id}" style="width:{width:.2}px"><i class="fa-solid {icon} tab-icon {tone}"></i><span class="tab-title">{title}</span><span class="tab-close" data-action="close:{id}" aria-label="Close tab"><i class="fa-solid fa-xmark"></i></span></div>"#,
            id = chip.id,
            title = escape(&title),
        );
    }
    let (start, end) = (layout.start, layout.end);
    let hidden = count - (end - start);
    if hidden > 0 {
        put!(
            html,
            r#"<button id="tabmore" class="chip" data-action="show:tabs"><span>+{hidden}</span><span class="tip tip-below-end">{} more in the Tabs panel</span></button>"#,
            plural(hidden, "tab", "tabs")
        );
    }
    html
}

/// The find box's match count, and whether it reports no match.
fn find_count(query: &str, hits: usize, index: usize) -> (String, bool) {
    match (query.is_empty(), hits) {
        (true, _) => (String::new(), false),
        (false, 0) => ("0/0".into(), true),
        (false, n) => (format!("{}/{n}", index + 1), false),
    }
}

/// What the address box shows for `url`: the new-tab page leaves it empty,
/// ready for typing (its hint says what to type).
fn display_address(url: &str) -> &str {
    match url {
        "gaze://newtab" => "",
        other => other,
    }
}

/// `"<tab id>:<session label>"` → the tab and the label.
fn session_target(arg: &str) -> Option<(u64, &str)> {
    let (tab, label) = arg.split_once(':')?;
    Some((tab.parse().ok()?, label))
}

// ── The chrome document ─────────────────────────────────────────────────

pub struct ChromeDocument {
    inner: BaseDocument,
    eng: Rc<Engine>,
    tabs: Vec<(Tab, NodeId)>,
    active: usize,
    next_id: u64,
    wake: WakeHandle,
    panel: String,
    remember: bool,
    consoles: BTreeMap<u64, Vec<(String, String)>>,
    rendered: BTreeMap<&'static str, String>,
    url_dirty: bool,
    title: String,
    /// The status bar's transient message.
    flash: Option<Flash>,
    /// Wakes the window when the message expires or find should look again.
    pacer: Pacer,
    /// Decides the window's cursor and the pointer's hover inside pages
    /// (`crate::cursor`; docs/ui/ledger.md, L8).
    cursor: CursorArbiter,
    /// Each tab's page's hovered node when the window began painting its
    /// frame, in tab order (`begin_paint`; ledger L9, H9). Kept to be reused.
    paint_hovers: Vec<Option<NodeId>>,
    wallet: Arc<Mutex<WalletView>>,
    // Was `ui: UiState`, the whole `workspace.json`.
    /// The open tabs and the sidebar's layout (`state/session.json`).
    session: SessionState,
    /// The pages visited (`state/history.json`).
    history: History,
    parents: BTreeMap<u64, Option<u64>>,
    lazy: BTreeMap<u64, String>,
    /// Closed tabs, the most recent last.
    closed: Vec<SavedTab>,
    drag: Option<u64>,
    menu_tab: Option<u64>,
    side_query: String,
    side_pages: bool,
    side_results: BTreeMap<u64, usize>,
    side_scan_cursor: usize,
    side_scan_at: Instant,
    address_query: String,
    /// The address box's suggestions as last rendered, and the one the
    /// arrow keys highlight.
    suggestions: Vec<Suggestion>,
    suggest_index: Option<usize>,
    history_confirm: bool,
    history_limit: usize,
    find_query: String,
    find_visible: bool,
    find_hits: Vec<FindHit>,
    find_index: usize,
    /// The tab whose page selection find made. Only that selection is ever
    /// cleared, so a selection the user made survives opening find.
    find_selected_tab: Option<u64>,
    /// A page attached while find was open, before it had a layout: look
    /// again once it has one.
    find_rescan: bool,
    /// A text field whose whole content the next render selects, if it
    /// still has the focus: the address box after Ctrl+L or Escape, the
    /// find box after Ctrl+F. Typing then replaces the old text.
    select_pending: Option<&'static str>,
    // Was `state_dirty`: any change rewrote the session, the history and
    // the theme together.
    /// The session changed since it was saved.
    session_dirty: bool,
    /// When the history is to be written (`history_changed`).
    history_due: Option<Instant>,
    /// The operating system's light or dark preference.
    system: SystemScheme,
    /// Where `system` comes from: reports from the window count only for
    /// `Source::Window`.
    system_source: Source,
    /// The preference the colours were resolved with.
    applied_preference: Option<Scheme>,
    /// The colours shown, and the theme file they came from.
    resolved: theme::Resolved,
    /// The built-in pages' style sheet for those colours.
    host_css: String,
    /// The theme files the Appearance panel lists: read when the panel is
    /// shown, by New theme and by Reload, never at each render (`None`: read
    /// at the next render).
    themes: Option<Vec<ThemeFile>>,
    /// What the chrome asks of its window (`take_window_requests`).
    window_requests: Vec<WindowRequest>,
    /// Start-up's notices, and failed saves of the freshness records, still
    /// to be shown in the status bar, most severe first.
    notices: VecDeque<(Tone, String)>,
    last_find_scan: Instant,
    fitter: TextFitter,
}

fn qn(k: &str) -> QualName {
    QualName::new(None, ns!(), LocalName::from(k))
}

/// Whether a page has been laid out (find works on its laid-out text).
fn page_is_laid_out(page: &RhoDocument) -> bool {
    let base = page.base();
    let doc = base.borrow();
    doc.root_element().final_layout().size.height > 0.0
}

impl ChromeDocument {
    /// A window on `url`, with the system's preference fixed as unknown:
    /// tests, which never run `dbus-send` (`launch` passes the real one).
    pub fn new(eng: Rc<Engine>, url: &str) -> ChromeDocument {
        ChromeDocument::with_system_scheme(
            eng,
            url,
            SystemScheme::fixed(Known::Unknown(None)),
            Source::Fixed(None),
            WakeHandle::default(),
        )
    }

    /// A window on `url`, following `system`'s preference, woken by `wake`.
    pub fn with_system_scheme(
        eng: Rc<Engine>,
        url: &str,
        system: SystemScheme,
        system_source: Source,
        wake: WakeHandle,
    ) -> ChromeDocument {
        // Was `UiState::load(&eng.dir)`: `workspace.json`, whose bad JSON
        // gave the default, which the next save wrote over (ledger S1, H1).
        // Start-up has checked, backed up and repaired both files now.
        let mut session: SessionState = eng.profile.read_state();
        let history: History = eng.profile.read_state();
        // Every window starts with the sidebar collapsed, unless the profile
        // asks to reopen it as it was left (`restore_sidebar`). The panel is
        // still restored, so Ctrl+B reopens the one last shown.
        if !eng.settings.restore_sidebar {
            session.sidebar_open = false;
        }
        let panel = session.panel.clone();
        // Was the palette file in the single profile folder, laid over the
        // chosen scheme whatever it was (ledger L14).
        let applied_preference = preference_of(&system.known());
        let resolved = theme::resolve(
            &eng.profile.theme(),
            applied_preference,
            eng.profile.fs(),
            &eng.profile.theme_dirs(),
        );
        let css = chrome_css_of(&resolved.colours);
        let host_css = theme::builtin_css_of(&resolved.colours);
        let html = shell_html(&css, session.sidebar_open);
        let inner = gaze_dom_blitz::parse_html(
            &html,
            DocumentConfig {
                net_provider: Some(Arc::new(gaze_net::GazeNetProvider::new(
                    eng.http.clone(),
                    eng.schemes.clone(),
                    eng.pool.clone(),
                    Some(Arc::new(ChromeNetWake(wake.clone()))),
                ))),
                font_ctx: Some(theme::font_context()),
                ..Default::default()
            },
        );
        let mut c = ChromeDocument {
            inner,
            eng,
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            pacer: Pacer::new(wake.clone()),
            cursor: CursorArbiter::new(wake.clone()),
            paint_hovers: Vec::new(),
            wake,
            panel,
            remember: false,
            consoles: BTreeMap::new(),
            rendered: BTreeMap::new(),
            url_dirty: true,
            title: String::new(),
            flash: None,
            wallet: Default::default(),
            session,
            history,
            parents: BTreeMap::new(),
            lazy: BTreeMap::new(),
            closed: Vec::with_capacity(CLOSED_LIMIT + 1),
            drag: None,
            menu_tab: None,
            side_query: String::new(),
            side_pages: false,
            side_results: BTreeMap::new(),
            side_scan_cursor: 0,
            side_scan_at: Instant::now(),
            address_query: String::new(),
            suggestions: Vec::new(),
            suggest_index: None,
            history_confirm: false,
            history_limit: HISTORY_PAGE,
            find_query: String::new(),
            find_visible: false,
            find_hits: Vec::new(),
            find_index: 0,
            find_selected_tab: None,
            find_rescan: false,
            select_pending: None,
            session_dirty: false,
            history_due: None,
            system,
            system_source,
            applied_preference,
            resolved,
            host_css,
            themes: None,
            window_requests: Vec::with_capacity(2),
            notices: VecDeque::with_capacity(8),
            last_find_scan: Instant::now(),
            fitter: TextFitter::new(),
        };
        if url == c.eng.settings.home && !c.session.tabs.is_empty() {
            let saved = c.session.tabs.clone();
            let mut ids = Vec::with_capacity(saved.len());
            for t in &saved {
                let parent = t.parent.and_then(|i| ids.get(i).copied());
                c.add_tab(&t.url, parent, false, &t.title);
                ids.push(c.next_id);
            }
            c.select(c.session.active.min(c.tabs.len() - 1));
        } else {
            c.open_tab(url);
        }
        c.paint_theme();
        // What start-up found is shown first, most severe first; so is a
        // chosen theme that cannot be used.
        c.take_notices();
        if let Some(problem) = c.resolved.problem.clone() {
            let shown = match c.resolved.scheme {
                Scheme::Dark => "dark",
                Scheme::Light => "light",
            };
            c.notices.push_back((Tone::Warn, format!("The chosen theme cannot be used ({problem}); the {shown} scheme is shown")));
        }
        // A profile reopened on the Wallet panel shows balances at once.
        if c.session.sidebar_open && c.panel == "wallet" {
            c.refresh_balances();
        }
        c
    }

    fn id(&self, id: &str) -> Option<NodeId> {
        self.inner.get_element_by_id(id)
    }

    fn focused(&self, id: &str) -> bool {
        self.id(id)
            .is_some_and(|node| self.inner.get_focussed_node_id() == Some(node))
    }

    fn key_focus(&self) -> KeyFocus {
        match self.inner.get_focussed_node_id() {
            Some(node) if Some(node) == self.id("url") => KeyFocus::Address,
            Some(node) if Some(node) == self.id("find") => KeyFocus::Find,
            Some(node) if Some(node) == self.id("side-search") => KeyFocus::SideSearch,
            _ => KeyFocus::Elsewhere,
        }
    }

    /// Set (`Some`) or remove (`None`) an attribute on the element with a
    /// stable id, touching the DOM only when the live value differs.
    fn sync_attr(&mut self, id: &str, name: &str, want: Option<&str>) -> bool {
        let Some(node) = self.id(id) else {
            return false;
        };
        let have = attr_of(&self.inner, node, name);
        match (have.as_deref(), want) {
            (Some(have), Some(want)) if have == want => false,
            (None, None) => false,
            (_, Some(want)) => {
                self.inner.mutate().set_attribute(node, qn(name), want);
                true
            }
            (Some(_), None) => {
                self.inner.mutate().clear_attribute(node, qn(name));
                true
            }
        }
    }

    /// A text field's content as typed (not trimmed).
    fn raw_input_value(&self, id: &str) -> String {
        self.id(id)
            .and_then(|n| self.inner.get_node(n))
            .and_then(|n| n.element_data())
            .and_then(|e| e.text_input_data())
            .map(|t| t.editor.raw_text().to_string())
            .unwrap_or_default()
    }

    fn input_value(&self, id: &str) -> String {
        self.raw_input_value(id).trim().to_string()
    }

    fn set_input_value(&mut self, id: &str, value: &str) {
        if let Some(n) = self.id(id) {
            self.inner.mutate().set_attribute(n, qn("value"), value);
        }
    }

    fn clear_input(&mut self, id: &str) {
        self.set_input_value(id, "");
    }

    /// Show a transient message in the status bar.
    fn flash(&mut self, tone: Tone, text: impl Into<String>) {
        let ttl = match tone {
            Tone::Info | Tone::Ok => FLASH_BRIEF,
            Tone::Warn | Tone::Err => FLASH_LONG,
        };
        let until = Instant::now() + ttl;
        self.flash = Some(Flash {
            tone,
            text: text.into(),
            until,
        });
        self.pacer.at(until);
    }

    /// The window's width in CSS px (0 before the window has a size).
    fn window_width(&self) -> f32 {
        let viewport = self.inner.viewport();
        viewport.window_size.0 as f32 / viewport.scale()
    }

    fn open_tab(&mut self, url: &str) {
        let parent = self.tabs.get(self.active).map(|(t, _)| t.id);
        self.add_tab(url, parent, true, "");
    }

    /// Open a URL delivered to the running macOS app by Launch Services.
    #[cfg(target_os = "macos")]
    pub(crate) fn open_external_url(&mut self, url: &str) {
        self.open_tab(url);
    }

    fn add_tab(&mut self, url: &str, parent: Option<u64>, load: bool, title: &str) {
        self.next_id += 1;
        let main = self.id("main").expect("chrome has #main");
        let view = {
            let mut m = self.inner.mutate();
            let v = m.create_element(
                QualName::new(None, ns!(html), LocalName::from("div")),
                vec![blitz_dom::Attribute {
                    name: qn("class"),
                    value: "view".into(),
                }],
            );
            m.append_children(main, &[v]);
            v
        };
        let mut tab = Tab::new(
            Rc::clone(&self.eng),
            self.next_id,
            self.wake.clone(),
            Some(self.cursor.page_shell()),
        );
        if load {
            tab.navigate(url, true);
        } else {
            tab.url = url.to_string();
            tab.title = title.to_string();
            self.lazy.insert(tab.id, url.to_string());
        }
        self.parents.insert(tab.id, parent);
        self.tabs.push((tab, view));
        if load {
            self.select(self.tabs.len() - 1);
        }
        self.session_dirty = true;
    }

    fn select(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        // Ledger L1/H6: the outgoing tab keeps no find selection.
        self.clear_find_selection();
        self.active = i;
        let id = self.tabs[i].0.id;
        if let Some(url) = self.lazy.remove(&id) {
            self.tabs[i].0.navigate(&url, true);
        }
        let views: Vec<(NodeId, bool)> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(j, (_, v))| (*v, j == i))
            .collect();
        {
            let mut m = self.inner.mutate();
            for (v, on) in &views {
                m.set_attribute(*v, qn("class"), if *on { "view" } else { "view hidden" });
            }
        }
        for (v, on) in views {
            if let Some(r) = rho_mut(&mut self.inner, v) {
                r.set_foreground(on);
            }
        }
        self.menu_tab = None;
        self.dismiss_address();
        // Ledger L1/H6: find re-runs on the incoming tab.
        self.refresh_find();
        self.session_dirty = true;
    }

    fn close(&mut self, id: u64) {
        let Some(i) = self
            .tabs
            .iter()
            .position(|(t, _)| t.id == id || id == u64::MAX && t.id == self.tabs[self.active].0.id)
        else {
            return;
        };
        let selected = self.tabs[self.active].0.id;
        let (mut tab, view) = self.tabs.remove(i);
        // Keep the most recent closed tabs; the oldest are dropped first.
        self.closed.push(SavedTab {
            url: tab.url.clone(),
            title: tab.title.clone(),
            parent: None,
        });
        if self.closed.len() > CLOSED_LIMIT {
            let excess = self.closed.len() - CLOSED_LIMIT;
            self.closed.drain(..excess);
        }
        if self.find_selected_tab == Some(tab.id) {
            self.find_selected_tab = None;
        }
        if self.menu_tab == Some(tab.id) {
            self.menu_tab = None;
        }
        tab.close();
        self.cursor.view_removed(view);
        self.inner.remove_sub_document(view);
        {
            let mut m = self.inner.mutate();
            m.remove_and_drop_node(view);
        }
        self.consoles.remove(&tab.id);
        self.side_results.remove(&tab.id);
        self.side_scan_cursor = 0;
        self.parents.remove(&tab.id);
        self.lazy.remove(&tab.id);
        for p in self.parents.values_mut() {
            if *p == Some(tab.id) {
                *p = None;
            }
        }
        if self.tabs.is_empty() {
            let home = self.eng.settings.home.clone();
            self.open_tab(&home);
        } else {
            let next = self
                .tabs
                .iter()
                .position(|(t, _)| t.id == selected)
                .unwrap_or(i.min(self.tabs.len() - 1));
            self.select(next);
        }
        self.session_dirty = true;
    }

    fn saved_tabs(&self) -> Vec<SavedTab> {
        self.tabs
            .iter()
            .map(|(t, _)| SavedTab {
                url: t.url.clone(),
                title: t.title.clone(),
                parent: self
                    .parents
                    .get(&t.id)
                    .and_then(|p| *p)
                    .and_then(|p| self.tabs.iter().position(|(t, _)| t.id == p)),
            })
            .collect()
    }

    // Disabled at the switch-over (ledger S14): it wrote the session, the
    // whole history and the theme into one file after every change, a tab
    // switch included. `save_state` writes each file when it changed.
    // fn save_workspace(&mut self) {
    //     if !self.state_dirty {
    //         return;
    //     }
    //     self.ui.tabs = self.saved_tabs();
    //     self.ui.active = self.active;
    //     let _ = self.ui.save(&self.eng.dir);
    //     self.state_dirty = false;
    // }

    /// Saves what changed: the session at once, the history once its delay
    /// is over at `now`. A save that fails changes nothing on screen: a
    /// session that only reads said so when it started.
    fn save_state(&mut self, now: Instant) {
        if self.session_dirty {
            self.session.tabs = self.saved_tabs();
            self.session.active = self.active;
            let _ = self.eng.profile.write_state(&self.session);
            self.session_dirty = false;
        }
        if self.history_due.is_some_and(|due| due <= now) {
            let _ = self.eng.profile.write_state(&self.history);
            self.history_due = None;
        }
    }

    /// The history changed. A visit is written once [`HISTORY_SAVE_DELAY`]
    /// has passed, with the visits made meanwhile; clearing and forgetting
    /// are written at the next save (`at_once`), since the user expects
    /// them gone.
    fn history_changed(&mut self, at_once: bool) {
        let now = Instant::now();
        match at_once {
            true => self.history_due = Some(now),
            false => {
                self.history_due.get_or_insert(now + HISTORY_SAVE_DELAY);
            }
        }
    }

    /// Saves everything still waiting, the history's delay or not: the
    /// window is closing.
    fn flush_state(&mut self) {
        let now = Instant::now();
        if self.history_due.is_some() {
            self.history_due = Some(now);
        }
        self.save_state(now);
    }

    /// Error pages are F1R3Gaze's own, so they are themed like `gaze://`.
    fn themed_as_builtin(tab: &Tab) -> bool {
        tab.url.starts_with("gaze://") || tab.error_page
    }

    // Disabled at the switch-over (ledger S14): it laid the old folder's
    // `palette.css` over whichever scheme was chosen (ledger L14), and saved
    // the theme with the session. The theme is a setting now, saved by
    // `Profile::set_theme`.
    // fn apply_theme(&mut self) {
    //     let custom = theme::custom_palette(&self.eng.dir).ok();
    //     let colors = theme::palette(&self.ui.theme, custom.as_ref());
    //     if let Some(root) = self
    //         .id("html")
    //         .or_else(|| Some(self.inner.root_element().id))
    //     {
    //         let mut m = self.inner.mutate();
    //         for (key, value) in &colors {
    //             m.set_style_property(root, key, value);
    //         }
    //     }
    //     let css = theme::builtin_css(&self.ui.theme, custom.as_ref());
    //     for (t, view) in &self.tabs {
    //         if Self::themed_as_builtin(t)
    //             && let Some(r) = rho_mut(&mut self.inner, *view)
    //         {
    //             r.set_host_theme(&css);
    //         }
    //     }
    //     // Rebuild every rendered region with fresh boxes. The constant side
    //     // controls stay: rebuilding them would take focus from their field.
    //     self.rendered.retain(|id, _| *id == "sidecontrols");
    //     self.state_dirty = true;
    // }

    /// Resolves the chosen theme with the system's preference now, and
    /// shows it.
    fn apply_theme(&mut self) {
        let preference = preference_of(&self.system.known());
        self.applied_preference = preference;
        self.resolved = theme::resolve(
            &self.eng.profile.theme(),
            preference,
            self.eng.profile.fs(),
            &self.eng.profile.theme_dirs(),
        );
        self.paint_theme();
    }

    /// Shows the resolved colours: the chrome's variables, the style sheet
    /// of every built-in page, and the scheme, to the pages (ledger L13) and
    /// to the window.
    fn paint_theme(&mut self) {
        if let Some(root) = self
            .id("html")
            .or_else(|| Some(self.inner.root_element().id))
        {
            let mut m = self.inner.mutate();
            for (key, value) in &self.resolved.colours {
                m.set_style_property(root, key, value);
            }
        }
        self.host_css = theme::builtin_css_of(&self.resolved.colours);
        for (t, view) in &self.tabs {
            if Self::themed_as_builtin(t)
                && let Some(r) = rho_mut(&mut self.inner, *view)
            {
                r.set_host_theme(&self.host_css);
            }
        }
        // Rebuild every rendered region with fresh boxes. The constant side
        // controls stay: rebuilding them would take focus from their field.
        self.rendered.retain(|id, _| *id == "sidecontrols");
        // L13: pages see the scheme shown. Blitz copies this document's scheme
        // into every page's viewport at each layout (`BaseDocument::resolve`),
        // and the window is asked to keep its own theme from replacing it
        // (`WindowRequest::Scheme`).
        self.inner.viewport_mut().color_scheme = color_scheme_of(self.resolved.scheme);
        self.request_scheme();
    }

    /// Follows a change of the system's preference: the theme is resolved
    /// again, and shown only when its colours changed (System, or a named
    /// theme that fell back to the preferred scheme). Returns whether it
    /// was shown.
    fn follow_system(&mut self) -> bool {
        let preference = preference_of(&self.system.known());
        if preference == self.applied_preference {
            return false;
        }
        let colours = self.resolved.colours.clone();
        self.apply_theme();
        self.resolved.colours != colours
    }

    /// The system's preference as the window system reported it (macOS and
    /// Windows: winit's `system_theme` and `ThemeChanged`). The preference
    /// is ignored unless it comes from the window; either way the window is
    /// asked again to show the scheme shown, since the window system may have
    /// changed its decorations itself (Windows applies the system's theme on
    /// a settings change).
    // Was: an early return for any other source, which asked the window
    // nothing (step 9).
    pub(crate) fn window_reported_scheme(&mut self, preference: Option<Scheme>) {
        if self.system_source == Source::Window {
            self.system.set(preference);
            if self.follow_system() {
                self.inner.shell_provider.request_redraw();
            }
        }
        self.request_scheme();
    }

    /// The window gained the focus: the portal is asked again, at most every
    /// `REQUERY_INTERVAL` (Linux; the answer wakes the window).
    pub(crate) fn window_focused(&mut self, now: Instant) {
        self.system.refresh(now);
    }

    /// Asks the window to show the scheme shown.
    fn request_scheme(&mut self) {
        let request = WindowRequest::Scheme {
            effective: self.resolved.scheme,
            follows_system: self.follows_system(),
        };
        self.request(request);
    }

    /// Queues a request for the window. Only the latest `Scheme` matters,
    /// and one `FollowSystem` is enough.
    fn request(&mut self, request: WindowRequest) {
        match request {
            WindowRequest::Scheme { .. } => self
                .window_requests
                .retain(|queued| !matches!(queued, WindowRequest::Scheme { .. })),
            WindowRequest::FollowSystem if self.window_requests.contains(&request) => return,
            WindowRequest::FollowSystem | WindowRequest::ToggleFullScreen => {}
        }
        self.window_requests.push(request);
    }

    /// Whether the scheme shown is the system's: System, or a theme file
    /// that cannot be used and fell back to the system's preference.
    fn follows_system(&self) -> bool {
        match self.eng.profile.theme() {
            ThemeChoice::System => true,
            ThemeChoice::Named(_) => self.resolved.file.is_none(),
            ThemeChoice::BuiltIn(_) => false,
        }
    }

    /// What the chrome asked of its window since the last call
    /// (`ChromeApplication` carries it out).
    pub(crate) fn take_window_requests(&mut self) -> Vec<WindowRequest> {
        std::mem::take(&mut self.window_requests)
    }

    /// The scheme shown, and whether it is the system's: what the window is
    /// made with (`ChromeApplication::create_window`).
    pub(crate) fn scheme_shown(&self) -> (Scheme, bool) {
        (self.resolved.scheme, self.follows_system())
    }

    /// The window entered or left full screen. On entering, also when it is
    /// made full screen as it was left, the status bar says how to leave;
    /// on leaving, that message goes.
    pub(crate) fn full_screen_changed(&mut self, on: bool) {
        let hint = full_screen_hint(KeyPlatform::CURRENT);
        match on {
            true => self.flash(Tone::Info, hint),
            false if self.flash.as_ref().is_some_and(|flash| flash.text == hint) => self.flash = None,
            false => {}
        }
        // The status bar is drawn at the next poll.
        self.wake.wake();
    }

    /// Moves start-up's events not shown yet, and a failed save of the
    /// freshness records, into the notices, most severe first.
    fn take_notices(&mut self) {
        let profile = &self.eng.profile;
        if let Some(error) = self.eng.bridge.take_freshness_error() {
            profile.report.push(Event::new(
                EventKind::Other,
                Severity::Alert,
                profile.layout.trust_file(),
                format!("The freshness records could not be saved ({error}); they hold for this session only"),
            ));
        }
        let mut events = profile.report.take_unshown();
        // Stable: the most severe first, each severity in the order found.
        events.sort_by_key(|event| std::cmp::Reverse(event.severity));
        for event in events {
            let tone = match event.severity {
                Severity::Alert => Tone::Err,
                Severity::Warning => Tone::Warn,
                Severity::Notice | Severity::Quiet => Tone::Info,
            };
            self.notices.push_back((tone, event.message));
        }
    }

    /// Shows the next notice once the status bar's message has expired.
    /// Returns whether one is shown.
    fn show_next_notice(&mut self, now: Instant) -> bool {
        if self.flash.as_ref().is_some_and(|f| f.until > now) {
            return false;
        }
        let Some((tone, text)) = self.notices.pop_front() else {
            return false;
        };
        let until = now + FLASH_LONG;
        self.flash = Some(Flash { tone, text, until });
        self.pacer.at(until);
        true
    }

    // ── Find ──

    /// Clear the page selection that find made, wherever it is.
    fn clear_find_selection(&mut self) {
        let Some(tab_id) = self.find_selected_tab.take() else {
            return;
        };
        let view = self
            .tabs
            .iter()
            .find_map(|(t, view)| (t.id == tab_id).then_some(*view));
        if let Some(page) = view.and_then(|view| rho_mut(&mut self.inner, view)) {
            page.select_find_hit(None);
        }
    }

    /// Select (and, when `reveal`, scroll to) hit `find_index` in the active
    /// tab, recording that find owns the selection.
    fn show_find_hit(&mut self, reveal: bool) {
        let (tab_id, view) = (self.tabs[self.active].0.id, self.tabs[self.active].1);
        let Some(page) = rho_mut(&mut self.inner, view) else {
            return;
        };
        match self.find_hits.get(self.find_index) {
            Some(hit) => {
                page.select_find_hit(Some(hit));
                if reveal {
                    page.reveal_find_hit(hit, FIND_REVEAL_TOP_MARGIN);
                }
                self.find_selected_tab = Some(tab_id);
            }
            None => self.clear_find_selection(),
        }
    }

    /// Search the active tab for the current query. Ledger L1/H1: whatever
    /// the outcome, the previous query's selection never outlives it.
    fn refresh_find(&mut self) {
        self.clear_find_selection();
        self.find_hits.clear();
        self.find_index = 0;
        self.find_rescan = false;
        if let (true, false) = (self.find_visible, self.find_query.is_empty()) {
            let view = self.tabs[self.active].1;
            if let Some(page) = rho_mut(&mut self.inner, view) {
                match page_is_laid_out(page) {
                    true => self.find_hits = page.find(&self.find_query, FIND_LIMIT),
                    // Ledger L1/H7: a page attached a moment ago has no
                    // layout yet, so its text cannot be searched; look again
                    // once it has been laid out.
                    false => self.find_rescan = true,
                }
            }
            self.show_find_hit(true);
        }
    }

    fn step_find(&mut self, delta: i32) {
        if self.find_hits.is_empty() {
            self.refresh_find();
        }
        let count = self.find_hits.len();
        if count == 0 {
            return;
        }
        self.find_index =
            (self.find_index as i64 + i64::from(delta)).rem_euclid(count as i64) as usize;
        self.show_find_hit(true);
    }

    // ── Address box ──

    /// Close the suggestions and put the page's address back in the box.
    /// While the box keeps the focus (Escape), the restored address is
    /// selected, as after Ctrl+L.
    fn dismiss_address(&mut self) {
        self.address_query.clear();
        self.suggestions.clear();
        self.suggest_index = None;
        self.url_dirty = true;
        if self.focused("url") {
            self.select_pending = Some("url");
        }
    }

    /// Select the whole content of the text field `id`, the caret at its
    /// start (a long address shows its beginning). Whether anything was done.
    fn select_whole(&mut self, id: &str) -> bool {
        let Some(node) = self.id(id) else {
            return false;
        };
        let len = self.raw_input_value(id).len();
        self.inner
            .with_text_input(node, |mut editor| editor.select_byte_range(len, 0));
        true
    }

    fn navigate_active(&mut self, url: &str) {
        let i = self.active;
        self.tabs[i].0.navigate(url, true);
        self.url_dirty = true;
        self.session_dirty = true;
    }

    /// The suggestions for what is typed: open tabs (switched to, not
    /// reopened) and history, without the active tab itself.
    fn build_suggestions(&self) -> Vec<Suggestion> {
        let active_url = &self.tabs[self.active].0.url;
        self.history
            .suggestions(&self.address_query, &self.saved_tabs())
            .into_iter()
            .filter(|found| &found.url != active_url)
            .map(|found| Suggestion {
                open_tab: self
                    .tabs
                    .iter()
                    .find(|(t, _)| t.url == found.url)
                    .map(|(t, _)| t.id),
                title: found.title,
                url: found.url,
            })
            .collect()
    }

    // ── Sidebar ──

    /// Show `panel` in the sidebar (opening it). A different panel starts
    /// with a fresh search, a closed menu and no pending confirmation.
    fn show_panel(&mut self, panel: &str) {
        if !PANELS.iter().any(|(name, _, _)| *name == panel) {
            return;
        }
        if self.panel != panel {
            self.reset_side_search();
            self.menu_tab = None;
            self.history_confirm = false;
            self.panel = panel.to_string();
        }
        self.session.panel = self.panel.clone();
        self.set_sidebar(true);
        if self.panel == "wallet" {
            self.refresh_balances();
        }
        // The theme files may have changed while the panel was hidden.
        if self.panel == "appearance" {
            self.themes = None;
        }
        if self.panel == "tabs" {
            self.side_scan_cursor = 0;
            self.side_results.clear();
        }
        self.session_dirty = true;
    }

    fn set_sidebar(&mut self, open: bool) {
        let opening = open && !self.session.sidebar_open;
        self.session.sidebar_open = open;
        if !open {
            self.menu_tab = None;
            self.history_confirm = false;
        }
        if opening && self.panel == "wallet" {
            self.refresh_balances();
        }
        if opening && self.panel == "appearance" {
            self.themes = None;
        }
        self.session_dirty = true;
    }

    fn reset_side_search(&mut self) {
        self.side_query.clear();
        self.side_results.clear();
        self.side_scan_cursor = 0;
        self.history_limit = HISTORY_PAGE;
    }

    fn tree_depth(&self, id: u64) -> usize {
        let mut depth = 0;
        let mut parent = self.parents.get(&id).copied().flatten();
        while let Some(p) = parent {
            depth += 1;
            if depth >= 5 {
                break;
            }
            parent = self.parents.get(&p).copied().flatten();
        }
        depth
    }

    fn tab_op(&mut self, verb: &str, id: u64) {
        match verb {
            "restore" => {
                if let Some(t) = self.closed.pop() {
                    self.open_tab(&t.url);
                }
            }
            "restore-at" => {
                let index = id as usize;
                if index < self.closed.len() {
                    let t = self.closed.remove(index);
                    self.open_tab(&t.url);
                }
            }
            "move-to" => {
                let (from, to) = (id >> 32, id & 0xffff_ffff);
                if let (Some(a), Some(b)) = (
                    self.tabs.iter().position(|(t, _)| t.id == from),
                    self.tabs.iter().position(|(t, _)| t.id == to),
                ) {
                    let selected = self.tabs[self.active].0.id;
                    let tab = self.tabs.remove(a);
                    let dest = if a < b { b - 1 } else { b };
                    self.tabs.insert(dest, tab);
                    self.active = self
                        .tabs
                        .iter()
                        .position(|(t, _)| t.id == selected)
                        .unwrap_or(self.active);
                }
            }
            _ => self.tab_op_on(verb, id),
        }
        // The menu stays open only for the verbs that open or toggle it.
        if !matches!(verb, "menu" | "menu-open") {
            self.menu_tab = None;
        }
        self.session_dirty = true;
    }

    /// Tab verbs that act on one existing tab.
    fn tab_op_on(&mut self, verb: &str, id: u64) {
        let Some(i) = self.tabs.iter().position(|(t, _)| t.id == id) else {
            return;
        };
        let selected = self.tabs[self.active].0.id;
        let reselect = |tabs: &[(Tab, NodeId)]| {
            tabs.iter()
                .position(|(t, _)| t.id == selected)
                .expect("the selected tab is still open")
        };
        match verb {
            "menu" => {
                self.menu_tab = match self.menu_tab == Some(id) {
                    true => None,
                    false => Some(id),
                };
            }
            "menu-open" => {
                self.show_panel("tabs");
                self.menu_tab = Some(id);
            }
            "duplicate" => {
                let url = self.tabs[i].0.url.clone();
                self.add_tab(&url, Some(id), true, "");
            }
            "left" if i > 0 => {
                self.tabs.swap(i, i - 1);
                self.active = reselect(&self.tabs);
            }
            "right" if i + 1 < self.tabs.len() => {
                self.tabs.swap(i, i + 1);
                self.active = reselect(&self.tabs);
            }
            "others" => {
                let ids: Vec<u64> = self
                    .tabs
                    .iter()
                    .filter(|(t, _)| t.id != id)
                    .map(|(t, _)| t.id)
                    .collect();
                for id in ids {
                    self.close(id);
                }
                self.select(0);
            }
            "close-right" => {
                let ids: Vec<u64> = self.tabs.iter().skip(i + 1).map(|(t, _)| t.id).collect();
                for id in ids {
                    self.close(id);
                }
            }
            "branch" => {
                let mut ids = vec![id];
                let mut cursor = 0;
                while cursor < ids.len() {
                    let parent = ids[cursor];
                    ids.extend(
                        self.parents
                            .iter()
                            .filter(|(_, p)| **p == Some(parent))
                            .map(|(id, _)| *id),
                    );
                    cursor += 1;
                }
                for id in ids.into_iter().rev() {
                    self.close(id);
                }
            }
            "copy" => {
                let url = self.tabs[i].0.url.clone();
                match self.inner.shell_provider.set_clipboard_text(url) {
                    Ok(()) => self.flash(Tone::Ok, "Address copied"),
                    Err(_) => self.flash(Tone::Warn, "The clipboard is not available"),
                }
            }
            _ => {}
        }
    }

    fn act(&mut self, a: Action) {
        let i = self.active;
        match a {
            Action::Back => {
                self.tabs[i].0.back();
                self.url_dirty = true;
            }
            Action::Fwd => {
                self.tabs[i].0.forward();
                self.url_dirty = true;
            }
            Action::Reload => self.tabs[i].0.reload(),
            Action::Go(v) => {
                let v = v.unwrap_or_else(|| self.raw_input_value("url"));
                if !v.trim().is_empty() {
                    self.navigate_active(&v);
                }
                self.dismiss_address();
            }
            Action::AddressSubmit => {
                let typed = self.raw_input_value("url");
                let chosen = self
                    .suggest_index
                    .and_then(|index| self.suggestions.get(index))
                    .cloned();
                match chosen {
                    Some(Suggestion {
                        open_tab: Some(id), ..
                    }) => {
                        if let Some(j) = self.tabs.iter().position(|(t, _)| t.id == id) {
                            self.select(j);
                        }
                    }
                    Some(Suggestion { url, .. }) => self.navigate_active(&url),
                    None if !typed.trim().is_empty() => self.navigate_active(&typed),
                    None => {}
                }
                self.dismiss_address();
                // The page takes the keyboard once its address is chosen.
                let view = self.tabs[self.active].1;
                self.inner.set_focus_to(view);
            }
            Action::SuggestMove(delta) => {
                self.suggest_index =
                    next_suggestion(self.suggest_index, delta, self.suggestions.len());
            }
            Action::AddressDismiss => self.dismiss_address(),
            Action::NewTab => {
                let home = self.eng.settings.home.clone();
                self.open_tab(&home);
                if let Some(n) = self.id("url") {
                    self.inner.set_focus_to(n);
                }
            }
            Action::Close(id) => self.close(id),
            Action::Select(id) => {
                if let Some(j) = self.tabs.iter().position(|(t, _)| t.id == id) {
                    self.select(j);
                }
            }
            Action::Answer(tid, pid, yes) => {
                let remember = self.remember;
                if let Some((t, _)) = self.tabs.iter_mut().find(|(t, _)| t.id == tid) {
                    t.answer(pid, yes, remember);
                }
                // "Remember" applies to the answer given, not to later ones.
                self.remember = false;
            }
            Action::Remember => self.remember = !self.remember,
            Action::Revoke(urn) => {
                let view = self.tabs[i].1;
                if let Some(r) = rho_mut(&mut self.inner, view) {
                    self.tabs[i].0.revoke(&urn, r);
                }
            }
            Action::Panel(p) => match (self.session.sidebar_open, self.panel == p) {
                (true, true) => self.set_sidebar(false),
                _ => self.show_panel(&p),
            },
            Action::ShowPanel(p) => self.show_panel(&p),
            Action::SidebarToggle => {
                let open = !self.session.sidebar_open;
                self.set_sidebar(open);
            }
            Action::Wallet(w) => self.wallet_action(&w),
            Action::LetItRun => {
                let view = self.tabs[i].1;
                if let Some(r) = rho_mut(&mut self.inner, view) {
                    r.grant_budget(200_000);
                }
            }
            Action::SaveLog => self.save_log(),
            Action::FocusUrl => {
                if let Some(n) = self.id("url") {
                    self.inner.set_focus_to(n);
                    self.select_pending = Some("url");
                }
            }
            Action::Input(name, value) => match name.as_str() {
                "url" => {
                    self.address_query = value;
                    self.suggest_index = None;
                }
                "find" => {
                    self.find_query = value;
                    self.refresh_find();
                }
                "side-search" => {
                    self.side_query = value;
                    self.side_results.clear();
                    self.side_scan_cursor = 0;
                    self.history_limit = HISTORY_PAGE;
                }
                _ => {}
            },
            Action::Theme(op) => self.theme_op(op, Instant::now()),
            Action::Find(_) => {
                self.find_visible = true;
                // Focus moves without a blur event, so drop the address box's
                // suggestions here.
                self.dismiss_address();
                if let Some(n) = self.id("find") {
                    self.inner.set_focus_to(n);
                    self.select_pending = Some("find");
                }
                self.refresh_find();
            }
            Action::FindNext(delta) => self.step_find(delta),
            Action::FindHide => {
                self.find_visible = false;
                self.find_hits.clear();
                self.find_index = 0;
                self.find_rescan = false;
                self.clear_find_selection();
                // The closed box is only hidden; keyboard focus returns to
                // the page instead of staying in an invisible field.
                if self.focused("find") {
                    self.inner.set_focus_to(self.tabs[i].1);
                }
            }
            Action::TabOp(verb, id) => self.tab_op(&verb, id),
            Action::ToggleTree => {
                self.session.tree_tabs = !self.session.tree_tabs;
                self.session_dirty = true;
            }
            Action::ToggleSidePages => {
                self.side_pages = !self.side_pages;
                self.side_results.clear();
                self.side_scan_cursor = 0;
            }
            Action::SideClear => {
                self.reset_side_search();
                self.clear_input("side-search");
            }
            Action::HistoryOp(verb, _) => self.history_op(&verb),
            Action::VisitOp(verb, url) => match verb.as_str() {
                "open" => self.open_tab(&url),
                "forget" => {
                    self.history.forget(&url);
                    self.history_changed(true);
                }
                _ => {}
            },
            Action::SiteOp(verb, site) => self.site_op(&verb, &site),
            Action::ConsoleClear => {
                self.consoles.remove(&self.tabs[i].0.id);
            }
            Action::FullScreen => self.request(WindowRequest::ToggleFullScreen),
            Action::Dismiss => match (
                self.address_query.is_empty(),
                self.menu_tab,
                self.history_confirm,
            ) {
                (false, _, _) => self.dismiss_address(),
                (true, Some(_), _) => self.menu_tab = None,
                (true, None, true) => self.history_confirm = false,
                (true, None, false) => {
                    if let Ok(mut v) = self.wallet.lock() {
                        v.confirm = None;
                    }
                }
            },
        }
        self.wake.wake();
    }

    fn save_log(&mut self) {
        let (tid, view) = (self.tabs[self.active].0.id, self.tabs[self.active].1);
        match rho_mut(&mut self.inner, view).and_then(|r| r.log_bytes()) {
            Some(bytes) => {
                let profile = &self.eng.profile;
                if let Some(why) = profile.read_only_reason() {
                    self.flash(Tone::Err, format!("The log is not saved: {why}"));
                    return;
                }
                // Was `<profile>/logs/`, in the single profile folder.
                let dir = profile.layout.replay_logs_dir();
                let p = dir.join(format!("tab-{tid}-{}.gzlog", std::process::id()));
                let written = gaze_fs::create_dir_durably(profile.fs(), &dir)
                    .and_then(|_| gaze_fs::write_atomic(profile.fs(), &p, &bytes, Perm::Private));
                match written {
                    Ok(()) => self.flash(Tone::Ok, format!("Replay log saved to {}", p.display())),
                    Err(e) => self.flash(Tone::Err, format!("Could not save the log: {e}")),
                }
            }
            None => self.flash(
                Tone::Info,
                "This page runs no f1r3lang, so it has no replay log",
            ),
        }
    }

    // Disabled at the switch-over (ledger S14): the theme was a name saved in
    // `workspace.json`, and Custom the old folder's `palette.css`.
    // fn theme_action(&mut self, choice: &str) {
    //     let has_custom = theme::custom_palette(&self.eng.dir).is_ok();
    //     if choice == "template" {
    //         let path = self.eng.dir.join("palette.css");
    //         if !path.exists() {
    //             let body: String = theme::palette("dark", None)
    //                 .iter()
    //                 .map(|(k, v)| format!("{k}: {v};\n"))
    //                 .collect();
    //             match gaze_fs::write_atomic(&StdFs, &path, body.as_bytes(), Perm::Private) {
    //                 Ok(()) => {
    //                     self.flash(Tone::Ok, format!("Palette created at {}", path.display()))
    //                 }
    //                 Err(e) => self.flash(Tone::Err, format!("Could not create the palette: {e}")),
    //             }
    //         }
    //     }
    //     let chosen = match choice {
    //         "next" => match self.ui.theme.as_str() {
    //             "dark" => "light",
    //             "light" if has_custom => "custom",
    //             _ => "dark",
    //         },
    //         other => other,
    //     };
    //     match (chosen, theme::custom_palette(&self.eng.dir)) {
    //         ("dark" | "light", _) | ("custom", Ok(_)) => {
    //             self.ui.theme = chosen.into();
    //             self.apply_theme();
    //         }
    //         ("custom", Err(_)) => self.flash(
    //             Tone::Warn,
    //             "The custom palette is missing or invalid; see Appearance",
    //         ),
    //         _ => {}
    //     }
    // }

    // Disabled in step 10 (ledger S12, part 3): the step-9 strings `dark`,
    // `light`, `custom`, `template` and `next` became `ThemeOp`, which parses
    // strictly; Custom became a row of the Themes list (`theme_op`).
//     /// The Appearance panel's segments and buttons, whose markup is
//     /// unchanged: Dark and Light choose the built-in schemes, Custom (and
//     /// "Reload palette") the theme file `themes/custom.css`, "Create palette
//     /// file" writes that file once, and `next` steps through them.
//     fn theme_action(&mut self, choice: &str) {
//         let profile = &self.eng.profile;
//         let dirs = profile.theme_dirs();
//         let custom_usable = matches!(
//             theme::find_theme(profile.fs(), &dirs, CUSTOM_THEME),
//             Some(ThemeFile { colours: Ok(_), .. })
//         );
//         if choice == "template" {
//             self.create_custom_theme();
//             return;
//         }
//         let custom = ThemeChoice::Named(CUSTOM_THEME.into());
//         let chosen = match choice {
//             "dark" => ThemeChoice::BuiltIn(Scheme::Dark),
//             "light" => ThemeChoice::BuiltIn(Scheme::Light),
//             "custom" => custom.clone(),
//             "next" => match profile.theme() {
//                 ThemeChoice::BuiltIn(Scheme::Dark) => ThemeChoice::BuiltIn(Scheme::Light),
//                 ThemeChoice::BuiltIn(Scheme::Light) if custom_usable => custom.clone(),
//                 _ => ThemeChoice::BuiltIn(Scheme::Dark),
//             },
//             _ => return,
//         };
//         if chosen == custom && !custom_usable {
//             self.flash(Tone::Warn, "The custom palette is missing or invalid; see Appearance");
//             return;
//         }
//         self.choose_theme(chosen);
//     }

    /// The Appearance panel's controls.
    fn theme_op(&mut self, op: ThemeOp, now: Instant) {
        match op {
            ThemeOp::System => {
                self.choose_theme(ThemeChoice::System);
                // The preference may have changed unseen: Linux asks the
                // portal again (at most every REQUERY_INTERVAL), and macOS
                // reported nothing while the window had a theme of its own.
                self.system.refresh(now);
                self.request(WindowRequest::FollowSystem);
            }
            ThemeOp::Dark => self.choose_theme(ThemeChoice::BuiltIn(Scheme::Dark)),
            ThemeOp::Light => self.choose_theme(ThemeChoice::BuiltIn(Scheme::Light)),
            ThemeOp::Use(name) => self.use_theme(name),
            ThemeOp::New => self.new_theme(),
            ThemeOp::Reload => self.reload_themes(),
        }
    }

    /// Chooses a theme file from the list; one that cannot be used is not
    /// chosen, and the status bar says why.
    fn use_theme(&mut self, name: String) {
        let eng = Rc::clone(&self.eng);
        let profile = &eng.profile;
        // Read again at the next render: the file may have changed.
        self.themes = None;
        match theme::find_theme(profile.fs(), &profile.theme_dirs(), &name) {
            Some(ThemeFile { colours: Ok(_), .. }) => self.choose_theme(ThemeChoice::Named(name)),
            Some(ThemeFile { colours: Err(why), .. }) => {
                self.flash(Tone::Warn, format!("“{name}” cannot be used: {why}"))
            }
            None => {
                let folder = profile.theme_dirs().user;
                self.flash(
                    Tone::Warn,
                    format!("“{name}” cannot be used: there is no {name}.css in {}", folder.display()),
                );
            }
        }
    }

    /// Chooses a theme: for this window at once, and in `settings.toml`
    /// (comments kept) when the session may write it.
    fn choose_theme(&mut self, choice: ThemeChoice) {
        let saved = self.eng.profile.set_theme(choice);
        self.apply_theme();
        if let Err(why) = saved {
            self.flash(Tone::Warn, format!("The theme applies to this session only: {why}"));
        }
    }

    // Disabled in step 10 (ledger S12, part 3): "Create palette file" wrote
    // the dark scheme's colours into `themes/custom.css` once. New theme
    // (`new_theme`) writes the colours shown under a new name, and chooses it.
//     /// "Create palette file": `themes/custom.css` with the dark scheme's
//     /// colours, written only if no file has that name (never replacing one).
//     fn create_custom_theme(&mut self) {
//         let profile = &self.eng.profile;
//         if let Some(why) = profile.read_only_reason() {
//             self.flash(Tone::Err, format!("The palette file is not created: {why}"));
//             return;
//         }
//         let dir = profile.theme_dirs().user;
//         let path = dir.join(format!("{CUSTOM_THEME}.css"));
//         let css = theme::new_theme_css(CUSTOM_THEME, &theme::builtin_palette(Scheme::Dark));
//         let written = gaze_fs::create_dir_durably(profile.fs(), &dir)
//             .and_then(|_| gaze_fs::write_new(profile.fs(), &path, css.as_bytes(), Perm::Private));
//         match written {
//             Ok(()) => self.flash(Tone::Ok, format!("Palette created at {}", path.display())),
//             Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
//             Err(e) => self.flash(Tone::Err, format!("Could not create the palette: {e}")),
//         }
//     }

    /// New theme: the colours shown, as `themes/<name>.css` (`my-theme`,
    /// `my-theme-2`, …; `theme::new_theme_name`), written only where nothing
    /// is, then chosen, so that it can be edited and reloaded.
    fn new_theme(&mut self) {
        let eng = Rc::clone(&self.eng);
        let profile = &eng.profile;
        if let Some(why) = profile.read_only_reason() {
            self.flash(Tone::Err, format!("Could not create the theme: {why}"));
            return;
        }
        let (fs, dirs) = (profile.fs(), profile.theme_dirs());
        let mut taken = theme::list_themes(fs, &dirs);
        for _ in 0..NEW_THEME_TRIES {
            let name = theme::new_theme_name(&taken);
            let path = dirs.user.join(format!("{name}.css"));
            let css = theme::new_theme_css(&name, &self.resolved.colours);
            let made = gaze_fs::create_dir_durably(fs, &dirs.user)
                .and_then(|_| gaze_fs::write_new(fs, &path, css.as_bytes(), Perm::Private));
            match made {
                Ok(()) => {
                    self.themes = Some(theme::list_themes(fs, &dirs));
                    self.flash(Tone::Ok, format!("Theme created at {}", path.display()));
                    // A setting that cannot be saved says so instead
                    // (`choose_theme`).
                    self.choose_theme(ThemeChoice::Named(name));
                    return;
                }
                // Something the list does not show has that name: the next
                // name is tried.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => taken.push(ThemeFile {
                    name,
                    path,
                    packaged: false,
                    colours: Err(String::new()),
                }),
                Err(e) => {
                    self.flash(Tone::Err, format!("Could not create the theme: {e}"));
                    return;
                }
            }
        }
        self.flash(Tone::Err, "Could not create the theme: every name tried is taken");
    }

    /// Reload: the theme files, and the chosen theme, read again after
    /// editing.
    fn reload_themes(&mut self) {
        let eng = Rc::clone(&self.eng);
        let profile = &eng.profile;
        self.themes = Some(theme::list_themes(profile.fs(), &profile.theme_dirs()));
        self.apply_theme();
        match (profile.theme(), self.resolved.problem.clone()) {
            (ThemeChoice::Named(name), Some(why)) => {
                self.flash(Tone::Warn, format!("“{name}” cannot be used: {why}"))
            }
            _ => self.flash(Tone::Ok, "Themes reloaded"),
        }
    }

    fn history_op(&mut self, verb: &str) {
        match verb {
            "clear-ask" => self.history_confirm = true,
            "clear-cancel" => self.history_confirm = false,
            "clear" => {
                self.history.visits.clear();
                self.history_confirm = false;
                self.history_changed(true);
                // "Clear history" means all of it: the copies start-up kept
                // of damaged history files go too.
                let cleared = match self.eng.profile.read_only_reason() {
                    Some(why) => Err(format!("in this window only: {why}")),
                    None => self.eng.profile.forget_history_backups().map(drop),
                };
                match cleared {
                    Ok(()) => self.flash(Tone::Ok, "History cleared"),
                    Err(e) => self.flash(Tone::Warn, format!("History cleared {e}")),
                }
            }
            "more" => self.history_limit += HISTORY_PAGE,
            // Opening and removing visits by index were replaced by the
            // address-keyed `visit:open` and `visit:forget`: indexes shift
            // whenever a page finishes loading and is recorded. (They took
            // the visit's index from the action's argument.)
            // "open" => {
            //     if let Some(url) = self.ui.visits.get(index).map(|v| v.url.clone()) {
            //         self.open_tab(&url);
            //     }
            // }
            // "remove" => {
            //     if index < self.ui.visits.len() {
            //         self.ui.visits.remove(index);
            //         self.state_dirty = true;
            //     }
            // }
            _ => {}
        }
    }

    fn site_op(&mut self, verb: &str, site: &str) {
        let result = match verb {
            "store" => self.eng.clear_store(site),
            "cache" => self.eng.blobs.cache.clear(),
            "grants" => gaze_broker::Site::of_url(site)
                .ok_or_else(|| "invalid site".to_string())
                .and_then(|s| self.eng.broker.borrow_mut().forget(&s)),
            // A session belongs to the tab whose page opened it.
            "session" => match session_target(site) {
                Some((tab_id, label)) => {
                    if let Some(core) = self
                        .tabs
                        .iter()
                        .find(|(t, _)| t.id == tab_id)
                        .and_then(|(t, _)| t.core.as_ref())
                        && let Some(shard) = &mut core.borrow_mut().shard
                    {
                        shard.close_session(label);
                    }
                    Ok(())
                }
                None => Err("malformed session reference".into()),
            },
            _ => Err("unknown Site Data action".into()),
        };
        match result {
            Ok(()) => self.flash(Tone::Ok, "Site data updated"),
            Err(e) => self.flash(Tone::Err, format!("Site data: {e}")),
        }
    }

    fn say(&self, tone: Tone, msg: impl Into<String>) {
        if let Ok(mut v) = self.wallet.lock() {
            v.notice = Some((tone, msg.into()));
        }
    }

    fn refresh_balances(&self) {
        if self.eng.wallets.embers.is_none() {
            return;
        }
        let (w, view, wake) = (
            self.eng.wallets.clone(),
            self.wallet.clone(),
            self.wake.clone(),
        );
        self.eng.pool.spawn(move || {
            for (e, _) in w.list() {
                let b = match w.state(&e.address) {
                    Ok(s) => s.balance.to_string(),
                    Err(_) => "?".into(),
                };
                if let Ok(mut v) = view.lock() {
                    v.balances.insert(e.address.as_str().to_string(), b);
                }
            }
            wake.wake();
        });
    }

    fn wallet_action(&mut self, w: &str) {
        use gaze_wallet::Address;
        let (verb, arg) = w.split_once(':').unwrap_or((w, ""));
        let wallets = self.eng.wallets.clone();
        match verb {
            "new" => match wallets.create("") {
                Ok(a) => self.say(
                    Tone::Ok,
                    format!(
                        "Created {}. Export it (and keep the file safe) before funding it.",
                        display::abbreviate(a.as_str(), 8, 6)
                    ),
                ),
                Err(e) => self.say(Tone::Err, e),
            },
            "import" => {
                let text = self.input_value("wallet-import");
                match wallets.import(&text, "imported") {
                    Ok(a) => {
                        self.clear_input("wallet-import");
                        self.say(
                            Tone::Ok,
                            format!("Imported {}.", display::abbreviate(a.as_str(), 8, 6)),
                        );
                    }
                    Err(e) => self.say(Tone::Err, format!("Could not import: {e}")),
                }
            }
            "use" => match Address::parse(arg).and_then(|a| wallets.set_active(&a)) {
                Ok(()) => self.say(Tone::Ok, "This wallet now pays for deploys."),
                Err(e) => self.say(Tone::Err, e),
            },
            "copy" => match self.inner.shell_provider.set_clipboard_text(arg.to_string()) {
                Ok(()) => self.flash(Tone::Ok, "Wallet address copied"),
                Err(_) => self.flash(Tone::Warn, "The clipboard is not available"),
            },
            "export" => {
                let r = Address::parse(arg).and_then(|a| {
                    let profile = &self.eng.profile;
                    if let Some(why) = profile.read_only_reason() {
                        return Err(format!("the wallet file is not written: {why}"));
                    }
                    let body = wallets.export(&a)?;
                    // Was `<profile>/exports/`, in the single profile folder.
                    let dir = profile.layout.exports_dir();
                    gaze_fs::create_dir_durably(profile.fs(), &dir).map_err(|e| e.to_string())?;
                    let p = dir.join(format!("{a}.json"));
                    // A wallet file is a private key: owner-only from the start.
                    gaze_fs::write_atomic(profile.fs(), &p, body.as_bytes(), Perm::Private)
                        .map_err(|e| e.to_string())?;
                    Ok(p)
                });
                match r {
                    Ok(p) => self.say(
                        Tone::Ok,
                        format!(
                            "Wallet file written to {} (F1R3Sky can import it).",
                            p.display()
                        ),
                    ),
                    Err(e) => self.say(Tone::Err, e),
                }
            }
            "remove" => {
                let label = wallets
                    .list()
                    .into_iter()
                    .find(|(e, _)| e.address.as_str() == arg)
                    .map(|(e, _)| e.label)
                    .unwrap_or_default();
                if let Ok(mut v) = self.wallet.lock() {
                    v.confirm = Some(WalletConfirm::Remove {
                        address: arg.to_string(),
                        label,
                    });
                }
            }
            "send" => {
                let to = self.input_value("wallet-to");
                let amount = self.input_value("wallet-amount");
                let note = self.input_value("wallet-desc");
                let parsed = Address::parse(&to).and_then(|t| {
                    let n: i64 = amount
                        .parse()
                        .map_err(|_| "the amount must be a whole number".to_string())?;
                    if n <= 0 {
                        return Err("the amount must be positive".into());
                    }
                    Ok((t, n))
                });
                match (parsed, wallets.active()) {
                    (Ok((t, n)), Some(from)) => {
                        if let Ok(mut v) = self.wallet.lock() {
                            v.notice = None;
                            v.confirm = Some(WalletConfirm::Send {
                                from: from.as_str().to_string(),
                                to: t.as_str().to_string(),
                                amount: n,
                                note: Some(note).filter(|d| !d.is_empty()),
                            });
                        }
                    }
                    (Err(e), _) => self.say(Tone::Err, e),
                    (_, None) => self.say(Tone::Err, "No wallet pays for deploys yet."),
                }
            }
            "confirm" => {
                let pending = self.wallet.lock().ok().and_then(|mut v| v.confirm.take());
                match pending {
                    Some(WalletConfirm::Remove { address, .. }) => {
                        match Address::parse(&address).and_then(|a| wallets.remove(&a)) {
                            Ok(()) => self.say(Tone::Ok, "Removed."),
                            Err(e) => self.say(Tone::Err, e),
                        }
                    }
                    Some(WalletConfirm::Send {
                        from,
                        to,
                        amount,
                        note,
                    }) => match wallets.active() {
                        // The reviewed transfer names its paying wallet; it
                        // is sent only if that wallet still pays.
                        Some(active) if active.as_str() == from => {
                            self.say(
                                Tone::Info,
                                "Sending: checking the prepared contract, then signing…",
                            );
                            let (view, wake) = (self.wallet.clone(), self.wake.clone());
                            self.eng.pool.spawn(move || {
                                let r = Address::parse(&to).and_then(|t| {
                                    wallets.transfer(&active, &t, amount, note.as_deref())
                                });
                                if let Ok(mut v) = view.lock() {
                                    v.notice = Some(match r {
                                        Ok(id) => (
                                            Tone::Ok,
                                            format!(
                                                "Sent. Deploy {}.",
                                                gaze_shard::bridge::short(&id)
                                            ),
                                        ),
                                        Err(e) => (Tone::Err, format!("Not sent: {e}")),
                                    });
                                }
                                wake.wake();
                            });
                            self.clear_input("wallet-amount");
                        }
                        _ => self.say(
                            Tone::Warn,
                            "The paying wallet changed since the review; review the transfer again.",
                        ),
                    },
                    None => {}
                }
                self.refresh_balances();
            }
            "cancel" => {
                if let Ok(mut v) = self.wallet.lock() {
                    v.confirm = None;
                }
            }
            "dismiss" => {
                if let Ok(mut v) = self.wallet.lock() {
                    v.notice = None;
                }
            }
            _ => {}
        }
    }

    // ── Panels: gather plain data, then build ──

    fn render_tab_panel(&mut self) -> PanelView {
        let query = self.side_query.trim().to_lowercase();
        let total = self.tabs.len();
        let active_id = self.tabs[self.active].0.id;
        let with_children: HashSet<u64> = self.parents.values().flatten().copied().collect();
        let mut rows = Vec::with_capacity(total);
        for (index, (tab, _)) in self.tabs.iter().enumerate() {
            let title = if tab.title.is_empty() {
                tab.url.as_str()
            } else {
                tab.title.as_str()
            };
            let page_matches = self.side_results.get(&tab.id).copied().unwrap_or(0);
            if search_score(&query, title, &tab.url).is_none() && page_matches == 0 {
                continue;
            }
            let depth = match (self.session.tree_tabs, query.is_empty()) {
                (true, true) => self.tree_depth(tab.id),
                _ => 0,
            };
            let attention = match self.lazy.contains_key(&tab.id) {
                // Restored tabs load when selected; until then they are idle.
                true => TabAttention::Idle,
                false => TabAttention::of(&tab.stage, !tab.prompts().is_empty()),
            };
            rows.push(TabRow {
                id: tab.id,
                title,
                url: &tab.url,
                depth,
                active: tab.id == active_id,
                attention,
                page_matches,
                menu_open: self.menu_tab == Some(tab.id),
                has_children: with_children.contains(&tab.id),
                first: index == 0,
                last: index + 1 == total,
            });
        }
        let closed: Vec<ClosedRow<'_>> = self
            .closed
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, t)| search_score(&query, &t.title, &t.url).is_some())
            .take(CLOSED_SHOWN)
            .map(|(index, t)| ClosedRow {
                index,
                title: &t.title,
                url: &t.url,
            })
            .collect();
        tab_panel_html(&mut self.fitter, &rows, &closed, &query, total)
    }

    fn render_history_panel(&mut self) -> PanelView {
        history_panel_html(
            &mut self.fitter,
            &self.history.visits,
            &self.side_query,
            display::unix_now(),
            self.history_limit,
            self.history_confirm,
        )
    }

    fn render_site_panel(&mut self) -> PanelView {
        let stores = self.eng.store_sites();
        let broker = self.eng.broker.borrow();
        let mut names: BTreeSet<String> = stores.iter().cloned().collect();
        for grant in broker.remembered_all() {
            names.insert(grant.site.as_str().to_string());
        }
        for (tab, _) in &self.tabs {
            if let Some(site) = gaze_broker::Site::of_url(&tab.url) {
                names.insert(site.as_str().to_string());
            }
        }
        let mut sites = Vec::with_capacity(names.len());
        for site in names {
            let remembered = broker
                .remembered_all()
                .iter()
                .filter(|g| g.site.as_str() == site)
                .count();
            let has_store = stores.contains(&site);
            let stored = match has_store {
                true => gaze_broker::Site::of_url(&site)
                    .and_then(|s| self.eng.store_for(&s).ok())
                    .map(|s| s.borrow().used())
                    .unwrap_or(0),
                false => 0,
            };
            let sessions: Vec<SessionRow> = self
                .tabs
                .iter()
                .filter_map(|(t, _)| {
                    let core = t.core.as_ref()?;
                    let core = core.borrow();
                    (core.site.as_str() == site).then(|| {
                        core.shard
                            .as_ref()
                            .map(|s| s.list_sessions())
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(label, uri)| SessionRow {
                                tab: t.id,
                                label,
                                uri,
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .flatten()
                .collect();
            // Only sites that keep something are listed.
            if stored > 0 || remembered > 0 || !sessions.is_empty() {
                sites.push(SiteUsage {
                    site,
                    has_store,
                    stored,
                    remembered,
                    sessions,
                });
            }
        }
        drop(broker);
        let stats = self.eng.blobs.cache.stats();
        site_data_html(&mut self.fitter, &sites, &stats)
    }

    fn render_permissions_panel(&mut self) -> PanelView {
        let view_node = self.tabs[self.active].1;
        let grants = rho_mut(&mut self.inner, view_node)
            .map(|r| r.grants())
            .unwrap_or_default();
        let tab = &self.tabs[self.active].0;
        let site = gaze_broker::Site::of_url(&tab.url);
        let broker = self.eng.broker.borrow();
        let mut remembered: Vec<RememberedChoice> = Vec::new();
        if let Some(site) = &site {
            for stored in broker.remembered_all().iter().filter(|s| &s.site == site) {
                if !remembered
                    .iter()
                    .any(|r| r.urn == stored.urn && r.allow == stored.allow)
                {
                    remembered.push(RememberedChoice {
                        urn: stored.urn.clone(),
                        allow: stored.allow,
                        classes: stored.classes.iter().map(|c| c.name()).collect(),
                    });
                }
            }
        }
        drop(broker);
        let view = PermissionsView {
            site: site.map(|s| s.as_str().to_string()),
            stage: tab.stage.clone(),
            grants,
            remembered,
        };
        permissions_html(&mut self.fitter, &view)
    }

    // Disabled in step 10 (ledger S12, part 3): it lit Dark, Light or Custom
    // and showed the status of `themes/custom.css`.
//     fn render_appearance_panel(&mut self) -> PanelView {
//         // Was the old folder's `palette.css`, read and laid over the chosen
//         // scheme here as well (ledger L14).
//         let profile = &self.eng.profile;
//         let dirs = profile.theme_dirs();
//         let path = dirs.user.join(format!("{CUSTOM_THEME}.css"));
//         let status = match theme::find_theme(profile.fs(), &dirs, CUSTOM_THEME) {
//             None => PaletteStatus::Missing,
//             Some(ThemeFile { colours: Ok(_), .. }) => PaletteStatus::Valid,
//             Some(ThemeFile { colours: Err(why), .. }) => PaletteStatus::Invalid(why),
//         };
//         // The segment lit: Dark, Light or Custom; none for System or another
//         // theme file.
//         let choice = profile.theme();
//         let lit = match &choice {
//             ThemeChoice::BuiltIn(Scheme::Dark) => "dark",
//             ThemeChoice::BuiltIn(Scheme::Light) => "light",
//             ThemeChoice::Named(name) if name == CUSTOM_THEME => "custom",
//             other => other.as_str(),
//         };
//         appearance_html(
//             &mut self.fitter,
//             lit,
//             &self.resolved.colours,
//             &path.display().to_string(),
//             &status,
//         )
//     }

    fn render_appearance_panel(&mut self) -> PanelView {
        let eng = Rc::clone(&self.eng);
        let profile = &eng.profile;
        let dirs = profile.theme_dirs();
        let themes = self.themes.get_or_insert_with(|| theme::list_themes(profile.fs(), &dirs));
        let (choice, known, folder) = (profile.theme(), self.system.known(), dirs.user.display().to_string());
        let view = AppearanceView {
            choice: &choice,
            system: &known,
            colours: &self.resolved.colours,
            shown: self.resolved.scheme,
            problem: self.resolved.problem.as_deref(),
            themes,
            folder: &folder,
        };
        appearance_html(&mut self.fitter, &view)
    }

    /// The wallet section: dynamic parts and the state of the static form.
    fn render_wallet(&mut self) -> bool {
        if self.panel != "wallet" || !self.session.sidebar_open {
            return false;
        }
        let (balances, notice, confirm) = match self.wallet.lock() {
            Ok(v) => (v.balances.clone(), v.notice.clone(), v.confirm.clone()),
            Err(_) => return false,
        };
        let embers = self.eng.wallets.embers.is_some();
        let listed = self.eng.wallets.list();
        let mut rows: Vec<(WalletRow, bool)> = Vec::with_capacity(listed.len());
        for (entry, active) in listed {
            let address = entry.address.as_str().to_string();
            rows.push((
                WalletRow {
                    balance: balances.get(&address).cloned(),
                    address,
                    label: entry.label,
                },
                active,
            ));
        }
        let labels: BTreeMap<String, String> = rows
            .iter()
            .map(|(row, _)| (row.address.clone(), row.label.clone()))
            .collect();
        let active = rows.iter().find(|(_, a)| *a).map(|(row, _)| row);
        let others: Vec<WalletRow> = rows
            .iter()
            .filter(|(_, a)| !*a)
            .map(|(row, _)| row.clone())
            .collect();
        let has_active = active.is_some();
        let card = wallet_card_html(&mut self.fitter, active, rows.len(), embers);
        let list = wallet_list_html(&mut self.fitter, &others, embers);
        let message =
            wallet_message_html(&mut self.fitter, notice.as_ref(), confirm.as_ref(), &labels);
        let mut changed = self.set_html("walletcard", card);
        changed |= self.set_html("walletlist", list);
        changed |= self.set_html("walletmsg", message);
        changed |= self.sync_attr(
            "wallet-send-form",
            "class",
            rows.is_empty().then_some("hidden"),
        );
        changed |= self.sync_attr(
            "wallet-send",
            "disabled",
            (!has_active || !embers).then_some(""),
        );
        changed
    }

    fn render_find_overlay(&mut self) -> bool {
        let mut html = String::new();
        if self.find_visible
            && !self.find_hits.is_empty()
            && let Some(r) = rho_mut(&mut self.inner, self.tabs[self.active].1)
        {
            html.reserve(128 * self.find_hits.len());
            for (i, hit) in self.find_hits.iter().enumerate() {
                for rect in r.find_rects(hit) {
                    if rect.width <= 0.0 || rect.height <= 0.0 {
                        continue;
                    }
                    put!(
                        html,
                        r#"<div class="findhit{}" style="left:{:.2}px;top:{:.2}px;width:{:.2}px;height:{:.2}px"></div>"#,
                        if i == self.find_index { " active" } else { "" },
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height
                    );
                }
            }
        }
        self.set_html("findoverlay", html)
    }

    fn scan_sidebar_pages(&mut self, page_changed: bool) {
        if !self.session.sidebar_open
            || self.panel != "tabs"
            || !self.side_pages
            || self.side_query.trim().is_empty()
        {
            return;
        }
        if page_changed
            && self.side_scan_cursor == self.tabs.len()
            && self.side_scan_at.elapsed() >= Duration::from_secs(1)
        {
            self.side_scan_cursor = 0;
        }
        let pending = self.side_scan_cursor < self.tabs.len();
        let query = self.side_query.trim().to_string();
        for _ in 0..4 {
            let Some((tab, view)) = self.tabs.get(self.side_scan_cursor) else {
                break;
            };
            let (id, view) = (tab.id, *view);
            let count = rho_mut(&mut self.inner, view)
                .map(|r| r.find(&query, 200).len())
                .unwrap_or(0);
            self.side_results.insert(id, count);
            self.side_scan_cursor += 1;
        }
        if self.side_scan_cursor < self.tabs.len() {
            self.wake.wake();
        } else if pending {
            self.side_scan_at = Instant::now();
        }
    }

    fn set_html(&mut self, id: &'static str, html: String) -> bool {
        if self.rendered.get(id) == Some(&html) {
            return false;
        }
        if let Some(n) = self.id(id) {
            let mut m = self.inner.mutate();
            m.set_inner_html(n, &html);
        }
        self.rendered.insert(id, html);
        true
    }

    // ── Rendering ──

    /// Show or hide a `display:none` region by class. Returns whether it
    /// just became visible: content first inserted beneath a hidden ancestor
    /// is not laid out by Blitz, so its dynamic regions must be re-inserted.
    fn show_region(&mut self, id: &str, shown: bool) -> bool {
        let became_visible = shown
            && self
                .id(id)
                .and_then(|n| attr_of(&self.inner, n, "class"))
                .is_some_and(|c| c == "hidden");
        self.sync_attr(id, "class", (!shown).then_some("hidden"));
        became_visible
    }

    fn render_visibility(&mut self) -> bool {
        let sidebar = self.session.sidebar_open;
        let wallet = sidebar && self.panel == "wallet";
        let mut changed = false;
        if self.show_region("sidebar", sidebar) {
            for region in [
                "sidehead",
                "sidecontrols",
                "panel",
                "walletcard",
                "walletmsg",
                "walletlist",
            ] {
                self.rendered.remove(region);
            }
            changed = true;
        }
        // The panel area is hidden for the wallet before its content is set,
        // so the wallet section fills the sidebar from the top.
        if self.show_region("panel", sidebar && !wallet) {
            self.rendered.remove("panel");
            changed = true;
        }
        if self.show_region("wallet", wallet) {
            for region in ["walletcard", "walletmsg", "walletlist"] {
                self.rendered.remove(region);
            }
            changed = true;
        }
        changed |= self.sync_attr(
            "findbar",
            "class",
            (!self.find_visible).then_some("off"),
        );
        changed
    }

    fn render_toolbar_state(&mut self) -> bool {
        let mut changed = false;
        for (panel, rail_id, _) in PANELS {
            let on = self.session.sidebar_open && self.panel == panel;
            changed |= self.sync_attr(
                rail_id,
                "class",
                Some(if on { "rail-btn on" } else { "rail-btn" }),
            );
        }
        changed |= self.sync_attr(
            "sidetoggle",
            "class",
            Some(if self.session.sidebar_open {
                "icon-btn on"
            } else {
                "icon-btn"
            }),
        );
        let (back, forward) = {
            let tab = &self.tabs[self.active].0;
            (tab.can_go_back(), tab.can_go_forward())
        };
        changed |= self.sync_attr("back", "disabled", (!back).then_some(""));
        changed |= self.sync_attr("fwd", "disabled", (!forward).then_some(""));
        changed
    }

    fn render_address(&mut self) -> bool {
        let mut changed = false;
        let typing = self.focused("url") && !self.address_query.is_empty();
        // The page's address replaces the field's content unless the user is
        // typing in it.
        if self.url_dirty && !typing {
            self.url_dirty = false;
            let address = display_address(&self.tabs[self.active].0.url).to_string();
            if self.raw_input_value("url") != address {
                self.set_input_value("url", &address);
                changed = true;
            }
        }
        let focus = self.focused("url");
        changed |= self.sync_attr("urlwrap", "class", focus.then_some("focus"));
        for (field, hint) in [
            ("url", "url-ph"),
            ("find", "find-ph"),
            ("side-search", "side-ph"),
        ] {
            let empty = self.raw_input_value(field).is_empty();
            changed |= self.sync_attr(
                hint,
                "class",
                Some(if empty { "ghost-hint" } else { "ghost-hint off" }),
            );
        }
        changed
    }

    fn render_tab_strip(&mut self) -> bool {
        let width = self.window_width();
        let chips: Vec<TabChip<'_>> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(j, (tab, _))| TabChip {
                id: tab.id,
                title: &tab.title,
                url: &tab.url,
                attention: match self.lazy.contains_key(&tab.id) {
                    true => TabAttention::Idle,
                    false => TabAttention::of(&tab.stage, !tab.prompts().is_empty()),
                },
                active: j == self.active,
            })
            .collect();
        let html = tab_strip_html(&mut self.fitter, &chips, width);
        self.set_html("tablist", html)
    }

    fn render_suggestions(&mut self) -> bool {
        let focused = self.focused("url");
        let typing = focused && !self.address_query.trim().is_empty();
        // Once the box holds anything but the page's address (an empty box
        // included), Enter searches or opens what is typed: the badge shows
        // a magnifier instead of the page's identity.
        let page = &self.tabs[self.active].0.url;
        let address = display_address(page);
        let searching = focused && (address.is_empty() || self.raw_input_value("url") != address);
        let badge = scheme_badge_html(page, searching);
        let mut changed = self.set_html("scheme", badge);
        let html = match typing {
            true => {
                self.suggestions = self.build_suggestions();
                if self
                    .suggest_index
                    .is_some_and(|index| index >= self.suggestions.len())
                {
                    self.suggest_index = None;
                }
                let width = self.window_width();
                suggestions_html(
                    &mut self.fitter,
                    &self.suggestions,
                    &self.address_query,
                    self.suggest_index,
                    width,
                )
            }
            false => {
                self.suggestions.clear();
                self.suggest_index = None;
                String::new()
            }
        };
        changed |= self.set_html("suggestions", html);
        changed
    }

    fn render_prompt(&mut self) -> bool {
        let tab = &self.tabs[self.active].0;
        let prompts = tab.prompts();
        let html = match prompts.first() {
            Some((pid, text)) => {
                let site = gaze_broker::Site::of_url(&tab.url).map(|s| s.as_str().to_string());
                prompt_bar_html(
                    site.as_deref(),
                    text,
                    tab.id,
                    *pid,
                    prompts.len(),
                    self.remember,
                )
            }
            None => String::new(),
        };
        self.set_html("prompt", html)
    }

    fn render_sidebar(&mut self) -> bool {
        if !self.session.sidebar_open {
            return false;
        }
        let panel = self.panel.clone();
        let view = match panel.as_str() {
            "tabs" => self.render_tab_panel(),
            "history" => self.render_history_panel(),
            "sites" => self.render_site_panel(),
            "grants" => self.render_permissions_panel(),
            "console" => {
                let tid = self.tabs[self.active].0.id;
                console_html(self.consoles.get(&tid).map(Vec::as_slice).unwrap_or(&[]))
            }
            "appearance" => self.render_appearance_panel(),
            _ => PanelView {
                html: String::new(),
                count: None,
            },
        };
        let title = PANELS
            .iter()
            .find(|(name, _, _)| *name == panel)
            .map(|(_, _, title)| *title)
            .unwrap_or("Tabs");
        let mut changed = self.set_html("sidehead", side_head_html(title, view.count.as_deref()));
        // Re-inserted controls hold an empty field: the search they filter by
        // starts empty too (S9).
        if self.set_html("sidecontrols", side_controls_html(&panel).to_string()) {
            changed = true;
            if !self.side_query.is_empty() {
                self.reset_side_search();
            }
        }
        changed |= self.sync_attr(
            "chip-tree",
            "class",
            Some(if self.session.tree_tabs { "chip on" } else { "chip" }),
        );
        changed |= self.sync_attr(
            "chip-tree",
            "aria-pressed",
            Some(if self.session.tree_tabs { "true" } else { "false" }),
        );
        changed |= self.sync_attr(
            "chip-pages",
            "class",
            Some(if self.side_pages { "chip on" } else { "chip" }),
        );
        changed |= self.sync_attr(
            "chip-pages",
            "aria-pressed",
            Some(if self.side_pages { "true" } else { "false" }),
        );
        let panel_html = match panel.as_str() {
            "wallet" => String::new(),
            _ => view.html,
        };
        changed |= self.set_html("panel", panel_html);
        changed |= self.render_wallet();
        changed
    }

    fn render_find(&mut self) -> bool {
        let (text, none) = find_count(&self.find_query, self.find_hits.len(), self.find_index);
        let mut changed = self.set_html("findcount", text);
        changed |= self.sync_attr("findcount", "class", none.then_some("none"));
        changed |= self.render_find_overlay();
        changed
    }

    fn render_status(&mut self) -> bool {
        let width = self.window_width();
        let view = self.tabs[self.active].1;
        let stalled = match rho_mut(&mut self.inner, view) {
            Some(r) => r.stalled_frames() > 0,
            None => false,
        };
        let tab = &self.tabs[self.active].0;
        let running = tab.stage == Stage::Running;
        // Before the window has a size the bar is not shown; its text is
        // fitted again on the next render.
        let html = status_html(
            &mut self.fitter,
            if width > 0.0 { width } else { f32::MAX },
            &tab.stage,
            &tab.url,
            tab.notice.as_deref(),
            stalled && running,
            self.flash.as_ref(),
        );
        self.set_html("status", html)
    }

    fn render(&mut self) -> bool {
        let _span = frame_stats::RENDER.span();
        let scale = self.inner.viewport().scale();
        self.fitter.set_scale(scale);
        if self.flash.as_ref().is_some_and(|f| f.until <= Instant::now()) {
            self.flash = None;
        }
        let mut changed = self.render_visibility();
        changed |= self.render_toolbar_state();
        changed |= self.render_address();
        // After the address is restored, so the selection covers it.
        if let Some(id) = self.select_pending.take()
            && self.focused(id)
        {
            changed |= self.select_whole(id);
        }
        changed |= self.render_tab_strip();
        changed |= self.render_suggestions();
        changed |= self.render_prompt();
        changed |= self.render_sidebar();
        changed |= self.render_find();
        changed |= self.render_status();
        let title = &self.tabs[self.active].0.title;
        let wt = if title.is_empty() {
            "F1R3Gaze".to_string()
        } else {
            format!("{title} — F1R3Gaze")
        };
        if wt != self.title {
            self.inner.shell_provider.set_window_title(wt.clone());
            self.title = wt;
        }
        changed
    }

    /// Ask the pacer to wake the window for whatever is due next: the
    /// status message's expiry, or another look by find.
    fn schedule_wakes(&mut self) {
        let now = Instant::now();
        if let Some(flash) = &self.flash
            && flash.until > now
        {
            self.pacer.at(flash.until);
        }
        if self.find_rescan {
            self.pacer.at(now + FIND_RESCAN_DELAY);
        }
        if let Some(due) = self.history_due {
            self.pacer.at(due);
        }
    }
}

/// A scheme as Blitz's viewport names it.
fn color_scheme_of(scheme: Scheme) -> ColorScheme {
    match scheme {
        Scheme::Dark => ColorScheme::Dark,
        Scheme::Light => ColorScheme::Light,
    }
}

/// How to leave full screen, on `platform`.
fn full_screen_hint(platform: KeyPlatform) -> &'static str {
    match platform {
        KeyPlatform::Other => "Press F11 to leave full screen",
        KeyPlatform::MacOs => "Press Ctrl+Cmd+F to leave full screen",
    }
}

/// The preference the theme is resolved with: none until the system has
/// answered, which means dark (`theme::resolve`).
fn preference_of(known: &Known) -> Option<Scheme> {
    match known {
        Known::Answered(preference) => *preference,
        Known::Unknown(_) => None,
    }
}

impl Drop for ChromeDocument {
    /// The window is closing: what is still to be saved is saved.
    fn drop(&mut self) {
        self.flush_state();
    }
}

impl ChromeDocument {
    fn dispatch_ui_event(&mut self, event: UiEvent) {
        // The window owns browser shortcuts and its fields' keys even while a
        // page sub-document has focus; they are handled before Blitz routes
        // the key anywhere.
        if let UiEvent::KeyDown(k) = &event
            && !k.is_composing
        {
            let tab_ids: Vec<u64> = self.tabs.iter().map(|(t, _)| t.id).collect();
            // Was the browser's shortcuts alone (storage ledger S13, part 2):
            // the full-screen chord comes first, since on macOS it holds Cmd+F.
            // if let Some(action) =
            //     global_shortcut(&k.key, k.modifiers, &tab_ids, self.active, KeyPlatform::CURRENT)
            // {
            //     self.act(action);
            //     return;
            // }
            if let Some(action) = browser_shortcut(
                &k.key,
                k.modifiers,
                k.is_auto_repeating,
                &tab_ids,
                self.active,
                KeyPlatform::CURRENT,
            ) {
                if let Some(action) = action {
                    self.act(action);
                }
                return;
            }
            let focus = self.key_focus();
            if let Some((action, consumed)) =
                chrome_key(&k.key, k.modifiers, focus, !self.suggestions.is_empty())
            {
                self.act(action);
                if consumed {
                    return;
                }
            }
        }
        let mut actions = Vec::new();
        let url_node = self.id("url");
        {
            let handler = ChromeHandler {
                actions: &mut actions,
                url_node,
                find_node: self.id("find"),
                side_node: self.id("side-search"),
                drag: &mut self.drag,
            };
            let mut driver = EventDriver::new(&mut self.inner, handler);
            driver.handle_ui_event(event);
        }
        for a in actions {
            self.act(a);
        }
    }

    /// Bring the cursor and the pages' hover up to date (ledger L8).
    /// Returns whether the window must paint: a page's hover changed, so its
    /// `:hover` styles changed, or a page asked to be painted.
    /// The mouse left the window: nothing in it is hovered any more, until
    /// the pointer comes back (ledger L9, H8). blitz-shell forwards no event
    /// for this, so the window's application handler calls it.
    pub(crate) fn pointer_left(&mut self) {
        self.cursor.install(&mut self.inner);
        if self.cursor.pointer_left(&mut self.inner) {
            self.inner.shell_provider.request_redraw();
        }
    }

    /// The window is about to paint a frame (ledger L9, H9). Each page's
    /// hovered node is noted, for [`Self::end_paint`].
    pub(crate) fn begin_paint(&mut self) {
        self.cursor.begin_paint();
        self.paint_hovers.clear();
        self.paint_hovers.extend(
            self.tabs
                .iter()
                .map(|&(_, view)| page_hover_node(&self.inner, view)),
        );
    }

    /// The window has painted a frame. A page that asked to be painted
    /// meanwhile gets one more frame only if its hovered node changed during
    /// the frame. That change came from Blitz's `refresh_hover`, which runs
    /// after the page's layout and leaves its restyle to the next pass. The
    /// other requests a frame brings are answered by the frame itself. Above
    /// all, a page whose viewport changed with the window asks
    /// (`queue_device_changes`), but Blitz resolves each sub-document right
    /// after setting its viewport, so this frame laid the page out and painted
    /// it.
    pub(crate) fn end_paint(&mut self) {
        if self.cursor.end_paint() && self.a_page_hover_changed_in_paint() {
            self.inner.shell_provider.request_redraw();
        }
    }

    /// Lay the window out now, as a frame would, without painting (ledger L9).
    /// The window's application handler calls this after a poll of a resized
    /// window changed the chrome. Blitz hit-tests with the paint tree of the
    /// last layout, which still holds the nodes the poll replaced
    /// (`StackingContext::hoisted_content_bbox` indexes them and panics), and
    /// an input event can arrive before the frame. This is bracketed like a
    /// frame, so a page's request for its new viewport is answered by the
    /// frame that follows.
    pub(crate) fn lay_out_now(&mut self, animation_time: f64) {
        self.begin_paint();
        self.inner.resolve(animation_time);
        self.end_paint();
    }

    /// Whether a page's hovered node differs from the one noted when the
    /// window began painting.
    fn a_page_hover_changed_in_paint(&self) -> bool {
        self.tabs
            .iter()
            .zip(&self.paint_hovers)
            .any(|(&(_, view), &before)| page_hover_node(&self.inner, view) != before)
    }

    fn settle_pointer(&mut self) -> bool {
        let mut repaint = self.cursor.take_repaint();
        if self.cursor.needs_sync() {
            repaint |= self.cursor.sync(&mut self.inner);
        }
        repaint
    }
}

/// The node hovered in the page `view` hosts, if it hosts one.
fn page_hover_node(doc: &BaseDocument, view: NodeId) -> Option<NodeId> {
    doc.subdoc(view)
        .and_then(|page| page.inner().get_hover_node_id())
}

fn rho_mut(doc: &mut BaseDocument, view: NodeId) -> Option<&mut RhoDocument> {
    let d: &mut dyn Document = doc.get_node_mut(view)?.subdoc_mut()?;
    let a: &mut dyn Any = d;
    a.downcast_mut::<RhoDocument>()
}

impl Document for ChromeDocument {
    fn inner(&self) -> DocGuard<'_> {
        DocGuard::Ref(&self.inner)
    }
    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        DocGuardMut::Ref(&mut self.inner)
    }

    fn handle_ui_event(&mut self, event: UiEvent) {
        let _span = frame_stats::EVENT.span();
        // Ledger L8: the window's cursor is decided once the page under the
        // pointer has seen the event, not by Blitz before it is forwarded.
        self.cursor.install(&mut self.inner);
        self.cursor.note_pointer(&event);
        self.dispatch_ui_event(event);
        if self.settle_pointer() {
            self.inner.shell_provider.request_redraw();
        }
    }

    fn poll(&mut self, cx: Option<TaskContext>) -> bool {
        let _span = frame_stats::POLL.span();
        if let Some(cx) = &cx {
            self.wake.set_waker(cx.waker());
        }
        self.cursor.install(&mut self.inner);
        let waker = cx.as_ref().map(|c| c.waker().clone());
        let mut changed = self.inner.poll_subdocuments(waker.as_ref());
        // Ledger L1/H7: a page attached while find was open is searched once
        // it has been laid out.
        if self.find_rescan
            && rho_mut(&mut self.inner, self.tabs[self.active].1)
                .is_some_and(|page| page_is_laid_out(page))
        {
            self.refresh_find();
            changed = true;
        }
        if changed
            && self.find_visible
            && !self.find_query.is_empty()
            && self.last_find_scan.elapsed() >= Duration::from_millis(250)
        {
            self.last_find_scan = Instant::now();
            if let Some(r) = rho_mut(&mut self.inner, self.tabs[self.active].1) {
                self.find_hits = r.find(&self.find_query, FIND_LIMIT);
                self.find_index = self.find_index.min(self.find_hits.len().saturating_sub(1));
            }
            self.show_find_hit(false);
        }
        for i in 0..self.tabs.len() {
            if let Some(doc) = self.tabs[i].0.pump() {
                if !matches!(self.tabs[i].0.stage, Stage::Failed(_)) {
                    let tab = &self.tabs[i].0;
                    self.history.visit(&tab.url, &tab.title);
                    // The tab's address changed too.
                    self.session_dirty = true;
                    self.history_changed(false);
                }
                let view = self.tabs[i].1;
                self.inner.remove_sub_document(view);
                self.inner.set_sub_document(view, Box::new(doc));
                // L8: a page that loads under a resting pointer learns where
                // the pointer is.
                self.cursor.page_attached(&mut self.inner, view);
                // The page takes the scheme shown at the chrome's next layout,
                // which copies it into every page's viewport (ledger L13).
                let themed = Self::themed_as_builtin(&self.tabs[i].0);
                if let Some(r) = rho_mut(&mut self.inner, view) {
                    r.set_foreground(i == self.active);
                    if themed {
                        r.set_host_theme(&self.host_css);
                    }
                }
                if i == self.active {
                    self.url_dirty = true;
                    self.refresh_find();
                }
                changed = true;
            }
            let view = self.tabs[i].1;
            let tid = self.tabs[i].0.id;
            if let Some(r) = rho_mut(&mut self.inner, view) {
                self.tabs[i].0.pump_doc(r);
                let lines = r.take_console();
                if !lines.is_empty() {
                    let console = self.consoles.entry(tid).or_default();
                    console.extend(lines);
                    if console.len() > CONSOLE_LIMIT {
                        let excess = console.len() - CONSOLE_LIMIT;
                        console.drain(..excess);
                    }
                }
            }
            for n in self.tabs[i].0.take_nav() {
                match n {
                    // "Try again" on an error page reloads it instead of
                    // adding the same address to history again.
                    NavRequest::Go(u)
                        if self.tabs[i].0.error_page && u == self.tabs[i].0.url =>
                    {
                        self.tabs[i].0.reload()
                    }
                    NavRequest::Go(u) => self.tabs[i].0.navigate(&u, true),
                    NavRequest::Replace(u) => self.tabs[i].0.navigate(&u, false),
                    NavRequest::Back => {
                        self.tabs[i].0.back();
                    }
                    NavRequest::Open(u) => self.open_tab(&u),
                }
                if i == self.active {
                    self.url_dirty = true;
                }
                self.session_dirty = true;
            }
        }
        self.scan_sidebar_pages(changed);
        // A new answer from the system, and notices still to show.
        changed |= self.follow_system();
        let now = Instant::now();
        self.take_notices();
        changed |= self.show_next_notice(now);
        changed |= self.render();
        // L8: hover changes reported during layout (Blitz's refresh_hover),
        // pages loaded under the pointer, and pages' redraw requests.
        changed |= self.settle_pointer();
        // Was `save_workspace()`.
        self.save_state(now);
        self.schedule_wakes();
        changed
    }
}

/// Open the browser window and run until it closes.
pub fn launch(eng: Rc<Engine>, url: &str) -> Result<(), String> {
    // The system's preference is asked for first, so the portal's answer
    // (Linux) comes while the window is being made; start-up waits for it at
    // most `STARTUP_WAIT`. macOS and Windows report it through the window.
    let wake = WakeHandle::default();
    let window_reports = cfg!(any(target_os = "macos", windows));
    let override_value = std::env::var(OVERRIDE_VAR).ok();
    let source = match Source::choose(override_value.as_deref(), window_reports) {
        Ok(source) => source,
        Err(why) => {
            eng.profile.report.push(Event::new(
                EventKind::Other,
                Severity::Warning,
                std::path::PathBuf::new(),
                format!("{why}; the system's preference is read as usual"),
            ));
            Source::choose(None, window_reports).expect("without an override every platform has a source")
        }
    };
    let waker = wake.clone();
    let system = SystemScheme::start(&source, Some(Arc::new(move || waker.wake())), Instant::now());
    system.wait(STARTUP_WAIT);
    // Built here rather than by `create_default_event_loop`, which panics
    // when there is no display; the user gets a message instead.
    let event_loop = EventLoop::builder().build().map_err(|e| {
        format!("cannot open a window ({e}); use --headless to run pages without one")
    })?;
    event_loop.set_control_flow(ControlFlow::Wait);
    #[cfg(target_os = "macos")]
    let _url_delegate = crate::macos_url::install(event_loop.create_proxy())?;
    let (proxy, rx) = BlitzShellProxy::new(event_loop.create_proxy());
    let app = BlitzApplication::new(proxy, rx);
    // `window.json` as start-up left it, and how to write it: the chrome,
    // which holds the profile, goes with the window when it closes.
    let saved: WindowState = eng.profile.read_state();
    let keeps = Rc::clone(&eng);
    let chrome = ChromeDocument::with_system_scheme(eng, url, system, source, wake);
    // Disabled in step 11 (storage ledger S13, part 2): the window was made
    // with default attributes before the monitors could be listed. It is made
    // once the event loop can list them, where it was left
    // (`ChromeApplication::create_window`).
    // app.add_window(WindowConfig::new(
    //     Box::new(chrome),
    //     // Resizes are coalesced into the next frame (ledger L9).
    //     CoalescingRenderer::new(VelloWindowRenderer::new()),
    // ));
    let window = PendingWindow {
        chrome,
        // Resizes are coalesced into the next frame (ledger L9).
        renderer: CoalescingRenderer::new(VelloWindowRenderer::new()),
        saved,
        save: Box::new(move |state: &WindowState| keeps.profile.write_state(state)),
    };
    // A resize is painted once, with the chrome already fitted to it (L9, H7).
    event_loop
        .run_app(ChromeApplication::new(app, window))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
