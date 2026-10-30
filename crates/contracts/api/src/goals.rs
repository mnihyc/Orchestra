//! A thread's goal: the work and its "Done when", who set it, where it stands, what it has used, and the requests that set, hold, change and clear it.
//!
//! Goal mode is Orch's own and the same for every agent program. A person sets a goal on a thread; when a turn ends
//! and nothing else needs doing first, Orch queues the next input in that person's name, so that it passes every check
//! any input passes. The agent reports with its `goal` tool; a report of "met" is checked against the condition by the
//! place's helper model where there is one. The limits default to none and follow the settings of the place.

use crate::routes::Access::{Permission, Rule};
use crate::users::UserRef;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The most characters of a goal's objective: what a person writes about the work.
pub const GOAL_OBJECTIVE_MAX_CHARS: u32 = 4_000;
/// The most characters of a goal's condition: one verifiable sentence or a short list.
pub const GOAL_CONDITION_MAX_CHARS: u32 = 2_000;

/// Where a goal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// Orch keeps the thread working on it.
    Active,
    /// Held: a person paused it, or the agent said it is blocked.
    Paused,
    /// The condition holds.
    Met,
    /// It ended without the condition: a limit was reached, or nobody may go on.
    Stopped,
    /// A person took it away.
    Cleared,
}

/// Why a goal is paused or stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalReason {
    /// A person paused it.
    Person,
    /// A person stopped the turn that was working on it.
    Interrupted,
    /// The agent reported that only a person can go on.
    Blocked,
    /// A turn of it failed, and what recovers failed turns left it to a person.
    Failed,
    /// The turns it may use are used.
    Turns,
    /// The minutes it may use are used.
    Minutes,
    /// The credits it may use are used.
    Credits,
    /// The person who set it may no longer drive the thread or use its model.
    Access,
    /// The thread, or a place above it, was archived.
    Archived,
}

/// What an active goal waits for while it does not work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalWaiting {
    /// A question, an approval or a request waits for a person's answer.
    Approval,
    /// A plan waits for Proceed or Keep planning.
    Plan,
    /// The thread cannot go on until somebody does something.
    Blocked,
    /// A person's Stop holds the queue.
    Held,
}

/// What the agent said of its goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalReportStatus {
    /// It is working on it.
    Working,
    /// The condition holds.
    Met,
    /// Only a person can go on.
    Blocked,
}

/// What Orch made of a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalOutcome {
    /// Work going on, or a report that the agent's later words replaced.
    Noted,
    /// The condition was found to hold.
    Met,
    /// The check found that it did not yet, and said why.
    NotYet,
    /// The goal was paused for it.
    Blocked,
}

/// Who found a goal met.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalCheckedBy {
    /// The place's helper model, from the evidence.
    Helper,
    /// The agent's own report, where the place has no helper model.
    Agent,
}

/// A goal's own limits. A limit left out follows the setting of the place; zero is no limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalLimits {
    /// The most turns it may use.
    #[serde(default)]
    pub max_turns: Option<i64>,
    /// The most minutes it may be active.
    #[serde(default)]
    pub max_minutes: Option<i64>,
    /// The most credits it may use, as a decimal string.
    #[serde(default)]
    pub max_credits: Option<String>,
}

/// The limits that apply to a goal: its own where the place lets them, else the settings'. Zero is no limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GoalBudgets {
    /// The most turns; zero is no limit.
    pub max_turns: i64,
    /// The most minutes; zero is no limit.
    pub max_minutes: i64,
    /// The most credits, as a decimal string; zero is no limit.
    pub max_credits: String,
    /// Whether a place locked at least one of the settings, so that the goal's own value for it is not taken.
    pub locked: bool,
}

/// What a goal has used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GoalUsed {
    /// How many turns.
    pub turns: i64,
    /// Seconds spent active before the run that `running_since` names.
    pub elapsed_secs: i64,
    /// While it is active: when the run that is under way began, so that a screen can go on counting from it.
    pub running_since: Option<Time>,
    /// How many tokens.
    pub tokens: i64,
    /// How many credits, as a decimal string.
    pub credits: String,
}

