//! One reported hook execution stays one native item; progress bodies are never projected.
use super::*;

#[derive(Debug, Clone)]
pub(super) struct HookRun {
    id: String,
    native_ref: String,
    turn: Option<TurnId>,
    stream: String,
    parent: Option<String>,
    name: Option<String>,
    event: Option<String>,
    terminal: bool,
    started_at: Option<orch_types::Time>,
    finished_at: Option<orch_types::Time>,
    value: Option<Json>,
    signature: Option<Json>,
}
impl EventMapper {
    pub(super) fn observe_hook(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) -> bool {
        let kind = frame.subtype().unwrap_or_default();
        if !matches!(kind, "hook_started" | "hook_progress" | "hook_response") {
            return false;
        }
        let Some(id) = text(frame.raw(), "hook_id") else {
            return false;
        };
        let parent = frame.parent_tool_use_id();
        let mut key = format!("{}:{id}", parent.unwrap_or("main"));
        if parent.is_none() && !self.native_changes.hooks.contains_key(&key) {
            let mut matching = self
                .native_changes
                .hooks
                .iter()
                .filter(|(_, run)| run.id == id);
            if let Some((candidate, _)) = matching.next() {
                if matching.next().is_some() {
                    return false;
                }
                key = candidate.clone();
            }
        }
        let turn = self.observed_turn(frame);
        let now = self.clock.time();
        let (stream, parent_ref) = stream_of(parent);
        let run = self
            .native_changes
            .hooks
            .entry(key)
            .or_insert_with(|| HookRun {
                id: id.clone(),
                native_ref: format!(
                    "claude-hook:{}:{}:{id}",
                    self.native_generation,
                    parent.unwrap_or("main")
                ),
                turn,
                stream,
                parent: parent_ref,
                name: None,
                event: None,
                terminal: false,
                started_at: None,
                finished_at: None,
                value: None,
                signature: None,
            });
        let terminal = kind == "hook_response";
        if run.terminal && !terminal {
            return true;
        }
        if let Some(name) = text(frame.raw(), "hook_name") {
            run.name = Some(name);
        }
        if let Some(event) = text(frame.raw(), "hook_event") {
            run.event = Some(event);
        }
        if kind == "hook_started" && run.started_at.is_none() {
            run.started_at = Some(now);
        }
        let outcome = frame.text("outcome");
        let exit = frame.get("exit_code").and_then(Json::as_i64);
        let status = if !terminal {
            "running"
        } else {
            match outcome {
                Some("success") => "completed",
                Some("error") => "failed",
                Some("cancelled") => "stopped",
                _ => "unknown",
            }
        };
        if terminal && run.finished_at.is_none() {
            run.finished_at = Some(now);
        }
        let duration = run.finished_at.zip(run.started_at).map(|(end, start)| {
            end.get()
                .signed_duration_since(start.get())
                .num_milliseconds()
                .max(0)
        });
        let value =
            json!({"status":status,"event_name":run.event,"duration_ms":duration,"exit_code":exit});
        let signature = json!([run.name, value]);
        if run.signature.as_ref() == Some(&signature) {
            return true;
        }
        let previous = run.value.replace(value.clone());
        let first = run.signature.replace(signature).is_none();
        run.terminal = terminal;
        let marker = MarkerPayload::HarnessChanged(Box::new(MarkerHarnessChanged {
            program: "Claude Code".into(),
            change: "hook".into(),
            subject: run.name.clone(),
            previous: previous
                .map(|value| UsageMeasurement::Told { value })
                .unwrap_or(UsageMeasurement::Unsupported),
            value,
            expected: UsageMeasurement::Unsupported,
            changed: !first,
            diverged: false,
            next_turn: false,
            turn_id: run.turn,
            reported_at: now,
        }));
        let Ok(payload) = serde_json::to_value(&marker) else {
            return true;
        };
        if first && let Some(turn_id) = run.turn {
            out.push(SessionEvent::ItemStarted(ItemStarted {
                turn_id,
                native_ref: run.native_ref.clone(),
                stream: run.stream.clone(),
                parent_native_ref: run.parent.clone(),
                content: ItemContent::Marker(marker),
            }));
        }
        if terminal || run.turn.is_none() {
            let item_status = match status {
                "failed" => ItemStatus::Failed,
                "stopped" => ItemStatus::Cancelled,
                _ => ItemStatus::Completed,
            };
            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                native_ref: run.native_ref.clone(),
                status: item_status,
                payload,
                output_blob: None,
            }));
        } else if !first {
            out.push(SessionEvent::ItemUpdated(ItemUpdated {
                native_ref: run.native_ref.clone(),
                payload,
            }));
        }
        true
    }
}

#[cfg(test)]
#[path = "hooks/tests.rs"]
mod tests;
