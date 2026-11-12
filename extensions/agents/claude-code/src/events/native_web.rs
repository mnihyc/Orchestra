//! Native server blocks share the ordinary tool lifecycle and the common web-action payload.
use super::*;
use orch_agent_contract::events::{WebAction, WebSearchResult};

fn text<'a>(value: &'a Json, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
}

pub(super) fn payload(name: &str, input: &Json, outcome: Option<&ToolOutcome>) -> ToolPayload {
    let content = outcome
        .and_then(|outcome| outcome.structured.as_ref())
        .and_then(|block| block.get("content"));
    let action = if name == "web_fetch" {
        WebAction::OpenPage {
            url: text(input, "url").map(str::to_owned),
        }
    } else {
        WebAction::Search
    };
    let results = if name == "web_fetch" {
        content
            .filter(|result| result["type"] == "web_fetch_result")
            .and_then(|result| {
                Some(WebSearchResult {
                    url: text(result, "url")?.to_owned(),
                    title: result
                        .pointer("/content/title")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                })
            })
            .into_iter()
            .collect()
    } else {
        content
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter(|result| result["type"] == "web_search_result")
            .filter_map(|result| {
                Some(WebSearchResult {
                    url: text(result, "url")?.to_owned(),
                    title: result
                        .get("title")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                })
            })
            .collect()
    };
    ToolPayload::WebSearch {
        action: Some(action),
        query: if name == "web_search" {
            text(input, "query").unwrap_or_default().to_owned()
        } else {
            String::new()
        },
        results,
    }
}

pub(super) fn title(name: &str, input: &Json) -> String {
    if name == "web_fetch" {
        text(input, "url").map_or_else(|| "Open page".into(), |url| format!("Open page: {url}"))
    } else {
        text(input, "query").map_or_else(
            || "Search the web".into(),
            |query| format!("Search the web: {query}"),
        )
    }
}

