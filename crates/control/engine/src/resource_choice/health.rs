//! Immutable native failure epochs, pool-local policy and fenced recovery attempts. No probes run here.
use super::{Choice, Hop, Leaf};
use crate::EngineError;
use chrono::{DateTime, Duration, Utc};
use orch_db::{Conn, DbError};
use orch_provider_contract::failure::{Classification, Evidence};
use orch_types::{Id, Json};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    identity: Json,
    credential: bool,
}
impl Scope {
    /// Use the generation captured by the actual native request, never a newly looked-up secret.
    pub fn credential(provider: Id, generation: Json) -> Self {
        Self {
            identity: serde_json::json!({"provider":provider,"generation":generation,"kind":"credential"}),
            credential: true,
        }
    }
    pub fn member(provider: Id, generation: Json, member: Json) -> Self {
        Self {
            identity: serde_json::json!({"provider":provider,"generation":generation,"kind":"member","member":member}),
            credential: false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureOrigin {
    Vendor,
    CredentialKeeper,
    Local,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suppression {
    Ready,
    Cooling { epoch: i64, until: DateTime<Utc> },
    TrialReady { epoch: i64 },
    TrialInProgress { epoch: i64 },
}
#[derive(sqlx::FromRow)]
struct Epoch {
    id: Id,
    failure_epoch: i64,
    succeeded_epoch: i64,
}
fn bad(message: impl Into<String>) -> EngineError {
    EngineError::unreadable(message)
}
async fn lookup(conn: &mut Conn, scope: &Scope) -> Result<Option<Epoch>, EngineError> {
    Ok(sqlx::query_as("SELECT id,failure_epoch,succeeded_epoch FROM pool_failure_scopes WHERE identity_hash=md5($1::jsonb::text) AND identity=$1").bind(&scope.identity).fetch_optional(conn).await.map_err(DbError::from)?)
}
async fn locked(conn: &mut Conn, scope: &Scope) -> Result<Epoch, EngineError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('pool-failure/'||$1::jsonb::text,0))",
    )
    .bind(&scope.identity)
    .execute(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if lookup(conn, scope).await?.is_none() {
        sqlx::query("INSERT INTO pool_failure_scopes(id,identity) VALUES($1,$2)")
            .bind(orch_types::new_id())
            .bind(&scope.identity)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
    }
    Ok(sqlx::query_as("SELECT id,failure_epoch,succeeded_epoch FROM pool_failure_scopes WHERE identity_hash=md5($1::jsonb::text) AND identity=$1 FOR UPDATE").bind(&scope.identity).fetch_one(conn).await.map_err(DbError::from)?)
}
pub async fn failure(
    conn: &mut Conn,
    scope: &Scope,
    attempt: Id,
    origin: FailureOrigin,
    classification: &Classification,
    at: DateTime<Utc>,
    reset: Option<DateTime<Utc>>,
) -> Result<bool, EngineError> {
    if origin == FailureOrigin::Local || matches!(classification.evidence, Evidence::Unknown) {
        return Ok(false);
    }
    let supported = matches!(
        classification.code.as_str(),
        "provider_unauthorized"
            | "provider_busy"
            | "rate_limited"
            | "overloaded"
            | "provider_exhausted"
            | "provider_unreachable"
    ) || classification.code == "model_unavailable" && !scope.credential;
    if !supported {
        return Ok(false);
    }
    let row = locked(conn, scope).await?;
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pool_failures WHERE scope_id=$1 AND attempt_id=$2)",
    )
    .bind(row.id)
    .bind(attempt)
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if duplicate {
        return Ok(false);
    }
    let epoch = row
        .failure_epoch
        .checked_add(1)
        .ok_or_else(|| bad("the failure epoch is exhausted"))?;
    sqlx::query("INSERT INTO pool_failures(scope_id,attempt_id,epoch,occurred_at,reset_at,classification) VALUES($1,$2,$3,$4,$5,$6)").bind(row.id).bind(attempt).bind(epoch).bind(at).bind(reset).bind(serde_json::to_value(classification)?).execute(&mut *conn).await.map_err(DbError::from)?;
    sqlx::query("UPDATE pool_failure_scopes SET failure_epoch=$2 WHERE id=$1")
        .bind(row.id)
        .bind(epoch)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(true)
}
pub async fn epoch(conn: &mut Conn, scope: &Scope) -> Result<i64, EngineError> {
    Ok(lookup(conn, scope)
        .await?
        .map_or(0, |row| row.failure_epoch))
}
pub async fn success(
    conn: &mut Conn,
    scope: &Scope,
    answers: i64,
    attempt: Id,
    now: DateTime<Utc>,
) -> Result<bool, EngineError> {
    let row = locked(conn, scope).await?;
    sqlx::query("UPDATE pool_recovery_trials SET settled_at=$3 WHERE scope_id=$1 AND attempt_id=$2 AND settled_at IS NULL").bind(row.id).bind(attempt).bind(now).execute(&mut *conn).await.map_err(DbError::from)?;
    if row.failure_epoch != answers {
        return Ok(false);
    }
    sqlx::query("UPDATE pool_failure_scopes SET succeeded_epoch=$2 WHERE id=$1")
        .bind(row.id)
        .bind(answers)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(true)
}
async fn setting(conn: &mut Conn, pool: Id, key: &str) -> Result<u64, EngineError> {
    orch_settings::get(conn, orch_settings::Place::Pool(pool), key)
        .await
        .map_err(|error| bad(error.to_string()))?
        .ok_or_else(|| bad(format!("the pool setting {key} has no default")))
}
pub async fn state(
    conn: &mut Conn,
    pool: Id,
    scope: &Scope,
    now: DateTime<Utc>,
) -> Result<Suppression, EngineError> {
    let Some(row) = lookup(conn, scope).await? else {
        return Ok(Suppression::Ready);
    };
    let clear: Option<i64> = sqlx::query_scalar(
        "SELECT epoch FROM pool_failure_clears WHERE pool_id=$1 AND scope_id=$2",
    )
    .bind(pool)
    .bind(row.id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let base = row.succeeded_epoch.max(clear.unwrap_or(0));
    if row.failure_epoch <= base {
        return Ok(Suppression::Ready);
    }
    let threshold = i64::try_from(setting(conn, pool, "pools.failures_before_cooldown").await?)
        .map_err(|error| bad(error.to_string()))?;
    let window = i64::try_from(setting(conn, pool, "pools.failure_window_secs").await?)
        .map_err(|error| bad(error.to_string()))?;
    let triggered:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pool_failures f WHERE f.scope_id=$1 AND f.epoch>$2 AND (SELECT count(*) FROM pool_failures recent WHERE recent.scope_id=f.scope_id AND recent.epoch>$2 AND recent.epoch<=f.epoch AND recent.occurred_at<=f.occurred_at AND recent.occurred_at>=f.occurred_at-make_interval(secs=>$3::double precision))>=$4)").bind(row.id).bind(base).bind(window).bind(threshold).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    if !triggered {
        return Ok(Suppression::Ready);
    }
    let (last, reset): (DateTime<Utc>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT max(occurred_at),max(reset_at) FROM pool_failures WHERE scope_id=$1 AND epoch>$2",
    )
    .bind(row.id)
    .bind(base)
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let minutes = i64::try_from(setting(conn, pool, "pools.cooldown_minutes").await?)
        .map_err(|error| bad(error.to_string()))?;
    let duration = Duration::try_minutes(minutes)
        .ok_or_else(|| bad("the cooldown exceeds its time representation"))?;
    let until = last
        .checked_add_signed(duration)
        .ok_or_else(|| bad("the cooldown exceeds its time representation"))?;
    let until = reset.map_or(until, |reset| reset.max(until));
    if now < until {
        return Ok(Suppression::Cooling {
            epoch: row.failure_epoch,
            until,
        });
    }
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pool_recovery_trials WHERE scope_id=$1 AND settled_at IS NULL",
    )
    .bind(row.id)
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if u64::try_from(active).map_err(|error| bad(error.to_string()))?
        >= setting(conn, pool, "pools.recovery_trials").await?
    {
        Ok(Suppression::TrialInProgress {
            epoch: row.failure_epoch,
        })
    } else {
        Ok(Suppression::TrialReady {
            epoch: row.failure_epoch,
        })
    }
}
/// Call only at the actual native attempt's admission fence. Replaying that attempt owns its slot.
pub async fn claim(
    conn: &mut Conn,
    pool: Id,
    scope: &Scope,
    attempt: Id,
    expected: i64,
    now: DateTime<Utc>,
) -> Result<bool, EngineError> {
    let row = locked(conn, scope).await?;
    if row.failure_epoch != expected {
        return Ok(false);
    }
    let existing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pool_recovery_trials WHERE scope_id=$1 AND failure_epoch=$2 AND attempt_id=$3 AND settled_at IS NULL)").bind(row.id).bind(expected).bind(attempt).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    if existing {
        return Ok(true);
    }
    if !matches!(state(conn,pool,scope,now).await?,Suppression::TrialReady{epoch} if epoch==expected)
    {
        return Ok(false);
    }
    let slot:i64=sqlx::query_scalar("SELECT COALESCE(max(slot),-1)+1 FROM pool_recovery_trials WHERE scope_id=$1 AND failure_epoch=$2 AND settled_at IS NULL").bind(row.id).bind(expected).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    sqlx::query("INSERT INTO pool_recovery_trials(scope_id,failure_epoch,attempt_id,pool_id,slot,created_at) VALUES($1,$2,$3,$4,$5,$6)").bind(row.id).bind(expected).bind(attempt).bind(pool).bind(slot).bind(now).execute(conn).await.map_err(DbError::from)?;
    Ok(true)
}
pub async fn settle(
    conn: &mut Conn,
    scope: &Scope,
    attempt: Id,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    if lookup(conn, scope).await?.is_some() {
        let row = locked(conn, scope).await?;
        sqlx::query("UPDATE pool_recovery_trials SET settled_at=$3 WHERE scope_id=$1 AND attempt_id=$2 AND settled_at IS NULL").bind(row.id).bind(attempt).bind(now).execute(conn).await.map_err(DbError::from)?;
    }
    Ok(())
}
/// Releases only this pool's preference, retaining shared facts and every outstanding native claim.
pub async fn clear(
    conn: &mut Conn,
    pool: Id,
    scope: &Scope,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let row = locked(conn, scope).await?;
    sqlx::query("INSERT INTO pool_failure_clears(pool_id,scope_id,epoch,cleared_at) VALUES($1,$2,$3,$4) ON CONFLICT(pool_id,scope_id) DO UPDATE SET epoch=excluded.epoch,cleared_at=excluded.cleared_at").bind(pool).bind(row.id).bind(row.failure_epoch).bind(now).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}

