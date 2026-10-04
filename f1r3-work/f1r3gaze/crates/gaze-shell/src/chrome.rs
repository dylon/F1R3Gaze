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
//!   [`ChromeDocument::sync_attr`], so a field being typed into is never
//!   rebuilt.
//! * Dynamic regions are rebuilt as HTML strings from plain data by the free
//!   `*_html` builders, and inserted only when their markup changed
//!   ([`ChromeDocument::set_html`]).
//! * Labels are shortened to the exact width they are given
//!   ([`TextFitter`]), because Blitz has no `text-overflow`.
//! * Labels are elements, never bare text in a flex box: Blitz never
//!   restyles the anonymous box such text gets (`docs/ui/ledger.md`, L2).

use crate::display::{self, LogLevel, Recency, SchemeKind, Tone};
use crate::engine::{Engine, NavRequest, escape};
use crate::tab::{Stage, Tab};
use crate::text_fit::{ELLIPSIS, Face, Font, Marked, TextFitter};
use crate::theme;
use crate::ui_state::{SavedTab, UiState, Visit, match_span, search_score};
use anyrender_vello::VelloWindowRenderer;
use blitz_dom::{
    BaseDocument, DocGuard, DocGuardMut, Document, DocumentConfig, EventDriver, EventHandler,
    LocalName, NodeId, QualName, ns,
};
use blitz_shell::{BlitzApplication, BlitzShellProxy, ControlFlow, EventLoop, WindowConfig};
use blitz_traits::events::{DomEvent, DomEventData, EventState, MouseEventButton, UiEvent};
use blitz_traits::net::NetWaker;
use gaze_dom_blitz::{FindHit, Pacer, RhoDocument, WakeHandle};
use keyboard_types::{Key, Modifiers};
use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, HashSet};
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
const TAB_STRIP_HTML: &str = r#"<div id="tablist"></div><button id="newtab" class="icon-btn" data-action="newtab" aria-label="New tab"><i class="fa-solid fa-plus"></i><span class="tip tip-below">New tab · Ctrl+T</span></button>"#;

const TOOLBAR_HTML: &str = r#"<button id="sidetoggle" class="icon-btn" data-action="sidebar:toggle" aria-label="Sidebar"><i class="fa-solid fa-table-columns"></i><span class="tip tip-below">Show or hide the sidebar · Ctrl+B</span></button>
<button id="back" class="icon-btn" data-action="back" aria-label="Back"><i class="fa-solid fa-arrow-left"></i><span class="tip tip-below">Back</span></button>
<button id="fwd" class="icon-btn" data-action="fwd" aria-label="Forward"><i class="fa-solid fa-arrow-right"></i><span class="tip tip-below">Forward</span></button>
<button id="reload" class="icon-btn" data-action="reload" aria-label="Reload"><i class="fa-solid fa-rotate-right"></i><span class="tip tip-below">Reload · Ctrl+R</span></button>
<div id="urlwrap"><span id="scheme"></span><span class="inbox"><input id="url" type="text" value="" aria-label="Address"><span id="url-ph" class="ghost-hint off">Enter an address, or search tabs and history</span></span><div id="suggestions"></div></div>
<!-- Go was removed: Enter opens the address, and its arrow icon was the same as Forward's.
<button data-action="go" title="Open address"><i class="fa-solid fa-arrow-right"></i></button> -->
<button id="findbtn" class="icon-btn" data-action="find:show" aria-label="Find in page"><i class="fa-solid fa-magnifying-glass"></i><span class="tip tip-below-end">Find in page · Ctrl+F</span></button>
<!-- The menu button was removed: it did the same as the rail's Tabs button.
<button data-action="panel:tabs" title="Toggle tab sidebar"><i class="fa-solid fa-bars"></i></button> -->
<!-- Scheme cycling was removed: Appearance shows and sets the color scheme.
<button data-action="theme:next" title="Change color scheme"><i class="fa-solid fa-palette"></i></button> -->"#;

