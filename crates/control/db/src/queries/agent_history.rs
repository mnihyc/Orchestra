//! Stored native history observations and thread-locked removal reservations.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// One native history's stored inventory; reading it never contacts a machine.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct History {
    /// The segment whose native identity is measured.
    pub id: Id,
    /// The conversation retained by Orch.
    pub thread_id: Id,
    /// The provider owning the home.
    pub provider_id: Id,
    /// The profile owning the harness within the home.
    pub agent_id: Id,
    /// The immutable machine that owns the segment's stream.
    pub machine_id: Option<Id>,
    /// The adapter's native identity.
    pub native_session_ref: String,
    /// Last recorded physical size, or unknown.
    pub bytes: Option<i64>,
    /// When the machine measured it.
    pub measured_at: Option<DateTime<Utc>>,
    /// When physical removal was confirmed.
    pub removed_at: Option<DateTime<Utc>>,
    /// The person whose explicit act removed it, or automatic retention.
    pub removed_by: Option<Id>,
    /// The adapter verified the server's creation provenance in a managed home.
    pub owned: bool,
    /// The retained native identity was created by Orch, rather than adopted from its program.
    pub started_by_orch: bool,
    /// Durable native removal command, kept until its outcome is reconciled.
    pub claim: Option<Id>,
    /// Time the reservation was committed.
    pub claimed_at: Option<DateTime<Utc>>,
    /// Last explicit refusal or failed observation.
    pub error: Option<String>,
    /// Exact latest turn activity across this thread's retained lanes.
    pub last_turn_at: DateTime<Utc>,
    /// Captured native-close counter across physical aliases.
    pub measure_revision: i64,
}

const SELECT: &str = "SELECT s.id,s.thread_id,s.provider_id,s.agent_id,s.machine_id,s.native_session_ref,
    s.agent_history_bytes AS bytes,s.agent_history_measured_at AS measured_at,
    s.agent_history_removed_at AS removed_at,s.agent_history_removed_by AS removed_by,
    s.agent_history_owned AS owned,
    NOT EXISTS(SELECT 1 FROM segments adopted WHERE adopted.machine_id IS NOT DISTINCT FROM s.machine_id
        AND adopted.provider_id=s.provider_id AND adopted.agent_id=s.agent_id AND adopted.native_session_ref=s.native_session_ref
        AND adopted.import_report ? 'adopted') AS started_by_orch,
    s.agent_history_claim AS claim,s.agent_history_claimed_at AS claimed_at,
    s.agent_history_error AS error,COALESCE((SELECT GREATEST(u.created_at,u.started_at,u.completed_at)
        FROM turns u WHERE u.thread_id=s.thread_id
        ORDER BY GREATEST(u.created_at,u.started_at,u.completed_at) DESC LIMIT 1),s.started_at) AS last_turn_at,
    COALESCE((SELECT sum(alias.agent_history_measure_revision) FROM segments alias WHERE alias.machine_id IS NOT DISTINCT FROM s.machine_id
        AND alias.provider_id=s.provider_id AND alias.agent_id=s.agent_id AND alias.native_session_ref=s.native_session_ref),0)::bigint AS measure_revision FROM segments s";

const LATEST: &str = "s.id=(SELECT alias.id FROM segments alias WHERE alias.machine_id IS NOT DISTINCT FROM s.machine_id AND alias.provider_id=s.provider_id
            AND alias.agent_id=s.agent_id AND alias.native_session_ref=s.native_session_ref
            ORDER BY alias.started_at DESC,alias.id DESC LIMIT 1)";

/// Reads one captured native identity, including removal state.
pub async fn get(conn: &mut Conn, segment: Id) -> Result<Option<History>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE s.id=$1 AND s.native_session_ref IS NOT NULL"
    )))
    .bind(segment)
    .fetch_optional(conn)
    .await?)
}

