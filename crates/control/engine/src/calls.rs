//! Durable model-call recovery, guarded per subject and executed under the interrupted input author's authority.

use crate::Engine;
use crate::access::ProviderUse;
use crate::error::EngineError;
use crate::unit::Unit;
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::Permission;
use orch_authz::Caller;
use orch_db::queries::threads::{self, ThreadRow, TurnRow, thread_state};
use orch_db::{Conn, DbError};
use orch_provider_contract::calls::{Candidate, Cursor, Match, Purpose, Rule, Step, Until};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The persisted recovery visible to a thread's readers.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Recovery {
    /// The subject whose calls are recovering.
    pub node_id: Id,
    /// Agent, summary, review or embedding.
    pub purpose: String,
    /// The thread slot for an agent, or the original logical request for a service.
    pub call_key: Id,
    /// The failed turn or service call whose failure reserved this step.
    pub reference: Id,
    /// The normalized, redacted failure.
    pub failure: Json,
    /// The rule that reserved the step.
    pub rule: Json,
    /// Progress through the rule's steps.
    pub cursor: Json,
    /// The next action, if any.
    pub action: Option<Json>,
    /// Scheduled, switching, running, stopped or waiting.
    pub state: String,
    /// The known time work may resume.
    pub due_at: Option<DateTime<Utc>>,
    /// The original author, input and selected history, plus actual failover choice.
    pub context: Json,
}

/// The configurable bounds read together with a place's inherited rules.
pub struct Policy {
    /// First matching rule wins.
    pub rules: Vec<Rule>,
    /// Automatic recoveries allowed in the rolling window.
    pub limit: i64,
    /// The rolling window's duration, in seconds.
    pub window: i64,
    /// Whether a loop stop tells the owner.
    pub notify: bool,
    /// Whether the thread returns to its original choice at the next independent turn.
    pub return_next: bool,
}

impl Policy {
    /// Reads inherited values through the settings registry, including locks.
    pub async fn load(conn: &mut Conn, node: Id) -> Result<Self, EngineError> {
        let wanted = [
            keys::CALLS_RULES,
            keys::CALLS_REPEAT_LIMIT,
            keys::CALLS_REPEAT_WINDOW,
            keys::CALLS_NOTIFY_ON_STOP,
            keys::CALLS_RETURN,
        ]
        .into_iter()
        .filter_map(orch_settings_contract::registry::get)
        .collect::<Vec<_>>();
        let snapshot = orch_settings::Snapshot::load(
            conn,
            Place::Node(node),
            orch_settings::Wanted::Keys(&wanted),
        )
        .await?;
        let get = |key: &str| -> Result<Json, EngineError> {
            let def = orch_settings_contract::registry::get(key)
                .ok_or_else(|| EngineError::unreadable("call recovery setting is missing"))?;
            snapshot
                .resolve(def)
                .value
                .ok_or_else(|| EngineError::unreadable("call recovery setting has no value"))
        };
        Ok(Self {
            rules: orch_provider_contract::calls::validate(&get(keys::CALLS_RULES)?)
                .map_err(EngineError::unreadable)?,
            limit: get(keys::CALLS_REPEAT_LIMIT)?
                .as_i64()
                .ok_or_else(|| EngineError::unreadable("invalid repeat limit"))?,
            window: get(keys::CALLS_REPEAT_WINDOW)?
                .as_i64()
                .ok_or_else(|| EngineError::unreadable("invalid repeat window"))?,
            notify: get(keys::CALLS_NOTIFY_ON_STOP)?
                .as_bool()
                .ok_or_else(|| EngineError::unreadable("invalid stop notice"))?,
            return_next: get(keys::CALLS_RETURN)?.as_str() == Some("next_turn"),
        })
    }
}

/// Reads the subject's latest recovery; service callers use their explicit logical root instead.
pub async fn read(
    conn: &mut Conn,
    node: Id,
    purpose: Purpose,
) -> Result<Option<Recovery>, EngineError> {
    Ok(sqlx::query_as("SELECT node_id,purpose,call_key,reference,failure,rule,cursor,action,state,due_at,context FROM model_call_recovery WHERE node_id=$1 AND purpose=$2 ORDER BY updated_at DESC,reference DESC LIMIT 1")
        .bind(node).bind(purpose.as_str()).fetch_optional(conn).await.map_err(DbError::from)?)
}

/// Reads only the named logical request, keeping concurrent service workflows independent.
pub async fn read_root(
    conn: &mut Conn,
    node: Id,
    purpose: Purpose,
    root: Id,
) -> Result<Option<Recovery>, EngineError> {
    let key = if purpose == Purpose::Agent {
        node
    } else {
        root
    };
    Ok(sqlx::query_as("SELECT node_id,purpose,call_key,reference,failure,rule,cursor,action,state,due_at,context FROM model_call_recovery WHERE node_id=$1 AND purpose=$2 AND call_key=$3")
        .bind(node).bind(purpose.as_str()).bind(key).fetch_optional(conn).await.map_err(DbError::from)?)
}

/// Reads a workflow's pending recovery, or its completed request needed for retained-result recovery.
pub async fn read_workflow(
    conn: &mut Conn,
    node: Id,
    purpose: Purpose,
    workflow: Id,
) -> Result<Option<Recovery>, EngineError> {
    Ok(sqlx::query_as("SELECT node_id,purpose,call_key,reference,failure,rule,cursor,action,state,due_at,context FROM model_call_recovery WHERE node_id=$1 AND purpose=$2 AND context->>'workflow_id'=$3 AND state<>'dismissed' ORDER BY (state='completed'),updated_at DESC,reference DESC LIMIT 1")
        .bind(node).bind(purpose.as_str()).bind(workflow.to_string()).fetch_optional(conn).await.map_err(DbError::from)?)
}

/// Marks a validated service result complete without changing another logical call's state.
pub async fn finish_service(
    conn: &mut Conn,
    node: Id,
    purpose: Purpose,
    root: Id,
) -> Result<(), EngineError> {
    sqlx::query("UPDATE model_call_recovery SET state='completed',due_at=NULL WHERE node_id=$1 AND purpose=$2 AND call_key=$3 AND state<>'dismissed'")
        .bind(node).bind(purpose.as_str()).bind(root).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}

/// The selected identity of a safe-to-repeat auxiliary model request.
pub struct ServiceCall<'a> {
    /// The place whose policy applies and whose guard is used.
    pub node: Id,
    /// Summary, review or embedding.
    pub purpose: Purpose,
    /// The durable identity of this original logical request.
    pub root: Id,
    /// The workflow whose status owns this request, where there is one.
    pub workflow: Option<Id>,
    /// The task within that workflow, so a changed body retires only its own older request.
    pub task: Option<&'a str>,
    /// The selected provider before failover.
    pub provider: Id,
    /// The configured model, before failover.
    pub model: &'a str,
    /// The immutable generation's dimensions for an embedding.
    pub dimensions: Option<u32>,
}

/// What a service caller may attempt now, or the reason it must remain paused.
pub enum ServicePlan {
    /// One request, whose authority and caps the caller still checks before sending.
    Ready {
        /// Stable across process loss for this attempt.
        call: Id,
        /// An explicitly configured compatible destination, if selected.
        candidate: Option<Candidate>,
    },
    /// No request may be sent until its stored due time or an explicit change.
    Paused,
}

