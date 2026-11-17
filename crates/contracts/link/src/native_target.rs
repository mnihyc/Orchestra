//! Immutable physical operands of an accepted native command, separate from its permission evidence.
use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The original native unit/start that produced an observation; None is explicitly untracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("required" = ["incarnation"]))]
pub struct RuntimeSource {
    #[serde(deserialize_with = "explicit_incarnation")]
    #[schemars(extend("not" = {"const":"00000000-0000-0000-0000-000000000000"}))]
    pub incarnation: Option<Id>,
}

fn explicit_incarnation<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Id>, D::Error> {
    let incarnation = Option::<Id>::deserialize(deserializer)?;
    if incarnation.is_some_and(|id| id.is_nil()) {
        return Err(serde::de::Error::custom(
            "a tracked native source is not a nil identity",
        ));
    }
    Ok(incarnation)
}
impl RuntimeSource {
    #[must_use]
    pub const fn untracked() -> Self {
        Self { incarnation: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionTarget {
    pub thread_id: Id,
    pub segment_id: Option<Id>,
    /// None records that no physical session existed when this act was accepted.
    pub opening_id: Option<Id>,
    pub opening_epoch: Option<i64>,
}
impl SessionTarget {
    #[must_use]
    pub const fn absent(thread_id: Id) -> Self {
        Self {
            thread_id,
            segment_id: None,
            opening_id: None,
            opening_epoch: None,
        }
    }
    #[must_use]
    pub fn coherent(&self) -> bool {
        match (self.opening_id, self.opening_epoch) {
            (None, None) => true,
            (Some(_), Some(epoch)) => self.segment_id.is_some() && epoch > 0,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "target",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum NativeTarget {
    Session(SessionTarget),
    Lifecycle(LifecycleTarget),
}

/// A physical lifecycle subject, independent of the operation that may control it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LifecycleSubject {
    Runtime { runtime_id: Id },
    Harness { provider_id: Id, agent_id: Id },
}
impl LifecycleSubject {
    /// An exact filename component; no caller-supplied path or resource name enters this key.
    #[must_use]
    pub fn storage_key(&self) -> String {
        match self {
            Self::Runtime { runtime_id } => format!("runtime-{runtime_id}"),
            Self::Harness {
                provider_id,
                agent_id,
            } => format!("harness-{provider_id}-{agent_id}"),
        }
    }
    fn coherent(&self) -> bool {
        match self {
            Self::Runtime { runtime_id } => !runtime_id.is_nil(),
            Self::Harness {
                provider_id,
                agent_id,
            } => !provider_id.is_nil() && !agent_id.is_nil(),
        }
    }
}

/// Original factual operands captured before a lifecycle action waits or enters a native queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("required" = ["subject", "observed", "opening", "pending", "revision"]))]
pub struct LifecycleTarget {
    pub subject: LifecycleSubject,
    /// None means no physical instance existed at acceptance; it never selects a later one.
    #[serde(deserialize_with = "explicit_incarnation")]
    pub observed: Option<Id>,
    /// The candidate identity of a genuinely new start; reuse keeps the observed physical identity.
    #[serde(deserialize_with = "explicit_incarnation")]
    pub opening: Option<Id>,
    /// Earlier admitted openings that may have performed their native effect before a reply.
    /// They belong to this original footprint, never a later source lookup or permission grant.
    pub pending: Vec<Id>,
    /// The subject's monotone accepted control order, separate from physical identity and authority.
    pub revision: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleAction {
    Start,
    Stop,
    Remove,
}
impl LifecycleTarget {
    #[must_use]
    pub fn coherent_for(&self, action: LifecycleAction) -> bool {
        self.subject.coherent()
            && self.revision > 0
            && self.observed.is_none_or(|id| !id.is_nil())
            && self.opening.is_none_or(|id| !id.is_nil())
            && (self.opening.is_some() == (action == LifecycleAction::Start))
            && self.opening.is_none_or(|id| Some(id) != self.observed)
            && self
                .pending
                .iter()
                .all(|id| !id.is_nil() && Some(*id) != self.observed && Some(*id) != self.opening)
            && self
                .pending
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.pending.len()
    }

    /// Only a recorded earlier source can be affected; a proposed next opening is not one.
    #[must_use]
    pub fn matches_source(&self, source: Option<Id>) -> bool {
        source.is_some_and(|source| Some(source) == self.observed || self.pending.contains(&source))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    Open,
    Model,
    Mcp,
    Close,
    Use,
}

/// Classifies a session command without reinterpreting its native operation as another one.
#[must_use]
pub fn session_action(kind: &str, body: &Json) -> Option<SessionAction> {
    use SessionAction::*;
    Some(match kind {
        "session.open"
        | "session.open.fenced"
        | "session.prefix.open"
        | "session.continuation.resume" => {
            let session = if kind == "session.open" {
                body
            } else {
                body.get("session")?
            };
            if session.get("transport_only").and_then(Json::as_bool) == Some(true) {
                Model
            } else {
                Open
            }
        }
        "session.close" | "session.close.fenced" | "session.continuation.close" => Close,
        "session.mcp.reload" => Mcp,
        "turn.start"
        | "turn.start.fenced"
        | "turn.steer"
        | "turn.interrupt"
        | "approval.resolve"
        | "orch.tool_result"
        | "session.background"
        | "session.background.end"
        | "session.native_actions"
        | "session.context"
        | "session.export"
        | "session.resync" => Use,
        _ => return None,
    })
}

/// The requested logical address, before an accepted act captures its physical opening.
#[must_use]
pub fn session_address(kind: &str, body: &Json) -> Option<(Id, Option<Id>)> {
    session_action(kind, body)?;
    let scoped = if matches!(
        kind,
        "session.open.fenced" | "session.prefix.open" | "session.continuation.resume"
    ) {
        body.get("session")?
    } else if kind == "session.continuation.close" {
        body.get("source")?
    } else {
        body
    };
    let id = |key: &str| {
        scoped
            .get(key)
            .and_then(Json::as_str)
            .and_then(|text| text.parse::<Id>().ok())
    };
    let thread = id("thread_id")?;
    if !std::ptr::eq(scoped, body)
        && body
            .get("thread_id")
            .and_then(Json::as_str)
            .and_then(|text| text.parse::<Id>().ok())
            != Some(thread)
    {
        return None;
    }
    Some((thread, id("segment_id")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_original_lifecycle_target_keeps_every_admitted_pending_opening_without_selecting_an_unrelated_source()
    -> Result<(), serde_json::Error> {
        let runtime = orch_types::new_id();
        let observed = orch_types::new_id();
        let pending = orch_types::new_id();
        let opening = orch_types::new_id();
        let value = serde_json::json!({"subject":{"kind":"runtime","runtime_id":runtime},
            "observed":observed,"opening":opening,"pending":[pending],"revision":3});
        let target: LifecycleTarget = serde_json::from_value(value.clone())?;
        assert!(target.coherent_for(LifecycleAction::Start));
        assert!(target.matches_source(Some(observed)));
        assert!(target.matches_source(Some(pending)));
        assert!(
            !target.matches_source(Some(opening)),
            "a proposed new opening is not an earlier physical source"
        );
        assert!(!target.matches_source(Some(orch_types::new_id())));
        assert!(
            !target.matches_source(None),
            "an unknown source is not one of these originals"
        );
        assert_eq!(serde_json::to_value(&target)?, value);
        for field in ["observed", "opening", "pending"] {
            let mut missing = value.clone();
            if let Some(object) = missing.as_object_mut() {
                object.remove(field);
            }
            assert!(
                serde_json::from_value::<LifecycleTarget>(missing).is_err(),
                "missing {field} cannot become absence"
            );
        }
        for invalid in [
            serde_json::json!([pending, pending]),
            serde_json::json!([observed]),
            serde_json::json!([opening]),
            serde_json::json!([Id::nil()]),
        ] {
            let mut invalid_target = value.clone();
            invalid_target["pending"] = invalid;
            if let Ok(target) = serde_json::from_value::<LifecycleTarget>(invalid_target) {
                assert!(!target.coherent_for(LifecycleAction::Start));
            }
        }
        Ok(())
    }

    #[test]
    fn a_stop_receipt_distinguishes_the_actual_stopped_source_from_proved_absent_originals()
    -> Result<(), serde_json::Error> {
        use crate::commands::NodeRuntimeStopResult;
        let a = orch_types::new_id();
        let b = orch_types::new_id();
        for value in [
            serde_json::json!({"outcome":"stopped","source":{"incarnation":b}}),
            serde_json::json!({"outcome":"absent","originals":[a,b]}),
        ] {
            let result: NodeRuntimeStopResult = serde_json::from_value(value.clone())?;
            assert!(result.coherent());
            assert_eq!(serde_json::to_value(result)?, value);
        }
        for value in [
            serde_json::json!({}),
            serde_json::json!({"outcome":"stopped"}),
            serde_json::json!({"outcome":"absent"}),
        ] {
            assert!(serde_json::from_value::<NodeRuntimeStopResult>(value).is_err());
        }
        let unknown: NodeRuntimeStopResult = serde_json::from_value(
            serde_json::json!({"outcome":"stopped","source":{"incarnation":null}}),
        )?;
        assert!(
            !unknown.coherent(),
            "an untracked result is not confirmed physical absence"
        );
        Ok(())
    }

    #[test]
    fn a_runtime_source_explicitly_names_its_native_start_or_says_it_is_untracked()
    -> Result<(), serde_json::Error> {
        let incarnation = orch_types::new_id();
        let tracked: RuntimeSource =
            serde_json::from_value(serde_json::json!({"incarnation":incarnation}))?;
        assert_eq!(tracked.incarnation, Some(incarnation));
        let untracked: RuntimeSource =
            serde_json::from_value(serde_json::json!({"incarnation":null}))?;
        assert_eq!(untracked, RuntimeSource::untracked());
        for missing in [
            serde_json::json!({}),
            serde_json::json!(null),
            serde_json::json!({"incarnation":orch_types::Id::nil()}),
            serde_json::json!({"incarnation":incarnation,"later":true}),
        ] {
            assert!(serde_json::from_value::<RuntimeSource>(missing).is_err());
        }
        Ok(())
    }

    #[test]
    fn a_created_units_reply_retains_its_explicit_original_native_source()
    -> Result<(), serde_json::Error> {
        let runtime = orch_types::new_id();
        let incarnation = orch_types::new_id();
        for source in [
            serde_json::json!({"incarnation":incarnation}),
            serde_json::json!({"incarnation":null}),
        ] {
            let value = serde_json::json!({"unit":{"reference":"native-container", "name":"native-unit",
                "runtime_id":runtime,"kind":"environment"},"source":source});
            let result: crate::commands::NodeRuntimeCreateResult =
                serde_json::from_value(value.clone())?;
            assert_eq!(serde_json::to_value(&result)?, value);
            let mut missing = value;
            if let Some(body) = missing.as_object_mut() {
                let _ = body.remove("source");
            }
            assert!(
                serde_json::from_value::<crate::commands::NodeRuntimeCreateResult>(missing)
                    .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn a_runtime_start_reply_keeps_its_original_source_and_explicit_reuse_fact()
    -> Result<(), serde_json::Error> {
        for incarnation in [Some(orch_types::new_id()), None] {
            for reused in [false, true] {
                let value =
                    serde_json::json!({"source":{"incarnation":incarnation},"reused":reused});
                let reply: crate::commands::NodeRuntimeStartResult =
                    serde_json::from_value(value.clone())?;
                assert_eq!(serde_json::to_value(reply)?, value);
                let mut no_source = value.clone();
                if let Some(object) = no_source.as_object_mut() {
                    object.remove("source");
                }
                assert!(
                    serde_json::from_value::<crate::commands::NodeRuntimeStartResult>(no_source)
                        .is_err()
                );
                let mut no_reuse = value;
                if let Some(object) = no_reuse.as_object_mut() {
                    object.remove("reused");
                }
                assert!(
                    serde_json::from_value::<crate::commands::NodeRuntimeStartResult>(no_reuse)
                        .is_err()
                );
            }
        }
        assert!(
            serde_json::from_value::<crate::commands::NodeRuntimeStartResult>(
                serde_json::json!({})
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn runtime_and_harness_envelopes_retain_their_original_source_and_control_order()
    -> Result<(), serde_json::Error> {
        let runtime = orch_types::new_id();
        let provider = orch_types::new_id();
        let agent = orch_types::new_id();
        let original = orch_types::new_id();
        let opening = orch_types::new_id();
        for subject in [
            serde_json::json!({"kind":"runtime","runtime_id":runtime}),
            serde_json::json!({"kind":"harness","provider_id":provider,"agent_id":agent}),
        ] {
            let value = serde_json::json!({"kind":"lifecycle","target":{
                "subject":subject,"observed":original,"opening":opening,"pending":[],"revision":7}});
            let target: NativeTarget = serde_json::from_value(value.clone())?;
            assert_eq!(serde_json::to_value(target)?, value);
            let absent = serde_json::json!({"kind":"lifecycle","target":{
                "subject":subject,"observed":null,"opening":null,"pending":[],"revision":8}});
            let target: NativeTarget = serde_json::from_value(absent.clone())?;
            assert_eq!(serde_json::to_value(target)?, absent);
        }
        Ok(())
    }

    #[test]
    fn a_wrapped_native_address_cannot_borrow_another_threads_inner_session() {
        let a = orch_types::new_id();
        let b = orch_types::new_id();
        let segment = orch_types::new_id();
        let body =
            serde_json::json!({"thread_id":a,"session":{"thread_id":b,"segment_id":segment}});
        assert_eq!(session_address("session.open.fenced", &body), None);
        let body = serde_json::json!({"thread_id":a,"source":{"thread_id":b,"segment_id":segment}});
        assert_eq!(session_address("session.continuation.close", &body), None);
    }
}
