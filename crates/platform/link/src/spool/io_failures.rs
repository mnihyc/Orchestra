//! Actual kernel write and sync failures must not turn a later success into lost history.
#![allow(clippy::unwrap_used, reason = "fault fixtures fail by panicking")]
use super::*;
use serde_json::json;

fn options() -> SpoolOptions {
    SpoolOptions {
        max_bytes: 1024 * 1024,
        max_age: Duration::from_secs(86_400),
        segment_bytes: 1024 * 1024,
    }
}
fn event(text: &str) -> SpoolEvent {
    SpoolEvent {
        at: "2026-10-13T08:00:00.000Z".parse().unwrap(),
        kind: "item.completed".into(),
        body: json!({"text":text}),
    }
}
fn full() -> File {
    OpenOptions::new().write(true).open("/dev/full").unwrap()
}

#[test]
fn a_failed_capture_floor_write_remains_owed_when_the_same_floor_is_requested_again() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = Spool::open(directory.path(), options()).unwrap().0;
    let original = spool.metadata_root.clone();
    let obstruction = directory.path().join("not-a-directory");
    std::fs::write(&obstruction, b"fixture").unwrap();
    spool.metadata_root = obstruction;
    assert!(spool.raw_after("machine", 17).is_err());
    assert!(
        spool.raw_after("machine", 17).is_err(),
        "an in-memory floor is not a successful durability receipt"
    );
    spool.metadata_root = original;
    spool.raw_after("machine", 17).unwrap();
    drop(spool);
    let spool = Spool::open(directory.path(), options()).unwrap().0;
    assert_eq!(spool.raw_cursors.get("machine"), Some(&17));
}

#[test]
fn a_finished_force_never_clears_the_durability_of_a_newer_append() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = Spool::open(directory.path(), options()).unwrap().0;
    spool.append("machine", &event("before")).unwrap();
    let before = spool.flush_for_sync().unwrap();
    spool.append("machine", &event("after")).unwrap();
    before.force().unwrap();
    spool.complete_sync(&before);
    assert!(
        spool.is_dirty(),
        "a receipt proves only the revision it captured"
    );
    spool.sync().unwrap();
    assert!(!spool.is_dirty());
}

#[test]
fn a_failed_sync_keeps_the_event_dirty_until_the_same_bytes_are_durable() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = Spool::open(directory.path(), options()).unwrap().0;
    spool.append("machine", &event("kept")).unwrap();
    let log = &mut spool.streams.get_mut("machine").unwrap().main;
    log.flush().unwrap();
    let (id, writer) = log.writer.take().unwrap();
    log.writer = Some((id, BufWriter::new(full())));
    assert!(
        spool.sync().is_err(),
        "the kernel refuses fsync on the fault device"
    );
    assert!(spool.is_dirty(), "failed durability is still owed");
    spool.streams.get_mut("machine").unwrap().main.writer = Some((id, writer));
    spool.sync().unwrap();
    assert!(!spool.is_dirty());
    assert_eq!(spool.number("machine", 10).unwrap()[0].event, event("kept"));
}

#[test]
fn a_later_flush_failure_does_not_forget_an_earlier_streams_durability() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = Spool::open(directory.path(), options()).unwrap().0;
    for stream in ["a", "b"] {
        spool.append(stream, &event(stream)).unwrap();
    }
    let log = &mut spool.streams.get_mut("b").unwrap().main;
    log.flush().unwrap();
    let (id, writer) = log.writer.take().unwrap();
    let mut fault = BufWriter::new(full());
    fault.write_all(b"buffered flush fault").unwrap();
    log.writer = Some((id, fault));
    assert!(spool.flush_for_sync().is_err());
    assert!(
        spool.streams["a"].main.dirty,
        "the earlier file still needs a durability receipt"
    );
    assert!(spool.streams["b"].main.dirty);
    spool.streams.get_mut("b").unwrap().main.writer = Some((id, writer));
    spool.sync().unwrap();
    assert!(!spool.is_dirty());
}

#[test]
fn a_full_device_before_any_record_bytes_can_recover_without_losing_earlier_or_later_events() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = Spool::open(directory.path(), options()).unwrap().0;
    spool.append("machine", &event("before")).unwrap();
    spool.sync().unwrap();
    let log = &mut spool.streams.get_mut("machine").unwrap().main;
    let (id, writer) = log.writer.take().unwrap();
    log.writer = Some((id, BufWriter::new(full())));
    assert!(
        matches!(spool.append("machine",&event(&"x".repeat(64*1024))),Err(SpoolError::Io {source,..}) if source.raw_os_error()==Some(28))
    );
    spool.streams.get_mut("machine").unwrap().main.writer = Some((id, writer));
    spool.append("machine", &event("after")).unwrap();
    spool.sync().unwrap();
    assert_eq!(
        spool
            .number("machine", 10)
            .unwrap()
            .iter()
            .map(|row| row.event.clone())
            .collect::<Vec<_>>(),
        [event("before"), event("after")]
    );
    spool.sync().unwrap();
    drop(spool);
    let (mut reopened, recovery) = Spool::open(directory.path(), options()).unwrap();
    assert!(recovery.damaged.is_empty());
    assert_eq!(reopened.read("machine", 1, 10).unwrap().len(), 2);
}

