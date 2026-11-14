//! What a machine keeps on disk until the system has taken it in: numbering, restarts, and a spool that is full.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_link::spool::{Relief, Spool, SpoolEvent, SpoolOptions};
use orch_types::Time;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

const STREAM: &str = "env:0199c1a0-0000-7000-8000-00000000000a";
const OTHER: &str = "node";

fn options() -> SpoolOptions {
    SpoolOptions {
        max_bytes: 64 * 1024 * 1024,
        max_age: Duration::from_secs(7 * 86_400),
        segment_bytes: 8 * 1024 * 1024,
    }
}

fn at(second: i64) -> Time {
    Time::new(chrono::DateTime::from_timestamp(1_791_000_000 + second, 0).unwrap())
}

fn event(kind: &str, n: i64, body: serde_json::Value) -> SpoolEvent {
    SpoolEvent {
        at: at(n),
        kind: kind.to_owned(),
        body,
    }
}

fn plain(n: i64) -> SpoolEvent {
    event("env.status", n, json!({ "n": n, "status": "ready" }))
}

fn open(dir: &Path, options: SpoolOptions) -> Spool {
    Spool::open(dir, options).unwrap().0
}

#[test]
fn events_take_their_numbers_when_they_are_sent_in_the_order_they_were_spooled() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(dir.path(), options());
    for n in 0..5 {
        spool.append(STREAM, &plain(n)).unwrap();
    }
    assert_eq!(spool.unsent(STREAM), 5);
    assert_eq!(spool.sent_through(STREAM), 0);
    let first = spool.number(STREAM, 3).unwrap();
    assert_eq!(first.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(first[1].event.body["n"], 1);
    let rest = spool.number(STREAM, 10).unwrap();
    assert_eq!(rest.iter().map(|e| e.seq).collect::<Vec<_>>(), [4, 5]);
    assert_eq!(spool.unsent(STREAM), 0);
}

#[test]
fn a_number_once_given_stays_with_its_event_across_a_stop() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut spool = open(dir.path(), options());
        for n in 0..5 {
            spool.append(STREAM, &plain(n)).unwrap();
        }
        spool.number(STREAM, 3).unwrap();
        spool.sync().unwrap();
    }
    let (mut spool, recovery) = Spool::open(dir.path(), options()).unwrap();
    assert_eq!(recovery.streams, [STREAM]);
    assert!(recovery.damaged.is_empty());
    assert_eq!(spool.oldest(STREAM), 1);
    assert_eq!(spool.sent_through(STREAM), 3);
    assert_eq!(spool.unsent(STREAM), 2);
    let again = spool.read(STREAM, 2, 10).unwrap();
    assert_eq!(
        again
            .iter()
            .map(|e| (e.seq, e.event.body["n"].as_i64().unwrap()))
            .collect::<Vec<_>>(),
        [(2, 1), (3, 2)]
    );
    assert_eq!(spool.number(STREAM, 10).unwrap()[0].seq, 4);
}

#[test]
fn an_acknowledgement_lets_go_and_the_numbering_goes_on_after_everything_was_acknowledged() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut spool = open(
            dir.path(),
            SpoolOptions {
                segment_bytes: 300,
                ..options()
            },
        );
        for n in 0..12 {
            spool.append(STREAM, &plain(n)).unwrap();
        }
        spool.number(STREAM, 12).unwrap();
        spool.acknowledge(STREAM, 12).unwrap();
        assert_eq!(spool.oldest(STREAM), 13);
        assert_eq!(spool.unsent(STREAM), 0);
        spool.sync().unwrap();
    }
    let mut spool = open(
        dir.path(),
        SpoolOptions {
            segment_bytes: 300,
            ..options()
        },
    );
    spool.append(STREAM, &plain(99)).unwrap();
    let next = spool.number(STREAM, 1).unwrap();
    assert_eq!(
        next[0].seq, 13,
        "the system has everything through 12, and the machine knows it"
    );
}

