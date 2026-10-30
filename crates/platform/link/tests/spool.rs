//! What a machine keeps on disk until the system has taken it in: numbering, restarts, and a spool that is full.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_link::spool::{Relief, Spool, SpoolEvent, SpoolOptions};
use orch_link_contract::streams::ResetReason;
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
fn a_full_spool_gives_up_diagnostics_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(
        dir.path(),
        SpoolOptions {
            max_bytes: 2_600,
            segment_bytes: 700,
            ..options()
        },
    );
    for n in 0..6 {
        spool
            .append(
                STREAM,
                &event(
                    "raw.frame",
                    n,
                    json!({ "direction": "in", "payload": "p".repeat(300) }),
                ),
            )
            .unwrap();
    }
    let mut relief = Vec::new();
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
fn then_the_deltas_of_an_item_that_is_completed_and_not_yet_sent() {
    let dir = tempfile::tempdir().unwrap();
    let mut spool = open(
        dir.path(),
        SpoolOptions {
            max_bytes: 3_000,
            segment_bytes: 800,
            ..options()
        },
    );
    let mut relief = Vec::new();
    for n in 0..10 {
        relief.extend(spool.append(STREAM, &delta("item-1", n)).unwrap());
    }
    relief.extend(spool.append(STREAM, &completed("item-1", 10)).unwrap());
    for n in 11..30 {
        relief.extend(spool.append(STREAM, &delta("item-2", n)).unwrap());
    }
    let superseded: usize = relief
        .iter()
        .filter_map(|r| {
            if let Relief::SupersededDeltas { events } = r {
                Some(*events)
            } else {
                None
            }
        })
        .sum();
    assert!(superseded > 0, "{relief:?}");
    let all = spool.number(STREAM, 1_000).unwrap();
    assert!(
        all.iter().any(|e| e.event.kind == "item.completed"),
        "the completed item holds the whole text"
    );
    let item1_deltas = all
        .iter()
        .filter(|e| e.event.kind == "item.delta" && e.event.body["native_ref"] == "item-1")
        .count();
    assert!(
        item1_deltas < 10,
        "deltas of the completed item were left out: {item1_deltas}"
    );
    let item2_deltas = all
        .iter()
        .filter(|e| e.event.body["native_ref"] == "item-2")
        .count();
    assert!(
        item2_deltas > 0,
        "the deltas of an item that is still growing stay"
    );
    let numbers: Vec<_> = all.iter().map(|e| e.seq).collect();
    assert_eq!(
        numbers,
        (1..=numbers.len() as i64).collect::<Vec<_>>(),
        "no hole in the numbering"
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
        sent_deltas == 6 || spool.oldest(STREAM) > 1,
        "sent deltas are numbered and stay, or the oldest went whole"
    );
}

#[test]
fn last_the_oldest_events_of_the_fullest_stream_go_and_the_loss_is_reported() {
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
    let mut relief = Vec::new();
    for n in 0..60 {
        relief.extend(spool.append(STREAM, &plain(n)).unwrap());
    }
    let gone: Vec<&Relief> = relief
        .iter()
        .filter(|r| matches!(r, Relief::OldestEvents { .. }))
        .collect();
    assert!(!gone.is_empty(), "{relief:?}");
    for r in &gone {
        let Relief::OldestEvents {
            stream,
            reason,
            events,
            ..
        } = r
        else {
            unreachable!()
        };
        assert_eq!(
            stream, STREAM,
            "the fullest stream gives, not the quiet one"
        );
        assert_eq!(*reason, ResetReason::SpoolOverflow);
        assert!(*events > 0);
    }
    assert_eq!(spool.unsent(OTHER), 4);
    assert!(spool.bytes() <= 3_000);
    assert!(
        spool.unsent(STREAM) < 60,
        "the oldest events of the fullest stream went"
    );
    assert_eq!(
        spool.oldest(STREAM),
        1,
        "events that never had a number leave no gap in the numbering when they go"
    );
    assert_eq!(spool.number(STREAM, 1).unwrap()[0].seq, 1);
}

#[test]
fn dropping_events_that_were_already_numbered_says_how_far_the_numbers_went() {
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
    let mut relief = Vec::new();
    for n in 8..40 {
        relief.extend(spool.append(STREAM, &plain(n)).unwrap());
    }
    let first = relief
        .iter()
        .find_map(|r| {
            if let Relief::OldestEvents {
                numbered_through, ..
            } = r
            {
                Some(*numbered_through)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        first.is_some_and(|through| (1..=8).contains(&through)),
        "{relief:?}"
    );
}

#[test]
fn events_older_than_the_age_limit_go_from_the_front() {
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
    let relief = spool.expire(at(190)).unwrap();
    // Events 0..=4 are 100 or more seconds old at 190, events from 5 on are younger.
    assert!(
        matches!(
            &relief[..],
            [Relief::OldestEvents {
                events: 5,
                numbered_through: None,
                ..
            }]
        ),
        "{relief:?}"
    );
    assert_eq!(spool.unsent(STREAM), 5);
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
