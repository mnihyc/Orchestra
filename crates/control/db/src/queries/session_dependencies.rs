//! Physical admission and delivery facts survive preparation before logical history publication.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_link_contract::native_target::{
    NativeTarget, SessionAction, SessionTarget, session_action, session_address,
};
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
    opening_epoch: i64,
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
    opening_epoch: i64,
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
                opening_epoch,
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
        "session.close" | "session.close.fenced" | "session.continuation.close" => {
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
    let Some(session) = opening_body(&kind, &body) else {
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
    let updated=sqlx::query("UPDATE native_session_states SET delivered_resources=delivered_resources||$3::jsonb WHERE id=$1 AND delivered_resources->>'state'='open' AND delivered_resources->>'mcp_command'=$2")
        .bind(segment).bind(command.to_string()).bind(&values).execute(&mut *conn).await?.rows_affected();
    if updated == 0 {
        return Ok(());
    }
    sqlx::query("UPDATE turns SET resource_delivery=NULLIF($3::jsonb,'null'::jsonb) WHERE segment_id=$1 AND resource_opening_id=$2 AND resource_delivery IS NULL AND state IN ('starting','running')")
        .bind(segment).bind(command).bind(&values["mcp_names"]).execute(conn).await?;
    Ok(())
}

fn fact_target(
    thread: Id,
    segment: Option<Id>,
    facts: Option<&Json>,
) -> Result<SessionTarget, DbError> {
    let opening = facts.and_then(|value| id(value, "opening_id"));
    let epoch = facts
        .and_then(|value| value.get("opening_epoch"))
        .and_then(Json::as_i64);
    let target = SessionTarget {
        thread_id: thread,
        segment_id: segment,
        opening_id: opening,
        opening_epoch: epoch,
    };
    if !target.coherent() {
        return Err(DbError::Rule {
            constraint: "native_session_target".into(),
        });
    }
    Ok(target)
}

/// Captures a physical operand now, including a known absence, before a producer waits.
pub async fn target(
    conn: &mut Conn,
    thread: Id,
    segment: Option<Id>,
) -> Result<NativeTarget, DbError> {
    let selected:Option<(Id,Option<Json>)>=match segment {
        Some(segment)=>sqlx::query_as("SELECT id,delivered_resources->'tool_admission' FROM native_session_states WHERE id=$1 AND thread_id=$2")
            .bind(segment).bind(thread).fetch_optional(&mut *conn).await?,
        None=>sqlx::query_as("SELECT s.id,s.delivered_resources->'tool_admission' FROM native_session_states s JOIN threads t ON t.node_id=s.thread_id WHERE s.thread_id=$1 AND (s.delivered_resources#>>'{tool_admission,opening_epoch}')::bigint=t.native_opening_epoch LIMIT 1")
            .bind(thread).fetch_optional(&mut *conn).await?,
    };
    if selected.is_none()
        && let Some(segment) = segment
    {
        let owner: Option<Id> = sqlx::query_scalar("SELECT thread_id FROM segments WHERE id=$1")
            .bind(segment)
            .fetch_optional(&mut *conn)
            .await?;
        if owner.is_some_and(|owner| owner != thread) {
            return Err(DbError::Rule {
                constraint: "native_session_address".into(),
            });
        }
    }
    let (segment, facts) =
        selected.map_or((segment, None), |(segment, facts)| (Some(segment), facts));
    Ok(NativeTarget::Session(fact_target(
        thread,
        segment,
        facts.as_ref(),
    )?))
}

async fn freeze_target(
    conn: &mut Conn,
    command: Id,
    facts: Option<Json>,
    target: &NativeTarget,
) -> Result<(), DbError> {
    let mut facts = facts.unwrap_or_else(|| json!({}));
    facts["native_target"] = serde_json::to_value(target).map_err(|_| DbError::Rule {
        constraint: "native_session_target".into(),
    })?;
    sqlx::query("UPDATE commands SET tool_session=$2 WHERE id=$1")
        .bind(command)
        .bind(facts)
        .execute(conn)
        .await?;
    Ok(())
}

async fn capture_target(
    conn: &mut Conn,
    thread: Id,
    segment: Option<Id>,
    provided: Option<&NativeTarget>,
) -> Result<SessionTarget, DbError> {
    match provided {
        Some(NativeTarget::Session(target)) => {
            if !target.coherent()
                || target.thread_id != thread
                || segment.is_some_and(|segment| target.segment_id != Some(segment))
            {
                return Err(DbError::Rule {
                    constraint: "native_session_target".into(),
                });
            }
            Ok(target.clone())
        }
        Some(NativeTarget::Lifecycle(_)) => Err(DbError::Rule {
            constraint: "native_session_target".into(),
        }),
        None => {
            let NativeTarget::Session(target) = target(conn, thread, segment).await? else {
                return Err(DbError::Rule {
                    constraint: "native_session_target".into(),
                });
            };
            Ok(target)
        }
    }
}

#[derive(sqlx::FromRow)]
struct UsageSource {
    thread_id: Id,
    source_machine: Option<Id>,
    recipient: Id,
    admission: Option<Json>,
}

async fn freeze_usage(
    conn: &mut Conn,
    command: Id,
    thread: Id,
    segment: Option<Id>,
    provided: Option<&NativeTarget>,
) -> Result<(), DbError> {
    let captured = capture_target(conn, thread, segment, provided).await?;
    let Some(segment) = segment.or(captured.segment_id) else {
        return freeze_target(conn, command, None, &NativeTarget::Session(captured)).await;
    };
    let source: Option<UsageSource> = sqlx::query_as(
        "SELECT s.thread_id,s.machine_id AS source_machine,c.machine_id AS recipient,
         s.delivered_resources->'tool_admission' AS admission
         FROM native_session_states s JOIN commands c ON c.id=$2 WHERE s.id=$1",
    )
    .bind(segment)
    .bind(command)
    .fetch_optional(&mut *conn)
    .await?;
    let admission = match source {
        Some(source) if source.thread_id != thread => {
            return Err(DbError::Rule {
                constraint: "native_session_address".into(),
            });
        }
        Some(source)
            if source
                .source_machine
                .is_none_or(|machine| machine == source.recipient) =>
        {
            source.admission.filter(|value| !value.is_null())
        }
        _ => None,
    };
    let actual = fact_target(thread, Some(segment), admission.as_ref())?;
    let admission = admission.filter(|_| captured == actual);
    freeze_target(conn, command, admission, &NativeTarget::Session(captured)).await
}

/// Retains the exact native binding before a fork/import opens its unpublished segment.
/// The caller owns its short source-authorized transaction; this grants no operation.
pub async fn prepare(
    conn: &mut Conn,
    segment: &super::threads::SegmentRow,
    machine: Id,
) -> Result<(), DbError> {
    sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
        .bind(segment.thread_id)
        .fetch_one(&mut *conn)
        .await?;
    let owner: Option<Id> = sqlx::query_scalar("SELECT thread_id FROM segments WHERE id=$1")
        .bind(segment.id)
        .fetch_optional(&mut *conn)
        .await?;
    if owner.is_some_and(|owner| owner != segment.thread_id) {
        return Err(DbError::Rule {
            constraint: "native_session_address".into(),
        });
    }
    sqlx::query("INSERT INTO native_session_states(id,thread_id,machine_id,environment_id,env_generation,provider_id,agent_id,executor,tool_set,ended_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(id) DO NOTHING")
        .bind(segment.id).bind(segment.thread_id).bind(machine).bind(segment.environment_id)
        .bind(segment.env_generation).bind(segment.provider_id).bind(segment.agent_id)
        .bind(&segment.executor).bind(&segment.tool_set).bind(segment.ended_at)
        .execute(&mut *conn).await?;
    let same: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_session_states WHERE id=$1 AND thread_id=$2 AND machine_id=$3 AND environment_id=$4 AND env_generation=$5 AND provider_id=$6 AND agent_id=$7 AND executor IS NOT DISTINCT FROM $8 AND tool_set IS NOT DISTINCT FROM $9 AND ended_at IS NOT DISTINCT FROM $10)")
        .bind(segment.id).bind(segment.thread_id).bind(machine).bind(segment.environment_id)
        .bind(segment.env_generation).bind(segment.provider_id).bind(segment.agent_id)
        .bind(&segment.executor).bind(&segment.tool_set).bind(segment.ended_at)
        .fetch_one(conn).await?;
    if !same {
        return Err(DbError::Rule {
            constraint: "native_session_address".into(),
        });
    }
    Ok(())
}

