//! What start-up found and did to the profile's files.
//!
//! Every event is printed on stderr from [`Severity::Notice`] up, and the
//! window shows the ones it has not shown yet ([`Report::take_unshown`]),
//! so a repaired or regenerated file never goes unnoticed: its message names
//! the backup that holds the original.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// How much an event matters to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Routine: a default file written on a first launch, an example
    /// refreshed. Kept, not shown.
    Quiet,
    /// Worth knowing: a migration done, a torn record dropped after a copy
    /// was kept.
    Notice,
    /// Something was repaired or replaced; the original is in a backup.
    Warning,
    /// Security or data at risk: a damaged key, a regenerated user id, reset
    /// freshness records, an unfinished migration.
    Alert,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Quiet => "quiet",
            Severity::Notice => "notice",
            Severity::Warning => "warning",
            Severity::Alert => "alert",
        }
    }
}

/// What happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// A missing file was written from its default.
    Created,
    /// An `.example` file was rewritten with the current built-in text.
    Refreshed,
    /// A file kept its usable lines; the rest are in the backup.
    Repaired { kept: usize, dropped: usize },
    /// A damaged file was replaced by its default; the original is in the
    /// backup.
    Regenerated { why: String },
    /// A file could not be read, and was left as it is.
    Unreadable { error: String },
    /// A setting could not be used, and keeps its default.
    Setting,
    /// A wallet key file cannot be read as a key. It is never changed.
    CorruptKey { wallet: Option<String> },
    /// A wallet was listed again from its key file.
    Recovered { address: String },
    /// A site's damaged store was copied, then cut back to its readable
    /// records.
    Salvaged { site: String, kept: u64, torn_tail: bool },
    /// An old single-folder profile was moved into the new layout.
    Migrated { from: PathBuf, moved: usize, conflicts: usize },
    /// A migration could not finish; it resumes at the next start.
    MigrationPending { error: String },
    /// The session cannot write the profile.
    ReadOnly { why: String },
    /// The platform's runtime directory is not available; the lock lives
    /// under the state directory instead.
    RuntimeFallback,
    /// The instance lock could not be taken for a reason other than another
    /// F1R3Gaze holding it.
    LockUnavailable { error: String },
    /// Temporary files of writers that died were removed from a folder.
    Swept { removed: usize },
    /// An item was not moved: something else is already at its new place.
    /// Both are kept.
    Conflict { other: PathBuf },
    /// A file written by a newer F1R3Gaze, in a format this one does not
    /// know. It is left as it is.
    Newer { version: u32, known: u32 },
    /// A file start-up could not repair or create: its backup or its new
    /// content could not be written, or it kept changing while start-up
    /// looked at it. It is left as it is, and nothing is saved to it this
    /// session.
    Unrepaired { error: String },
    /// A listed wallet has no key file: it cannot pay.
    MissingKey { wallet: String },
    /// An unfinished save holds the only copy of a wallet's key.
    StrayKey { wallet: String },
    /// A file was written in a way the rest of the event explains.
    Other,
}

/// One thing that happened to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: EventKind,
    pub severity: Severity,
    /// The file or directory it is about.
    pub path: PathBuf,
    /// Where the original went, when it was backed up.
    pub backup: Option<PathBuf>,
    /// A sentence for the user.
    pub message: String,
}

impl Event {
    pub fn new(kind: EventKind, severity: Severity, path: impl Into<PathBuf>, message: impl Into<String>) -> Event {
        Event {
            kind,
            severity,
            path: path.into(),
            backup: None,
            message: message.into(),
        }
    }

    pub fn with_backup(mut self, backup: impl Into<PathBuf>) -> Event {
        self.backup = Some(backup.into());
        self
    }

    /// The line stderr gets: `f1r3gaze: <severity>: <message>`.
    pub fn line(&self) -> String {
        format!("f1r3gaze: {}: {}", self.severity.name(), self.message)
    }
}

#[derive(Debug, Default)]
struct Inner {
    events: Vec<Event>,
    shown: usize,
    echo: bool,
}

/// The events of one start, shared by the threads that open the profile's
/// files.
#[derive(Clone, Debug)]
pub struct Report(Arc<Mutex<Inner>>);

impl Default for Report {
    fn default() -> Report {
        Report::new()
    }
}

impl Report {
    /// A report that also prints every event from [`Severity::Notice`] up on
    /// stderr.
    pub fn new() -> Report {
        Report(Arc::new(Mutex::new(Inner {
            events: Vec::with_capacity(16),
            shown: 0,
            echo: true,
        })))
    }

    /// A report that prints nothing, for tests.
    pub fn silent() -> Report {
        let report = Report::new();
        report.lock().echo = false;
        report
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().expect("the report's lock is never poisoned")
    }

    pub fn push(&self, event: Event) {
        let mut inner = self.lock();
        if inner.echo && event.severity >= Severity::Notice {
            eprintln!("{}", event.line());
        }
        inner.events.push(event);
    }

    /// Every event so far.
    pub fn events(&self) -> Vec<Event> {
        self.lock().events.clone()
    }

    /// The events from [`Severity::Notice`] up that the window has not shown
    /// yet; each is handed out once.
    pub fn take_unshown(&self) -> Vec<Event> {
        let mut inner = self.lock();
        let unshown: Vec<Event> = inner.events[inner.shown..]
            .iter()
            .filter(|e| e.severity >= Severity::Notice)
            .cloned()
            .collect();
        inner.shown = inner.events.len();
        unshown
    }

    /// How many events are at least `severity`.
    pub fn count(&self, severity: Severity) -> usize {
        self.lock().events.iter().filter(|e| e.severity >= severity).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_shown_once_and_quiet_ones_never() {
        let report = Report::silent();
        report.push(Event::new(EventKind::Created, Severity::Quiet, "/c/settings.toml", "created"));
        report.push(
            Event::new(EventKind::Regenerated { why: "syntax".into() }, Severity::Warning, "/c/settings.toml", "replaced")
                .with_backup("/c/backups/x/settings.toml"),
        );
        let shown = report.take_unshown();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].backup.as_deref(), Some(std::path::Path::new("/c/backups/x/settings.toml")));
        assert!(report.take_unshown().is_empty(), "each event is shown once");
        report.push(Event::new(EventKind::RuntimeFallback, Severity::Notice, "/s/runtime", "fallback"));
        assert_eq!(report.take_unshown().len(), 1);
        assert_eq!(report.events().len(), 3);
        assert_eq!(report.count(Severity::Notice), 2);
        assert_eq!(shown[0].line(), "f1r3gaze: warning: replaced");
    }
}
