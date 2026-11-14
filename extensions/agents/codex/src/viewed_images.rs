//! Retain exact native image-tool output using the existing blob cache and event types.
use crate::events::{OutputStore, retain_inline_image};
use crate::native_history::Results;
use orch_agent_contract::events::{
    ErrorPayload, ItemCompleted, ItemContent, ItemStarted, ItemStatus, OutputRef, RawCall,
    SessionEvent, ToolCallPayload, ToolPayload,
};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key {
    pub native_thread: String,
    pub native_turn: String,
    pub call: String,
}
#[derive(Debug, Clone)]
pub(crate) struct Saved {
    pub parts: Vec<(usize, Result<ToolPayload, &'static str>)>,
    pub raw: Option<OutputRef>,
}
pub(crate) type Prepared = Result<Saved, &'static str>;

/// The path is supplied by the native thread's own opening/started metadata, not an image path.
#[derive(Debug)]
pub(crate) struct Sources {
    home: Option<PathBuf>,
    paths: BTreeMap<String, PathBuf>,
    indices: BTreeMap<String, Results>,
    cancel: Arc<AtomicBool>,
    web_verified: bool,
}
impl Default for Sources {
    fn default() -> Self {
        Self {
            home: None,
            paths: BTreeMap::new(),
            indices: BTreeMap::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            web_verified: false,
        }
    }
}
impl Sources {
    pub(crate) fn with_web_version(mut self, version: &str) -> Self {
        self.web_verified = crate::descriptor::is_pinned_build(version);
        self
    }
    pub(crate) fn new(home: Option<PathBuf>, thread: &Json) -> Self {
        let mut source = Self {
            home,
            ..Self::default()
        };
        source.observe(thread);
        source
    }
    /// The caller accepts this only for its registered root or already known child.
    pub(crate) fn observe(&mut self, thread: &Json) {
        if let (Some(native), Some(path)) = (thread["id"].as_str(), thread["path"].as_str()) {
            self.paths.insert(native.into(), path.into());
        }
    }
    pub(crate) fn cancellation(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Keep source metadata available if an IO task is cancelled or fails before returning its indexes.
    pub(crate) fn take_work(&mut self) -> Self {
        if self.cancel.load(Ordering::Acquire) {
            self.cancel = Arc::new(AtomicBool::new(false));
            self.indices.clear();
        }
        Self {
            home: self.home.clone(),
            paths: self.paths.clone(),
            indices: std::mem::take(&mut self.indices),
            cancel: Arc::clone(&self.cancel),
            web_verified: self.web_verified,
        }
    }
    pub(crate) fn finish_work(&mut self, work: Self) {
        self.indices = work.indices;
    }

    /// Run outside the shared session lock; retain only blob metadata for the waiting mapper.
    pub(crate) fn prepare(
        &mut self,
        keys: &[Key],
        outputs: &OutputStore,
    ) -> BTreeMap<Key, Prepared> {
        let mut refreshed = BTreeMap::new();
        keys.iter()
            .map(|key| {
                let result = self.one(key, outputs, &mut refreshed);
                (key.clone(), result)
            })
            .collect()
    }
    fn one(
        &mut self,
        key: &Key,
        outputs: &OutputStore,
        refreshed: &mut BTreeMap<String, bool>,
    ) -> Prepared {
        self.result(key, refreshed)
            .map(|result| retain_output(&result.payload, outputs))
    }
    pub(crate) fn prepare_web_inputs(
        &mut self,
        keys: &[Key],
        reported: &BTreeMap<Key, Json>,
        outputs: &OutputStore,
    ) -> BTreeMap<Key, crate::native_web::Proof> {
        if !self.web_verified {
            return keys
                .iter()
                .map(|key| {
                    (
                        key.clone(),
                        crate::native_web::Proof::unavailable("native_web_version_unverified"),
                    )
                })
                .collect();
        }
        let mut refreshed = BTreeMap::new();
        keys.iter()
            .map(|key| {
                (
                    key.clone(),
                    match reported.get(key) {
                        Some(reported) => crate::native_web::Proof::of_reported(
                            self.result(key, &mut refreshed),
                            Some(reported),
                            outputs,
                        ),
                        None => {
                            crate::native_web::Proof::of(self.result(key, &mut refreshed), outputs)
                        }
                    },
                )
            })
            .collect()
    }
    fn result(
        &mut self,
        key: &Key,
        refreshed: &mut BTreeMap<String, bool>,
    ) -> Result<crate::native_history::ResultItem, &'static str> {
        let home = self.home.as_ref().ok_or("native_result_home_unknown")?;
        let path = self
            .paths
            .get(&key.native_thread)
            .ok_or("native_result_path_unknown")?;
        let index = match self.indices.entry(key.native_thread.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => entry.insert(
                Results::with_cancel(
                    home.clone(),
                    key.native_thread.clone(),
                    Arc::clone(&self.cancel),
                )
                .map_err(|_| "native_result_index_unavailable")?,
            ),
        };
        let ready = *refreshed
            .entry(key.native_thread.clone())
            .or_insert_with(|| index.refresh(path).is_ok());
        if !ready {
            return Err("native_result_history_unavailable");
        }
        let payload = index
            .get_with_model(&key.native_turn, &key.call)
            .map_err(|_| "native_result_identity_changed")?
            .ok_or("native_result_not_found")?;
        Ok(payload)
    }
}

/// Whole history pages name their own native turns, independent of the currently active turn.
pub(crate) fn page_keys(native_thread: &str, turns: &[Json]) -> Vec<Key> {
    page_keys_for(native_thread, turns, "imageView")
}
pub(crate) fn page_web_keys(native_thread: &str, turns: &[Json]) -> Vec<Key> {
    page_keys_for(native_thread, turns, "webSearch")
}
fn page_keys_for(native_thread: &str, turns: &[Json], kind: &str) -> Vec<Key> {
    let mut keys = BTreeSet::new();
    for turn in turns {
        let Some(native_turn) = turn["id"].as_str().filter(|turn| !turn.is_empty()) else {
            continue;
        };
        if turn["status"] == "inProgress" {
            continue;
        }
        for item in turn["items"].as_array().into_iter().flatten() {
            if item["type"] != kind {
                continue;
            }
            let Some(call) = item["id"].as_str().filter(|call| !call.is_empty()) else {
                continue;
            };
            keys.insert(Key {
                native_thread: native_thread.into(),
                native_turn: native_turn.into(),
                call: call.into(),
            });
        }
    }
    keys.into_iter().collect()
}

fn retain_output(payload: &Json, outputs: &OutputStore) -> Saved {
    let raw = retain_original(payload, outputs);
    let parts = payload["output"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, part)| part["type"] == "input_image")
        .map(|(index, part)| {
            let image = inline_image(part, outputs);
            (index, image)
        })
        .collect();
    Saved { parts, raw }
}
pub(crate) fn retain_original(payload: &Json, outputs: &OutputStore) -> Option<OutputRef> {
    let original = payload.to_string();
    let raw = outputs.put(original.as_bytes()).ok().map(|blob| OutputRef {
        inline_limit: None,
        inline: String::new(),
        total_len: i64::try_from(original.len()).unwrap_or(i64::MAX),
        truncated: true,
        blob: Some(blob.hash),
        retained: Some(orch_api_contract::threads::OutputRange {
            offset: 0,
            len: blob.size,
        }),
    });
    drop(original);
    raw
}
fn inline_image(part: &Json, outputs: &OutputStore) -> Result<ToolPayload, &'static str> {
    let url = part["image_url"]
        .as_str()
        .ok_or("native_image_data_not_inline")?;
    let (media, encoded) = url
        .strip_prefix("data:")
        .and_then(|value| value.split_once(";base64,"))
        .ok_or("native_image_data_not_inline")?;
    retain_inline_image(media, Some(encoded), None, outputs)
}