/// Pages a complete inventory, including archived and recently deleted conversations.
pub async fn page(conn: &mut Conn, after: Option<Id>, limit: i64) -> Result<Vec<History>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE s.native_session_ref IS NOT NULL
        AND s.agent_history_removed_at IS NULL AND ($1::uuid IS NULL OR s.id>$1)
        AND {LATEST} ORDER BY s.id LIMIT $2"
    )))
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// A close creates dirty work even when no time passed since the previous size observation. Each
/// native identity keeps the closes of all its segments and the closes its last measurement covered
/// (`agent_history_natives`, kept by a trigger on `segments`), so the page starts from the identities
/// whose closes are not measured yet, by index, and reads each one's newest segment.
pub async fn closed_page(
    conn: &mut Conn,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<History>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT observed.* FROM agent_history_natives n
        CROSS JOIN LATERAL ({SELECT} WHERE s.machine_id IS NOT DISTINCT FROM n.machine_id AND s.provider_id=n.provider_id
            AND s.agent_id=n.agent_id AND s.native_session_ref=n.native_session_ref
            ORDER BY s.started_at DESC,s.id DESC LIMIT 1) observed
        WHERE n.measured<n.closes AND observed.removed_at IS NULL AND ($1::uuid IS NULL OR observed.id>$1)
        ORDER BY observed.id LIMIT $2")))
        .bind(after).bind(limit).fetch_all(conn).await?)
}

/// Commits only the close revision captured before the machine measured these physical aliases.
pub async fn measured_revision(conn: &mut Conn, segment: Id, revision: i64) -> Result<(), DbError> {
    sqlx::query("UPDATE segments s SET agent_history_measured_revision=GREATEST(s.agent_history_measured_revision,$2)
        FROM segments requested WHERE requested.id=$1 AND s.machine_id IS NOT DISTINCT FROM requested.machine_id
        AND s.provider_id=requested.provider_id AND s.agent_id=requested.agent_id AND s.native_session_ref=requested.native_session_ref")
        .bind(segment).bind(revision).execute(conn).await?;
    Ok(())
}

// Separate first/next suppliers keep the index range selective with a generic prepared plan too.
const CLAIMS_FIRST: &str =
    " WHERE s.agent_history_claim IS NOT NULL AND s.id<=$1 ORDER BY s.id LIMIT $2";
const CLAIMS_AFTER: &str =
    " WHERE s.agent_history_claim IS NOT NULL AND s.id>$1 AND s.id<=$2 ORDER BY s.id LIMIT $3";

/// Captures this pass's final claim, so arriving commands cannot extend the reconciliation forever.
pub async fn claim_ceiling(conn: &mut Conn) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM segments WHERE agent_history_claim IS NOT NULL ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(conn)
    .await?)
}

/// Pages unfinished commands through a captured fence, including already removed native histories.
pub async fn claims(
    conn: &mut Conn,
    after: Option<Id>,
    through: Id,
    limit: i64,
) -> Result<Vec<History>, DbError> {
    Ok(match after {
        Some(after) => {
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT}{CLAIMS_AFTER}")))
                .bind(after)
                .bind(through)
                .bind(limit)
                .fetch_all(conn)
                .await?
        }
        None => {
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT}{CLAIMS_FIRST}")))
                .bind(through)
                .bind(limit)
                .fetch_all(conn)
                .await?
        }
    })
}