#[test]
fn a_short_kernel_write_is_rolled_back_before_later_records_and_survives_reopen() {
    const CHILD: &str = "ORCH_SPOOL_SHORT_WRITE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new("bash").args(["-c","trap '' XFSZ; ulimit -S -f 1; exec \"$@\"","short-write"])
            .arg(std::env::current_exe().unwrap()).args(["--exact","spool::io_failures::a_short_kernel_write_is_rolled_back_before_later_records_and_survives_reopen","--test-threads=1"])
            .env(CHILD,"1").status().unwrap();
        assert!(result.success());
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let mut spool = Spool::open(directory.path(), options()).unwrap().0;
    spool.append("machine", &event("before")).unwrap();
    spool.sync().unwrap();
    assert!(
        matches!(spool.append("machine",&event(&"x".repeat(64*1024))),Err(SpoolError::Io {source,..}) if source.raw_os_error()==Some(27))
    );
    assert!(
        std::process::Command::new("prlimit")
            .args([
                "--pid",
                &std::process::id().to_string(),
                "--fsize=unlimited:unlimited"
            ])
            .status()
            .unwrap()
            .success()
    );
    spool.append("machine", &event("after")).unwrap();
    spool.sync().unwrap();
    let sent = spool.number("machine", 10).unwrap();
    assert_eq!(
        sent.iter()
            .map(|row| (row.seq, row.event.clone()))
            .collect::<Vec<_>>(),
        [(1, event("before")), (2, event("after"))]
    );
    spool.sync().unwrap();
    drop(spool);
    let (mut reopened, recovery) = Spool::open(directory.path(), options()).unwrap();
    assert!(recovery.damaged.is_empty());
    assert_eq!(reopened.read("machine", 1, 10).unwrap(), sent);
}

#[test]
fn an_interrupted_diagnostic_promotion_finishes_before_a_later_authoritative_append() {
    let directory = tempfile::tempdir().unwrap();
    let options = SpoolOptions {
        max_bytes: 64 * 1024 * 1024,
        max_age: Duration::from_secs(86_400),
        segment_bytes: 512,
    };
    let at: Time = "2026-10-13T08:00:00.000Z".parse().unwrap();
    let mut spool = Spool::open(directory.path(), options).unwrap().0;
    spool
        .append(
            "machine",
            &SpoolEvent {
                at,
                kind: "machine.facts".into(),
                body: json!({}),
            },
        )
        .unwrap();
    spool.number("machine", 1).unwrap();
    for ordinal in 1..=3 {
        spool
            .append(
                "machine",
                &SpoolEvent {
                    at,
                    kind: "raw.frame".into(),
                    body: json!({"ordinal":ordinal}),
                },
            )
            .unwrap();
    }
    spool.sync().unwrap();
    let log = spool.streams.get_mut("machine").unwrap();
    let plan = Promotion {
        main_from: 2,
        diagnostic_from: 1,
        count: 3,
    };
    let mut pending = File::create(log.main.dir.join(PROMOTION_FILE)).unwrap();
    serde_json::to_writer(&mut pending, &plan).unwrap();
    pending.sync_all().unwrap();
    let first = log
        .diagnostics
        .read_entry(&log.diagnostics.entries[0])
        .unwrap();
    log.main.append_event(&first).unwrap();
    spool.sync().unwrap();
    let final_event = SpoolEvent {
        at,
        kind: "session.title".into(),
        body: json!({"title":"Final authoritative state"}),
    };
    spool.append("machine", &final_event).unwrap();
    spool.sync().unwrap();
    drop(spool);
    let (mut spool, recovery) = Spool::open(directory.path(), options).unwrap();
    assert!(recovery.damaged.is_empty());
    let events = spool.number("machine", 10).unwrap();
    assert_eq!(
        events
            .iter()
            .filter_map(|event| event.event.body.get("ordinal").and_then(Json::as_i64))
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(events.last().unwrap().event, final_event);
    assert_eq!(spool.diagnostics_waiting("machine"), 0);
}
