//! Tests of the chrome: the action protocol, keyboard handling, markup
//! conventions and text budgets, panel behaviour, and the regression tests
//! of the defect ledger (`docs/ui/ledger.md`).

use super::*;
use crate::test_support::{ScratchProfile, stale_text_colours};
use blitz_traits::events::{
    BlitzKeyEvent, BlitzPointerEvent, BlitzPointerId, KeyState, MouseEventButtons, PointerCoords,
};
use blitz_traits::shell::ShellProvider;
use cursor_icon::CursorIcon;
use keyboard_types::{Code, Location};
use std::sync::atomic::{AtomicUsize, Ordering};

// ── Helpers ─────────────────────────────────────────────────────────────

const NOTES: &str = "<h1>Field notes on gaze</h1><p>Alpha: a page holds capabilities.</p><p>Gamma: search for alpha.</p>";

/// Poll until tab `i` has a document (bounded), without laying it out.
fn attach_unlaid(chrome: &mut ChromeDocument, i: usize) -> NodeId {
    let view = chrome.tabs[i].1;
    let deadline = Instant::now() + Duration::from_secs(10);
    while rho_mut(&mut chrome.inner, view).is_none() {
        assert!(
            Instant::now() < deadline,
            "tab {i} was not attached within 10 s"
        );
        chrome.poll(None);
        std::thread::sleep(Duration::from_millis(5));
    }
    view
}

/// Lay a page out as a window would.
fn lay_out_page(chrome: &mut ChromeDocument, view: NodeId) {
    let base = rho_mut(&mut chrome.inner, view)
        .expect("attached")
        .base();
    base.borrow_mut().viewport_mut().window_size = (800, 600);
    base.borrow_mut().resolve(0.0);
}

/// Poll until tab `i` has a document, then lay it out: `poll()` attaches
/// documents before any layout exists.
fn attach(chrome: &mut ChromeDocument, i: usize) -> NodeId {
    let view = attach_unlaid(chrome, i);
    lay_out_page(chrome, view);
    view
}

/// The page's selected text, when it has a selection.
fn selection(chrome: &mut ChromeDocument, view: NodeId) -> Option<String> {
    let base = rho_mut(&mut chrome.inner, view)
        .expect("attached")
        .base();
    let doc = base.borrow();
    doc.has_text_selection()
        .then(|| doc.get_selected_text().unwrap_or_default())
}

/// Lay the chrome itself out at a window size.
fn lay_out_chrome_at(chrome: &mut ChromeDocument, width: u32, height: u32) {
    chrome.inner.viewport_mut().window_size = (width, height);
    chrome.inner.resolve(0.0);
}

fn lay_out_chrome(chrome: &mut ChromeDocument) {
    lay_out_chrome_at(chrome, 1280, 800);
}

/// A chrome on `page` with the given workspace state saved first.
fn chrome_with(profile: &ScratchProfile, state: UiState, url: &str) -> ChromeDocument {
    // The seeded state is restored as saved, the sidebar included.
    profile.restore_sidebar();
    state.save(profile.path()).expect("save the workspace");
    ChromeDocument::new(Engine::new(profile.path().to_path_buf()), url)
}

fn key_down(key: Key) -> UiEvent {
    UiEvent::KeyDown(BlitzKeyEvent {
        key,
        code: Code::Unidentified,
        modifiers: Modifiers::empty(),
        location: Location::Standard,
        is_auto_repeating: false,
        is_composing: false,
        state: KeyState::Pressed,
        text: None,
    })
}

fn pointer(
    kind: fn(BlitzPointerEvent) -> UiEvent,
    x: f32,
    y: f32,
    buttons: MouseEventButtons,
) -> UiEvent {
    kind(BlitzPointerEvent {
        id: BlitzPointerId::Mouse,
        is_primary: true,
        coords: PointerCoords {
            page_x: x,
            page_y: y,
            screen_x: x,
            screen_y: y,
            client_x: x,
            client_y: y,
        },
        button: MouseEventButton::Main,
        buttons,
        mods: Modifiers::empty(),
        details: Default::default(),
        element: Default::default(),
        active_pointers: Default::default(),
    })
}

fn width_of(doc: &BaseDocument, selector: &str) -> f32 {
    let node = doc
        .query_selector(selector)
        .expect("a valid selector")
        .unwrap_or_else(|| panic!("nothing matches {selector}"));
    doc.get_node(node)
        .expect("the matched node")
        .final_layout()
        .size
        .width
}

fn assert_close(what: &str, laid_out: f32, budget: f32) {
    assert!(
        (laid_out - budget).abs() <= 0.6,
        "{what}: Blitz lays it out {laid_out} px wide, the budget assumes {budget} px"
    );
}

/// Visible text sitting in an anonymous box, which Blitz never restyles
/// (convention R1). Generated content (`::before`/`::after`, which is how
/// the icon font draws its glyphs) is restyled with its element and is not
/// counted.
fn bare_text_in_anonymous_boxes(doc: &BaseDocument) -> Vec<String> {
    let mut found = Vec::new();
    for (id, node) in doc.tree().iter() {
        if !node.is_anonymous() {
            continue;
        }
        if let Some(parent) = node.parent.and_then(|p| doc.get_node(p))
            && (parent.before() == Some(id) || parent.after() == Some(id))
        {
            continue;
        }
        for child in node.children.iter().filter_map(|c| doc.get_node(*c)) {
            if let Some(text) = child.text_data()
                && !text.content.trim().is_empty()
            {
                found.push(text.content.trim().to_string());
            }
        }
    }
    found
}

/// Every `data-action` value in a document.
fn data_actions(doc: &BaseDocument) -> Vec<String> {
    doc.tree()
        .iter()
        .filter_map(|(id, _)| attr_of(doc, id, "data-action"))
        .collect()
}

/// Representative output of every builder, with the region it renders into.
fn sample_fragments() -> Vec<(&'static str, String)> {
    let mut fit = TextFitter::new();
    let long = "A page with a very long title that should be truncated in the tab strip and the sidebar";
    let rows = [
        TabRow {
            id: 1,
            title: "Field notes on gaze and capability",
            url: "file:///tmp/site/notes.html",
            depth: 0,
            active: true,
            attention: TabAttention::Idle,
            page_matches: 0,
            menu_open: true,
            has_children: true,
            first: true,
            last: false,
        },
        TabRow {
            id: 2,
            title: long,
            url: "https://docs.example.invalid/very/long/path?query=1",
            depth: 2,
            active: false,
            attention: TabAttention::NeedsAnswer,
            page_matches: 3,
            menu_open: false,
            has_children: false,
            first: false,
            last: true,
        },
    ];
    let closed = [ClosedRow {
        index: 0,
        title: "Closed page",
        url: "https://example.org/closed",
    }];
    let now = 2_000_000_000;
    let visits = [
        Visit {
            url: "https://example.org/".into(),
            title: "Example Domain".into(),
            at: now - 30,
        },
        Visit {
            url: "https://f1r3fly.io/".into(),
            title: long.into(),
            at: now - 3 * 3600,
        },
        Visit {
            url: "f1r3://0123456789abcdef/project/index.html".into(),
            title: String::new(),
            at: now - 40 * 86_400,
        },
    ];
    let site = SiteUsage {
        site: "https://example.org:443".into(),
        has_store: true,
        stored: 12_700,
        remembered: 2,
        sessions: vec![SessionRow {
            tab: 7,
            label: "session-1".into(),
            uri: "rho:id:abcdef0123456789".into(),
        }],
    };
    let cache = gaze_blob::CacheStats {
        entries: 3,
        bytes: 4_096,
        partial_entries: 1,
        partial_bytes: 512,
    };
    let permissions = PermissionsView {
        site: Some("file:///tmp/site/notes.html".into()),
        stage: Stage::Running,
        grants: vec![
            gaze_exec::Grant {
                ident: "net".into(),
                urn: gaze_broker::NET.into(),
                granted: true,
            },
            gaze_exec::Grant {
                ident: "shard".into(),
                urn: gaze_broker::SHARD.into(),
                granted: false,
            },
        ],
        remembered: vec![RememberedChoice {
            urn: gaze_broker::SHARD.into(),
            allow: true,
            classes: vec!["read", "explore"],
        }],
    };
    let colors = theme::palette("dark", None);
    let wallet = WalletRow {
        address: "11112eMWq1fS7F8iUcujR3zktD1woWR6ZALP4mW5acQq9xyz".into(),
        label: "Snapshot wallet".into(),
        balance: Some("1250000".into()),
    };
    let other = WalletRow {
        label: String::new(),
        balance: None,
        ..wallet.clone()
    };
    let labels = BTreeMap::from([(wallet.address.clone(), wallet.label.clone())]);
    let chips: Vec<TabChip<'_>> = (0..24)
        .map(|n| TabChip {
            id: n + 1,
            title: if n % 2 == 0 { long } else { "Short" },
            url: "https://example.org/",
            attention: match n % 4 {
                0 => TabAttention::Idle,
                1 => TabAttention::Loading,
                2 => TabAttention::NeedsAnswer,
                _ => TabAttention::Failed,
            },
            active: n == 15,
        })
        .collect();
    let suggestions = [
        Suggestion {
            title: "Shard reader".into(),
            url: "file:///tmp/site/shard.html".into(),
            open_tab: Some(4),
        },
        Suggestion {
            title: long.into(),
            url: "https://en.wikipedia.org/wiki/Capability-based_security".into(),
            open_tab: None,
        },
    ];
    let flash = Flash {
        tone: Tone::Ok,
        text: "Address copied".into(),
        until: Instant::now(),
    };
    vec![
        ("tablist", tab_strip_html(&mut fit, &chips, 1280.0)),
        ("scheme", scheme_badge_html("http://example.org/", false)),
        ("scheme", scheme_badge_html("https://example.org/", true)),
        ("suggestions", suggestions_html(&mut fit, &suggestions, "example", Some(1), 1280.0)),
        (
            "prompt",
            prompt_bar_html(
                Some("file:///tmp/site/prompt.html"),
                "file:///tmp/site/prompt.html wants to fetch from https://example.org:443",
                1,
                2,
                3,
                true,
            ),
        ),
        ("prompt", prompt_bar_html(None, "A question", 1, 2, 1, false)),
        ("sidehead", side_head_html("Tabs", Some("3 of 12"))),
        ("sidecontrols", TABS_CONTROLS_HTML.to_string()),
        ("sidecontrols", HISTORY_CONTROLS_HTML.to_string()),
        ("sidecontrols", CONSOLE_CONTROLS_HTML.to_string()),
        ("panel", tab_panel_html(&mut fit, &rows, &closed, "", 2).html),
        ("panel", tab_panel_html(&mut fit, &rows, &closed, "notes", 2).html),
        ("panel", tab_panel_html(&mut fit, &[], &[], "zzz", 2).html),
        ("panel", history_panel_html(&mut fit, &visits, "", now, 1, true).html),
        ("panel", history_panel_html(&mut fit, &visits, "example", now, 200, false).html),
        ("panel", history_panel_html(&mut fit, &[], "", now, 200, false).html),
        ("panel", site_data_html(&mut fit, &[site], &cache).html),
        (
            "panel",
            site_data_html(&mut fit, &[], &gaze_blob::CacheStats::default()).html,
        ),
        ("panel", permissions_html(&mut fit, &permissions).html),
        (
            "panel",
            console_html(&[
                ("info".into(), "\"started\"".into()),
                ("warn".into(), "\"careful\"".into()),
                ("error".into(), "(\"err\", 1)".into()),
            ])
            .html,
        ),
        ("panel", console_html(&[]).html),
        (
            "panel",
            appearance_html(&mut fit, "dark", &colors, "/tmp/profile/palette.css", &PaletteStatus::Valid).html,
        ),
        (
            "panel",
            appearance_html(&mut fit, "light", &colors, "/tmp/profile/palette.css", &PaletteStatus::Missing).html,
        ),
        (
            "panel",
            appearance_html(
                &mut fit,
                "dark",
                &colors,
                "/tmp/profile/palette.css",
                &PaletteStatus::Invalid("invalid palette color: x".into()),
            )
            .html,
        ),
        ("walletcard", wallet_card_html(&mut fit, Some(&wallet), 2, true)),
        ("walletcard", wallet_card_html(&mut fit, None, 1, false)),
        ("walletcard", wallet_card_html(&mut fit, None, 0, false)),
        ("walletlist", wallet_list_html(&mut fit, &[other], true)),
        (
            "walletmsg",
            wallet_message_html(
                &mut fit,
                Some(&(Tone::Ok, "Imported 11112eMW…acQq9x.".into())),
                Some(&WalletConfirm::Send {
                    from: wallet.address.clone(),
                    to: "1111Recipient000000000000000000000000000000000000".into(),
                    amount: 50,
                    note: Some("thanks".into()),
                }),
                &labels,
            ),
        ),
        (
            "walletmsg",
            wallet_message_html(
                &mut fit,
                None,
                Some(&WalletConfirm::Remove {
                    address: wallet.address.clone(),
                    label: String::new(),
                }),
                &labels,
            ),
        ),
        (
            "status",
            status_html(
                &mut fit,
                1280.0,
                &Stage::Failed(format!("network error: https://x.invalid/: Dns Failed: {long}")),
                "https://x.invalid/",
                None,
                false,
                Some(&flash),
            ),
        ),
        (
            "status",
            status_html(
                &mut fit,
                1280.0,
                &Stage::Static,
                "https://x.invalid/",
                Some("This page's JavaScript was not run; F1R3Gaze runs only f1r3lang."),
                true,
                None,
            ),
        ),
    ]
}

