//! Current-authority due claims atomically create ordinary work and report each civil occurrence once.

use super::*;
use crate::state::AppState;
use chrono::Utc;
use orch_engine::inputs::Submitted;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Uses the ordinary supervisor page budget; another sweep takes any remaining due rows.
const PAGE: i64 = 200;
/// Inputs sent later have the specification's one-minute default cadence.
const EVERY: Duration = Duration::from_secs(60);

pub(super) async fn accept(
    app: &AppState,
    conn: &mut Conn,
    caller: &Caller,
    row: &rows::ScheduleRow,
    occurrence: &str,
    date: Option<chrono::NaiveDate>,
) -> ApiResult<Submitted> {
    let native: Option<NativeInvocation> = row.native_action.clone().map(decode).transpose()?;
    let now = app.clock.now();
    let thread = match target(row)? {
        InputScheduleTarget::Thread { thread_id } => {
            let thread = threads::lock(conn, thread_id)
                .await?
                .filter(|thread| thread.topic_id == row.topic_id)
                .ok_or_else(ApiError::not_found)?;
            thread.node_id
        }
        InputScheduleTarget::NewThread { config } => {
            let suffix = date.map_or_else(String::new, |date| format!(" · {date}"));
            let base = config.name.as_deref().unwrap_or(&row.name);
            let name = format!(
                "{}{suffix}",
                base.chars()
                    .take(120 - suffix.chars().count())
                    .collect::<String>()
            );
            let (new, birth) = crate::routes::threads::make::scheduled_choices(
                app,
                conn,
                row.topic_id,
                caller.user(),
                &config,
                &name,
            )
            .await?;
            let id = orch_engine::ids::derived(
                "input-schedule.thread",
                &[row.id.as_bytes(), occurrence.as_bytes()],
            );
            match app
                .engine
                .create_thread_in_transaction(conn, caller, &new, &birth, id, now)
                .await
            {
                Ok(created) => created.thread,
                Err(error) => return Err(crate::routes::threads::create_failed(conn, error).await),
            }
        }
    };
    let request = InputRequest {
        plan: false,
        content: content(row)?,
        delivery: Delivery::Queue,
        client_id: format!("schedule:{}:{occurrence}", row.id),
        bypass: None,
    };
    match app
        .engine
        .submit_selected_scheduled_input(conn, caller, thread, &request, now, native.as_ref())
        .await
    {
        Ok(submitted) => Ok(submitted),
        Err(error) => Err(crate::routes::threads::input_failed(conn, thread, error).await),
    }
}

async fn cannot_start(
    conn: &mut Conn,
    row: &rows::ScheduleRow,
    run: &rows::RunRow,
    reason: &str,
    now: chrono::DateTime<Utc>,
) -> ApiResult<()> {
    let already:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log WHERE action='input_schedule.cannot_start' AND object_id=$1 AND details->>'run'=$2)")
        .bind(row.id).bind(run.id.to_string()).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    if already {
        return Ok(());
    }
    record(
        conn,
        row,
        Actor::User(row.author_id),
        orch_db::audit::action!(AuditAction::InputScheduleCannotStart),
        &Origin::default(),
        json!({"run":run.id,"occurrence":run.occurrence,"reason":reason}),
        now,
    )
    .await?;
    let event = orch_engine::notifications::Event {
        kind: orch_engine::notifications::Kind::ScheduleCannotStart,
        node: Some(row.topic_id),
        thread: None,
        approval: None,
        title: "A scheduled input could not start".to_owned(),
        body: format!("{}: {reason}", row.name),
        data: json!({"schedule":row.id,"run":run.id,"reason":reason,"path":format!("/topics/{}/schedules",row.topic_id)}),
        source: run.id.to_string(),
        actor: None,
        at: now,
    };
    let org: Option<Id> = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
        .bind(row.topic_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
    orch_engine::notifications::tell(conn, &event, &[row.author_id], org)
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}