#[test]
fn diagnostics_wait_apart_and_are_numbered_only_when_they_are_let_into_the_stream() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(dir.path(), options());
    spool
        .append(
            STREAM,
            &event(
                "raw.frame",
                0,
                json!({ "direction": "in", "payload": "{}" }),
            ),
        )
        .unwrap();
    spool
        .append(
            STREAM,
            &event(
                "raw.frame",
                1,
                json!({ "direction": "out", "payload": "{}" }),
            ),
        )
        .unwrap();
    spool.append(STREAM, &plain(2)).unwrap();
    assert_eq!(
        spool.unsent(STREAM),
        1,
        "the event that matters is first in line"
    );
    assert_eq!(spool.diagnostics_waiting(STREAM), 2);
    assert_eq!(
        spool.number(STREAM, 10).unwrap()[0].event.kind,
        "env.status"
    );
    assert_eq!(spool.promote_diagnostics(STREAM, 1).unwrap(), 1);
    let promoted = spool.number(STREAM, 10).unwrap();
    assert_eq!(
        (
            promoted[0].seq,
            promoted[0].event.body["direction"].as_str()
        ),
        (2, Some("in"))
    );
    assert_eq!(spool.diagnostics_waiting(STREAM), 1);
}

fn delta(item: &str, n: i64) -> SpoolEvent {
    event(
        "item.delta",
        n,
        json!({ "native_ref": item, "field": "text", "offset": n, "len": 1, "data": "x" }),
    )
}

fn completed(item: &str, n: i64) -> SpoolEvent {
    event(
        "item.completed",
        n,
        json!({ "native_ref": item, "status": "completed", "payload": { "text": "xxxx" } }),
    )
}

#[test]
fn diagnostic_pressure_sheds_only_waiting_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(
        dir.path(),
        SpoolOptions {
            max_bytes: 2_600,
            segment_bytes: 700,
            ..options()
        },
    );
    let mut relief = Vec::new();
    for n in 0..30 {
        relief.extend(
            spool
                .append(
                    STREAM,
                    &event(
                        "raw.frame",
                        n,
                        json!({ "direction": "in", "payload": "p".repeat(300) }),
                    ),
                )
                .unwrap(),
        );
    }
    for n in 0..8 {
        relief.extend(spool.append(STREAM, &plain(n)).unwrap());
    }
    assert!(
        matches!(relief.first(), Some(Relief::Diagnostics { events }) if *events > 0),
        "{relief:?}"
    );
    assert!(
        relief
            .iter()
            .all(|r| matches!(r, Relief::Diagnostics { .. })),
        "nothing else had to go: {relief:?}"
    );
    assert_eq!(spool.unsent(STREAM), 8, "no event that matters was lost");
}

#[test]
fn completed_items_preserve_the_deltas_that_explain_their_progress() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(
        dir.path(),
        SpoolOptions {
            max_bytes: 3_000,
            segment_bytes: 800,
            ..options()
        },
    );
    for n in 0..10 {
        assert!(
            spool
                .append(STREAM, &delta("item-1", n))
                .unwrap()
                .is_empty()
        );
    }
    spool.append(STREAM, &completed("item-1", 10)).unwrap();
    for n in 11..30 {
        assert!(
            spool
                .append(STREAM, &delta("item-2", n))
                .unwrap()
                .is_empty()
        );
    }
    let all = spool.number(STREAM, 1_000).unwrap();
    assert_eq!(all.len(), 30);
    assert_eq!(
        all.iter()
            .filter(|e| e.event.kind == "item.delta" && e.event.body["native_ref"] == "item-1")
            .count(),
        10
    );
    assert!(all.iter().any(|e| e.event.kind == "item.completed"));
    assert_eq!(
        all.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=30).collect::<Vec<_>>()
    );
}

#[test]
fn deltas_already_sent_are_never_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(
        dir.path(),
        SpoolOptions {
            max_bytes: 2_500,
            segment_bytes: 700,
            ..options()
        },
    );
    for n in 0..6 {
        spool.append(STREAM, &delta("item-1", n)).unwrap();
    }
    assert_eq!(spool.number(STREAM, 6).unwrap().len(), 6);
    spool.append(STREAM, &completed("item-1", 6)).unwrap();
    for n in 7..20 {
        spool.append(STREAM, &plain(n)).unwrap();
    }
    let held = spool.read(STREAM, spool.oldest(STREAM), 1_000).unwrap();
    let sent_deltas = held.iter().filter(|e| e.event.kind == "item.delta").count();
    assert!(
        sent_deltas == 6 && spool.oldest(STREAM) == 1,
        "numbered deltas stay until acknowledged"
    );
}

