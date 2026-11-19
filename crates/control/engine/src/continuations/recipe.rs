//! Durable prompt-transition identity; native work never chooses a new outcome on replay.

use crate::ids;
use orch_agent_contract::history::{NativeInputPrefix, NativePrefixOpened, NativePrefixSource};
use orch_api_contract::threads::ContentPart;
use orch_types::{Hash, Id, Json, Seq, Time};
use serde::{Deserialize, Serialize};

/// A transition creates revised words or selects retained work for future execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Edit,
    Activate,
}

/// A running turn may only be waited for or interrupted by its exact accepted identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunningAction {
    Wait,
    Interrupt,
}

/// The person's explicit treatment of the turn they actually saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    pub turn_id: Id,
    pub action: RunningAction,
}

/// The selected execution and queue which admission promises not to replace silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expected {
    pub active_continuation_id: Id,
    pub execution_revision: i64,
    pub queue_revision: i64,
}

impl Expected {
    /// Revisions are positive permanent counters, not unscoped sequence numbers.
    pub fn validate(self) -> Result<Self, &'static str> {
        if self.execution_revision < 1 || self.queue_revision < 1 {
            return Err("execution and queue revisions must be positive");
        }
        Ok(self)
    }

    /// A stale wait/interrupt policy must never be applied to a newer turn.
    pub fn admits(
        self,
        current: Self,
        turn: Option<Id>,
        policy: Option<Running>,
    ) -> Result<(), &'static str> {
        if self != current {
            return Err("the selected execution or its queue changed");
        }
        match (turn, policy) {
            (None, None) => Ok(()),
            (Some(turn), Some(policy)) if policy.turn_id == turn => Ok(()),
            (Some(_), None) => Err("the running turn needs an explicit wait or interrupt choice"),
            _ => Err("the running-turn choice names a different execution"),
        }
    }
}

/// Exactly the native lane observed idle before another lane can replace it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    pub continuation_id: Id,
    pub source: NativePrefixSource,
    pub machine_id: Id,
    pub agent_id: Id,
    pub provider_id: Id,
    pub environment_id: Id,
    pub environment_generation: i32,
}

/// Every external phase has an immutable command identity derived from the operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commands {
    pub preinspect: Id,
    pub inspect: Id,
    pub close_active: Id,
    pub resume_source: Id,
    pub close_source: Id,
    pub open_prefix: Id,
    pub resume_target: Id,
    pub close_candidate: Id,
    pub restore_active: Id,
}

impl Commands {
    pub fn of(operation: Id) -> Self {
        let command = |word| ids::derived(word, &[operation.as_bytes().as_slice()]);
        Self {
            preinspect: command("continuation.preinspect"),
            inspect: command("continuation.inspect"),
            close_active: command("continuation.close_active"),
            resume_source: command("continuation.resume_source"),
            close_source: command("continuation.close_source"),
            open_prefix: command("continuation.open_prefix"),
            resume_target: command("continuation.resume_target"),
            close_candidate: command("continuation.close_candidate"),
            restore_active: command("continuation.restore_active"),
        }
    }
}

/// One flattened immutable span. Permanent ordinals/sequences can contain gaps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub owner: Id,
    pub first_turn: i32,
    pub last_turn: i32,
    pub first_seq: Seq,
    pub last_seq: Seq,
    pub frozen_at: Seq,
}

impl Span {
    /// A zero-width turn interval may still retain before-Orch/null-turn metadata.
    /// A missing native boundary is an error, never a manufactured empty prefix.
    pub fn before(&self, turn: i32, item: Seq) -> Result<Option<Self>, &'static str> {
        if turn < 1
            || item < 1
            || self.first_turn < 1
            || self.first_seq < 0
            || self.last_seq < self.first_seq
            || self.frozen_at < self.last_seq
        {
            return Err("the retained history span or its actual input boundary is invalid");
        }
        let mut span = self.clone();
        span.last_turn = span.last_turn.min(turn - 1);
        span.last_seq = span.last_seq.min(item - 1);
        Ok(
            (span.last_turn >= span.first_turn - 1 && span.first_seq <= span.last_seq)
                .then_some(span),
        )
    }
}

/// The exact request body kept before any native work. No old bypass or invocation is inherited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub kind: Kind,
    pub source_continuation_id: Id,
    pub before_input_id: Option<Id>,
    pub target_continuation_id: Option<Id>,
    pub expected: Expected,
    pub active_turn: Option<Running>,
    pub content: Option<Vec<ContentPart>>,
}

/// Stable typed intent plus actual native results; an unknown open outcome remains the same command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub operation_id: Id,
    pub thread_id: Id,
    pub actor_id: Id,
    /// Retained credential identity, reloaded under current authority before every native effect/commit.
    pub authorization_token: Option<Id>,
    /// Original token narrowing is checked as well as its currently stored grant.
    pub authorization_scope: Option<AuthorizationScope>,
    pub request: Request,
    pub request_hash: Hash,
    /// Failed-version retries reuse their immutable cut; known verified children are resumed.
    pub needs_prefix: bool,
    pub source_continuation_id: Id,
    pub target_continuation_id: Id,
    pub input_id: Option<Id>,
    pub segment_id: Id,
    pub original_lane: Option<Lane>,
    pub source_lane: Lane,
    /// Actual admitted Orch/native client-input identity, not a prompt's display ordinal.
    pub source_turn_id: Id,
    /// Existing switch wait setting, frozen when the explicit wait/interrupt was accepted.
    pub wait_minutes: i64,
    pub commands: Commands,
    pub accepted_at: Time,
    pub deadline: Time,
    #[serde(default)]
    pub prefix: Option<NativeInputPrefix>,
    #[serde(default)]
    pub opened: Option<NativePrefixOpened>,
    #[serde(default)]
    pub activation_seq: Option<Seq>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationScope {
    pub scopes: Vec<String>,
    pub subtree: Option<Id>,
}