/// The chrome's markup with `fragment` rendered into `region`, laid out at
/// 1280 × 800 px. Wallet regions are made visible (they are hidden until the
/// Wallet panel shows).
fn laid_out_shell_with(region: &str, fragment: &str) -> BaseDocument {
    let html = shell_html(&chrome_css("dark", None), true);
    let mut doc = gaze_dom_blitz::parse_html(
        &html,
        DocumentConfig {
            font_ctx: Some(theme::font_context()),
            ..Default::default()
        },
    );
    {
        let target = doc.get_element_by_id(region).expect("the region exists");
        let wallet = doc.get_element_by_id("wallet").expect("#wallet");
        let panel = doc.get_element_by_id("panel").expect("#panel");
        let mut m = doc.mutate();
        m.set_inner_html(target, fragment);
        if region.starts_with("wallet") {
            m.clear_attribute(wallet, qn("class"));
            m.set_attribute(panel, qn("class"), "hidden");
        }
    }
    doc.viewport_mut().window_size = (1280, 800);
    doc.resolve(0.0);
    doc
}

// ── The action protocol ─────────────────────────────────────────────────

#[test]
fn actions_parse() {
    assert_eq!(
        Action::parse("allow:3:77"),
        Some(Action::Answer(3, 77, true))
    );
    assert_eq!(
        Action::parse("revoke:rho:gaze:net"),
        Some(Action::Revoke("rho:gaze:net".into()))
    );
    assert_eq!(Action::parse("select:x"), None);
    assert_eq!(
        Action::parse("wallet:use:1111abc"),
        Some(Action::Wallet("use:1111abc".into()))
    );
    assert_eq!(
        Action::parse("history:clear-ask:0"),
        Some(Action::HistoryOp("clear-ask".into(), 0))
    );
    assert_eq!(
        Action::parse("tab:duplicate:7"),
        Some(Action::TabOp("duplicate".into(), 7))
    );
    assert_eq!(
        Action::parse("visit:open:https://example.org/a:b"),
        Some(Action::VisitOp("open".into(), "https://example.org/a:b".into()))
    );
    assert_eq!(
        Action::parse("show:grants"),
        Some(Action::ShowPanel("grants".into()))
    );
    assert_eq!(Action::parse("sidebar:toggle"), Some(Action::SidebarToggle));
    assert_eq!(Action::parse("console:clear"), Some(Action::ConsoleClear));
    assert_eq!(Action::parse("side:pages"), Some(Action::ToggleSidePages));
    assert_eq!(Action::parse("side:clear"), Some(Action::SideClear));
    assert_eq!(Action::parse("dismiss"), Some(Action::Dismiss));
    assert_eq!(
        Action::parse("site:session:7:label"),
        Some(Action::SiteOp("session".into(), "7:label".into()))
    );
    // Unknown sub-verbs are refused rather than guessed.
    assert_eq!(Action::parse("side:bogus"), None);
    assert_eq!(Action::parse("sidebar:x"), None);
    assert_eq!(Action::parse("console:x"), None);
    assert_eq!(Action::parse("find:x"), None);
}

#[test]
fn every_emitted_action_parses() {
    let shell = gaze_dom_blitz::parse_html(
        &shell_html(&chrome_css("dark", None), true),
        DocumentConfig::default(),
    );
    let mut actions = data_actions(&shell);
    for (_, fragment) in sample_fragments() {
        let doc = gaze_dom_blitz::parse_html(
            &format!("<html><body>{fragment}</body></html>"),
            DocumentConfig::default(),
        );
        actions.extend(data_actions(&doc));
    }
    assert!(actions.len() > 60, "the samples emit actions");
    for action in actions {
        assert!(
            Action::parse(&action).is_some(),
            "markup emits an action that does not parse: {action}"
        );
    }
}

#[test]
fn removed_toolbar_buttons_stay_commented() {
    let doc = gaze_dom_blitz::parse_html(
        &shell_html(&chrome_css("dark", None), true),
        DocumentConfig::default(),
    );
    let actions = data_actions(&doc);
    for removed in ["go", "theme:next", "panel:tabs"] {
        // "panel:tabs" stays on the rail; the toolbar copy is gone.
        let count = actions.iter().filter(|a| *a == removed).count();
        let expected = usize::from(removed == "panel:tabs");
        assert_eq!(count, expected, "{removed} in the live markup");
    }
    for rationale in [
        "Go was removed",
        "The menu button was removed",
        "Scheme cycling was removed",
    ] {
        assert!(TOOLBAR_HTML.contains(rationale), "missing: {rationale}");
    }
}

// ── Keyboard ────────────────────────────────────────────────────────────

#[test]
fn browser_shortcuts_select_tabs_even_when_page_has_focus() {
    let tabs = [11, 22, 33];
    assert_eq!(
        global_shortcut(&Key::Tab, Modifiers::CONTROL, &tabs, 1),
        Some(Action::Select(33))
    );
    assert_eq!(
        global_shortcut(&Key::Tab, Modifiers::CONTROL | Modifiers::SHIFT, &tabs, 0),
        Some(Action::Select(33))
    );
    assert_eq!(
        global_shortcut(&Key::Character("9".into()), Modifiers::CONTROL, &tabs, 0),
        Some(Action::Select(33))
    );
    assert_eq!(
        global_shortcut(&Key::Character("F".into()), Modifiers::CONTROL, &tabs, 0),
        Some(Action::Find(String::new()))
    );
    assert_eq!(
        global_shortcut(&Key::Character("b".into()), Modifiers::CONTROL, &tabs, 0),
        Some(Action::SidebarToggle)
    );
    assert_eq!(
        global_shortcut(&Key::Tab, Modifiers::empty(), &tabs, 0),
        None
    );
}

#[test]
fn chrome_keys_route_by_focus() {
    let none = Modifiers::empty();
    assert_eq!(
        chrome_key(&Key::Enter, none, KeyFocus::Address, false),
        Some((Action::AddressSubmit, true))
    );
    assert_eq!(
        chrome_key(&Key::ArrowDown, none, KeyFocus::Address, true),
        Some((Action::SuggestMove(1), true))
    );
    assert_eq!(
        chrome_key(&Key::ArrowUp, none, KeyFocus::Address, true),
        Some((Action::SuggestMove(-1), true))
    );
    // Without a dropdown, arrows are the field's.
    assert_eq!(chrome_key(&Key::ArrowDown, none, KeyFocus::Address, false), None);
    assert_eq!(
        chrome_key(&Key::Escape, none, KeyFocus::Address, true),
        Some((Action::AddressDismiss, true))
    );
    assert_eq!(
        chrome_key(&Key::Enter, Modifiers::SHIFT, KeyFocus::Find, false),
        Some((Action::FindNext(-1), true))
    );
    assert_eq!(
        chrome_key(&Key::Escape, none, KeyFocus::Find, false),
        Some((Action::FindHide, true))
    );
    assert_eq!(
        chrome_key(&Key::Escape, none, KeyFocus::SideSearch, false),
        Some((Action::SideClear, true))
    );
    // Escape elsewhere also reaches the page.
    assert_eq!(
        chrome_key(&Key::Escape, none, KeyFocus::Elsewhere, false),
        Some((Action::Dismiss, false))
    );
    assert_eq!(chrome_key(&Key::Enter, none, KeyFocus::Elsewhere, false), None);
}

#[test]
fn arrow_keys_cycle_through_suggestions_and_back_to_the_text() {
    assert_eq!(next_suggestion(None, 1, 3), Some(0));
    assert_eq!(next_suggestion(Some(0), 1, 3), Some(1));
    assert_eq!(next_suggestion(Some(2), 1, 3), None);
    assert_eq!(next_suggestion(None, -1, 3), Some(2));
    assert_eq!(next_suggestion(Some(0), -1, 3), None);
    assert_eq!(next_suggestion(Some(2), -1, 3), Some(1));
    assert_eq!(next_suggestion(Some(1), 1, 0), None);
}

// ── Markup ──────────────────────────────────────────────────────────────

