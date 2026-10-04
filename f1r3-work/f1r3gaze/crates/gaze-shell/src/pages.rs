//! Built-in pages (`gaze://…`) and the page shown when a load fails.
//!
//! Labels are always elements (`<button><span>lamp</span></button>`), never
//! bare text directly inside a button: Blitz never restyles the anonymous
//! box such text gets, so it would keep its colour across a theme switch
//! (docs/ui/ledger.md, L2).
//!
//! Buttons show the hand, as the chrome's do. Blitz's default style sheet
//! sets no `cursor` for buttons, which would leave a text cursor over the
//! label (L8). Links already get the hand from Blitz.

use crate::display::{failure_reason, failure_summary};
use crate::engine::escape;

const STYLE: &str = "body{font-family:system-ui,sans-serif;margin:48px auto;max-width:680px;padding:0 24px;color:#1d2330;line-height:1.5}
h1{font-weight:600}code{background:#eef1f6;padding:1px 4px;border-radius:3px}code.url{overflow-wrap:anywhere}
button{font:inherit;padding:8px 16px;border:1px solid #9aa4b5;border-radius:6px;background:#fff;cursor:pointer}
button.lit{background:#ffcf3f;border-color:#c79a00}.muted{color:#667085}
a.btn{display:inline-block;padding:8px 16px;border:1px solid #9aa4b5;border-radius:6px;background:#fff;color:#1d2330;text-decoration:none}
summary{cursor:pointer;color:#667085}details p{margin:8px 0 0}";

/// The title of the page shown when a load fails (also the tab's title).
pub const ERROR_TITLE: &str = "Can't open this page";

pub fn builtin(url: &str) -> Option<String> {
    let page = url.strip_prefix("gaze://")?.split(['/', '?', '#']).next()?;
    Some(match page {
        "newtab" => format!(
            r##"<html><head><title>New tab</title><style>{STYLE}</style></head><body>
<h1>F1R3Gaze</h1>
<p>A browser whose only execution mechanism is f1r3lang on a native RSpace. Type an address above:
<code>https://…</code>, a shard site <code>f1r3://publisher/project/</code>, or content by hash
<code>f1r3h://blake2b-256/…</code>.</p>
<p>This page runs a f1r3lang program. It holds one capability, the document:</p>
<p><button id="lamp"><span>lamp</span></button></p>
<p class="muted">Pages with JavaScript are shown without it. <a href="gaze://about">About F1R3Gaze</a></p>
<script type="application/f1r3lang" imports="doc">
new found, sub, clicks, on, off in {{
  doc!("query1", "#lamp", *found) |
  for (@("ok", *lamp) <- found) {{
    lamp!("listen", "click", *clicks, {{}}, *sub) |
    off!(Nil) |
    for (_ <= clicks & _ <- off) {{ lamp!("classAdd", "lit") | on!(Nil) }} |
    for (_ <= clicks & _ <- on)  {{ lamp!("classRemove", "lit") | off!(Nil) }}
  }}
}}
</script></body></html>"##
        ),
        "about" => format!(
            r#"<html><head><title>About F1R3Gaze</title><style>{STYLE}</style></head><body>
<h1>F1R3Gaze {}</h1>
<p>Document engine: Blitz. Executive: CampF1R3. License: Apache-2.0.</p>
<p>Settings live in <code>settings.conf</code> in the profile directory
(set <code>F1R3GAZE_PROFILE</code> to move it). Shard observers, the validator,
the quorum and blob mirrors are configured there.</p>
<p class="muted">Not in this release: graded and K2 pages (work packages U2, U5), the proof rung
(node work package N1), legacy JavaScript tabs, devtools time travel.</p></body></html>"#,
            env!("CARGO_PKG_VERSION")
        ),
        _ => return None,
    })
}

/// The page shown when `url` could not be loaded because of `why`: one
/// sentence saying what went wrong, the address once, a link that tries
/// again, and the technical reason behind a disclosure.
pub fn error(url: &str, why: &str) -> String {
    let url_html = escape(url);
    format!(
        r#"<html><head><title>{ERROR_TITLE}</title><style>{STYLE}</style></head><body>
<h1>{ERROR_TITLE}</h1>
<p>{summary}</p>
<p><code class="url">{url_html}</code></p>
<p><a class="btn" href="{url_html}"><span>Try again</span></a></p>
<details><summary>Details</summary><p class="muted">{reason}</p></details>
</body></html>"#,
        summary = escape(failure_summary(why)),
        reason = escape(&failure_reason(url, why)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::stale_text_colours;
    use crate::theme;
    use gaze_dom_blitz::RhoDocument;

    #[test]
    fn error_page_names_the_problem_once_and_offers_a_retry() {
        let url = "https://docs.example.invalid/a?b=<c>";
        let why = format!("network error: {url}: Dns Failed: resolve dns name");
        let html = error(url, &why);
        assert!(html.contains("<title>Can't open this page</title>"));
        assert!(html.contains("The server's address could not be found."));
        // The address is shown once as text and once as the retry link.
        assert_eq!(html.matches("https://docs.example.invalid/a?b=&lt;c&gt;").count(), 2);
        assert!(html.contains(r#"<a class="btn" href="https://docs.example.invalid/a?b=&lt;c&gt;">"#));
        assert!(html.contains("network error: Dns Failed: resolve dns name"));
        assert!(!html.contains("<c>"), "the URL is escaped");
    }

    /// Ledger L2 for built-in pages: swapping the host theme must recolour
    /// every label, including the new-tab page's button.
    #[test]
    fn host_theme_swap_leaves_no_stale_label_colours() {
        let html = builtin("gaze://newtab").expect("the new-tab page exists");
        let mut page = RhoDocument::from_html(&html, blitz_dom::DocumentConfig::default());
        let lay_out = |page: &RhoDocument| {
            let base = page.base();
            base.borrow_mut().viewport_mut().window_size = (800, 600);
            base.borrow_mut().resolve(0.0);
        };
        page.set_host_theme(&theme::builtin_css("dark", None));
        lay_out(&page);
        page.set_host_theme(&theme::builtin_css("light", None));
        lay_out(&page);
        let stale = stale_text_colours(&page.base().borrow());
        assert!(stale.is_empty(), "stale labels after the theme swap: {stale:?}");
    }
}
