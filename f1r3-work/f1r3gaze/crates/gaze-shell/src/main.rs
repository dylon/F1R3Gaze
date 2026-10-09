//! `f1r3gaze` — the browser.
//!
//! ```text
//! f1r3gaze [--profile DIR] [URL]     open a window
//! f1r3gaze [--profile DIR] --headless URL [--allow] [--click SELECTOR]...
//!          [--timeout SECS] [--wait SECS] [--log FILE.gzlog]
//!                                    run a page without a window and print
//!                                    its committed document and console
//! f1r3gaze [--profile DIR] wallet list|new [LABEL]|import FILE [LABEL]
//!                 |export ADDRESS [FILE]|use ADDRESS|remove ADDRESS
//!                 |balance [ADDRESS]|send TO AMOUNT [DESCRIPTION]
//!                                    manage the wallets that pay for deploys
//! f1r3gaze [--profile DIR] paths     print where the profile's folders are
//! f1r3gaze [--profile DIR] profile check
//!                                    say what a start would change, and
//!                                    change nothing
//! f1r3gaze [--profile DIR] profile backups [prune --older-than DAYS]
//!                                    list start-up's backups, or remove the
//!                                    ones older than DAYS
//! f1r3gaze [--profile DIR] trust list|forget BINDING|forget --all
//!                                    list or forget this shard's freshness
//!                                    records (after the shard was reset)
//! f1r3gaze compiler ARGS...          run the bundled f1r3c compiler
//! f1r3gaze --version
//! ```
//!
//! `--profile DIR` (or `F1R3GAZE_PROFILE`) keeps every folder under DIR:
//! `DIR/{config,data,state,cache,runtime}`. Otherwise the folders are the
//! platform's (docs/storage/README.md §3; `f1r3gaze paths` prints them).
//!
//! **Start-up**, in order (docs/storage/README.md §9.1): the roots are found
//! (`layout::locate`); `paths` stops there. The instance lock is taken as the
//! command needs it (`Command::locking`): the window, `--headless` and the
//! commands that change the profile need it free, and the commands that only
//! read go on read-only while another F1R3Gaze holds it. Then
//! `Profile::open`: the busy flag, the move of an old single-folder profile,
//! the folders, the repair of every managed file, and the settings. Then the
//! engine. `profile check` runs the same steps reading only, with no lock.
//!
//! **Exit status:** 0 when done; 1 when the command failed, or another
//! F1R3Gaze holds the profile this command must change (`profile check`: a
//! start would change something); 2 for a usage error.

use gaze_fs::{Fs, Perm, StdFs};
use gaze_shell::profile::backup::{list_sessions, prune_sessions};
use gaze_shell::profile::layout::{self, Layout, Machine};
use gaze_shell::profile::lock::Mode;
use gaze_shell::profile::migrate::{Action, Detected};
use gaze_shell::profile::reconcile::Managed;
use gaze_shell::profile::report::Severity;
use gaze_shell::profile::{Locking, Profile, StartEnv};
use gaze_shell::{Engine, headless, profile};
use gaze_shard::{FileFreshness, Forget, FreshnessLog};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// Was the usage until the switch-over (ledger S14), which named neither the
// wallet commands nor the profile's:
// fn usage() -> ! {
//     eprintln!(
//         "usage: f1r3gaze [--profile DIR] [URL]\n       f1r3gaze [--profile DIR] --headless URL [--allow] [--click SELECTOR]... [--timeout SECS] [--log FILE]"
//     );
//     std::process::exit(2)
// }

/// What `--help` and every usage error print.
const USAGE: &str = "usage: f1r3gaze [--profile DIR] [URL]
       f1r3gaze [--profile DIR] --headless URL [--allow] [--click SELECTOR]... [--timeout SECS] [--wait SECS] [--log FILE]
       f1r3gaze [--profile DIR] wallet list|new [LABEL]|import FILE [LABEL]|export ADDRESS [FILE]|use ADDRESS|remove ADDRESS|balance [ADDRESS]|send TO AMOUNT [NOTE]
       f1r3gaze [--profile DIR] paths | profile check | profile backups [prune --older-than DAYS] | trust list | trust forget BINDING|--all
       f1r3gaze profile import SOURCE    import an existing profile into a fresh macOS Store container
       f1r3gaze compiler ARGS...         run the bundled f1r3c compiler
       f1r3gaze --version
--profile DIR (or F1R3GAZE_PROFILE) keeps every folder under DIR.
Exit: 0 done; 1 failed, or the profile is in use (profile check: a start would change something); 2 usage.";

/// Prints why, if anything, then the usage; exits 2.
fn usage(why: &str) -> ! {
    if !why.is_empty() {
        eprintln!("f1r3gaze: {why}");
    }
    eprintln!("{USAGE}");
    std::process::exit(2)
}