/// Resumes service work inside the caller's transaction under the same rules and frozen embedding identity.
pub async fn plan_service(
    conn: &mut Conn,
    call: &ServiceCall<'_>,
    now: DateTime<Utc>,
) -> Result<ServicePlan, EngineError> {
    lock_subject(conn, call.node).await?;
    if let (Some(workflow), Some(task)) = (call.workflow, call.task) {
        sqlx::query("UPDATE model_call_recovery SET state='dismissed',due_at=NULL WHERE node_id=$1 AND purpose=$2 AND call_key<>$3 AND context->>'workflow_id'=$4 AND context->>'task'=$5 AND state<>'completed'")
            .bind(call.node).bind(call.purpose.as_str()).bind(call.root).bind(workflow.to_string()).bind(task).execute(&mut *conn).await.map_err(DbError::from)?;
    }
    let Some(mut row) = read_root(conn, call.node, call.purpose, call.root).await? else {
        return Ok(ServicePlan::Ready {
            call: call.root,
            candidate: None,
        });
    };
    if row.state == "dismissed" {
        return Ok(ServicePlan::Paused);
    }
    if row.state == "running" {
        let candidate = row
            .context
            .get("candidate")
            .map(|value| serde_json::from_value::<Candidate>(value.clone()))
            .transpose()?
            .or_else(|| match step(&row) {
                Some(Step::Failover { candidate }) => Some(candidate),
                _ => None,
            });
        return Ok(ServicePlan::Ready {
            call: id(&row.context, "next_call").unwrap_or(call.root),
            candidate,
        });
    }
    let same_choice = row.context["configured_provider"] == json!(call.provider)
        && row.context["configured_model"] == call.model;
    let policy = Policy::load(conn, call.node).await?;
    let mut same_rule = policy
        .rules
        .iter()
        .find(|rule| rule.when.matches(&match_of(&row, call.purpose)))
        .is_some_and(|rule| json!(rule) == row.rule);
    if same_choice
        && row.context["root_call"] == json!(call.root)
        && !same_rule
        && row.state != "running"
    {
        let replanned = reserve_row(
            conn,
            Reservation {
                node: call.node,
                purpose: call.purpose,
                reference: row.reference,
                failure: &row.failure,
                context: row.context.clone(),
                now,
                replan: true,
            },
        )
        .await?;
        if let Some(current) = replanned {
            row = current;
            same_rule = true;
        } else {
            sqlx::query("UPDATE model_call_recovery SET state='waiting',due_at=NULL,context=jsonb_set(context,'{detail}',to_jsonb('rules_changed'::text)) WHERE node_id=$1 AND purpose=$2 AND call_key=$3")
                .bind(call.node).bind(call.purpose.as_str()).bind(call.root).execute(conn).await.map_err(DbError::from)?;
            return Ok(ServicePlan::Paused);
        }
    }
    if matches!(row.state.as_str(), "stopped" | "waiting") && same_choice && same_rule {
        return Ok(ServicePlan::Paused);
    }
    if row.context["root_call"] != json!(call.root) || !same_choice || !same_rule {
        return Ok(ServicePlan::Ready {
            call: call.root,
            candidate: None,
        });
    }
    if row.state == "scheduled" {
        let cutoff = history_cutoff(conn, call.node, now, policy.window).await?;
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1 AND failure_kind=$2 AND automatic AND ($3::timestamptz IS NULL OR at>$3)")
            .bind(call.node).bind(row.failure["code"].as_str().unwrap_or("harness_error")).bind(cutoff).fetch_one(&mut *conn).await.map_err(DbError::from)?;
        if policy.limit == 0 || count > policy.limit {
            sqlx::query("UPDATE model_call_recovery SET state='stopped',due_at=NULL,context=jsonb_set(context,'{detail}',to_jsonb($4::text)) WHERE node_id=$1 AND purpose=$2 AND call_key=$3")
                .bind(call.node).bind(call.purpose.as_str()).bind(call.root).bind(if policy.limit==0 {"auto_disabled"} else {"repeat_limit_changed"}).execute(conn).await.map_err(DbError::from)?;
            return Ok(ServicePlan::Paused);
        }
    }
    if row.due_at.is_some_and(|at| at > now) {
        return Ok(ServicePlan::Paused);
    }
    let mut candidate = None;
    if let Some(Step::Failover { candidate: next }) = step(&row) {
        if call.purpose == Purpose::Embedding
            && !call.dimensions.is_some_and(|dimensions| {
                orch_provider_contract::calls::compatible_embedding(&next, call.model, dimensions)
            })
        {
            skip_step(conn, &row, "embedding_identity", now).await?;
            return Ok(ServicePlan::Paused);
        }
        candidate = Some(next);
    } else if let Some(value) = row.context.get("candidate") {
        candidate = Some(serde_json::from_value(value.clone())?);
    }
    let attempt = if row.state == "running" {
        id(&row.context, "next_call").unwrap_or(call.root)
    } else {
        crate::ids::derived(
            "service.recovery",
            &[
                call.root.as_bytes(),
                row.reference.as_bytes(),
                row.cursor.to_string().as_bytes(),
                row.rule.to_string().as_bytes(),
            ],
        )
    };
    if row.context["next_call"] != json!(attempt)
        && let Some(context) = row.context.as_object_mut()
    {
        context.remove("next_protocol");
    }
    row.context["next_call"] = json!(attempt);
    sqlx::query("UPDATE model_call_recovery SET state='running',due_at=NULL,context=$3,updated_at=$4 WHERE node_id=$1 AND purpose=$2 AND call_key=$5")
        .bind(call.node).bind(call.purpose.as_str()).bind(row.context).bind(now).bind(call.root).execute(&mut *conn).await.map_err(DbError::from)?;
    Ok(ServicePlan::Ready {
        call: attempt,
        candidate,
    })
}

fn vendor_seconds(failure: &Json) -> Option<i64> {
    failure["retry_after_secs"]
        .as_i64()
        .or_else(|| {
            failure
                .pointer("/limit/retry_after_secs")
                .and_then(Json::as_i64)
        })
        .filter(|seconds| *seconds >= 0)
}

fn relative_deadline(context: &Json, seconds: i64, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let failed = context["failed_at"]
        .as_str()
        .and_then(|value| value.parse::<Time>().ok())
        .map(Time::get)
        .unwrap_or(now);
    failed.checked_add_signed(chrono::Duration::try_seconds(seconds)?)
}

fn vendor_reset(failure: &Json, context: &Json, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    failure["resets_at"]
        .as_str()
        .and_then(|value| value.parse::<Time>().ok())
        .map(Time::get)
        .or_else(|| relative_deadline(context, vendor_seconds(failure)?, now))
}

fn vendor_delay(failure: &Json, context: &Json, now: DateTime<Utc>) -> Option<i64> {
    // Relative evidence belongs to the failed call, not to each later cursor or rule edit.
    if let Some(seconds) = vendor_seconds(failure) {
        return Some(
            relative_deadline(context, seconds, now)
                .map(|deadline| {
                    orch_provider_contract::calls::seconds_until(
                        Time::new(deadline),
                        Time::new(now),
                    )
                })
                .unwrap_or(i64::MAX),
        );
    }
    vendor_reset(failure, context, now)
        .map(|reset| orch_provider_contract::calls::seconds_until(Time::new(reset), Time::new(now)))
}

/// Skips a configured candidate that cannot be used, retaining the reason and the rest of the chain.
pub async fn skip_step(
    conn: &mut Conn,
    row: &Recovery,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let mut cursor: Cursor = serde_json::from_value(row.cursor.clone())?;
    let rule: Rule = serde_json::from_value(row.rule.clone())?;
    let next = cursor.next(&rule);
    let mut due = None;
    if let Some((step, attempt)) = &next {
        due = match step {
            Step::Wait {
                until: Until::Person,
            } => None,
            Step::Wait {
                until: Until::Reset,
            } => vendor_reset(&row.failure, &row.context, now),
            _ => chrono::Duration::try_seconds(orch_provider_contract::calls::retry_delay(
                step,
                *attempt,
                vendor_delay(&row.failure, &row.context, now),
            ))
            .and_then(|delay| now.checked_add_signed(delay)),
        };
    }
    sqlx::query("UPDATE model_call_recovery SET cursor=$3,action=$4,state=$5,due_at=$6,context=jsonb_set(context,'{skipped}',to_jsonb($7::text)),updated_at=$8 WHERE node_id=$1 AND purpose=$2 AND reference=$9 AND call_key=$10")
        .bind(row.node_id).bind(&row.purpose).bind(json!(cursor)).bind(next.map(|(step,attempt)|json!({"step":step,"attempt":attempt})))
        .bind(if due.is_some() {"scheduled"} else {"waiting"}).bind(due).bind(reason).bind(now).bind(row.reference).bind(row.call_key).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}

/// Reserves a durable decision inside the caller's transaction; the shared subject guard serializes its count.
pub async fn reserve(
    conn: &mut Conn,
    node: Id,
    purpose: Purpose,
    reference: Id,
    failure: &Json,
    context: Json,
    now: DateTime<Utc>,
) -> Result<Option<Recovery>, EngineError> {
    reserve_row(
        conn,
        Reservation {
            node,
            purpose,
            reference,
            failure,
            context,
            now,
            replan: false,
        },
    )
    .await
}

/// Reserves a service failure with an explicit logical root and owning workflow.
pub async fn reserve_service(
    conn: &mut Conn,
    call: &ServiceCall<'_>,
    reference: Id,
    failure: &Json,
    mut context: Json,
    now: DateTime<Utc>,
) -> Result<Option<Recovery>, EngineError> {
    context["root_call"] = json!(call.root);
    context["root_input"] = json!(call.root);
    context["configured_provider"] = json!(call.provider);
    context["configured_model"] = json!(call.model);
    if let Some(workflow) = call.workflow {
        context["workflow_id"] = json!(workflow);
    }
    if let Some(task) = call.task {
        context["task"] = json!(task);
    }
    reserve(
        conn,
        call.node,
        call.purpose,
        reference,
        failure,
        context,
        now,
    )
    .await
}

async fn lock_subject(conn: &mut Conn, node: Id) -> Result<(), EngineError> {
    // A subject shares one guard across call purposes without reversing node/thread row lock order.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('model-call:'||$1::text,0))")
        .bind(node)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

async fn history_cutoff(
    conn: &mut Conn,
    node: Id,
    now: DateTime<Utc>,
    window: i64,
) -> Result<Option<DateTime<Utc>>, EngineError> {
    let cutoff =
        chrono::Duration::try_seconds(window).and_then(|window| now.checked_sub_signed(window));
    // A very large window includes all stored history without constructing an out-of-range SQL timestamp.
    if cutoff.is_some_and(|at| at >= DateTime::<Utc>::UNIX_EPOCH) {
        return Ok(cutoff);
    }
    let first: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT min(at) FROM model_call_recovery_attempts WHERE node_id=$1")
            .bind(node)
            .fetch_one(conn)
            .await
            .map_err(DbError::from)?;
    Ok(cutoff.filter(|cutoff| first.is_some_and(|first| *cutoff >= first)))
}

struct Reservation<'a> {
    node: Id,
    purpose: Purpose,
    reference: Id,
    failure: &'a Json,
    context: Json,
    now: DateTime<Utc>,
    replan: bool,
}