#[test]
fn a_busy_stream_keeps_its_history_after_the_diagnostic_allowance_is_full() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(
        dir.path(),
        SpoolOptions {
            max_bytes: 3_000,
            segment_bytes: 600,
            ..options()
        },
    );
    for n in 0..4 {
        spool.append(OTHER, &plain(n)).unwrap();
    }
    for n in 0..60 {
        assert!(spool.append(STREAM, &plain(n)).unwrap().is_empty());
    }
    assert_eq!(spool.unsent(OTHER), 4);
    assert_eq!(spool.unsent(STREAM), 60);
    assert!(spool.bytes() > 3_000);
    assert_eq!(spool.oldest(STREAM), 1);
    assert_eq!(spool.number(STREAM, 1).unwrap()[0].seq, 1);
}

#[test]
fn numbered_events_keep_their_numbers_while_unacknowledged_work_accumulates() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(
        dir.path(),
        SpoolOptions {
            max_bytes: 2_000,
            segment_bytes: 500,
            ..options()
        },
    );
    for n in 0..8 {
        spool.append(STREAM, &plain(n)).unwrap();
    }
    spool.number(STREAM, 8).unwrap();
    for n in 8..40 {
        assert!(spool.append(STREAM, &plain(n)).unwrap().is_empty());
    }
    assert_eq!(spool.oldest(STREAM), 1);
    assert_eq!(spool.sent_through(STREAM), 8);
    assert_eq!(spool.unsent(STREAM), 32);
}

#[test]
fn authoritative_events_stay_after_the_diagnostic_age_passes() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(
        dir.path(),
        SpoolOptions {
            max_age: Duration::from_secs(100),
            ..options()
        },
    );
    for n in 0..10 {
        spool.append(STREAM, &plain(n * 20)).unwrap();
    }
    assert!(spool.expire(at(190)).unwrap().is_empty());
    assert_eq!(spool.unsent(STREAM), 10);
}

#[test]
fn a_record_that_was_torn_by_a_crash_is_cut_off_and_what_came_before_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut spool = open(dir.path(), options());
        for n in 0..5 {
            spool.append(STREAM, &plain(n)).unwrap();
        }
        spool.number(STREAM, 2).unwrap();
        spool.append(STREAM, &plain(5)).unwrap();
        spool.append(STREAM, &plain(6)).unwrap();
        spool.sync().unwrap();
    }
    let stream_dir = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let segment = std::fs::read_dir(&stream_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let length = std::fs::metadata(&segment).unwrap().len();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&segment)
        .unwrap();
    file.set_len(length - 7).unwrap();
    let (mut spool, recovery) = Spool::open(dir.path(), options()).unwrap();
    assert_eq!(recovery.damaged, [STREAM]);
    assert_eq!(spool.sent_through(STREAM), 2);
    assert_eq!(
        spool.unsent(STREAM),
        4,
        "the last event was torn, the six before it are whole"
    );
    assert_eq!(spool.read(STREAM, 1, 10).unwrap().len(), 2);
}

#[test]
fn a_rewrite_that_was_cut_off_is_finished_or_undone_so_that_one_whole_copy_remains() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut spool = open(dir.path(), options());
        for n in 0..3 {
            spool.append(STREAM, &plain(n)).unwrap();
        }
        spool.number(STREAM, 3).unwrap();
        spool.sync().unwrap();
    }
    let main = dir.path().join(STREAM.replace(':', "_"));
    // Cut off after the old one was moved aside and before the new one was moved in.
    let staged = dir
        .path()
        .join(format!("{}.rewriting", STREAM.replace(':', "_")));
    let replaced = dir
        .path()
        .join(format!("{}.replaced", STREAM.replace(':', "_")));
    std::fs::rename(&main, &replaced).unwrap();
    std::fs::create_dir_all(&staged).unwrap();
    let (spool, _) = Spool::open(dir.path(), options()).unwrap();
    assert_eq!(
        spool.sent_through(STREAM),
        3,
        "the old copy came back, because the new one was not whole"
    );
    assert!(!staged.exists() && !replaced.exists());
}