fn wallet(eng: &gaze_shell::Engine, args: &[String]) -> Result<(), String> {
    use gaze_wallet::Address;
    let w = &eng.wallets;
    let arg = |i: usize| args.get(i).map(String::as_str);
    let need = |i: usize, what: &str| args.get(i).cloned().ok_or(format!("wallet {} needs {what}", args[0]));
    let chosen = |i: usize| -> Result<Address, String> {
        match arg(i) {
            Some(a) => Address::parse(a),
            None => w.active().ok_or_else(|| "no active wallet".to_string()),
        }
    };
    match args.first().map(String::as_str).unwrap_or("list") {
        "list" => {
            for (e, active) in w.list() {
                println!("{} {}  {}", if active { "*" } else { " " }, e.address, e.label);
            }
        }
        "new" => println!("{}", w.create(arg(1).unwrap_or(""))?),
        "import" => {
            let f = need(1, "a wallet file")?;
            let text = std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?;
            println!("{}", w.import(&text, arg(2).unwrap_or(""))?);
        }
        "export" => {
            let a = Address::parse(&need(1, "an address")?)?;
            let body = w.export(&a)?;
            match arg(2) {
                Some(f) => {
                    // A wallet file is a private key: owner-only from the start.
                    gaze_fs::write_atomic(&StdFs, Path::new(f), body.as_bytes(), Perm::Private)
                        .map_err(|e| format!("{f}: {e}"))?;
                    println!("wrote {f}");
                }
                None => println!("{body}"),
            }
        }
        "use" => w.set_active(&Address::parse(&need(1, "an address")?)?)?,
        "remove" => w.remove(&Address::parse(&need(1, "an address")?)?)?,
        "balance" => {
            let a = chosen(1)?;
            let s = w.state(&a)?;
            println!("{a}  {}", s.balance);
            for t in s.transfers.iter().rev().take(20) {
                println!("  {}  {} -> {}  {}  {}", t.timestamp, t.from, t.to, t.amount, t.description.as_deref().unwrap_or(""));
            }
        }
        "send" => {
            let from = w.active().ok_or("no active wallet")?;
            let to = Address::parse(&need(1, "a recipient")?)?;
            let amount: i64 = need(2, "an amount")?.parse().map_err(|_| "the amount must be a whole number")?;
            let id = w.transfer(&from, &to, amount, arg(3))?;
            println!("deploy {id}");
        }
        other => return Err(format!("unknown wallet command {other}")),
    }
    Ok(())
}

/// What the command line asks for.
#[derive(Debug, PartialEq)]
enum Command {
    /// The window, on a page or on the home page.
    Window { url: Option<String> },
    Headless { url: String, opts: HeadlessOptions, log: Option<String> },
    /// `wallet …`, with its arguments.
    Wallet(Vec<String>),
    Paths,
    ProfileCheck,
    ProfileImport(PathBuf),
    Backups,
    PruneBackups { older_than: Duration },
    TrustList,
    TrustForget(ForgetWhat),
}

/// `headless::Options`, comparable for the parser's tests.
#[derive(Debug, Default, PartialEq)]
struct HeadlessOptions {
    allow: bool,
    clicks: Vec<String>,
    timeout: Option<Duration>,
    wait: Option<Duration>,
}

/// What `trust forget` forgets.
#[derive(Debug, PartialEq)]
enum ForgetWhat {
    All,
    Binding(String),
}

/// The command line, read.
#[derive(Debug, PartialEq)]
enum Asked {
    Run { profile: Option<PathBuf>, command: Command },
    Compiler(Vec<String>),
    Version,
}

impl Command {
    /// How the command takes the instance lock (docs/storage/README.md §8).
    fn locking(&self) -> Locking {
        let only_if_free = |name| Locking::IfFree(Mode::Command(name));
        let exclusive = |name| Locking::Exclusive(Mode::Command(name));
        match self {
            Command::Window { .. } => Locking::Exclusive(Mode::Window),
            Command::Headless { .. } => Locking::Exclusive(Mode::Headless),
            Command::Wallet(args) => match args.first().map(String::as_str) {
                Some("new") => exclusive("wallet new"),
                Some("import") => exclusive("wallet import"),
                Some("use") => exclusive("wallet use"),
                Some("remove") => exclusive("wallet remove"),
                // list, balance, export (to a file the user names) and send
                // change nothing in the profile.
                _ => only_if_free("wallet"),
            },
            Command::Backups => only_if_free("profile backups"),
            Command::PruneBackups { .. } => exclusive("profile backups prune"),
            Command::TrustList => only_if_free("trust list"),
            Command::TrustForget(_) => exclusive("trust forget"),
            Command::Paths | Command::ProfileCheck => Locking::ReadOnly("this command only looks at the profile"),
            Command::ProfileImport(_) => unreachable!("profile import runs before the profile opens"),
        }
    }
}

/// Seconds, as `--timeout` and `--wait` take them.
fn seconds(option: &str, value: Option<&String>) -> Result<Duration, String> {
    let value = value.ok_or_else(|| format!("{option} needs a number of seconds"))?;
    value
        .parse()
        .map(Duration::from_secs)
        .map_err(|_| format!("{option} {value:?} is not a whole number of seconds"))
}

