//! Durable provider observation timing, discovered model contexts and confirmed reset receipts.

use crate::{Conn, DbError};
use chrono::{DateTime, Duration, Utc};
use orch_types::{Id, Json};

/// A vendor reset is observed once its minute of settling has elapsed, even at the idle cadence.
/// The attempt clock prevents a vendor's unchanged, past reset time from causing repeated reads.
const RESET_READ_DUE: &str = "EXISTS (
    SELECT 1 FROM (
        SELECT fact ->> 'resets_at' AS at
        FROM jsonb_array_elements(CASE WHEN jsonb_typeof(p.facts->'windows')='array' THEN p.facts->'windows' ELSE '[]'::jsonb END) AS vendor_windows(fact)
        UNION ALL SELECT fact ->> 'resets_at'
        FROM jsonb_array_elements(CASE WHEN jsonb_typeof(p.facts->'spend')='array' THEN p.facts->'spend' ELSE '[]'::jsonb END) AS source_spend(fact)
        UNION ALL SELECT p.facts #>> '{limit_reached,resets_at}'
    ) resets
    WHERE NULLIF(resets.at,'')::timestamptz + interval '1 minute' <= $1
      AND COALESCE(p.facts_requested_at,p.facts_at,p.created_at)
          < NULLIF(resets.at,'')::timestamptz + interval '1 minute'
)";

// Decimal comparison avoids overflowing an interval for a large, valid source-selected minimum.
// A source without a learned minimum adds no restriction to the deployment's own cadence.
const SOURCE_INTERVAL_DUE: &str = "(p.facts_requested_at IS NULL OR
    EXTRACT(EPOCH FROM ($1-p.facts_requested_at)) >= CASE
      WHEN p.sources #>> '{facts,descriptor,facts,min_interval_secs}' ~ '^[0-9]+$'
      THEN GREATEST(60,(p.sources #>> '{facts,descriptor,facts,min_interval_secs}')::numeric)
      ELSE 0 END)";

/// Changes choices in the management transaction and makes the new source's first read immediate.
/// Learned metadata is kept separately below; only a manager's selection changes this revision.
pub async fn set_sources(
    conn: &mut Conn,
    provider: Id,
    expected_revision: i64,
    sources: &Json,
    facts_changed: bool,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE providers SET sources=$3,facts_requested_at=CASE WHEN $4 THEN NULL ELSE facts_requested_at END,revision=revision+1,updated_at=$5 WHERE id=$1 AND revision=$2 AND deleted_at IS NULL")
        .bind(provider).bind(expected_revision).bind(sources).bind(facts_changed).bind(now).execute(conn).await?.rows_affected()>0)
}

/// Keeps a learned descriptor and a scrubbed error only for the unchanged source choice.
/// A late answer from a replaced address/program may not overwrite the new choice's metadata.
pub async fn source_observed(
    conn: &mut Conn,
    provider: Id,
    element: &str,
    expected: &Json,
    descriptor: Option<&Json>,
    error: Option<&Json>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    if !matches!(element, "facts" | "models") {
        return Ok(false);
    }
    let mut patch = serde_json::json!({"last_error":error});
    if let Some(descriptor) = descriptor {
        patch["descriptor"] = descriptor.clone();
        patch["described_at"] = serde_json::json!(orch_types::Time::new(now));
    }
    Ok(sqlx::query("UPDATE providers SET sources=jsonb_set(sources,ARRAY[$2],COALESCE(NULLIF(sources->$2,'null'::jsonb),jsonb_build_object('by','default'))||$4,true),updated_at=$5 WHERE id=$1 AND deleted_at IS NULL AND COALESCE(NULLIF(sources->$2,'null'::jsonb),'{}'::jsonb)=$3")
        .bind(provider).bind(element).bind(expected).bind(patch).bind(now).execute(conn).await?.rows_affected()>0)
}

/// Deployment-selected source-read intervals, independent of the vendor's own clock.
#[derive(Debug, Clone, Copy)]
pub struct ReadSchedule {
    /// How far back bound-thread work keeps a provider active.
    pub recent: Duration,
    /// The interval while work was recent.
    pub active: Duration,
    /// The interval without recent work.
    pub idle: Duration,
    /// The recovery interval while a credential is refused.
    pub refused: Duration,
}

