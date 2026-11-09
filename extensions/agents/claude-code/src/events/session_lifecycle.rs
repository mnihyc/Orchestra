//! Each actual resumed process has its own native lifecycle observation within the segment.
use super::*;

impl EventMapper {
    pub(crate) fn expect_process_resume(&mut self, resumed: bool) {
        self.resuming_process = resumed;
    }

    pub(super) fn observe_process_resume(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        if !self.resuming_process
            || frame.subtype() != Some("init")
            || !frame.get("parent_tool_use_id").is_none_or(Json::is_null)
            || self
                .native_session
                .as_deref()
                .is_some_and(|expected| frame.text("session_id") != Some(expected))
        {
            return;
        }
        self.resuming_process = false;
        self.native_marker(
            frame,
            "session_lifecycle",
            None,
            json!("resumed"),
            true,
            out,
        );
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
        mapper.set_native_session("same-session");
        mapper
    }
    fn init(session: &str, parent: Option<&str>) -> Frame {
        Frame::from_value(
            json!({"type":"system","subtype":"init","session_id":session,
            "uuid":"same-native-init-id","parent_tool_use_id":parent}),
        )
    }
    fn resumes(events: &[SessionEvent]) -> Vec<&ItemCompleted> {
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ItemCompleted(item)
                    if item.payload["marker"] == "harness_changed"
                        && item.payload["data"]["change"] == "session_lifecycle"
                        && item.payload["data"]["value"] == "resumed" =>
                {
                    Some(item)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn each_resumed_process_in_one_segment_gets_one_marker_and_its_actual_origin() {
        let mut mapper = mapper();
        mapper.process_started(true);
        assert!(resumes(&mapper.on_frame(&init("same-session", None))).is_empty());
        mapper.process_started(false);
        mapper.expect_process_resume(true);
        let turn = new_id();
        mapper.begin_turn(turn);
        let first = mapper.on_frame(&init("same-session", None));
        assert_eq!(resumes(&first).len(), 1);
        assert_eq!(resumes(&first)[0].payload["data"]["turn_id"], json!(turn));
        assert!(resumes(&mapper.on_frame(&init("same-session", None))).is_empty());
        let _ = mapper.on_frame(&Frame::from_value(
            json!({"type":"result","subtype":"success"}),
        ));
        mapper.process_started(false);
        mapper.expect_process_resume(true);
        let idle = mapper.on_frame(&init("same-session", None));
        assert_eq!(resumes(&idle).len(), 1);
        assert!(resumes(&idle)[0].payload["data"]["turn_id"].is_null());
        assert_ne!(resumes(&first)[0].native_ref, resumes(&idle)[0].native_ref);
        assert!(
            idle.iter()
                .all(|event| !matches!(event, SessionEvent::ItemStarted(_)))
        );
    }

    #[test]
    fn a_spawn_request_fork_child_or_foreign_init_is_not_a_completed_main_resume() {
        let mut mapper = mapper();
        mapper.process_started(false);
        mapper.expect_process_resume(false);
        assert!(resumes(&mapper.on_frame(&init("same-session", None))).is_empty());
        mapper.process_started(false);
        mapper.expect_process_resume(true);
        assert!(mapper.on_frame(&init("foreign-session", None)).is_empty());
        assert!(
            mapper
                .on_frame(&init("same-session", Some("child")))
                .is_empty()
        );
        assert!(mapper.resuming_process);
        // A failed launch never supplies the main init; its pending flag is reset by the next process.
        mapper.process_started(true);
        assert!(resumes(&mapper.on_frame(&init("same-session", None))).is_empty());
    }
}
