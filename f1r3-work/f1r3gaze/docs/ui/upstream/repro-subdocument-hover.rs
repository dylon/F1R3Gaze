// Reproduction for blitz-subdocument-hover.md. A standalone binary crate:
//
//   [dependencies]
//   blitz-dom = { git = "https://github.com/DioxusLabs/blitz", rev = "0db8c74a5f8a0df77eed1b33c846eb041515b6c7" }
//   blitz-html = { git = "https://github.com/DioxusLabs/blitz", rev = "0db8c74a5f8a0df77eed1b33c846eb041515b6c7" }
//   blitz-traits = { git = "https://github.com/DioxusLabs/blitz", rev = "0db8c74a5f8a0df77eed1b33c846eb041515b6c7" }
//   cursor-icon = "1"
//   keyboard-types = "0.7"
//
// Measured with these crates as path dependencies on a clone of that revision
// (main on 2026-10-04), and on 674d7d2: the output is identical.

//! A sub-document keeps its hover after the pointer leaves its host.
//!
//! A parent document hosts a child document in `#host`, as an `<iframe>`
//! does, and the child shares the parent's shell provider, as iframes do
//! (`iframe.rs`). The pointer moves onto the child's link, then out of the
//! host onto the parent's `#outside`, which has `cursor: pointer`. Finally the
//! child's layout changes while the pointer is still over `#outside`.
//!
//! Prints what the window was asked for and the hover state at each step.

use blitz_dom::{
    BaseDocument, DEFAULT_CSS, DocumentConfig, EventDriver, LocalName, NoopEventHandler, PlainDocument,
    QualName, ns,
};
use blitz_html::HtmlDocument;
use blitz_traits::events::{
    BlitzPointerEvent, BlitzPointerId, MouseEventButton, MouseEventButtons, PointerCoords, UiEvent,
};
use blitz_traits::shell::ShellProvider;
use cursor_icon::CursorIcon;
use keyboard_types::Modifiers;
use std::sync::{Arc, Mutex};

/// Records the cursor requests a window would receive.
#[derive(Default)]
struct Window {
    cursors: Mutex<Vec<Option<CursorIcon>>>,
}

impl ShellProvider for Window {
    fn set_cursor(&self, icon: Option<CursorIcon>) {
        self.cursors.lock().expect("cursor log").push(icon);
    }
}

const PARENT: &str = r#"<html><body style="margin:0">
<div id="host" style="width:300px;height:200px"></div>
<div id="outside" style="width:300px;height:100px;cursor:pointer"></div>
</body></html>"#;

const CHILD: &str = r#"<html><head><style>
body{margin:0}
#go{display:block;width:200px;height:50px;background:#eeeeee}
#go:hover{background:#cc0000}
</style></head><body><a id="go" href="next.html">link</a></body></html>"#;

fn document(html: &str, window: &Arc<Window>) -> BaseDocument {
    let config = DocumentConfig {
        shell_provider: Some(window.clone()),
        ua_stylesheets: Some(vec![DEFAULT_CSS.to_string()]),
        ..Default::default()
    };
    HtmlDocument::from_html(html, config).into_inner()
}

fn pointer_move(x: f32, y: f32) -> UiEvent {
    UiEvent::PointerMove(BlitzPointerEvent {
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
        buttons: MouseEventButtons::None,
        mods: Modifiers::empty(),
        details: Default::default(),
        element: Default::default(),
        active_pointers: Default::default(),
    })
}

/// The `id` of the element `doc` considers hovered.
fn hovered(doc: &BaseDocument) -> Option<String> {
    doc.get_hover_node_id().map(|id| {
        doc.get_node(id)
            .and_then(|node| node.attr(LocalName::from("id")))
            .unwrap_or("(no id)")
            .to_string()
    })
}

fn main() {
    let window = Arc::new(Window::default());
    let mut parent = document(PARENT, &window);
    parent.viewport_mut().window_size = (300, 300);
    let host = parent
        .query_selector("#host")
        .expect("selector")
        .expect("#host");
    parent.set_sub_document(host, Box::new(PlainDocument(document(CHILD, &window))));
    parent.resolve(0.0);
    let mut parent = PlainDocument(parent);

    let report = |step: &str, parent: &PlainDocument, window: &Window| {
        let child = parent.0.subdoc(host).expect("hosted");
        let child = child.inner();
        let go = child.query_selector("#go").expect("selector");
        let background = go
            .map(|go| child.resolved_style_value(go, "background-color"))
            .unwrap_or_else(|| "(removed)".into());
        println!("{step}");
        println!("  parent hover:          {:?}", hovered(&parent.0));
        println!("  child hover:           {:?}", hovered(&child));
        println!("  child #go background:  {background}");
        println!(
            "  window cursor requests: {:?}",
            window.cursors.lock().expect("cursor log")
        );
    };

    // 1. Onto the child's link.
    EventDriver::new(&mut parent, NoopEventHandler).handle_ui_event(pointer_move(100.0, 25.0));
    parent.0.resolve(0.0);
    report("1. pointer on the child's link (100, 25)", &parent, &window);

    // 2. Out of the host, onto the parent's #outside (cursor: pointer).
    EventDriver::new(&mut parent, NoopEventHandler).handle_ui_event(pointer_move(100.0, 250.0));
    parent.0.resolve(0.0);
    report("2. pointer on the parent's #outside (100, 250)", &parent, &window);

    // 3. The child's layout changes while the pointer stays on #outside: its
    //    link is hidden. The child re-resolves hover at its stale pointer
    //    position (refresh_hover) and sets the window's cursor.
    {
        let child = parent.0.subdoc_mut(host).expect("hosted");
        let mut child = child.inner_mut();
        let go = child
            .query_selector("#go")
            .expect("selector")
            .expect("#go");
        let mut m = child.mutate();
        m.set_attribute(go, QualName::new(None, ns!(), LocalName::from("style")), "display:none");
    }
    parent.0.resolve(0.0);
    parent.0.resolve(0.0);
    report("3. child relaid out, pointer still on #outside", &parent, &window);
}