/// Reads the command line (without the program's name).
fn parse(args: &[String]) -> Result<Asked, String> {
    let mut profile: Option<PathBuf> = None;
    let mut url: Option<String> = None;
    let mut headless = false;
    let mut opts = HeadlessOptions::default();
    let mut log: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let value = args.get(i + 1);
        let page_options = headless || url.is_some() || opts != HeadlessOptions::default() || log.is_some();
        let rest: Vec<&str> = args[i + 1..].iter().map(String::as_str).collect();
        let command = match arg {
            "--version" | "-V" => return Ok(Asked::Version),
            "--help" | "-h" => return Err(String::new()),
            "--profile" => {
                profile = Some(PathBuf::from(value.ok_or("--profile needs a folder")?));
                i += 2;
                continue;
            }
            "--headless" => {
                headless = true;
                i += 1;
                continue;
            }
            "--allow" => {
                opts.allow = true;
                i += 1;
                continue;
            }
            "--click" => {
                opts.clicks.push(value.ok_or("--click needs a selector")?.clone());
                i += 2;
                continue;
            }
            "--timeout" => {
                opts.timeout = Some(seconds(arg, value)?);
                i += 2;
                continue;
            }
            "--wait" => {
                opts.wait = Some(seconds(arg, value)?);
                i += 2;
                continue;
            }
            "--log" => {
                log = Some(value.ok_or("--log needs a file")?.clone());
                i += 2;
                continue;
            }
            "wallet" | "paths" | "profile" | "trust" if page_options => {
                return Err(format!("{arg} takes no page and no page options"));
            }
            "compiler" if page_options || profile.is_some() => {
                return Err("compiler takes no profile, page or page options".into());
            }
            "compiler" => return Ok(Asked::Compiler(args[i + 1..].to_vec())),
            "wallet" => Command::Wallet(args[i + 1..].to_vec()),
            "paths" => match rest.as_slice() {
                [] => Command::Paths,
                _ => return Err("paths takes no arguments".into()),
            },
            "profile" => match rest.as_slice() {
                ["check"] => Command::ProfileCheck,
                ["import", source] if profile.is_none() => Command::ProfileImport(PathBuf::from(source)),
                ["backups"] => Command::Backups,
                ["backups", "prune", "--older-than", days] => {
                    let days: u64 = days.parse().map_err(|_| format!("--older-than {days:?} is not a whole number of days"))?;
                    let seconds = days.checked_mul(86_400).ok_or_else(|| format!("--older-than {days} is too many days"))?;
                    Command::PruneBackups {
                        older_than: Duration::from_secs(seconds),
                    }
                }
                _ => return Err("profile takes check, import SOURCE, backups, or backups prune --older-than DAYS".into()),
            },
            "trust" => match rest.as_slice() {
                ["list"] => Command::TrustList,
                ["forget", "--all"] => Command::TrustForget(ForgetWhat::All),
                ["forget", binding] if !binding.starts_with("--") => Command::TrustForget(ForgetWhat::Binding(binding.to_string())),
                _ => return Err("trust takes list, forget BINDING or forget --all".into()),
            },
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other => match url {
                None => {
                    url = Some(other.to_string());
                    i += 1;
                    continue;
                }
                Some(_) => return Err(format!("one page at a time: {other}")),
            },
        };
        return Ok(Asked::Run { profile, command });
    }
    let command = match (headless, url) {
        (true, Some(url)) => Command::Headless { url, opts, log },
        (true, None) => return Err("--headless needs a page".into()),
        (false, url) if opts == HeadlessOptions::default() && log.is_none() => Command::Window { url },
        (false, _) => return Err("--allow, --click, --timeout, --wait and --log go with --headless".into()),
    };
    Ok(Asked::Run { profile, command })
}

/// The file system start-up runs on: the real one, recorded line by line
/// into the file `F1R3GAZE_STORAGE_TRACE` names in a `storage-trace` build
/// (the real-kill checks; docs/storage/ledger.md, S8 part 2).
fn storage_fs() -> Arc<dyn Fs + Send + Sync> {
    #[cfg(feature = "storage-trace")]
    if let Some(file) = std::env::var_os("F1R3GAZE_STORAGE_TRACE") {
        let file = PathBuf::from(file);
        match gaze_fs::TraceFs::appending_to(Arc::new(StdFs), &file) {
            Ok(trace) => return Arc::new(trace),
            Err(e) => {
                eprintln!("f1r3gaze: {}: {e}", file.display());
                std::process::exit(2);
            }
        }
    }
    Arc::new(StdFs)
}

/// Launching the embedded helper from this executable lets it inherit the
/// signed app's sandbox. A direct Terminal launch of the helper has no app
/// parent and cannot reliably access the app container on macOS.
fn run_compiler(args: &[String]) -> i32 {
    let result = std::env::current_exe()
        .and_then(|exe| {
            let parent = exe.parent().ok_or_else(|| std::io::Error::other("executable has no parent folder"))?;
            Ok(parent.join(if cfg!(windows) { "f1r3c.exe" } else { "f1r3c" }))
        })
        .and_then(|compiler| std::process::Command::new(compiler).args(args).status());
    match result {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("f1r3gaze: cannot run bundled f1r3c: {error}");
            1
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_store_target<'a>(layout: &'a Layout, machine: &Machine) -> Option<&'a Path> {
    if matches!(layout.kind, layout::Kind::Platform(layout::Platform::MacOs))
        && machine.home.as_deref().is_some_and(layout::is_macos_container_home)
    {
        layout.config.parent()
    } else {
        None
    }
}

#[cfg(all(target_os = "macos", feature = "window"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportDecision {
    Import,
    Fresh,
    Cancel,
}

#[cfg(all(target_os = "macos", feature = "window"))]
trait ProfileImportDialog {
    fn decide(&self) -> ImportDecision;
    fn choose_source(&self) -> Option<PathBuf>;
    fn report_failure(&self, message: &str);
    fn report_success(&self);
}

#[cfg(all(target_os = "macos", feature = "window"))]
struct NativeProfileImportDialog {
    old_support_parent: Option<PathBuf>,
}

