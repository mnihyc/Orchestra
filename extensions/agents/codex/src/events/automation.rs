//! Safe observations of native environment recovery and otherwise disabled autonomous work.
use super::*;
use orch_api_contract::threads::MarkerHarnessChanged;
use orch_api_contract::usage::UsageMeasurement;

impl Mapper {
    pub(super) fn runtime_warning(
        &mut self,
        params: &Json,
        out: &mut Vec<SessionEvent>,
        ctx: &Context<'_>,
    ) {
        let thread = text(params, "threadId");
        let Some(stream) = self.stream_of(thread) else {
            return;
        };
        let message = text(params, "message");
        if message.trim().is_empty() {
            return;
        }
        let turn = self.turn_for(thread);
        let Some((serial, data)) = self.native_settings.state(
            thread,
            "warning",
            None,
            json!({"message":message}),
            turn,
            ctx.clock.time(),
        ) else {
            return;
        };
        let parent = self
            .children
            .get(thread)
            .and_then(|child| child.parent_item.clone());
        self.emit_harness_change(serial, &stream, parent, data, None, out);
    }

    pub(super) fn turn_diff_updated(
        &mut self,
        params: &Json,
        out: &mut Vec<SessionEvent>,
        ctx: &Context<'_>,
    ) {
        let thread = text(params, "threadId");
        let native = text(params, "turnId");
        let Some(stream) = self.stream_of(thread) else {
            return;
        };
        let Some(turn) = self.reported_turn(thread, native) else {
            return;
        };
        let Some(diff) = params.get("diff").and_then(Json::as_str) else {
            return;
        };
        let hash = Hash::of(diff.as_bytes());
        if self
            .turn_diffs
            .get(thread)
            .is_some_and(|(old_turn, old_hash)| old_turn == native && *old_hash == hash)
        {
            return;
        }
        let Some(changes) = super::turn_diff::changes(diff, &self.config.directory) else {
            self.runtime_warning(&json!({"threadId":thread,"message":"Codex reported a turn diff in a format Orch could not read. Earlier changed files are kept."}), out, ctx);
            return;
        };
        self.turn_diffs.insert(thread.into(), (native.into(), hash));
        let data = MarkerHarnessChanged {
            program: "Codex".into(),
            change: "turn_diff".into(),
            subject: None,
            previous: UsageMeasurement::Unsupported,
            expected: UsageMeasurement::Unsupported,
            value: json!({"changes":changes}),
            changed: true,
            diverged: false,
            next_turn: false,
            turn_id: Some(turn),
            reported_at: ctx.clock.time(),
        };
        let reference = format!(
            "turn-diff-{}",
            Hash::of(json!([thread, native]).to_string().as_bytes())
        );
        let parent = self
            .children
            .get(thread)
            .and_then(|child| child.parent_item.clone());
        self.emit_harness_observation(reference, &stream, parent, data, None, out);
    }

    fn reported_turn(&self, thread: &str, native: &str) -> Option<TurnId> {
        if native.is_empty() {
            return None;
        }
        if thread == self.config.native_thread {
            self.turn
                .as_ref()
                .filter(|turn| turn.native == native)
                .map(|turn| turn.id)
                .or_else(|| {
                    self.last_owned_turn
                        .as_ref()
                        .filter(|(id, _)| id == native)
                        .map(|(_, turn)| *turn)
                })
        } else {
            self.children
                .get(thread)
                .filter(|child| child.native_turn.as_deref() == Some(native))
                .and_then(|child| child.turn_id)
        }
    }

    pub(super) fn automation_updated(
        &mut self,
        method: &str,
        params: &Json,
        out: &mut Vec<SessionEvent>,
        ctx: &Context<'_>,
    ) {
        let thread = text(params, "threadId");
        let Some(stream) = self.stream_of(thread) else {
            return;
        };
        let (change, subject, value, turn, forbidden) = match method {
            "thread/environment/connected" | "thread/environment/disconnected" => {
                let environment = text(params, "environmentId");
                if environment.is_empty() {
                    return;
                }
                (
                    "environment_connection",
                    Some(environment.to_owned()),
                    json!(if method.ends_with("/connected") {
                        "connected"
                    } else {
                        "disconnected"
                    }),
                    self.turn_for(thread),
                    false,
                )
            }
            "model/safetyBuffering/updated" => {
                let Some(turn) = self.reported_turn(thread, text(params, "turnId")) else {
                    return;
                };
                let Some(show) = params.get("showBufferingUi").and_then(Json::as_bool) else {
                    return;
                };
                // Visibility and a suggested faster model do not prove a reroute or activation.
                (
                    "reply_buffering",
                    None,
                    json!({"show_ui":show}),
                    Some(turn),
                    false,
                )
            }
            "thread/goal/updated" if params.get("goal").is_some_and(Json::is_object) => (
                "native_goal",
                None,
                json!(true),
                self.reported_turn(thread, text(params, "turnId")),
                true,
            ),
            "thread/goal/cleared" => (
                "native_goal",
                None,
                json!(false),
                self.turn_for(thread),
                false,
            ),
            _ => return,
        };
        if thread != self.config.native_thread && turn.is_none() {
            return;
        }
        let Some((serial, mut data)) =
            self.native_settings
                .state(thread, change, subject, value, turn, ctx.clock.time())
        else {
            return;
        };
        if change == "native_goal" {
            data.expected = UsageMeasurement::Told {
                value: json!(false),
            };
            data.diverged = forbidden;
        }
        let parent = self
            .children
            .get(thread)
            .and_then(|child| child.parent_item.clone());
        self.emit_harness_change(serial, &stream, parent, data, None, out);
    }

    pub(super) fn compaction_observed(
        &mut self,
        params: &Json,
        item: Option<&str>,
        out: &mut Vec<SessionEvent>,
        ctx: &Context<'_>,
    ) {
        let thread = text(params, "threadId");
        let native = text(params, "turnId");
        let Some(stream) = self.stream_of(thread) else {
            return;
        };
        if native.is_empty() || item == Some("") {
            return;
        }
        let turn = self.reported_turn(thread, native);
        if thread != self.config.native_thread && turn.is_none() {
            return;
        }
        if self
            .compaction_sources
            .get(thread)
            .is_some_and(|(old_turn, old_item)| {
                old_turn == native && (item.is_none() || old_item.as_deref() == item)
            })
        {
            return;
        }
        self.compaction_sources.insert(
            thread.to_owned(),
            (native.to_owned(), item.map(str::to_owned)),
        );
        if self.owns_active_turn(params) && self.pending_shortening.is_none() {
            self.note_shortening(ctx.clock.time());
        }
        // The deprecated turn-only notice has no distinct compaction id. It is a witness,
        // not a count; item identities preserve distinct compact operations in one turn.
        let native_ref = format!(
            "compaction-{}",
            Hash::of(json!([thread, native, item]).to_string().as_bytes())
        );
        let data = MarkerHarnessChanged {
            program: "Codex".into(),
            change: "context_compaction".into(),
            subject: None,
            previous: UsageMeasurement::Unsupported,
            value: json!({"duration_ms":null}),
            expected: UsageMeasurement::Unsupported,
            changed: true,
            diverged: false,
            next_turn: false,
            turn_id: turn,
            reported_at: ctx.clock.time(),
        };
        let parent = self
            .children
            .get(thread)
            .and_then(|child| child.parent_item.clone());
        self.emit_harness_observation(native_ref, &stream, parent, data, None, out);
    }
}
