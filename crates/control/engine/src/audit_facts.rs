//! A person's committed thread operations leave one factual audit row.

use crate::error::EngineError;
use chrono::{DateTime, Utc};
use orch_db::{
    Conn,
    audit::{self, Actor, Entry, Origin},
};
use orch_types::{Id, Json};

/// Records a person's accepted act at its actual commit, with no conversation content.
pub(crate) async fn person(
    conn: &mut Conn,
    user: Id,
    thread: Id,
    action: &str,
    details: Json,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let summary = match action {
        "approval.decided" => "An approval was decided",
        "turn.interrupted" => "A running turn was stopped",
        "input.cancelled" => "Another person's input was cancelled",
        "queue.held" => "A person's Stop held what waited in the queue",
        "queue.resumed" => "What a Stop held was resumed",
        "thread.background_end" => "Reviewed native commands were requested to end",
        "thread.moved" => "A thread was moved to another topic",
        "input.held" => "An input was held because its files cannot reach the agent",
        "input.recovered" => "An input held for its files was tried again",
        "approval.rule_added" => "An action was allowed for this thread",
        "approval.rule_revoked" => "An action allowed for this thread was taken back",
        "thread.goal_set" => "A goal was set on a thread",
        "thread.goal_changed" => "A thread's goal was edited, paused or resumed",
        "thread.goal_cleared" => "A thread's goal was taken away",
        _ => "A thread was operated",
    };
    let mut entry = Entry::new(Actor::User(user), action, "thread", Some(thread), summary);
    entry.node_id = Some(thread);
    entry.details = details;
    audit::append(conn, &entry, &Origin::default(), now).await?;
    Ok(())
}

/// Records what Orch itself did to a thread's goal: that it was met, or stopped without being met.
pub(crate) async fn system(
    conn: &mut Conn,
    thread: Id,
    action: &str,
    details: Json,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let summary = match action {
        "thread.goal_ended" => "A thread's goal ended",
        _ => "A thread was operated",
    };
    let mut entry = Entry::new(Actor::System, action, "thread", Some(thread), summary);
    entry.node_id = Some(thread);
    entry.details = details;
    audit::append(conn, &entry, &Origin::default(), now).await?;
    Ok(())
}