#[cfg(all(target_os = "macos", feature = "window"))]
impl ProfileImportDialog for NativeProfileImportDialog {
    fn decide(&self) -> ImportDecision {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSApplication};
        use objc2_foundation::NSString;

        let Some(mtm) = MainThreadMarker::new() else {
            return ImportDecision::Cancel;
        };
        let app = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        unsafe {
            app.activateIgnoringOtherApps(true);
        }
        let alert = unsafe { NSAlert::new(mtm) };
        unsafe {
            alert.setMessageText(&NSString::from_str("Import an existing F1R3Gaze profile?"));
            alert.setInformativeText(&NSString::from_str(concat!(
                "If you used the Developer ID or Homebrew version, select its F1R3Gaze profile folder. ",
                "Close that version before importing. Your existing files will be left in place. ",
                "Wallet keys stored in Keychain may still need to be exported from the old app and ",
                "imported here. Choose No to start with a new profile."
            )));
            alert.addButtonWithTitle(&NSString::from_str("Yes"));
            alert.addButtonWithTitle(&NSString::from_str("No"));
        }
        match unsafe { alert.runModal() } {
            NSAlertFirstButtonReturn => ImportDecision::Import,
            NSAlertSecondButtonReturn => ImportDecision::Fresh,
            _ => ImportDecision::Cancel,
        }
    }
    fn choose_source(&self) -> Option<PathBuf> {
        let mut picker = rfd::FileDialog::new()
            .set_title("Select the existing io.f1r3fly.f1r3gaze profile folder");
        if let Some(directory) = &self.old_support_parent {
            picker = picker.set_directory(directory);
        }
        picker.pick_folder()
    }
    fn report_failure(&self, message: &str) {
        rfd::MessageDialog::new()
            .set_title("F1R3Gaze profile import failed")
            .set_description(message)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
    fn report_success(&self) {
        rfd::MessageDialog::new()
            .set_title("F1R3Gaze profile imported")
            .set_description(concat!(
                "Your settings and profile files were copied. Wallet addresses were copied, but ",
                "Keychain keys may need to be exported from the old app and imported through ",
                "Choose wallet file in this app. Verify each address before using a wallet."
            ))
            .show();
    }
}

#[cfg(all(target_os = "macos", feature = "window"))]
fn offer_macos_store_import_with(
    layout: &Layout,
    machine: &Machine,
    dialog: &impl ProfileImportDialog,
) -> Result<(), String> {
    let Some(target) = macos_store_target(layout, machine) else {
        return Ok(());
    };
    if target.exists() {
        return Ok(());
    }
    match dialog.decide() {
        ImportDecision::Fresh => return Ok(()),
        ImportDecision::Import => {}
        ImportDecision::Cancel => {
            return Err("profile import was canceled; launch F1R3Gaze again to choose".into());
        }
    }
    let source = dialog
        .choose_source()
        .ok_or("profile folder selection was canceled; launch F1R3Gaze again to choose")?;
    if let Err(error) = profile::macos_import::import_existing(&source, target) {
        let message = format!(
            "The profile could not be imported: {error}\n\nYour existing profile was not changed. Close the old F1R3Gaze, check the selected folder, and try again."
        );
        dialog.report_failure(&message);
        return Err(message);
    }
    dialog.report_success();
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "window"))]
fn offer_macos_store_import(layout: &Layout, machine: &Machine) -> Result<(), String> {
    let old_support_parent = machine
        .home
        .as_deref()
        .filter(|home| layout::is_macos_container_home(home))
        .and_then(|home| home.ancestors().nth(4))
        .map(|home| home.join("Library/Application Support"));
    offer_macos_store_import_with(
        layout,
        machine,
        &NativeProfileImportDialog { old_support_parent },
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (profile_arg, command) = match parse(&args) {
        Ok(Asked::Version) => {
            println!("f1r3gaze {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Ok(Asked::Compiler(args)) => std::process::exit(run_compiler(&args)),
        Ok(Asked::Run { profile, command }) => (profile, command),
        Err(why) => usage(&why),
    };
    let machine = Machine::detect();
    let layout = match layout::locate(profile_arg, &machine) {
        Ok(layout) => layout,
        Err(e) => {
            eprintln!("f1r3gaze: {e}; use --profile DIR to keep F1R3Gaze's folders in DIR");
            std::process::exit(2);
        }
    };
    // `paths` touches no file: it only says where they are.
    if command == Command::Paths {
        print!("{}", layout.describe());
        return;
    }
    #[cfg(target_os = "macos")]
    if let Some(target) = macos_store_target(&layout, &machine) {
        if let Err(error) = profile::macos_import::finish_completed_import(target) {
            eprintln!("f1r3gaze: could not finish profile import cleanup: {error}");
            std::process::exit(1);
        }
    }
    if let Command::ProfileImport(source) = &command {
        #[cfg(target_os = "macos")]
        {
            let Some(target) = macos_store_target(&layout, &machine) else {
                eprintln!("f1r3gaze: profile import requires a fresh macOS App Store container");
                std::process::exit(2);
            };
            if let Err(error) = profile::macos_import::import_existing(source, target) {
                eprintln!("f1r3gaze: could not import profile: {error}");
                std::process::exit(1);
            }
            println!("Imported the existing profile into the App Store container.");
            return;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = source;
            eprintln!("f1r3gaze: profile import is available only in the macOS App Store build");
            std::process::exit(2);
        }
    }
    #[cfg(all(target_os = "macos", feature = "window"))]
    if matches!(command, Command::Window { .. }) {
        if let Err(error) = offer_macos_store_import(&layout, &machine) {
            eprintln!("f1r3gaze: {error}");
            std::process::exit(1);
        }
    }
    let fs = storage_fs();
    if command == Command::ProfileCheck {
        std::process::exit(profile_check(&layout, fs));
    }
    #[cfg(all(windows, feature = "window"))]
    let handoff_runtime = layout.runtime.clone();
    let profile = match Profile::open(layout, command.locking(), StartEnv::real(fs)) {
        Ok(profile) => profile,
        Err(e) => {
            #[cfg(all(windows, feature = "window"))]
            if let (
                Command::Window { url: Some(url) },
                profile::OpenError::Held(profile::lock::LockError::Held { holder, .. }),
            ) = (&command, &e)
            {
                if holder.mode.as_deref() == Some("window")
                    && gaze_shell::windows_url::forward(&handoff_runtime, url)
                {
                    return;
                }
            }
            eprintln!("f1r3gaze: {e}");
            std::process::exit(1);
        }
    };
    // The end of start-up, in a traced run (`storage-trace` builds): every
    // storage operation before it went through the trace, so the real-kill
    // checks crash only before it. The engine's own files (stores, wallets,
    // grants) are written with `StdFs`, untraced. A real file system ignores
    // notes.
    profile.fs().note("profile opened");
    let code = match command {
        Command::Window { url } => window(Engine::open(profile), url),
        Command::Headless { url, opts, log } => run_headless(Engine::open(profile), &url, opts, log),
        Command::Wallet(rest) => match wallet(&Engine::open(profile), &rest) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("f1r3gaze: {e}");
                1
            }
        },
        Command::Backups => backups(&profile),
        Command::PruneBackups { older_than } => prune(&profile, older_than),
        Command::TrustList => trust_list(&profile),
        Command::TrustForget(what) => trust_forget(&profile, &what),
        Command::Paths | Command::ProfileCheck | Command::ProfileImport(_) => {
            unreachable!("handled before the profile opens")
        }
    };
    std::process::exit(code);
}

fn window(eng: std::rc::Rc<Engine>, url: Option<String>) -> i32 {
    #[cfg(feature = "window")]
    {
        let home = eng.settings.home.clone();
        match gaze_shell::chrome::launch(eng, &url.unwrap_or(home)) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("f1r3gaze: {e}");
                1
            }
        }
    }
    #[cfg(not(feature = "window"))]
    {
        let _ = (eng, url);
        eprintln!("this build has no window; use --headless");
        2
    }
}

