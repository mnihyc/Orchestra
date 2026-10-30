//! Model-call recovery state and the controls available to a thread's readers and drivers.

use crate::routes::Access::Permission;
use crate::threads::TurnError;
use orch_provider_contract::calls::Step;
use orch_types::Time;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One durable recovery, with a known wait and the rule that caused it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallRecovery {
    /// Scheduled, switching, running, stopped or waiting.
    pub state: String,
    /// The failure being handled.
    pub failure: TurnError,
    /// The configured rule's stable name.
    pub rule_id: String,
    /// The next reserved step, absent after a guard stop.
    pub step: Option<Step>,
    /// An agent's retained effects require a visible Continue instead of reusing its input.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub continuing: bool,
    /// The attempt within the current step.
    pub attempt: u32,
    /// The known time of the next attempt; null when a person is needed.
    pub due_at: Option<Time>,
    /// A stable reason such as repeat_limit, or the blocker requiring attention.
    pub detail: Option<String>,
    /// Occurrences counted when a repeat guard stopped handling.
    pub count: Option<i64>,
    /// The guard window in seconds, where the guard stopped handling.
    pub window_secs: Option<i64>,
}

/// The current recovery, absent when no automatic handling is pending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadCallRecoveryReply {
    /// The recovery visible in the thread's live line.
    pub recovery: Option<CallRecovery>,
}

/// A driver's explicit control of a thread's automatic recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CallRecoveryAction {
    /// Run the interrupted input again under its original author's access.
    Retry,
    /// Add a visible Continue under the interrupted input author's access.
    Continue,
    /// Remove the delay and check the original author's authority again now.
    RetryNow,
    /// Stop automatic handling and leave the failed turn for a person.
    Stop,
}

/// The requested control, checked under the thread's action guard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadCallRecoveryUpdateBody {
    /// The action the person chose.
    pub action: CallRecoveryAction,
}

crate::endpoints! {
    /// Reads the thread's current automatic call recovery.
    ThreadCallRecoveryGet: GET "/threads/{id}/calls/recovery", Permission("thread.view"), none, none, json ThreadCallRecoveryReply, 200;
    /// Retries now or stops the thread's automatic call recovery.
    ThreadCallRecoveryUpdate: POST "/threads/{id}/calls/recovery", Permission("thread.interact"), none, json ThreadCallRecoveryUpdateBody, json ThreadCallRecoveryReply, 200;
}