impl EventMapper {
    pub(super) fn native_web_start(
        &mut self,
        block: &Json,
        stream: &str,
        parent: Option<&str>,
        out: &mut Vec<SessionEvent>,
    ) -> bool {
        let (Some(id), Some(name)) = (text(block, "id"), text(block, "name")) else {
            return false;
        };
        if !matches!(name, "web_search" | "web_fetch") {
            return false;
        }
        if let Some(state) = self.tools.get(id) {
            if !state.native_web
                || state.name != name
                || state.stream != stream
                || state.parent.as_deref() != parent
            {
                return false;
            }
            if state.native_terminal.is_some() {
                return true;
            }
            if let Some(input) = block.get("input").filter(|input| input.is_object()) {
                self.tool_input_known(id, name, input.clone(), stream, parent, out);
            }
        } else {
            let input = block
                .get("input")
                .filter(|input| input.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            self.open_tool_kind(id, (name, true), &input, stream, parent, out);
        }
        true
    }

    pub(super) fn native_web_result(
        &mut self,
        block: &Json,
        stream: &str,
        out: &mut Vec<SessionEvent>,
    ) {
        let Some(id) = text(block, "tool_use_id") else {
            return;
        };
        let Some(state) = self.tools.get(id) else {
            return;
        };
        let wanted = if state.name == "web_search" {
            "web_search_tool_result"
        } else {
            "web_fetch_tool_result"
        };
        if !state.native_web
            || state.stream != stream
            || state.native_terminal.is_some()
            || block.get("type").and_then(Json::as_str) != Some(wanted)
        {
            return;
        }
        self.finish_input(id, out);
        let content = block.get("content");
        let error = |value: &Json| {
            matches!(
                value.get("type").and_then(Json::as_str),
                Some("web_search_tool_result_error" | "web_fetch_tool_result_error")
            )
        };
        let is_error = block.get("is_error").and_then(Json::as_bool) == Some(true)
            || content.is_some_and(|content| {
                error(content)
                    || content
                        .as_array()
                        .is_some_and(|items| items.iter().any(error))
            });
        self.tool_result(
            id,
            ToolOutcome {
                text: content.map_or_else(String::new, Json::to_string),
                images: Vec::new(),
                is_error,
                structured: Some(block.clone()),
            },
            out,
        );
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod tests {
    use super::*;
    use orch_types::{ManualClock, new_id};

    fn new_mapper() -> EventMapper {
        let mut mapper = EventMapper::new(
            MapperSettings {
                directory: "/fixture/work".into(),
                cwd: "/fixture/work".into(),
                raw_events: false,
            },
            Arc::new(ManualClock::new("2026-11-01T00:00:00Z".parse().unwrap())),
        );
        mapper.begin_turn(new_id());
        mapper
    }
    fn assistant(blocks: Vec<Json>) -> Json {
        json!({"type":"assistant","parent_tool_use_id":null,"message":{"id":"native-web-message","content":blocks}})
    }
    fn stream(index: u64, block: Json) -> Json {
        json!({"type":"stream_event","parent_tool_use_id":null,"api_message_id":"native-web-message","event":{"type":"content_block_start","index":index,"content_block":block}})
    }
    fn feed(mapper: &mut EventMapper, frames: Vec<Json>) -> Vec<SessionEvent> {
        frames
            .into_iter()
            .flat_map(|frame| mapper.on_frame(&Frame::from_value(frame)))
            .collect()
    }
    fn finished(events: &[SessionEvent]) -> Vec<&ItemCompleted> {
        events
            .iter()
            .filter_map(|event| {
                if let SessionEvent::ItemCompleted(item) = event {
                    item.payload.get("tool").is_some().then_some(item)
                } else {
                    None
                }
            })
            .collect()
    }
    fn search() -> (Json, Json) {
        (
            json!({"type":"server_tool_use","id":"native-search","name":"web_search","input":{"query":"Python official website"}}),
            json!({"type":"web_search_tool_result","tool_use_id":"native-search","content":[{"type":"web_search_result","title":"Python","url":"https://www.python.org/","encrypted_content":"fixture-source"},{"type":"web_search_result","url":"https://docs.python.org/","page_age":"reported"}]}),
        )
    }
    fn fetch() -> (Json, Json) {
        (
            json!({"type":"server_tool_use","id":"native-fetch","name":"web_fetch","input":{"url":"https://docs.python.org/"}}),
            json!({"type":"web_fetch_tool_result","tool_use_id":"native-fetch","content":{"type":"web_fetch_result","url":"https://docs.python.org/","retrieved_at":"2026-11-01T00:00:00Z","content":{"type":"document","title":"Python documentation","source":{"type":"text","media_type":"text/plain","data":"Only reported document text"}}}}),
        )
    }

    #[test]
    fn native_search_and_fetch_have_the_same_actions_links_and_raw_source_as_other_programs() {
        let (call, result) = search();
        let (fetch, fetched) = fetch();
        let mut mapper = new_mapper();
        let events = feed(
            &mut mapper,
            vec![assistant(vec![call, result, fetch, fetched])],
        );
        let completed = finished(&events);
        assert_eq!(completed.len(), 2);
        assert_eq!(
            completed[0].payload["tool"],
            json!({"kind":"web_search","action":{"type":"search"},"query":"Python official website","results":[{"title":"Python","url":"https://www.python.org/"},{"title":"","url":"https://docs.python.org/"}]})
        );
        assert_eq!(
            completed[1].payload["tool"],
            json!({"kind":"web_search","action":{"type":"open_page","url":"https://docs.python.org/"},"query":"","results":[{"title":"Python documentation","url":"https://docs.python.org/"}]})
        );
        assert_eq!(completed[0].payload["raw"]["name"], "web_search");
        assert_eq!(completed[1].payload["raw"]["name"], "web_fetch");
        assert!(
            completed[1]
                .payload
                .pointer("/raw/output/inline")
                .and_then(Json::as_str)
                .unwrap()
                .contains("Only reported document text")
        );
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, SessionEvent::RawFrame(_)))
        );
        assert!(!mapper.waiting_on_a_tool());
        let mut closed = Vec::new();
        mapper.close_open_items(ItemStatus::Failed, &mut closed);
        assert!(
            finished(&closed).is_empty(),
            "a later stream close cannot replace a completed native result"
        );
    }