fn run_headless(eng: std::rc::Rc<Engine>, url: &str, opts: HeadlessOptions, log: Option<String>) -> i32 {
    let mut run = headless::Options {
        allow: opts.allow,
        clicks: opts.clicks,
        ..headless::Options::default()
    };
    if let Some(timeout) = opts.timeout {
        run.timeout = timeout;
    }
    if let Some(wait) = opts.wait {
        run.wait = wait;
    }
    let r = headless::run(eng, url, &run);
    println!("url:    {}", r.url);
    println!("stage:  {:?}", r.stage);
    println!("title:  {}", r.title);
    if let Some(n) = &r.notice {
        println!("notice: {n}");
    }
    for p in &r.prompts {
        println!("prompt: {p} -> {}", if run.allow { "allowed" } else { "denied" });
    }
    for (lvl, line) in &r.console {
        println!("console[{lvl}]: {line}");
    }
    println!("{}", r.document);
    // A replay log holds the page's inputs: owner-only. It is written where
    // the user asked, outside the profile.
    if let (Some(path), Some(bytes)) = (log, r.log)
        && let Err(e) = gaze_fs::write_atomic(&StdFs, Path::new(&path), &bytes, Perm::Private)
    {
        eprintln!("could not write {path}: {e}");
    }
    match r.stage {
        gaze_shell::tab::Stage::Failed(_) => 1,
        _ => 0,
    }
}

/// One line of a plan, for `profile check`.
fn action_line(action: &Action) -> String {
    match action {
        Action::Dir { path } => format!("make   {path}"),
        Action::Write { dst, from, .. } => format!("write  {dst} (converted from {})", from.join(", ")),
        Action::Move { src, dst, .. } => format!("move   {src} -> {dst}"),
        Action::MoveCacheShard { src, dst } => format!("move   {src} -> {dst} (cache)"),
        Action::Leave { path, why } => format!("leave  {path}: {why}"),
    }
}