/// One report of the agent, with what Orch made of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GoalReport {
    /// Its identifier.
    pub id: Id,
    /// The number of the turn it was made in; `null` where that turn is gone.
    pub turn: Option<i64>,
    /// What the agent said.
    pub status: GoalReportStatus,
    /// What shows it.
    pub evidence: String,
    /// What the agent does next, or needs from a person.
    pub next: Option<String>,
    /// What Orch made of it; `null` while it waits for its turn to end.
    pub outcome: Option<GoalOutcome>,
    /// Who found it met, for a report that was.
    pub checked_by: Option<GoalCheckedBy>,
    /// The helper's reason for "not yet", or why it could not check.
    pub reason: Option<String>,
    /// When it was made.
    pub at: Time,
}

/// A thread's goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadGoal {
    /// Its identifier.
    pub id: Id,
    /// The work.
    pub objective: String,
    /// Done when: what is true when it is over.
    pub condition: String,
    /// Who set it, and whose inputs the continuations are; `null` once that person is gone.
    pub set_by: Option<UserRef>,
    /// When it was set.
    pub set_at: Time,
    /// Where it stands.
    pub status: GoalStatus,
    /// Why it is paused or stopped.
    pub reason: Option<GoalReason>,
    /// The words that go with the reason: the agent's, where it said it is blocked.
    pub detail: Option<String>,
    /// While it is active and does not work: what it waits for.
    pub waiting: Option<GoalWaiting>,
    /// Whether the helper model is checking the agent's report of "met" now.
    pub checking: bool,
    /// Whether its first turn is a planning turn, so that Proceed starts the work.
    pub plan_first: bool,
    /// Whether the person who set it is told when it ends.
    pub notify_on_end: bool,
    /// Its own limits; those left out follow the settings.
    pub limits: GoalLimits,
    /// The limits that apply to it now.
    pub budgets: GoalBudgets,
    /// What it has used.
    pub used: GoalUsed,
    /// Whether a helper model checks a report of "met" in this place; without one the agent's report decides.
    pub helper: bool,
    /// Whether the thread's agent is given the goal tool, and so can report.
    pub agent_reports: bool,
    /// The agent's latest report.
    pub last_report: Option<GoalReport>,
    /// When it ended.
    pub ended_at: Option<Time>,
}

/// What the input of a goal's loop says of its goal, for the block the timeline shows in its place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GoalInputFacts {
    /// The goal.
    pub goal_id: Id,
    /// The condition it works toward.
    pub condition: String,
    /// The number of this turn of the goal, from one.
    pub turn: i64,
    /// The most turns it may use; zero is no limit.
    pub max_turns: i64,
    /// The tokens it had used before this turn.
    pub tokens: i64,
    /// Why the check found the last report not yet enough, where it did.
    pub not_yet: Option<String>,
    /// Whether this turn plans first.
    pub plan: bool,
}

/// Sets a goal on a thread: the work, its condition, and what to do beside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadGoalCreateBody {
    /// The work.
    pub objective: String,
    /// Done when.
    pub condition: String,
    /// The goal's own limits; those left out follow the settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<GoalLimits>,
    /// Whether to tell the person who sets it when it ends.
    #[serde(default)]
    pub notify_on_end: bool,
    /// Whether the first turn plans: the agent proposes a plan, and Proceed starts the work.
    #[serde(default)]
    pub plan: bool,
}

/// What a person does to a goal beside editing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalAction {
    /// Holds it.
    Pause,
    /// Sets a paused, or a stopped, goal going again.
    Resume,
}

/// Changes a goal: the words and the limits of it, and whether it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadGoalUpdateBody {
    /// A new objective.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    /// A new condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// New limits, replacing all three; one left out follows the setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<GoalLimits>,
    /// Whether to tell the person who set it when it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_on_end: Option<bool>,
    /// Pause or resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<GoalAction>,
}