/// Further images and explicit retention failures keep the original tool/turn/stream ownership.
pub(crate) fn additional_events(
    parent: &str,
    prepared: &Prepared,
    turn: Id,
    stream: &str,
    started_at: Time,
    name: Option<&str>,
) -> Vec<SessionEvent> {
    let mut out = Vec::new();
    let (parts, raw) = match prepared {
        Ok(saved) => (saved.parts.clone(), saved.raw.clone()),
        Err(reason) => (vec![(0, Err(*reason))], None),
    };
    let parts = if parts.is_empty() {
        vec![(0, Err("native_result_has_no_image"))]
    } else {
        parts
    };
    // A failed image still keeps its exact original on the ordinary raw-output path.
    if parts.iter().all(|(_, image)| image.is_err()) && raw.is_some() {
        let reference = format!("image-source:{parent}");
        let call = ToolCallPayload {
            title: "view_image".into(),
            started_at,
            duration_ms: None,
            tool: ToolPayload::Other {
                name: "view_image".into(),
                arguments: None,
                result: None,
            },
            raw: RawCall {
                name: "view_image".into(),
                input: OutputRef::of("{}"),
                output: raw.clone(),
                is_error: true,
            },
        };
        let payload = serde_json::to_value(&call).unwrap_or(Json::Null);
        out.push(SessionEvent::ItemStarted(ItemStarted {
            native_ref: reference.clone(),
            turn_id: turn,
            stream: stream.into(),
            parent_native_ref: Some(parent.into()),
            content: ItemContent::ToolCall(call),
        }));
        out.push(SessionEvent::ItemCompleted(ItemCompleted {
            native_ref: reference,
            status: ItemStatus::Failed,
            payload,
            output_blob: None,
        }));
    }
    for (index, image) in parts {
        let reference = format!("image-result:{parent}:{index}");
        let (content, payload, status) = match image {
            Ok(mut tool) => {
                if let ToolPayload::Image {
                    name: image_name, ..
                } = &mut tool
                    && image_name.is_none()
                {
                    *image_name = name.map(str::to_owned);
                }
                let call = ToolCallPayload {
                    title: "Image".into(),
                    started_at,
                    duration_ms: None,
                    tool,
                    raw: RawCall {
                        name: "image_result".into(),
                        input: OutputRef::of(&json!({"part":index}).to_string()),
                        output: raw.clone(),
                        is_error: false,
                    },
                };
                let payload = serde_json::to_value(&call).unwrap_or(Json::Null);
                (ItemContent::ToolCall(call), payload, ItemStatus::Completed)
            }
            Err(reason) => {
                let error = ErrorPayload {
                    code: "image_result_unavailable".into(),
                    message: "The returned image could not be retained.".into(),
                    retryable: false,
                    details: Some(json!({"reason":reason})),
                    window: None,
                    resets_at: None,
                };
                let payload = serde_json::to_value(&error).unwrap_or(Json::Null);
                (ItemContent::Error(error), payload, ItemStatus::Failed)
            }
        };
        out.push(SessionEvent::ItemStarted(ItemStarted {
            native_ref: reference.clone(),
            turn_id: turn,
            stream: stream.into(),
            parent_native_ref: Some(parent.into()),
            content,
        }));
        out.push(SessionEvent::ItemCompleted(ItemCompleted {
            native_ref: reference,
            status,
            payload,
            output_blob: None,
        }));
    }
    out
}

