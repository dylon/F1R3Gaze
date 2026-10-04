# A sub-document keeps its hover after the pointer leaves its host, and still sets the window's cursor

**Filed as** <https://github.com/DioxusLabs/blitz/issues/1040> (2026-10-04).

**Project:** [DioxusLabs/blitz](https://github.com/DioxusLabs/blitz) (blitz-dom,
with the effect seen through blitz-shell). Present at rev `674d7d2` and on
`main` at `0db8c74` (2026-10-04).

## Reproduction

A parent document hosts a child document in `#host`, as an `<iframe>` does.
The child uses the parent's shell provider, as a loaded iframe does
(`blitz-dom/src/iframe.rs:67`).

Parent:

```html
<html><body style="margin:0">
<div id="host" style="width:300px;height:200px"></div>
<div id="outside" style="width:300px;height:100px;cursor:pointer"></div>
</body></html>
```

Child:

```html
<html><head><style>
body{margin:0}
#go{display:block;width:200px;height:50px;background:#eeeeee}
#go:hover{background:#cc0000}
</style></head><body><a id="go" href="next.html">link</a></body></html>
```

The program [`repro-subdocument-hover.rs`](repro-subdocument-hover.rs) works
headlessly through the public API. It builds both documents with
`HtmlDocument::from_html`, hosts the child with `set_sub_document`, and drives
the parent with `EventDriver`. A `ShellProvider` records the cursor requests a
window would receive. The steps are:

1. Move the pointer onto the child's link, at (100, 25).
2. Move it out of the host onto the parent's `#outside` (`cursor: pointer`),
   at (100, 250).
3. With the pointer still there, hide the child's link (`style="display:none"`)
   and resolve.

**Observed.** On 2026-10-04 the output was identical at `674d7d2` and at
`main` `0db8c74`:

```text
1. pointer on the child's link (100, 25)
  parent hover:          Some("host")
  child hover:           Some("go")
  child #go background:  rgb(204, 0, 0)
  window cursor requests: [None, Some(Pointer)]
2. pointer on the parent's #outside (100, 250)
  parent hover:          Some("outside")
  child hover:           Some("go")
  child #go background:  rgb(204, 0, 0)
  window cursor requests: [None, Some(Pointer), Some(Pointer)]
3. child relaid out, pointer still on #outside
  parent hover:          Some("outside")
  child hover:           None
  child #go background:  rgb(238, 238, 238)
  window cursor requests: [None, Some(Pointer), Some(Pointer), None]
```

- **Stale hover (step 2).** The pointer is over the parent's `#outside`, but
  the child still has `#go` hovered, and `#go:hover` still applies
  (`rgb(204, 0, 0)`). In a window the link stays highlighted after the pointer
  has left the frame.
- **A document the pointer is not over sets the cursor (step 3).** The child
  re-resolves hover at its last pointer position, and its hover changes. It
  then sends `set_cursor(None)` through the shared provider. In blitz-shell,
  `None` hides the cursor (`blitz-shell/src/lib.rs:101`). So the cursor
  disappears while the pointer is over an element that asks for `pointer`.
- **The cursor from before the event (step 1).** The parent first sends the
  child's cursor as it was *before* the event. The child had nothing hovered,
  so that is `None`. Only then does the child send its own (`Pointer`). blitz-shell
  hides the cursor and then shows it again within the one event. When the
  child already had a hover, the parent's request is the cursor of wherever
  the pointer last was in the child.

**Expected.**
- When the pointer leaves the host, the child's hover ends. Browsers deliver
  `pointerleave`/`mouseleave` to a frame's document and drop its `:hover`
  chain.
- Only the document under the pointer decides the window's cursor.
- The cursor for a host reflects the child after the child has seen the
  event.

## Cause

1. `map_dom_event_to_ui_event` (`blitz-dom/src/events/mod.rs:29`) forwards
   pointer moves to a sub-document but drops enter, leave, over and out. The
   comment at line 53 says they "will be recreated by sub-document's event
   driver based move events". That only works while the pointer is inside the
   host. Once it leaves, the child receives no more moves, so its driver never
   sees the pointer leave.
2. Nothing else ends the child's hover. The only caller of `clear_hover` is
   the touch path in `EventDriver::handle_ui_event`
   (`blitz-dom/src/events/driver.rs:267`).
3. The child keeps `last_client_pointer_position` (set in `set_hover_to`,
   `blitz-dom/src/document.rs:1815`). `resolve` ends with `refresh_hover`
   (`blitz-dom/src/resolve.rs:138`), which hit-tests that stale point again.
   When the result changes, `set_hover_to` calls
   `shell_provider.set_cursor(self.get_cursor())` (`document.rs:1882`), and
   that provider is the window's.
4. `EventDriver::handle_ui_event` updates hover, and with it the cursor,
   before it dispatches the event (`driver.rs:148-186`). Dispatching is what
   forwards the event to the sub-document (`events/mod.rs:119`).
   `get_cursor` delegates to the sub-document (`document.rs:2158`), which has
   not seen the event yet.

## Suggested fix

- When a parent's hover leaves a node that hosts a sub-document, call the
  sub-document's `clear_hover()`. This could happen in `set_hover_to`, which
  already diffs the old and new hover chains, or in the default action of
  `PointerLeave` on the host. `clear_hover` also forgets
  `last_client_pointer_position`, so the child stops re-hovering a stale
  point. That removes both the stale `:hover` and the stray cursor requests.
- After forwarding a pointer event to a sub-document, have the parent send
  `set_cursor(self.get_cursor())` again, so that the cursor describes the
  child after the event. That also removes the transient `None`.

## Workaround

In F1R3Gaze each browser tab's page is a sub-document of the browser's own UI
document. The pages' provider only reports hover changes. The UI document
decides the window's cursor after each event:
- it ends the hover of a page the pointer has left;
- it tells a page that has come under the pointer where the pointer is;
- it treats `None` as "hide" only when the hovered element's computed
  `cursor` is `none`.

The details are in [`../README.md`](../README.md) §3.4 and
[`../ledger.md`](../ledger.md) L8.
