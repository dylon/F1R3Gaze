//! Reading a real run's trace back (ledger S8, part 2): the lines a
//! `TraceFs` appends to a file, and the plan the run noted.

use super::*;
use crate::profile::backup::Backups;
use crate::profile::layout::{Machine, Platform, platform_layout};
use crate::profile::migrate::convert::LEGACY_TEMPLATE;
use crate::profile::migrate::{self, Migration};
use crate::profile::reconcile::{self, Start};
use crate::profile::report::Report;
use crate::profile::{Access, KeystoreKind};
use gaze_fs::{Fs, MemFs, TraceFs};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// The XDG layout of a user whose home is /home/u: the old profile is
/// /home/u/.local/share/f1r3gaze.
fn xdg() -> Layout {
    let machine = Machine {
        home: Some(PathBuf::from("/home/u")),
        env: BTreeMap::new(),
        roaming_app_data: None,
        local_app_data: None,
    };
    platform_layout(Platform::Xdg, &machine).expect("an XDG layout")
}

/// One start in main.rs's order on `shared`: begin, the migration, prepare,
/// reconcile and finish.
fn start(l: &Layout, shared: Arc<dyn Fs + Send + Sync>, seconds: u64) -> Migration {
    let fs = shared.as_ref();
    let report = Report::silent();
    let now = UNIX_EPOCH + Duration::from_secs(seconds);
    let backups = Backups::new(Arc::new(l.clone()), shared.clone(), now);
    let new_id = || "00112233445566778899aabbccddeeff".to_string();
    migrate::begin(l, fs).expect("begin");
    let start = Start {
        layout: l,
        fs,
        backups: &backups,
        report: &report,
        access: &Access::Write,
        keystore: KeystoreKind::File,
        new_user_id: &new_id,
    };
    let migration = migrate::run(&start, now);
    reconcile::prepare(&start).expect("prepare");
    reconcile::reconcile(&start);
    migrate::finish(l, fs).expect("finish");
    migration
}

/// Every record a migration's trace appended reads back as the operation it
/// recorded, and the plan it noted is the plan the start made.
#[test]
fn json_lines_read_back() {
    let l = xdg();
    let old = l.legacy[0].clone();
    let mem = Arc::new(MemFs::new());
    mem.seed_file(old.join("settings.conf"), LEGACY_TEMPLATE.as_bytes());
    mem.seed_file(old.join("user-id"), b"5f1c2d3e4b5a69788796a5b4c3d2e1f0");
    mem.seed_file(old.join("grants.tsv"), b"");
    let seconds = 1_791_224_462;
    let planned = migrate::plan(&l, mem.as_ref(), &old, UNIX_EPOCH + Duration::from_secs(seconds)).expect("a plan");
    let file = gaze_fs::scratch_dir("gaze-shell-trace-ndjson").join("events.ndjson");
    let _ = std::fs::remove_file(&file);
    let trace = Arc::new(TraceFs::appending_to(mem.clone(), &file).expect("a trace file"));
    let migration = start(&l, trace.clone(), seconds);
    assert!(matches!(migration, Migration::Done(_)), "{migration:?}");
    let recorded: Vec<Traced> = trace.log();
    assert!(recorded.iter().any(|t| t.error.is_some()), "the run has failed operations to read back");
    assert!(recorded.iter().any(|t| t.fnv.is_some() && t.mode.is_some()), "and created files");
    let text = std::fs::read_to_string(&file).expect("the trace file");
    let events = events_of_ndjson(&text).expect("the lines read back");
    assert_eq!(events.len(), recorded.len());
    for (event, traced) in events.iter().zip(&recorded) {
        match event {
            Event::Op(read) => assert_eq!(read, traced),
            other => panic!("{other:?} is not {traced:?}"),
        }
    }
    assert_eq!(plan_of(&events), Ok(planned), "the plan the start noted");
}

/// A process the test killed is one line appended after its records.
#[test]
fn a_killed_process_reads_back_as_a_crash() {
    let text = concat!(
        "{\"n\":0,\"op\":\"note\",\"what\":\"barrier-begin\"}\n",
        "{\"n\":1,\"op\":\"sync_dir\",\"path\":\"/p\",\"ok\":true}\n",
        "{\"op\":\"crash\",\"kind\":\"process\"}\n",
        "\n",
        "{\"n\":0,\"op\":\"kind\",\"path\":\"/p/data/layout.json\",\"ok\":false,\"err\":\"NotFound\"}\n",
        "{\"n\":1,\"op\":\"same_contents\",\"path\":\"/a\",\"to\":\"/b\",\"equal\":false,\"ok\":true}\n",
        "{\"n\":2,\"op\":\"remove_file\",\"path\":\"/a\",\"ok\":false,\"err\":\"SomethingNew\"}\n",
    );
    let events = events_of_ndjson(text).expect("the lines read back");
    assert_eq!(events.len(), 6, "blank lines are skipped");
    assert!(matches!(events[2], Event::ProcessCrash));
    let Event::Op(missing) = &events[3] else { panic!("{:?}", events[3]) };
    assert_eq!((missing.op, missing.error), ("kind", Some(std::io::ErrorKind::NotFound)));
    let Event::Op(compared) = &events[4] else { panic!("{:?}", events[4]) };
    assert_eq!((compared.to.as_deref(), compared.equal), (Some(Path::new("/b")), Some(false)));
    let Event::Op(unknown) = &events[5] else { panic!("{:?}", events[5]) };
    assert_eq!(unknown.error, Some(std::io::ErrorKind::Other), "an unknown kind still failed");
    assert_eq!(plan_of(&events), Err("the run replayed no plan".to_string()));
}

/// A line that is not a record is refused, naming its line.
#[test]
fn lines_that_are_not_records_are_refused() {
    for (line, says) in [
        ("{\"op\":\"crash\",\"kind\":\"power\"}", "a crash of kind"),
        ("{\"n\":0,\"op\":\"truncate\",\"path\":\"/a\",\"ok\":true}", "no operation of a file system"),
        ("{\"n\":0,\"op\":\"read\",\"path\":\"/a\"}", "ok None"),
        ("{\"n\":0,\"op\":\"create_new\",\"path\":\"/a\",\"fnv\":\"xyz\",\"ok\":true}", "fnv"),
        ("{\"n\":0,\"op\":\"read\",\"path\":7,\"ok\":true}", "not a string"),
        ("{\"op\":\"read\",\"path\":\"/a\",\"ok\":true}", "no n"),
        ("not json", "line 2"),
    ] {
        let text = format!("{{\"n\":0,\"op\":\"note\",\"what\":\"x\"}}\n{line}\n");
        let refused = events_of_ndjson(&text).expect_err(line);
        assert!(refused.starts_with("line 2: "), "{refused}");
        assert!(refused.contains(says), "{line}: {refused}");
    }
}