/// Admits one ordinary reader per interval across processes, including attempts that tell nothing.
/// A changed credential bypasses the interval; commit this claim before asking its runtime.
pub async fn claim_read(
    conn: &mut Conn,
    provider: Id,
    force: bool,
    now: DateTime<Utc>,
    interval: Duration,
) -> Result<bool, DbError> {
    // This query's first argument is the observation clock, shared with the due-reader condition.
    let sql = format!(
        "UPDATE providers p SET facts_requested_at = $1 WHERE id = $2 AND deleted_at IS NULL
        AND ($3 OR facts_requested_at IS NULL OR facts_requested_at <= $4 OR {RESET_READ_DUE})
        AND {SOURCE_INTERVAL_DUE}"
    );
    // Only this module's constants are put into the statement.
    Ok(sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(now)
        .bind(provider)
        .bind(force)
        .bind(now - interval)
        .execute(conn)
        .await?
        .rows_affected()
        > 0)
}

/// Starting a stopped unit did not read its source; its eventual first read must not spend that interval.
/// The matching clock prevents this cleanup from removing a real read claimed by another caller.
pub async fn discard_unread_claim(
    conn: &mut Conn,
    provider: Id,
    claimed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE providers SET facts_requested_at=NULL WHERE id=$1 AND facts_requested_at=$2 AND (facts_at IS NULL OR facts_at<$2)")
        .bind(provider).bind(claimed_at).execute(conn).await?;
    Ok(())
}

