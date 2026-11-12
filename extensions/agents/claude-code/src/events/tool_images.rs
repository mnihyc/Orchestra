//! Tool-returned images use the same retained blob and image-item vocabulary as native image tools.
use super::*;
use base64::Engine as _;
use orch_agent_contract::events::BlobRef;
use orch_types::Hash;
use std::io::Write as _;

fn keep(
    directory: Option<&std::path::Path>,
    image: &Json,
) -> Result<(ToolPayload, BlobRef), &'static str> {
    let source = image.get("source").unwrap_or(image);
    let media = source
        .get("media_type")
        .or_else(|| source.get("mimeType"))
        .and_then(Json::as_str)
        .filter(|media| media.starts_with("image/"))
        .ok_or("The tool image has no image media type.")?;
    if source
        .get("type")
        .and_then(Json::as_str)
        .is_some_and(|kind| kind != "base64" && kind != "image")
    {
        return Err("The tool image has no inline bytes; no external address was fetched.");
    }
    let data = source
        .get("data")
        .and_then(Json::as_str)
        .ok_or("The tool image has no inline bytes.")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(data))
        .map_err(|_| "The tool image has invalid base64 data.")?;
    if bytes.is_empty() {
        return Err("The tool image has no bytes.");
    }
    let directory = directory.ok_or("The tool image has no retained output directory.")?;
    std::fs::create_dir_all(directory).map_err(|_| "The tool image could not be retained.")?;
    let hash = Hash::of(&bytes);
    let path = directory.join(hash.to_string());
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .map_err(|_| "The tool image could not be retained.")?;
    file.write_all(&bytes)
        .map_err(|_| "The tool image could not be retained.")?;
    match file.persist_noclobber(&path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::read(&path).map_err(|_| "The retained tool image could not be read.")?
                != bytes
            {
                return Err("The retained tool image differs from its hash.");
            }
        }
        Err(_) => return Err("The tool image could not be retained."),
    }
    let blob = BlobRef {
        hash,
        size: i64::try_from(bytes.len()).map_err(|_| "The tool image is too large to name.")?,
    };
    Ok((
        ToolPayload::Image {
            blob: hash,
            media_type: media.into(),
            name: image.get("name").and_then(Json::as_str).map(str::to_owned),
            caption: None,
            capability: None,
        },
        blob,
    ))
}

