//! Native rate-limit notices remain visible beside the unchanged provider facts reader.
use super::*;

impl EventMapper {
    pub(super) fn rate_limit_event(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        if self.native_session.as_deref().is_some_and(|session| {
            frame
                .text("session_id")
                .is_some_and(|actual| session != actual)
        }) {
            return;
        }
        let Some(info) = frame.get("rate_limit_info") else {
            return;
        };
        self.failure.limit = Some(LimitInfo::read(info));
        if !info.is_object() {
            return;
        }
        let mut value = serde_json::Map::new();
        for (native, public) in [("status", "status"), ("rateLimitType", "limit_type")] {
            if let Some(word) = info
                .get(native)
                .and_then(Json::as_str)
                .filter(|word| !word.is_empty())
            {
                value.insert(public.into(), json!(word));
            }
        }
        for key in ["message", "reason"] {
            if let Some(word) = info
                .get(key)
                .or_else(|| frame.get(key))
                .and_then(Json::as_str)
                .filter(|word| !word.is_empty())
            {
                value.insert(key.into(), json!(word));
            }
        }
        self.native_marker(frame, "rate_limit", None, Json::Object(value), false, out);
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "a protocol assertion fails by panicking"
    )]
    use super::*;
    use orch_types::{ManualClock, new_id};

    fn mapper() -> EventMapper {
        let mut mapper = EventMapper::new(
            MapperSettings {
                directory: "/work".into(),
                cwd: "/work".into(),
                raw_events: false,
            },
            Arc::new(ManualClock::new("2026-11-07T00:00:00Z".parse().unwrap())),
        );
        mapper.set_native_session("session");
        mapper
    }
    fn frame(status: &str, uuid: &str) -> Frame {
        Frame::from_value(
            json!({"type":"rate_limit_event","session_id":"session","uuid":uuid,
            "rate_limit_info":{"status":status,"rateLimitType":"five_hour","utilization":0.78,
                "resetsAt":1900000000,"message":"Native rate-limit words","headers":{"Authorization":"private-credential"}},
            "request":{"input":"private-prompt"}}),
        )
    }
    fn notices(events: &[SessionEvent]) -> Vec<&ItemCompleted> {
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ItemCompleted(item)
                    if item.payload["marker"] == "harness_changed"
                        && item.payload["data"]["change"] == "rate_limit" =>
                {
                    Some(item)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn rate_events_keep_the_actual_words_and_origin_without_replacing_provider_facts_or_ending_a_turn()
     {
        let mut mapper = mapper();
        let turn = new_id();
        mapper.begin_turn(turn);
        let frame = frame("allowed_warning", "warning");
        let events = mapper.on_frame(&frame);
        let data = &notices(&events)[0].payload["data"];
        assert_eq!(data["turn_id"], json!(turn));
        assert_eq!(
            data["value"],
            json!({"status":"allowed_warning","limit_type":"five_hour","message":"Native rate-limit words"})
        );
        assert!(mapper.failure.limit.is_some());
        assert!(mapper.turn_active());
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, SessionEvent::TurnCompleted(_)))
        );
        let shown = serde_json::to_string(&events).unwrap();
        assert!(!shown.contains("private-credential"));
        assert!(!shown.contains("private-prompt"));
        assert!(mapper.on_frame(&frame).is_empty());
        let changed = mapper.on_frame(&self::frame("rejected", "rejected"));
        assert_eq!(
            notices(&changed)[0].payload["data"]["previous"]["value"]["status"],
            "allowed_warning"
        );
    }

    #[test]
    fn idle_unknown_child_unknown_status_and_foreign_notices_never_borrow_another_turn() {
        let mut mapper = mapper();
        let idle = mapper.on_frame(&frame("allowed", "idle"));
        assert!(notices(&idle)[0].payload["data"]["turn_id"].is_null());
        mapper.begin_turn(new_id());
        let mut child = frame("future-status", "child").raw().clone();
        child["parent_tool_use_id"] = json!("unowned-child");
        let events = mapper.on_frame(&Frame::from_value(child));
        assert_eq!(
            notices(&events)[0].payload["data"]["value"]["status"],
            "future-status"
        );
        assert!(notices(&events)[0].payload["data"]["turn_id"].is_null());
        let mut foreign = frame("rejected", "foreign").raw().clone();
        foreign["session_id"] = json!("another-session");
        assert!(mapper.on_frame(&Frame::from_value(foreign)).is_empty());
        assert!(
            mapper
                .on_frame(&Frame::from_value(json!({"type":"rate_limit_event"})))
                .is_empty()
        );
    }

    #[tokio::test]
    async fn recorded_limit_frames_still_feed_provider_facts_and_the_same_turn_failure() {
        use futures::StreamExt;
        use orch_agent_contract::adapter::{AdapterEvent, EventSink};
        for recorded in [
            include_str!("../../tests/fixtures/failures/plan_limit_weekly_dated.json"),
            include_str!("../../tests/fixtures/failures/plan_limit_without_a_time.json"),
        ] {
            let recorded: Json = serde_json::from_str(recorded).unwrap();
            let mut mapper = mapper();
            mapper.set_native_session(recorded["frames"][0]["session_id"].as_str().unwrap());
            mapper.begin_turn(new_id());
            let (sink, mut facts) = EventSink::channel();
            let mut all = Vec::new();
            for raw in recorded["frames"].as_array().unwrap() {
                if raw["type"] == "rate_limit_event" {
                    crate::provider::observe_frame(raw, None, &sink, mapper.clock.time()).await;
                    let AdapterEvent::Facts(update) =
                        tokio::time::timeout(std::time::Duration::from_secs(2), facts.next())
                            .await
                            .unwrap()
                            .unwrap()
                    else {
                        panic!("the existing provider reader must still receive this notice");
                    };
                    assert!(update.limit_reached.is_some());
                    let events = mapper.on_frame(&Frame::from_value(raw.clone()));
                    assert_eq!(notices(&events).len(), 1);
                    assert_eq!(
                        notices(&events)[0].payload["data"]["value"]["status"],
                        "rejected"
                    );
                    assert!(mapper.turn_active());
                    all.extend(events);
                } else {
                    all.extend(mapper.on_frame(&Frame::from_value(raw.clone())));
                }
            }
            assert!(
                all.iter()
                    .any(|event| matches!(event,SessionEvent::TurnCompleted(end)
                if end.error.as_ref().is_some_and(|error|error.code=="provider_exhausted")))
            );
        }
    }
}