/// Once native creation may have been issued, another operation may not replace its identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCreation {
    pub operation_id: Id,
    pub command_id: Id,
    pub machine_id: Id,
    pub segment_id: Id,
}

/// The verified native result is retained independently of execution selection and job trimming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PreparedNative {
    Prefix(NativePrefixOpened),
    Resumed(orch_link_contract::commands::SessionOpenResult),
}

impl PreparedNative {
    pub fn native(&self) -> &str {
        match self {
            Self::Prefix(opened) => &opened.session.native_session_ref,
            Self::Resumed(opened) => &opened.native_session_ref,
        }
    }
}

impl Recipe {
    /// A machine may not return another source, boundary or reused source identity as success.
    pub fn verify_opened(&self, opened: &NativePrefixOpened) -> Result<(), &'static str> {
        let expected = self
            .prefix
            .as_ref()
            .ok_or("the native prefix was not inspected durably")?;
        if &opened.prefix != expected {
            return Err("the native program applied a different input boundary");
        }
        if opened.session.native_session_ref.is_empty()
            || opened.session.native_session_ref == expected.source.native_session_ref
        {
            return Err("the native program did not create a distinct prefix session");
        }
        if expected.source != self.source_lane.source {
            return Err("the inspected prefix belongs to another source lane");
        }
        Ok(())
    }
}

/// Typed JSON is canonicalized recursively so object key order is not a new request.
pub fn request_hash(request: &Request) -> Result<Hash, serde_json::Error> {
    fn canonical(value: Json) -> Json {
        match value {
            Json::Array(values) => Json::Array(values.into_iter().map(canonical).collect()),
            Json::Object(values) => {
                let sorted: std::collections::BTreeMap<_, _> = values.into_iter().collect();
                Json::Object(
                    sorted
                        .into_iter()
                        .map(|(key, value)| (key, canonical(value)))
                        .collect(),
                )
            }
            value => value,
        }
    }
    Ok(Hash::of(&serde_json::to_vec(&canonical(
        serde_json::to_value(request)?,
    ))?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn stale_execution_queue_or_turn_cannot_authorize_an_interrupt() {
        let expected = Expected {
            active_continuation_id: new_id(),
            execution_revision: 2,
            queue_revision: 8,
        };
        let turn = new_id();
        let policy = Some(Running {
            turn_id: turn,
            action: RunningAction::Interrupt,
        });
        assert!(expected.admits(expected, Some(turn), policy).is_ok());
        assert!(
            expected
                .admits(
                    Expected {
                        queue_revision: 9,
                        ..expected
                    },
                    Some(turn),
                    policy
                )
                .is_err()
        );
        assert!(
            expected
                .admits(
                    Expected {
                        execution_revision: 3,
                        ..expected
                    },
                    Some(turn),
                    policy
                )
                .is_err()
        );
        assert!(
            expected
                .admits(
                    Expected {
                        active_continuation_id: new_id(),
                        ..expected
                    },
                    Some(turn),
                    policy
                )
                .is_err()
        );
        assert!(expected.admits(expected, Some(new_id()), policy).is_err());
        assert!(expected.admits(expected, None, policy).is_err());
        assert!(expected.admits(expected, Some(turn), None).is_err());
        assert!(expected.admits(expected, None, None).is_ok());
    }

    #[test]
    fn command_replay_retains_the_phase_identity_and_does_not_select_another_child() {
        let operation = new_id();
        let first = Commands::of(operation);
        assert_eq!(first, Commands::of(operation));
        assert_ne!(first, Commands::of(new_id()));
        let ids = [
            first.preinspect,
            first.inspect,
            first.close_active,
            first.resume_source,
            first.close_source,
            first.open_prefix,
            first.resume_target,
            first.close_candidate,
            first.restore_active,
        ];
        assert_eq!(
            ids.into_iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            ids.len()
        );
    }

    #[test]
    fn prefix_spans_clip_at_the_actual_turn_and_item_without_renumbering_holes()
    -> Result<(), Box<dyn std::error::Error>> {
        let span = Span {
            owner: new_id(),
            first_turn: 1,
            last_turn: 30,
            first_seq: 1,
            last_seq: 900,
            frozen_at: 950,
        };
        let clipped = span
            .before(12, 400)?
            .ok_or("the earlier completed prefix exists")?;
        assert_eq!(
            (
                clipped.first_turn,
                clipped.last_turn,
                clipped.first_seq,
                clipped.last_seq
            ),
            (1, 11, 1, 399)
        );
        assert_eq!(clipped.frozen_at, 950);
        assert_eq!(clipped.owner, span.owner);
        assert!(span.before(1, 1)?.is_none());
        let metadata = span
            .before(1, 40)?
            .ok_or("before-Orch and null-turn metadata remains")?;
        assert_eq!(
            (metadata.first_turn, metadata.last_turn, metadata.last_seq),
            (1, 0, 39)
        );
        assert!(span.before(0, 40).is_err());
        assert!(span.before(1, 0).is_err());
        Ok(())
    }
}