async fn reserve_row(
    conn: &mut Conn,
    request: Reservation<'_>,
) -> Result<Option<Recovery>, EngineError> {
    let Reservation {
        node,
        purpose,
        reference,
        failure,
        mut context,
        now,
        replan,
    } = request;
    lock_subject(conn, node).await?;
    let call_key = if purpose == Purpose::Agent {
        node
    } else {
        id(&context, "root_call")
            .or_else(|| id(&context, "root_input"))
            .unwrap_or(reference)
    };
    let previous = read_root(conn, node, purpose, call_key).await?;
    if purpose != Purpose::Agent
        && previous
            .as_ref()
            .is_some_and(|row| row.state == "dismissed")
    {
        return Ok(previous);
    }

    if purpose != Purpose::Agent
        && let Some(previous) = previous.as_ref().filter(|row| {
            row.context["root_call"] == context["root_call"] && row.state == "running"
        })
        && context.get("candidate").is_none()
        && let Some(candidate) = previous.context.get("candidate")
    {
        context["candidate"] = candidate.clone();
    }
    if !replan
        && previous
            .as_ref()
            .is_some_and(|row| row.reference == reference)
    {
        return Ok(previous);
    }
    if !replan || context["failed_at"].as_str().is_none() {
        context["failed_at"] = json!(Time::new(now));
    }
    let policy = Policy::load(conn, node).await?;
    let kind = failure["code"].as_str().unwrap_or("harness_error");
    let provider = context["provider_id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .unwrap_or_default();
    let model = context["model"].as_str().unwrap_or_default();
    let matched = Match {
        kind,
        reason: failure["reason"].as_str(),
        provider,
        model,
        purpose,
        status: failure["status"].as_i64(),
        words: failure["said"]
            .as_str()
            .or_else(|| failure["message"].as_str())
            .unwrap_or_default(),
    };
    let Some(rule) = policy.rules.iter().find(|rule| rule.when.matches(&matched)) else {
        return Ok(None);
    };
    let same = previous.as_ref().filter(|row| {
        row.rule == json!(rule)
            && row.failure["code"] == failure["code"]
            && row.context["root_input"] == context["root_input"]
            && row.state == "running"
    });
    let mut cursor = same
        .map(|row| serde_json::from_value::<Cursor>(row.cursor.clone()))
        .transpose()?
        .unwrap_or_default();
    let next = cursor.next(rule);
    let mut action = next
        .as_ref()
        .map(|(step, attempt)| json!({"step":step,"attempt":attempt}));
    let mut state = "scheduled";
    let mut due = Some(now);
    let cutoff = history_cutoff(conn, node, now, policy.window).await?;
    let (count,observed,first):(i64,i64,Option<DateTime<Utc>>)=sqlx::query_as("WITH recent AS (SELECT failure_kind,automatic,at FROM model_call_recovery_attempts WHERE node_id=$1 AND ($3::timestamptz IS NULL OR at>$3) AND id<>$4) SELECT count(*) FILTER(WHERE failure_kind=$2 AND automatic),count(*) FILTER(WHERE failure_kind=$2),min(at) FROM recent")
        .bind(node).bind(kind).bind(cutoff).bind(reference).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    let automatic = next.as_ref().is_some_and(|(step, _)| match step {
        Step::Wait {
            until: Until::Person,
        } => false,
        Step::Wait {
            until: Until::Reset,
        } => vendor_reset(failure, &context, now).is_some(),
        _ => true,
    });
    if (automatic || count > 0) && count >= policy.limit {
        state = "stopped";
        due = None;
        action = Some(
            json!({"detail":"repeat_limit","count":observed.saturating_add(1),"window_secs":policy.window,"notify":policy.notify,"notice_group":Time::new(first.unwrap_or(now))}),
        );
    } else if let Some((Step::Wait { until }, _)) = &next {
        due = if *until == Until::Reset {
            vendor_reset(failure, &context, now)
        } else {
            None
        };
        state = if due.is_some() {
            "scheduled"
        } else {
            "waiting"
        };
    } else if next.is_none() {
        state = "waiting";
        due = None;
    } else {
        if let Some((step, attempt)) = &next {
            let vendor = vendor_delay(failure, &context, now);
            let delay = orch_provider_contract::calls::retry_delay(step, *attempt, vendor);
            due = chrono::Duration::try_seconds(delay)
                .and_then(|delay| now.checked_add_signed(delay));
            if due.is_none() {
                state = "waiting";
            }
        }
    }
    sqlx::query("INSERT INTO model_call_recovery_attempts(id,node_id,purpose,failure_kind,automatic,at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO UPDATE SET automatic=EXCLUDED.automatic")
        .bind(reference).bind(node).bind(purpose.as_str()).bind(kind).bind(state=="scheduled" && automatic).bind(now).execute(&mut *conn).await.map_err(DbError::from)?;
    let rule = serde_json::to_value(rule)?;
    let cursor = serde_json::to_value(cursor)?;
    sqlx::query("INSERT INTO model_call_recovery(node_id,purpose,reference,failure,rule,cursor,action,state,due_at,context,updated_at,call_key) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT(node_id,purpose,call_key) DO UPDATE SET reference=EXCLUDED.reference,failure=EXCLUDED.failure,rule=EXCLUDED.rule,cursor=EXCLUDED.cursor,action=EXCLUDED.action,state=EXCLUDED.state,due_at=EXCLUDED.due_at,context=EXCLUDED.context,updated_at=EXCLUDED.updated_at")
        .bind(node).bind(purpose.as_str()).bind(reference).bind(failure).bind(rule).bind(cursor).bind(action).bind(state).bind(due).bind(context).bind(now).bind(call_key)
        .execute(&mut *conn).await.map_err(DbError::from)?;
    read_root(conn, node, purpose, call_key).await
}

fn step(row: &Recovery) -> Option<Step> {
    serde_json::from_value(row.action.as_ref()?.get("step")?.clone()).ok()
}

fn id(context: &Json, key: &str) -> Option<Id> {
    context[key].as_str()?.parse().ok()
}

/// A provider switch-off pauses work without classifying a model failure.
pub const PROVIDER_SWITCHED_OFF: &str = "calls.details.provider_switched_off";
/// A model switch-off pauses work without classifying a model failure.
pub const MODEL_SWITCHED_OFF: &str = "calls.details.model_switched_off";
/// Cancellation stops an auxiliary request without starting a recovery chain.
pub const REQUEST_CANCELLED: &str = "calls.details.request_cancelled";

/// The live administrative reason a model call must wait, independent of vendor failures.
pub async fn switched_off_reason(
    conn: &mut Conn,
    provider: Id,
    model: &str,
) -> Result<Option<&'static str>, EngineError> {
    use orch_db::queries::resources::thread_engine as resources;
    if resources::provider(conn, provider)
        .await?
        .is_some_and(|provider| provider.status == "disabled")
    {
        return Ok(Some(PROVIDER_SWITCHED_OFF));
    }
    if resources::model(conn, provider, model)
        .await?
        .is_some_and(|model| !model.enabled)
    {
        return Ok(Some(MODEL_SWITCHED_OFF));
    }
    Ok(None)
}

/// Administrative pauses never reserve a new failed-call reference, attempt or rule step.
pub async fn pause_service_in(
    conn: &mut Conn,
    node: Id,
    purpose: Purpose,
    root: Id,
    reason: &str,
) -> Result<(), EngineError> {
    let detail = reason.strip_prefix("calls.details.").unwrap_or(reason);
    sqlx::query("UPDATE model_call_recovery SET state='waiting',due_at=NULL,context=jsonb_set(context,'{detail}',to_jsonb($4::text)) WHERE node_id=$1 AND purpose=$2 AND call_key=$3 AND state NOT IN ('completed','dismissed')")
        .bind(node).bind(purpose.as_str()).bind(root).bind(detail).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}

/// A retained administrative pause has its own reason instead of the preceding vendor failure.
#[must_use]
pub fn administrative_pause_reason(row: &Recovery) -> Option<&'static str> {
    match row.context["detail"].as_str() {
        Some("provider_switched_off") => Some(PROVIDER_SWITCHED_OFF),
        Some("model_switched_off") => Some(MODEL_SWITCHED_OFF),
        Some("request_cancelled") => Some(REQUEST_CANCELLED),
        _ => None,
    }
}

/// An automatic switch carries the failed turn's retained effects as well as earlier completed turns.
pub(crate) async fn carried_failure(
    conn: &mut Conn,
    node: Id,
    view: Id,
) -> Result<Option<i32>, EngineError> {
    let Some(row) = read(conn, node, Purpose::Agent).await?.filter(|row| {
        row.state == "switching" && id(&row.context, "continuation_id") == Some(view)
    }) else {
        return Ok(None);
    };
    // A return has its own action reference, but must still carry the interrupted turn when Stop
    // prevented a later completed turn from extending the ordinary history cut.
    let failed_turn = id(&row.context, "failed_turn").unwrap_or(row.reference);
    Ok(sqlx::query_scalar("SELECT ordinal FROM turns WHERE id=$1 AND thread_id=$2 AND continuation_id=$3 AND state='failed'")
        .bind(failed_turn).bind(node).bind(view).fetch_optional(conn).await.map_err(DbError::from)?)
}