#[test]
fn chrome_markup_has_stable_ids() {
    let doc = gaze_dom_blitz::parse_html(
        &shell_html(&chrome_css("dark", None), true),
        DocumentConfig::default(),
    );
    for id in [
        "tabs", "tablist", "newtab", "toolbar", "sidetoggle", "back", "fwd", "reload",
        "urlwrap", "scheme", "url", "url-ph", "suggestions", "findbtn", "prompt", "rail",
        "rail-tabs", "rail-history", "rail-sites", "rail-grants", "rail-wallet",
        "rail-console", "rail-appearance", "sidebar", "sidehead", "sidecontrols", "panel",
        "wallet", "walletmsg", "walletcard", "walletlist", "wallet-send-form", "wallet-to",
        "wallet-amount", "wallet-desc", "wallet-send", "wallet-import", "main",
        "findoverlay", "findbar", "find", "find-ph", "findcount", "status",
    ] {
        assert!(doc.get_element_by_id(id).is_some(), "missing #{id}");
    }
    for (controls, ids) in [
        (TABS_CONTROLS_HTML, &["side-search", "side-ph", "chip-tree", "chip-pages"][..]),
        (HISTORY_CONTROLS_HTML, &["side-search", "side-ph", "history-clear"][..]),
    ] {
        for id in ids {
            assert!(controls.contains(&format!(r#"id="{id}""#)), "missing #{id}");
        }
    }
}

#[test]
fn labels_are_never_bare_text_in_flex_boxes() {
    let mut failures = Vec::new();
    for (region, fragment) in sample_fragments() {
        let doc = laid_out_shell_with(region, &fragment);
        let bare = bare_text_in_anonymous_boxes(&doc);
        if !bare.is_empty() {
            failures.push(format!("#{region}: {bare:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "bare text in anonymous boxes (R1):\n{}",
        failures.join("\n")
    );
}

#[test]
fn side_controls_are_constant() {
    for controls in [TABS_CONTROLS_HTML, HISTORY_CONTROLS_HTML, CONSOLE_CONTROLS_HTML] {
        assert!(!controls.contains(" on\""), "no state classes in {controls}");
    }
    let profile = ScratchProfile::new("controls");
    let url = profile.page("notes.html", NOTES);
    let state = UiState {
        sidebar_open: true,
        panel: "tabs".into(),
        ..UiState::default()
    };
    let mut chrome = chrome_with(&profile, state, &url);
    attach(&mut chrome, 0);
    chrome.poll(None);
    let before = chrome.rendered["sidecontrols"].clone();
    chrome.act(Action::Input("side-search".into(), "gaze".into()));
    chrome.act(Action::ToggleTree);
    chrome.poll(None);
    assert_eq!(chrome.rendered["sidecontrols"], before);
}

#[test]
fn text_budgets_match_laid_out_boxes() {
    use geometry::*;
    let profile = ScratchProfile::new("budgets");
    let url = profile.page("alpha.html", NOTES);
    let state = UiState {
        sidebar_open: true,
        panel: "tabs".into(),
        ..UiState::default()
    };
    let mut chrome = chrome_with(&profile, state, &url);
    chrome
        .eng
        .wallets
        .import(&"11".repeat(32), "Budget wallet")
        .expect("import the test key");
    attach(&mut chrome, 0);
    // A second tab, so the strip has an inactive one too.
    chrome.open_tab(&url);
    for (width, height) in [(1280u32, 800u32), (900, 700)] {
        let window = width as f32;
        chrome.inner.viewport_mut().window_size = (width, height);
        chrome.act(Action::ShowPanel("tabs".into()));
        chrome.poll(None);
        lay_out_chrome_at(&mut chrome, width, height);
        let doc = &chrome.inner;
        assert_close("#panel .row-text", width_of(doc, "#panel .row-text"), row_text());
        assert_close("#urlwrap", width_of(doc, "#urlwrap"), address_box(window));
        let tab = width_of(doc, "#tablist .tab.on");
        assert_close(
            ".tab.on .tab-title",
            width_of(doc, "#tablist .tab.on .tab-title"),
            tab - TAB_CHROME,
        );
        assert!(tab <= TAB_MAX + 0.5, "a tab is at most {TAB_MAX} px");
        // An inactive tab's close button overlays its title, so the title
        // has everything but the padding, borders, icon, and one gap.
        let other = width_of(doc, "#tablist .tab:not(.on)");
        assert_close(
            ".tab:not(.on) .tab-title",
            width_of(doc, "#tablist .tab:not(.on) .tab-title"),
            other - TAB_CHROME_INACTIVE,
        );

        // Cards in #panel (the content cache card is always shown).
        chrome.act(Action::ShowPanel("sites".into()));
        chrome.poll(None);
        lay_out_chrome_at(&mut chrome, width, height);
        let card = width_of(&chrome.inner, "#panel .card");
        assert_close(
            "#panel .card content",
            card - 2.0 * CARD_BORDER - 2.0 * CARD_PAD_X,
            card_inner(),
        );

        // The paying wallet's card in #wallet.
        chrome.act(Action::ShowPanel("wallet".into()));
        chrome.poll(None);
        lay_out_chrome_at(&mut chrome, width, height);
        let card = width_of(&chrome.inner, "#walletcard .card");
        assert_close(
            "#wallet .card content",
            card - 2.0 * CARD_BORDER - 2.0 * CARD_PAD_X,
            card_inner(),
        );

        // A suggestion row in the address box's dropdown.
        chrome.act(Action::FocusUrl);
        chrome.act(Action::Input("url".into(), "alpha".into()));
        chrome.ui.visit("https://alpha.example/", "Alpha example");
        chrome.poll(None);
        lay_out_chrome_at(&mut chrome, width, height);
        let row = width_of(&chrome.inner, "#suggestions .suggestion");
        assert_close(
            ".suggestion content",
            row - SUGGESTION_FIXED,
            suggestion_row(window),
        );
        chrome.act(Action::AddressDismiss);
        chrome.poll(None);
    }
}

#[test]
fn the_status_bar_fits_on_one_line() {
    let mut fit = TextFitter::new();
    let why = format!(
        "network error: https://x.invalid/: Dns Failed: {}",
        "resolve dns name 'docs.example.invalid:443': failed to lookup address information ".repeat(3)
    );
    let flash = Flash {
        tone: Tone::Err,
        text: "Could not save the log: permission denied".into(),
        until: Instant::now(),
    };
    for width in [1280.0_f32, 900.0] {
        let html = status_html(
            &mut fit,
            width,
            &Stage::Failed(why.clone()),
            "https://x.invalid/",
            None,
            true,
            Some(&flash),
        );
        let mut doc = laid_out_shell_with("status", &html);
        doc.viewport_mut().window_size = (width as u32, 800);
        doc.resolve(0.0);
        let status = doc.get_element_by_id("status").expect("#status");
        let bar = doc.get_client_bounding_rect(status).expect("laid out");
        let last = doc
            .query_selector("#status .flash")
            .expect("a valid selector")
            .expect("the flash");
        let flash_box = doc.get_client_bounding_rect(last).expect("laid out");
        assert!(
            flash_box.x + flash_box.width <= bar.x + bar.width - f64::from(geometry::STATUS_PAD_X) + 1.0,
            "the status bar overflows at {width} px"
        );
        assert!(html.contains('…'), "the failure reason is shortened");
    }
}

// ── Builders ────────────────────────────────────────────────────────────

#[test]
fn tab_attention_follows_the_stage() {
    assert_eq!(TabAttention::of(&Stage::Running, false), TabAttention::Idle);
    assert_eq!(TabAttention::of(&Stage::Static, false), TabAttention::Idle);
    assert_eq!(TabAttention::of(&Stage::Fetching, false), TabAttention::Loading);
    assert_eq!(TabAttention::of(&Stage::Scripts, false), TabAttention::Loading);
    assert_eq!(TabAttention::of(&Stage::Grants, false), TabAttention::NeedsAnswer);
    assert_eq!(TabAttention::of(&Stage::Running, true), TabAttention::NeedsAnswer);
    assert_eq!(
        TabAttention::of(&Stage::Failed("x".into()), true),
        TabAttention::Failed
    );
}

#[test]
fn the_tab_strip_keeps_the_active_tab_in_view() {
    assert_eq!(visible_window(5, 2, 10), (0, 5));
    assert_eq!(visible_window(30, 0, 10), (0, 10));
    assert_eq!(visible_window(30, 29, 10), (20, 30));
    assert_eq!(visible_window(30, 15, 10), (10, 20));
    let mut fit = TextFitter::new();
    let chips: Vec<TabChip<'_>> = (0..30)
        .map(|n| TabChip {
            id: n + 1,
            title: "Tab",
            url: "https://example.org/",
            attention: TabAttention::Idle,
            active: n == 27,
        })
        .collect();
    let html = tab_strip_html(&mut fit, &chips, 1280.0);
    assert!(html.contains(r#"data-tab-id="28""#), "the active tab is shown");
    assert!(html.contains(r#"id="tabmore""#), "a +N chip leads to the rest");
    let shown = html.matches("data-tab-id=").count();
    assert!(shown < 30, "only a window of tabs is shown");
    assert!(html.contains(&format!("<span>+{}</span>", 30 - shown)));
}

#[test]
fn history_groups_by_recency_and_keeps_each_address_once_per_group() {
    let mut fit = TextFitter::new();
    let now = 1_000_000;
    let visit = |url: &str, title: &str, at: u64| Visit {
        url: url.into(),
        title: title.into(),
        at,
    };
    let visits = [
        visit("https://a/", "A", now - 10),
        visit("https://a/", "A again", now - 20),
        visit("https://b/", "B", now - 2 * 3600),
        visit("https://a/", "A last week", now - 3 * 86_400),
    ];
    let view = history_panel_html(&mut fit, &visits, "", now, 200, false);
    assert_eq!(view.count.as_deref(), Some("3"));
    let hour = view.html.find("Last hour").expect("a last-hour group");
    let day = view.html.find("Last 24 hours").expect("a last-day group");
    let week = view.html.find("Last 7 days").expect("a last-week group");
    assert!(hour < day && day < week);
    assert!(!view.html.contains("A again"), "the older duplicate is dropped");
    let limited = history_panel_html(&mut fit, &visits, "", now, 1, true);
    assert!(limited.html.contains("Show 2 more"));
    // The prompt counts the entries the list shows (L5 R7), like the header.
    assert!(limited.html.contains("Clear all 3 entries from history?"));
    let none = history_panel_html(&mut fit, &visits, "zzzz", now, 200, false);
    assert!(none.html.contains("No visits match"));
    assert_eq!(none.count.as_deref(), Some("0 of 3"));
    // A search counts as the Tabs panel does: matches of all entries (R12).
    let found = history_panel_html(&mut fit, &visits, "b", now, 200, true);
    assert_eq!(found.count.as_deref(), Some("1 of 3"));
    assert!(found.html.contains("Clear all 3 entries from history?"), "clearing is never filtered");
}

#[test]
fn small_pure_helpers() {
    assert_eq!(find_count("", 3, 0), (String::new(), false));
    assert_eq!(find_count("a", 0, 0), ("0/0".to_string(), true));
    assert_eq!(find_count("a", 12, 2), ("3/12".to_string(), false));
    assert_eq!(session_target("7:label:with:colons"), Some((7, "label:with:colons")));
    assert_eq!(session_target("x:label"), None);
    assert_eq!(display_address("gaze://newtab"), "");
    assert_eq!(display_address("https://example.org/"), "https://example.org/");
    assert_eq!(plural(1, "tab", "tabs"), "1 tab");
    assert_eq!(plural(3, "tab", "tabs"), "3 tabs");
}

// ── Behaviour ───────────────────────────────────────────────────────────

#[test]
fn reopening_closed_tabs_starts_with_the_most_recent() {
    let profile = ScratchProfile::new("closed");
    let first = profile.page("first.html", "<p>first</p>");
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &first);
    for n in 0..25 {
        let url = profile.page(&format!("p{n:02}.html"), "<p>x</p>");
        chrome.open_tab(&url);
    }
    let ids: Vec<u64> = chrome.tabs.iter().skip(1).map(|(t, _)| t.id).collect();
    for id in ids {
        chrome.act(Action::Close(id));
    }
    assert_eq!(chrome.closed.len(), CLOSED_LIMIT);
    assert!(chrome.closed.last().expect("closed").url.ends_with("p24.html"));
    assert!(chrome.closed.first().expect("closed").url.ends_with("p05.html"));
    chrome.act(Action::TabOp("restore".into(), 0));
    assert!(chrome.tabs.last().expect("reopened").0.url.ends_with("p24.html"));
}

#[test]
fn remember_applies_to_one_answer() {
    let profile = ScratchProfile::new("remember");
    let url = profile.page("notes.html", NOTES);
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url);
    chrome.act(Action::Remember);
    assert!(chrome.remember);
    let tab = chrome.tabs[0].0.id;
    chrome.act(Action::Answer(tab, 0, true));
    assert!(!chrome.remember, "the next prompt starts unchecked");
}

#[test]
fn switching_panels_or_reopening_the_sidebar_starts_a_fresh_search() {
    let profile = ScratchProfile::new("side-search");
    let url = profile.page("notes.html", NOTES);
    let state = UiState {
        sidebar_open: true,
        panel: "tabs".into(),
        ..UiState::default()
    };
    let mut chrome = chrome_with(&profile, state, &url);
    attach(&mut chrome, 0);
    chrome.poll(None);
    chrome.act(Action::Input("side-search".into(), "gaze".into()));
    assert_eq!(chrome.side_query, "gaze");
    chrome.act(Action::ShowPanel("history".into()));
    assert!(chrome.side_query.is_empty(), "History is not filtered by an invisible query");
    chrome.poll(None);
    chrome.act(Action::Input("side-search".into(), "wiki".into()));
    chrome.act(Action::SidebarToggle);
    chrome.poll(None);
    chrome.act(Action::SidebarToggle);
    chrome.poll(None);
    assert!(chrome.side_query.is_empty(), "the re-inserted field is empty, and so is its query");
}

#[test]
fn the_tab_menu_toggles_and_closes_after_an_action() {
    let profile = ScratchProfile::new("menu");
    let url = profile.page("notes.html", NOTES);
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url);
    let id = chrome.tabs[0].0.id;
    chrome.act(Action::TabOp("menu".into(), id));
    assert_eq!(chrome.menu_tab, Some(id));
    chrome.act(Action::TabOp("menu".into(), id));
    assert_eq!(chrome.menu_tab, None, "the same button closes it");
    chrome.act(Action::TabOp("menu".into(), id));
    chrome.act(Action::TabOp("copy".into(), id));
    assert_eq!(chrome.menu_tab, None, "an action closes it");
    chrome.act(Action::TabOp("menu-open".into(), id));
    assert_eq!(chrome.menu_tab, Some(id));
    assert!(chrome.ui.sidebar_open && chrome.panel == "tabs", "a right-click shows it");
    chrome.act(Action::Dismiss);
    assert_eq!(chrome.menu_tab, None, "Escape closes it");
}

#[test]
fn a_reviewed_transfer_is_refused_when_the_paying_wallet_changed() {
    let profile = ScratchProfile::new("payer");
    profile.restore_sidebar();
    let url = profile.page("notes.html", NOTES);
    let engine = Engine::new(profile.path().to_path_buf());
    let first = engine
        .wallets
        .import(&"11".repeat(32), "First")
        .expect("import the first test key");
    let second = engine
        .wallets
        .import(&"22".repeat(32), "Second")
        .expect("import the second test key");
    engine.wallets.set_active(&first).expect("the first pays");
    UiState {
        sidebar_open: true,
        panel: "wallet".into(),
        ..UiState::default()
    }
    .save(profile.path())
    .expect("save the workspace");
    let mut chrome = ChromeDocument::new(Rc::clone(&engine), &url);
    attach(&mut chrome, 0);
    chrome.poll(None);
    lay_out_chrome(&mut chrome);
    chrome.set_input_value("wallet-to", second.as_str());
    chrome.set_input_value("wallet-amount", "50");
    chrome.act(Action::Wallet("send".into()));
    let reviewed = chrome.wallet.lock().expect("wallet view").confirm.clone();
    assert!(
        matches!(&reviewed, Some(WalletConfirm::Send { from, amount: 50, .. }) if from == first.as_str()),
        "the transfer is reviewed first: {reviewed:?}"
    );
    engine.wallets.set_active(&second).expect("the second pays");
    chrome.act(Action::Wallet("confirm".into()));
    let notice = chrome.wallet.lock().expect("wallet view").notice.clone();
    assert!(
        matches!(&notice, Some((Tone::Warn, text)) if text.contains("paying wallet changed")),
        "nothing is sent: {notice:?}"
    );
}

/// Clicking a suggestion is handled before the address box loses focus, so
/// it switches to the open tab, and leaving the box then drops the dropdown.
/// This also shows that the dropdown, floating over the page, receives the
/// click.
#[test]
fn clicking_a_suggestion_switches_to_its_tab_then_dismisses() {
    let profile = ScratchProfile::new("suggest-click");
    let url_a = profile.page("first.html", NOTES);
    let url_b = profile.page("bee.html", "<p>bee</p>");
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url_a);
    attach(&mut chrome, 0);
    chrome.open_tab(&url_b);
    attach(&mut chrome, 1);
    let (id_a, id_b) = (chrome.tabs[0].0.id, chrome.tabs[1].0.id);
    chrome.act(Action::Select(id_a));
    chrome.act(Action::FocusUrl);
    chrome.act(Action::Input("url".into(), "bee".into()));
    chrome.inner.viewport_mut().window_size = (1280, 800);
    chrome.poll(None);
    lay_out_chrome(&mut chrome);
    let row = chrome
        .inner
        .query_selector("#suggestions .suggestion")
        .expect("a valid selector")
        .expect("a suggestion for the open tab");
    let rect = chrome
        .inner
        .get_client_bounding_rect(row)
        .expect("the suggestion is laid out");
    let (x, y) = (
        (rect.x + rect.width / 2.0) as f32,
        (rect.y + rect.height / 2.0) as f32,
    );
    chrome.handle_ui_event(pointer(UiEvent::PointerDown, x, y, MouseEventButtons::Primary));
    chrome.handle_ui_event(pointer(UiEvent::PointerUp, x, y, MouseEventButtons::None));
    assert_eq!(
        chrome.tabs[chrome.active].0.id, id_b,
        "the click switched to the open tab"
    );
    assert!(
        chrome.address_query.is_empty() && chrome.suggestions.is_empty(),
        "leaving the address box dropped the dropdown"
    );
}

// ── Ledger L1: find ─────────────────────────────────────────────────────

/// L1/H1 (with H3 and H4): emptying the query must clear the selection that
/// the previous 1-character query made.
#[test]
fn find_clears_selection_when_query_is_emptied() {
    let profile = ScratchProfile::new("find-empty");
    let url = profile.page("notes.html", NOTES);
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url);
    let view = attach(&mut chrome, 0);
    chrome.act(Action::Find(String::new()));
    chrome.act(Action::Input("find".into(), "a".into()));
    assert_eq!(
        selection(&mut chrome, view).as_deref(),
        Some("a"),
        "control: the harness reproduces the first hit"
    );
    chrome.act(Action::Input("find".into(), String::new()));
    assert!(
        selection(&mut chrome, view).is_none(),
        "H1: an empty query must not leave a hit selected"
    );
    for _ in 0..3 {
        chrome.poll(None);
    }
    assert!(
        selection(&mut chrome, view).is_none(),
        "H3: the periodic rescan must not reselect"
    );
    assert_eq!(
        chrome.rendered.get("findoverlay").map(String::as_str),
        Some(""),
        "H4: no highlight overlay remains"
    );
}

/// L1/H6: switching tabs moves the find selection with the active tab.
#[test]
fn find_selection_follows_the_active_tab() {
    let profile = ScratchProfile::new("find-tabs");
    let url_a = profile.page("a.html", NOTES);
    let url_b = profile.page("b.html", "<p>beta alpha beta</p>");
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url_a);
    let view_a = attach(&mut chrome, 0);
    chrome.open_tab(&url_b);
    let view_b = attach(&mut chrome, 1);
    let (id_a, id_b) = (chrome.tabs[0].0.id, chrome.tabs[1].0.id);
    chrome.act(Action::Select(id_a));
    chrome.act(Action::Find(String::new()));
    chrome.act(Action::Input("find".into(), "alpha".into()));
    assert_eq!(selection(&mut chrome, view_a).as_deref(), Some("Alpha"));
    chrome.act(Action::Select(id_b));
    assert!(
        selection(&mut chrome, view_a).is_none(),
        "H6: the outgoing tab keeps no find selection"
    );
    assert_eq!(
        selection(&mut chrome, view_b).as_deref(),
        Some("alpha"),
        "H6: find re-runs on the incoming tab"
    );
    assert_eq!(chrome.find_hits.len(), 1);
}

/// L1/H7: a page that attaches while find is open is searched once it has
/// been laid out (before that, its text cannot be searched).
#[test]
fn find_searches_a_new_page_once_it_is_laid_out() {
    let profile = ScratchProfile::new("find-attach");
    let url_a = profile.page("a.html", NOTES);
    let url_b = profile.page("b.html", "<p>beta alpha beta</p>");
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url_a);
    attach(&mut chrome, 0);
    chrome.act(Action::Find(String::new()));
    chrome.act(Action::Input("find".into(), "alpha".into()));
    chrome.open_tab(&url_b);
    let view_b = attach_unlaid(&mut chrome, 1);
    assert!(chrome.find_hits.is_empty() && chrome.find_rescan, "not laid out yet");
    lay_out_page(&mut chrome, view_b);
    // Hold off the 250 ms periodic rescan, which also searches a page that
    // reports a change, so only the H7 rescan can find the hit here.
    chrome.last_find_scan = Instant::now();
    chrome.poll(None);
    assert_eq!(chrome.find_hits.len(), 1, "searched once laid out");
    assert!(!chrome.find_rescan);
    assert_eq!(selection(&mut chrome, view_b).as_deref(), Some("alpha"));
}

/// L1: find clears only the selection it made, so text the user selected
/// survives opening and closing the find box.
#[test]
fn find_keeps_a_selection_it_did_not_make() {
    let profile = ScratchProfile::new("find-own");
    let url = profile.page("notes.html", NOTES);
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url);
    let view = attach(&mut chrome, 0);
    {
        let page = rho_mut(&mut chrome.inner, view).expect("attached");
        let word = page
            .find("Field", 1)
            .into_iter()
            .next()
            .expect("the heading contains the word");
        page.base()
            .borrow_mut()
            .set_text_selection(word.node, word.start, word.node, word.end);
    }
    chrome.act(Action::Find(String::new()));
    assert_eq!(selection(&mut chrome, view).as_deref(), Some("Field"));
    chrome.act(Action::FindHide);
    assert_eq!(selection(&mut chrome, view).as_deref(), Some("Field"));
}

/// L1/H2 refuted: real key events reach the find query, and Backspace to
/// empty produces an empty query (an Input event is sent). macOS routes
/// editing keys through standard key bindings instead.
#[cfg(not(target_os = "macos"))]
#[test]
fn find_input_follows_real_key_events() {
    let profile = ScratchProfile::new("find-keys");
    let url = profile.page("notes.html", NOTES);
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url);
    attach(&mut chrome, 0);
    chrome.act(Action::Find(String::new()));
    chrome.poll(None);
    lay_out_chrome(&mut chrome);
    chrome.handle_ui_event(key_down(Key::Character("a".into())));
    assert_eq!(chrome.find_query, "a");
    chrome.handle_ui_event(key_down(Key::Backspace));
    assert_eq!(chrome.find_query, "");
}

// ── Ledger L2: theme switches ───────────────────────────────────────────

/// L2: after a scheme switch, no label keeps the previous scheme's colour,
/// whichever panel is open and in either direction.
#[test]
fn theme_switch_leaves_no_stale_label_colours() {
    let profile = ScratchProfile::new("theme-stale");
    profile.restore_sidebar();
    let url = profile.page("notes.html", NOTES);
    let engine = Engine::new(profile.path().to_path_buf());
    // A fixed test key that is never funded, so the wallet panel lists a
    // wallet with its buttons.
    engine
        .wallets
        .import(&"11".repeat(32), "Snapshot")
        .expect("import the test key");
    let mut failures = Vec::new();
    for (panel, _, _) in PANELS {
        for (from, to) in [("dark", "light"), ("light", "dark")] {
            UiState {
                theme: from.into(),
                sidebar_open: true,
                panel: panel.into(),
                ..UiState::default()
            }
            .save(profile.path())
            .expect("save the workspace");
            let mut chrome = ChromeDocument::new(Rc::clone(&engine), &url);
            attach(&mut chrome, 0);
            if matches!(panel, "tabs" | "history") {
                // Search, so highlighted matches (<mark>) are on screen too.
                chrome.act(Action::Input("side-search".into(), "notes".into()));
            }
            chrome.poll(None);
            lay_out_chrome(&mut chrome);
            chrome.act(Action::Theme(to.into()));
            chrome.poll(None);
            lay_out_chrome(&mut chrome);
            let stale = stale_text_colours(&chrome.inner);
            if !stale.is_empty() {
                failures.push(format!("{panel} {from}→{to}: {stale:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "labels kept the previous scheme's colour:\n{}",
        failures.join("\n")
    );
}

// ── Ledger L5: review of the first complete "after" capture ────────────

/// The content of the text field `id` that its editor has selected.
fn selected_in(chrome: &ChromeDocument, id: &str) -> Option<String> {
    let node = chrome.id(id)?;
    chrome
        .inner
        .get_node(node)?
        .element_data()?
        .text_input_data()?
        .editor
        .selected_text()
        .map(str::to_string)
}

/// R6: the active tab keeps a readable width, the others shrink to their
/// icon, and only then does a window with a "+N" chip appear.
#[test]
fn the_strip_keeps_the_active_tab_readable_and_shrinks_the_rest() {
    use geometry::*;
    let room = |window: f32| window - 2.0 * STRIP_PAD_X - STRIP_GAP - NEWTAB;
    let few = strip_layout(6, 0, room(1280.0));
    assert_eq!((few.start, few.end, few.active, few.other), (0, 6, TAB_MAX, TAB_MAX));
    let many = strip_layout(16, 3, room(1280.0));
    assert_eq!((many.start, many.end), (0, 16), "sixteen tabs all show");
    assert_eq!(many.active, TAB_ACTIVE_MIN);
    assert!(TAB_MIN <= many.other && many.other < TAB_ACTIVE_MIN);
    let crowd = strip_layout(30, 27, room(1280.0));
    assert!(crowd.start <= 27 && 27 < crowd.end && crowd.end - crowd.start < 30);
    for window in [320.0, 400.0, 900.0, 1280.0, 1920.0, 3840.0] {
        let room = room(window);
        for count in 1..=60 {
            for active in [0, count / 2, count - 1] {
                let layout = strip_layout(count, active, room);
                let shown = layout.end - layout.start;
                let at = format!("{count} tabs, active {active}, {window} px: {layout:?}");
                assert!(layout.start <= active && active < layout.end, "{at}");
                assert!(layout.active <= TAB_MAX && layout.other <= TAB_MAX, "{at}");
                assert!(layout.active + 0.01 >= layout.other, "{at}");
                if shown >= 2 {
                    assert!(layout.other >= TAB_MIN - 0.01, "{at}");
                }
                let chip = if shown < count { TABMORE + TAB_GAP } else { 0.0 };
                let used = layout.active + (shown - 1) as f32 * (layout.other + TAB_GAP) + chip;
                assert!(used <= room + 0.01, "{at}: uses {used} of {room} px");
                if shown < count {
                    // As many tabs as fit at the minimum widths.
                    let one_more = TAB_ACTIVE_MIN + shown as f32 * (TAB_MIN + TAB_GAP);
                    assert!(one_more > room - TABMORE - TAB_GAP, "{at}");
                }
            }
        }
    }
}

/// R6: inactive tabs too narrow for any of their title show only their
/// icon; the active one keeps its title.
#[test]
fn crowded_tabs_shrink_to_their_icons() {
    let mut fit = TextFitter::new();
    let chips: Vec<TabChip<'_>> = (0..24)
        .map(|n| TabChip {
            id: n + 1,
            title: "Plain page",
            url: "file:///tmp/site/plain.html",
            attention: TabAttention::Idle,
            active: n == 0,
        })
        .collect();
    let html = tab_strip_html(&mut fit, &chips, 1280.0);
    assert!(!html.contains(r#"id="tabmore""#), "24 tabs fit at 1280 px");
    assert_eq!(html.matches(r#"class="tab narrow""#).count(), 23);
    let active = html
        .split(r#"class="tab on""#)
        .nth(1)
        .expect("the active tab");
    assert!(active.contains(r#"<span class="tab-title">Plain"#), "{active}");
    let html = tab_strip_html(&mut fit, &chips[..16], 1280.0);
    assert!(!html.contains("narrow"), "sixteen tabs keep part of their titles");
    // Every title is cut (the active one too: "Plain page" needs more than
    // 128 − 66 px), and none is cut down to a bare ellipsis.
    assert_eq!(html.matches(&format!("{ELLIPSIS}</span>")).count(), 16);
    assert!(!html.contains(&format!(r#"<span class="tab-title">{ELLIPSIS}</span>"#)));
}

/// R9: a row that matches in the part of its address an ellipsis would
/// hide shows the match, highlighted.
#[test]
fn search_results_show_why_they_matched() {
    let mut fit = TextFitter::new();
    let url = "file:///tmp/claude-1000/-home-dylon-Workspace-f1r3fly-io-F1R3Gaze/28795e59/scratchpad/f1r3gaze-ui-snapshots/site/prompt.html";
    let rows = [TabRow {
        id: 3,
        title: "Cross-site fetch",
        url,
        depth: 0,
        active: false,
        attention: TabAttention::Idle,
        page_matches: 0,
        menu_open: false,
        has_children: false,
        first: false,
        last: true,
    }];
    let plain = tab_panel_html(&mut fit, &rows, &[], "", 1).html;
    assert!(!plain.contains("<mark>"), "no highlight without a search");
    let found = tab_panel_html(&mut fit, &rows, &[], "gaze", 1).html;
    let detail = found
        .split(r#"<span class="row-detail">"#)
        .nth(1)
        .and_then(|rest| rest.split("</span>").next())
        .expect("the row's address");
    // The first occurrence ignoring case: "F1R3Gaze" in the path, before
    // "f1r3gaze-ui-snapshots".
    assert!(detail.contains("<mark>Gaze</mark>"), "{detail}");
    assert!(detail.starts_with(ELLIPSIS), "a window around the match: {detail}");
    let found = tab_panel_html(&mut fit, &rows, &[], "CROSS", 1).html;
    assert!(found.contains("<mark>Cross</mark>-site fetch"), "{found}");
    // When the title already shows why the row matched, the address keeps
    // its usual form instead of moving to its own match.
    let titled = [TabRow {
        title: "Field notes on gaze",
        ..rows[0]
    }];
    let html = tab_panel_html(&mut fit, &titled, &[], "gaze", 1).html;
    assert!(html.contains("Field notes on <mark>gaze</mark>"), "{html}");
    let detail = html
        .split(r#"<span class="row-detail">"#)
        .nth(1)
        .and_then(|rest| rest.split("</span>").next())
        .expect("the row's address");
    assert!(detail.starts_with("file://"), "the usual form: {detail}");

    let now = 2_000_000_000;
    let visits = [Visit {
        url: "https://en.wikipedia.org/wiki/Capability-based_security".into(),
        title: "Capability-based security - Wikipedia, the free encyclopedia".into(),
        at: now - 60,
    }];
    let html = history_panel_html(&mut fit, &visits, "wikipedia", now, 200, false).html;
    assert!(html.contains("<mark>Wikipedia</mark>"), "{html}");
    assert!(
        html.contains(r#"<span class="row-detail">https://en.wi"#),
        "the title shows the match, so the address keeps its usual form: {html}"
    );
    // Only the address matches: it moves to its match.
    let html = history_panel_html(&mut fit, &visits, "en.wiki", now, 200, false).html;
    assert!(html.contains("<mark>en.wiki</mark>"), "{html}");
    // A fuzzy match highlights the word that matched.
    let html = history_panel_html(&mut fit, &visits, "capabilty", now, 200, false).html;
    assert!(html.contains("<mark>Capability</mark>"), "{html}");
}

/// R4 and R13: Ctrl+L and Escape select the whole address, Ctrl+F the
/// whole query, so typing replaces the old text instead of extending it.
#[test]
fn focusing_or_reverting_a_field_selects_its_text() {
    let profile = ScratchProfile::new("select-whole");
    let url = profile.page("notes.html", NOTES);
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url);
    attach(&mut chrome, 0);
    chrome.poll(None);
    lay_out_chrome(&mut chrome);
    chrome.act(Action::FocusUrl);
    chrome.poll(None);
    assert_eq!(selected_in(&chrome, "url").as_deref(), Some(url.as_str()), "Ctrl+L");

    // Typing a query, then Escape: the address comes back, selected.
    chrome.set_input_value("url", "cap");
    chrome.act(Action::Input("url".into(), "cap".into()));
    chrome.poll(None);
    chrome.handle_ui_event(key_down(Key::Escape));
    chrome.poll(None);
    assert_eq!(chrome.raw_input_value("url"), url);
    assert_eq!(selected_in(&chrome, "url").as_deref(), Some(url.as_str()), "Escape");

    // Leaving the box restores the address without selecting anything.
    chrome.set_input_value("url", "cap");
    chrome.act(Action::Input("url".into(), "cap".into()));
    let view = chrome.tabs[0].1;
    chrome.inner.set_focus_to(view);
    chrome.act(Action::AddressDismiss);
    assert_eq!(chrome.select_pending, None, "no selection for an unfocused box");

    // Ctrl+F on an open find box selects its query.
    chrome.act(Action::Find(String::new()));
    chrome.set_input_value("find", "alpha");
    chrome.act(Action::Input("find".into(), "alpha".into()));
    chrome.act(Action::FindHide);
    chrome.act(Action::Find(String::new()));
    chrome.poll(None);
    assert_eq!(selected_in(&chrome, "find").as_deref(), Some("alpha"), "Ctrl+F");
}

/// R5: the badge shows a magnifier as soon as the box holds something other
/// than the page's address, including nothing.
#[test]
fn the_badge_turns_into_a_magnifier_when_the_address_is_edited() {
    let profile = ScratchProfile::new("badge");
    let url = profile.page("notes.html", NOTES);
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url);
    attach(&mut chrome, 0);
    chrome.poll(None);
    lay_out_chrome(&mut chrome);
    let magnifier = |chrome: &ChromeDocument| chrome.rendered["scheme"].contains("fa-magnifying-glass");
    assert!(!magnifier(&chrome), "the page's identity while unfocused");
    chrome.act(Action::FocusUrl);
    chrome.poll(None);
    assert!(!magnifier(&chrome), "focused but unedited: still the page's identity");
    chrome.set_input_value("url", "");
    chrome.act(Action::Input("url".into(), String::new()));
    chrome.poll(None);
    assert!(magnifier(&chrome), "emptied: a search");
    chrome.set_input_value("url", "cap");
    chrome.act(Action::Input("url".into(), "cap".into()));
    chrome.poll(None);
    assert!(magnifier(&chrome), "typed: a search");
}

/// R10: prompts write origins without their default port.
#[test]
fn prompts_drop_default_ports() {
    let html = prompt_bar_html(
        Some("file:///tmp/site/prompt.html"),
        "file:///tmp/site/prompt.html wants to fetch from https://example.org:443",
        1,
        2,
        1,
        false,
    );
    assert!(html.contains("<span> wants to fetch from https://example.org</span>"), "{html}");
    assert!(!html.contains(":443"), "{html}");
}

// ── Ledger L8: pointer feedback over pages ──────────────────────────────

/// What the chrome asks of its window, as blitz-shell's provider would
/// receive it: every cursor request in order, and the redraw requests.
#[derive(Default)]
struct WindowLog {
    cursors: Mutex<Vec<Option<CursorIcon>>>,
    redraws: AtomicUsize,
}

impl WindowLog {
    fn cursors(&self) -> Vec<Option<CursorIcon>> {
        self.cursors.lock().expect("the cursor log").clone()
    }
    /// What the window shows: `None` before any request, `Some(None)` while
    /// the cursor is hidden.
    fn shown(&self) -> Option<Option<CursorIcon>> {
        self.cursors.lock().expect("the cursor log").last().copied()
    }
    fn redraws(&self) -> usize {
        self.redraws.load(Ordering::SeqCst)
    }
}

impl ShellProvider for WindowLog {
    fn set_cursor(&self, icon: Option<CursorIcon>) {
        self.cursors.lock().expect("the cursor log").push(icon);
    }
    fn request_redraw(&self) {
        self.redraws.fetch_add(1, Ordering::SeqCst);
    }
}

/// `#go`'s background while it is hovered (`#go:hover`).
const HOVERED: &str = "rgb(204, 0, 0)";

/// A page for the cursor tests, stacked at the top left: a text-free block,
/// a link, a line of text, and a block whose CSS hides the cursor. With
/// `link_first` the link and the block swap places, so this page puts its
/// link where the other variant has its plain block.
fn cursor_page(title: &str, link_first: bool) -> String {
    let (first, second) = match link_first {
        true => (r#"<a id="go" href="next.html"></a>"#, r#"<div id="plain"></div>"#),
        false => (r#"<div id="plain"></div>"#, r#"<a id="go" href="next.html"></a>"#),
    };
    format!(
        "<html><head><title>{title}</title><style>\
         body{{margin:0;font:16px sans-serif;background:#fff}}\
         #plain{{width:240px;height:40px}}\
         #go{{display:block;width:240px;height:40px;background:#eeeeee}}\
         #go:hover{{background:#cc0000}}\
         #text{{display:inline-block;margin:0;line-height:40px}}\
         #nocursor{{width:240px;height:40px;cursor:none}}\
         </style></head><body>{first}{second}<p id=\"text\">Words to hover over</p>\
         <div id=\"nocursor\"></div></body></html>"
    )
}

/// A chrome showing `url` whose window is a `WindowLog`. It is installed
/// after the document is built, as blitz-shell's `View::init` installs its
/// own provider.
fn chrome_with_window(profile: &ScratchProfile, url: &str) -> (ChromeDocument, Arc<WindowLog>) {
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), url);
    let window = Arc::new(WindowLog::default());
    chrome.inner.set_shell_provider(window.clone());
    (chrome, window)
}

/// Poll until tab `i` shows a document titled `title` (bounded), then lay
/// the window out, which lays the page out at its view's size.
fn show(chrome: &mut ChromeDocument, i: usize, title: &str) {
    let view = chrome.tabs[i].1;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        chrome.poll(None);
        if rho_mut(&mut chrome.inner, view).is_some_and(|page| page.title() == title) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "tab {i} did not show {title:?} within 10 s"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    lay_out_chrome(chrome);
}

/// The centre of a chrome element, in window coordinates.
fn point_in_chrome(chrome: &ChromeDocument, selector: &str) -> (f32, f32) {
    let node = chrome
        .inner
        .query_selector(selector)
        .expect("a valid selector")
        .unwrap_or_else(|| panic!("nothing matches {selector}"));
    let rect = chrome
        .inner
        .get_client_bounding_rect(node)
        .expect("the element is laid out");
    (
        (rect.x + rect.width / 2.0) as f32,
        (rect.y + rect.height / 2.0) as f32,
    )
}

/// The centre of an element of tab `i`'s page, in window coordinates.
fn point_in_page(chrome: &mut ChromeDocument, i: usize, selector: &str) -> (f32, f32) {
    let view = chrome.tabs[i].1;
    let origin = chrome
        .inner
        .get_client_bounding_rect(view)
        .expect("the view is laid out");
    let base = rho_mut(&mut chrome.inner, view).expect("attached").base();
    let doc = base.borrow();
    let node = doc
        .query_selector(selector)
        .expect("a valid selector")
        .unwrap_or_else(|| panic!("the page has no {selector}"));
    let rect = doc
        .get_client_bounding_rect(node)
        .expect("the page is laid out");
    (
        (origin.x + rect.x + rect.width / 2.0) as f32,
        (origin.y + rect.y + rect.height / 2.0) as f32,
    )
}

/// Move the pointer to the centre of an element of tab `i`'s page.
fn hover_page(chrome: &mut ChromeDocument, i: usize, selector: &str) {
    let point = point_in_page(chrome, i, selector);
    move_to(chrome, point);
}

/// Move the pointer to the centre of a chrome element.
fn hover_chrome(chrome: &mut ChromeDocument, selector: &str) {
    let point = point_in_chrome(chrome, selector);
    move_to(chrome, point);
}

fn move_to(chrome: &mut ChromeDocument, (x, y): (f32, f32)) {
    chrome.handle_ui_event(pointer(UiEvent::PointerMove, x, y, MouseEventButtons::None));
}

fn click_at(chrome: &mut ChromeDocument, (x, y): (f32, f32)) {
    move_to(chrome, (x, y));
    chrome.handle_ui_event(pointer(UiEvent::PointerDown, x, y, MouseEventButtons::Primary));
    chrome.handle_ui_event(pointer(UiEvent::PointerUp, x, y, MouseEventButtons::None));
}

/// The cursor tab `i`'s page itself would show.
fn page_cursor(chrome: &mut ChromeDocument, i: usize) -> Option<CursorIcon> {
    let view = chrome.tabs[i].1;
    let base = rho_mut(&mut chrome.inner, view).expect("attached").base();
    base.borrow().get_cursor()
}

/// The `id` of the element under the pointer in tab `i`'s page, if any
/// (`""` for an element without one).
fn page_hover(chrome: &mut ChromeDocument, i: usize) -> Option<String> {
    let view = chrome.tabs[i].1;
    let base = rho_mut(&mut chrome.inner, view).expect("attached").base();
    let doc = base.borrow();
    doc.get_hover_node_id().map(|id| {
        doc.get_node(id)
            .and_then(|node| node.attr(LocalName::from("id")))
            .unwrap_or_default()
            .to_string()
    })
}

/// A computed style value of an element of tab `i`'s page.
fn page_style(chrome: &mut ChromeDocument, i: usize, selector: &str, property: &str) -> String {
    let view = chrome.tabs[i].1;
    let base = rho_mut(&mut chrome.inner, view).expect("attached").base();
    let doc = base.borrow();
    let node = doc
        .query_selector(selector)
        .expect("a valid selector")
        .unwrap_or_else(|| panic!("the page has no {selector}"));
    doc.resolved_style_value(node, property)
}

/// L8/H1: the pointer goes from Back into the page that Back has just
/// loaded. That page has never been under the pointer, and the cursor must
/// stay visible.
#[test]
fn entering_a_fresh_page_never_hides_the_cursor() {
    let profile = ScratchProfile::new("cursor-fresh");
    let first = profile.document("first.html", &cursor_page("First", false));
    let second = profile.document("second.html", &cursor_page("Second", false));
    let (mut chrome, window) = chrome_with_window(&profile, &first);
    show(&mut chrome, 0, "First");
    chrome.act(Action::Go(Some(second)));
    show(&mut chrome, 0, "Second");
    let back = point_in_chrome(&chrome, "#back");
    click_at(&mut chrome, back);
    // Back loads a new document for the first page.
    show(&mut chrome, 0, "First");
    hover_page(&mut chrome, 0, "#plain");
    assert_eq!(
        window.shown(),
        Some(Some(CursorIcon::Default)),
        "an arrow over a plain block of a page Back just loaded; the window was asked for {:?}",
        window.cursors()
    );
}

/// L8/H2: inside a page the cursor follows the element under the pointer: a
/// hand over a link, a text cursor over text.
#[test]
fn the_cursor_follows_links_and_text_inside_a_page() {
    let profile = ScratchProfile::new("cursor-follow");
    let url = profile.document("first.html", &cursor_page("First", false));
    let (mut chrome, window) = chrome_with_window(&profile, &url);
    show(&mut chrome, 0, "First");
    hover_page(&mut chrome, 0, "#plain");
    let entered = window.cursors();
    hover_page(&mut chrome, 0, "#go");
    assert_eq!(
        page_cursor(&mut chrome, 0),
        Some(CursorIcon::Pointer),
        "control: the page itself would show a hand over its link"
    );
    assert_eq!(
        window.shown(),
        Some(Some(CursorIcon::Pointer)),
        "a hand over the link; entering the page asked the window for {entered:?}, and the link then for {:?}",
        window.cursors()
    );
    hover_page(&mut chrome, 0, "#text");
    assert_eq!(
        window.shown(),
        Some(Some(CursorIcon::Text)),
        "a text cursor over text; the window was asked for {:?}",
        window.cursors()
    );
}

/// L8/H3: leaving a page for the browser's own controls ends the page's
/// hover. Its link loses `:hover`, and coming back over a plain block shows
/// the arrow, whatever the window showed in between.
#[test]
fn leaving_a_page_ends_its_hover() {
    let profile = ScratchProfile::new("cursor-leave");
    let url = profile.document("first.html", &cursor_page("First", false));
    let (mut chrome, window) = chrome_with_window(&profile, &url);
    show(&mut chrome, 0, "First");
    hover_page(&mut chrome, 0, "#go");
    lay_out_chrome(&mut chrome);
    assert_eq!(
        page_style(&mut chrome, 0, "#go", "background-color"),
        HOVERED,
        "control: the link is hovered"
    );
    hover_chrome(&mut chrome, "#rail");
    lay_out_chrome(&mut chrome);
    assert_eq!(
        page_hover(&mut chrome, 0),
        None,
        "nothing in the page is under the pointer once it is over the rail"
    );
    assert_ne!(
        page_style(&mut chrome, 0, "#go", "background-color"),
        HOVERED,
        "the link lost :hover"
    );
    hover_page(&mut chrome, 0, "#plain");
    assert_eq!(
        window.shown(),
        Some(Some(CursorIcon::Default)),
        "an arrow over the plain block on coming back; the window was asked for {:?}",
        window.cursors()
    );
}

/// L8/H4: a hover change inside a page gets painted. Moving onto a link with
/// a `:hover` rule must lead to a redraw, either asked of the window at once
/// or reported by `poll`, which blitz-shell turns into one.
#[test]
fn hover_changes_inside_a_page_are_repainted() {
    let profile = ScratchProfile::new("cursor-repaint");
    let url = profile.document("first.html", &cursor_page("First", false));
    let (mut chrome, window) = chrome_with_window(&profile, &url);
    show(&mut chrome, 0, "First");
    hover_page(&mut chrome, 0, "#plain");
    chrome.poll(None);
    let before = window.redraws();
    hover_page(&mut chrome, 0, "#go");
    let asked = window.redraws() - before;
    let polled = chrome.poll(None);
    assert!(
        asked > 0 || polled,
        "moving onto the link led to no redraw: {asked} requests, poll() = {polled}"
    );
}

/// L8: a page that loads under a resting pointer gets its cursor without
/// the pointer moving. Here its link lands where the previous page had a
/// plain block.
#[test]
fn a_page_loaded_under_a_resting_pointer_gets_its_cursor() {
    let profile = ScratchProfile::new("cursor-rest");
    let first = profile.document("first.html", &cursor_page("First", false));
    let second = profile.document("second.html", &cursor_page("Second", true));
    let (mut chrome, window) = chrome_with_window(&profile, &first);
    show(&mut chrome, 0, "First");
    hover_page(&mut chrome, 0, "#plain");
    chrome.act(Action::Go(Some(second)));
    show(&mut chrome, 0, "Second");
    chrome.poll(None);
    assert_eq!(
        window.shown(),
        Some(Some(CursorIcon::Pointer)),
        "a hand over the new page's link, with no pointer movement; the window was asked for {:?}",
        window.cursors()
    );
    // Between attaching and its first layout the new page has nothing
    // hovered. Blitz answers `None` for that, which must not hide the cursor.
    assert!(
        !window.cursors().contains(&None),
        "the cursor was never hidden while the page loaded; the window was asked for {:?}",
        window.cursors()
    );
}

/// L8: switching tabs under a resting pointer (here by keyboard) shows the
/// new page's cursor without the pointer moving. Its link is where the other
/// page has a plain block.
#[test]
fn switching_tabs_under_a_resting_pointer_shows_the_new_page_cursor() {
    let profile = ScratchProfile::new("cursor-switch");
    let first = profile.document("first.html", &cursor_page("First", false));
    let second = profile.document("second.html", &cursor_page("Second", true));
    let (mut chrome, window) = chrome_with_window(&profile, &first);
    show(&mut chrome, 0, "First");
    chrome.open_tab(&second);
    show(&mut chrome, 1, "Second");
    chrome.act(Action::Select(chrome.tabs[0].0.id));
    lay_out_chrome(&mut chrome);
    hover_page(&mut chrome, 0, "#plain");
    assert_eq!(window.shown(), Some(Some(CursorIcon::Default)), "control: the arrow");
    chrome.act(Action::Select(chrome.tabs[1].0.id));
    // The window lays itself out again, and polls once a frame has been drawn.
    lay_out_chrome(&mut chrome);
    chrome.poll(None);
    assert_eq!(
        window.shown(),
        Some(Some(CursorIcon::Pointer)),
        "a hand over the newly shown page's link; the window was asked for {:?}",
        window.cursors()
    );
    assert_eq!(page_hover(&mut chrome, 0).as_deref(), None, "the hidden page hovers nothing");
}

/// L8: a page may hide the cursor over its own elements with
/// `cursor: none`, as on the web, and the cursor returns when the pointer
/// leaves them.
#[test]
fn css_can_hide_the_cursor_over_page_elements() {
    let profile = ScratchProfile::new("cursor-none");
    let url = profile.document("first.html", &cursor_page("First", false));
    let (mut chrome, window) = chrome_with_window(&profile, &url);
    show(&mut chrome, 0, "First");
    hover_page(&mut chrome, 0, "#plain");
    hover_page(&mut chrome, 0, "#nocursor");
    assert_eq!(
        window.shown(),
        Some(None),
        "hidden over `cursor: none`; the window was asked for {:?}",
        window.cursors()
    );
    hover_page(&mut chrome, 0, "#plain");
    assert_eq!(
        window.shown(),
        Some(Some(CursorIcon::Default)),
        "back once the pointer leaves it; the window was asked for {:?}",
        window.cursors()
    );
}

/// L8: only the page under the pointer decides the cursor. A page in a
/// background tab whose hover changes (for example when it is laid out
/// again) leaves the cursor alone.
#[test]
fn background_pages_cannot_change_the_cursor() {
    let profile = ScratchProfile::new("cursor-background");
    let first = profile.document("first.html", &cursor_page("First", false));
    let second = profile.document("second.html", &cursor_page("Second", false));
    let (mut chrome, window) = chrome_with_window(&profile, &first);
    show(&mut chrome, 0, "First");
    chrome.open_tab(&second);
    show(&mut chrome, 1, "Second");
    chrome.act(Action::Select(chrome.tabs[0].0.id));
    lay_out_chrome(&mut chrome);
    hover_page(&mut chrome, 0, "#plain");
    // The background page, laid out on its own, gets its `cursor: none`
    // block under its last pointer position.
    let view = chrome.tabs[1].1;
    lay_out_page(&mut chrome, view);
    {
        let base = rho_mut(&mut chrome.inner, view).expect("attached").base();
        let mut doc = base.borrow_mut();
        let node = doc
            .query_selector("#nocursor")
            .expect("a valid selector")
            .expect("the page has #nocursor");
        let rect = doc.get_client_bounding_rect(node).expect("laid out");
        doc.set_hover_to(
            (rect.x + rect.width / 2.0) as f32,
            (rect.y + rect.height / 2.0) as f32,
        );
    }
    chrome.poll(None);
    assert_eq!(
        window.shown(),
        Some(Some(CursorIcon::Default)),
        "the arrow of the page under the pointer; the window was asked for {:?}",
        window.cursors()
    );
}

/// L8: moving within one element asks nothing of the window.
#[test]
fn moving_within_an_element_asks_nothing_of_the_window() {
    let profile = ScratchProfile::new("cursor-still");
    let url = profile.document("first.html", &cursor_page("First", false));
    let (mut chrome, window) = chrome_with_window(&profile, &url);
    show(&mut chrome, 0, "First");
    let (x, y) = point_in_page(&mut chrome, 0, "#plain");
    move_to(&mut chrome, (x, y));
    let asked = window.cursors().len();
    for dx in [-60.0, -30.0, 30.0, 60.0] {
        move_to(&mut chrome, (x + dx, y));
    }
    assert_eq!(
        window.cursors().len(),
        asked,
        "moves within one block changed nothing; the window was asked for {:?}",
        window.cursors()
    );
}

/// L8: the built-in pages' buttons show the hand, like the chrome's, even
/// over their label text (Blitz's default style sheet sets no `cursor`).
#[test]
fn built_in_buttons_show_the_hand() {
    let profile = ScratchProfile::new("cursor-lamp");
    let (mut chrome, window) = chrome_with_window(&profile, "gaze://newtab");
    show(&mut chrome, 0, "New tab");
    hover_page(&mut chrome, 0, "#lamp span");
    assert_eq!(
        window.shown(),
        Some(Some(CursorIcon::Pointer)),
        "a hand over the lamp's label; the window was asked for {:?}",
        window.cursors()
    );
}

// ── Ledger L11: the lamp lights ─────────────────────────────────────────

/// A `#rrggbb` colour as computed styles report it: `rgb(r, g, b)`.
fn css_rgb(hex: &str) -> String {
    let channel = |at: usize| {
        u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or_else(|_| panic!("{hex} is not #rrggbb"))
    };
    format!("rgb({}, {}, {})", channel(1), channel(3), channel(5))
}

/// What the lamp on tab 0's new-tab page shows: its class, and the computed
/// colours of its fill, its label and its edge.
#[derive(Debug, PartialEq)]
struct LampLook {
    class: String,
    fill: String,
    label: String,
    edge: String,
}

impl LampLook {
    fn of(chrome: &mut ChromeDocument) -> LampLook {
        let class = {
            let view = chrome.tabs[0].1;
            let base = rho_mut(&mut chrome.inner, view).expect("attached").base();
            let doc = base.borrow();
            let lamp = doc
                .query_selector("#lamp")
                .expect("a valid selector")
                .expect("the new-tab page has #lamp");
            doc.get_node(lamp)
                .and_then(|node| node.attr(LocalName::from("class")))
                .unwrap_or_default()
                .to_string()
        };
        LampLook {
            class,
            fill: page_style(chrome, 0, "#lamp", "background-color"),
            label: page_style(chrome, 0, "#lamp span", "color"),
            edge: page_style(chrome, 0, "#lamp", "border-top-color"),
        }
    }

    /// The look `class` should have in `scheme`: lit in the lamp's own
    /// colours, unlit in the scheme's button colours.
    fn expected(scheme: &str, class: &str) -> LampLook {
        let colors = theme::palette(scheme, None);
        let [fill, label, edge] = match class {
            "lit" => [theme::LAMP_LIT_FILL, theme::LAMP_LIT_LABEL, theme::LAMP_LIT_EDGE],
            _ => ["--gaze-surface", "--gaze-text", "--gaze-border"].map(|token| colors[token].as_str()),
        };
        LampLook {
            class: class.to_string(),
            fill: css_rgb(fill),
            label: css_rgb(label),
            edge: css_rgb(edge),
        }
    }
}

/// Poll until tab 0's f1r3lang program listens for clicks (bounded). When the
/// page's title appears, the program is not listening yet: it registers its
/// listener a few polls later (four, measured in ledger L11). A click before
/// that reaches no listener, as on the web before a page's script has run.
fn wait_until_listening(chrome: &mut ChromeDocument) {
    let view = chrome.tabs[0].1;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !rho_mut(&mut chrome.inner, view)
        .and_then(|page| page.tab())
        .is_some_and(|program| program.dom.has_listener("click"))
    {
        assert!(
            Instant::now() < deadline,
            "the page's program did not listen for clicks within 10 s"
        );
        chrome.poll(None);
        std::thread::sleep(Duration::from_millis(5));
    }
    lay_out_chrome(chrome);
}

/// Click the lamp, then poll until its f1r3lang program has set its class
/// to `class` (bounded), and lay the window out so the page is restyled.
fn click_lamp(chrome: &mut ChromeDocument, class: &str) {
    let point = point_in_page(chrome, 0, "#lamp");
    click_at(chrome, point);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        chrome.poll(None);
        lay_out_chrome(chrome);
        let now = LampLook::of(chrome).class;
        if now == class {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the lamp's class stayed {now:?} for 10 s, not {class:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// L11: each click on the new-tab page's lamp switches it between lit and
/// unlit, in both schemes. The host theme's `button` rule is an important
/// user-agent declaration, which beat the page's own `button.lit`: the class
/// changed and the lamp looked the same.
#[test]
fn the_lamp_lights_and_goes_out_in_both_schemes() {
    for scheme in ["dark", "light"] {
        let profile = ScratchProfile::new(&format!("lamp-{scheme}"));
        UiState {
            theme: scheme.into(),
            ..UiState::default()
        }
        .save(profile.path())
        .expect("save the workspace");
        let (mut chrome, _window) = chrome_with_window(&profile, "gaze://newtab");
        show(&mut chrome, 0, "New tab");
        wait_until_listening(&mut chrome);
        let unlit = LampLook::expected(scheme, "");
        let lit = LampLook::expected(scheme, "lit");
        assert_ne!(lit.fill, unlit.fill, "{scheme}: the lit lamp has a fill of its own");
        assert_eq!(LampLook::of(&mut chrome), unlit, "{scheme}: unlit before any click");
        click_lamp(&mut chrome, "lit");
        assert_eq!(LampLook::of(&mut chrome), lit, "{scheme}: lit after one click");
        click_lamp(&mut chrome, "");
        assert_eq!(LampLook::of(&mut chrome), unlit, "{scheme}: out again after two");
    }
}

// ── Sidebar at startup ──────────────────────────────────────────────────

fn sidebar_hidden(chrome: &ChromeDocument) -> bool {
    let sidebar = chrome.id("sidebar").expect("chrome has #sidebar");
    chrome
        .inner
        .get_node(sidebar)
        .and_then(|node| node.attr(LocalName::from("class")))
        .is_some_and(|class| class.split_whitespace().any(|c| c == "hidden"))
}

/// A window starts with the sidebar collapsed even when the last one closed
/// with it open, and Ctrl+B then reopens the panel last shown.
#[test]
fn the_sidebar_starts_collapsed() {
    let profile = ScratchProfile::new("sidebar-start");
    let url = profile.page("notes.html", NOTES);
    UiState {
        sidebar_open: true,
        panel: "appearance".into(),
        ..UiState::default()
    }
    .save(profile.path())
    .expect("save the workspace");
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url);
    attach(&mut chrome, 0);
    chrome.poll(None);
    assert!(!chrome.ui.sidebar_open && sidebar_hidden(&chrome), "collapsed at start");
    chrome.act(Action::SidebarToggle);
    chrome.poll(None);
    assert!(chrome.ui.sidebar_open && !sidebar_hidden(&chrome), "Ctrl+B opens it");
    assert_eq!(chrome.panel, "appearance", "on the panel last shown");
}

/// With `restore_sidebar = true`, a window reopens the sidebar as the last
/// one left it.
#[test]
fn restore_sidebar_reopens_it() {
    let profile = ScratchProfile::new("sidebar-restore");
    profile.restore_sidebar();
    let url = profile.page("notes.html", NOTES);
    UiState {
        sidebar_open: true,
        panel: "appearance".into(),
        ..UiState::default()
    }
    .save(profile.path())
    .expect("save the workspace");
    let mut chrome = ChromeDocument::new(Engine::new(profile.path().to_path_buf()), &url);
    attach(&mut chrome, 0);
    chrome.poll(None);
    assert!(chrome.ui.sidebar_open && !sidebar_hidden(&chrome), "open as it was left");
    assert_eq!(chrome.panel, "appearance");
}

// ── Ledger L9: resizing the window ──────────────────────────────────────

/// A page split into two halves side by side (a flex row: Blitz does not
/// float boxes beside each other). A window point near the middle of the
/// view falls in a different half once the window's width changes.
fn halves_page(title: &str) -> String {
    format!(
        "<html><head><title>{title}</title><style>body{{margin:0;display:flex}}\
         #left,#right{{flex:1;height:300px}}</style></head>\
         <body><div id=\"left\"></div><div id=\"right\"></div></body></html>"
    )
}

/// The window point `fraction` of the way across tab `i`'s view, 100 px below
/// its top.
fn point_across_view(chrome: &ChromeDocument, i: usize, fraction: f64) -> (f32, f32) {
    let view = chrome
        .inner
        .get_client_bounding_rect(chrome.tabs[i].1)
        .expect("the view is laid out");
    (
        (view.x + view.width * fraction) as f32,
        (view.y + 100.0) as f32,
    )
}

/// One frame as the window paints it, without drawing the scene: the window's
/// application handler brackets blitz-shell's redraw with `begin_paint` and
/// `end_paint`, and the redraw lays the chrome out with its pages.
fn paint(chrome: &mut ChromeDocument) {
    chrome.begin_paint();
    chrome.inner.resolve(0.0);
    chrome.end_paint();
}

/// A new window size, as blitz-shell applies it before the frame for it
/// (`View::with_viewport`).
fn set_window_size(chrome: &mut ChromeDocument, width: u32, height: u32) {
    chrome.inner.viewport_mut().window_size = (width, height);
}

/// The page's viewport width, in physical pixels (the tests' scale is 1).
fn page_viewport_width(chrome: &mut ChromeDocument, i: usize) -> u32 {
    let view = chrome.tabs[i].1;
    let base = rho_mut(&mut chrome.inner, view).expect("attached").base();
    base.borrow().viewport().window_size.0
}

/// L9/H8, the mechanism. Blitz re-resolves hover at the pointer's last
/// position after every layout (`refresh_hover`). A window narrowed under
/// that position hovers whatever slides under it, although the pointer has
/// not moved.
#[test]
fn a_relayout_rehovers_under_the_last_pointer_position() {
    let profile = ScratchProfile::new("hover-reflow");
    let url = profile.document("halves.html", &halves_page("Halves"));
    let (mut chrome, _window) = chrome_with_window(&profile, &url);
    show(&mut chrome, 0, "Halves");
    let point = point_across_view(&chrome, 0, 0.45);
    move_to(&mut chrome, point);
    assert_eq!(page_hover(&mut chrome, 0).as_deref(), Some("left"));
    lay_out_chrome_at(&mut chrome, 900, 800);
    assert_eq!(
        page_hover(&mut chrome, 0).as_deref(),
        Some("right"),
        "the right half slid under the pointer's last position"
    );
}

/// L9/H8: once the mouse has left the window, nothing in it is hovered.
/// Resizing the window then hovers nothing and asks for no frame beyond the
/// one for each size, until the pointer comes back.
#[test]
fn a_window_the_mouse_left_hovers_nothing_while_it_is_resized() {
    let profile = ScratchProfile::new("hover-left");
    let url = profile.document("halves.html", &halves_page("Halves"));
    let (mut chrome, window) = chrome_with_window(&profile, &url);
    show(&mut chrome, 0, "Halves");
    let point = point_across_view(&chrome, 0, 0.45);
    move_to(&mut chrome, point);
    assert_eq!(page_hover(&mut chrome, 0).as_deref(), Some("left"));
    chrome.pointer_left();
    assert_eq!(page_hover(&mut chrome, 0), None, "the page's hover ended");
    assert_eq!(chrome.inner.get_hover_node_id(), None, "and the chrome's");
    // Ending the hover is itself painted once.
    paint(&mut chrome);
    chrome.cursor.take_repaint();
    for width in [900, 1100, 1280] {
        set_window_size(&mut chrome, width, 800);
        let redraws = window.redraws();
        paint(&mut chrome);
        assert_eq!(page_hover(&mut chrome, 0), None, "nothing hovered again at {width} px");
        assert!(
            !chrome.cursor.take_repaint(),
            "no page asked for another frame at {width} px"
        );
        assert_eq!(
            window.redraws(),
            redraws,
            "no other frame was asked for at {width} px"
        );
    }
    move_to(&mut chrome, point);
    assert_eq!(
        page_hover(&mut chrome, 0).as_deref(),
        Some("left"),
        "the pointer coming back hovers again"
    );
}

/// L9/H9, the mechanism. The chrome's layout pass gives a page its new
/// viewport, and the page asks for a frame (`queue_device_changes`), although
/// the same pass has laid it out at the new size.
#[test]
fn a_page_asks_for_a_frame_when_its_viewport_changes() {
    let profile = ScratchProfile::new("viewport-request");
    let url = profile.document("halves.html", &halves_page("Halves"));
    let (mut chrome, _window) = chrome_with_window(&profile, &url);
    show(&mut chrome, 0, "Halves");
    chrome.cursor.take_repaint();
    lay_out_chrome_at(&mut chrome, 900, 800);
    assert!(chrome.cursor.take_repaint(), "the page asked for a frame");
    let view = chrome
        .inner
        .get_client_bounding_rect(chrome.tabs[0].1)
        .expect("the view is laid out");
    assert_eq!(
        f64::from(page_viewport_width(&mut chrome, 0)),
        view.width,
        "though the pass laid it out at its new width"
    );
}

/// L9/H9: a frame answers its pages' requests for itself. A page left a
/// restyle, by a hover that changed under a resting pointer, still gets one
/// more frame, and only one.
#[test]
fn a_frame_answers_its_pages_unless_one_is_left_a_restyle() {
    let profile = ScratchProfile::new("paint-answers");
    let url = profile.document("halves.html", &halves_page("Halves"));
    let (mut chrome, window) = chrome_with_window(&profile, &url);
    show(&mut chrome, 0, "Halves");
    chrome.cursor.take_repaint();
    // A new size alone: the frame for it answers the page.
    set_window_size(&mut chrome, 900, 800);
    let redraws = window.redraws();
    paint(&mut chrome);
    assert!(!chrome.cursor.take_repaint(), "no repaint is left for a poll");
    assert_eq!(window.redraws(), redraws, "and no other frame is asked for");
    // A pointer resting over the page, and a new size that slides the other
    // half under it.
    set_window_size(&mut chrome, 1280, 800);
    paint(&mut chrome);
    let point = point_across_view(&chrome, 0, 0.45);
    move_to(&mut chrome, point);
    assert_eq!(page_hover(&mut chrome, 0).as_deref(), Some("left"));
    chrome.cursor.take_repaint();
    set_window_size(&mut chrome, 900, 800);
    let redraws = window.redraws();
    paint(&mut chrome);
    assert_eq!(page_hover(&mut chrome, 0).as_deref(), Some("right"));
    assert_eq!(
        window.redraws(),
        redraws + 1,
        "one more frame, for the restyle the hover left"
    );
    let redraws = window.redraws();
    paint(&mut chrome);
    assert_eq!(
        window.redraws(),
        redraws,
        "that frame asks for no further one"
    );
}