async fn claim(app: &AppState, id: Id) -> ApiResult<bool> {
    let before = {
        let mut conn = app.db.acquire().await?;
        rows::schedule(&mut conn, id, false).await?
    };
    if let Some(row) = before
        && row.native_action.is_some()
        && let InputScheduleTarget::Thread { thread_id } = target(&row)?
    {
        app.engine
            .native_actions(thread_id, true)
            .await
            .map_err(ApiError::internal)?;
    }
    let mut tx = app.db.begin().await?;
    orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
    let now = app.clock.now();
    let Some(mut row) = rows::schedule(&mut tx, id, true).await? else {
        return Ok(false);
    };
    let Some(first) = row
        .next_run_at
        .filter(|at| row.state == "active" && *at <= now)
    else {
        return Ok(false);
    };
    let timing = timing_of(&row)?;
    let occurrence = timing::latest_due(&timing, first, now)?;
    let missed = timing::missed(&timing, first, &occurrence)?;
    let pending = rows::pending(&mut tx, row.id).await?;
    let mut run = rows::RunRow {
        id: new_id(),
        schedule_id: row.id,
        occurrence: occurrence.key.clone(),
        scheduled_for: occurrence.scheduled_for,
        state: "queued".to_owned(),
        thread_id: None,
        input_id: None,
        turn_id: None,
        reason: None,
        topic_seq: 0,
        created_at: now,
        started_at: None,
    };
    if !rows::insert_run(&mut tx, &run).await? {
        return Ok(false);
    }
    if occurrence.scheduled_for.is_none() {
        run.state = "skipped".to_owned();
        run.reason = Some("nonexistent_local_time".to_owned());
    } else if pending {
        run.state = "skipped".to_owned();
        run.reason = Some("previous_input_pending".to_owned());
    } else {
        let mut trial = tx.begin().await.map_err(DbError::from)?;
        let accepted = accept(
            app,
            &mut trial,
            &Caller::session(row.author_id),
            &row,
            &occurrence.key,
            Some(occurrence.date),
        )
        .await;
        match accepted {
            Ok(submitted) => {
                run.thread_id = Some(submitted.input.thread_id);
                run.input_id = Some(submitted.input.id);
                run.turn_id = submitted.input.turn_id;
                if submitted.input.state_detail.is_some() {
                    run.state = "blocked".to_owned();
                    run.reason = Some("input_held".to_owned());
                } else if missed > 0 {
                    run.reason = Some(format!("coalesced {missed} earlier occurrences"));
                }
                trial.commit().await.map_err(DbError::from)?;
            }
            Err(error) => {
                trial.rollback().await.map_err(DbError::from)?;
                if error.problem.code == ErrorCode::Internal {
                    return Err(error);
                }
                run.state = "failed".to_owned();
                run.reason = Some(error.problem.code.as_str().to_owned());
            }
        }
    }
    if matches!(timing, InputTiming::Once { .. }) {
        row.state = "finished".to_owned();
        row.next_run_at = None;
    } else {
        row.next_run_at = Some(timing::next(&timing, now)?.due);
    }
    row.updated_at = now;
    row.revision += 1;
    row.topic_seq = stamp(&mut tx, row.topic_id).await?;
    run.topic_seq = row.topic_seq;
    rows::update_run(&mut tx, &run).await?;
    rows::update(&mut tx, &row).await?;
    if run.input_id.is_some() {
        record(
            &mut tx,
            &row,
            Actor::User(row.author_id),
            orch_db::audit::action!(AuditAction::InputScheduleRan),
            &Origin::default(),
            json!({"run":run.id,"occurrence":run.occurrence,"state":run.state,"coalesced":missed}),
            now,
        )
        .await?;
    }
    if matches!(run.state.as_str(), "blocked" | "failed" | "skipped") {
        cannot_start(
            &mut tx,
            &row,
            &run,
            run.reason.as_deref().unwrap_or("cannot_start"),
            now,
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    app.engine.kick();
    Ok(true)
}

/// How long a pass's lease holds before the pass renews it after each claim: two minutes, twice the default
/// cadence and far longer than one claim's transaction, so a pass of a process that died holds the next pass
/// back no longer than that. Each claim locks and rechecks its row, so even two passes run an occurrence once.
const PASS_LEASE: chrono::Duration = chrono::Duration::minutes(2);

/// One explicit due pass, also used by held-clock HTTP/database operation tests.
pub async fn run_due(app: &AppState) -> ApiResult<u64> {
    // A lease, not a lock kept in an open transaction: each claim commits on its own.
    let lease = orch_db::queries::job_leases::Lease::for_run("input_schedules", PASS_LEASE);
    if !lease.hold(&app.db, app.clock.now()).await? {
        return Ok(0);
    }
    let ran = run_held(app, &lease).await;
    lease.release(&app.db).await?;
    ran
}

async fn run_held(app: &AppState, lease: &orch_db::queries::job_leases::Lease) -> ApiResult<u64> {
    let ids = {
        let mut conn = app.db.acquire().await?;
        rows::due(&mut conn, app.clock.now(), PAGE).await?
    };
    let mut ran = 0;
    for id in ids {
        if claim(app, id).await? {
            ran += 1;
        }
        if !lease.hold(&app.db, app.clock.now()).await? {
            return Ok(ran);
        }
    }
    refresh_runs(app).await?;
    Ok(ran)
}

async fn refresh_runs(app: &AppState) -> ApiResult<()> {
    // A terminal input no longer needs sweeping; the ordinary thread remains its detailed history.
    let ids: Vec<Id> = {
        let mut conn = app.db.acquire().await?;
        sqlx::query_scalar("SELECT r.id FROM input_schedule_runs r LEFT JOIN thread_inputs i ON i.id=r.input_id LEFT JOIN turns t ON t.id=i.turn_id LEFT JOIN threads th ON th.node_id=i.thread_id WHERE r.state IN ('queued','blocked') AND (i.id IS NULL OR i.state IN ('cancelled','failed') OR t.started_at IS NOT NULL OR (th.blocked_reason IS NOT NULL AND r.state='queued')) ORDER BY r.created_at,r.id LIMIT $1")
            .bind(PAGE).fetch_all(&mut *conn).await.map_err(DbError::from)?
    };
    for id in ids {
        let mut tx = app.db.begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let mut run: rows::RunRow =
            sqlx::query_as("SELECT * FROM input_schedule_runs WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(DbError::from)?;
        let Some(mut row) = rows::schedule(&mut tx, run.schedule_id, true).await? else {
            continue;
        };
        let now = app.clock.now();
        let input = match run.input_id {
            Some(id) => threads::input(&mut tx, id).await?,
            None => None,
        };
        if let Some(input) = input {
            run.turn_id = input.turn_id;
            if input.state == "cancelled" || input.state == "failed" {
                run.state = "failed".to_owned();
                run.reason = Some(
                    if input.state == "cancelled" {
                        "cancelled"
                    } else {
                        "input_failed"
                    }
                    .to_owned(),
                );
            } else if let Some(turn) = input.turn_id
                && let Some(turn) = threads::turn(&mut tx, turn).await?
                && turn.started_at.is_some()
            {
                run.state = "started".to_owned();
                run.started_at = turn.started_at;
            } else if let Some(thread) = threads::lock(&mut tx, input.thread_id).await?
                && let Some(reason) = thread.blocked_reason
            {
                run.state = "blocked".to_owned();
                run.reason = Some(reason);
            } else {
                continue;
            }
        } else {
            run.state = "failed".to_owned();
            run.reason = Some("destination_removed".to_owned());
        }
        row.topic_seq = stamp(&mut tx, row.topic_id).await?;
        run.topic_seq = row.topic_seq;
        rows::update_run(&mut tx, &run).await?;
        // Also stamp the schedule projection so TopicSync carries its changed last_run.
        rows::update(&mut tx, &row).await?;
        if matches!(run.state.as_str(), "failed" | "blocked") {
            cannot_start(
                &mut tx,
                &row,
                &run,
                run.reason.as_deref().unwrap_or("cannot_start"),
                now,
            )
            .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
    }
    Ok(())
}

/// Starts the API-owned due schedule, with the deployment's configured cadence and cancellation.
pub fn start(app: &Arc<AppState>, stop: &CancellationToken) -> tokio::task::JoinHandle<()> {
    let (app, stop) = (Arc::clone(app), stop.clone());
    tokio::spawn(async move {
        let every = crate::cache::interval_of(&app.config, "input_schedules", EVERY);
        let mut ticks = tokio::time::interval(every);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {()=stop.cancelled()=>return,_=ticks.tick()=>{}}
            if let Err(error) = run_due(&app).await {
                tracing::warn!(detail=%error.problem.detail,"input schedules will be checked again at the next interval");
            }
        }
    })
}
