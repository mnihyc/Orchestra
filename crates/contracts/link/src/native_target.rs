//! Immutable physical operands of an accepted native command, separate from its permission evidence.
use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The original native unit/start that produced an observation; None is explicitly untracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSource {
    pub incarnation: Option<Id>,
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
pub struct LifecycleTarget {
    pub subject: LifecycleSubject,
    /// None means no physical instance existed at acceptance; it never selects a later one.
    pub observed: Option<Id>,
    /// The candidate identity of a genuinely new start; reuse keeps the observed physical identity.
    pub opening: Option<Id>,
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
