# F1R3Gaze

A browser whose only execution mechanism is f1r3lang on a native RSpace.
HTML and CSS are rendered by [Blitz](https://github.com/DioxusLabs/blitz);
everything a page *does* is a f1r3lang program run by the CampF1R3 executive
in the tab. No JavaScript is ever executed. Pages hold capabilities, not
ambient authority, and sites published on a F1R3FLY shard are resolved and
verified through the shard bridge.

This repository implements the *F1R3Gaze Implementation Specification* v0.1
(`publications/f1r3gaze`): the portable core, the native browser (P1–P3), the
signed installers of P4, and the reach tier.

![F1R3Gaze running a f1r3lang page](docs/screenshots/01-demo-site.png)

## Using it

```
f1r3gaze [URL]                      open a window (default: gaze://newtab)
f1r3gaze --headless URL [--allow] [--click SELECTOR]... [--timeout SECS] [--wait SECS] [--log FILE.gzlog]
                                    run a page without a window; print its committed
                                    document and console (CI smoke tests)
f1r3gaze --profile DIR ...          keep every folder under DIR: DIR/{config,data,state,cache,runtime}
                                    (also: F1R3GAZE_PROFILE)
f1r3gaze wallet new|import|export|use|list|balance|send|remove ...
                                    the wallets that pay for deploys (docs/wallet.md)
f1r3gaze paths                      print where the folders are
f1r3gaze profile check              say what a start would change; change nothing
f1r3gaze profile backups [prune --older-than DAYS]
                                    list start-up's backups, or remove the old ones
f1r3gaze trust list|forget BINDING|forget --all
                                    this shard's freshness records (after a shard reset)

f1r3c compile app.rho [-o app.knf] [--level k1g] [--import IDENT=URN]...
f1r3c inspect app.knf               manifest, hashes, program text
f1r3c integrity app.knf             value for the HTML integrity attribute
f1r3c site DIR [--entry index.html] [--mirror URL]... [--out OUT]
                                    package a site: blobs by hash + the registry manifest
```

A page loads its behaviour like this, and the browser refuses it if the
bytes do not match:

```html
<script type="application/f1r3lang" src="app.knf" integrity="blake2b-256:420d…"></script>
```

or inline, naming the capabilities it imports:

```html
<script type="application/f1r3lang" imports="doc">
  new found, sub, clicks in {
    doc!("query1", "#lamp", *found) |
    for (@("ok", *lamp) <- found) { lamp!("listen", "click", *clicks, {}, *sub) | … }
  }
</script>
```

Addresses: `https://…` and `http://…` (plain HTTP can be switched off),
`f1r3://<publisher>/<project>@<range>/<path>` for shard sites,
`f1r3h://blake2b-256/<hex>` for content by hash, `file://…`, and the
built-in `gaze://newtab` and `gaze://about`.

Settings live in `settings.toml` in the settings folder (`f1r3gaze paths`
prints it; see [Where F1R3Gaze keeps its files](#where-f1r3gaze-keeps-its-files)),
one table per area:
- `[appearance]`: `theme`, and `restore_sidebar` for a window to reopen the
  sidebar as the last one left it;
- `[browsing]`: `home` and `https_only`;
- `[shard]`: `observers`, `validator`, `shard_id`, `quorum` and `phlo_price`;
- `[wallet]`: `embers_api` and `max_fee`;
- `[content]`: blob `mirrors` and `cache_bytes`;
- `[site_data]`: `store_quota`.

A new file lists every setting commented out at its default, and
`settings.toml.example` beside it stays a fresh copy. A value F1R3Gaze
cannot use is reported with its line and column, and the default (or a
system-wide file's value) is used for it. The format:
[`docs/storage/README.md`](docs/storage/README.md), section 6.1.

### Browser controls

The design of every control is in [`docs/ui/README.md`](docs/ui/README.md).

- **Tab strip.** Drag a tab onto another to reorder it, middle-click to close
  it, or right-click it for its menu. When tabs get crowded, the active tab
  keeps a readable width and the others shrink to their icons; beyond that, a
  "+N" chip opens the Tabs panel.
- **Toolbar.** Show or hide the sidebar (`Ctrl+B`), Back and Forward (disabled
  when there is nowhere to go), Reload, the address box, and Find.
  - The badge in the address box shows what kind of address the page has;
    click it for the page's permissions.
  - Typing suggests open tabs (**Switch to tab**) and history, with the matched
    text highlighted. ↓/↑ choose, Enter opens, and Esc puts the address back.
- **Rail.** Opens Tabs, History, Site data, Permissions, Wallet, Console, and
  Appearance. Each button names itself on hover; clicking the open panel's
  button hides the sidebar. A window opens with the sidebar hidden, and
  `Ctrl+B` brings back the panel last shown.
- **Tabs panel.**
  - Search covers titles and addresses, including small typing errors (through
    the native Rust `liblevenshtein` library), and shows the text that matched.
    **Page text** also searches the text of loaded pages.
  - **Tree** nests tabs under the tab that opened them.
  - Each row's ⋯ menu duplicates, copies the address, moves, or closes tabs.
  - **Recently closed** reopens tabs.
- **History.** Grouped by how long ago, with relative times; × forgets an
  entry, and **Clear…** asks first.
- **Find.** `Ctrl+F`/`Cmd+F` opens a box over the top right of the page.
  Enter and Shift+Enter step through the matches; Esc closes it.
- **Keys.** On macOS, Cmd stands for Ctrl, except in `Ctrl+Tab`.
  - `Ctrl+T` opens a tab and `Ctrl+W` closes one.
  - `Ctrl+Shift+T` reopens the last closed tab.
  - `Ctrl+L` selects the address.
  - `Ctrl+Tab` and `Ctrl+Shift+Tab` cycle tabs; `Ctrl+1`–`Ctrl+9` jump to a
    tab.
  - `Ctrl+H` opens History (`Cmd+Y` on macOS, where `Cmd+H` hides the
    application).
  - `F11` (`Ctrl+Cmd+F` on macOS) enters or leaves full screen.
  - `Ctrl+=`, `Ctrl+−` and `Ctrl+0` zoom the window, from 25 % to 500 %.

Open tabs and the sidebar's layout are kept in `state/session.json`, and
history in `state/history.json`; restored background tabs load when
selected. The window reopens at its size and place, maximized or full screen
as it was left, and at its zoom (`state/window.json`); on Wayland the
compositor chooses its place. **Clear history** also removes the copies start-up kept of
damaged history files.

Appearance offers System, Dark, and Light, and lists the theme files.
- **By default the browser follows the system's light or dark setting**
  (`theme = "system"`): the XDG desktop portal on Linux, the operating
  system on macOS and Windows. Without a preference it is dark, and
  Appearance says what it found. Choosing System, Dark, Light or a theme
  file saves the choice as `theme` under `[appearance]` in `settings.toml`.
- **Theme files.** Every `themes/<name>.css` in the settings folder, and any
  installed for everyone, is a row under **Themes**: click one to use it. A
  file that cannot be used says why. **New theme** saves the colors shown
  as `themes/my-theme.css` (never over an existing file) and chooses it;
  edit its `--gaze-*` hex colors, then press **Reload**. The format is in
  `docs/storage/README.md`, section 6.2, and `themes/default-dark.css.example`
  and `default-light.css.example` show every token.
- The browser checks text contrast (WCAG 4.5:1) and applies the scheme to its
  controls, its built-in `gaze://` pages and the window's title bar. Sites
  see it as well: a site's `prefers-color-scheme` styles follow the scheme
  shown, as in other browsers, and a site with no dark styles keeps its own
  colors.
- The UI bundles Font Awesome Free 7 icons, Noto Sans, and Fira Code; licenses
  are in `crates/gaze-shell/assets/`.

Site data lists the sites that keep something: capability store usage,
remembered permissions, and live shard sessions. It also shows the verified
cache. Web cookies and Web localStorage are not implemented, and JavaScript is
not executed. Clearing the cache does not remove wallet exports or replay logs,
which are in the data folder (`wallet/exports/`, `replay-logs/`).

### The wallet: the agent driving the browser pays

Every deploy the browser makes (a page's program, a session message) is
signed by the **active wallet**, whose account pays for it. Wallets are
created, imported and exported in the same file format as **F1R3Sky**, and
their addresses are the F1R3Cap addresses F1R3Sky shows. Keys live in the OS
keychain (macOS, Windows) or a `0600` file per key in the data folder's
`wallet/keys/` (Linux).

A program deployed under the user's key could otherwise take the deployer's
identity and spend from the wallet, so the browser renders every deploy
itself and binds only allow-listed system names: registry, standard output,
the deploy id, block data, crypto. **`rho:rchain:deployerId` is refused.** A
page can cost phlo (the consent prompt quotes it, with the paying wallet and
its balance); it cannot move funds. Transfers go through Embers, as in
F1R3Sky, but the browser decodes and checks every prepared contract against
Embers' transfer template before signing it. Details: `docs/wallet.md`.

### Events

A listener receives `(type, fields)`. `docs/events.md` lists the fields
guaranteed for each event type; hosts may add more, so pages end their
patterns with a remainder: `@(_, {"x": x, "y": y ..._})`.

## Where F1R3Gaze keeps its files

F1R3Gaze keeps five kinds of files, each in the place the operating system
has for it (the XDG Base Directory specification on Linux):

| Kind | Linux | macOS | Windows |
|---|---|---|---|
| settings and themes | `$XDG_CONFIG_HOME/f1r3fly-io/f1r3gaze` | `~/Library/Application Support/io.f1r3fly.f1r3gaze/config` | `%APPDATA%\f1r3fly-io\f1r3gaze` |
| data: wallets and keys, permissions, site data, freshness records, replay logs | `$XDG_DATA_HOME/f1r3fly-io/f1r3gaze` | `…/io.f1r3fly.f1r3gaze/data` | `%LOCALAPPDATA%\f1r3fly-io\f1r3gaze\data` |
| state: open tabs, history, the window | `$XDG_STATE_HOME/f1r3fly-io/f1r3gaze` | `…/io.f1r3fly.f1r3gaze/state` | `…\state` |
| the content cache | `$XDG_CACHE_HOME/f1r3fly-io/f1r3gaze` | `~/Library/Caches/io.f1r3fly.f1r3gaze` | `…\cache` |
| the instance lock | `$XDG_RUNTIME_DIR/f1r3fly-io/f1r3gaze` | `$TMPDIR/io.f1r3fly.f1r3gaze` | `…\runtime` |

- `f1r3gaze paths` prints them; `--profile DIR` (or `F1R3GAZE_PROFILE`)
  keeps all five under `DIR`.
- Settings and themes can also be installed for everyone: `settings.toml`
  in `$XDG_CONFIG_DIRS/f1r3fly-io/f1r3gaze` (`/etc/xdg` by default), and
  theme files in `$XDG_DATA_DIRS/f1r3fly-io/f1r3gaze/themes`. A user's own
  file comes last and wins.
- **One F1R3Gaze per profile.** A second window, `--headless`, or a command
  that changes the profile, exits with status 1 and names the one that
  holds it. Commands that only read (`wallet list`, `balance`, `export`,
  `send`, `profile backups`, `trust list`) go on, read-only.
- **Start-up checks every file.** A missing one is made from its default; a
  damaged one is copied into `backups/<time>/` in its own folder first, then
  repaired, and the window says so. Wallet keys are never changed.
- `f1r3gaze profile check` says what the next start would do, and changes
  nothing. `f1r3gaze profile backups` lists the backups, and
  `profile backups prune --older-than DAYS` removes the old ones; nothing
  else ever removes a backup.
- After a shard is reset, its new blocks look like replays of old answers.
  `f1r3gaze trust list` shows the freshness records, and
  `trust forget BINDING` or `trust forget --all` forgets them.

The design, the formats, and how start-up keeps the files whole through
crashes and power cuts: [`docs/storage/README.md`](docs/storage/README.md).

## Upgrading from a single-folder profile

Older builds kept everything in one folder (`~/.local/share/f1r3gaze`,
`~/Library/Application Support/F1R3Gaze` or `%APPDATA%\F1R3Gaze`). The
first start of this build moves it into the folders above:
- Every file is moved, never copied over anything: if something is already
  at a file's new place, both are kept, and the window says so.
- `settings.conf` becomes `settings.toml` (with only what was set), and
  `workspace.json` becomes `state/session.json` and `state/history.json`;
  the originals are kept in the backups.
- The old dark theme becomes "system", since dark was the old default:
  choose Dark in Appearance to keep it dark.
- On another file system, the content cache stays behind (it can be
  fetched again).
- `MIGRATED.txt` in the old folder says where each file went. An older
  build started afterwards finds the folder empty.
- `f1r3gaze profile check` shows the plan first, and changes nothing.

## Building

```
cargo build --release -p gaze-shell -p f1r3c     # target/release/{f1r3gaze,f1r3c}
cargo test --workspace
cargo build -p gaze-reach --target wasm32-unknown-unknown --profile reach
```

Rust 1.95, edition 2024. CampF1R3 (`gaze/map-remainders`, rev `ea2d323`) and
Blitz (rev `674d7d2`) are git dependencies pinned by revision; the CampF1R3
revision must be pushed to GitHub before a clean checkout can build. On Linux the
window needs Vulkan (or Mesa's `lavapipe`, `mesa-vulkan-drivers`) and
`libxkbcommon`; macOS and Windows need nothing extra. Building the windowed
binary in debug mode wants about 4 GB of memory to link.

## Releases (P4)

Pushing a tag `vX.Y.Z` runs `.github/workflows/release.yml`, which builds and
publishes a draft GitHub release with:

| platform | artifacts | signing |
| --- | --- | --- |
| macOS | `F1R3Gaze-<v>-macos-universal.dmg` and component `.pkg` (arm64 + x86_64) | Developer ID, hardened runtime with **no** exceptions (no JIT entitlement: there is no JavaScript), notarised and stapled when credentials are configured |
| Windows | `F1R3Gaze-<v>-x64.msi`, portable `.zip` | Authenticode (`signtool`, SHA-256, RFC 3161 timestamp) on both executables and the MSI |
| Linux x86_64 and arm64 | `.deb`, Arch `.pkg.tar.zst`, distro-specific `.rpm`, `.AppImage`, `.tar.gz` | GPG-signed `SHA256SUMS` covering every artifact of every platform when a key is configured |

Secrets: `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD`, `MACOS_SIGN_IDENTITY`,
`MACOS_INSTALLER_CERT_P12`, `MACOS_INSTALLER_CERT_PASSWORD`,
`MACOS_INSTALLER_SIGN_IDENTITY`,
`APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`; `WINDOWS_CERT_PFX`,
`WINDOWS_CERT_PASSWORD`; `GPG_PRIVATE_KEY`, `GPG_PASSPHRASE`. A platform whose
secrets are missing still builds, unsigned, with a warning. The Windows
installer and the Linux packages register `f1r3://` and `f1r3h://`. Scripts:
`packaging/{linux,macos,windows}/`, icons from the F1R3FLY.io brand kit in
`packaging/icons/`.

Verify a download: `gpg --verify SHA256SUMS.asc SHA256SUMS && sha256sum -c SHA256SUMS`.
See [the distribution build guide](../../docs/distribution.md) for local builds,
repository staging, Homebrew and WinGet manifests, and the current limits of
native installer validation. Gaze packages include `f1r3c` and its in-process
RSpace; F1R3Node remains a separate optional service and Embers is excluded.

## Reach tier

`crates/gaze-reach` compiles the executive and the DOM protocol to one wasm
module (no `wasm-bindgen`); `web/gaze-reach.js` is its host in any modern
browser: the DOM backend over the real page, the frame loop, `net` (same
origin, integrity checked inside the module), the per-origin `store`
capability and `nav`.
`shard` is a dead channel there, since a stock browser has no key custody a
page cannot reach. `.github/workflows/reach.yml` publishes the module and the
demo page to GitHub Pages. A page opts in with

```html
<script type="module" src="gaze-reach.js"></script>
```

## Crates

| crate | spec | contents |
| --- | --- | --- |
| `gaze-graded` | §10 | the four integer semirings, xoshiro256**, argmax / argmin / proportional sampling, the ψ ⊕ ε fairness combinator, ceilings |
| `gaze-knf` | §5 | the `.knf` container, the manifest, program and grant hashes, `integrity` strings |
| `gaze-dom-core` | §7 | the DOM protocol engine over `DomBackend`; attenuation, verbs, fragments, frame-batched writes, capture/bubble dispatch, `decide` listeners; `MemDom` reference backend |
| `gaze-exec` | §6, §8 | `TabExec`: grounding, apertures, `doc`/`log`/`clock`/`rand`, external capabilities routed out as requests, the frame loop, the page meter, `.gzlog` record and replay (dispatches included) |
| `gaze-dom-blitz` | §7.9, WP B1 | `BlitzDom` backend, `RhoDocument` (a Blitz `Document`: script collection, frame pacing, services), `RhoEventHandler` |
| `gaze-broker` | §8.4, WP C2 | sites, the default policy, grant plans and prompts, remembered grants, per-tab routes with attenuation, revocation |
| `gaze-net` | §8.5 | HTTP without ambient credentials, per-hop redirect checks, schemes, content hashes, the Blitz net provider |
| `gaze-store` | §8.6 | per-origin append-only store with quota and torn-tail recovery |
| `gaze-blob` | §9.7 | content-addressed blobs: verifying cache, mirrors |
| `gaze-shard` | §9, WP S1/S2 | the shard bridge: deploys signed by the payer, node client, graded answers (node, quorum), freshness, code-by-hash rendering with the system-name allow-list, sessions, watches, site manifests, on-chain blobs in F1R3Drive's layout, the keystore |
| `gaze-wallet` | — | wallets: F1R3Cap addresses, F1R3Sky-compatible key files, Embers balances and transfers with prepared contracts checked before signing; the bridge's payer |
| `gaze-shell` | §3, WP C1 | the application: engine, tab pipeline, chrome, headless runner, profile |
| `f1r3c` | §3 | the toolchain CLI |
| `gaze-reach` | §12 | the reach tier |

## Upstream changes to CampF1R3 (in `patches/`)

- **U1 ground data.** Booleans, 64-bit integers, strings, bytes, lists, tuples
  and maps through lexer, parser (level `K1G`), AST, normaliser, encoding
  (tags `0x09`–`0x0F`), decoding, substitution, JSON, printer and matcher.
  Maps are ordered by key encoding; duplicate literal keys are rejected.
- **U3** `Machine::with_observer`, and `Observer::observe_aperture`.
- **U4** `bind_aperture`, `take_outbound`, `inject`.
- **U6** the `Keyed` minter.
- A job whose cut an observer refuses is put back at the head of the queue,
  so an exhausted budget stops progress without losing work.
- **Remainder patterns** for lists and maps (branch `gaze/map-remainders`,
  commit `ea2d323`, offered as input to the map-pattern design):
  `[a, b ...rest]`, `{"k": v ...rest}`. A map pattern matches by key lookup;
  keys must be ground. See the commit message for the full design.

Three defects in the existing code were found and fixed on the way:

1. Substitution reversed the arguments of a polyadic send (`rebuild.rs`).
2. The matcher bound a name pattern to the raw quote `@P` instead of applying
   `@*x = x`, so a received name could not be sent on. The rewrite was missing
   at three sites: the `*y` process pattern, the datum-to-name goal, and
   `Chan::quote`.
3. A quoted pattern `@{*y}` could not match a concrete (unforgeable) name.


## Tests

478 tests in this workspace, all passing on stable and on Rust 1.95
(CampF1R3 carries its own 181), and the storage checks below:

| crate | tests | what they establish |
| --- | --- | --- |
| `gaze-exec` | 9 | the specification's lamp; replay with identical commit hashes; seeds; dead channels; confinement; timers; bounded runaway pages; a remainder listener survives extra event fields where an exact one goes silent |
| `gaze-dom-core`, `gaze-graded`, `gaze-knf` | 16 | the protocol engine, the semirings, the container |
| `gaze-dom-blitz` | 8 | Blitz and `MemDom` commit identical hashes frame by frame; a Blitz log replays identically on both, including a click on a never-named node; scoped queries cannot leak; `decide` prevents; `setHTML` |
| `gaze-broker` | 5 | policy, prompts, remembered grants, routes, revocation |
| `gaze-net`, `gaze-store`, `gaze-blob` | 16 | redirects, credentials stripped, hashes; torn-tail recovery, salvage before a store is cut back, quota; the verified cache, and a read-only one that never writes |
| `gaze-fs` | 39 | durable writes: atomic replacement, publishing without replacing, verified copies across file systems, folders made durably, the temporary-file sweeps; every crash point and power cut of each (`MemFs`), on every core |
| `gaze-shard` | 24 | the deploy preimage matches `prost`; signatures verify; against mock nodes: a lying observer is outvoted, a split is an error, a rollback is stale, nothing is deployed before consent, and the body the validator receives verifies; freshness records kept per shard, merged to the highest block, read-only, surviving a restart, and a save that keeps failing reported once |
| `gaze-wallet` | 11 | addresses, wallet files and signature bytes identical to the Embers SDK's own output; the contract check refuses a changed recipient, amount or note, smuggled code, hidden fields, a high fee or another shard; wallets kept, exported, switched; transfers through an honest mock Embers, and nothing signed for a dishonest one; a damaged list keeps its lines, and wallets are found again from their keys |
| `gaze-reach` | 3 | the reach tier commits exactly the native executive's hashes for the same page and clicks; integrity; `shard` is dead |
| `gaze-shell` | 347 | the profile: the five roots on every platform, `settings.toml`, the instance lock, start-up's repair of every state of every file, the move of an old profile (with every crash point and two crashes in a row), `Profile::open` and its lock policy, the engine's read-only and blocked files, themes and the system's scheme; the command line (`tests/cli.rs`, on the real binary); the chrome: the action protocol and keys, markup conventions, text widths checked against Blitz's layout, panels, find, theme switches, the tab strip, search highlights, the mouse cursor and hover over pages, the sidebar at startup, a window resized by its frame painting one frame per size, the session and history files, the Appearance panel (System, Dark and Light, theme files, New theme and Reload), the scheme in pages and in the window's decorations, and the defect ledgers' regression tests (`docs/ui/ledger.md`, `docs/storage/ledger.md`) |

End-to-end, on the real binary:

- the new-tab page's f1r3lang lamp toggles on a click. In the window it lights
  in both schemes, and goes out on a second click (snapshot scenes 69–71);
- in the window: System follows the system's preference as the portal
  answers it, when the window regains the focus too, and starts within 3 s
  when the portal never answers; theme files are listed, chosen, created and
  reloaded; and a page's own light and dark styles follow the scheme shown
  (snapshot scenes 72–84);
- a site's `.knf` is fetched by `src`, integrity-checked, fetches over `net`,
  writes and reads `store`, logs, and saves a replay log; a tampered copy is
  refused with "integrity mismatch";
- in the window (Linux, Xvfb, Vello on lavapipe): tabs, the address bar with
  typed navigation, the console and grants panels, and a real click handled
  by the page — screenshots in `docs/screenshots/`;
- the wallet, through the whole stack against a mock node: a page's deploy is
  quoted naming the paying wallet, signed by it, and finalized; a program
  asking for `rho:rchain:deployerId` is refused before anything reaches the
  node; a wallet file in F1R3Sky's format imports and exports byte for byte;
- in the window: the wallet panel lists wallets with balances from Embers,
  switches the paying wallet, and sends a transfer after a confirming click
  (screenshots 06–08);
- the `.deb` installs and runs, the AppImage runs, and the signed checksums
  verify (and fail on a changed byte).

The storage checks (`docs/storage/README.md`, section 15): the TLA+ model of
start-up (`scripts/storage-model.sh`, 29 runs), 171 start-ups checked against
it (`scripts/storage-trace.sh`), 164 real kills of the binary checked against
it (`scripts/storage-kill.sh`), and a mutation check for every fix
(`scripts/mutation-check.py`).

CI (`.github/workflows/ci.yml`) runs the tests on Linux, macOS and Windows,
with a storage smoke test on each (`scripts/storage-smoke.sh`: a private
skeleton, and a second start that writes nothing) and, on Linux and macOS, an
old profile moved for real (`scripts/storage-migration-ci.sh`); clippy with
`-D warnings` in five configurations (the `lint` job); the model and the
traces (the `model` job); and the real kills on all three systems (the
`kill` job).

`scripts/ui-snapshots.sh` drives the real binary under Xvfb through 71 scenes
of the chrome. It writes the screenshots and their pixel checks (no page
reflow, no stale colours, legible labels, exact bars) to
`docs/screenshots/ui/{before,after}/`. The eight cursor scenes read the X
cursor itself through XFixes (`scripts/x-cursor.py`, which needs python-xlib):
it stays visible over pages, becomes a hand over links, and follows `cursor:
none`. See `docs/ui/README.md` §12.

## Deviations from the specification, for review


- **Ground values** are two node variants, `Lit` and `Coll` (a map's items
  alternate key and value), not one `Ground(Lit)`.
- **Authority of names.** A name obtained through another name inherits its
  root, so a page can navigate its own document (`parent`, `closest`). A new
  verb, `attenuate`, mints a name rooted at its own node; that is the name a page
  hands to a component to confine it. The specification's literal rule (every
  name rooted at itself) would make `parent` useless.
- **Commit hash.** The hash recorded per frame is of the committed document's
  serialisation, not of the write batch.
- **One executive per window thread.** Each tab's executive runs on the
  window thread, paced to 16 ms in the foreground and 250 ms in the background,
  rather than on a worker per tab.
- **The chrome is Rust-driven HTML.** Making it a f1r3lang page with a
  privileged capability is K1 (P4) work.
- **The bridge speaks the node's HTTP API** (`/api/deploy`, `/api/registry`,
  `/api/explore-deploy`, `/ws/events`) rather than Embers' gRPC
  `firefly-client`, which needs `protoc` and `tonic` at build time. Deploys are
  byte-for-byte what the node verifies.
- **Linux keys** are in a `0600` file in the data folder's `wallet/keys/`; macOS and Windows use
  the OS keychain (`os-keyring`, on in release builds).
- **One wallet signs for every site** (the spec's per-site keys are gone:
  the account that pays is the deployer). Sites can therefore link a user's
  deploys; users who want separate identities keep several wallets. Session
  keys remain as session identities but no longer sign.

## Not in this release

- U2 guards and U5 (the resolver seam): K2 and graded pages are refused at
  load, naming the work package.
- The `proof` rung (node work package N1) and the `replayed` rung: they answer
  `("err", "unavailable", …)`.
- `rho:serve` resolution depends on the VersionedRegistry step 6 on the node.
- Legacy JavaScript tabs, devtools time travel, the gateway.
- Opening `f1r3://` links from other macOS apps (Apple-event URL handling).
- Authenticating session messages to their service (a signed-message
  scheme verified on chain); today a session is identified, not proven.
- A `pay` capability for pages (a payment request the user approves in the
  wallet); pages cannot initiate transfers.