#[test]
fn unacknowledged_history_survives_pressure_age_and_restart_with_its_original_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let limits = SpoolOptions {
        max_bytes: 1_000,
        max_age: Duration::from_secs(1),
        segment_bytes: 300,
    };
    {
        let mut spool = open(dir.path(), limits);
        for n in 0..8 {
            spool.append(STREAM, &plain(n)).unwrap();
        }
        spool.number(STREAM, 8).unwrap();
        for n in 8..40 {
            assert!(spool.append(STREAM, &plain(n)).unwrap().is_empty());
        }
        assert!(spool.expire(at(3_600)).unwrap().is_empty());
        assert_eq!(spool.oldest(STREAM), 1);
        assert_eq!(spool.sent_through(STREAM), 8);
        assert_eq!(spool.unsent(STREAM), 32);
        spool.sync().unwrap();
    }
    let mut spool = open(dir.path(), limits);
    assert_eq!(spool.oldest(STREAM), 1);
    assert_eq!(spool.sent_through(STREAM), 8);
    spool.number(STREAM, 100).unwrap();
    let all = spool.read(STREAM, 1, 100).unwrap();
    assert_eq!(all.len(), 40);
    for (index, event) in all.iter().enumerate() {
        assert_eq!(event.seq, index as i64 + 1);
        assert_eq!(event.event.body["n"], index as i64);
    }
}

#[test]
fn an_authoritative_event_can_outgrow_the_diagnostic_spool_allowance() {
    let dir = tempfile::tempdir().unwrap();
    let limits = SpoolOptions {
        max_bytes: 100,
        ..options()
    };
    let mut spool = open(dir.path(), limits);
    let expected = event(
        "item.updated",
        0,
        json!({"native_ref":"fixture-item", "payload":{"text":"x".repeat(1_000)}}),
    );
    assert!(spool.append(STREAM, &expected).unwrap().is_empty());
    spool.sync().unwrap();
    drop(spool);
    let mut spool = open(dir.path(), limits);
    assert_eq!(spool.number(STREAM, 1).unwrap()[0].event, expected);
}

#[test]
fn a_retained_authoritative_record_over_sixty_four_megabytes_is_not_mistaken_for_damage() {
    let dir = tempfile::tempdir().unwrap();
    let expected = event(
        "item.updated",
        0,
        json!({"native_ref":"fixture-item", "payload":{"text":"x".repeat(64 * 1024 * 1024 + 1)}}),
    );
    let mut spool = open(dir.path(), options());
    spool.append(STREAM, &expected).unwrap();
    spool.sync().unwrap();
    drop(spool);
    let (mut spool, recovery) = Spool::open(dir.path(), options()).unwrap();
    assert!(recovery.damaged.is_empty());
    assert_eq!(spool.number(STREAM, 1).unwrap()[0].event, expected);
}

#[test]
fn diagnostic_shedding_keeps_its_capture_gap_without_discarding_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(
        dir.path(),
        SpoolOptions {
            max_bytes: 1_000,
            segment_bytes: 300,
            ..options()
        },
    );
    for n in 0..40 {
        spool.append(STREAM, &plain(n)).unwrap();
    }
    let discarded = event(
        "raw.frame",
        0,
        json!({"direction":"in", "payload":"x".repeat(2_000)}),
    );
    assert_eq!(
        spool.append(STREAM, &discarded).unwrap(),
        [Relief::Diagnostics { events: 1 }]
    );
    assert_eq!(spool.oldest(STREAM), 1);
    assert_eq!(spool.unsent(STREAM), 40);
    spool.number(STREAM, 40).unwrap();
    spool.acknowledge(STREAM, 40).unwrap();
    spool
        .append(
            STREAM,
            &event("raw.frame", 1, json!({"direction":"in", "payload":"small"})),
        )
        .unwrap();
    spool.promote_diagnostics(STREAM, 1).unwrap();
    let next = spool.number(STREAM, 1).unwrap();
    assert_eq!(next[0].seq, 41);
    assert_eq!(
        next[0].event.body["n"], 2,
        "the discarded diagnostic's capture number is not reused"
    );
}