/// The current authentication context excludes labels, observations and model-list edits. Endpoint
/// and credential changes remain distinct, with only sealed-value digests read inside the database.
async fn scopes(conn: &mut Conn, leaf: &Leaf) -> Result<Vec<Scope>, EngineError> {
    let provider = match leaf {
        Leaf::Model { backend } => backend.offering.as_ref().map(|offering| offering.provider),
        Leaf::Provider { id } => Some(*id),
        _ => None,
    };
    let Some(provider) = provider else {
        return Ok(vec![]);
    };
    let generation: Option<Json> = sqlx::query_scalar("SELECT jsonb_build_array(p.machine_id,p.key_machine_id,p.agent_id,p.login_state,p.upstreams,p.config,p.credential_home,
        COALESCE((SELECT jsonb_agg(jsonb_build_array(s.id,s.name,s.rotated_at,s.created_at,md5(s.sealed)) ORDER BY s.name) FROM secrets s WHERE s.owner_kind='provider' AND s.owner_id=p.id),'[]'::jsonb)) FROM providers p WHERE p.id=$1 AND p.deleted_at IS NULL")
        .bind(provider).fetch_optional(conn).await.map_err(DbError::from)?;
    let Some(generation) = generation else {
        return Ok(vec![]);
    };
    let mut scopes = vec![Scope::credential(provider, generation.clone())];
    if matches!(leaf, Leaf::Model { .. }) {
        scopes.push(Scope::member(
            provider,
            generation,
            serde_json::to_value(leaf)?,
        ));
    }
    Ok(scopes)
}

