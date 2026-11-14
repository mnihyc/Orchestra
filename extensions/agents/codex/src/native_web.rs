//! Standalone native web actions are proved by their exact retained function result, not end events.
use crate::events::OutputStore;
use crate::viewed_images::Key;
use orch_agent_contract::events::{
    ErrorPayload, ItemCompleted, ItemContent, ItemStarted, ItemStatus, OutputRef, SessionEvent,
    ToolCallPayload, ToolPayload,
};
use orch_types::{Hash, Id, Json};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Read;
pub(crate) type Checked = (Proof, Option<ItemCompleted>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Success,
    Failed,
    Unproven(&'static str),
}
#[derive(Debug, Clone)]
pub(crate) struct Proof {
    pub outcome: Outcome,
    pub output: Option<OutputRef>,
    pub model: Option<String>,
}
impl Proof {
    pub(crate) fn unavailable(reason: &'static str) -> Self {
        Self {
            outcome: Outcome::Unproven(reason),
            output: None,
            model: None,
        }
    }
    pub(crate) fn of(
        payload: Result<crate::native_history::ResultItem, &'static str>,
        outputs: &OutputStore,
    ) -> Self {
        Self::of_reported(payload, None, outputs)
    }
    pub(crate) fn of_reported(
        payload: Result<crate::native_history::ResultItem, &'static str>,
        reported: Option<&Json>,
        outputs: &OutputStore,
    ) -> Self {
        let crate::native_history::ResultItem { payload, model } = match payload {
            Ok(payload) => payload,
            Err(reason) => return Self::unavailable(reason),
        };
        let outcome = if payload["output"].as_array().is_some_and(|parts| {
            parts
                .iter()
                .all(|part| part["type"] == "input_text" && part["text"].is_string())
        }) {
            Outcome::Success
        } else if payload["output"].is_string() {
            Outcome::Failed
        } else {
            Outcome::Unproven("native_web_output_shape_unknown")
        };
        let raw = match reported {
            Some(reported) => json!({"native_result":payload,"reported_result":reported}),
            None => payload,
        };
        Self {
            outcome,
            output: crate::viewed_images::retain_original(&raw, outputs),
            model,
        }
    }
}

pub(crate) fn reported_result(item: &Json) -> Option<&Json> {
    item.get("results")
        .filter(|value| !value.is_null())
        .or_else(|| item.pointer("/action/sources"))
        .filter(|value| !value.is_null())
}
pub(crate) fn page_reported(native: &str, turns: &[Json]) -> BTreeMap<Key, Json> {
    let mut reported = BTreeMap::new();
    for turn in turns {
        let Some(native_turn) = turn["id"].as_str() else {
            continue;
        };
        for item in turn["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["type"] == "webSearch")
        {
            if let Some(call) = item["id"].as_str()
                && let Some(value) = reported_result(item)
            {
                reported.insert(
                    Key {
                        native_thread: native.into(),
                        native_turn: native_turn.into(),
                        call: call.into(),
                    },
                    value.clone(),
                );
            }
        }
    }
    reported
}

fn full_output(outputs: &OutputStore, reference: &OutputRef) -> Option<Json> {
    let raw = if let Some(hash) = reference.blob {
        let file = std::fs::File::open(outputs.directory().join(hash.to_string())).ok()?;
        let limit = u64::try_from(crate::protocol::MAX_FRAME_BYTES.saturating_mul(2)).ok()?;
        if file.metadata().ok()?.len() > limit {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes).ok()?;
        if u64::try_from(bytes.len()).ok()? > limit || Hash::of(&bytes) != hash {
            return None;
        }
        String::from_utf8(bytes).ok()?
    } else {
        if reference.truncated {
            return None;
        }
        reference.inline.clone()
    };
    Some(serde_json::from_str(&raw).unwrap_or(Json::String(raw)))
}

/// Resolve both sources outside the session mutex, preserving all public result data with the native proof.
pub(crate) fn checked_results(
    source: &mut crate::viewed_images::Sources,
    keys: &[Key],
    snapshots: &BTreeMap<Key, Option<Hash>>,
    outputs: &OutputStore,
) -> BTreeMap<Key, Checked> {
    let mut completions = BTreeMap::new();
    let mut reported = BTreeMap::new();
    let mut valid = Vec::new();
    let mut refused = BTreeMap::new();
    for key in keys {
        let mut completion = read_completion(outputs, snapshots.get(key).copied().flatten());
        let call = completion
            .as_ref()
            .filter(|item| item.native_ref == key.call)
            .and_then(|item| serde_json::from_value::<ToolCallPayload>(item.payload.clone()).ok());
        if let Some(mut call) = call {
            let full = call
                .raw
                .output
                .as_ref()
                .map(|reference| full_output(outputs, reference));
            if let Some(None) = full {
                if let Some(output) = call.raw.output.as_mut() {
                    output.blob = None;
                    output.truncated = true;
                }
                if let Some(completion) = completion.as_mut() {
                    completion.payload = serde_json::to_value(call).unwrap_or(Json::Null);
                    completion.output_blob = None;
                }
                refused.insert(
                    key.clone(),
                    Proof::unavailable("native_web_reported_output_unavailable"),
                );
            } else {
                if let Some(Some(value)) = full {
                    reported.insert(key.clone(), value);
                }
                valid.push(key.clone());
            }
        } else {
            refused.insert(
                key.clone(),
                Proof::unavailable("native_web_metadata_unavailable"),
            );
        }
        completions.insert(key.clone(), completion);
    }
    let mut proofs = source.prepare_web_inputs(&valid, &reported, outputs);
    proofs.extend(refused);
    proofs
        .into_iter()
        .map(|(key, proof)| {
            let completion = completions.remove(&key).flatten();
            (key, (proof, completion))
        })
        .collect()
}

/// Only a hash and the original ownership remain in memory while the native rollout finishes.
#[derive(Debug)]
pub(crate) struct Pending {
    pub snapshot: Option<Hash>,
    pub turn: Id,
    pub stream: String,
    pub model: Option<String>,
    pub capability: Option<&'static str>,
    pub unproven: Option<&'static str>,
}
impl Pending {
    pub(crate) fn new(
        completion: &ItemCompleted,
        call: &ToolCallPayload,
        turn: Id,
        stream: &str,
        model: Option<String>,
        outputs: Option<&OutputStore>,
    ) -> Self {
        let snapshot = outputs.and_then(|outputs| {
            serde_json::to_vec(completion)
                .ok()
                .and_then(|bytes| outputs.put(&bytes).ok())
                .map(|blob| blob.hash)
        });
        let capability = match &call.tool {
            ToolPayload::WebSearch {
                action: Some(orch_agent_contract::events::WebAction::Search),
                ..
            } => Some("web_search"),
            ToolPayload::WebSearch {
                action:
                    Some(
                        orch_agent_contract::events::WebAction::OpenPage { .. }
                        | orch_agent_contract::events::WebAction::FindInPage { .. },
                    ),
                ..
            } => Some("page_fetch"),
            _ => None,
        };
        Self {
            snapshot,
            turn,
            stream: stream.into(),
            model,
            capability,
            unproven: None,
        }
    }
}

pub(crate) fn read_completion(outputs: &OutputStore, hash: Option<Hash>) -> Option<ItemCompleted> {
    let hash = hash?;
    let file = std::fs::File::open(outputs.directory().join(hash.to_string())).ok()?;
    // One normalized native frame plus its retained raw representation; never a whole history.
    let limit = u64::try_from(crate::protocol::MAX_FRAME_BYTES.saturating_mul(2)).ok()?;
    if file.metadata().ok()?.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).ok()?;
    if u64::try_from(bytes.len()).ok()? > limit || Hash::of(&bytes) != hash {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn finish(
    key: &Key,
    pending: &Pending,
    proof: &Proof,
    completion: Option<ItemCompleted>,
) -> (Vec<SessionEvent>, Option<i64>) {
    let mut outcome = pending
        .unproven
        .map(Outcome::Unproven)
        .unwrap_or(proof.outcome);
    let parsed = completion
        .filter(|item| item.native_ref == key.call)
        .and_then(|item| {
            let call = serde_json::from_value::<ToolCallPayload>(item.payload.clone()).ok()?;
            Some((item, call))
        });
    if let Some((item, _)) = &parsed {
        if item.status != ItemStatus::Completed && outcome == Outcome::Success {
            outcome = Outcome::Unproven("native_web_sources_disagree");
        }
    } else if !matches!(outcome, Outcome::Unproven(_)) {
        outcome = Outcome::Unproven("native_web_metadata_unavailable");
    }
    if outcome == Outcome::Success
        && pending.capability.is_some()
        && pending.model.is_none()
        && proof.model.is_none()
    {
        outcome = Outcome::Unproven("native_web_model_unreported");
    }
    let mut events = Vec::new();
    if let Some((mut item, mut call)) = parsed {
        if proof.output.is_some() {
            call.raw.output = proof.output.clone();
        }
        match outcome {
            Outcome::Success => {
                item.status = ItemStatus::Completed;
                call.raw.is_error = false;
            }
            Outcome::Failed => {
                item.status = ItemStatus::Failed;
                call.raw.is_error = true;
            }
            Outcome::Unproven(_) => {}
        }
        item.payload = serde_json::to_value(call).unwrap_or(Json::Null);
        events.push(SessionEvent::ItemCompleted(item));
    }
    let (count, notice) = match outcome {
        Outcome::Success => (Some(1), None),
        Outcome::Failed => (
            Some(0),
            Some((
                "native_web_action_failed",
                "The web action failed.",
                "native_web_request_failed",
            )),
        ),
        Outcome::Unproven(reason) => (
            None,
            Some((
                "native_web_action_unproven",
                "The web action could not be verified.",
                reason,
            )),
        ),
    };
    if let Some((code, message, reason)) = notice {
        let error = ErrorPayload {
            code: code.into(),
            message: message.into(),
            retryable: false,
            details: Some(json!({"reason":reason})),
            window: None,
            resets_at: None,
        };
        let payload = serde_json::to_value(&error).unwrap_or(Json::Null);
        let reference = format!("web-result-notice:{}", key.call);
        events.push(SessionEvent::ItemStarted(ItemStarted {
            native_ref: reference.clone(),
            turn_id: pending.turn,
            stream: pending.stream.clone(),
            parent_native_ref: Some(key.call.clone()),
            content: ItemContent::Error(error),
        }));
        events.push(SessionEvent::ItemCompleted(ItemCompleted {
            native_ref: reference,
            status: ItemStatus::Failed,
            payload,
            output_blob: None,
        }));
    }
    (events, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_history::ResultItem;
    use orch_agent_contract::events::RawCall;
    use orch_types::Time;
    fn result(output: Json) -> Result<ResultItem, &'static str> {
        Ok(ResultItem {
            payload: json!({"type":"function_call_output","call_id":"call","output":output}),
            model: Some("source-model".into()),
        })
    }
    fn call() -> ToolCallPayload {
        ToolCallPayload {
            title: "Search the web".into(),
            started_at: Time::new(chrono::DateTime::UNIX_EPOCH),
            duration_ms: None,
            tool: ToolPayload::WebSearch {
                query: "question".into(),
                results: Vec::new(),
                action: Some(orch_agent_contract::events::WebAction::Search),
            },
            raw: RawCall {
                name: "webSearch".into(),
                input: OutputRef::of("{}"),
                output: None,
                is_error: false,
            },
        }
    }
    #[test]
    fn only_a_content_array_proves_success_and_failure_words_never_replace_structural_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let outputs = OutputStore::new(dir.path().join("outputs"));
        for text in [
            "No results",
            "An error-looking quotation is a valid search result",
        ] {
            let proof = Proof::of(result(json!([{"type":"input_text","text":text}])), &outputs);
            assert_eq!(proof.outcome, Outcome::Success);
            assert_eq!(proof.model.as_deref(), Some("source-model"));
        }
        assert_eq!(
            Proof::of(result(json!("an HTTP failure")), &outputs).outcome,
            Outcome::Failed
        );
        assert!(matches!(
            Proof::of(result(json!({"looks":"successful"})), &outputs).outcome,
            Outcome::Unproven(_)
        ));
        for malformed in [
            json!(["not a content part"]),
            json!([{"type":"input_image","image_url":"not a web text result"}]),
        ] {
            assert!(matches!(
                Proof::of(result(malformed), &outputs).outcome,
                Outcome::Unproven(_)
            ));
        }
        assert!(matches!(
            Proof::of(Err("missing"), &outputs).outcome,
            Outcome::Unproven("missing")
        ));
    }
    #[test]
    fn a_timed_out_proof_keeps_its_reason_and_does_not_replace_the_reported_action() {
        let call = call();
        let key = Key {
            native_thread: "native".into(),
            native_turn: "turn".into(),
            call: "call".into(),
        };
        let complete = ItemCompleted {
            native_ref: key.call.clone(),
            status: ItemStatus::Completed,
            payload: serde_json::to_value(&call).unwrap(),
            output_blob: None,
        };
        let pending = Pending::new(&complete, &call, Id::nil(), "main", None, None);
        let (events, count) = finish(
            &key,
            &pending,
            &Proof::unavailable("native_result_timed_out"),
            None,
        );
        assert_eq!(count, None);
        assert!(!events.iter().any(
            |event| matches!(event, SessionEvent::ItemCompleted(item) if item.native_ref == "call")
        ));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SessionEvent::ItemCompleted(item)
            if item.payload["details"]["reason"] == "native_result_timed_out"))
        );
    }

    #[test]
    fn snapshots_are_hash_checked_and_failed_or_missing_results_never_count_success() {
        let dir = tempfile::tempdir().unwrap();
        let outputs = OutputStore::new(dir.path().join("outputs"));
        let call = call();
        let key = Key {
            native_thread: "native".into(),
            native_turn: "turn".into(),
            call: "call".into(),
        };
        let complete = ItemCompleted {
            native_ref: key.call.clone(),
            status: ItemStatus::Completed,
            payload: serde_json::to_value(&call).unwrap(),
            output_blob: None,
        };
        let pending = Pending::new(&complete, &call, Id::nil(), "main", None, Some(&outputs));
        let proof = Proof::of(result(json!([{"type":"input_text","text":"ok"}])), &outputs);
        let (events, count) = finish(
            &key,
            &pending,
            &proof,
            read_completion(&outputs, pending.snapshot),
        );
        assert_eq!(count, Some(1));
        assert!(
            matches!(&events[0],SessionEvent::ItemCompleted(item) if item.status==ItemStatus::Completed)
        );
        let failed = Proof::of(result(json!("failure")), &outputs);
        let (events, count) = finish(
            &key,
            &pending,
            &failed,
            read_completion(&outputs, pending.snapshot),
        );
        assert_eq!(count, Some(0));
        assert!(
            matches!(&events[0],SessionEvent::ItemCompleted(item) if item.status==ItemStatus::Failed)
        );
        let (events, count) = finish(
            &key,
            &pending,
            &Proof::unavailable("missing"),
            read_completion(&outputs, pending.snapshot),
        );
        assert_eq!(count, None);
        assert!(events.iter().any(|event|matches!(event,SessionEvent::ItemCompleted(item) if item.payload["code"]=="native_web_action_unproven")));
        std::fs::write(
            outputs
                .directory()
                .join(pending.snapshot.unwrap().to_string()),
            b"tampered",
        )
        .unwrap();
        assert!(read_completion(&outputs, pending.snapshot).is_none());
        assert_eq!(
            finish(&key, &pending, &proof, None).1,
            None,
            "lost metadata cannot be charged from another completion"
        );
    }
}