const RAIL_HTML: &str = r#"<button id="rail-tabs" class="rail-btn" data-action="panel:tabs" aria-label="Tabs"><i class="fa-solid fa-layer-group"></i><span class="tip tip-right">Tabs</span></button>
<button id="rail-history" class="rail-btn" data-action="panel:history" aria-label="History"><i class="fa-solid fa-clock-rotate-left"></i><span class="tip tip-right">History · Ctrl+H</span></button>
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
fn chrome_css(scheme: &str, custom: Option<&BTreeMap<String, String>>) -> String {
    format!(
        "{}{}{}",
        theme::font_css(),
        theme::variables(scheme, custom),
        CSS
    )
}

/// The window's initial markup.
fn shell_html(css: &str, sidebar_open: bool) -> String {
    SHELL
        .replace("__CSS__", css)
        .replace("__TAB_STRIP__", TAB_STRIP_HTML)
        .replace("__TOOLBAR__", TOOLBAR_HTML)
        .replace("__RAIL__", RAIL_HTML)
        .replace("__WALLET_FORM__", WALLET_FORM_HTML)
        .replace("__FIND_BOX__", FIND_BOX_HTML)
        .replace(
            "__SIDEBAR_CLASS__",
            if sidebar_open { "" } else { "hidden" },
        )
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
    Theme(String),
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
            "theme" => Action::Theme(rest.into()),
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

fn global_shortcut(key: &Key, modifiers: Modifiers, tabs: &[u64], active: usize) -> Option<Action> {
    let cmd = modifiers.contains(Modifiers::CONTROL) || modifiers.contains(Modifiers::META);
    if !cmd {
        return None;
    }
    if *key == Key::Tab && !tabs.is_empty() {
        let next = if modifiers.contains(Modifiers::SHIFT) {
            (active + tabs.len() - 1) % tabs.len()
        } else {
            (active + 1) % tabs.len()
        };
        return Some(Action::Select(tabs[next]));
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
        "h" => Some(Action::Panel("history".into())),
        "b" => Some(Action::SidebarToggle),
        _ => None,
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum PaletteStatus {
    Valid,
    Missing,
    Invalid(String),
}

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
    html.push_str(r#"<span class="grow"></span><button class="icon-btn" data-action="sidebar:toggle" aria-label="Hide sidebar"><i class="fa-solid fa-angles-left"></i><span class="tip tip-below-end">Hide sidebar · Ctrl+B</span></button>"#);
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
            r#"<div class="section-head"><span>Recently closed</span><span class="count">{}</span><span class="grow"></span><span class="tag">Ctrl+Shift+T</span></div>"#,
            closed.len()
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

/// The Appearance panel: the scheme, its colours, and the custom palette.
fn appearance_html(
    fit: &mut TextFitter,
    scheme: &str,
    colors: &BTreeMap<String, String>,
    palette_path: &str,
    status: &PaletteStatus,
) -> PanelView {
    let mut html = String::with_capacity(4_096);
    let segment = |value: &str, icon: &str, label: &str, disabled: bool| {
        format!(
            r#"<button class="segment{on}" data-action="theme:{value}"{off}><i class="fa-solid {icon}"></i><span>{label}</span></button>"#,
            on = if scheme == value { " on" } else { "" },
            off = if disabled { " disabled" } else { "" },
        )
    };
    put!(
        html,
        r#"<div class="section-head"><span>Color scheme</span></div><div class="segmented">{}{}{}</div><span class="footnote">Colors the browser controls and built-in pages. Sites keep their own styles.</span>"#,
        segment("dark", "fa-moon", "Dark", false),
        segment("light", "fa-sun", "Light", false),
        segment("custom", "fa-swatchbook", "Custom", *status != PaletteStatus::Valid),
    );
    html.push_str(r#"<div class="section-head"><span>Current palette</span></div><div class="swatches">"#);
    for name in theme::token_names() {
        let color = colors.get(*name).map(String::as_str).unwrap_or("transparent");
        put!(
            html,
            r#"<div class="swatch"><span class="swatch-color" style="background:{}"></span><span class="swatch-name">{}</span></div>"#,
            escape(color),
            escape(name.trim_start_matches("--gaze-"))
        );
    }
    let path_width = geometry::card_inner() - geometry::ROUNDING;
    put!(
        html,
        r#"</div><div class="section-head"><span>Custom palette file</span></div><div class="card"><div class="mono wallet-address">{}</div>"#,
        escape(&fit.middle(MONO, palette_path, path_width))
    );
    let (notice, button) = match status {
        PaletteStatus::Valid => (
            notice_html(
                Tone::Ok,
                "The palette is valid. Edit the file, then reload it to apply the changes.",
                None,
            ),
            r#"<button class="btn btn-sm" data-action="theme:custom"><i class="fa-solid fa-rotate-right"></i><span>Reload palette</span></button>"#,
        ),
        PaletteStatus::Missing => (
            notice_html(
                Tone::Info,
                "Create a palette file to choose your own colors. It lists every --gaze-* color as #RRGGBB.",
                None,
            ),
            r#"<button class="btn btn-sm" data-action="theme:template"><i class="fa-solid fa-plus"></i><span>Create palette file</span></button>"#,
        ),
        PaletteStatus::Invalid(why) => (
            notice_html(Tone::Err, &format!("The palette cannot be used: {why}"), None),
            r#"<button class="btn btn-sm" data-action="theme:custom"><i class="fa-solid fa-rotate-right"></i><span>Reload palette</span></button>"#,
        ),
    };
    put!(
        html,
        r#"{notice}<div class="card-actions">{button}</div></div>"#
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
            "Balances and transfers need an Embers service: set embers_api in settings.conf.",
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
    wallet: Arc<Mutex<WalletView>>,
    ui: UiState,
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
    state_dirty: bool,
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
    pub fn new(eng: Rc<Engine>, url: &str) -> ChromeDocument {
        let ui = UiState::load(&eng.dir);
        let panel = ui.panel.clone();
        let custom = theme::custom_palette(&eng.dir).ok();
        let css = chrome_css(&ui.theme, custom.as_ref());
        let html = shell_html(&css, ui.sidebar_open);
        let wake = WakeHandle::default();
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
            wake,
            panel,
            remember: false,
            consoles: BTreeMap::new(),
            rendered: BTreeMap::new(),
            url_dirty: true,
            title: String::new(),
            flash: None,
            wallet: Default::default(),
            ui,
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
            state_dirty: false,
            last_find_scan: Instant::now(),
            fitter: TextFitter::new(),
        };
        if url == c.eng.settings.home && !c.ui.tabs.is_empty() {
            let saved = c.ui.tabs.clone();
            let mut ids = Vec::with_capacity(saved.len());
            for t in &saved {
                let parent = t.parent.and_then(|i| ids.get(i).copied());
                c.add_tab(&t.url, parent, false, &t.title);
                ids.push(c.next_id);
            }
            c.select(c.ui.active.min(c.tabs.len() - 1));
        } else {
            c.open_tab(url);
        }
        c.apply_theme();
        // A profile reopened on the Wallet panel shows balances at once.
        if c.ui.sidebar_open && c.panel == "wallet" {
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
        let mut tab = Tab::new(Rc::clone(&self.eng), self.next_id, self.wake.clone());
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
        self.state_dirty = true;
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
        self.state_dirty = true;
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
        self.state_dirty = true;
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

    fn save_workspace(&mut self) {
        if !self.state_dirty {
            return;
        }
        self.ui.tabs = self.saved_tabs();
        self.ui.active = self.active;
        let _ = self.ui.save(&self.eng.dir);
        self.state_dirty = false;
    }

    /// Error pages are F1R3Gaze's own, so they are themed like `gaze://`.
    fn themed_as_builtin(tab: &Tab) -> bool {
        tab.url.starts_with("gaze://") || tab.error_page
    }

    fn apply_theme(&mut self) {
        let custom = theme::custom_palette(&self.eng.dir).ok();
        let colors = theme::palette(&self.ui.theme, custom.as_ref());
        if let Some(root) = self
            .id("html")
            .or_else(|| Some(self.inner.root_element().id))
        {
            let mut m = self.inner.mutate();
            for (key, value) in &colors {
                m.set_style_property(root, key, value);
            }
        }
        let css = theme::builtin_css(&self.ui.theme, custom.as_ref());
        for (t, view) in &self.tabs {
            if Self::themed_as_builtin(t)
                && let Some(r) = rho_mut(&mut self.inner, *view)
            {
                r.set_host_theme(&css);
            }
        }
        // Rebuild every rendered region with fresh boxes. The constant side
        // controls stay: rebuilding them would take focus from their field.
        self.rendered.retain(|id, _| *id == "sidecontrols");
        self.state_dirty = true;
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
        self.state_dirty = true;
    }

    /// The suggestions for what is typed: open tabs (switched to, not
    /// reopened) and history, without the active tab itself.
    fn build_suggestions(&self) -> Vec<Suggestion> {
        let active_url = &self.tabs[self.active].0.url;
        self.ui
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
        self.ui.panel = self.panel.clone();
        self.set_sidebar(true);
        if self.panel == "wallet" {
            self.refresh_balances();
        }
        if self.panel == "tabs" {
            self.side_scan_cursor = 0;
            self.side_results.clear();
        }
        self.state_dirty = true;
    }

    fn set_sidebar(&mut self, open: bool) {
        let opening = open && !self.ui.sidebar_open;
        self.ui.sidebar_open = open;
        if !open {
            self.menu_tab = None;
            self.history_confirm = false;
        }
        if opening && self.panel == "wallet" {
            self.refresh_balances();
        }
        self.state_dirty = true;
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
        self.state_dirty = true;
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
            Action::Panel(p) => match (self.ui.sidebar_open, self.panel == p) {
                (true, true) => self.set_sidebar(false),
                _ => self.show_panel(&p),
            },
            Action::ShowPanel(p) => self.show_panel(&p),
            Action::SidebarToggle => {
                let open = !self.ui.sidebar_open;
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
            Action::Theme(choice) => self.theme_action(&choice),
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
                self.ui.tree_tabs = !self.ui.tree_tabs;
                self.state_dirty = true;
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
                    self.ui.forget(&url);
                    self.state_dirty = true;
                }
                _ => {}
            },
            Action::SiteOp(verb, site) => self.site_op(&verb, &site),
            Action::ConsoleClear => {
                self.consoles.remove(&self.tabs[i].0.id);
            }
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
                let p = self
                    .eng
                    .dir
                    .join("logs")
                    .join(format!("tab-{tid}-{}.gzlog", std::process::id()));
                let _ = std::fs::create_dir_all(p.parent().expect("a log path has a parent"));
                match std::fs::write(&p, bytes) {
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

    fn theme_action(&mut self, choice: &str) {
        let has_custom = theme::custom_palette(&self.eng.dir).is_ok();
        if choice == "template" {
            let path = self.eng.dir.join("palette.css");
            if !path.exists() {
                let body: String = theme::palette("dark", None)
                    .iter()
                    .map(|(k, v)| format!("{k}: {v};\n"))
                    .collect();
                match std::fs::write(&path, body) {
                    Ok(()) => {
                        self.flash(Tone::Ok, format!("Palette created at {}", path.display()))
                    }
                    Err(e) => self.flash(Tone::Err, format!("Could not create the palette: {e}")),
                }
            }
        }
        let chosen = match choice {
            "next" => match self.ui.theme.as_str() {
                "dark" => "light",
                "light" if has_custom => "custom",
                _ => "dark",
            },
            other => other,
        };
        match (chosen, theme::custom_palette(&self.eng.dir)) {
            ("dark" | "light", _) | ("custom", Ok(_)) => {
                self.ui.theme = chosen.into();
                self.apply_theme();
            }
            ("custom", Err(_)) => self.flash(
                Tone::Warn,
                "The custom palette is missing or invalid; see Appearance",
            ),
            _ => {}
        }
    }

    fn history_op(&mut self, verb: &str) {
        match verb {
            "clear-ask" => self.history_confirm = true,
            "clear-cancel" => self.history_confirm = false,
            "clear" => {
                self.ui.visits.clear();
                self.history_confirm = false;
                self.state_dirty = true;
                self.flash(Tone::Ok, "History cleared");
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
                    let body = wallets.export(&a)?;
                    let dir = self.eng.dir.join("exports");
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    let p = dir.join(format!("{a}.json"));
                    std::fs::write(&p, body).map_err(|e| e.to_string())?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ =
                            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
                    }
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
            let depth = match (self.ui.tree_tabs, query.is_empty()) {
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
            &self.ui.visits,
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

    fn render_appearance_panel(&mut self) -> PanelView {
        let custom = theme::custom_palette(&self.eng.dir);
        let path = self.eng.dir.join("palette.css");
        let status = match &custom {
            Ok(_) => PaletteStatus::Valid,
            Err(_) if !path.exists() => PaletteStatus::Missing,
            Err(e) => PaletteStatus::Invalid(e.clone()),
        };
        let colors = theme::palette(&self.ui.theme, custom.as_ref().ok());
        appearance_html(
            &mut self.fitter,
            &self.ui.theme,
            &colors,
            &path.display().to_string(),
            &status,
        )
    }

    /// The wallet section: dynamic parts and the state of the static form.
    fn render_wallet(&mut self) -> bool {
        if self.panel != "wallet" || !self.ui.sidebar_open {
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
        if !self.ui.sidebar_open
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
        let sidebar = self.ui.sidebar_open;
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
            let on = self.ui.sidebar_open && self.panel == panel;
            changed |= self.sync_attr(
                rail_id,
                "class",
                Some(if on { "rail-btn on" } else { "rail-btn" }),
            );
        }
        changed |= self.sync_attr(
            "sidetoggle",
            "class",
            Some(if self.ui.sidebar_open {
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
        if !self.ui.sidebar_open {
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
            Some(if self.ui.tree_tabs { "chip on" } else { "chip" }),
        );
        changed |= self.sync_attr(
            "chip-tree",
            "aria-pressed",
            Some(if self.ui.tree_tabs { "true" } else { "false" }),
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
    }
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
        // The window owns browser shortcuts and its fields' keys even while a
        // page sub-document has focus; they are handled before Blitz routes
        // the key anywhere.
        if let UiEvent::KeyDown(k) = &event
            && !k.is_composing
        {
            let tab_ids: Vec<u64> = self.tabs.iter().map(|(t, _)| t.id).collect();
            if let Some(action) = global_shortcut(&k.key, k.modifiers, &tab_ids, self.active) {
                self.act(action);
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

    fn poll(&mut self, cx: Option<TaskContext>) -> bool {
        if let Some(cx) = &cx {
            self.wake.set_waker(cx.waker());
        }
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
                    self.ui.visit(&tab.url, &tab.title);
                    self.state_dirty = true;
                }
                let view = self.tabs[i].1;
                self.inner.remove_sub_document(view);
                self.inner.set_sub_document(view, Box::new(doc));
                let themed = Self::themed_as_builtin(&self.tabs[i].0);
                if let Some(r) = rho_mut(&mut self.inner, view) {
                    r.set_foreground(i == self.active);
                    if themed {
                        let custom = theme::custom_palette(&self.eng.dir).ok();
                        r.set_host_theme(&theme::builtin_css(&self.ui.theme, custom.as_ref()));
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
                self.state_dirty = true;
            }
        }
        self.scan_sidebar_pages(changed);
        changed |= self.render();
        self.save_workspace();
        self.schedule_wakes();
        changed
    }
}

/// Open the browser window and run until it closes.
pub fn launch(eng: Rc<Engine>, url: &str) -> Result<(), String> {
    // Built here rather than by `create_default_event_loop`, which panics
    // when there is no display; the user gets a message instead.
    let event_loop = EventLoop::builder().build().map_err(|e| {
        format!("cannot open a window ({e}); use --headless to run pages without one")
    })?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let (proxy, rx) = BlitzShellProxy::new(event_loop.create_proxy());
    let mut app = BlitzApplication::new(proxy, rx);
    let chrome = ChromeDocument::new(eng, url);
    app.add_window(WindowConfig::new(
        Box::new(chrome),
        VelloWindowRenderer::new(),
    ));
    event_loop.run_app(app).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