/// `profile check`: what a start would do, done reading only. Exits 1 when
/// a start would change something, as `fsck -n` does when it finds work.
fn profile_check(layout: &Layout, fs: Arc<dyn Fs + Send + Sync>) -> i32 {
    let checked = profile::check(layout, fs, SystemTime::now());
    print!("{}", layout.describe());
    match &checked.detected {
        Ok(Detected::Current { .. }) => println!("profile: in this layout"),
        Ok(Detected::Fresh) => println!("profile: new; nothing to move"),
        Ok(Detected::Legacy { root }) => println!("profile: an old profile at {} would be moved", root.display()),
        Ok(Detected::Resume) => println!("profile: an unfinished move would be finished"),
        Ok(Detected::Newer { layout }) => println!("profile: set up by a newer F1R3Gaze (layout {layout}); a start only reads"),
        Err(e) => println!("profile: cannot be looked at: {e}"),
    }
    match &checked.plan {
        Some(Ok(plan)) => {
            println!("plan ({} steps):", plan.actions.len());
            for action in &plan.actions {
                println!("  {}", action_line(action));
            }
        }
        Some(Err(e)) => println!("plan: cannot be made: {e}"),
        None => {}
    }
    let changes = checked.changes();
    match changes.is_empty() {
        true => println!("a start would change nothing"),
        false => {
            println!("a start would change:");
            for change in &changes {
                println!("  {change}");
            }
        }
    }
    for event in checked.events.iter().filter(|e| e.severity >= Severity::Notice) {
        println!("{}: {}", event.severity.name(), event.message);
    }
    for diagnostic in &checked.settings.diagnostics {
        println!("setting: {diagnostic}");
    }
    i32::from(!changes.is_empty())
}

/// `profile backups`: every backup session, oldest first, and the total.
fn backups(profile: &Profile) -> i32 {
    match list_sessions(&profile.layout, profile.fs()) {
        Ok(sessions) => {
            let (mut files, mut bytes) = (0, 0);
            for s in &sessions {
                println!("{}\t{}\t{} files\t{} bytes", s.class.name(), s.path.display(), s.files, s.bytes);
                files += s.files;
                bytes += s.bytes;
            }
            println!("{} sessions, {files} files, {bytes} bytes", sessions.len());
            0
        }
        Err(e) => {
            eprintln!("f1r3gaze: the backups cannot be listed: {e}");
            1
        }
    }
}

/// `profile backups prune --older-than DAYS`: removes the sessions that
/// began longer ago; nothing else ever removes a backup.
fn prune(profile: &Profile, older_than: Duration) -> i32 {
    if let Some(why) = profile.read_only_reason() {
        eprintln!("f1r3gaze: no backup is removed: {why}");
        return 1;
    }
    match prune_sessions(&profile.layout, profile.fs(), older_than, SystemTime::now()) {
        Ok(removed) => {
            for s in &removed {
                println!("removed {}", s.path.display());
            }
            println!("{} sessions removed", removed.len());
            0
        }
        Err(e) => {
            eprintln!("f1r3gaze: the backups cannot be pruned: {e}");
            1
        }
    }
}

/// The freshness records of the shard the settings name.
fn records(profile: &Profile) -> FileFreshness {
    let records = FileFreshness::open(profile.layout.trust_file(), &profile.settings.shard.shard_id);
    match profile.blocked.why(Managed::Freshness) {
        Some(why) => records.read_only(why),
        None => records,
    }
}

/// `trust list`: this shard's records, by binding.
fn trust_list(profile: &Profile) -> i32 {
    let shard = &profile.settings.shard.shard_id;
    if gaze_shell::engine::observers_are_loopback(&profile.settings.shard.observers) {
        println!("(the observers run on this machine: records of shard {shard} are kept in memory only, so these are from earlier sessions)");
    }
    let records = records(profile);
    if let Some(why) = records.blocked() {
        println!("(the records cannot be changed: {why})");
    }
    let all = records.load();
    for (binding, block) in &all {
        println!("{block}\t{binding}");
    }
    println!("{} records of shard {shard}", all.len());
    0
}

/// `trust forget BINDING|--all`: for a shard that was reset, whose new
/// blocks would otherwise all be refused as replays.
fn trust_forget(profile: &Profile, what: &ForgetWhat) -> i32 {
    let shard = profile.settings.shard.shard_id.clone();
    let which = match what {
        ForgetWhat::All => Forget::All,
        ForgetWhat::Binding(binding) => Forget::Binding(binding),
    };
    match records(profile).forget(which) {
        Ok(n) => {
            println!("forgot {n} records of shard {shard}");
            0
        }
        Err(e) => {
            eprintln!("f1r3gaze: {e}");
            1
        }
    }
}