/// The recovery state served to readers without exposing internal switch and author bookkeeping.
pub async fn view(
    conn: &mut Conn,
    node: Id,
) -> Result<Option<orch_api_contract::calls::CallRecovery>, EngineError> {
    view_purpose(conn, node, Purpose::Agent).await
}

/// Reads the presentation of one model-call purpose without internal attribution and switch bookkeeping.
pub async fn view_purpose(
    conn: &mut Conn,
    node: Id,
    purpose: Purpose,
) -> Result<Option<orch_api_contract::calls::CallRecovery>, EngineError> {
    read(conn, node, purpose)
        .await?
        .filter(|row| !matches!(row.state.as_str(), "completed" | "dismissed"))
        .map(|row| presentation(&row))
        .transpose()
}

/// Produces a compact status while keeping arbitrary configured parameters out of status replies.
pub fn presentation(row: &Recovery) -> Result<orch_api_contract::calls::CallRecovery, EngineError> {
    let action = row.action.as_ref();
    let mut next = step(row);
    if let Some(Step::Failover { candidate }) = next.as_mut() {
        candidate.params = json!({});
    }
    Ok(orch_api_contract::calls::CallRecovery {
        state: row.state.clone(),
        failure: serde_json::from_value(row.failure.clone())?,
        rule_id: row.rule["id"].as_str().unwrap_or_default().to_owned(),
        step: next,
        continuing: row.purpose == Purpose::Agent.as_str() && row.context["effects"] == true,
        attempt: action
            .and_then(|action| action["attempt"].as_u64())
            .and_then(|number| u32::try_from(number).ok())
            .unwrap_or_default(),
        due_at: row.due_at.map(Time::new),
        detail: row.context["detail"]
            .as_str()
            .or_else(|| row.context["skipped"].as_str())
            .or_else(|| action.and_then(|action| action["detail"].as_str()))
            .map(str::to_owned),
        count: action.and_then(|action| action["count"].as_i64()),
        window_secs: action.and_then(|action| action["window_secs"].as_i64()),
    })
}

async fn choice_of(conn: &mut Conn, thread: &ThreadRow) -> Result<Json, EngineError> {
    let wanted = [
        keys::AGENT_MODEL,
        keys::AGENT_EFFORT,
        keys::AGENT_PARAMS,
        keys::AGENT_FEATURES,
    ]
    .into_iter()
    .filter_map(orch_settings_contract::registry::get)
    .collect::<Vec<_>>();
    let snapshot = orch_settings::Snapshot::load(
        conn,
        Place::Node(thread.node_id),
        orch_settings::Wanted::Keys(&wanted),
    )
    .await?;
    let selected = wanted
        .into_iter()
        .map(|def| {
            (
                def.key.to_owned(),
                snapshot.resolve(def).value.unwrap_or(Json::Null),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    Ok(
        json!({"provider_id":thread.provider_id,"agent_id":thread.agent_id,"environment_id":thread.environment_id,"directory":thread.workspace_dir,"continuation_id":thread.active_continuation_id,"execution_revision":thread.execution_revision,"settings":selected}),
    )
}

/// A return needs a ready route; structural model access needs fresh configuration evidence.
async fn original_ready(conn: &mut Conn, original: &Json) -> Result<bool, EngineError> {
    use orch_db::queries::resources::thread_engine as resources;
    let (Some(provider), Some(model)) = (id(original, "provider_id"), original["model"].as_str())
    else {
        return Ok(false);
    };
    let Some(provider) = resources::provider(conn, provider)
        .await?
        .filter(|provider| provider.status == "ready" && provider.deleted_at.is_none())
    else {
        return Ok(false);
    };
    let Some(model) = resources::model(conn, provider.id, model)
        .await?
        .filter(|model| model.enabled)
    else {
        return Ok(false);
    };
    if let Some(agent) = id(original, "agent_id")
        && !resources::agent(conn, agent)
            .await?
            .is_some_and(|agent| !agent.disabled && agent.deleted_at.is_none())
    {
        return Ok(false);
    }
    if original["failure_kind"] == "model_unavailable" {
        return Ok(original["provider_revision"] != json!(provider.revision)
            || original["model_parameters"] != model.parameters
            || original["model_enabled"] == false);
    }
    Ok(true)
}

fn match_of<'a>(row: &'a Recovery, purpose: Purpose) -> Match<'a> {
    Match {
        kind: row.failure["code"].as_str().unwrap_or("harness_error"),
        reason: row.failure["reason"].as_str(),
        provider: id(&row.context, "provider_id").unwrap_or_default(),
        model: row.context["model"].as_str().unwrap_or_default(),
        purpose,
        status: row.failure["status"].as_i64(),
        words: row.failure["said"]
            .as_str()
            .or_else(|| row.failure["message"].as_str())
            .unwrap_or_default(),
    }
}

/// Applies a temporary candidate while leaving the thread's current selected settings intact.
pub(crate) async fn apply_override(
    conn: &mut Conn,
    node: Id,
    settings: &mut crate::resolved::Resolved,
) -> Result<(), EngineError> {
    let Some(row) = read(conn, node, Purpose::Agent)
        .await?
        .filter(|row| row.context["candidate_active"] == true && row.state != "dismissed")
    else {
        return Ok(());
    };
    let Some(value) = row.context.get("candidate") else {
        return Ok(());
    };
    let Some(thread) = threads::thread(conn, node).await? else {
        return Ok(());
    };
    if row.context.get("choice").is_some()
        && row.context["choice"] != choice_of(conn, &thread).await?
    {
        return Ok(());
    }
    if row.state == "completed"
        && Policy::load(conn, node).await?.return_next
        && let Some(original) = row.context.get("original")
        && original_ready(conn, original).await?
    {
        return Ok(());
    }
    let candidate: Candidate = serde_json::from_value(value.clone())?;
    settings.model = Some(candidate.model);
    if let Some(effort) = candidate.params.get("effort") {
        settings.effort = effort.as_str().map(str::to_owned);
    }
    if let (Some(params), Some(extra)) = (
        settings.params.as_object_mut(),
        candidate.params.as_object(),
    ) {
        params.extend(extra.clone());
    }
    Ok(())
}

/// Parameters explicitly requested by a candidate must remain eligible rather than disappear silently.
pub(crate) fn parameters_ready(
    descriptor: &orch_agent_contract::descriptor::AgentDescriptor,
    context: &orch_agent_contract::parameters::Eligibility,
    settings: &crate::resolved::Resolved,
    params: &Json,
    opening: bool,
) -> bool {
    use orch_agent_contract::parameters::{ParameterApply, Resolution};
    params.as_object().is_some_and(|params| {
        params.iter().all(|(key, value)| {
            if key == "effort" {
                return value.is_null()
                    || value.as_str().is_some_and(|effort| {
                        context
                            .model
                            .efforts()
                            .iter()
                            .any(|allowed| allowed == effort)
                    });
            }
            descriptor
                .parameters
                .iter()
                .find(|parameter| parameter.key == *key)
                .is_some_and(|parameter| {
                    (parameter.apply == ParameterApply::NextTurn
                        || (opening && parameter.apply == ParameterApply::NewSession))
                        && match parameter.resolve(context, &settings.features, Some(value)) {
                            Resolution::Send(sent) => sent == *value,
                            Resolution::SendNothing => value.is_null(),
                            _ => false,
                        }
                })
        })
    })
}

async fn candidate_parameters_ready(
    conn: &mut Conn,
    thread: &ThreadRow,
    candidate: &Candidate,
) -> Result<bool, EngineError> {
    use orch_agent_contract::{descriptor::Placement, parameters::Eligibility};
    use orch_db::queries::{
        environments::thread_engine as environments, resources::thread_engine as resources,
    };
    if candidate
        .params
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        return Ok(true);
    }
    let (Some(agent), Some(environment), Some(arrangement)) = (
        thread.agent_id,
        thread.environment_id,
        thread.arrangement.as_deref(),
    ) else {
        return Ok(false);
    };
    let Some(agent) = resources::agent(conn, agent).await? else {
        return Ok(false);
    };
    let Some(descriptor) = crate::harnesses::descriptor_of(&agent)? else {
        return Ok(false);
    };
    let Some(provider) = resources::provider(conn, candidate.provider_id).await? else {
        return Ok(false);
    };
    let Some(model) = resources::model(conn, candidate.provider_id, &candidate.model).await? else {
        return Ok(false);
    };
    let Some(environment) = environments::environment(conn, environment).await? else {
        return Ok(false);
    };
    let arrangement = serde_json::from_value(json!(arrangement))?;
    let context = Eligibility {
        credential_path: crate::binding::credential_path(arrangement, &provider, &agent),
        provider_kind: crate::binding::provider_kind(&provider),
        environment_placement: if environment
            .facts
            .pointer("/environment_descriptor/lifecycle")
            .and_then(Json::as_str)
            == Some("unit")
        {
            Placement::Managed
        } else {
            Placement::Connected
        },
        model: crate::binding::model_parameters(&model)?,
        harness_version: None,
    };
    let settings = crate::resolved::load(conn, thread.node_id).await?;
    Ok(parameters_ready(
        &descriptor,
        &context,
        &settings,
        &candidate.params,
        false,
    ))
}