    #[test]
    fn streamed_partial_input_then_whole_server_blocks_complete_exactly_one_native_item() {
        let (call, result) = search();
        let mut mapper = new_mapper();
        let events = feed(
            &mut mapper,
            vec![
                stream(
                    0,
                    json!({"type":"server_tool_use","id":"native-search","name":"web_search","input":{}}),
                ),
                json!({"type":"stream_event","parent_tool_use_id":null,"api_message_id":"native-web-message","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"Python official website\"}"}}}),
                json!({"type":"stream_event","parent_tool_use_id":null,"api_message_id":"native-web-message","event":{"type":"content_block_stop","index":0}}),
                stream(1, result.clone()),
                assistant(vec![call, result]),
            ],
        );
        assert_eq!(events.iter().filter(|event| matches!(event, SessionEvent::ItemStarted(item) if item.native_ref == "native-search")).count(), 1);
        let completed = finished(&events);
        assert_eq!(completed.len(), 1);
        assert_eq!(
            completed[0].payload["tool"]["query"],
            "Python official website"
        );
        assert_eq!(
            completed[0].payload["tool"]["results"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(!mapper.waiting_on_a_tool());
    }

    #[test]
    fn native_result_errors_are_failed_items_without_fabricated_links_status_or_counts() {
        for (name, kind) in [
            ("web_search", "web_search_tool_result"),
            ("web_fetch", "web_fetch_tool_result"),
        ] {
            let mut mapper = new_mapper();
            let error =
                json!({"type":format!("{kind}_error"),"error_code":"future_vendor_refusal"});
            let events = feed(
                &mut mapper,
                vec![assistant(vec![
                    json!({"type":"server_tool_use","id":"native-error","name":name,"input":{}}),
                    json!({"type":kind,"tool_use_id":"native-error","content":error}),
                ])],
            );
            let completed = finished(&events);
            assert_eq!(completed.len(), 1);
            assert_eq!(completed[0].status, ItemStatus::Failed);
            assert_eq!(completed[0].payload["raw"]["is_error"], true);
            let tool = &completed[0].payload["tool"];
            assert_eq!(tool["kind"], "web_search");
            assert_eq!(tool["results"], json!([]));
            assert!(tool.get("status").is_none() && tool.get("content").is_none());
            assert!(tool.get("count").is_none());
            if name == "web_fetch" {
                assert_eq!(tool["action"], json!({"type":"open_page","url":null}));
            }
        }
    }

    #[test]
    fn orphan_foreign_stream_and_wrong_family_results_cannot_complete_a_native_call() {
        let (call, result) = search();
        let mut mapper = new_mapper();
        let unowned = feed(&mut mapper, vec![assistant(vec![result.clone()])]);
        assert!(finished(&unowned).is_empty());
        feed(&mut mapper, vec![assistant(vec![call])]);
        let foreign = json!({"type":"assistant","parent_tool_use_id":"another-agent","message":{"id":"foreign-message","content":[result.clone()]}});
        let wrong =
            json!({"type":"web_fetch_tool_result","tool_use_id":"native-search","content":{}});
        let ignored = feed(&mut mapper, vec![foreign, assistant(vec![wrong])]);
        assert!(finished(&ignored).is_empty());
        assert!(mapper.waiting_on_a_tool());
        let actual = feed(&mut mapper, vec![assistant(vec![result])]);
        assert_eq!(finished(&actual).len(), 1);

        let mut client = new_mapper();
        let rows = feed(
            &mut client,
            vec![assistant(vec![
                json!({"type":"tool_use","id":"client-tool","name":"web_search","input":{"query":"ordinary client tool"}}),
                json!({"type":"web_search_tool_result","tool_use_id":"client-tool","content":[]}),
            ])],
        );
        assert!(
            finished(&rows).is_empty(),
            "a client name is not a native server-tool declaration"
        );
    }

    #[test]
    fn historical_server_blocks_keep_the_same_pending_and_completed_state_on_the_existing_spool() {
        let mut history = HistoryMapper::new(
            MapperSettings {
                directory: "/fixture/work".into(),
                cwd: "/fixture/work".into(),
                raw_events: false,
            },
            Arc::new(ManualClock::new("2026-11-01T00:00:00Z".parse().unwrap())),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();
        history.begin_turn(new_id()).unwrap();
        let (call, result) = search();
        let started = history
            .on_frame(&Frame::from_value(assistant(vec![call.clone()])))
            .unwrap();
        assert_eq!(
            started
                .iter()
                .filter(|event| matches!(event, SessionEvent::ItemStarted(_)))
                .count(),
            1
        );
        let done = history
            .on_frame(&Frame::from_value(assistant(vec![result.clone()])))
            .unwrap();
        assert_eq!(finished(&done).len(), 1);
        let repeated = history
            .on_frame(&Frame::from_value(assistant(vec![call, result])))
            .unwrap();
        assert!(finished(&repeated).is_empty());
        assert!(
            repeated
                .iter()
                .all(|event| !matches!(event, SessionEvent::ItemStarted(_)))
        );
        let mut cursor = 0;
        while let Some(closed) = history.close_step(false, &mut cursor).unwrap() {
            assert!(finished(&closed).is_empty());
        }
    }
}
