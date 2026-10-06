// Reproduction for blitz-canvas-color-scheme.md. A standalone binary crate:
//
//   [dependencies]
//   blitz-dom = { git = "https://github.com/DioxusLabs/blitz", rev = "2335458530518cdf167c55ce635fae99323e0789" }
//   blitz-html = { git = "https://github.com/DioxusLabs/blitz", rev = "2335458530518cdf167c55ce635fae99323e0789" }
//   blitz-traits = { git = "https://github.com/DioxusLabs/blitz", rev = "2335458530518cdf167c55ce635fae99323e0789" }
//   blitz-paint = { git = "https://github.com/DioxusLabs/blitz", rev = "2335458530518cdf167c55ce635fae99323e0789" }
//   anyrender = "0.14.0"
//   kurbo = "0.13.1"
//
// Measured with the Blitz crates as path dependencies on a clone of that
// revision (main on 2026-10-06).

//! Does the canvas take the `Canvas` colour of the root element's colour
//! scheme? Each document is painted into anyrender's recording scene, and
//! the program prints the fills that cover the whole 400 x 300 canvas.

use anyrender::Paint;
use anyrender::recording::{RenderCommand, Scene};
use blitz_dom::{BaseDocument, DEFAULT_CSS, DocumentConfig};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use kurbo::Shape;

fn painted(html: &str, scheme: ColorScheme) -> Scene {
    let config = DocumentConfig {
        viewport: Some(Viewport::new(400, 300, 1.0, scheme)),
        ua_stylesheets: Some(vec![DEFAULT_CSS.to_string()]),
        ..Default::default()
    };
    let mut doc: BaseDocument = HtmlDocument::from_html(html, config).into_inner();
    doc.resolve(0.0);
    let mut scene = Scene::default();
    blitz_paint::paint_scene(&mut scene, &mut doc, 1.0, 400, 300, 0, 0);
    scene
}

fn report(label: &str, html: &str, scheme: ColorScheme) {
    let scene = painted(html, scheme);
    let mut covering = Vec::new();
    let mut text = None;
    for command in &scene.commands {
        match command {
            RenderCommand::Fill(fill) => {
                let bounds = fill.shape.bounding_box();
                if bounds.x0 <= 0.0 && bounds.y0 <= 0.0 && bounds.x1 >= 400.0 && bounds.y1 >= 300.0 {
                    if let Paint::Solid(color) = &fill.brush {
                        covering.push(format!("{:?}", color.to_rgba8()));
                    }
                }
            }
            RenderCommand::GlyphRun(run) if text.is_none() => {
                if let Paint::Solid(color) = &run.brush {
                    text = Some(format!("{:?}", color.to_rgba8()));
                }
            }
            _ => {}
        }
    }
    println!("{label:52} fills covering the canvas: {covering:?}; text: {}", text.unwrap_or_else(|| "-".into()));
}

fn main() {
    let body = "<body><p>text</p></body>";
    report("light preference, nothing declared", &format!("<html>{body}</html>"), ColorScheme::Light);
    report("dark preference, :root { color-scheme: dark }", &format!("<html><style>:root{{color-scheme:dark}}</style>{body}</html>"), ColorScheme::Dark);
    report("dark preference, html { background: Canvas } too", &format!("<html><style>:root{{color-scheme:dark}} html{{background:Canvas}}</style>{body}</html>"), ColorScheme::Dark);
    report("dark preference, html { color: CanvasText } too", &format!("<html><style>:root{{color-scheme:dark}} html{{color:CanvasText}}</style>{body}</html>"), ColorScheme::Dark);
}