/// The credential's current end, rechecked under authority before an expiry job changes availability.
pub async fn credential_end(
    conn: &mut Conn,
    provider: Id,
) -> Result<Option<DateTime<Utc>>, DbError> {
    Ok(sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT (facts #>> '{credential,expires_at}')::timestamptz
        FROM providers WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(provider)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// Providers whose online runtime is due an observation at its cadence or a minute after a vendor reset.
pub async fn due_reads(
    conn: &mut Conn,
    now: DateTime<Utc>,
    schedule: ReadSchedule,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    let interval = |duration: Duration| sqlx::postgres::types::PgInterval {
        months: 0,
        days: 0,
        microseconds: duration.num_microseconds().unwrap_or_default(),
    };
    let sql = format!("SELECT p.id FROM providers p
         LEFT JOIN machines m ON m.id = p.key_machine_id
         WHERE p.deleted_at IS NULL AND p.status <> 'disabled'
           AND ((m.deleted_at IS NULL AND m.status = 'active' AND m.online)
             OR (p.machine_id IS NULL AND EXISTS (
                 SELECT 1 FROM runtimes r JOIN hosts h ON h.id = r.host_id JOIN machines hm ON hm.id = h.machine_id
                 WHERE r.provider_id = p.id AND r.state = 'stopped' AND r.successor_of IS NULL
                   AND hm.online AND hm.status = 'active' AND hm.deleted_at IS NULL)))
           AND (p.facts_requested_at IS NULL OR COALESCE(p.facts_requested_at,p.facts_at,p.created_at) <= $1 - CASE
             WHEN p.status = 'error' THEN $5::interval
             WHEN EXISTS (SELECT 1 FROM threads th JOIN turns t ON t.thread_id = th.node_id
               WHERE th.provider_id = p.id AND (t.state = 'running' OR GREATEST(t.started_at,t.completed_at) >= $2))
             OR (p.status='exhausted' AND EXISTS (
               SELECT 1 FROM threads th JOIN nodes n ON n.id=th.node_id
               JOIN thread_inputs i ON i.thread_id=th.node_id
               WHERE th.provider_id=p.id AND n.deleted_at IS NULL AND n.archived_at IS NULL
                 AND i.state IN ('pending','delivering')))
             THEN $3::interval ELSE $4::interval END OR {RESET_READ_DUE})
           AND {SOURCE_INTERVAL_DUE}
         ORDER BY COALESCE(p.facts_requested_at,p.facts_at,p.created_at),p.id LIMIT $6");
    // Only this module's constants are put into the statement.
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(now)
        .bind(now - schedule.recent)
        .bind(interval(schedule.active))
        .bind(interval(schedule.idle))
        .bind(interval(schedule.refused))
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Fills an accepted model's missing native context and never replaces an explicit context.
pub async fn learn_model_window(
    conn: &mut Conn,
    provider: Id,
    model: &str,
    window: i64,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE provider_models SET parameters = jsonb_set(parameters,'{context_window}',to_jsonb($3::bigint)), updated_at = $4
        WHERE provider_id = $1 AND model_name = $2 AND parameters ->> 'context_window' IS NULL AND $3 > 0")
        .bind(provider).bind(model).bind(window).bind(at).execute(conn).await?.rows_affected() > 0)
}

/// Locks the provider before comparing a reset confirmation with its current facts.
pub async fn lock_provider(conn: &mut Conn, provider: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar::<_, Id>(
        "SELECT id FROM providers WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(provider)
    .fetch_optional(conn)
    .await?
    .is_some())
}

/// The receipt of one person's logical reset request, kept in the append-only audit trail.
pub async fn reset_receipt(
    conn: &mut Conn,
    provider: Id,
    user: Id,
    request: Id,
) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar("SELECT details FROM audit_log WHERE action = 'provider.reset_used' AND object_id = $1 AND actor_id = $2
        AND details ->> 'request_id' = $3 ORDER BY at DESC,id DESC LIMIT 1")
        .bind(provider).bind(user).bind(request.to_string()).fetch_optional(conn).await?)
}

/// A reset whose resulting windows have not begun again yet.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LastReset {
    /// The provider.
    pub provider_id: Id,
    /// The person who confirmed it.
    pub user_id: Id,
    /// The credit the vendor used.
    pub reset_id: String,
    /// When it was used.
    pub at: DateTime<Utc>,
    /// The actual post-use cycle snapshot, not a guessed expiry.
    pub windows_after: Json,
    /// The provider's current reported windows.
    pub windows_current: Json,
}

/// Last reset users for a batch of providers, without exposing a credential.
pub async fn last_resets(
    conn: &mut Conn,
    providers: &[Id],
    now: DateTime<Utc>,
) -> Result<Vec<LastReset>, DbError> {
    let candidates: Vec<LastReset> = sqlx::query_as(
        "SELECT DISTINCT ON (a.object_id) a.object_id AS provider_id,
        a.actor_id AS user_id,a.details ->> 'reset_id' AS reset_id,a.at,
        COALESCE(a.details -> 'windows_after','[]'::jsonb) AS windows_after,
        COALESCE(p.facts -> 'windows','[]'::jsonb) AS windows_current
        FROM audit_log a JOIN providers p ON p.id = a.object_id
        WHERE a.action = 'provider.reset_used' AND a.object_id = ANY($1) AND a.actor_kind = 'user'
        ORDER BY a.object_id,a.at DESC,a.id DESC",
    )
    .bind(providers)
    .fetch_all(conn)
    .await?;
    Ok(candidates
        .into_iter()
        .filter(|receipt| {
            let after = receipt.windows_after.as_array();
            let current = receipt.windows_current.as_array();
            // No cycle was reported: retain the receipt as dated, without claiming a made-up reset moment.
            after.is_none_or(|after| {
                after.is_empty()
                    || after.iter().any(|old| {
                        let reported = current.and_then(|windows| {
                            windows
                                .iter()
                                .find(|window| window.get("id") == old.get("id"))
                        });
                        let reset = |window: &Json| {
                            window
                                .get("resets_at")
                                .and_then(Json::as_str)
                                .and_then(|time| time.parse::<orch_types::Time>().ok())
                                .map(orch_types::Time::get)
                        };
                        if let Some(end) = reset(old) {
                            return now < end;
                        }
                        let Some(window) = reported else {
                            return true;
                        };
                        if window.get("reset_not_started").and_then(Json::as_bool) == Some(true) {
                            return true;
                        }
                        let begun = reset(window)
                            .zip(window.get("duration_minutes").and_then(Json::as_i64))
                            .and_then(|(end, minutes)| {
                                Duration::try_minutes(minutes)
                                    .and_then(|length| end.checked_sub_signed(length))
                            });
                        begun.is_none_or(|begun| begun <= receipt.at)
                    })
            })
        })
        .collect())
}

/// Independent state and read-attempt clocks for a batch of provider pages.
pub async fn observation_times(
    conn: &mut Conn,
    providers: &[Id],
) -> Result<Vec<(Id, DateTime<Utc>, Option<DateTime<Utc>>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id,status_at,facts_requested_at FROM providers WHERE id = ANY($1) AND deleted_at IS NULL",
    )
    .bind(providers)
    .fetch_all(conn)
    .await?)
}