impl EventMapper {
    pub(super) fn tool_image_results(
        &self,
        id: &str,
        state: &ToolState,
        outcome: &ToolOutcome,
        status: ItemStatus,
        out: &mut Vec<SessionEvent>,
    ) {
        for (index, part) in outcome.images.iter().enumerate() {
            let native_ref = format!("claude-tool-image:{id}:{index}");
            let (tool, output_blob, image_status) = match keep(self.image_outputs.as_deref(), part)
            {
                Ok((tool, blob)) => (tool, Some(blob), status),
                Err(message) => (
                    ToolPayload::Other {
                        name: "image_result".into(),
                        arguments: None,
                        result: Some(json!({"error":message})),
                    },
                    None,
                    ItemStatus::Failed,
                ),
            };
            let payload = ToolCallPayload {
                tool,
                title: "Image result".into(),
                started_at: state.started_at,
                duration_ms: None,
                raw: raw_call(&state.name, &state.input, None),
            };
            if let Some(turn_id) = state.origin_turn {
                out.push(SessionEvent::ItemStarted(ItemStarted {
                    turn_id,
                    native_ref: native_ref.clone(),
                    stream: state.stream.clone(),
                    parent_native_ref: Some(id.into()),
                    content: ItemContent::ToolCall(payload.clone()),
                }));
            }
            if let Ok(payload) = serde_json::to_value(payload) {
                out.push(SessionEvent::ItemCompleted(ItemCompleted {
                    native_ref,
                    status: image_status,
                    payload,
                    output_blob,
                }));
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use orch_types::{ManualClock, new_id};

    const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==";

    fn mapper() -> (EventMapper, tempfile::TempDir, TurnId) {
        let directory = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::new("2026-11-08T00:00:00Z".parse().unwrap()));
        let mut mapper = EventMapper::new(
            MapperSettings {
                directory: "/work".into(),
                cwd: "/work".into(),
                raw_events: false,
            },
            clock,
        );
        mapper.set_image_outputs(directory.path().join("outputs"));
        let turn = new_id();
        mapper.begin_turn(turn);
        (mapper, directory, turn)
    }

    fn call(mapper: &mut EventMapper) {
        mapper.on_frame(&Frame::from_value(json!({
            "type":"assistant","message":{"id":"message","role":"assistant","content":[
                {"type":"tool_use","id":"snapshot","name":"mcp__camera__snapshot","input":{}}
            ]}
        })));
    }

    fn result(mapper: &mut EventMapper, parts: Json) -> Vec<SessionEvent> {
        mapper.on_frame(&Frame::from_value(json!({
            "type":"user","message":{"role":"user","content":[
                {"type":"tool_result","tool_use_id":"snapshot","content":parts}
            ]}
        })))
    }

    #[test]
    fn mcp_images_keep_native_bytes_and_original_scope_without_replaying_the_tool() {
        let (mut mapper, directory, original_turn) = mapper();
        mapper.process_started(true);
        call(&mut mapper);
        mapper.begin_turn(new_id());
        let parts = json!([
            {"type":"text","text":"The camera returned two images"},
            {"type":"image","name":"snapshot.png","source":{"type":"base64","media_type":"image/png","data":PIXEL}},
            {"type":"image","mimeType":"image/png","data":PIXEL}
        ]);
        let events = result(&mut mapper, parts.clone());
        let images: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ItemCompleted(item)
                    if item.payload.pointer("/tool/kind").and_then(Json::as_str)
                        == Some("image") =>
                {
                    Some(item)
                }
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 2);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(PIXEL)
            .unwrap();
        let hash = Hash::of(&bytes);
        assert_eq!(
            std::fs::read(directory.path().join("outputs").join(hash.to_string())).unwrap(),
            bytes
        );
        for image in &images {
            assert_eq!(image.status, ItemStatus::Completed);
            assert_eq!(image.output_blob.as_ref().unwrap().hash, hash);
            assert_eq!(image.payload["tool"]["blob"], hash.to_string());
            assert_eq!(image.payload["tool"]["media_type"], "image/png");
            let started = events
                .iter()
                .find_map(|event| match event {
                    SessionEvent::ItemStarted(started)
                        if started.native_ref == image.native_ref =>
                    {
                        Some(started)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(started.turn_id, original_turn);
            assert_eq!(started.stream, MAIN_STREAM);
            assert_eq!(started.parent_native_ref.as_deref(), Some("snapshot"));
            assert!(!image.payload.to_string().contains(PIXEL));
        }
        assert!(result(&mut mapper, parts).iter().all(|event| !matches!(event, SessionEvent::ItemCompleted(item) if item.output_blob.is_some())));
        assert_eq!(
            std::fs::read_dir(directory.path().join("outputs"))
                .unwrap()
                .count(),
            1,
            "identical bytes are one retained blob"
        );
    }

    #[test]
    fn image_identity_matches_the_same_native_tool_in_live_and_historical_generations() {
        let (mut live, _live_cache, _) = mapper();
        live.process_started(true);
        call(&mut live);
        let parts = json!([{"type":"image","source":{"type":"base64","media_type":"image/png","data":PIXEL}}]);
        let live_events = result(&mut live, parts.clone());
        let (mut historical, _history_cache, _) = mapper();
        call(&mut historical);
        let historical_events = result(&mut historical, parts);
        let identity = |events: &[SessionEvent]| {
            events
                .iter()
                .find_map(|event| match event {
                    SessionEvent::ItemCompleted(item) if item.output_blob.is_some() => {
                        Some(item.native_ref.clone())
                    }
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(identity(&live_events), identity(&historical_events));
        assert_eq!(identity(&live_events), "claude-tool-image:snapshot:0");
    }

    #[test]
    fn malformed_remote_and_unretainable_images_are_visible_failures_without_fake_blobs() {
        for part in [
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"not base64"}}),
            json!({"type":"image","source":{"type":"url","url":"https://private.invalid/image","media_type":"image/png"}}),
            json!({"type":"image","source":{"type":"base64","media_type":"text/html","data":PIXEL}}),
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":""}}),
        ] {
            let (mut mapper, directory, _) = mapper();
            call(&mut mapper);
            let events = result(&mut mapper, json!([part]));
            let image = events
                .iter()
                .find_map(|event| match event {
                    SessionEvent::ItemCompleted(item)
                        if item.native_ref.starts_with("claude-tool-image:") =>
                    {
                        Some(item)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(image.status, ItemStatus::Failed);
            assert!(image.output_blob.is_none());
            assert_eq!(image.payload["tool"]["kind"], "other");
            assert!(!image.payload.to_string().contains("private.invalid"));
            assert!(!directory.path().join("outputs").exists());
        }
        let directory = tempfile::tempdir().unwrap();
        let blocked = directory.path().join("file");
        std::fs::write(&blocked, b"not a directory").unwrap();
        assert!(
            keep(
                Some(&blocked),
                &json!({"type":"image","mimeType":"image/png","data":PIXEL})
            )
            .is_err()
        );
        assert!(
            keep(
                None,
                &json!({"type":"image","mimeType":"image/png","data":PIXEL})
            )
            .is_err()
        );
    }
}