/// A current native dependency prevents removal even when its conversation is idle.
pub async fn has_dependents(conn: &mut Conn, history: &History) -> Result<bool, DbError> {
    if crate::queries::jobs::forks::lock_of(conn, history.thread_id)
        .await?
        .is_some()
    {
        return Ok(true);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads side JOIN segments live ON live.id=side.current_segment_id
        JOIN segments requested ON requested.id=$2 WHERE side.side_of IS NOT NULL AND side.side_reading='no_tools'
        AND live.machine_id=requested.machine_id AND live.provider_id=requested.provider_id
        AND live.agent_id=requested.agent_id AND live.native_session_ref=requested.native_session_ref)
        OR EXISTS(SELECT 1 FROM segments alias,segments requested
        WHERE requested.id=$2 AND alias.machine_id IS NOT DISTINCT FROM requested.machine_id
        AND alias.provider_id=requested.provider_id AND alias.agent_id=requested.agent_id
        AND alias.native_session_ref=requested.native_session_ref AND alias.thread_id<>requested.thread_id)
        OR EXISTS(SELECT 1 FROM thread_continuations c
        WHERE (c.native_source->>'native_session_ref'=$1 OR c.native_prefix#>>'{source,native_session_ref}'=$1)
          AND EXISTS(SELECT 1 FROM segments s,segments requested WHERE requested.id=$2
            AND s.machine_id=requested.machine_id AND s.provider_id=requested.provider_id AND s.agent_id=requested.agent_id
            AND s.native_session_ref=requested.native_session_ref
            AND (c.native_source->>'segment_id'=s.id::text OR c.native_prefix#>>'{source,segment_id}'=s.id::text)))
        OR EXISTS(SELECT 1 FROM thread_continuations c JOIN segments s ON s.thread_id=c.thread_id
            JOIN segments requested ON requested.id=$2
            WHERE s.machine_id=requested.machine_id AND s.provider_id=requested.provider_id AND s.agent_id=requested.agent_id
            AND s.native_session_ref=requested.native_session_ref AND (
                c.native_creation->>'segment_id'=s.id::text AND c.native_creation->>'machine_id'=s.machine_id::text
                    AND c.native_creation_result#>>'{session,native_session_ref}'=s.native_session_ref
                OR c.current_segment_id=s.id AND (c.open_result#>>'{value,session,native_session_ref}'=s.native_session_ref
                    OR c.open_result#>>'{value,native_session_ref}'=s.native_session_ref)))
        OR EXISTS(SELECT 1 FROM thread_history_cuts cut JOIN segments s ON s.id=cut.source_segment_id
            JOIN segments requested ON requested.id=$2 WHERE cut.source_native_ref=$1
            AND s.machine_id=requested.machine_id AND s.provider_id=requested.provider_id AND s.agent_id=requested.agent_id
            AND s.native_session_ref=requested.native_session_ref
            AND (EXISTS(SELECT 1 FROM jobs job WHERE job.id=cut.job_id AND job.state IN ('queued','running'))
                OR cut.owner_thread_id IS NOT NULL AND cut.owner_thread_id<>cut.thread_id AND NOT (cut.artifacts ? 'side_native_result')))")
        .bind(&history.native_session_ref).bind(history.id)
        .fetch_one(conn).await?)
}

/// Selects recovery notice recipients in this logical history and physical native lane.
pub async fn notice_receivers(
    conn: &mut Conn,
    thread: Id,
    current: Id,
    recipients: &[Id],
) -> Result<Vec<Id>, DbError> {
    if recipients.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "SELECT original.id FROM segments original JOIN segments recipient ON recipient.id=$2
        WHERE recipient.thread_id=$1 AND original.thread_id=recipient.thread_id AND original.id=ANY($3)
        AND original.continuation_id=recipient.continuation_id
        AND original.provider_id=recipient.provider_id AND original.agent_id=recipient.agent_id
        AND original.machine_id IS NOT DISTINCT FROM recipient.machine_id
        AND (original.id=recipient.id OR original.native_session_ref IS NOT NULL
            AND original.native_session_ref=recipient.native_session_ref)",
    )
    .bind(thread)
    .bind(current)
    .bind(recipients)
    .fetch_all(conn)
    .await?)
}

/// Every reason that makes an idle-history removal unsafe, under the thread's write lock.
pub async fn refusal(conn: &mut Conn, history: &History) -> Result<Option<&'static str>, DbError> {
    if !history.owned || !history.started_by_orch {
        return Ok(Some("history.not_owned"));
    }
    if history.machine_id.is_none() {
        return Ok(Some("history.machine_unavailable"));
    }
    if history.removed_at.is_some() {
        return Ok(Some("history.already_removed"));
    }
    if history.claim.is_some() {
        return Ok(Some("history.removing"));
    }
    let running: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1 AND state IN ('starting','running'))
        OR EXISTS(SELECT 1 FROM threads WHERE node_id=$1 AND (current_turn_id IS NOT NULL OR state IN ('starting','running','waiting_approval','stopping')))")
        .bind(history.thread_id).fetch_one(&mut *conn).await?;
    if running {
        return Ok(Some("history.turn_running"));
    }
    let inputs: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND state IN ('pending','delivering'))")
        .bind(history.thread_id).fetch_one(&mut *conn).await?;
    if inputs {
        return Ok(Some("history.inputs_waiting"));
    }
    let approvals: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM approvals WHERE thread_id=$1 AND state IN ('pending','approved'))")
        .bind(history.thread_id).fetch_one(&mut *conn).await?;
    if approvals {
        return Ok(Some("history.approval_waiting"));
    }
    if has_dependents(conn, history).await? {
        return Ok(Some("history.in_use"));
    }
    Ok(None)
}

