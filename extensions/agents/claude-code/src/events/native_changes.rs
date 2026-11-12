//! Safe native observations use the same marker and actual idle/current-turn ownership as other adapters.
use super::*;
use orch_agent_contract::events::{MarkerHarnessChanged, UsageMeasurement};

#[derive(Debug, Default, Clone)]
pub(super) struct NativeChanges {
    pub(super) values: BTreeMap<String, Json>,
    last: BTreeMap<String, (String, Json)>,
    serial: u64,
    hooks: BTreeMap<String, hooks::HookRun>,
    pub(super) expected_pins: BTreeMap<String, Json>,
    /// Issued child calls keep their source after the foreground tool closes. A reused
    /// reference with different owners is ambiguous until the actual process restarts.
    pub(super) parent_turns: BTreeMap<String, Option<TurnId>>,
}
pub(super) struct Observation<'a> {
    pub change: &'a str,
    pub subject: Option<String>,
    pub value: Json,
    pub explicit_change: bool,
    pub expected: UsageMeasurement<Json>,
    pub turn: Option<Option<TurnId>>,
}
fn text(value: &Json, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}
fn mcp_value(value: &Json) -> Option<Json> {
    let native = text(value, "status")?;
    let status = match native.as_str() {
        "connected" | "ready" => "ready",
        "pending" | "starting" => "starting",
        "needs-auth" => "failed",
        other => other,
    };
    let message = text(value, "message").or_else(|| text(value, "error"));
    let reason = (native == "needs-auth").then_some("reauthenticationRequired");
    Some(json!({"status":status,"native_status":native,"message":message,"failure_reason":reason}))
}
#[derive(Default)]
pub(super) struct ObservationScope {
    pub(super) explicit_change: bool,
    /// None uses the actual frame's source; Some(None) is an explicitly idle observation.
    pub(super) turn: Option<Option<TurnId>>,
}
impl EventMapper {
    /// Unknown stream methods are passive diagnostics, never evidence of model progress or a new wake.
    pub(crate) fn unknown_stream_event_name(frame: &Frame) -> Option<String> {
        if frame.kind() != FrameKind::StreamEvent {
            return None;
        }
        let name = frame.get("event")?.get("type")?.as_str()?;
        (!matches!(
            name,
            "message_start"
                | "message_delta"
                | "message_stop"
                | "content_block_start"
                | "content_block_delta"
                | "content_block_stop"
        ))
        .then(|| format!("stream_event/{name}"))
    }
    /// A malformed native line is visible without assigning idle output to the previous turn.
    pub(crate) fn protocol_error(&mut self, message: &str) -> Vec<SessionEvent> {
        self.native_changes.serial = self.native_changes.serial.saturating_add(1);
        let native_ref = format!(
            "claude-malformed:{}:{}",
            self.native_generation, self.native_changes.serial
        );
        let error = ErrorPayload {
            code: orch_agent_contract::events::turn_error::HARNESS_ERROR.into(),
            message: message.into(),
            retryable: false,
            details: None,
            window: None,
            resets_at: None,
        };
        let mut out = Vec::new();
        if let Some(turn_id) = self.turn {
            out.push(SessionEvent::ItemStarted(ItemStarted {
                turn_id,
                native_ref: native_ref.clone(),
                stream: MAIN_STREAM.into(),
                parent_native_ref: None,
                content: ItemContent::Error(error.clone()),
            }));
        }
        if let Ok(payload) = serde_json::to_value(error) {
            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                native_ref,
                status: ItemStatus::Failed,
                payload,
                output_blob: None,
            }));
        }
        out
    }
    /// An unsupported control method still gets its native error and a passive counted observation.
    pub(crate) fn unrecognized_control(&mut self, frame: &Frame) -> Vec<SessionEvent> {
        let name = frame
            .get("request")
            .and_then(|request| request.get("subtype"))
            .and_then(Json::as_str)
            .unwrap_or("unknown");
        let mut out = Vec::new();
        self.native_marker(
            frame,
            "unrecognized_event",
            Some(format!("control_request/{name}")),
            json!({"count":1}),
            false,
            &mut out,
        );
        out
    }
    pub(super) fn parent_turn(&self, parent: &str) -> Option<TurnId> {
        if parent.is_empty() {
            return None;
        }
        if let Some(turn) = self.native_changes.parent_turns.get(parent) {
            return *turn;
        }
        self.tools
            .get(parent)
            .filter(|tool| tool.generation == self.native_generation)
            .and_then(|tool| tool.origin_turn)
            .or_else(|| {
                self.background_records
                    .values()
                    .find(|task| task.native_item_ref == parent)
                    .map(|task| task.origin_turn)
            })
    }

    fn observed_turn(&self, frame: &Frame) -> Option<TurnId> {
        match frame.get("parent_tool_use_id") {
            None | Some(Json::Null) => self.turn,
            Some(Json::String(parent)) => self.parent_turn(parent),
            _ => None,
        }
    }
    pub(crate) fn native_marker(
        &mut self,
        frame: &Frame,
        change: &str,
        subject: Option<String>,
        value: Json,
        explicit_change: bool,
        out: &mut Vec<SessionEvent>,
    ) {
        self.native_marker_scoped(
            frame,
            change,
            subject,
            value,
            ObservationScope {
                explicit_change,
                turn: None,
            },
            out,
        );
    }
    pub(super) fn native_marker_scoped(
        &mut self,
        frame: &Frame,
        change: &str,
        subject: Option<String>,
        value: Json,
        scope: ObservationScope,
        out: &mut Vec<SessionEvent>,
    ) {
        let expected = (change == "managed_setting")
            .then(|| {
                subject
                    .as_ref()
                    .and_then(|name| self.native_changes.expected_pins.get(name))
            })
            .flatten()
            .cloned()
            .map(|value| UsageMeasurement::Told { value })
            .unwrap_or(UsageMeasurement::Unsupported);
        let _ = self.native_observation(
            frame,
            Observation {
                change,
                subject,
                value,
                explicit_change: scope.explicit_change,
                expected,
                turn: scope.turn,
            },
            out,
        );
    }
    pub(super) fn native_observation(
        &mut self,
        frame: &Frame,
        observation: Observation<'_>,
        out: &mut Vec<SessionEvent>,
    ) -> Option<(String, Option<TurnId>)> {
        if let (Some(expected), Some(actual)) =
            (self.native_session.as_deref(), frame.text("session_id"))
            && expected != actual
        {
            return None;
        }
        let Observation {
            change,
            subject,
            value,
            explicit_change,
            expected,
            turn,
        } = observation;
        let turn = turn.unwrap_or_else(|| self.observed_turn(frame));
        let compared_turn =
            change == "approval_mode" && matches!(&expected, UsageMeasurement::Told { .. });
        let diverged =
            matches!(&expected,UsageMeasurement::Told {value:requested} if requested != &value);
        let parent = frame.parent_tool_use_id();
        let discriminator = if change == "system_event" {
            frame.subtype().unwrap_or("system")
        } else {
            ""
        };
        let key = format!(
            "{change}:{}:{}{discriminator}",
            subject.as_deref().unwrap_or_default(),
            parent.unwrap_or("main")
        );
        let uuid = frame.uuid().unwrap_or_default();
        // A living process supplies each owned turn's approval comparison; it does not
        // claim another native change. Other observations retain their existing identity.
        let dedup_key = if compared_turn {
            format!("{key}:{turn:?}:{expected:?}")
        } else {
            key.clone()
        };
        if !uuid.is_empty()
            && self
                .native_changes
                .last
                .get(&dedup_key)
                .is_some_and(|(old, seen)| old == uuid && seen == &value)
        {
            return None;
        }
        let previous = self.native_changes.values.get(&key).cloned();
        if matches!(change, "managed_setting" | "managed_settings_readback")
            && previous.as_ref() == Some(&value)
        {
            return None;
        }
        self.native_changes.values.insert(key, value.clone());
        if !uuid.is_empty() {
            self.native_changes
                .last
                .insert(dedup_key, (uuid.to_owned(), value.clone()));
        }
        self.native_changes.serial = self.native_changes.serial.saturating_add(1);
        let mut native_ref = if uuid.is_empty() {
            format!(
                "claude-observation:{}:{}",
                self.native_generation, self.native_changes.serial
            )
        } else {
            format!(
                "claude-observation:{}:{uuid}:{change}:{}",
                self.native_generation,
                subject.as_deref().unwrap_or_default()
            )
        };
        if compared_turn {
            native_ref.push_str(&format!(":request:{}", self.native_changes.serial));
        }
        let marker = MarkerPayload::HarnessChanged(Box::new(MarkerHarnessChanged {
            program: "Claude Code".into(),
            change: change.into(),
            subject,
            changed: explicit_change || previous.as_ref().is_some_and(|old| old != &value),
            previous: previous
                .map(|value| UsageMeasurement::Told { value })
                .unwrap_or(UsageMeasurement::Unsupported),
            value,
            expected,
            diverged,
            next_turn: false,
            turn_id: turn,
            reported_at: self.clock.time(),
        }));
        if let Some(turn_id) = turn {
            let (stream, parent_ref) = stream_of(parent);
            out.push(SessionEvent::ItemStarted(ItemStarted {
                turn_id,
                native_ref: native_ref.clone(),
                stream,
                parent_native_ref: parent_ref,
                content: ItemContent::Marker(marker.clone()),
            }));
        }
        if let Ok(payload) = serde_json::to_value(marker) {
            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                native_ref: native_ref.clone(),
                status: ItemStatus::Completed,
                payload,
                output_blob: None,
            }));
        }
        Some((native_ref, turn))
    }
    pub(super) fn observe_system(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        if let (Some(expected), Some(actual)) =
            (self.native_session.as_deref(), frame.text("session_id"))
            && expected != actual
        {
            return;
        }
        let subtype = frame.subtype().unwrap_or("system");
        if subtype == "init" {
            if frame.get("parent_tool_use_id").is_none_or(Json::is_null) {
                for server in frame
                    .get("mcp_servers")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let (Some(name), Some(value)) = (text(server, "name"), mcp_value(server)) {
                        self.native_changes
                            .values
                            .insert(format!("mcp_startup:{name}:main"), value);
                    }
                }
            }
            return;
        }
        if matches!(
            subtype,
            "mcp_status" | "mcp_server_status" | "mcp_servers_changed" | "mcp_connection_status"
        ) {
            let servers = frame
                .get("mcp_servers")
                .or_else(|| frame.get("servers"))
                .and_then(Json::as_array);
            let recognized = if let Some(servers) = servers {
                let mut recognized = false;
                for server in servers {
                    recognized = self.mcp_observation(frame, server, out) || recognized;
                }
                recognized
            } else {
                self.mcp_observation(frame, frame.raw(), out)
            };
            if recognized {
                return;
            }
        }
        if self.observe_hook(frame, out) {
            return;
        }
        if subtype == "status" {
            if let Some(status) = frame.text("status") {
                let status = match status {
                    "requesting" => "working",
                    other => other,
                };
                // The existing compaction phase/marker owns that native state and its duration.
                if status != "compacting" {
                    self.native_marker(
                        frame,
                        "session_status",
                        None,
                        json!({"type":status,"active_flags":[]}),
                        false,
                        out,
                    );
                }
            }
            return;
        }
        if matches!(
            subtype,
            "compact_boundary"
                | "api_retry"
                | "permission_denied"
                | "background_tasks_changed"
                | "task_started"
                | "task_progress"
                | "task_updated"
                | "task_notification"
        ) {
            return;
        }
        // Only explicit user-facing scalar words are projected. Config, prompts, private bodies,
        // hook stdout/stderr, headers, input and opaque payloads have no marker display port.
        let mut value = serde_json::Map::new();
        value.insert("kind".into(), json!(subtype));
        for key in [
            "message",
            "title",
            "reason",
            "status",
            "level",
            "hook_name",
            "hook_event",
            "mcp_server_name",
            "outcome",
        ] {
            if let Some(word) = frame.text(key).filter(|word| !word.is_empty()) {
                value.insert(key.into(), json!(word));
            }
        }
        if let Some(exit) = frame.get("exit_code").and_then(Json::as_i64) {
            value.insert("exit_code".into(), json!(exit));
        }
        self.native_marker(frame, "system_event", None, Json::Object(value), false, out);
    }
    fn mcp_observation(
        &mut self,
        frame: &Frame,
        server: &Json,
        out: &mut Vec<SessionEvent>,
    ) -> bool {
        let Some(name) = text(server, "name")
            .or_else(|| text(server, "mcp_server_name"))
            .or_else(|| text(server, "server_name"))
        else {
            return false;
        };
        let Some(value) = mcp_value(server) else {
            return false;
        };
        let failed = value["status"] == "failed";
        let before = out.len();
        self.native_marker(
            frame,
            "mcp_startup",
            Some(name.clone()),
            value.clone(),
            false,
            out,
        );
        if failed && out.len() != before {
            let native_ref = format!(
                "claude-mcp-error:{}:{}:{name}",
                self.native_generation,
                frame.uuid().unwrap_or("status")
            );
            let message = value["message"]
                .as_str()
                .unwrap_or("The tool server could not be used.")
                .to_owned();
            let turn = self.observed_turn(frame);
            let error = ErrorPayload {
                details: None,
                window: None,
                resets_at: None,
                code: "mcp_server_failed".into(),
                message: format!("{name}: {message}"),
                retryable: false,
            };
            if let Some(turn_id) = turn {
                let (stream, parent_native_ref) = stream_of(frame.parent_tool_use_id());
                out.push(SessionEvent::ItemStarted(ItemStarted {
                    turn_id,
                    native_ref: native_ref.clone(),
                    stream,
                    parent_native_ref,
                    content: ItemContent::Error(error.clone()),
                }));
            }
            if let Ok(mut payload) = serde_json::to_value(error) {
                payload["details"] = json!({"turn_id":turn,"change":"mcp_startup","next_turn":false,"server":name,"message":message});
                out.push(SessionEvent::ItemCompleted(ItemCompleted {
                    native_ref,
                    status: ItemStatus::Completed,
                    payload,
                    output_blob: None,
                }));
            }
        }
        true
    }
}

#[cfg(test)]
#[path = "native_changes/tests.rs"]
mod tests;

#[path = "native_changes/hooks.rs"]
mod hooks;