/// Order admission against the original unit's Stop before taking its session row.
/// These shared factual locks grant no operation and do not lock other units on the machine.
async fn lock_runtime_bindings(conn: &mut Conn, thread: Id, segment: Id) -> Result<(), DbError> {
    sqlx::query_scalar::<_, Id>(
        "WITH binding AS (
           SELECT environment_id,provider_id FROM native_session_states WHERE id=$1 AND thread_id=$2
           UNION ALL
           SELECT environment_id,provider_id FROM segments WHERE id=$1 AND thread_id=$2
             AND NOT EXISTS(SELECT 1 FROM native_session_states WHERE id=$1 AND thread_id=$2)
         )
         SELECT r.id FROM runtimes r
         WHERE r.successor_of IS NULL AND r.state NOT IN ('retired','removed')
           AND ((r.kind='environment' AND r.environment_id IN (SELECT environment_id FROM binding))
             OR (r.kind='provider' AND r.provider_id IN (SELECT provider_id FROM binding)))
         ORDER BY r.id FOR SHARE OF r",
    )
    .bind(segment)
    .bind(thread)
    .fetch_all(conn)
    .await?;
    Ok(())
}

/// Captures the accepted command's own physical operand and monotone native-lane order.
pub async fn admit(
    conn: &mut Conn,
    command: Id,
    kind: &str,
    body: &Json,
    provided: Option<&NativeTarget>,
) -> Result<(), DbError> {
    let Some(action) = session_action(kind, body) else {
        if provided.is_some() {
            return Err(DbError::Rule {
                constraint: "native_target_command_kind".into(),
            });
        }
        return Ok(());
    };
    let Some((thread, addressed_segment)) = session_address(kind, body) else {
        return Err(DbError::Rule {
            constraint: "native_session_address".into(),
        });
    };
    // Using an existing opening changes no admission. Its committed facts can be
    // captured while its controller holds the thread across an exact native End.
    // The native effect still checks this frozen opening and member set.
    if action == SessionAction::Use && !matches!(kind, "turn.start" | "turn.start.fenced") {
        return freeze_usage(conn, command, thread, addressed_segment, provided).await;
    }
    // Different logical segments still compete for this thread's one current physical slot.
    let present: Option<i64> =
        sqlx::query_scalar("SELECT native_opening_epoch FROM threads WHERE node_id=$1 FOR UPDATE")
            .bind(thread)
            .fetch_optional(&mut *conn)
            .await?;
    if present.is_none() && action == SessionAction::Open {
        return Err(DbError::Rule {
            constraint: "native_session_thread".into(),
        });
    }
    let captured = capture_target(conn, thread, addressed_segment, provided).await?;
    let segment = addressed_segment.or(captured.segment_id);
    let Some(segment) = segment else {
        return freeze_target(conn, command, None, &NativeTarget::Session(captured)).await;
    };
    if matches!(
        action,
        SessionAction::Open | SessionAction::Model | SessionAction::Mcp
    ) {
        lock_runtime_bindings(conn, thread, segment).await?;
    }
    if action == SessionAction::Open {
        sqlx::query("INSERT INTO native_session_states(id,thread_id,machine_id,environment_id,env_generation,provider_id,agent_id,executor,tool_set,ended_at,delivered_resources) SELECT s.id,s.thread_id,COALESCE(s.machine_id,c.machine_id),s.environment_id,s.env_generation,s.provider_id,s.agent_id,s.executor,s.tool_set,s.ended_at,s.delivered_resources FROM segments s JOIN commands c ON c.id=$3 WHERE s.id=$1 AND s.thread_id=$2 ON CONFLICT(id) DO NOTHING")
            .bind(segment).bind(thread).bind(command).execute(&mut *conn).await?;
    }
    let owner: Option<Id> = sqlx::query_scalar("SELECT s.thread_id FROM native_session_states s JOIN commands c ON c.id=$2 WHERE s.id=$1 AND (s.machine_id IS NULL OR s.machine_id=c.machine_id)")
        .bind(segment)
        .bind(command)
        .fetch_optional(&mut *conn)
        .await?;
    if owner.is_some_and(|owner| owner != thread)
        || owner.is_none() && action == SessionAction::Open
    {
        return Err(DbError::Rule {
            constraint: "native_session_address".into(),
        });
    }
    let facts: Option<Json> = sqlx::query_scalar(
        "SELECT delivered_resources FROM native_session_states WHERE id=$1 AND thread_id=$2 FOR UPDATE",
    )
    .bind(segment)
    .bind(thread)
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    let old = facts
        .as_ref()
        .and_then(|facts| facts.get("tool_admission"))
        .filter(|value| !value.is_null())
        .cloned();
    let actual = fact_target(thread, Some(segment), old.as_ref())?;
    let matches = provided.is_none() || captured == actual;
    if action == SessionAction::Use
        && matches
        && captured.opening_id.is_some()
        && captured.opening_epoch == present
        && matches!(kind, "turn.start" | "turn.start.fenced")
    {
        let native = body.get("turn").unwrap_or(body);
        if let Some(turn) = id(native, "turn_id") {
            let model_nonce = old
                .as_ref()
                .and_then(|facts| facts.pointer("/members/llm"))
                .and_then(|member| id(member, "nonce"));
            sqlx::query("UPDATE turns SET native_opening_id=$2,native_model_nonce=$5,native_end_confirmed=false WHERE id=$1 AND thread_id=$3 AND segment_id=$4 AND state IN ('starting','running')")
                .bind(turn).bind(captured.opening_id).bind(thread).bind(segment).bind(model_nonce)
                .execute(&mut *conn).await?;
        }
    }
    if action == SessionAction::Use
        || !matches
        || owner.is_none()
        || action != SessionAction::Open && captured.opening_id.is_none()
    {
        return freeze_target(
            conn,
            command,
            if matches && owner.is_some() {
                old
            } else {
                None
            },
            &NativeTarget::Session(captured),
        )
        .await;
    }
    let epoch = if action == SessionAction::Open {
        sqlx::query_scalar::<_, i64>("SELECT nextval('native_session_openings')")
            .fetch_one(&mut *conn)
            .await?
    } else {
        actual.opening_epoch.unwrap_or_default()
    };
    if action == SessionAction::Open {
        sqlx::query("UPDATE threads SET native_opening_epoch=$2 WHERE node_id=$1")
            .bind(thread)
            .bind(epoch)
            .execute(&mut *conn)
            .await?;
    }
    let facts:Option<Json>=sqlx::query_scalar("UPDATE native_session_states SET delivered_resources=COALESCE(delivered_resources,'{}'::jsonb)||jsonb_build_object('admitted_revision',GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1) WHERE id=$1 RETURNING delivered_resources")
        .bind(segment).fetch_optional(&mut *conn).await?;
    let Some(facts) = facts else {
        return freeze_target(conn, command, None, &NativeTarget::Session(captured)).await;
    };
    let revision = facts
        .get("admitted_revision")
        .and_then(Json::as_i64)
        .ok_or_else(|| DbError::Rule {
            constraint: "tool_session_shape".into(),
        })?;
    sqlx::query("UPDATE commands SET session_fact_revision=$2 WHERE id=$1")
        .bind(command)
        .bind(revision)
        .execute(&mut *conn)
        .await?;
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
    let scoped = opening_body(kind, body).unwrap_or(body);
    if let Some(admission) = tool_admission(old, scoped, command, machine, revision, epoch, kind) {
        if action == SessionAction::Open {
            // Source facts outlive queue collection; no later current-slot lookup supplies them.
            sqlx::query("INSERT INTO native_opening_sources(opening_id,thread_id,segment_id,machine_id,opening_epoch) VALUES($1,$2,$3,$4,$5)")
                .bind(admission.opening_id).bind(thread).bind(segment).bind(machine).bind(epoch)
                .execute(&mut *conn).await?;
        }
        let target = NativeTarget::Session(SessionTarget {
            thread_id: thread,
            segment_id: Some(segment),
            opening_id: Some(admission.opening_id),
            opening_epoch: Some(admission.opening_epoch),
        });
        let value = serde_json::to_value(admission).map_err(|_| DbError::Rule {
            constraint: "tool_session_shape".into(),
        })?;
        sqlx::query("UPDATE native_session_states SET delivered_resources=jsonb_set(delivered_resources,'{tool_admission}',$2) WHERE id=$1").bind(segment).bind(&value).execute(&mut *conn).await?;
        freeze_target(conn, command, Some(value), &target).await?;
    }
    Ok(())
}

