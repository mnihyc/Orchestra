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

fn opening<'a>(kind: &str, body: &'a Json) -> Option<&'a Json> {
    match kind {
        "session.open" => Some(body),
        "session.open.fenced" | "session.prefix.open" | "session.continuation.resume" => {
            body.get("session")
        }
        _ => None,
    }
}

fn memberships(body: &Json, field: &str, key: &str) -> Option<Vec<Id>> {
    body.get(field)?
        .as_array()?
        .iter()
        .map(|member| id(member, key))
        .collect()
}

fn named_mcp(body: &Json, names: &std::collections::BTreeSet<String>) -> Option<Vec<Json>> {
    body.get("mcp")?
        .as_array()?
        .iter()
        .filter(|member| {
            member
                .get("name")
                .and_then(Json::as_str)
                .is_some_and(|name| names.contains(name))
        })
        .map(|member| {
            Some(json!({"name":member.get("name")?.as_str()?,"server_id":id(member,"server_id")?}))
        })
        .collect()
}

fn names(value: &Json) -> Option<std::collections::BTreeSet<String>> {
    value
        .as_array()?
        .iter()
        .map(|name| name.as_str().map(str::to_owned))
        .collect()
}

fn mcp_values(body: &Json, supplied: Option<&std::collections::BTreeSet<String>>) -> Json {
    let named = supplied.and_then(|names| named_mcp(body, names));
    let ids = named.as_ref().map(|members| {
        members
            .iter()
            .map(|member| member["server_id"].clone())
            .collect::<Vec<_>>()
    });
    json!({"mcp":ids,"mcp_names":named})
}

/// A complete launch observation is bound to its controller command, never to event arrival time.
pub async fn observe_mcp(
    conn: &mut Conn,
    segment: Id,
    payload: &Json,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    if payload.get("marker").and_then(Json::as_str) != Some("harness_changed")
        || payload.pointer("/data/change").and_then(Json::as_str) != Some("mcp_configuration")
    {
        return Ok(());
    }
    let Some(value) = payload.pointer("/data/value") else {
        return Ok(());
    };
    let Some(command) = id(value, "opening_command") else {
        return Ok(());
    };
    let Some(servers) = value.get("servers").and_then(Json::as_array) else {
        return Ok(());
    };
    let mut definitions = std::collections::BTreeMap::new();
    for server in servers {
        let (Some(name), Some(source)) = (
            server.get("name").and_then(Json::as_str),
            server.get("definition_source").and_then(Json::as_str),
        ) else {
            return Ok(());
        };
        if !matches!(source, "native" | "orch")
            || definitions
                .insert(name, source)
                .is_some_and(|old| old != source)
        {
            return Ok(());
        }
    }
    let supplied = definitions
        .into_iter()
        .filter(|(_, source)| *source == "orch")
        .map(|(name, _)| name.to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    let row:Option<(String,Json,String)>=sqlx::query_as("SELECT c.kind,c.body,c.state FROM commands c JOIN segments s ON s.id=$2 AND s.machine_id=c.machine_id WHERE c.id=$1 AND c.kind IN ('session.open','session.open.fenced','session.prefix.open','session.continuation.resume') FOR UPDATE OF c")
        .bind(command).bind(segment).fetch_optional(&mut *conn).await?;
    let Some((kind, body, state)) = row else {
        return Ok(());
    };
    if !matches!(state.as_str(), "queued" | "sent" | "accepted" | "succeeded") {
        return Ok(());
    }
    let Some(session) = opening(&kind, &body) else {
        return Ok(());
    };
    if id(session, "segment_id") != Some(segment)
        || session.get("transport_only").and_then(Json::as_bool) == Some(true)
    {
        return Ok(());
    }
    sqlx::query("UPDATE commands SET actual_mcp=$2 WHERE id=$1")
        .bind(command)
        .bind(json!(&supplied))
        .execute(&mut *conn)
        .await?;
    if state != "succeeded" {
        return Ok(());
    }
    let mut values = mcp_values(session, Some(&supplied));
    values["mcp_observed_at"] = json!(Time::new(at));
    // A transport refresh keeps the same opening; a close or a different opening fences this fact.
    let updated=sqlx::query("UPDATE segments SET delivered_resources=delivered_resources||$3::jsonb WHERE id=$1 AND delivered_resources->>'state'='open' AND delivered_resources->>'mcp_command'=$2")
        .bind(segment).bind(command.to_string()).bind(&values).execute(&mut *conn).await?.rows_affected();
    if updated == 0 {
        return Ok(());
    }
    sqlx::query("UPDATE turns SET resource_delivery=NULLIF($3::jsonb,'null'::jsonb) WHERE segment_id=$1 AND resource_opening_id=$2 AND resource_delivery IS NULL AND state IN ('starting','running')")
        .bind(segment).bind(command).bind(&values["mcp_names"]).execute(conn).await?;
    Ok(())
}

/// Captures monotone native-lane order when a new durable command is admitted.
pub async fn admit(conn: &mut Conn, command: Id, kind: &str, body: &Json) -> Result<(), DbError> {
    let scoped = opening(kind, body).unwrap_or_else(|| {
        if kind == "session.continuation.close" {
            body.get("source").unwrap_or(body)
        } else {
            body
        }
    });
    if !matches!(
        kind,
        "session.open"
            | "session.open.fenced"
            | "session.prefix.open"
            | "session.continuation.resume"
            | "session.mcp.reload"
            | "session.close.fenced"
            | "session.continuation.close"
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
        "session.open.fenced" | "session.prefix.open" | "session.continuation.resume" => {
            (body.get("session").unwrap_or(body), true)
        }
        "session.mcp.reload" => (body, false),
        "session.close.fenced" | "session.continuation.close" => {
            let scoped = if kind == "session.continuation.close" {
                body.get("source").unwrap_or(body)
            } else {
                body
            };
            if let Some(segment) = id(scoped, "segment_id") {
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
    // Replacing only the model transport does not reconfigure the native program's tools.
    let transport_only =
        patch && session.get("transport_only").and_then(Json::as_bool) == Some(true);
    let mut values = if transport_only {
        json!({})
    } else {
        let supplied = if patch {
            let receipt: Option<(Option<Json>, Option<Json>)> =
                sqlx::query_as("SELECT actual_mcp,result FROM commands WHERE id=$1")
                    .bind(command)
                    .fetch_optional(&mut *conn)
                    .await?;
            receipt.and_then(|(actual, result)| {
                actual.as_ref().and_then(names).or_else(|| {
                    result
                        .as_ref()
                        .and_then(|result| {
                            if kind == "session.prefix.open" {
                                result.pointer("/session/mcp")
                            } else {
                                result.get("mcp")
                            }
                        })
                        .and_then(names)
                })
            })
        } else {
            session
                .get("mcp")
                .and_then(Json::as_array)
                .and_then(|members| {
                    members
                        .iter()
                        .map(|member| member.get("name").and_then(Json::as_str).map(str::to_owned))
                        .collect()
                })
        };
        let mut values = mcp_values(session, supplied.as_ref());
        values["mcp_command"] = json!(command);
        values
    };
    if patch {
        if !transport_only {
            values["skills"] = json!(memberships(session, "skills", "version_id"));
        }
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