/// Supplies only the automatic switch's confirmed candidate to its ordinary session-opening path.
pub(crate) async fn switch_candidate(
    conn: &mut Conn,
    node: Id,
    provider: Id,
    agent: Id,
    model: &str,
) -> Result<Option<Candidate>, EngineError> {
    let Some(row) = read(conn, node, Purpose::Agent)
        .await?
        .filter(|row| row.state == "switching" && row.context["returning"] != true)
    else {
        return Ok(None);
    };
    let Some(value) = row.context.get("candidate") else {
        return Ok(None);
    };
    let candidate: Candidate = serde_json::from_value(value.clone())?;
    Ok((candidate.provider_id == provider
        && candidate.agent_id.is_none_or(|id| id == agent)
        && candidate.model == model)
        .then_some(candidate))
}

/// Defers the next independent input only while the ordinary switch returns to a ready original choice.
pub(crate) async fn maybe_return_in(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
) -> Result<bool, EngineError> {
    let Some(mut row) = read(unit.conn, thread.node_id, Purpose::Agent)
        .await?
        .filter(|row| row.state == "completed")
    else {
        return Ok(false);
    };
    if !Policy::load(unit.conn, thread.node_id).await?.return_next {
        return Ok(false);
    }
    if row.context.get("choice").is_some()
        && row.context["choice"] != choice_of(unit.conn, thread).await?
    {
        sqlx::query("UPDATE model_call_recovery SET state='dismissed',due_at=NULL WHERE node_id=$1 AND purpose='agent'")
            .bind(thread.node_id).execute(&mut *unit.conn).await.map_err(DbError::from)?;
        return Ok(false);
    }
    let Some(original) = row.context.get("original").cloned() else {
        return Ok(false);
    };
    let Some(provider) = id(&original, "provider_id") else {
        return Ok(false);
    };
    let Some(input) = threads::pending_inputs(unit.conn, thread.node_id)
        .await?
        .into_iter()
        .find(|input| !input.queue_held)
    else {
        return Ok(false);
    };
    let Some(author) = input.author_id else {
        return Ok(false);
    };
    if !original_ready(unit.conn, &original).await? {
        return Ok(false);
    }
    if Some(provider) == thread.provider_id && id(&original, "agent_id") == thread.agent_id {
        record_step_marker(unit, thread, &row, "return", input.id, None).await?;
        sqlx::query("UPDATE model_call_recovery SET state='dismissed',context=context-'candidate'-'original'-'candidate_active' WHERE node_id=$1 AND purpose='agent'")
            .bind(thread.node_id).execute(&mut *unit.conn).await.map_err(DbError::from)?;
        return Ok(false);
    }
    let mut params = original["params"].clone();
    if !params.is_object() {
        params = json!({});
    }
    params["effort"] = original["effort"].clone();
    let candidate = Candidate {
        provider_id: provider,
        model: original["model"].as_str().unwrap_or_default().to_owned(),
        agent_id: id(&original, "agent_id"),
        params,
        dimensions: None,
    };
    row.context["returning"] = json!(true);
    row.context["detail"] = json!("returning");
    row.context["author_id"] = json!(author);
    row.context["input_id"] = json!(input.id);
    if let Some(context) = row.context.as_object_mut() {
        context.remove("switch_job");
    }
    let reference = crate::ids::derived(
        "call.return",
        &[row.reference.as_bytes(), input.id.as_bytes()],
    );
    sqlx::query("UPDATE model_call_recovery SET reference=$2,state='scheduled',due_at=$3,context=$4,action=$5,updated_at=$3 WHERE node_id=$1 AND purpose='agent'")
        .bind(thread.node_id).bind(reference).bind(unit.now()).bind(row.context).bind(json!({"step":{"type":"failover","candidate":candidate},"attempt":1})).execute(&mut *unit.conn).await.map_err(DbError::from)?;
    unit.wake(thread.node_id);
    Ok(true)
}

async fn record_step_marker(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    row: &Recovery,
    action: &str,
    input: Id,
    detail: Option<&str>,
) -> Result<(), EngineError> {
    if let Some(segment) = thread.current_segment_id {
        let model = if action == "skip" {
            step(row).and_then(|step| match step {
                Step::Failover { candidate } => Some(candidate.model),
                _ => None,
            })
        } else {
            row.context
                .pointer(if action == "return" {
                    "/original/model"
                } else {
                    "/candidate/model"
                })
                .and_then(Json::as_str)
                .map(str::to_owned)
        };
        let provider = if action == "skip" {
            step(row).and_then(|step| match step {
                Step::Failover { candidate } => Some(candidate.provider_id),
                _ => None,
            })
        } else {
            row.context
                .get(if action == "return" {
                    "original"
                } else {
                    "candidate"
                })
                .and_then(|choice| id(choice, "provider_id"))
        };
        let key = format!(
            "call-recovery:{}:{}:{}:{action}",
            row.reference,
            row.cursor["step"].as_u64().unwrap_or_default(),
            row.cursor["attempt"].as_u64().unwrap_or_default()
        );
        let attempt = row
            .action
            .as_ref()
            .and_then(|action| action["attempt"].as_u64())
            .and_then(|attempt| u32::try_from(attempt).ok())
            .unwrap_or(1);
        let rule = if action == "return" {
            row.context
                .pointer("/original/rule_id")
                .unwrap_or(&row.rule["id"])
        } else {
            &row.rule["id"]
        };
        let total = match step(row) {
            Some(Step::Retry { times, .. } | Step::Continue { times }) => Some(times),
            _ => None,
        };
        crate::ingest::add_marker(unit,thread.node_id,segment,None,&json!({"marker":"call_recovery","data":{"action":action,"model":model,"provider_id":provider,"detail":detail,"rule":rule,"kind":row.failure["code"],"attempt":attempt,"total":total,"automatic":true,"input_id":input}}),&key).await?;
    }
    Ok(())
}

/// Tells only the owner, only when the resolved stop-notice setting requests it.
pub(crate) async fn notify_stop(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    row: &Recovery,
) -> Result<(), EngineError> {
    if row
        .action
        .as_ref()
        .and_then(|action| action.get("notify"))
        .and_then(Json::as_bool)
        != Some(true)
    {
        return Ok(());
    }
    let owner =
        orch_db::queries::notifications::subscribers::owner_of(unit.conn, thread.node_id).await?;
    if let Some(owner) = owner {
        let event = crate::notifications::Event {
            kind: crate::notifications::Kind::ThreadBlocked,
            node: Some(thread.node_id),
            thread: Some(thread.node_id),
            approval: None,
            title: format!("{} needs attention", thread.name),
            body: "Automatic call recovery stopped".to_owned(),
            data: json!({"reason":"repeat_limit","failure_kind":row.failure["code"],"count":row.action.as_ref().and_then(|action|action["count"].as_i64()),"window_secs":row.action.as_ref().and_then(|action|action["window_secs"].as_i64())}),
            source: format!(
                "call-recovery-stop:{}:{}",
                thread.node_id,
                row.action
                    .as_ref()
                    .and_then(|action| action["notice_group"].as_str())
                    .unwrap_or_default()
            ),
            actor: None,
            at: unit.now(),
        };
        crate::notifications::tell(unit.conn, &event, &[owner], Some(thread.org_id)).await?;
    }
    Ok(())
}

impl Engine {
    /// Queues a visible Continue with its origin's live authority, without creating a failure or
    /// reserving recovery attempts. A lifecycle caller owns enabling the resource and releasing or
    /// holding its queue; use one stable reference for each stopped turn and resume operation.
    pub async fn continue_original_input_in(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        origin: &orch_db::queries::threads::InputRow,
        reference: Id,
        candidate: Option<&Candidate>,
    ) -> Result<Option<Id>, EngineError> {
        if origin.author_id.is_none()
            || origin.thread_id != thread.node_id
            || origin.continuation_id != thread.active_continuation_id
        {
            return Err(EngineError::unreadable(
                "the original input or its author is unavailable",
            ));
        }
        self.check_input_author_driving(unit, thread, origin)
            .await
            .map_err(EngineError::unreadable)?;
        let serving = crate::inputs::input_caller(unit, thread, origin)
            .await?
            .ok_or_else(|| {
                EngineError::unreadable("the original submission's authority is unavailable")
            })?;
        let allowed = if let Some(candidate) = candidate {
            self.candidate_allowed(
                unit.conn,
                thread.node_id,
                &serving,
                candidate,
                Purpose::Agent,
            )
            .await?
        } else {
            self.check_sender(unit, &serving, thread, true)
                .await
                .is_ok()
        };
        if !allowed {
            return Err(EngineError::unreadable(
                "the original actor cannot use the selected model",
            ));
        }
        let provenance = crate::inputs::derived_input_provenance(origin, None)?;
        let content = json!([{"type":"text","text":"Continue"}]);
        let client = format!("recovery:{reference}");
        let seq = unit.stamp(thread.node_id).await?;
        let at = unit.now();
        let next = threads::insert_input_in(
            unit.conn,
            &threads::NewInput {
                id: crate::ids::derived("call.continue", &[reference.as_bytes()]),
                thread_id: thread.node_id,
                author_id: provenance.author_id,
                submission_authority: provenance.submission_authority.as_ref(),
                run_as: provenance.run_as,
                run_as_authority: provenance.run_as_authority.as_ref(),
                source: "user",
                content: &content,
                delivery: "queue",
                state: "pending",
                turn_id: None,
                client_id: Some(&client),
                limit_bypass: None,
                place: threads::Place::Front,
                at,
                plan: origin.plan,
            },
            Some(origin.continuation_id),
            seq,
        )
        .await?;
        threads::set_seq_state(unit.conn, thread.node_id, seq).await?;
        unit.wake(thread.node_id);
        Ok(next.map(|input| input.id))
    }

