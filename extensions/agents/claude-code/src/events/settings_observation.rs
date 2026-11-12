//! Read back the native effective settings, never the raw settings sources or credentials.
use super::native_changes::ObservationScope;
use super::*;
use crate::stream::ControlOutcome;

fn safe_pin(expected: &Json, actual: &Json) -> Json {
    if (expected.is_boolean() && actual.is_boolean())
        || (expected.is_number() && actual.as_u64().is_some())
        || (expected.is_string()
            && actual.as_str().is_some_and(|word| {
                matches!(
                    word,
                    "0" | "1" | "true" | "false" | "refuse" | "hold" | "allow" | "5m" | "1h"
                )
            }))
    {
        actual.clone()
    } else {
        json!({"reported":true,"valid":false})
    }
}
impl EventMapper {
    pub(crate) fn expect_managed_pins(&mut self, pins: BTreeMap<String, Json>) {
        self.native_changes.expected_pins = pins;
    }
    /// A real adapter timeout is not a reported native value or a successful settings check.
    pub(crate) fn native_settings_unavailable(
        &self,
        request: &str,
        turn: Option<TurnId>,
    ) -> Vec<SessionEvent> {
        let marker = MarkerPayload::HarnessChanged(Box::new(
            orch_api_contract::threads::MarkerHarnessChanged {
                program: "Claude Code".into(),
                change: "managed_settings_readback".into(),
                subject: None,
                previous: orch_agent_contract::events::UsageMeasurement::Unsupported,
                value: json!({"status":"unavailable","reason":"timeout"}),
                expected: orch_agent_contract::events::UsageMeasurement::Unsupported,
                changed: false,
                diverged: false,
                next_turn: false,
                turn_id: turn,
                reported_at: self.clock.time(),
            },
        ));
        serde_json::to_value(marker)
            .ok()
            .map(|payload| {
                SessionEvent::ItemCompleted(ItemCompleted {
                    native_ref: format!(
                        "claude-settings-timeout:{}:{request}",
                        self.native_generation
                    ),
                    status: ItemStatus::Completed,
                    payload,
                    output_blob: None,
                })
            })
            .into_iter()
            .collect()
    }
    /// The requesting boundary owns this report even if the next turn starts before it arrives.
    pub(crate) fn native_settings(
        &mut self,
        frame: &Frame,
        outcome: &ControlOutcome,
        turn: Option<TurnId>,
    ) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        if let (Some(expected), Some(actual)) =
            (self.native_session.as_deref(), frame.text("session_id"))
            && expected != actual
        {
            return out;
        }
        if self.settings.raw_events {
            out.push(raw_event(FROM_HARNESS, frame.raw()));
        }
        let effective = match outcome {
            ControlOutcome::Success(value) => value.get("effective").and_then(Json::as_object),
            ControlOutcome::Error(_) => None,
        };
        let pins = self.native_changes.expected_pins.clone();
        let mut missing = Vec::new();
        let mut reported = Vec::new();
        for (name, expected) in pins {
            let actual = effective.and_then(|settings| {
                let mut keys = name.split('.');
                let mut value = settings.get(keys.next()?)?;
                for key in keys {
                    value = value.get(key)?;
                }
                Some(value)
            });
            let Some(actual) = actual else {
                missing.push(name);
                continue;
            };
            let value = safe_pin(&expected, actual);
            reported.push(name.clone());
            let key = format!("managed_setting:{name}:main");
            if value == expected && !self.native_changes.values.contains_key(&key) {
                self.native_changes.values.insert(key, value);
                continue;
            }
            let diverged = value != expected;
            let before = out.len();
            self.native_marker_scoped(
                frame,
                "managed_setting",
                Some(name.clone()),
                value,
                ObservationScope {
                    explicit_change: false,
                    turn: Some(turn),
                },
                &mut out,
            );
            if diverged && out.len() != before {
                let native_ref = out.iter().rev().find_map(|event| match event {
                    SessionEvent::ItemCompleted(item) => {
                        Some(format!("{}:diverged", item.native_ref))
                    }
                    _ => None,
                });
                if let Some(native_ref) = native_ref {
                    let message = format!(
                        "Claude Code reported a different value for the managed setting {name}."
                    );
                    if let Some(turn_id) = turn {
                        out.push(SessionEvent::ItemStarted(ItemStarted {
                            turn_id,
                            native_ref: native_ref.clone(),
                            stream: MAIN_STREAM.into(),
                            parent_native_ref: None,
                            content: ItemContent::Error(ErrorPayload {
                                details: None,
                                window: None,
                                resets_at: None,
                                code: "harness_diverged".into(),
                                message: message.clone(),
                                retryable: false,
                            }),
                        }));
                    }
                    out.push(SessionEvent::ItemCompleted(ItemCompleted {
                        native_ref,status:ItemStatus::Completed,output_blob:None,
                        payload:json!({"code":"harness_diverged","message":message,"retryable":false,
                            "details":{"turn_id":turn,"change":"managed_setting","setting":name}}),
                    }));
                }
            }
        }
        if !reported.is_empty() || !missing.is_empty() {
            self.native_marker_scoped(frame,"managed_settings_readback",None,
                json!({"status":if missing.is_empty() {"reported"} else {"incomplete"},"reported":reported,"missing":missing}),
                ObservationScope {explicit_change:false,turn:Some(turn)},&mut out);
        }
        out
    }
}

#[cfg(test)]
#[path = "settings_observation/tests.rs"]
mod tests;
