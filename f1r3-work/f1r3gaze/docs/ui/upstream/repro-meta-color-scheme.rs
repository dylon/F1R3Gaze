// Reproduction for blitz-meta-color-scheme.md. A standalone binary crate:
//
//   [dependencies]
//   blitz-dom = { git = "https://github.com/DioxusLabs/blitz", rev = "2335458530518cdf167c55ce635fae99323e0789" }
//   blitz-html = { git = "https://github.com/DioxusLabs/blitz", rev = "2335458530518cdf167c55ce635fae99323e0789" }
//   blitz-traits = { git = "https://github.com/DioxusLabs/blitz", rev = "2335458530518cdf167c55ce635fae99323e0789" }
//
// Measured with these crates as path dependencies on a clone of that revision
// (main on 2026-10-06), and on 674d7d2: the output is identical.

//! Does `<meta name="color-scheme" content="light">` keep a page light when
//! the user prefers dark? Compared with the same declaration made in CSS.
//! The viewport prefers dark; `#p` is painted with system colours
//! (`CanvasText` on `Canvas`), and `<mark>` with blitz-dom's default sheet,
//! which also uses them.
use blitz_dom::{BaseDocument, DEFAULT_CSS, DocumentConfig};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};

fn document(html: &str) -> BaseDocument {
    let config = DocumentConfig {
        viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Dark)),
        ua_stylesheets: Some(vec![DEFAULT_CSS.to_string()]),
        ..Default::default()
    };
    let mut doc = HtmlDocument::from_html(html, config).into_inner();
    doc.resolve(0.0);
    doc
}

fn report(label: &str, html: &str) {
    let doc = document(html);
    let p = doc.query_selector("#p").expect("selector").expect("#p");
    let input = doc.query_selector("#i").expect("selector").expect("#i");
    println!(
        "{label:48} p CanvasText {:22} mark background {}",
        doc.resolved_style_value(p, "color"),
        doc.resolved_style_value(input, "background-color")
    );
}

fn main() {
    let body = r#"<body><p id="p" style="color: CanvasText; background-color: Canvas">text</p><mark id="i">marked</mark></body>"#;
    report("no declaration", &format!("<html><head></head>{body}</html>"));
    report("<meta name=color-scheme content=light>", &format!(r#"<html><head><meta name="color-scheme" content="light"></head>{body}</html>"#));
    report(":root { color-scheme: light } (CSS)", &format!(r#"<html><head><style>:root{{color-scheme:light}}</style></head>{body}</html>"#));
    report("<meta name=color-scheme content=dark>", &format!(r#"<html><head><meta name="color-scheme" content="dark"></head>{body}</html>"#));
    report(":root { color-scheme: dark } (CSS)", &format!(r#"<html><head><style>:root{{color-scheme:dark}}</style></head>{body}</html>"#));
}
