//! Native session acknowledgements keep actual delivered memberships on their existing immutable segment.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

fn opens(kind: &str) -> bool {
    matches!(
        kind,
        "session.open"
            | "session.open.fenced"
            | "session.prefix.open"
            | "session.continuation.resume"
    )
}

fn opening_body<'a>(kind: &str, body: &'a Json) -> Option<&'a Json> {
    if kind == "session.open" {
        Some(body)
    } else if opens(kind) {
        body.get("session")
    } else {
        None
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct ToolMember {
    nonce: Id,
    binding: Json,
}

/// These are native command operands. Every transport still needs current admission.
#[derive(Clone, Serialize, Deserialize)]
struct ToolAdmission {
    thread_id: Id,
    segment_id: Id,
    opening_id: Id,
    machine_id: Id,
    revision: i64,
    live: bool,
    members: BTreeMap<String, ToolMember>,
}

fn member(previous: Option<&ToolMember>, binding: Json, command: Id) -> ToolMember {
    ToolMember {
        nonce: previous
            .filter(|old| old.binding == binding)
            .map_or(command, |old| old.nonce),
        binding,
    }
}

fn mcp_members(
    previous: &BTreeMap<String, ToolMember>,
    body: &Json,
    command: Id,
) -> BTreeMap<String, ToolMember> {
    body.get("mcp")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|binding| {
            let server = id(binding, "server_id")?;
            let key = format!("mcp:{server}");
            Some((
                key.clone(),
                member(previous.get(&key), binding.clone(), command),
            ))
        })
        .collect()
}

fn tool_admission(
    old: Option<ToolAdmission>,
    body: &Json,
    command: Id,
    machine: Id,
    revision: i64,
    kind: &str,
) -> Option<ToolAdmission> {
    match kind {
        kind if opens(kind) && body.get("transport_only").and_then(Json::as_bool) != Some(true) => {
            let mut members = mcp_members(&BTreeMap::new(), body, command);
            if let Some(binding) = body.get("exec") {
                members.insert("exec".into(), member(None, binding.clone(), command));
            }
            if let Some(binding) = body.get("provider") {
                members.insert("llm".into(), member(None, binding.clone(), command));
            }
            Some(ToolAdmission {
                thread_id: id(body, "thread_id")?,
                segment_id: id(body, "segment_id")?,
                opening_id: command,
                machine_id: machine,
                revision,
                live: true,
                members,
            })
        }
        kind if opens(kind) => {
            let mut old = old?;
            old.revision = revision;
            if old.live
                && let Some(binding) = body.get("provider")
            {
                old.members.insert(
                    "llm".into(),
                    member(old.members.get("llm"), binding.clone(), command),
                );
            }
            Some(old)
        }
        "session.mcp.reload" => {
            let mut old = old?;
            old.revision = revision;
            if old.live {
                let fresh = mcp_members(&old.members, body, command);
                old.members.retain(|key, _| !key.starts_with("mcp:"));
                old.members.extend(fresh);
            }
            Some(old)
        }
        "session.close.fenced" => {
            let mut old = old?;
            old.live = false;
            old.revision = revision;
            Some(old)
        }
        _ => old,
    }
}

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
    if !opens(kind) && !matches!(kind, "session.mcp.reload" | "session.close.fenced") {
        return Ok(());
    }
    let scoped = if opens(kind) {
        let Some(session) = opening_body(kind, body) else {
            return Ok(());
        };
        session
    } else {
        body
    };
    let Some(segment) = id(scoped, "segment_id") else {
        return Ok(());
    };
    let facts:Option<Json>=sqlx::query_scalar("UPDATE segments SET delivered_resources=COALESCE(delivered_resources,'{}'::jsonb)||jsonb_build_object('admitted_revision',GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1) WHERE id=$1 RETURNING delivered_resources")
        .bind(segment).fetch_optional(&mut *conn).await?;
    let revision = facts
        .as_ref()
        .and_then(|facts| facts.get("admitted_revision"))
        .and_then(Json::as_i64);
    sqlx::query("UPDATE commands SET session_fact_revision=$2 WHERE id=$1")
        .bind(command)
        .bind(revision)
        .execute(&mut *conn)
        .await?;
    if let (Some(facts), Some(revision)) = (facts, revision) {
        let machine: Id = sqlx::query_scalar("SELECT machine_id FROM commands WHERE id=$1")
            .bind(command)
            .fetch_one(&mut *conn)
            .await?;
        let old = facts
            .get("tool_admission")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| DbError::Rule {
                constraint: "tool_session_shape".into(),
            })?;
        if let Some(admission) = tool_admission(old, scoped, command, machine, revision, kind) {
            let value = serde_json::to_value(admission).map_err(|_| DbError::Rule {
                constraint: "tool_session_shape".into(),
            })?;
            sqlx::query("UPDATE segments SET delivered_resources=jsonb_set(delivered_resources,'{tool_admission}',$2) WHERE id=$1")
                .bind(segment).bind(&value).execute(&mut *conn).await?;
            sqlx::query("UPDATE commands SET tool_session=$2 WHERE id=$1")
                .bind(command)
                .bind(value)
                .execute(conn)
                .await?;
        }
    }
    Ok(())
}

/// The accepted command's own frozen facts; a later physical opening cannot replace them.
pub async fn command_tool_session(conn: &mut Conn, command: Id) -> Result<Option<Json>, DbError> {
    Ok(
        sqlx::query_scalar::<_, Option<Json>>("SELECT tool_session FROM commands WHERE id=$1")
            .bind(command)
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

/// A failed physical opening cannot keep its tickets live or close a replacement opening.
pub async fn failed_open(
    conn: &mut Conn,
    command: Id,
    kind: &str,
    body: &Json,
) -> Result<(), DbError> {
    let Some(body) = opening_body(kind, body) else {
        return Ok(());
    };
    if body.get("transport_only").and_then(Json::as_bool) == Some(true) {
        return Ok(());
    }
    let Some(segment) = id(body, "segment_id") else {
        return Ok(());
    };
    sqlx::query("UPDATE segments SET delivered_resources=jsonb_set(jsonb_set(delivered_resources,'{tool_admission,live}','false'::jsonb),'{tool_admission,revision}',to_jsonb(GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1))||jsonb_build_object('admitted_revision',GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1) WHERE id=$1 AND delivered_resources->'tool_admission'->>'opening_id'=$2")
        .bind(segment).bind(command.to_string()).execute(conn).await?;
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
        kind if opens(kind) => {
            let Some(body) = opening_body(kind, body) else {
                return Ok(());
            };
            (body, true)
        }
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
    sqlx::query("UPDATE segments SET delivered_resources=jsonb_set(jsonb_set(COALESCE(delivered_resources,'{}'::jsonb),'{tool_admission,live}','false'::jsonb),'{tool_admission,revision}',to_jsonb(GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1))||jsonb_build_object('state','closed','admitted_revision',GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1,'revision',GREATEST(COALESCE((delivered_resources->>'revision')::bigint,0),COALESCE((delivered_resources->>'admitted_revision')::bigint,0))+1,'observed_at',$4::timestamptz) WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
        .bind(provider).bind(agent).bind(machine).bind(observed_at).execute(conn).await?;
    Ok(())
}
