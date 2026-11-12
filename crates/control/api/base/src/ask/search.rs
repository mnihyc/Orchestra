//! The native ranked word/meaning stream, decoded without a second search engine.

use super::*;

use orch_api_contract::search::SearchFound;

use orch_api_contract::search::SearchStreamEvent;
pub(super) fn decode(bytes: &[u8]) -> ApiResult<Json> {
    let mut events = Vec::new();
    let mut citations = Vec::new();
    let mut ended = false;
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let event: SearchStreamEvent = serde_json::from_slice(line)
            .map_err(|_| denied("The source search stream was incomplete"))?;
        if ended {
            return Err(denied(
                "The source search stream continued after its cursor",
            ));
        }
        if let SearchStreamEvent::Hit { hit, .. } = &event {
            let (url, title) = match &hit.found {
                SearchFound::Name { node, href, .. } => (href.clone(), node.name.clone()),
                SearchFound::Item { thread, .. } => {
                    (format!("/thread/{}", thread.id), thread.name.clone())
                }
                SearchFound::Document {
                    document, block_id, ..
                } => (
                    format!(
                        "/doc/{}{}",
                        document.id,
                        block_id.map_or_else(String::new, |id| format!("?block={id}"))
                    ),
                    Some(document.title.clone()),
                ),
                SearchFound::ChatMessage { chat, message, .. } => (
                    format!(
                        "/chat/{}{}",
                        chat.id,
                        message
                            .reference_number
                            .map_or_else(String::new, |number| format!("?message={number}"))
                    ),
                    chat.name
                        .clone()
                        .or_else(|| chat.node.as_ref().and_then(|node| node.name.clone())),
                ),
            };
            citations.push(json!({"reference":url,"url":url,"title":title}));
        }
        ended |= matches!(event, SearchStreamEvent::Cursor { .. });
        events.push(serde_json::to_value(event).map_err(ApiError::internal)?);
    }
    if !ended {
        return Err(denied(
            "The source search ended before its continuation cursor",
        ));
    }
    Ok(json!({"events":events,"citations":citations}))
}

#[cfg(any(test, feature = "test-support"))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a deciding case fails by panicking"
)]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn ranked_search_keeps_native_sources_and_requires_its_terminal_cursor() {
        let id = "00000000-0000-4000-8000-000000000041";
        let hit = json!({"type":"hit","id":"native-part","found_by":"both","hit":{
            "kind":"name","node":{"kind":"topic","id":id,"name":"Readable topic","redacted":false},
            "path":[],"href":format!("/topic/{id}"),"at":"2026-11-08T00:00:00.000Z", "snippet":"", "matches":[]}});
        let cut = format!("{hit}\n");
        assert!(decode(cut.as_bytes()).is_err());
        let complete = format!("{cut}{{\"type\":\"cursor\",\"cursor\":null}}\n");
        let result = decode(complete.as_bytes()).unwrap();
        assert_eq!(result["events"][0]["found_by"], "both");
        assert_eq!(result["citations"][0]["title"], "Readable topic");
        assert_eq!(result["citations"][0]["url"], format!("/topic/{id}"));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_ranked_search_keeps_native_sources_and_requires_its_terminal_cursor() {
        self::ranked_search_keeps_native_sources_and_requires_its_terminal_cursor()
    }
}