/// Claims only the observed idle history; any turn committed since observation wins the comparison.
pub async fn claim(
    conn: &mut Conn,
    observed: &History,
    command: Id,
    by: Option<Id>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
        .bind(observed.thread_id)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(current) = get(conn, observed.id).await? else {
        return Ok(false);
    };
    if current.last_turn_at != observed.last_turn_at
        || current.native_session_ref != observed.native_session_ref
        || refusal(conn, &current).await?.is_some()
    {
        return Ok(false);
    }
    Ok(sqlx::query("UPDATE segments SET agent_history_claim=$2,agent_history_claimed_at=$3,
        agent_history_removed_by=$4,agent_history_error=NULL WHERE id=$1 AND agent_history_claim IS NULL
        AND agent_history_removed_at IS NULL AND native_session_ref=$5")
        .bind(current.id).bind(command).bind(at).bind(by).bind(&current.native_session_ref)
        .execute(conn).await?.rows_affected()==1)
}

/// An outstanding native removal never loses its fence merely because a process restarted.
pub async fn removing(conn: &mut Conn, thread: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM segments WHERE thread_id=$1 AND agent_history_claim IS NOT NULL)")
        .bind(thread).fetch_one(conn).await?)
}

/// Saves a machine observation only for the exact identity that was requested.
pub async fn measured(
    conn: &mut Conn,
    segment: Id,
    native: &str,
    bytes: Option<i64>,
    removed: bool,
    error: Option<&str>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE segments s SET agent_history_bytes=CASE WHEN $5::text IS NULL THEN $3 ELSE s.agent_history_bytes END,
        agent_history_measured_at=CASE WHEN $5::text IS NULL THEN $6 ELSE s.agent_history_measured_at END,
        agent_history_owned=s.agent_history_owned OR $5::text IS NULL,
        agent_history_removed_at=CASE WHEN $4 AND $5::text IS NULL THEN COALESCE(s.agent_history_removed_at,$6) ELSE s.agent_history_removed_at END,
        agent_history_error=$5 FROM segments requested WHERE requested.id=$1 AND requested.native_session_ref=$2
        AND s.machine_id IS NOT DISTINCT FROM requested.machine_id AND s.provider_id=requested.provider_id AND s.agent_id=requested.agent_id AND s.native_session_ref=requested.native_session_ref")
        .bind(segment).bind(native).bind(bytes).bind(removed).bind(error).bind(at).execute(conn).await?;
    Ok(())
}

/// One observation from a bounded native command, including the captured close counter.
#[derive(serde::Serialize)]
pub struct Observation {
    /// The requested immutable segment.
    pub segment: Id,
    /// The exact physical identity, checked again during the write.
    pub native: String,
    /// The measured size, or unknown.
    pub bytes: Option<i64>,
    /// Whether the adapter confirmed physical absence.
    pub removed: bool,
    /// An explicit failed observation never erases an earlier successful size.
    pub error: Option<String>,
    /// Only a settled measurement advances the previously captured close counter.
    pub revision: Option<i64>,
}

