//! A diagnostic queue stays consumed after a restart and unacknowledged frames keep their original numbers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_link::spool::{Spool, SpoolEvent, SpoolOptions};
use orch_types::Time;
use serde_json::json;

#[test]
fn a_server_cursor_after_spool_loss_resumes_delivery_without_changing_capture_numbers() {
    use orch_link::streams::StreamSender;
    use orch_link_contract::frames::Frame;
    use orch_link_contract::session::Role;
    use orch_link_contract::streams::{Ack, Stream};
    use std::collections::BTreeMap;

    fn events(frames: Vec<Frame>) -> Vec<(String, i64, Time, serde_json::Value)> {
        frames
            .into_iter()
            .flat_map(|frame| match frame {
                Frame::Ev(event) => vec![(event.stream, event.seq, event.at, event.body)],
                Frame::Evb(batch) => batch
                    .events
                    .into_iter()
                    .map(|event| (batch.stream.clone(), event.seq, event.at, event.body))
                    .collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    let directory = tempfile::tempdir().unwrap();
    let options = SpoolOptions {
        max_bytes: 64 * 1024,
        max_age: Duration::from_secs(86_400),
        segment_bytes: 512,
    };
    let stream = Stream::Session {
        thread_id: orch_types::new_id(),
        segment_id: orch_types::new_id(),
    };
    let name = stream.to_string();
    let at: Time = "2026-10-13T08:00:00.000Z".parse().unwrap();
    let now = tokio::time::Instant::now();
    let mut sender =
        StreamSender::open_for_machine(directory.path(), options, vec![Role::Agent], now)
            .unwrap()
            .0;
    sender.raw_after(&stream, 5000).unwrap();
    sender
        .publish(
            &stream,
            at,
            "session.title",
            json!({"title": "waiting"}),
            now,
        )
        .unwrap();
    sender
        .publish(
            &stream,
            at,
            "raw.frame",
            json!({"direction": "in", "payload": "captured before reconnect"}),
            now,
        )
        .unwrap();
    let mut cursors = BTreeMap::from([(name.clone(), 100), ("machine".to_owned(), 50)]);
    let resets = sender.on_ready(&cursors, 64 * 1024, now).unwrap();
    assert!(resets.iter().any(
        |frame| matches!(frame, Frame::Reset(reset) if reset.stream == name && reset.from == 101)
    ));
    assert!(resets.iter().any(
        |frame| matches!(frame, Frame::Reset(reset) if reset.stream == "machine" && reset.from == 51)
    ));
    // The machine stream was absent from disk and had not published anything at handshake time.
    sender
        .publish(&Stream::Machine, at, "machine.facts", json!({}), now)
        .unwrap();
    let first = events(sender.pump(64 * 1024, now).unwrap());
    assert_eq!(first.len(), 2);
    assert!(
        first
            .iter()
            .any(|(stream, q, _, _)| stream == &name && *q == 101)
    );
    assert!(
        first
            .iter()
            .any(|(stream, q, _, _)| stream == "machine" && *q == 51)
    );
    let raw = events(sender.pump(64 * 1024, now).unwrap());
    assert_eq!(raw.len(), 1);
    assert_eq!((&raw[0].0, raw[0].1, raw[0].2), (&name, 102, at));
    assert_eq!(raw[0].3["n"], 5001);
    assert_eq!(raw[0].3["payload"], "captured before reconnect");
    for (stream, seq) in [(name.clone(), 102), ("machine".to_owned(), 51)] {
        sender.on_ack(&Ack { stream, seq }, now).unwrap();
    }
    sender.sync().unwrap();
    drop(sender);
    cursors.insert(name.clone(), 102);
    cursors.insert("machine".to_owned(), 51);
    let mut sender =
        StreamSender::open_for_machine(directory.path(), options, vec![Role::Agent], now)
            .unwrap()
            .0;
    assert!(
        sender
            .on_ready(&cursors, 64 * 1024, now)
            .unwrap()
            .is_empty()
    );
    sender
        .publish(
            &stream,
            at,
            "raw.frame",
            json!({"direction": "in", "payload": "captured after restart"}),
            now,
        )
        .unwrap();
    let next = events(sender.pump(64 * 1024, now).unwrap());
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].1, 103);
    assert_eq!(next[0].3["n"], 5002);
}
use std::time::Duration;

#[test]
fn promoted_diagnostics_are_not_queued_again_and_a_partial_queue_resumes_in_order() {
    let directory = tempfile::tempdir().unwrap();
    // A small segment exercises both a consumed prefix left in the active file and deleted files.
    for segment_bytes in [512, 8 * 1024 * 1024] {
        let root = directory.path().join(segment_bytes.to_string());
        let options = SpoolOptions {
            max_bytes: 64 * 1024 * 1024,
            max_age: Duration::from_secs(7 * 86_400),
            segment_bytes,
        };
        let stream =
            "session:0199c1a0-0000-7000-8000-00000000000a:0199c1a0-0000-7000-8000-00000000000b";
        let at: Time = "2026-10-13T08:00:00.000Z".parse().unwrap();
        {
            let mut spool = Spool::open(&root, options).unwrap().0;
            for ordinal in 1..=12 {
                spool
                    .append(
                        stream,
                        &SpoolEvent {
                            at,
                            kind: "raw.frame".to_owned(),
                            body: json!({ "direction": "in", "payload": { "ordinal": ordinal } }),
                        },
                    )
                    .unwrap();
            }
            spool.promote_diagnostics(stream, 7).unwrap();
            assert_eq!(spool.number(stream, 7).unwrap().len(), 7);
            spool.acknowledge(stream, 5).unwrap();
            spool.sync().unwrap();
        }
        let (mut spool, recovery) = Spool::open(&root, options).unwrap();
        assert!(recovery.damaged.is_empty());
        assert_eq!(spool.diagnostics_waiting(stream), 5);
        assert_eq!(
            spool.oldest(stream),
            6,
            "the spool itself retains the server's acknowledgment"
        );
        spool.acknowledge(stream, 5).unwrap();
        let again = spool.read(stream, 6, 10).unwrap();
        assert_eq!(
            again
                .iter()
                .map(|event| (
                    event.seq,
                    event.event.body["payload"]["ordinal"].as_i64().unwrap()
                ))
                .collect::<Vec<_>>(),
            [(6, 6), (7, 7)]
        );
        spool.promote_diagnostics(stream, 10).unwrap();
        let tail = spool.number(stream, 10).unwrap();
        assert_eq!(
            tail.iter()
                .map(|event| (
                    event.seq,
                    event.event.body["payload"]["ordinal"].as_i64().unwrap()
                ))
                .collect::<Vec<_>>(),
            [(8, 8), (9, 9), (10, 10), (11, 11), (12, 12)]
        );
        spool.acknowledge(stream, 12).unwrap();
        spool.sync().unwrap();
        drop(spool);
        let mut spool = Spool::open(&root, options).unwrap().0;
        assert_eq!(spool.diagnostics_waiting(stream), 0);
        spool.acknowledge(stream, 12).unwrap();
        spool
            .append(
                stream,
                &SpoolEvent {
                    at,
                    kind: "raw.frame".to_owned(),
                    body: json!({ "direction": "in", "payload": "next" }),
                },
            )
            .unwrap();
        spool.promote_diagnostics(stream, 1).unwrap();
        assert_eq!(spool.number(stream, 1).unwrap()[0].seq, 13);
    }
}

#[test]
fn ordinary_delivery_numbers_do_not_advance_the_first_capture_number_after_a_restart() {
    let directory = tempfile::tempdir().unwrap();
    let options = SpoolOptions {
        max_bytes: 64 * 1024,
        max_age: Duration::from_secs(86_400),
        segment_bytes: 512,
    };
    let stream =
        "session:0199c1a0-0000-7000-8000-00000000000a:0199c1a0-0000-7000-8000-00000000000b";
    let at: Time = "2026-10-13T08:00:00.000Z".parse().unwrap();
    let mut spool = Spool::open(directory.path(), options).unwrap().0;
    for _ in 0..100 {
        spool
            .append(
                stream,
                &SpoolEvent {
                    at,
                    kind: "session.title".to_owned(),
                    body: json!({"title": "ordinary"}),
                },
            )
            .unwrap();
    }
    spool.number(stream, 100).unwrap();
    spool.acknowledge(stream, 100).unwrap();
    spool.sync().unwrap();
    drop(spool);
    let mut spool = Spool::open(directory.path(), options).unwrap().0;
    spool
        .append(
            stream,
            &SpoolEvent {
                at,
                kind: "raw.frame".to_owned(),
                body: json!({"direction": "in", "payload": "first capture"}),
            },
        )
        .unwrap();
    spool.promote_diagnostics(stream, 1).unwrap();
    let sent = spool.number(stream, 1).unwrap();
    assert_eq!(sent[0].seq, 101);
    assert_eq!(sent[0].event.body["n"], 1);
}
#[test]
fn capture_numbers_precede_delivery_numbers_and_a_server_floor_never_renumbers_queued_frames() {
    let directory = tempfile::tempdir().unwrap();
    let options = SpoolOptions {
        max_bytes: 64 * 1024,
        max_age: Duration::from_secs(86_400),
        segment_bytes: 8 * 1024,
    };
    let stream =
        "session:0199c1a0-0000-7000-8000-00000000000a:0199c1a0-0000-7000-8000-00000000000b";
    let at: Time = "2026-10-13T08:00:00.000Z".parse().unwrap();
    let raw = SpoolEvent {
        at,
        kind: "raw.frame".to_owned(),
        body: json!({"direction": "in", "payload": "same"}),
    };
    let mut spool = Spool::open(directory.path(), options).unwrap().0;
    spool
        .append(
            stream,
            &SpoolEvent {
                at,
                kind: "session.title".to_owned(),
                body: json!({"title": "Hello"}),
            },
        )
        .unwrap();
    spool.append(stream, &raw).unwrap();
    spool.append(stream, &raw).unwrap();
    spool.raw_after(stream, 1000).unwrap();
    spool.append(stream, &raw).unwrap();
    spool.sync().unwrap();
    drop(spool);
    let mut spool = Spool::open(directory.path(), options).unwrap().0;
    spool.raw_after(stream, 20).unwrap();
    spool.append(stream, &raw).unwrap();
    spool.promote_diagnostics(stream, 10).unwrap();
    let numbered = spool.number(stream, 10).unwrap();
    assert_eq!(numbered[0].event.kind, "session.title");
    assert_eq!(
        numbered
            .iter()
            .skip(1)
            .map(|e| (e.seq, e.event.body["n"].as_i64().unwrap()))
            .collect::<Vec<_>>(),
        [(2, 1), (3, 2), (4, 1001), (5, 1002)]
    );
    assert!(
        numbered.iter().all(|e| e.event.at == at),
        "promotion preserves capture times"
    );
    spool.acknowledge(stream, 5).unwrap();
    spool.sync().unwrap();
    drop(spool);
    let mut spool = Spool::open(directory.path(), options).unwrap().0;
    assert_eq!(spool.oldest(stream), 6);
    // Expired queued frames must stay consumed, while their allocated numbers remain used.
    spool.append(stream, &raw).unwrap();
    spool
        .expire(Time::new(at.get() + chrono::Duration::days(2)))
        .unwrap();
    spool.sync().unwrap();
    drop(spool);
    let mut spool = Spool::open(directory.path(), options).unwrap().0;
    assert_eq!(spool.diagnostics_waiting(stream), 0);
    spool
        .append(
            stream,
            &SpoolEvent {
                at: Time::new(at.get() + chrono::Duration::days(2)),
                ..raw
            },
        )
        .unwrap();
    spool.promote_diagnostics(stream, 1).unwrap();
    assert_eq!(spool.number(stream, 1).unwrap()[0].event.body["n"], 1004);
}

#[test]
fn dropping_all_diagnostic_segment_files_does_not_reuse_a_capture_number() {
    let directory = tempfile::tempdir().unwrap();
    // One large frame fits; two force the diagnostic drop path before the second is appended.
    let options = SpoolOptions {
        max_bytes: 700,
        max_age: Duration::from_secs(86_400),
        segment_bytes: 512,
    };
    let stream =
        "session:0199c1a0-0000-7000-8000-00000000000a:0199c1a0-0000-7000-8000-00000000000b";
    let at: Time = "2026-10-13T08:00:00.000Z".parse().unwrap();
    let raw = SpoolEvent {
        at,
        kind: "raw.frame".to_owned(),
        body: json!({"direction": "in", "payload": "x".repeat(400)}),
    };
    let mut spool = Spool::open(directory.path(), options).unwrap().0;
    spool.append(stream, &raw).unwrap();
    let relief = spool.append(stream, &raw).unwrap();
    assert!(
        relief
            .iter()
            .any(|step| matches!(step, orch_link::spool::Relief::Diagnostics { events: 1 }))
    );
    spool.sync().unwrap();
    drop(spool);
    let mut spool = Spool::open(directory.path(), options).unwrap().0;
    spool
        .append(
            stream,
            &SpoolEvent {
                at,
                kind: "raw.frame".to_owned(),
                body: json!({"direction": "in", "payload": "new"}),
            },
        )
        .unwrap();
    spool.promote_diagnostics(stream, 2).unwrap();
    let numbered = spool.number(stream, 2).unwrap();
    assert_eq!(
        numbered
            .iter()
            .map(|e| e.event.body["n"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [2, 3]
    );
}