// Disabled at the switch-over (ledger S14): every command opened the single
// profile folder (`Engine::new(dir)`) with no instance lock and no start-up.
// `main` above finds the roots, takes the lock as the command needs it, and
// opens the profile in start-up's order (`Profile::open`).
// fn main() {
//     let mut args = std::env::args().skip(1);
//     let mut dir = profile::default_dir();
//     let mut url: Option<String> = None;
//     let mut is_headless = false;
//     let mut opts = headless::Options::default();
//     let mut log: Option<String> = None;
//     while let Some(a) = args.next() {
//         match a.as_str() {
//             "--version" | "-V" => {
//                 println!("f1r3gaze {}", env!("CARGO_PKG_VERSION"));
//                 return;
//             }
//             "--help" | "-h" => usage(),
//             "--profile" => dir = args.next().unwrap_or_else(|| usage()).into(),
//             "--headless" => is_headless = true,
//             "--allow" => opts.allow = true,
//             "--click" => opts.clicks.push(args.next().unwrap_or_else(|| usage())),
//             "--timeout" => opts.timeout = Duration::from_secs(args.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())),
//             "--log" => log = Some(args.next().unwrap_or_else(|| usage())),
//             "--wait" => opts.wait = Duration::from_secs(args.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())),
//             "wallet" => {
//                 let rest: Vec<String> = args.by_ref().collect();
//                 let eng = Engine::new(dir.clone());
//                 if let Err(e) = wallet(&eng, &rest) {
//                     eprintln!("f1r3gaze: {e}");
//                     std::process::exit(1);
//                 }
//                 return;
//             }
//             s if s.starts_with("--") => usage(),
//             s => url = Some(s.to_string()),
//         }
//     }
//     let eng = Engine::new(dir);
//     if is_headless {
//         let url = url.unwrap_or_else(|| usage());
//         let r = headless::run(eng, &url, &opts);
//         println!("url:    {}", r.url);
//         println!("stage:  {:?}", r.stage);
//         println!("title:  {}", r.title);
//         if let Some(n) = &r.notice {
//             println!("notice: {n}");
//         }
//         for p in &r.prompts {
//             println!("prompt: {p} -> {}", if opts.allow { "allowed" } else { "denied" });
//         }
//         for (lvl, line) in &r.console {
//             println!("console[{lvl}]: {line}");
//         }
//         println!("{}", r.document);
//         // A replay log holds the page's inputs: owner-only.
//         if let (Some(path), Some(bytes)) = (log, r.log)
//             && let Err(e) = gaze_fs::write_atomic(&StdFs, Path::new(&path), &bytes, Perm::Private)
//         {
//             eprintln!("could not write {path}: {e}");
//         }
//         let code = if matches!(r.stage, gaze_shell::tab::Stage::Failed(_)) { 1 } else { 0 };
//         std::process::exit(code);
//     }
//     #[cfg(feature = "window")]
//     {
//         let home = eng.settings.home.clone();
//         if let Err(e) = gaze_shell::chrome::launch(eng, &url.unwrap_or(home)) {
//             eprintln!("f1r3gaze: {e}");
//             std::process::exit(1);
//         }
//     }
//     #[cfg(not(feature = "window"))]
//     {
//         let _ = url;
//         eprintln!("this build has no window; use --headless");
//         std::process::exit(2);
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;

    fn read(line: &str) -> Result<Asked, String> {
        let args: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        parse(&args)
    }

    fn command(line: &str) -> Command {
        match read(line) {
            Ok(Asked::Run { command, .. }) => command,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[cfg(all(target_os = "macos", feature = "window"))]
    struct FakeImportDialog {
        decision: ImportDecision,
        source: Option<PathBuf>,
        decisions: std::cell::Cell<usize>,
        selections: std::cell::Cell<usize>,
        failures: std::cell::Cell<usize>,
        successes: std::cell::Cell<usize>,
    }

    #[cfg(all(target_os = "macos", feature = "window"))]
    impl FakeImportDialog {
        fn new(decision: ImportDecision, source: Option<PathBuf>) -> Self {
            Self {
                decision,
                source,
                decisions: std::cell::Cell::new(0),
                selections: std::cell::Cell::new(0),
                failures: std::cell::Cell::new(0),
                successes: std::cell::Cell::new(0),
            }
        }
    }

    #[cfg(all(target_os = "macos", feature = "window"))]
    impl ProfileImportDialog for FakeImportDialog {
        fn decide(&self) -> ImportDecision {
            self.decisions.set(self.decisions.get() + 1);
            self.decision
        }
        fn choose_source(&self) -> Option<PathBuf> {
            self.selections.set(self.selections.get() + 1);
            self.source.clone()
        }
        fn report_failure(&self, _: &str) {
            self.failures.set(self.failures.get() + 1);
        }
        fn report_success(&self) {
            self.successes.set(self.successes.get() + 1);
        }
    }

    #[cfg(all(target_os = "macos", feature = "window"))]
    fn store_import_fixture(name: &str) -> (Machine, Layout, PathBuf, PathBuf) {
        let base = gaze_fs::scratch_dir(name);
        let home = base.join("home/Library/Containers/io.f1r3fly.f1r3gaze/Data");
        std::fs::create_dir_all(&home).unwrap();
        let machine = Machine { home: Some(home), ..Machine::default() };
        let layout = layout::platform_layout(layout::Platform::MacOs, &machine).unwrap();
        let source = base.join("old/io.f1r3fly.f1r3gaze");
        for root in ["config", "data", "state"] {
            std::fs::create_dir_all(source.join(root)).unwrap();
        }
        std::fs::write(
            source.join("data/layout.json"),
            profile::migrate::Marker::repaired().to_bytes(),
        )
        .unwrap();
        std::fs::write(source.join("config/imported"), b"consented").unwrap();
        let target = layout.config.parent().unwrap().to_path_buf();
        (machine, layout, source, target)
    }

    #[cfg(all(target_os = "macos", feature = "window"))]
    #[test]
    fn store_import_requires_a_positive_folder_choice() {
        let (machine, layout, source, target) = store_import_fixture("gaze-store-import-choice");
        let fresh = FakeImportDialog::new(ImportDecision::Fresh, Some(source.clone()));
        offer_macos_store_import_with(&layout, &machine, &fresh).unwrap();
        assert_eq!((fresh.decisions.get(), fresh.selections.get()), (1, 0));
        assert!(!target.exists());

        let canceled = FakeImportDialog::new(ImportDecision::Cancel, Some(source.clone()));
        assert!(offer_macos_store_import_with(&layout, &machine, &canceled).is_err());
        assert_eq!((canceled.decisions.get(), canceled.selections.get()), (1, 0));
        assert!(!target.exists());

        let no_folder = FakeImportDialog::new(ImportDecision::Import, None);
        assert!(offer_macos_store_import_with(&layout, &machine, &no_folder).is_err());
        assert_eq!((no_folder.decisions.get(), no_folder.selections.get()), (1, 1));
        assert!(!target.exists());

        let bad_folder = FakeImportDialog::new(ImportDecision::Import, Some(source.join("missing")));
        assert!(offer_macos_store_import_with(&layout, &machine, &bad_folder).is_err());
        assert_eq!(bad_folder.failures.get(), 1);
        assert!(!target.exists());
        assert!(source.join("data/layout.json").exists());
    }

    #[cfg(all(target_os = "macos", feature = "window"))]
    #[test]
    fn store_import_preserves_source_and_prompts_only_for_a_fresh_container() {
        let (machine, layout, source, target) = store_import_fixture("gaze-store-import-consented");
        let selected = FakeImportDialog::new(ImportDecision::Import, Some(source.clone()));
        offer_macos_store_import_with(&layout, &machine, &selected).unwrap();
        assert_eq!((selected.decisions.get(), selected.selections.get(), selected.successes.get()), (1, 1, 1));
        assert_eq!(std::fs::read(source.join("config/imported")).unwrap(), b"consented");
        assert_eq!(std::fs::read(target.join("config/imported")).unwrap(), b"consented");
        offer_macos_store_import_with(&layout, &machine, &selected).unwrap();
        assert_eq!(selected.decisions.get(), 1);
    }

    #[test]
    fn commands_read_back() {
        assert_eq!(command(""), Command::Window { url: None });
        assert_eq!(command("https://example.org/"), Command::Window { url: Some("https://example.org/".into()) });
        assert_eq!(
            command("--headless gaze://newtab --click #lamp --timeout 5 --allow"),
            Command::Headless {
                url: "gaze://newtab".into(),
                opts: HeadlessOptions {
                    allow: true,
                    clicks: vec!["#lamp".into()],
                    timeout: Some(Duration::from_secs(5)),
                    wait: None,
                },
                log: None,
            }
        );
        assert_eq!(command("wallet list"), Command::Wallet(vec!["list".into()]));
        assert_eq!(command("paths"), Command::Paths);
        assert_eq!(command("profile check"), Command::ProfileCheck);
        assert_eq!(command("profile import /old"), Command::ProfileImport(PathBuf::from("/old")));
        assert_eq!(command("profile backups"), Command::Backups);
        assert_eq!(
            command("profile backups prune --older-than 30"),
            Command::PruneBackups { older_than: Duration::from_secs(30 * 86_400) }
        );
        assert_eq!(command("trust list"), Command::TrustList);
        assert_eq!(command("trust forget --all"), Command::TrustForget(ForgetWhat::All));
        assert_eq!(command("trust forget rho:id:abc"), Command::TrustForget(ForgetWhat::Binding("rho:id:abc".into())));
        assert_eq!(read("--version"), Ok(Asked::Version));
        assert_eq!(read("compiler compile file.rho -o file.knf"), Ok(Asked::Compiler(vec![
            "compile".into(), "file.rho".into(), "-o".into(), "file.knf".into(),
        ])));
        match read("--profile /p wallet new Savings") {
            Ok(Asked::Run { profile, command }) => {
                assert_eq!(profile.as_deref(), Some(Path::new("/p")));
                assert_eq!(command, Command::Wallet(vec!["new".into(), "Savings".into()]));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn usage_errors_are_named() {
        for (line, says) in [
            ("--headless", "needs a page"),
            ("--timeout x", "not a whole number"),
            ("--profile", "needs a folder"),
            ("--click", "needs a selector"),
            ("--allow https://example.org/", "go with --headless"),
            ("https://a.example/ https://b.example/", "one page at a time"),
            ("https://a.example/ wallet list", "takes no page"),
            ("--headless x paths", "takes no page"),
            ("paths now", "no arguments"),
            ("profile", "check, import"),
            ("--profile /p profile import /old", "check, import"),
            ("profile backups prune --older-than many", "not a whole number of days"),
            ("profile backups prune --older-than 999999999999999999", "too many days"),
            ("trust", "list, forget"),
            ("trust forget", "list, forget"),
            ("--bogus", "unknown option"),
            ("--profile /p compiler --version", "takes no profile"),
            ("--headless x compiler compile a.rho", "takes no profile, page"),
        ] {
            let why = read(line).expect_err(line);
            assert!(why.contains(says), "{line}: {why}");
        }
        assert_eq!(read("--help"), Err(String::new()));
    }

    #[test]
    fn only_commands_that_change_the_profile_need_it_free() {
        let exclusive = |c: &Command| matches!(c.locking(), Locking::Exclusive(_));
        let if_free = |c: &Command| matches!(c.locking(), Locking::IfFree(_));
        for line in ["", "--headless gaze://newtab", "wallet new", "wallet import f", "wallet use a", "wallet remove a", "profile backups prune --older-than 1", "trust forget --all"] {
            assert!(exclusive(&command(line)), "{line}");
        }
        for line in ["wallet", "wallet list", "wallet balance", "wallet export a f", "wallet send a 1", "profile backups", "trust list"] {
            assert!(if_free(&command(line)), "{line}");
        }
        for line in ["paths", "profile check"] {
            assert!(matches!(command(line).locking(), Locking::ReadOnly(_)), "{line}");
        }
    }
}