pub(super) async fn reason(
    conn: &mut Conn,
    leaf: &Leaf,
    path: &[Hop],
    now: DateTime<Utc>,
) -> Result<Option<String>, EngineError> {
    for scope in scopes(conn, leaf).await? {
        for hop in path {
            match state(conn, hop.pool_id, &scope, now).await? {
                Suppression::Cooling { .. } => return Ok(Some("member_cooling".into())),
                Suppression::TrialInProgress { .. } => {
                    return Ok(Some("member_trial_in_progress".into()));
                }
                Suppression::Ready | Suppression::TrialReady { .. } => {}
            }
        }
    }
    Ok(None)
}

/// Bind after the consumer's current native Use/capacity checks, in the transaction admitting the
/// actual command or turn. The actual member can refine a provider-only intent without widening it.
pub async fn admit(
    conn: &mut Conn,
    choice: &Choice,
    attempt: Id,
    actual: &Leaf,
    now: DateTime<Utc>,
) -> Result<bool, EngineError> {
    let Some(selected) = choice.leaf.as_ref().filter(|_| choice.reason.is_none()) else {
        return Ok(false);
    };
    let agrees = selected == actual
        || matches!((selected, actual), (Leaf::Provider { id }, Leaf::Model { backend }) if backend.offering.as_ref().is_some_and(|offering| offering.provider == *id));
    if !agrees {
        return Ok(false);
    }
    let captured = scopes(conn, actual).await?;
    // A source replay cannot recapture another generation under the same native attempt identity.
    let previous: Vec<Json> = sqlx::query_scalar("SELECT s.identity FROM pool_attempt_scopes a JOIN pool_failure_scopes s ON s.id=a.scope_id WHERE a.attempt_id=$1 ORDER BY s.identity::text")
        .bind(attempt).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    if !previous.is_empty() {
        let mut current = captured
            .iter()
            .map(|scope| scope.identity.clone())
            .collect::<Vec<_>>();
        current.sort_by_key(Json::to_string);
        return Ok(previous == current);
    }
    // Lock and check every scope before claiming any of them. A rejected admission leaves no trial.
    let mut ready = Vec::new();
    for scope in captured {
        let row = locked(conn, &scope).await?;
        ready.push((scope, row));
    }
    let mut trials = Vec::new();
    for (scope, _) in &ready {
        for hop in &choice.path {
            match state(conn, hop.pool_id, scope, now).await? {
                Suppression::Ready => {}
                Suppression::TrialReady { epoch } => trials.push((hop.pool_id, scope, epoch)),
                _ => return Ok(false),
            }
        }
    }
    for (pool, scope, epoch) in trials {
        if !claim(conn, pool, scope, attempt, epoch, now).await? {
            return Err(bad("the native recovery trial changed during admission"));
        }
    }
    for (scope, row) in ready {
        for hop in &choice.path {
            sqlx::query("INSERT INTO pool_scope_observations(pool_id,scope_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
                .bind(hop.pool_id).bind(row.id).execute(&mut *conn).await.map_err(DbError::from)?;
        }
        sqlx::query("INSERT INTO pool_attempt_scopes(attempt_id,scope_id,choice_id,failure_epoch,credential) VALUES($1,$2,$3,$4,$5)")
            .bind(attempt).bind(row.id).bind(choice.id).bind(row.failure_epoch).bind(scope.credential).execute(&mut *conn).await.map_err(DbError::from)?;
    }
    Ok(true)
}

/// Reconcile an actual terminal native receipt. Uncertain, cancelled or local failures settle the
/// claim without fabricating vendor failure or success. Retransmitted receipts are idempotent.
pub async fn outcome(
    conn: &mut Conn,
    attempt: Id,
    succeeded: bool,
    failure: Option<&Json>,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let rows: Vec<(Id, Json, i64, bool)> = sqlx::query_as("SELECT s.id,s.identity,a.failure_epoch,a.credential FROM pool_attempt_scopes a JOIN pool_failure_scopes s ON s.id=a.scope_id WHERE a.attempt_id=$1 AND a.settled_at IS NULL ORDER BY a.credential DESC,s.id")
        .bind(attempt).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    for (id, identity, epoch, credential) in rows {
        let scope = Scope {
            identity,
            credential,
        };
        let _fence = locked(conn, &scope).await?;
        if succeeded {
            success(conn, &scope, epoch, attempt, now).await?;
        } else {
            settle(conn, &scope, attempt, now).await?;
            if let Some(failed) = failure {
                let code = failed
                    .get("code")
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                let credential_failure = code == "provider_unauthorized"
                    || code == "provider_exhausted"
                        && failed.pointer("/limit/scope").and_then(Json::as_str) == Some("plan");
                if credential == credential_failure {
                    let classification = Classification {
                        code: code.into(),
                        reason: failed
                            .get("reason")
                            .and_then(Json::as_str)
                            .map(str::to_owned),
                        evidence: Evidence::Structured,
                    };
                    let reset = failed
                        .get("resets_at")
                        .filter(|value| !value.is_null())
                        .cloned()
                        .map(serde_json::from_value::<orch_types::Time>)
                        .transpose()?
                        .map(orch_types::Time::get);
                    self::failure(
                        conn,
                        &scope,
                        attempt,
                        FailureOrigin::Vendor,
                        &classification,
                        now,
                        reset,
                    )
                    .await?;
                }
            }
        }
        sqlx::query("UPDATE pool_attempt_scopes SET settled_at=$3 WHERE attempt_id=$1 AND scope_id=$2 AND settled_at IS NULL")
            .bind(attempt).bind(id).bind(now).execute(&mut *conn).await.map_err(DbError::from)?;
    }
    Ok(())
}

/// The ordinary Pool Manage writer calls this with its actual source bound; no member names or
/// shared facts are returned or erased, and pending native attempts retain their fenced claims.
pub async fn clear_pool(conn: &mut Conn, pool: Id, now: DateTime<Utc>) -> Result<(), EngineError> {
    sqlx::query("INSERT INTO pool_failure_clears(pool_id,scope_id,epoch,cleared_at) SELECT $1,s.id,s.failure_epoch,$2 FROM pool_scope_observations o JOIN pool_failure_scopes s ON s.id=o.scope_id WHERE o.pool_id=$1 ON CONFLICT(pool_id,scope_id) DO UPDATE SET epoch=excluded.epoch,cleared_at=excluded.cleared_at")
        .bind(pool).bind(now).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}