/// The physical operand already frozen beside this command, never a later thread lookup.
pub async fn command_native_target(
    conn: &mut Conn,
    command: Id,
) -> Result<Option<NativeTarget>, DbError> {
    let facts = command_tool_session(conn, command).await?;
    facts
        .and_then(|value| value.get("native_target").cloned())
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| DbError::Rule {
            constraint: "native_session_target".into(),
        })
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
    sqlx::query("UPDATE native_session_states SET delivered_resources=jsonb_set(jsonb_set(delivered_resources,'{tool_admission,live}','false'::jsonb),'{tool_admission,revision}',to_jsonb(GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1))||jsonb_build_object('admitted_revision',GREATEST(COALESCE((delivered_resources->>'admitted_revision')::bigint,0),COALESCE((delivered_resources->>'revision')::bigint,0))+1) WHERE id=$1 AND delivered_resources->'tool_admission'->>'opening_id'=$2")
        .bind(segment).bind(command.to_string()).execute(conn).await?;
    Ok(())
}

/// Projects actual delivery only for the physical opening that produced its receipt.
pub async fn record(
    conn: &mut Conn,
    command: Id,
    kind: &str,
    body: &Json,
    issued_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    if kind == "agent.stop" {
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
    let Some(action) = session_action(kind, body) else {
        return Ok(());
    };
    if matches!(action, SessionAction::Model | SessionAction::Use) {
        return Ok(());
    }
    let Some(NativeTarget::Session(target)) = command_native_target(conn, command).await? else {
        return Ok(());
    };
    let Some(opening) = target.opening_id else {
        return Ok(());
    };
    let original: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_opening_sources opening JOIN commands receipt ON receipt.id=$2 WHERE opening.opening_id=$1 AND opening.machine_id=receipt.machine_id AND opening.thread_id=$3 AND opening.segment_id=$4 AND opening.opening_epoch=$5)")
        .bind(opening).bind(command).bind(target.thread_id).bind(target.segment_id).bind(target.opening_epoch)
        .fetch_one(&mut *conn).await?;
    if !original {
        return Ok(());
    }
    if action == SessionAction::Close {
        return save(
            conn,
            command,
            issued_at,
            observed_at,
            json!({"state":"closed"}),
            &target,
        )
        .await;
    }
    let patch = opens(kind);
    let session = opening_body(kind, body).unwrap_or(body);
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
    if patch && !transport_only {
        values["skills"] = json!(memberships(session, "skills", "version_id"));
        values["state"] = json!("open");
    }
    save(conn, command, issued_at, observed_at, values, &target).await
}