/// The goal was set, in the thread's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerGoalSet {
    /// The goal.
    pub goal_id: Id,
    /// Who set it.
    pub by: UserRef,
    /// The work.
    pub objective: String,
    /// Done when.
    pub condition: String,
}

/// The goal was edited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerGoalChanged {
    /// The goal.
    pub goal_id: Id,
    /// Who edited it.
    pub by: UserRef,
    /// Done when, as it is now.
    pub condition: String,
}

/// The goal was paused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerGoalPaused {
    /// The goal.
    pub goal_id: Id,
    /// Who paused it, or stopped the turn; `null` where the agent said it is blocked.
    pub by: Option<UserRef>,
    /// Why.
    pub reason: GoalReason,
    /// The agent's words, where it said it is blocked.
    pub detail: Option<String>,
}

/// The goal was set going again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerGoalResumed {
    /// The goal.
    pub goal_id: Id,
    /// Who did.
    pub by: UserRef,
}

/// The goal was met.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerGoalMet {
    /// The goal.
    pub goal_id: Id,
    /// Done when.
    pub condition: String,
    /// What shows it, as the agent reported it.
    pub evidence: String,
    /// Who found it met.
    pub checked_by: GoalCheckedBy,
    /// Why the check could not run, where the agent's report decided for want of it.
    pub reason: Option<String>,
    /// How many turns it took.
    pub turns: i64,
}

/// The goal stopped without being met.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerGoalStopped {
    /// The goal.
    pub goal_id: Id,
    /// Why.
    pub reason: GoalReason,
    /// How many turns it had used.
    pub turns: i64,
    /// What it had used of the limit that was reached, where one was.
    pub limit: Option<String>,
}

/// The goal was taken away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerGoalCleared {
    /// The goal.
    pub goal_id: Id,
    /// Who took it away.
    pub by: UserRef,
}

crate::endpoints! {
    /// Reads the thread's goal: the one under way, else the last that ended and was not cleared.
    ThreadGoalGet: GET "/threads/{id}/goal", Permission("thread.view"), none, none, json Option<ThreadGoal>, 200;
    /// Sets a goal on a thread, in the caller's name: Orch then keeps the thread working on it, queuing each next input as the caller's.
    ThreadGoalCreate: POST "/threads/{id}/goal", Rule("thread.interact on the thread, and the caller's own use of the model it runs on"), none, json ThreadGoalCreateBody, json ThreadGoal, 201;
    /// Edits a goal, pauses it, or sets it going again.
    ThreadGoalUpdate: PATCH "/threads/{id}/goal", Permission("thread.interact"), none, json ThreadGoalUpdateBody, json ThreadGoal, 200;
    /// Takes the thread's goal away; what it did stays.
    ThreadGoalClear: DELETE "/threads/{id}/goal", Permission("thread.interact"), none, none, json ThreadGoal, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_goal_is_created_from_two_texts_and_nothing_else_is_asked() {
        let body: ThreadGoalCreateBody = serde_json::from_value(
            json!({ "objective": "Make the tests pass", "condition": "npm test exits 0" }),
        )
        .unwrap();
        assert_eq!(body.limits, None);
        assert!(!body.notify_on_end && !body.plan);
        assert!(
            serde_json::from_value::<ThreadGoalCreateBody>(
                json!({ "objective": "a", "condition": "b", "max_turns": 3 })
            )
            .is_err(),
            "limits are one object, never loose members"
        );
    }

    #[test]
    fn limits_left_out_follow_the_settings_and_zero_is_no_limit() {
        let limits: GoalLimits = serde_json::from_value(json!({ "max_turns": 0 })).unwrap();
        assert_eq!(limits.max_turns, Some(0));
        assert_eq!(limits.max_minutes, None);
        assert_eq!(limits.max_credits, None);
    }

    #[test]
    fn an_update_names_only_what_changes() {
        let body: ThreadGoalUpdateBody =
            serde_json::from_value(json!({ "action": "pause" })).unwrap();
        assert_eq!(body.action, Some(GoalAction::Pause));
        assert!(body.condition.is_none() && body.limits.is_none());
    }
}