/// Applies one native command's observations and alias counters in one indexed statement.
pub async fn measured_batch(
    conn: &mut Conn,
    observations: &[Observation],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    if observations.is_empty() {
        return Ok(());
    }
    sqlx::query("WITH observations AS (
        SELECT requested.machine_id,requested.provider_id,requested.agent_id,o.*
        FROM jsonb_to_recordset($1::jsonb) AS o(segment uuid,native text,bytes bigint,removed boolean,error text,revision bigint)
        JOIN segments requested ON requested.id=o.segment AND requested.native_session_ref=o.native)
        UPDATE segments s SET agent_history_bytes=CASE WHEN o.error IS NULL THEN o.bytes ELSE s.agent_history_bytes END,
        agent_history_measured_at=CASE WHEN o.error IS NULL THEN $2 ELSE s.agent_history_measured_at END,
        agent_history_owned=s.agent_history_owned OR o.error IS NULL,
        agent_history_removed_at=CASE WHEN o.removed AND o.error IS NULL THEN COALESCE(s.agent_history_removed_at,$2) ELSE s.agent_history_removed_at END,
        agent_history_measured_revision=GREATEST(s.agent_history_measured_revision,COALESCE(o.revision,s.agent_history_measured_revision)),
        agent_history_error=o.error FROM observations o
        WHERE s.provider_id=o.provider_id AND s.agent_id=o.agent_id AND s.native_session_ref=o.native
          AND s.machine_id IS NOT DISTINCT FROM o.machine_id")
        .bind(sqlx::types::Json(observations)).bind(at).execute(conn).await?;
    Ok(())
}

/// One bounded home candidate, including a current decision for the supported program kinds.
#[derive(sqlx::FromRow)]
pub struct Home {
    /// The machine owning the native files.
    pub machine: Id,
    /// The provider owning the home.
    pub provider: Id,
    /// The profile that manages the home.
    pub agent: Id,
    /// The program supports cleanup and every current home member is idle and owned.
    pub eligible: bool,
}

/// Pages home identities before inspecting their work, so blocked pages never starve later homes.
pub async fn home_page(
    conn: &mut Conn,
    kinds: &[String],
    after: Option<(Id, Id, Id)>,
    limit: i64,
) -> Result<Vec<Home>, DbError> {
    let (machine, provider, agent) =
        after.map_or((None, None, None), |(m, p, a)| (Some(m), Some(p), Some(a)));
    Ok(sqlx::query_as("WITH candidates AS MATERIALIZED (
        SELECT DISTINCT s.machine_id,s.provider_id,s.agent_id FROM segments s
        WHERE s.machine_id IS NOT NULL AND s.agent_history_owned
        AND ($2::uuid IS NULL OR (s.machine_id,s.provider_id,s.agent_id)>($2,$3,$4))
        ORDER BY s.machine_id,s.provider_id,s.agent_id LIMIT $5)
        SELECT c.machine_id AS machine,c.provider_id AS provider,c.agent_id AS agent,
        COALESCE((SELECT a.harness_kind=ANY($1) FROM agents a WHERE a.id=c.agent_id),false)
        AND NOT EXISTS(SELECT 1 FROM (
            SELECT other.thread_id,bool_or(other.native_session_ref IS NOT NULL AND NOT other.agent_history_owned
                OR other.agent_history_claim IS NOT NULL) AS unsafe_history
            FROM segments other WHERE other.machine_id=c.machine_id AND other.provider_id=c.provider_id AND other.agent_id=c.agent_id
            GROUP BY other.thread_id) membership JOIN threads t ON t.node_id=membership.thread_id
            WHERE membership.unsafe_history OR t.current_turn_id IS NOT NULL OR t.state IN ('starting','running','waiting_approval')
                OR EXISTS(SELECT 1 FROM thread_inputs i WHERE i.thread_id=t.node_id AND i.state IN ('pending','delivering'))
                OR EXISTS(SELECT 1 FROM approvals q WHERE q.thread_id=t.node_id AND q.state IN ('pending','approved')))
        AS eligible FROM candidates c ORDER BY c.machine_id,c.provider_id,c.agent_id")
        .bind(kinds).bind(machine).bind(provider).bind(agent).bind(limit).fetch_all(conn).await?)
}

