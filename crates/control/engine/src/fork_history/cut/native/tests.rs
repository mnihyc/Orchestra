//! Stream contracts: demand, frame boundaries, EOF identity and exact ordered semantic entries.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use super::*;
use orch_agent_contract::discovery::RecordedWords;
use orch_blob_contract::store::{BlobError, stream_of};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn said(turn: i64, text: &str) -> RecordedEntry {
    RecordedEntry::Said(RecordedWords {
        native_ref: format!("said:{turn}"),
        turn,
        text: text.to_owned(),
        left_out: 0,
        at: None,
    })
}

fn record(entries: &[RecordedEntry]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut bytes, entry).unwrap();
        bytes.push(b'\n');
    }
    bytes
}

fn identity(bytes: &[u8]) -> RecordRef {
    RecordRef {
        blob: Hash::of(bytes),
        size: bytes.len() as i64,
    }
}

fn evidence() -> NativeRecordEvidence {
    NativeRecordEvidence {
        turns: 2,
        entries: 2,
        report: orch_agent_contract::history::HistoryReport {
            fidelity: Fidelity::Structured,
            items_total: 2,
            by_kind: BTreeMap::from([(
                "user_message".to_owned(),
                KindCounts {
                    kept: 2,
                    ..Default::default()
                },
            )]),
            dropped: Vec::new(),
            warnings: Vec::new(),
        },
    }
}

#[tokio::test]
async fn the_native_line_supplier_does_not_pull_the_next_frame_to_yield_a_whole_entry() {
    let first = said(1, "first");
    let second = said(2, "second");
    let a = record(std::slice::from_ref(&first));
    let b = record(std::slice::from_ref(&second));
    let all = [a.as_slice(), b.as_slice()].concat();
    let pulls = Arc::new(AtomicUsize::new(0));
    let seen = pulls.clone();
    let body: ByteStream = Box::pin(futures::stream::iter([a, b]).map(move |bytes| {
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(Bytes::from(bytes))
    }));
    let mut entries = lines(body, identity(&all));
    assert_eq!(entries.next().await.unwrap().unwrap(), Some(first));
    assert_eq!(
        pulls.load(Ordering::SeqCst),
        1,
        "a record is not collected before comparison"
    );
    assert_eq!(entries.next().await.unwrap().unwrap(), Some(second));
    assert!(entries.next().await.is_none());
    assert_eq!(pulls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn native_record_identity_is_checked_after_the_last_entry_and_the_actual_eof() {
    let bytes = record(&[said(1, "a complete line")]);
    for mode in ["wrong_hash", "short_size", "large_size", "late_error"] {
        let mut claimed = identity(&bytes);
        let body: ByteStream = if mode == "late_error" {
            Box::pin(futures::stream::iter([
                Ok(Bytes::from(bytes.clone())),
                Err(BlobError::Io("actual upstream end failed".into())),
            ]))
        } else {
            stream_of(vec![Bytes::from(bytes.clone())])
        };
        match mode {
            "wrong_hash" => claimed.blob = Hash::of(b"another record"),
            "short_size" => claimed.size -= 1,
            "large_size" => claimed.size += 1,
            _ => {}
        }
        let result: Vec<_> = lines(body, claimed).collect().await;
        assert!(
            result.iter().any(Result::is_err),
            "{mode} cannot publish a witness from a plausible prefix"
        );
    }
}

#[tokio::test]
async fn native_entries_survive_split_utf8_and_an_unterminated_final_line() {
    let expected = [said(1, "边界"), said(2, "a second line")];
    let mut bytes = record(&expected);
    bytes.pop();
    let chunks = bytes
        .iter()
        .map(|byte| Bytes::copy_from_slice(std::slice::from_ref(byte)))
        .collect();
    let mut entries = lines(stream_of(chunks), identity(&bytes));
    for entry in expected {
        assert_eq!(entries.next().await.unwrap().unwrap(), Some(entry));
    }
    assert!(entries.next().await.is_none());
}

#[tokio::test]
async fn an_unknown_record_line_cannot_be_hidden_by_valid_counts_or_warning_words() {
    let bytes = b"{\"unrecognized\":true}\n";
    let mut entries = lines(stream_of(vec![Bytes::from_static(bytes)]), identity(bytes));
    assert_eq!(entries.next().await.unwrap().unwrap(), None);
    assert!(entries.next().await.is_none());
    let mut coverage = Coverage::default();
    coverage.observe(&said(1, "known")).unwrap();
    coverage.observe(&said(2, "known")).unwrap();
    coverage.invalid = true;
    let mut report = evidence();
    report.report.warnings.push("complete record".into());
    assert!(!coverage.complete(&report));
}

#[test]
fn suffixes_gaps_missing_attachments_and_inexact_reports_do_not_certify_a_prefix() {
    for entries in [
        vec![said(2, "suffix"), said(3, "suffix")],
        vec![said(1, "gap"), said(3, "gap")],
    ] {
        let mut coverage = Coverage::default();
        for entry in entries {
            coverage.observe(&entry).unwrap();
        }
        assert!(!coverage.complete(&evidence()));
    }
    let mut coverage = Coverage::default();
    coverage.observe(&said(1, "first")).unwrap();
    coverage.observe(&said(2, "second")).unwrap();
    assert!(coverage.complete(&evidence()));
    let mut report = evidence();
    report
        .report
        .by_kind
        .get_mut("user_message")
        .unwrap()
        .converted = 1;
    assert!(!coverage.complete(&report));
    let mut omitted = said(1, "an omitted file");
    if let RecordedEntry::Said(words) = &mut omitted {
        words.left_out = 1;
    }
    let mut coverage = Coverage::default();
    coverage.observe(&omitted).unwrap();
    coverage.observe(&said(2, "second")).unwrap();
    assert!(!coverage.complete(&evidence()));
}

#[test]
fn a_native_prefix_commits_all_ordered_words_and_metadata_without_object_key_order() {
    let first = said(1, "first words");
    let second = said(2, "second words");
    let expected = append(append(empty_digest(), &first).unwrap(), &second).unwrap();
    assert_ne!(
        expected,
        append(append(empty_digest(), &second).unwrap(), &first).unwrap()
    );
    assert_ne!(
        expected,
        append(
            append(empty_digest(), &first).unwrap(),
            &said(2, "changed words")
        )
        .unwrap()
    );
    let mut timed = second.clone();
    if let RecordedEntry::Said(words) = &mut timed {
        words.at = Some("2026-11-07T00:00:00Z".parse().unwrap());
    }
    assert_ne!(
        expected,
        append(append(empty_digest(), &first).unwrap(), &timed).unwrap()
    );
    assert_eq!(
        value_digest(&json!({"outer":{"b":2,"a":1}})),
        value_digest(&serde_json::from_str::<Json>("{\"outer\":{\"a\":1,\"b\":2}}").unwrap())
    );
    assert_ne!(value_digest(&json!([1, 2])), value_digest(&json!([2, 1])));
    let positive = json!({"value":0.0});
    let negative = json!({"value":-0.0});
    assert_eq!(
        positive, negative,
        "the commitment preserves existing typed JSON equality"
    );
    assert_eq!(value_digest(&positive), value_digest(&negative));
}