/// Claims one oldest waiting input while the caller holds the provider's row and current management.
/// An unfinished trial keeps its same input; another turn needs a later explicit manager act.
pub async fn claim_trial(
    conn: &mut Conn,
    provider: Id,
    now: DateTime<Utc>,
) -> Result<Option<(Id, Id)>, DbError> {
    let active = trial_pending(conn, provider).await?;
    if active {
        return Ok(None);
    }
    let next:Option<(Id,Id)> = sqlx::query_as("SELECT i.id,i.thread_id FROM thread_inputs i
        JOIN threads th ON th.node_id = i.thread_id JOIN nodes n ON n.id = th.node_id
        WHERE th.provider_id = $1 AND th.current_turn_id IS NULL AND th.state = 'blocked'
          AND th.blocked_reason = 'provider_unavailable' AND i.state = 'pending' AND i.state_detail IS NULL
          AND n.deleted_at IS NULL AND n.archived_at IS NULL
        ORDER BY i.created_at,i.position,i.id LIMIT 1").bind(provider).fetch_optional(&mut *conn).await?;
    let Some((input, thread)) = next else {
        return Ok(None);
    };
    let claimed = sqlx::query("UPDATE providers SET credential_trial_input_id = $2,credential_trial_turn_id = NULL,
        revision = revision+1,updated_at = $3 WHERE id = $1 AND status = 'error' AND deleted_at IS NULL")
        .bind(provider).bind(input).bind(now).execute(conn).await?.rows_affected() > 0;
    Ok(claimed.then_some((input, thread)))
}

/// Whether this exact input may start or resume the manager's single credential trial.
pub async fn trial_allowed(conn: &mut Conn, provider: Id, input: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM providers p WHERE p.id = $1
        AND p.status = 'error' AND p.credential_trial_input_id = $2
        AND (p.credential_trial_turn_id IS NULL OR EXISTS (
            SELECT 1 FROM turns t WHERE t.id = p.credential_trial_turn_id AND t.input_id = $2 AND t.state IN ('starting','running'))))")
        .bind(provider).bind(input).fetch_one(conn).await?)
}

/// Consumes a trial by attaching the one turn it created; its later completion ends eligibility.
pub async fn bind_trial_turn(
    conn: &mut Conn,
    provider: Id,
    input: Id,
    turn: Id,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE providers SET credential_trial_turn_id = $3 WHERE id = $1
        AND credential_trial_input_id = $2 AND credential_trial_turn_id IS NULL",
    )
    .bind(provider)
    .bind(input)
    .bind(turn)
    .execute(conn)
    .await?;
    Ok(())
}

/// The reserved safe body of one confirmed reset in the existing durable command journal.
pub async fn reset_request(conn: &mut Conn, command: Id) -> Result<Option<Json>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT body FROM commands WHERE id = $1 AND kind = 'provider.reset'")
            .bind(command)
            .fetch_optional(conn)
            .await?,
    )
}

/// The current provider binding and credential generation, without exposing any credential material.
/// The sealed-value digest also distinguishes two rotations made at the same stepped clock time.
pub async fn credential_epoch(
    conn: &mut Conn,
    provider: Id,
    revision: Option<i64>,
) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_array(p.revision,p.machine_id,p.key_machine_id,p.agent_id,p.login_state,
        COALESCE((SELECT jsonb_agg(jsonb_build_array(s.id,s.name,s.rotated_at,s.created_at,md5(s.sealed)) ORDER BY s.name)
          FROM secrets s WHERE s.owner_kind = 'provider' AND s.owner_id = p.id),'[]'::jsonb))
        FROM providers p WHERE p.id = $1 AND p.deleted_at IS NULL AND ($2::bigint IS NULL OR p.revision = $2)")
        .bind(provider).bind(revision).fetch_optional(conn).await?)
}

/// Whether a manager's one reserved input or turn is still pending.
pub async fn trial_pending(conn: &mut Conn, provider: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS (
        SELECT 1 FROM providers p JOIN thread_inputs i ON i.id = p.credential_trial_input_id
        JOIN threads th ON th.node_id = i.thread_id LEFT JOIN turns t ON t.id = p.credential_trial_turn_id
        WHERE p.id = $1 AND th.provider_id = p.id AND
          ((p.credential_trial_turn_id IS NULL AND i.state IN ('pending','delivering')) OR t.state IN ('starting','running')))")
        .bind(provider).fetch_one(conn).await?)
}
