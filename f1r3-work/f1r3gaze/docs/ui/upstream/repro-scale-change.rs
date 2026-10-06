// Reproduction for blitz-scale-change-border-widths.md. A standalone binary
// crate:
//
//   [dependencies]
//   blitz-dom = { git = "https://github.com/DioxusLabs/blitz", rev = "2335458530518cdf167c55ce635fae99323e0789" }
//   blitz-html = { git = "https://github.com/DioxusLabs/blitz", rev = "2335458530518cdf167c55ce635fae99323e0789" }
//   blitz-traits = { git = "https://github.com/DioxusLabs/blitz", rev = "2335458530518cdf167c55ce635fae99323e0789" }
//
// Measured with these crates as path dependencies on a clone of that revision
// (main on 2026-10-06), and on 674d7d2.

//! A document whose scale changes keeps the border widths of its first scale.
//!
//! The same document is made three ways, each ending at a total scale of 1.2:
//! at zoom 1.2 from the start; at zoom 1.0, laid out, then zoomed to 1.2; and
//! at a hidpi scale of 1.0, laid out, then at 1.2. It prints the computed
//! `border-top-width` of the first of six bordered boxes, and where the
//! last one is laid out.

use blitz_dom::{BaseDocument, DEFAULT_CSS, DocumentConfig};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};

const HTML: &str = r#"<html><head><style>
body { margin: 0 }
div { width: 200px; height: 20px; border: 1px solid black }
</style></head><body>
<div id="box"></div><div></div><div></div><div></div><div></div><div id="last"></div>
</body></html>"#;

fn document(hidpi_scale: f32, zoom: f32) -> BaseDocument {
    let config = DocumentConfig {
        viewport: Some(Viewport::new(1280, 800, hidpi_scale, ColorScheme::Light)),
        ua_stylesheets: Some(vec![DEFAULT_CSS.to_string()]),
        ..Default::default()
    };
    let mut doc = HtmlDocument::from_html(HTML, config).into_inner();
    doc.viewport_mut().set_zoom(zoom);
    doc.resolve(0.0);
    doc
}

fn report(label: &str, doc: &BaseDocument) {
    let first = doc.query_selector("#box").expect("selector").expect("#box");
    let last = doc.query_selector("#last").expect("selector").expect("#last");
    let layout = doc.get_node(last).expect("the last box").final_layout();
    println!(
        "{label:34} scale {:.1}  border-top-width {:12}  last box at y = {}",
        doc.viewport().scale(),
        doc.resolved_style_value(first, "border-top-width"),
        layout.location.y,
    );
}

fn main() {
    report("made at zoom 1.2", &document(1.0, 1.2));

    let mut zoomed = document(1.0, 1.0);
    zoomed.viewport_mut().set_zoom(1.2);
    zoomed.resolve(0.0);
    report("made at zoom 1.0, then zoom 1.2", &zoomed);

    report("made at hidpi scale 1.2", &document(1.2, 1.0));

    let mut rescaled = document(1.0, 1.0);
    rescaled.viewport_mut().set_hidpi_scale(1.2);
    rescaled.resolve(0.0);
    report("made at scale 1.0, then scale 1.2", &rescaled);
}