    /// Applies a driver's control under their current action guard; retries still act for the original author.
    pub async fn control_call_recovery(
        &self,
        mut tx: orch_db::Tx,
        principal: orch_authz::Principal,
        node: Id,
        action: orch_api_contract::calls::CallRecoveryAction,
    ) -> Result<(), crate::inputs::InputError> {
        let thread = threads::lock(&mut tx, node)
            .await?
            .ok_or(crate::inputs::InputError::NotFound)?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        self.check_sender(&mut unit, principal.caller(), &thread, false)
            .await?;
        let row = read(unit.conn, node, Purpose::Agent)
            .await?
            .ok_or(crate::inputs::InputError::NotFound)?;
        if !matches!(
            row.state.as_str(),
            "scheduled" | "switching" | "running" | "stopped" | "waiting"
        ) {
            return Err(crate::inputs::InputError::Invalid(
                "there is no active call recovery",
            ));
        }
        if action != orch_api_contract::calls::CallRecoveryAction::Stop
            && (thread.current_turn_id.is_some() || row.state == "switching")
        {
            return Err(crate::inputs::InputError::Invalid(
                "a running turn or switch must be stopped through its own control",
            ));
        }
        match action {
            orch_api_contract::calls::CallRecoveryAction::Stop => {
                let finishing_switch =
                    row.state == "switching" && id(&row.context, "switch_job").is_some();
                // The accepted switch still owns its candidate and return bookkeeping. Stop forbids
                // the follow-up input; the switch itself has its ordinary Cancel control.
                if !finishing_switch {
                    self.stop_recovery_in(&mut unit, &thread, &row, "stopped_by_person")
                        .await?;
                }
                sqlx::query("UPDATE model_call_recovery SET context=jsonb_set(jsonb_set(context,'{manual_stop}','true'),'{detail}',to_jsonb('stopped_by_person'::text)),due_at=CASE WHEN $3 THEN $2 ELSE due_at END WHERE node_id=$1 AND purpose='agent'")
                    .bind(node).bind(unit.now()).bind(finishing_switch).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            }
            action => {
                if matches!(
                    action,
                    orch_api_contract::calls::CallRecoveryAction::Retry
                        | orch_api_contract::calls::CallRecoveryAction::Continue
                ) {
                    sqlx::query("UPDATE model_call_recovery SET context=jsonb_set(context-'manual_stop','{effects}',$2) WHERE node_id=$1 AND purpose='agent'")
                        .bind(node).bind(json!(action==orch_api_contract::calls::CallRecoveryAction::Continue)).execute(&mut *unit.conn).await.map_err(DbError::from)?;
                }
                let next = if action != orch_api_contract::calls::CallRecoveryAction::RetryNow
                    || matches!(row.state.as_str(), "stopped" | "waiting")
                {
                    json!({"step":{"type":"continue","times":1},"attempt":1})
                } else {
                    row.action.unwrap_or(Json::Null)
                };
                sqlx::query("UPDATE model_call_recovery SET state='scheduled',due_at=$2,updated_at=$2,context=jsonb_set(context-'manual_stop','{manual_request}','true'),action=$3 WHERE node_id=$1 AND purpose='agent'")
                    .bind(node).bind(unit.now()).bind(next).execute(&mut *unit.conn).await.map_err(DbError::from)?;
                unit.wake(node);
            }
        }
        let audit_action = orch_db::audit::action!(
            orch_access_contract::audit::AuditAction::ThreadCallRecoveryChanged
        );
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(principal.user()),
            audit_action.as_str(),
            "thread",
            Some(node),
            "Automatic call recovery changed",
        );
        entry.history = true;
        entry.node_id = Some(node);
        entry.org_id = Some(thread.org_id);
        entry.details =
            json!({"action":action,"input_id":row.context["input_id"],"rule":row.rule["id"]});
        let at = unit.now();
        orch_db::audit::append(unit.conn, &entry, &orch_db::audit::Origin::default(), at).await?;
        let seq = unit.stamp(node).await?;
        threads::set_seq_state(unit.conn, node, seq).await?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        self.kick();
        Ok(())
    }

    /// Keeps a failed agent turn's recovery in its terminal transaction.
    pub(crate) async fn recover_agent_in(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        turn: &TurnRow,
        failure: &Json,
    ) -> Result<Option<Recovery>, EngineError> {
        if thread.current_turn_id != Some(turn.id) {
            return Ok(None);
        }
        let Some(input) = threads::input(unit.conn, turn.input_id).await? else {
            return Ok(None);
        };
        if input.native_action.is_some() {
            return Ok(None);
        }
        let effects: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM items WHERE turn_id=$1 AND ((kind='tool_call') OR (kind='agent_message' AND length(COALESCE(payload->>'text',''))>0)))
            OR EXISTS(SELECT 1 FROM thread_inputs WHERE turn_id=$1 AND id<>$2 AND thread_id=$3
              AND (state='delivered' OR (state='delivering' AND state_detail='steering')))")
            .bind(turn.id).bind(turn.input_id).bind(thread.node_id).fetch_one(&mut *unit.conn).await.map_err(DbError::from)?;
        let previous = read(unit.conn, thread.node_id, Purpose::Agent).await?;
        if previous
            .as_ref()
            .is_some_and(|row| row.state == "stopped" && row.context["manual_stop"] == true)
        {
            return Ok(previous);
        }
        let mut context = json!({"input_id":input.id,"root_input":input.id,"failed_turn":turn.id,"continuation_id":input.continuation_id,
            "author_id":input.author_id,"effects":effects,"provider_id":thread.provider_id,"model":turn.model,
            "choice":choice_of(unit.conn,thread).await?});
        if let Some(row) = previous {
            let inherited = row.state == "running"
                || row.state == "completed"
                    && !Policy::load(unit.conn, thread.node_id).await?.return_next
                    && row.context["choice"] == context["choice"];
            if inherited {
                for key in ["original", "candidate", "candidate_active", "return_next"] {
                    if let Some(value) = row.context.get(key) {
                        context[key] = value.clone();
                    }
                }
                if row.state == "running"
                    && let Some(root) = row.context.get("root_input")
                {
                    context["root_input"] = root.clone();
                }
            }
        }
        let now = unit.now();
        let recovery = reserve(
            unit.conn,
            thread.node_id,
            Purpose::Agent,
            turn.id,
            failure,
            context,
            now,
        )
        .await?;
        Ok(recovery)
    }

    /// Drives due recoveries without retaining an in-memory timer or an open transaction during a switch.
    pub async fn keep_call_recoveries(&self) -> Result<(), EngineError> {
        let mut conn = self.db().acquire().await?;
        let nodes: Vec<Id> = sqlx::query_scalar("SELECT node_id FROM model_call_recovery WHERE purpose='agent' AND state IN ('scheduled','switching') AND due_at<=$1 ORDER BY due_at,node_id LIMIT $2")
            .bind(self.inner.clock.now()).bind(self.config().sweep_batch).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        drop(conn);
        for node in nodes {
            self.drive_call_recovery(node).await?;
        }
        Ok(())
    }

    async fn drive_call_recovery(&self, node: Id) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let Some(thread) = threads::lock(&mut tx, node).await? else {
            return Ok(());
        };
        if thread.current_turn_id.is_some()
            || thread.queue_held
            || thread.deleted_at.is_some()
            || thread.archived_at.is_some()
            || threads::turn_end_work_pending(&mut tx, node).await?
        {
            return Ok(());
        }
        let Some(mut row) = read(&mut tx, node, Purpose::Agent).await?.filter(|row| {
            matches!(row.state.as_str(), "scheduled" | "switching")
                && row.due_at.is_some_and(|due| due <= self.inner.clock.now())
        }) else {
            return Ok(());
        };
        let now = self.inner.clock.now();
        let mut unit = Unit::new(&mut tx, now);
        let input_exists = if let Some(input) = id(&row.context, "input_id") {
            threads::input(unit.conn, input).await?.is_some()
        } else {
            false
        };
        if step(&row).is_none() || !input_exists {
            self.stop_recovery_in(
                &mut unit,
                &thread,
                &row,
                if input_exists {
                    "rule_unavailable"
                } else {
                    "origin_missing"
                },
            )
            .await?;
            let finished = unit.finish().await?;
            self.commit(tx, finished).await?;
            return Ok(());
        }
        if row.state == "scheduled" {
            if row.context.get("choice").is_some()
                && row.context["choice"] != choice_of(unit.conn, &thread).await?
            {
                sqlx::query("UPDATE model_call_recovery SET state='dismissed',due_at=NULL WHERE node_id=$1 AND purpose='agent'").bind(node).execute(&mut *unit.conn).await.map_err(DbError::from)?;
                let seq = unit.stamp(node).await?;
                if thread.blocked_detail.as_deref() == Some("call_recovery") {
                    threads::set_state(
                        unit.conn,
                        node,
                        &[thread_state::BLOCKED],
                        &threads::StateChange::plain(thread_state::IDLE),
                        seq,
                    )
                    .await?;
                }
                unit.wake(node);
                let finished = unit.finish().await?;
                self.commit(tx, finished).await?;
                return Ok(());
            }
            if row.context["manual_request"] != true {
                let policy = Policy::load(unit.conn, node).await?;
                let current = policy
                    .rules
                    .iter()
                    .find(|rule| rule.when.matches(&match_of(&row, Purpose::Agent)));
                let cutoff = history_cutoff(unit.conn, node, now, policy.window).await?;
                let reserved:i64=sqlx::query_scalar("SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1 AND failure_kind=$2 AND automatic AND ($3::timestamptz IS NULL OR at>$3)")
                    .bind(node).bind(row.failure["code"].as_str().unwrap_or("harness_error")).bind(cutoff).fetch_one(&mut *unit.conn).await.map_err(DbError::from)?;
                let reason = if current.is_none_or(|rule| json!(rule) != row.rule) {
                    Some("rules_changed")
                } else if policy.limit == 0 {
                    Some("auto_disabled")
                } else if reserved > policy.limit {
                    Some("repeat_limit_changed")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    self.stop_recovery_in(&mut unit, &thread, &row, reason)
                        .await?;
                    let finished = unit.finish().await?;
                    self.commit(tx, finished).await?;
                    return Ok(());
                }
            }
        }
        if id(&row.context, "continuation_id") != Some(thread.active_continuation_id) {
            self.stop_recovery_in(&mut unit, &thread, &row, "history_changed")
                .await?;
            let finished = unit.finish().await?;
            self.commit(tx, finished).await?;
            return Ok(());
        }
        if row.state == "switching"
            && let Some(job) = id(&row.context, "switch_job")
        {
            let state: Option<String> = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
                .bind(job)
                .fetch_optional(&mut *unit.conn)
                .await
                .map_err(DbError::from)?;
            match state.as_deref() {
                Some("succeeded") => self.queue_recovery_in(&mut unit, &thread, &row).await?,
                Some("queued" | "running") => return Ok(()),
                _ => {
                    self.stop_recovery_in(&mut unit, &thread, &row, "switch_failed")
                        .await?
                }
            }
            let finished = unit.finish().await?;
            self.commit(tx, finished).await?;
            return Ok(());
        }
        let Some(author) = id(&row.context, "author_id") else {
            self.stop_recovery_in(&mut unit, &thread, &row, "author_missing")
                .await?;
            let finished = unit.finish().await?;
            self.commit(tx, finished).await?;
            return Ok(());
        };
        let input = threads::input(
            unit.conn,
            id(&row.context, "input_id")
                .ok_or_else(|| EngineError::unreadable("the recovery origin is unavailable"))?,
        )
        .await?
        .ok_or_else(|| EngineError::unreadable("the recovery origin is unavailable"))?;
        if input.state == "cancelled" {
            sqlx::query("UPDATE model_call_recovery SET state='dismissed',due_at=NULL WHERE node_id=$1 AND purpose='agent'").bind(node).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            let seq = unit.stamp(node).await?;
            if thread.blocked_detail.as_deref() == Some("call_recovery") {
                threads::set_state(
                    unit.conn,
                    node,
                    &[thread_state::BLOCKED],
                    &threads::StateChange::plain(thread_state::IDLE),
                    seq,
                )
                .await?;
            }
            let finished = unit.finish().await?;
            self.commit(tx, finished).await?;
            return Ok(());
        }
        let driving = crate::inputs::input_author_caller(&mut unit, node, &input).await?;
        let serving = crate::inputs::input_caller(&mut unit, &thread, &input).await?;
        let Some((caller, serving)) = driving.zip(serving) else {
            self.stop_recovery_in(&mut unit, &thread, &row, "origin_authority")
                .await?;
            let finished = unit.finish().await?;
            self.commit(tx, finished).await?;
            return Ok(());
        };
        if caller.user() != author {
            return Err(EngineError::unreadable(
                "the recovery origin's author changed",
            ));
        }
        if self
            .check_sender(&mut unit, &caller, &thread, false)
            .await
            .is_err()
        {
            self.stop_recovery_in(&mut unit, &thread, &row, "author_lacks_interact")
                .await?;
        } else if let Some(Step::Failover { candidate }) = step(&row) {
            if !self
                .candidate_allowed(&mut *unit.conn, node, &serving, &candidate, Purpose::Agent)
                .await
                .unwrap_or(false)
            {
                if let Some(input) = id(&row.context, "input_id") {
                    record_step_marker(
                        &mut unit,
                        &thread,
                        &row,
                        "skip",
                        input,
                        Some("candidate_not_allowed"),
                    )
                    .await?;
                }
                skip_step(unit.conn, &row, "candidate_not_allowed", now).await?;
            } else if candidate.provider_id != thread.provider_id.unwrap_or_default()
                || candidate
                    .agent_id
                    .is_some_and(|agent| Some(agent) != thread.agent_id)
            {
                // The ordinary switch owns its history, access checks, blockers and rollback.
                self.apply_candidate(&mut unit, &thread, &mut row, &candidate)
                    .await?;
                sqlx::query("UPDATE model_call_recovery SET state='switching',updated_at=$2 WHERE node_id=$1 AND purpose='agent'").bind(node).bind(now).execute(&mut *unit.conn).await.map_err(DbError::from)?;
                let finished = unit.finish().await?;
                self.commit(tx, finished).await?;
                return self.recovery_switch(node, caller, &row, &candidate).await;
            } else if !candidate_parameters_ready(unit.conn, &thread, &candidate).await? {
                if let Some(input) = id(&row.context, "input_id") {
                    record_step_marker(
                        &mut unit,
                        &thread,
                        &row,
                        "skip",
                        input,
                        Some("candidate_parameters"),
                    )
                    .await?;
                }
                skip_step(unit.conn, &row, "candidate_parameters", now).await?;
            } else {
                self.apply_candidate(&mut unit, &thread, &mut row, &candidate)
                    .await?;
                self.queue_recovery_in(&mut unit, &thread, &row).await?;
            }
        } else if self
            .check_sender(&mut unit, &serving, &thread, true)
            .await
            .is_err()
        {
            self.stop_recovery_in(&mut unit, &thread, &row, "author_lacks_use")
                .await?;
        } else {
            self.queue_recovery_in(&mut unit, &thread, &row).await?;
        }
        let seq = unit.stamp(node).await?;
        threads::set_seq_state(unit.conn, node, seq).await?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Checks current provider, model and optional profile authority for the original caller.
    pub(crate) async fn candidate_allowed(
        &self,
        conn: &mut Conn,
        node: Id,
        caller: &Caller,
        candidate: &Candidate,
        purpose: Purpose,
    ) -> Result<bool, EngineError> {
        let Ok(effort) = orch_provider_contract::calls::requested_effort(&candidate.params) else {
            return Ok(false);
        };
        if !caller.permits(Permission::ProviderUse)
            || purpose == Purpose::Agent
                && candidate.agent_id.is_some()
                && !caller.permits(Permission::AgentUse)
        {
            return Ok(false);
        }
        if purpose == Purpose::Agent {
            let principal = orch_authz::Principal::load(
                conn,
                caller.clone(),
                Time::new(self.inner.clock.now()),
            )
            .await?;
            if !orch_authz::nodes::check(
                conn,
                &principal,
                node,
                Permission::ThreadInteract,
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Ok(false);
            }
        }

        let provider = self
            .inner
            .access
            .provider_use(
                conn,
                &ProviderUse {
                    person: caller.user(),
                    node,
                    provider: candidate.provider_id,
                    model: Some(&candidate.model),
                    effort,
                },
            )
            .await?
            .is_allowed();
        if !provider {
            return Ok(false);
        }
        if purpose == Purpose::Agent
            && let Some(agent) = candidate.agent_id
        {
            return Ok(self
                .inner
                .access
                .agent_use(conn, caller.user(), node, agent)
                .await?
                .is_allowed());
        }
        Ok(true)
    }

    async fn apply_candidate(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        row: &mut Recovery,
        candidate: &Candidate,
    ) -> Result<(), EngineError> {
        if row.context.get("original").is_none() {
            let settings = crate::resolved::load(unit.conn, thread.node_id).await?;
            let provider = if let Some(provider) = thread.provider_id {
                orch_db::queries::resources::thread_engine::provider(unit.conn, provider).await?
            } else {
                None
            };
            let model = if let (Some(provider), Some(model)) =
                (thread.provider_id, settings.model.as_deref())
            {
                orch_db::queries::resources::thread_engine::model(unit.conn, provider, model)
                    .await?
            } else {
                None
            };
            row.context["original"] = json!({"provider_id":thread.provider_id,"agent_id":thread.agent_id,"model":settings.model,"effort":settings.effort,"params":settings.params,"failure_kind":row.failure["code"],"rule_id":row.rule["id"],"provider_revision":provider.as_ref().map(|provider|provider.revision),"model_parameters":model.as_ref().map(|model|&model.parameters),"model_enabled":model.as_ref().map(|model|model.enabled)});
            row.context["return_next"] =
                json!(Policy::load(unit.conn, thread.node_id).await?.return_next);
        }
        row.context["candidate"] = json!(candidate);
        sqlx::query(
            "UPDATE model_call_recovery SET context=$2 WHERE node_id=$1 AND purpose='agent'",
        )
        .bind(thread.node_id)
        .bind(&row.context)
        .execute(&mut *unit.conn)
        .await
        .map_err(DbError::from)?;
        Ok(())
    }

    async fn queue_recovery_in(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        row: &Recovery,
    ) -> Result<(), EngineError> {
        if row.context["returning"] == true {
            sqlx::query("UPDATE model_call_recovery SET state='completed',due_at=NULL,context=context-'candidate'-'original'-'returning'-'switch_job' WHERE node_id=$1 AND purpose='agent'")
                .bind(thread.node_id).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            if let Some(input) = id(&row.context, "input_id") {
                record_step_marker(unit, thread, row, "return", input, None).await?;
            }
            unit.wake(thread.node_id);
            return Ok(());
        }
        if row.context["manual_stop"] == true {
            // A switch accepted before Stop may finish. Retain the actual selected choice so a later
            // independent input can return normally, without deriving another input for this failure.
            let choice = choice_of(unit.conn, thread).await?;
            sqlx::query("UPDATE model_call_recovery SET state='completed',due_at=NULL,context=jsonb_set(jsonb_set(context-'switch_job','{candidate_active}','true'),'{choice}',$3),updated_at=$2 WHERE node_id=$1 AND purpose='agent'")
                .bind(thread.node_id).bind(unit.now()).bind(choice).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            if let Some(input) = id(&row.context, "input_id") {
                record_step_marker(unit, thread, row, "failover", input, None).await?;
            }
            unit.wake(thread.node_id);
            return Ok(());
        }
        let Some(input_id) = id(&row.context, "input_id") else {
            return Err(EngineError::unreadable("recovery input is missing"));
        };
        let Some(input) = threads::input(unit.conn, input_id).await? else {
            return Err(EngineError::unreadable("recovery input is unavailable"));
        };
        if input.state == "cancelled" {
            sqlx::query("UPDATE model_call_recovery SET state='dismissed',due_at=NULL WHERE node_id=$1 AND purpose='agent'").bind(thread.node_id).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            let seq = unit.stamp(thread.node_id).await?;
            if thread.blocked_detail.as_deref() == Some("call_recovery") {
                threads::set_state(
                    unit.conn,
                    thread.node_id,
                    &[thread_state::BLOCKED],
                    &threads::StateChange::plain(thread_state::IDLE),
                    seq,
                )
                .await?;
            }
            return Ok(());
        }
        if self
            .check_input_author_driving(unit, thread, &input)
            .await
            .is_err()
        {
            self.stop_recovery_in(unit, thread, row, "author_lacks_interact")
                .await?;
            return Ok(());
        }
        let Some(serving) = crate::inputs::input_caller(unit, thread, &input).await? else {
            self.stop_recovery_in(unit, thread, row, "origin_authority")
                .await?;
            return Ok(());
        };
        // A newly selected overlay is not active until this input is queued. Check its real destination here.
        let allowed = if let Some(candidate) = row.context.get("candidate") {
            let candidate: Candidate = serde_json::from_value(candidate.clone())?;
            self.candidate_allowed(
                unit.conn,
                thread.node_id,
                &serving,
                &candidate,
                Purpose::Agent,
            )
            .await?
        } else {
            self.check_sender(unit, &serving, thread, true)
                .await
                .is_ok()
        };
        if !allowed {
            self.stop_recovery_in(unit, thread, row, "author_lacks_use")
                .await?;
            return Ok(());
        }
        let effect = row.context["effects"].as_bool().unwrap_or(true);
        let actual = if effect { "continue" } else { "retry" };
        let mut recovered_input = input.id;
        if effect {
            let candidate = row
                .context
                .get("candidate")
                .map(|value| serde_json::from_value(value.clone()))
                .transpose()?;
            let next = self
                .continue_original_input_in(unit, thread, &input, row.reference, candidate.as_ref())
                .await?;
            if let Some(next) = next {
                recovered_input = next;
                sqlx::query("UPDATE model_call_recovery SET context=jsonb_set(context,'{input_id}',$2) WHERE node_id=$1 AND purpose='agent'")
                    .bind(thread.node_id).bind(json!(next)).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            }
        } else {
            let provenance = crate::inputs::derived_input_provenance(&input, None)?;
            let seq = unit.stamp(thread.node_id).await?;
            sqlx::query("UPDATE thread_inputs SET state='pending',state_detail=NULL,turn_id=NULL,delivered_at=NULL,seq_updated=$2,author_id=$3,submission_authority=$4,run_as=$5,run_as_authority=$6 WHERE id=$1 AND state IN ('pending','delivered','failed','delivering')")
                .bind(input.id).bind(seq).bind(provenance.author_id).bind(provenance.submission_authority.as_ref()).bind(provenance.run_as).bind(provenance.run_as_authority.as_ref()).execute(&mut *unit.conn).await.map_err(DbError::from)?;
        }
        let choice = choice_of(unit.conn, thread).await?;
        sqlx::query("UPDATE model_call_recovery SET state='running',due_at=NULL,context=jsonb_set(jsonb_set(context-'manual_request','{candidate_active}','true'),'{choice}',$3),updated_at=$2 WHERE node_id=$1 AND purpose='agent'")
            .bind(thread.node_id).bind(unit.now()).bind(choice).execute(&mut *unit.conn).await.map_err(DbError::from)?;
        let seq = unit.stamp(thread.node_id).await?;
        threads::set_state(
            unit.conn,
            thread.node_id,
            &[thread_state::BLOCKED, thread_state::IDLE],
            &threads::StateChange::plain(thread_state::IDLE),
            seq,
        )
        .await?;
        record_step_marker(
            unit,
            thread,
            row,
            if step(row).is_some_and(|step| matches!(step, Step::Failover { .. })) {
                "failover"
            } else {
                actual
            },
            recovered_input,
            None,
        )
        .await?;
        unit.wake(thread.node_id);
        Ok(())
    }

    async fn stop_recovery_in(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        row: &Recovery,
        detail: &str,
    ) -> Result<(), EngineError> {
        sqlx::query("UPDATE model_call_recovery SET state='stopped',due_at=NULL,context=jsonb_set(context,'{detail}',to_jsonb($3::text)),updated_at=$2 WHERE node_id=$1 AND purpose='agent' AND reference=$4")
            .bind(thread.node_id).bind(unit.now()).bind(detail).bind(row.reference).execute(&mut *unit.conn).await.map_err(DbError::from)?;
        let seq = unit.stamp(thread.node_id).await?;
        threads::set_state(
            unit.conn,
            thread.node_id,
            &[thread_state::IDLE, thread_state::BLOCKED],
            &threads::StateChange::blocked("error", Some("call_recovery")),
            seq,
        )
        .await?;
        Ok(())
    }

    async fn recovery_switch(
        &self,
        node: Id,
        caller: Caller,
        row: &Recovery,
        candidate: &Candidate,
    ) -> Result<(), EngineError> {
        let request: orch_api_contract::forks::ForkRequest = serde_json::from_value(
            json!({"result":"replace","to":{"provider_id":candidate.provider_id,"agent_id":candidate.agent_id,"model":candidate.model,"effort":candidate.params.get("effort")},"history":{"mode":"all"}}),
        )?;
        let mut tx = self.db().begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let Some(thread) = threads::lock(&mut tx, node).await? else {
            return Ok(());
        };
        let current = read(&mut tx, node, Purpose::Agent).await?;
        if !current.as_ref().is_some_and(|current| {
            current.reference == row.reference
                && current.state == "switching"
                && current.context.get("switch_job").is_none()
        }) {
            return Ok(());
        }
        let result = async {
            let draft = self
                .draft_in(
                    &mut tx,
                    &caller,
                    node,
                    &request,
                    crate::fork_plan::Reach::Seen,
                    None,
                )
                .await
                .map_err(EngineError::unreadable)?;
            if !draft.is_clear() {
                return Err(EngineError::unreadable(
                    "The configured switch has blockers",
                ));
            }
            self.start_fork_job(
                &mut tx,
                &caller,
                &thread,
                draft,
                None,
                crate::ids::derived("call.switch", &[row.reference.as_bytes()]),
            )
            .await
            .map_err(EngineError::unreadable)
        }
        .await;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        match result {
            Ok(job) => {
                let mut context = row.context.clone();
                context["switch_job"] = json!(job.id);
                context["candidate"] = json!(candidate);
                sqlx::query("UPDATE model_call_recovery SET context=$2,due_at=$3 WHERE node_id=$1 AND purpose='agent' AND reference=$4")
                    .bind(node).bind(context).bind(unit.now()).bind(row.reference).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            }
            Err(_) => {
                self.stop_recovery_in(&mut unit, &thread, row, "switch_blocked")
                    .await?
            }
        }
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        self.kick();
        Ok(())
    }
}