/// Settles only this reservation; an unknown outcome keeps the fence until reconciliation.
pub async fn finish(
    conn: &mut Conn,
    segment: Id,
    command: Id,
    error: Option<&str>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE segments SET agent_history_claim=NULL,agent_history_claimed_at=NULL,agent_history_error=$3
        WHERE id=$1 AND agent_history_claim=$2")
        .bind(segment).bind(command).bind(error).execute(conn).await?.rows_affected()==1)
}

/// Stores a frozen recovery recipe on its fresh segment before native history is reconstructed.
pub async fn recovery(conn: &mut Conn, segment: Id, body: &Json) -> Result<(), DbError> {
    sqlx::query("UPDATE segments SET agent_history_recovery=$2 WHERE id=$1 AND agent_history_recovery IS NULL")
        .bind(segment).bind(body).execute(conn).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::database::TestDb;

    async fn fixture() -> TestDb {
        let db = TestDb::fresh().await;
        let fixture =
            include_str!("../../tests/agent_history_upgrade_fixture.sql").replace(":rows", "401");
        // Both the fixture and its replacement row count are fixed test text, never caller input.
        sqlx::raw_sql(sqlx::AssertSqlSafe(fixture))
            .execute(&db.pool)
            .await
            .unwrap();
        db
    }

    #[tokio::test]
    async fn unknown_receipts_do_not_starve_later_claims_and_new_arrivals_wait_for_the_next_pass() {
        let db = fixture().await;
        let mut conn = db.pool.acquire().await.unwrap();
        let through = claim_ceiling(&mut conn).await.unwrap().unwrap();
        assert_eq!(through, Id::from_u128(400));
        let first = claims(&mut conn, None, through, 200).await.unwrap();
        assert_eq!(first.len(), 200);
        assert!(first.iter().all(|row| {
            row.claim.is_some() && row.error.as_deref() == Some("history.command_unconfirmed")
        }));
        let next = claims(&mut conn, first.last().map(|row| row.id), through, 200)
            .await
            .unwrap();
        assert_eq!(next.first().unwrap().id, Id::from_u128(201));
        assert_eq!(next.last().unwrap().id, through);
        let still_first = claims(&mut conn, None, through, 200).await.unwrap();
        assert_eq!(
            still_first.len(),
            200,
            "unknown receipts were not discarded"
        );

        sqlx::query("INSERT INTO segments(id,thread_id,continuation_id,ordinal,agent_id,provider_id,environment_id,machine_id,env_generation,arrangement,credential_path,seq_updated,native_session_ref,agent_history_claim,agent_history_claimed_at)
            SELECT $1,thread_id,continuation_id,3,agent_id,provider_id,environment_id,machine_id,env_generation,arrangement,credential_path,seq_updated,'new-after-fence',$2,now() FROM segments WHERE id=$3")
            .bind(Id::from_u128(500)).bind(orch_types::new_id()).bind(Id::from_u128(1))
            .execute(&mut *conn).await.unwrap();
        assert!(
            claims(&mut conn, Some(through), through, 200)
                .await
                .unwrap()
                .is_empty()
        );
        let later_fence = claim_ceiling(&mut conn).await.unwrap().unwrap();
        assert_eq!(later_fence, Id::from_u128(500));
        let later = claims(&mut conn, Some(through), later_fence, 200)
            .await
            .unwrap();
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].id, Id::from_u128(500));
    }

    #[tokio::test]
    async fn a_close_receipt_changes_only_its_exact_uuid_segment() {
        let db = fixture().await;
        let before: Vec<(Id, i64)> =
            sqlx::query_as("SELECT id,agent_history_measure_revision FROM segments ORDER BY id")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        sqlx::query("UPDATE commands SET state='succeeded',result='{}' WHERE id=md5('history-proof-command-1')::uuid")
            .execute(&db.pool).await.unwrap();
        let after: Vec<(Id, i64)> =
            sqlx::query_as("SELECT id,agent_history_measure_revision FROM segments ORDER BY id")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(before.len(), after.len());
        for ((id, previous), (actual, revision)) in before.into_iter().zip(after) {
            assert_eq!(id, actual);
            assert_eq!(revision, previous + i64::from(id == Id::from_u128(1)));
        }
    }

    #[tokio::test]
    async fn unmeasured_closes_are_found_by_native_identity_as_the_sum_over_aliases_finds_them() {
        let db = fixture().await;
        let mut conn = db.pool.acquire().await.unwrap();
        // The definition the identity rows replace: the newest alias's measured revision below the closes of all aliases.
        let summed = "SELECT s.id FROM segments s WHERE s.native_session_ref IS NOT NULL AND s.agent_history_removed_at IS NULL
            AND s.id=(SELECT a.id FROM segments a WHERE a.machine_id IS NOT DISTINCT FROM s.machine_id AND a.provider_id=s.provider_id
                AND a.agent_id=s.agent_id AND a.native_session_ref=s.native_session_ref ORDER BY a.started_at DESC,a.id DESC LIMIT 1)
            AND s.agent_history_measured_revision<COALESCE((SELECT sum(a.agent_history_measure_revision) FROM segments a
                WHERE a.machine_id IS NOT DISTINCT FROM s.machine_id AND a.provider_id=s.provider_id AND a.agent_id=s.agent_id
                AND a.native_session_ref=s.native_session_ref),0) ORDER BY s.id";
        async fn paged(conn: &mut Conn) -> Vec<Id> {
            let mut found = Vec::new();
            loop {
                let page = closed_page(conn, found.last().copied(), 50).await.unwrap();
                if page.is_empty() {
                    return found;
                }
                found.extend(page.iter().map(|history| history.id));
            }
        }
        let expected: Vec<Id> = sqlx::query_scalar(summed)
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        assert_eq!(expected.len(), 257, "the fixture's dirty identities");
        assert_eq!(paged(&mut conn).await, expected);
        let third = Id::from_u128(3);
        let alias = Id::from_u128(0xaaaa);
        sqlx::query("INSERT INTO segments(id,thread_id,ordinal,agent_id,provider_id,environment_id,machine_id,env_generation,arrangement,
                credential_path,seq_updated,native_session_ref,started_by,started_at)
            SELECT $1,thread_id,ordinal+5000,agent_id,provider_id,environment_id,machine_id,env_generation,arrangement,credential_path,
                seq_updated,native_session_ref,started_by,now() FROM segments WHERE id=$2")
            .bind(alias).bind(third).execute(&mut *conn).await.unwrap();
        let after_alias = paged(&mut conn).await;
        assert!(
            after_alias.contains(&alias) && !after_alias.contains(&third),
            "the newest alias stands for its history"
        );
        assert_eq!(
            after_alias,
            sqlx::query_scalar::<_, Id>(summed)
                .fetch_all(&mut *conn)
                .await
                .unwrap()
        );
        let captured = get(&mut conn, alias)
            .await
            .unwrap()
            .unwrap()
            .measure_revision;
        measured_revision(&mut conn, alias, captured).await.unwrap();
        assert!(
            !paged(&mut conn).await.contains(&alias),
            "a measurement settles every alias"
        );
        sqlx::query("UPDATE segments SET agent_history_measure_revision=agent_history_measure_revision+1 WHERE id=$1")
            .bind(third).execute(&mut *conn).await.unwrap();
        assert!(
            paged(&mut conn).await.contains(&alias),
            "a close of another alias needs a new measurement"
        );
        sqlx::query("UPDATE segments SET machine_id=NULL WHERE id=$1")
            .bind(Id::from_u128(5))
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("DELETE FROM segments WHERE id=ANY($1)")
            .bind(vec![third, alias])
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            paged(&mut conn).await,
            sqlx::query_scalar::<_, Id>(summed)
                .fetch_all(&mut *conn)
                .await
                .unwrap()
        );
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM agent_history_natives WHERE native_session_ref='history-3'",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(left, 0, "an identity goes with its last segment");
    }
}
