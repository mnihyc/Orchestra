//! Native session acknowledgements keep actual delivered memberships on their existing immutable segment.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Time};
use serde_json::json;

fn id(body: &Json, key: &str) -> Option<Id> {
    body.get(key)
        .and_then(Json::as_str)
        .and_then(|id| id.parse().ok())
}

fn memberships(body: &Json, field: &str, key: &str) -> Option<Vec<Id>> {
    body.get(field)?
        .as_array()?
        .iter()
        .map(|member| id(member, key))
        .collect()
}

/// Captures monotone native-lane order when a new durable command is admitted.
pub async fn admit(conn: &mut Conn, command: Id, kind: &str, body: &Json) -> Result<(), DbError> {
    let scoped = if kind == "session.open.fenced" {
        body.get("session").unwrap_or(body)
    } else {
        body
    };
    if !matches!(
        kind,
        "session.open" | "session.open.fenced" | "session.mcp.reload" | "session.close.fenced"
    ) {
        return Ok(());
    }
    let Some(segment) = id(scoped, "segment_id") else {
        return Ok(());
    };
    let revision:Option<i64>=sqlx::query_scalar("UPDATE segments SET delivered_resources=COALESCE(delivered_resources,'{}'::jsonb)||jsonb_build_object('admitted_revision',GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1) WHERE id=$1 RETURNING (delivered_resources->>'admitted_revision')::bigint")
        .bind(segment).fetch_optional(&mut *conn).await?;
    sqlx::query("UPDATE commands SET session_fact_revision=$2 WHERE id=$1")
        .bind(command)
        .bind(revision)
        .execute(conn)
        .await?;
    Ok(())
}

/// Projects only acknowledged opens/reloads/closes; missing legacy metadata remains unknown.
pub async fn record(
    conn: &mut Conn,
    command: Id,
    kind: &str,
    body: &Json,
    issued_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    let (session, patch) = match kind {
        "agent.stop" => {
            if let (Some(provider), Some(agent)) = (id(body, "provider_id"), id(body, "agent_id")) {
                let machine: Option<Id> =
                    sqlx::query_scalar("SELECT machine_id FROM commands WHERE id=$1")
                        .bind(command)
                        .fetch_optional(&mut *conn)
                        .await?;
                if let Some(machine) = machine {
                    retire_harness(conn, provider, agent, machine, observed_at).await?;
                }
            }
            return Ok(());
        }
        "session.open" => (body, true),
        "session.open.fenced" => (body.get("session").unwrap_or(body), true),
        "session.mcp.reload" => (body, false),
        "session.close.fenced" => {
            if let Some(segment) = id(body, "segment_id") {
                save(
                    conn,
                    segment,
                    command,
                    issued_at,
                    observed_at,
                    json!({"state":"closed"}),
                )
                .await?;
            }
            return Ok(());
        }
        "session.close" => {
            // An old close did not capture its native lane. It cannot claim a later lane closed.
            if let Some(thread) = id(body, "thread_id") {
                sqlx::query("UPDATE segments SET delivered_resources=COALESCE(delivered_resources,'{}'::jsonb)||'{\"state\":\"unknown\"}'::jsonb WHERE thread_id=$1 AND delivered_resources->>'state'='open' AND (delivered_resources->>'command_at')::timestamptz<=$2")
                    .bind(thread).bind(issued_at).execute(conn).await?;
            }
            return Ok(());
        }
        _ => return Ok(()),
    };
    let Some(segment) = id(session, "segment_id") else {
        return Ok(());
    };
    let mut values = json!({"mcp":memberships(session,"mcp","server_id")});
    if patch {
        values["skills"] = json!(memberships(session, "skills", "version_id"));
        values["state"] = json!("open");
    }
    save(conn, segment, command, issued_at, observed_at, values).await
}

async fn save(
    conn: &mut Conn,
    segment: Id,
    command: Id,
    issued_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    mut values: Json,
) -> Result<(), DbError> {
    let revision: Option<i64> =
        sqlx::query_scalar("SELECT session_fact_revision FROM commands WHERE id=$1")
            .bind(command)
            .fetch_optional(&mut *conn)
            .await?
            .flatten();
    let Some(revision) = revision else {
        return Ok(());
    };
    values["command"] = json!(command);
    values["command_at"] = json!(Time::new(issued_at));
    values["observed_at"] = json!(Time::new(observed_at));
    values["revision"] = json!(revision);
    sqlx::query("UPDATE segments SET delivered_resources=COALESCE(delivered_resources,'{}'::jsonb)||$2::jsonb WHERE id=$1 AND COALESCE((delivered_resources->>'revision')::bigint,0)<=$3")
        .bind(segment).bind(values).bind(revision).execute(conn).await?;
    Ok(())
}

/// A confirmed process stop closes its delivered sessions and fences acknowledgements admitted before it.
pub async fn retire_harness(
    conn: &mut Conn,
    provider: Id,
    agent: Id,
    machine: Id,
    observed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    // A pause still blocks new admission until its quiesce finishes. Advancing the same segment
    // revision prevents a delayed open acknowledgement from reopening the process just stopped.
    sqlx::query("UPDATE segments SET delivered_resources=COALESCE(delivered_resources,'{}'::jsonb)||jsonb_build_object('state','closed','admitted_revision',GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1,'revision',GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1,'observed_at',$4::timestamptz) WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
        .bind(provider).bind(agent).bind(machine).bind(observed_at).execute(conn).await?;
    Ok(())
}