async fn save(
    conn: &mut Conn,
    command: Id,
    issued_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    mut values: Json,
    target: &SessionTarget,
) -> Result<(), DbError> {
    let (Some(segment), Some(opening), Some(epoch)) =
        (target.segment_id, target.opening_id, target.opening_epoch)
    else {
        return Ok(());
    };
    let revision: Option<i64> =
        sqlx::query_scalar("SELECT session_fact_revision FROM commands WHERE id=$1")
            .bind(command)
            .fetch_optional(&mut *conn)
            .await?
            .flatten();
    let terminal = values.get("state").and_then(Json::as_str) == Some("closed");
    if revision.is_none() && !terminal {
        return Ok(());
    }
    let before: Option<Json> = sqlx::query_scalar(
        "SELECT delivered_resources FROM native_session_states WHERE id=$1 FOR UPDATE",
    )
    .bind(segment)
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    let before = before.unwrap_or_else(|| json!({}));
    let previous = before
        .get("delivered_opening_epoch")
        .and_then(Json::as_i64)
        .unwrap_or_default();
    let previous_revision = before
        .get("revision")
        .and_then(Json::as_i64)
        .unwrap_or_default();
    if previous > epoch
        || previous == epoch
            && before.get("state").and_then(Json::as_str) == Some("closed")
            && !terminal
        || previous == epoch && !terminal && previous_revision > revision.unwrap_or_default()
    {
        return Ok(());
    }
    if previous < epoch {
        let mut base =
            json!({"mcp":null,"skills":null,"state":if terminal {"closed"} else {"open"}});
        // A later same-opening reload proves the ordinary open was delivered even when its reply lags.
        let source: Option<(String, Json)> =
            sqlx::query_as("SELECT kind,body FROM commands WHERE id=$1")
                .bind(opening)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some((kind, body)) = source
            && let Some(session) = opening_body(&kind, &body)
        {
            base["mcp"] = json!(memberships(session, "mcp", "server_id"));
            base["skills"] = json!(memberships(session, "skills", "version_id"));
        }
        if let (Some(base), Some(patch)) = (base.as_object_mut(), values.as_object()) {
            base.extend(patch.clone());
        }
        values = base;
    }
    values["delivered_opening_id"] = json!(opening);
    values["delivered_opening_epoch"] = json!(epoch);
    values["command"] = json!(command);
    values["command_at"] = json!(Time::new(issued_at));
    values["observed_at"] = json!(Time::new(observed_at));
    values["revision"] = json!(if previous == epoch {
        previous_revision.max(revision.unwrap_or_default())
    } else {
        revision.unwrap_or_default()
    });
    sqlx::query("UPDATE native_session_states SET delivered_resources=COALESCE(delivered_resources,'{}'::jsonb)||$2::jsonb WHERE id=$1")
        .bind(segment).bind(values).execute(conn).await?;
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
    // The harness may stop before an admitted opening replies. Record which opening closed
    // so a late delivery cannot reopen it and a later, newly admitted opening can succeed.
    sqlx::query(
        "UPDATE native_session_states
         SET delivered_resources = orch_tool_close(COALESCE(delivered_resources,'{}'::jsonb))
            || jsonb_strip_nulls(jsonb_build_object(
                 'delivered_opening_id', delivered_resources#>'{tool_admission,opening_id}',
                 'delivered_opening_epoch', delivered_resources#>'{tool_admission,opening_epoch}'))
            || jsonb_build_object('state','closed','revision',orch_tool_revision(delivered_resources),
                                  'observed_at',$4::timestamptz)
         WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3",
    )
    .bind(provider)
    .bind(agent)
    .bind(machine)
    .bind(observed_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The currently admitted physical opening; a logical segment can have several successive openings.
pub async fn current_opening(conn: &mut Conn, segment: Id) -> Result<Option<Id>, DbError> {
    let value:Option<String>=sqlx::query_scalar("SELECT s.delivered_resources#>>'{tool_admission,opening_id}' FROM native_session_states s JOIN threads t ON t.node_id=s.thread_id WHERE s.id=$1 AND s.delivered_resources#>>'{tool_admission,live}'='true' AND (s.delivered_resources#>>'{tool_admission,opening_epoch}')::bigint=t.native_opening_epoch")
        .bind(segment).fetch_optional(conn).await?.flatten();
    value
        .map(|text| {
            text.parse().map_err(|_| DbError::Rule {
                constraint: "tool_session_shape".into(),
            })
        })
        .transpose()
}

/// Compares the observed original model member with the current facts; it grants no operation.
pub async fn model_member_matches(
    conn: &mut Conn,
    segment: Id,
    opening: Option<Id>,
    turn: Option<Id>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_session_states s JOIN threads t ON t.node_id=s.thread_id
        LEFT JOIN turns u ON u.id=$3 AND u.thread_id=s.thread_id AND u.segment_id=s.id
        WHERE s.id=$1 AND s.delivered_resources#>>'{tool_admission,live}'='true'
        AND s.delivered_resources#>>'{tool_admission,opening_id}'=$2::uuid::text
        AND (s.delivered_resources#>>'{tool_admission,opening_epoch}')::bigint=t.native_opening_epoch
        AND CASE WHEN $3::uuid IS NULL THEN (s.delivered_resources#>>'{tool_admission,members,llm,nonce}')::uuid=$2
          ELSE u.native_opening_id=$2 AND u.native_model_nonce=(s.delivered_resources#>>'{tool_admission,members,llm,nonce}')::uuid END)")
        .bind(segment).bind(opening).bind(turn).fetch_one(conn).await?)
}

/// The last admitted physical opening, including one whose authority is already withdrawn.
/// The caller keeps its source row shared until its report transaction commits.
pub async fn recorded_opening(conn: &mut Conn, segment: Id) -> Result<Option<Id>, DbError> {
    let value: Option<String> = sqlx::query_scalar(
        "SELECT delivered_resources#>>'{tool_admission,opening_id}' FROM native_session_states WHERE id=$1 FOR SHARE",
    )
    .bind(segment)
    .fetch_optional(conn)
    .await?
    .flatten();
    value
        .map(|text| {
            text.parse().map_err(|_| DbError::Rule {
                constraint: "tool_session_shape".into(),
            })
        })
        .transpose()
}

/// A physical loss closes only its own facts. The answer says whether its admission was still live.
pub async fn lost(
    conn: &mut Conn,
    segment: Id,
    opening: Id,
    at: DateTime<Utc>,
) -> Result<Option<bool>, DbError> {
    let facts:Option<Json>=sqlx::query_scalar("SELECT delivered_resources FROM native_session_states WHERE id=$1 AND delivered_resources#>>'{tool_admission,opening_id}'=$2 FOR UPDATE")
        .bind(segment).bind(opening.to_string()).fetch_optional(&mut *conn).await?;
    let Some(facts) = facts else {
        return Ok(None);
    };
    let latest:Option<i64>=sqlx::query_scalar("SELECT t.native_opening_epoch FROM native_session_states s JOIN threads t ON t.node_id=s.thread_id WHERE s.id=$1").bind(segment).fetch_optional(&mut *conn).await?;
    let was_live = facts
        .pointer("/tool_admission/live")
        .and_then(Json::as_bool)
        == Some(true)
        && facts
            .pointer("/tool_admission/opening_epoch")
            .and_then(Json::as_i64)
            == latest;
    if was_live || facts.get("state").and_then(Json::as_str) != Some("closed") {
        sqlx::query("UPDATE native_session_states SET delivered_resources=orch_tool_close(delivered_resources)||jsonb_build_object('state','closed','delivered_opening_id',delivered_resources#>>'{tool_admission,opening_id}','delivered_opening_epoch',(delivered_resources#>>'{tool_admission,opening_epoch}')::bigint,'revision',orch_tool_revision(delivered_resources),'observed_at',$3::timestamptz) WHERE id=$1 AND delivered_resources#>>'{tool_admission,opening_id}'=$2")
            .bind(segment).bind(opening.to_string()).bind(at).execute(conn).await?;
    }
    Ok(Some(was_live))
}

/// Admits a live-only model transport update and returns its immutable signing/target operands.
/// The caller owns the transaction; no command body or result is stored for this request.
pub async fn admit_transport(
    conn: &mut Conn,
    observation: Id,
    machine: Id,
    body: &Json,
) -> Result<Json, DbError> {
    if session_action("session.open", body) != Some(SessionAction::Model) {
        return Err(DbError::Rule {
            constraint: "native_transport_only".into(),
        });
    }
    let (thread, segment) = session_address("session.open", body).ok_or_else(|| DbError::Rule {
        constraint: "native_session_address".into(),
    })?;
    let segment = segment.ok_or_else(|| DbError::Rule {
        constraint: "native_session_address".into(),
    })?;
    sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
        .bind(thread)
        .fetch_one(&mut *conn)
        .await?;
    let facts: Option<Json> = sqlx::query_scalar(
        "SELECT delivered_resources FROM native_session_states WHERE id=$1 AND thread_id=$2 FOR UPDATE",
    )
    .bind(segment)
    .bind(thread)
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    let facts = facts.ok_or_else(|| DbError::Rule {
        constraint: "native_session_missing".into(),
    })?;
    let old: ToolAdmission =
        serde_json::from_value(facts.get("tool_admission").cloned().unwrap_or(Json::Null))
            .map_err(|_| DbError::Rule {
                constraint: "native_session_missing".into(),
            })?;
    let NativeTarget::Session(current) = target(conn, thread, None).await? else {
        return Err(DbError::Rule {
            constraint: "native_session_target".into(),
        });
    };
    if !old.live || old.machine_id != machine || current.opening_id != Some(old.opening_id) {
        return Err(DbError::Rule {
            constraint: "native_session_changed".into(),
        });
    }
    let revision = facts
        .get("admitted_revision")
        .and_then(Json::as_i64)
        .unwrap_or_default()
        .max(
            facts
                .get("revision")
                .and_then(Json::as_i64)
                .unwrap_or_default(),
        )
        .checked_add(1)
        .ok_or_else(|| DbError::Rule {
            constraint: "native_session_revision".into(),
        })?;
    let epoch = old.opening_epoch;
    let admitted = tool_admission(
        Some(old),
        body,
        observation,
        machine,
        revision,
        epoch,
        "session.open",
    )
    .ok_or_else(|| DbError::Rule {
        constraint: "native_session_missing".into(),
    })?;
    let native = NativeTarget::Session(SessionTarget {
        thread_id: thread,
        segment_id: Some(segment),
        opening_id: Some(admitted.opening_id),
        opening_epoch: Some(admitted.opening_epoch),
    });
    let mut value = serde_json::to_value(admitted).map_err(|_| DbError::Rule {
        constraint: "native_session_target".into(),
    })?;
    sqlx::query("UPDATE native_session_states SET delivered_resources=jsonb_set(delivered_resources,'{tool_admission}',$2)||jsonb_build_object('admitted_revision',$3::bigint) WHERE id=$1")
        .bind(segment).bind(&value).bind(revision).execute(conn).await?;
    value["native_target"] = serde_json::to_value(native).map_err(|_| DbError::Rule {
        constraint: "native_session_target".into(),
    })?;
    Ok(value)
}