pub(crate) fn promote_record(
    record: &std::path::Path,
    temporary: &OutputStore,
    retained: &OutputStore,
) -> std::io::Result<()> {
    use orch_agent_contract::discovery::{RECORD_MAX_BYTES, RecordedEntry};
    use std::io::{BufRead, BufReader, Read};
    let file = std::fs::File::open(record)?;
    let maximum = u64::try_from(RECORD_MAX_BYTES).map_err(std::io::Error::other)?;
    if file.metadata()?.len() > maximum {
        return Err(std::io::Error::other(
            "the native record exceeds its declared format budget",
        ));
    }
    for line in BufReader::new(file.take(maximum)).lines() {
        let line = line?;
        let Ok(RecordedEntry::Item(item)) = serde_json::from_str::<RecordedEntry>(&line) else {
            continue;
        };
        if let ItemContent::ToolCall(call) = item.content {
            for hash in call.output_blobs() {
                let bytes = match std::fs::read(temporary.directory().join(hash.to_string())) {
                    Ok(bytes) => bytes,
                    // A carried pre-image may already live in the control content store. The shared
                    // keeper checks missing bytes before consulting this adapter's output cache.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                if orch_types::Hash::of(&bytes) != hash {
                    return Err(std::io::Error::other(
                        "the retained record output does not match its hash",
                    ));
                }
                retained.put(&bytes)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use orch_types::Hash;
    const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    fn native_source(home: &std::path::Path) -> (Sources, Key, Json) {
        let path = home.join("sessions/2026/11/08/rollout-2026-11-08T00-00-00-native.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let payload = json!({"type":"function_call_output","call_id":"view","output":[
            {"type":"input_text","text":"The native tool read this through its selected filesystem"},
            {"type":"input_image","image_url":format!("data:image/png;base64,{PIXEL}"),"future":"kept in raw output"}
        ]});
        let rows = [
            json!({"type":"session_meta","payload":{"id":"native"}}),
            json!({"type":"turn_context","payload":{"turn_id":"one"}}),
            json!({"type":"response_item","payload":payload}),
        ];
        std::fs::write(
            &path,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();
        (
            Sources::new(Some(home.into()), &json!({"id":"native","path":path})),
            Key {
                native_thread: "native".into(),
                native_turn: "one".into(),
                call: "view".into(),
            },
            payload,
        )
    }
    #[test]
    fn exact_native_bytes_and_the_complete_original_are_retained_without_ordinary_base64_metadata()
    {
        let home = tempfile::tempdir().unwrap();
        let (mut sources, key, payload) = native_source(home.path());
        let output = OutputStore::new(home.path().join("output-cache"));
        let mut prepared = sources.prepare(std::slice::from_ref(&key), &output);
        let saved = prepared.remove(&key).unwrap().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(PIXEL)
            .unwrap();
        assert!(
            matches!(&saved.parts[..], [(1,Ok(ToolPayload::Image {blob,media_type,..}))] if *blob==Hash::of(&bytes)&&media_type=="image/png")
        );
        let raw = saved.raw.as_ref().unwrap();
        assert!(raw.inline.is_empty());
        assert!(raw.truncated);
        let original: Json = serde_json::from_slice(
            &std::fs::read(output.directory().join(raw.blob.unwrap().to_string())).unwrap(),
        )
        .unwrap();
        assert_eq!(original, payload);
        assert_eq!(
            std::fs::read(output.directory().join(Hash::of(&bytes).to_string())).unwrap(),
            bytes
        );
        assert!(
            !serde_json::to_string(saved.parts[0].1.as_ref().unwrap())
                .unwrap()
                .contains(PIXEL)
        );
    }
    #[test]
    fn unreported_url_only_empty_and_invalid_images_have_no_fabricated_blob() {
        let home = tempfile::tempdir().unwrap();
        let outputs = OutputStore::new(home.path().join("outputs"));
        for part in [
            json!({"type":"input_image","file_id":"not-inline"}),
            json!({"type":"input_image","image_url":"https://outside.invalid/pixel.png"}),
            json!({"type":"input_image","image_url":"data:image/png;base64,"}),
            json!({"type":"input_image","image_url":"data:image/png;base64,%%%"}),
        ] {
            let saved = retain_output(
                &json!({"type":"function_call_output","call_id":"view","output":[part]}),
                &outputs,
            );
            assert!(saved.parts.iter().all(|(_, image)| image.is_err()));
            let events = additional_events(
                "view",
                &Ok(saved),
                Id::nil(),
                "main",
                Time::new(chrono::DateTime::UNIX_EPOCH),
                None,
            );
            assert!(events.iter().any(|event| matches!(event,SessionEvent::ItemCompleted(item)
                if item.native_ref=="image-source:view" && item.payload["raw"]["output"]["blob"].is_string())));
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event,SessionEvent::ItemCompleted(item)
                if item.payload["tool"]["kind"]=="image"))
            );
        }
    }
    #[test]
    fn cancelling_a_batch_does_not_poison_the_next_batch_or_discard_its_actual_path() {
        let home = tempfile::tempdir().unwrap();
        let (mut source, key, _) = native_source(home.path());
        let outputs = OutputStore::new(home.path().join("outputs"));
        let old = source.take_work();
        old.cancellation().store(true, Ordering::Release);
        drop(old);
        let mut next = source.take_work();
        assert!(
            next.prepare(std::slice::from_ref(&key), &outputs)
                .remove(&key)
                .unwrap()
                .is_ok()
        );
        source.finish_work(next);
    }
    #[test]
    fn only_retained_records_typed_dependencies_leave_the_temporary_output_cache() {
        let root = tempfile::tempdir().unwrap();
        let temporary = OutputStore::new(root.path().join("temporary"));
        let retained = OutputStore::new(root.path().join("retained"));
        let image = temporary.put(b"image bytes").unwrap().hash;
        let raw = temporary.put(b"complete raw result").unwrap().hash;
        let hidden = temporary.put(b"opaque data").unwrap().hash;
        let payload = json!({"title":"View","started_at":"2026-01-01T00:00:00Z","duration_ms":null,
            "tool":{"kind":"image","blob":image,"media_type":"image/png","name":null,"caption":null},
            "raw":{"name":"view_image","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},
                "output":{"inline":"","total_len":19,"truncated":true,"blob":raw},"is_error":false}});
        let record = root.path().join("record.jsonl");
        let item = json!({"entry":"item","native_ref":"image","turn":1,"stream":"main","parent_native_ref":null,
            "status":"completed","at":null,"content":{"kind":"tool_call","payload":payload}});
        let mut opaque = item.clone();
        opaque["native_ref"] = json!("opaque");
        opaque["content"]["payload"]["tool"] = json!({"kind":"mcp","server":"fixture","tool":"read","arguments":{"blob":hidden},"result":{"blob":hidden},"error":null});
        std::fs::write(&record, format!("{item}\n{opaque}\n")).unwrap();
        promote_record(&record, &temporary, &retained).unwrap();
        assert_eq!(
            std::fs::read(retained.directory().join(image.to_string())).unwrap(),
            b"image bytes"
        );
        assert_eq!(
            std::fs::read(retained.directory().join(raw.to_string())).unwrap(),
            b"complete raw result"
        );
        assert!(!retained.directory().join(hidden.to_string()).exists());
        assert!(temporary.directory().join(hidden.to_string()).exists());
    }
}
