//! Values stay private: retain only the first unexpected frame's class and changed schema paths.

use crate::realtime_support::Heard;
use serde_json::{Value, json};

// At most the existing request diagnostic's 4096 characters of schema-path evidence.
const FIELDS: usize = 32;
const FIELD_CHARS: usize = 128;

pub(super) fn phase(name: &'static str) {
    eprintln!("entry_request_phase {name}");
}

fn record(paths: &mut Vec<String>, path: &str) {
    if paths.len() < FIELDS {
        paths.push(path.chars().take(FIELD_CHARS).collect());
    }
}

fn changed(before: &Value, after: &Value, path: &str, paths: &mut Vec<String>) {
    if paths.len() >= FIELDS || before == after {
        return;
    }
    match (before, after) {
        (Value::Object(old), Value::Object(new)) => {
            for (key, value) in old {
                if paths.len() >= FIELDS {
                    break;
                }
                let at = format!("{path}/{key}");
                match new.get(key) {
                    Some(next) => changed(value, next, &at, paths),
                    None => record(paths, &at),
                }
            }
            // Unknown new keys may themselves be private names. Report shape, never their spelling.
            if new.keys().any(|key| !old.contains_key(key)) {
                record(paths, &format!("{path}/<added-fields>"));
            }
        }
        (Value::Array(old), Value::Array(new)) => {
            if old.len() != new.len() {
                record(paths, &format!("{path}/<length>"));
            }
            for (index, (value, next)) in old.iter().zip(new).enumerate() {
                if paths.len() >= FIELDS {
                    break;
                }
                changed(value, next, &format!("{path}/{index}"), paths);
            }
        }
        _ => record(paths, path),
    }
}

pub(super) fn unexpected(heard: Option<&Heard>, feed: &str, before: &Value) -> Value {
    match heard {
        None => json!({"kind":"none"}),
        Some(Heard::Closed(code)) => json!({"kind":"closed","code":code}),
        Some(Heard::Frame(frame)) => {
            let op = frame["op"]
                .as_str()
                .filter(|op| {
                    matches!(
                        *op,
                        "event"
                            | "sync"
                            | "delta"
                            | "presence"
                            | "subscribed"
                            | "resync"
                            | "error"
                            | "closed"
                            | "pong"
                            | "ready"
                    )
                })
                .unwrap_or("other");
            let entry = frame.get("data").and_then(|data| data.get("entry"));
            let mut paths = Vec::new();
            if let Some(entry) = entry {
                changed(before, entry, "", &mut paths);
            }
            let members: Vec<_> = [
                "entry",
                "directory",
                "what",
                "offline",
                "error",
                "reset",
                "items",
                "thread",
                "seq",
            ]
            .into_iter()
            .filter(|key| frame.get("data").and_then(|data| data.get(*key)).is_some())
            .collect();
            json!({"kind":"frame","op":op,"same_feed":frame["feed"].as_str()==Some(feed),
                "data_members":members,"entry_present":entry.is_some(),"same_entry":entry==Some(before),"changed_schema_paths":paths})
        }
    }
}

#[test]
fn unexpected_frame_evidence_keeps_schema_changes_but_neither_values_nor_added_private_keys() {
    let before = json!({"label":"value-before","rows":[1]});
    let heard = Heard::Frame(json!({"op":"event","feed":"expected","data":{"entry":{
        "label":"value-after","rows":[2,3],"not-a-schema-key":"not-a-public-value"}}}));
    let summary = unexpected(Some(&heard), "expected", &before);
    assert_eq!(summary["op"], "event");
    assert_eq!(summary["same_feed"], true);
    let text = summary.to_string();
    assert!(
        text.contains("/label")
            && text.contains("/rows/<length>")
            && text.contains("/<added-fields>")
    );
    for value in [
        "value-before",
        "value-after",
        "not-a-schema-key",
        "not-a-public-value",
    ] {
        assert!(!text.contains(value));
    }
    let before = json!({"rows":vec![0;FIELDS*2]});
    let heard = Heard::Frame(json!({"op":"event","data":{"entry":{"rows":vec![1;FIELDS*2]}}}));
    let summary = unexpected(Some(&heard), "expected", &before);
    assert_eq!(
        summary["changed_schema_paths"].as_array().map(Vec::len),
        Some(FIELDS)
    );
}
