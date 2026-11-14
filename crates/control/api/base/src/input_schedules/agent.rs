//! Agent-authored definitions use the same schedule rows, due claims and admitted input queue.

use super::*;

use crate::state::App;

use crate::state::AppState;

use async_trait::async_trait;

use chrono::DateTime;

use chrono::Duration;

use chrono::Utc;

use orch_engine::tools::CallContext;

use orch_engine::tools::definitions::Code;

use orch_engine::tools::definitions::Refusal;

use orch_engine::tools::schedule::ScheduleTools;

use orch_settings::Place;

use orch_settings::Snapshot;

use orch_settings::Wanted;

use orch_settings_contract::registry;

use orch_settings_contract::registry::keys;

use serde::Deserialize;

use serde::Serialize;

use std::sync::Arc;

use std::sync::Weak;
/// A native delegation by one accepted tool source; no HTTP credential is invented or stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    kind: String,
    for_user: Id,
    input_authority: Json,
    schedule: AgentInputSchedule,
}

fn origin(row: &rows::ScheduleRow) -> ApiResult<Option<Definition>> {
    let Some(raw) = row.definition_authority.as_ref() else {
        return Ok(None);
    };
    if raw.is_array() {
        return Ok(None);
    }
    let origin: Definition = serde_json::from_value(raw.clone()).map_err(|_| {
        ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the native schedule origin is invalid",
        )
    })?;
    if origin.kind != "agent"
        || origin.for_user != row.author_id
        || origin.schedule.agent_id.is_nil()
        || origin.schedule.source_turn_id.is_nil()
        || origin.schedule.max_runs < 1
        || origin.schedule.runs < 0
        || row.native_action.is_some()
        || target(row)?
            != (InputScheduleTarget::Thread {
                thread_id: origin.schedule.thread_id,
            })
    {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the native schedule changed its source or destination",
        ));
    }
    Ok(Some(origin))
}

pub(super) fn metadata(row: &rows::ScheduleRow) -> ApiResult<Option<AgentInputSchedule>> {
    Ok(origin(row)?.map(|origin| origin.schedule))
}

pub(super) fn input_authority(row: &rows::ScheduleRow) -> ApiResult<Option<Json>> {
    Ok(origin(row)?.map(|origin| origin.input_authority))
}

pub(super) async fn validate(
    app: &AppState,
    conn: &mut Conn,
    row: &rows::ScheduleRow,
) -> ApiResult<()> {
    let authority = input_authority(row)?.ok_or_else(ApiError::not_found)?;
    let info = metadata(row)?.ok_or_else(ApiError::not_found)?;
    let request = super::runner::request(row, "validation")?;
    match app
        .engine
        .validate_agent_scheduled_input(
            conn,
            row.author_id,
            &authority,
            info.thread_id,
            &request,
            app.clock.now(),
        )
        .await
    {
        Ok(()) => Ok(()),
        Err(error) => Err(crate::routes::threads::input_failed(conn, info.thread_id, error).await),
    }
}

/// Restore the named starter's CURRENT ordinary holdings for the same native thread only.
/// This is not a browser credential, an HTTP operation admission or an administrator mode.
pub(super) async fn current(
    _app: &AppState,
    conn: &mut Conn,
    row: &rows::ScheduleRow,
) -> ApiResult<Option<Json>> {
    let Some(origin) = origin(row)? else {
        return Ok(None);
    };
    let thread = threads::thread(conn, origin.schedule.thread_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let starter: Option<Id> =
        sqlx::query_scalar("SELECT created_by FROM nodes WHERE id=$1 AND deleted_at IS NULL")
            .bind(thread.node_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .flatten();
    if starter != Some(origin.for_user)
        || thread.topic_id != row.topic_id
        || !orch_engine::tools::schedule::offered(conn, &thread)
            .await
            .map_err(ApiError::engine)?
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "agent schedules are disabled or their original thread is unavailable",
        ));
    }
    // The actual accepted input envelope is restored by the ordinary engine input
    // producer below, including every key, exact operation and current ordinary gate.
    Ok(Some(origin.input_authority))
}

/// End conditions are frozen explicit choices; defaults are consulted only at creation.
pub(super) fn allows_due(row: &rows::ScheduleRow, now: DateTime<Utc>) -> ApiResult<bool> {
    let Some(origin) = origin(row)? else {
        return Ok(true);
    };
    Ok(origin.schedule.runs < origin.schedule.max_runs
        && (!matches!(timing_of(row)?, InputTiming::Cron { .. })
            || now <= origin.schedule.until.get()))
}

pub(super) fn due_cutoff(row: &rows::ScheduleRow, now: DateTime<Utc>) -> ApiResult<DateTime<Utc>> {
    Ok(match metadata(row)? {
        Some(info) if matches!(timing_of(row)?, InputTiming::Cron { .. }) => {
            now.min(info.until.get())
        }
        _ => now,
    })
}

pub(super) fn actor(row: &rows::ScheduleRow) -> ApiResult<Actor> {
    Ok(metadata(row)?.map_or(Actor::User(row.author_id), |info| {
        Actor::Agent(info.thread_id)
    }))
}

pub(super) fn after_claim(row: &mut rows::ScheduleRow, now: DateTime<Utc>) -> ApiResult<()> {
    let Some(mut origin) = origin(row)? else {
        return Ok(());
    };
    origin.schedule.runs = origin
        .schedule
        .runs
        .checked_add(1)
        .ok_or_else(|| ApiError::internal("the schedule occurrence counter overflowed"))?;
    if origin.schedule.runs >= origin.schedule.max_runs
        || now >= origin.schedule.until.get()
        || row
            .next_run_at
            .is_some_and(|at| at > origin.schedule.until.get())
    {
        row.state = "finished".into();
        row.next_run_at = None;
    }
    row.definition_authority = Some(encode(&origin)?);
    Ok(())
}

pub(super) fn check_resume(row: &mut rows::ScheduleRow, now: DateTime<Utc>) -> ApiResult<()> {
    if let Some(origin) = origin(row)?
        && (origin.schedule.runs >= origin.schedule.max_runs
            || now >= origin.schedule.until.get()
            || row
                .next_run_at
                .is_some_and(|at| at > origin.schedule.until.get()))
    {
        row.state = "finished".into();
        row.next_run_at = None;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    command: Option<String>,
    at: Option<Time>,
    in_seconds: Option<i64>,
    cron: Option<String>,
    until: Option<Time>,
    max_runs: Option<i64>,
    note: Option<String>,
    id: Option<Id>,
    cursor: Option<Id>,
    limit: Option<i64>,
}

fn refusal(error: ApiError) -> Refusal {
    let code = match error.code() {
        ErrorCode::NotFound => Code::NotFound,
        ErrorCode::AuthzDenied | ErrorCode::AuthSuspended => Code::Disabled,
        ErrorCode::ConflictStaleRevision => Code::Changed,
        ErrorCode::Internal => Code::Unsupported,
        _ => Code::Invalid,
    };
    Refusal::new(
        code,
        format!("{}: {}", error.code().as_str(), error.problem.detail),
    )
}

struct Tools {
    app: Weak<AppState>,
}

/// Register this API's existing future-input producer as the engine's managed tool.
pub fn install(app: &App) {
    app.engine.attach_schedule_tools(Arc::new(Tools {
        app: Arc::downgrade(app),
    }));
}

#[async_trait]
impl ScheduleTools for Tools {
    async fn call(
        &self,
        call: &CallContext,
        _thread: &threads::ThreadRow,
    ) -> Result<Json, Refusal> {
        let app = self
            .app
            .upgrade()
            .ok_or_else(|| Refusal::new(Code::Unsupported, "Managed schedules are unavailable."))?;
        dispatch(&app, call).await.map_err(refusal)
    }
}

fn view(row: &rows::ScheduleRow) -> ApiResult<Json> {
    Ok(
        json!({"id":row.id,"kind":"agent","name":row.name,"timing":row.timing,"plan":row.plan,"delivery":row.delivery,
        "state":row.state,"next_run_at":row.next_run_at,"agent":metadata(row)?}),
    )
}

async fn defaults(conn: &mut Conn, thread: Id) -> ApiResult<(i64, i64)> {
    let hours = registry::get(keys::THREADS_AGENT_SCHEDULE_HOURS)
        .ok_or_else(|| ApiError::internal("agent schedule duration is not declared"))?;
    let runs = registry::get(keys::THREADS_AGENT_SCHEDULE_RUNS)
        .ok_or_else(|| ApiError::internal("agent schedule count is not declared"))?;
    let wanted = [hours, runs];
    let snapshot = Snapshot::load(conn, Place::Node(thread), Wanted::Keys(&wanted))
        .await
        .map_err(ApiError::internal)?;
    Ok((
        snapshot
            .resolve(hours)
            .value
            .and_then(|value| value.as_i64())
            .ok_or_else(|| ApiError::internal("invalid agent schedule duration"))?,
        snapshot
            .resolve(runs)
            .value
            .and_then(|value| value.as_i64())
            .ok_or_else(|| ApiError::internal("invalid agent schedule count"))?,
    ))
}

async fn dispatch(app: &App, call: &CallContext) -> ApiResult<Json> {
    let args: Arguments = serde_json::from_value(call.arguments.clone())
        .map_err(|error| ApiError::invalid("arguments", &error.to_string()))?;
    let command = args.command.as_deref().unwrap_or("create");
    let mut tx = app.db.begin().await?;
    orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
    orch_engine::tools::source::check_call(&mut tx, call)
        .await
        .map_err(ApiError::engine)?;
    let thread = threads::lock(&mut tx, call.thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !orch_engine::tools::schedule::offered(&mut tx, &thread)
        .await
        .map_err(ApiError::engine)?
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "agent schedules are off for this thread",
        ));
    }
    let starter: Option<Id> = sqlx::query_scalar("SELECT created_by FROM nodes WHERE id=$1")
        .bind(thread.node_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(DbError::from)?
        .flatten();
    let starter = starter.ok_or_else(ApiError::not_found)?;
    let now = app.clock.now();
    let result = match command {
        "list" => {
            if args.at.is_some()
                || args.in_seconds.is_some()
                || args.cron.is_some()
                || args.note.is_some()
                || args.until.is_some()
                || args.max_runs.is_some()
                || args.id.is_some()
            {
                return Err(ApiError::invalid(
                    "command",
                    "list takes only cursor and limit",
                ));
            }
            let limit = args.limit.unwrap_or(20);
            if !(1..=orch_engine::tools::definitions::PAGE_MAX).contains(&limit) {
                return Err(ApiError::invalid("limit", "use 1 through 200"));
            }
            let selected:Vec<rows::ScheduleRow>=sqlx::query_as("SELECT * FROM input_schedules WHERE topic_id=$1 AND target->>'kind'='thread' AND target->>'thread_id'=$2 AND definition_authority->>'kind'='agent' AND deleted_at IS NULL AND ($3::uuid IS NULL OR id>$3) ORDER BY id LIMIT $4")
                .bind(thread.topic_id).bind(thread.node_id.to_string()).bind(args.cursor).bind(limit+1).fetch_all(&mut *tx).await.map_err(DbError::from)?;
            let mut items = Vec::new();
            let mut bytes = 0_usize;
            for row in selected.iter().take(usize::try_from(limit).unwrap_or(0)) {
                let value = view(row)?;
                let size = serde_json::to_vec(&value)
                    .map_err(ApiError::internal)?
                    .len();
                // The existing complete-read budget bounds a page, never the schedule's lifetime/count.
                if bytes.saturating_add(size)
                    > usize::try_from(orch_engine::tools::definitions::READ_BYTES_MAX).unwrap_or(0)
                {
                    break;
                }
                bytes = bytes.saturating_add(size);
                items.push(value);
            }
            let more = selected.len() > items.len();
            let cursor = more
                .then(|| items.last().and_then(|item| item.get("id")).cloned())
                .flatten();
            json!({"items":items,"next_cursor":cursor})
        }
        "cancel" => {
            if args.at.is_some()
                || args.in_seconds.is_some()
                || args.cron.is_some()
                || args.note.is_some()
                || args.until.is_some()
                || args.max_runs.is_some()
                || args.cursor.is_some()
                || args.limit.is_some()
            {
                return Err(ApiError::invalid("command", "cancel takes only id"));
            }
            let mut row = rows::schedule(
                &mut tx,
                args.id
                    .ok_or_else(|| ApiError::invalid("id", "cancel takes a schedule id"))?,
                true,
            )
            .await?
            .ok_or_else(ApiError::not_found)?;
            let info = metadata(&row)?
                .filter(|info| info.thread_id == thread.node_id)
                .ok_or_else(ApiError::not_found)?;
            if row.state != "deleted" {
                row.state = "deleted".into();
                row.next_run_at = None;
                row.deleted_at = Some(now);
                row.updated_at = now;
                row.revision += 1;
                row.topic_seq = stamp(&mut tx, row.topic_id).await?;
                rows::update(&mut tx, &row).await?;
                record(
                    &mut tx,
                    &row,
                    Actor::Agent(thread.node_id),
                    orch_db::audit::action!(AuditAction::ScheduleCancelled),
                    &Origin::default(),
                    json!({"source_turn":call.turn,"agent":info.agent_id}),
                    now,
                )
                .await?;
            }
            view(&row)?
        }
        "create" => {
            if args.id.is_some() || args.cursor.is_some() || args.limit.is_some() {
                return Err(ApiError::invalid("command", "create takes timing and note"));
            }
            let note = args
                .note
                .as_deref()
                .map(str::trim)
                .filter(|note| {
                    !note.is_empty()
                        && note.chars().count() <= orch_engine::tools::definitions::TEXT_MAX_CHARS
                })
                .ok_or_else(|| {
                    ApiError::invalid("note", "give the future work in 1 through 8000 characters")
                })?;
            let choices = usize::from(args.at.is_some())
                + usize::from(args.in_seconds.is_some())
                + usize::from(args.cron.is_some());
            if choices != 1 {
                return Err(ApiError::invalid(
                    "timing",
                    "choose exactly one of at, in_seconds or cron",
                ));
            }
            let (hours, runs) = defaults(&mut tx, thread.node_id).await?;
            let timing = if let Some(expression) = args.cron {
                InputTiming::Cron {
                    expression,
                    zone: "UTC".into(),
                }
            } else {
                let at = if let Some(at) = args.at {
                    at
                } else {
                    let seconds = args
                        .in_seconds
                        .filter(|seconds| *seconds > 0)
                        .ok_or_else(|| ApiError::invalid("in_seconds", "use a positive delay"))?;
                    let delay = Duration::try_seconds(seconds).ok_or_else(|| {
                        ApiError::invalid(
                            "in_seconds",
                            "the delay is outside the supported calendar",
                        )
                    })?;
                    Time::new(now.checked_add_signed(delay).ok_or_else(|| {
                        ApiError::invalid(
                            "in_seconds",
                            "the delay is outside the supported calendar",
                        )
                    })?)
                };
                InputTiming::Once {
                    at,
                    zone: "UTC".into(),
                }
            };
            let next = timing::next(&timing, now)?;
            let recurring = matches!(timing, InputTiming::Cron { .. });
            if !recurring && (args.until.is_some() || args.max_runs.is_some()) {
                return Err(ApiError::invalid(
                    "timing",
                    "until and max_runs apply to recurring schedules",
                ));
            }
            let until = if recurring {
                match args.until {
                    Some(until) => until,
                    None => Time::new(
                        now.checked_add_signed(Duration::try_hours(hours).ok_or_else(|| {
                            ApiError::invalid("until", "choose an explicit end within the calendar")
                        })?)
                        .ok_or_else(|| {
                            ApiError::invalid("until", "choose an explicit end within the calendar")
                        })?,
                    ),
                }
            } else {
                Time::new(next.due)
            };
            let max_runs = if recurring {
                args.max_runs.unwrap_or(runs)
            } else {
                1
            };
            if until.get() < next.due || max_runs < 1 {
                return Err(ApiError::invalid(
                    "timing",
                    "the end must include a future occurrence and max_runs must be positive",
                ));
            }
            let (agent, plan):(Id,bool)=sqlx::query_as("SELECT s.agent_id,t.plan FROM turns t JOIN segments s ON s.id=t.segment_id WHERE t.id=$1 AND t.thread_id=$2")
                .bind(call.turn).bind(thread.node_id).fetch_one(&mut *tx).await.map_err(DbError::from)?;
            let id = orch_engine::ids::derived(
                "agent-input-schedule",
                &[thread.node_id.as_bytes(), call.call_ref.as_bytes()],
            );
            if let Some(row) = rows::schedule(&mut tx, id, true).await? {
                view(&row)?
            } else {
                let input_authority=app.engine.capture_agent_schedule_authority(&mut tx,thread.node_id,call.turn,starter)
                    .await.map_err(ApiError::engine)?.ok_or_else(||ApiError::new(ErrorCode::AuthzDenied,"the starter's accepted authority is unavailable; ask the starter to send a new input"))?;
                let definition = Definition {
                    kind: "agent".into(),
                    for_user: starter,
                    input_authority,
                    schedule: AgentInputSchedule {
                        thread_id: thread.node_id,
                        agent_id: agent,
                        source_turn_id: call.turn,
                        until,
                        max_runs,
                        runs: 0,
                    },
                };
                let mut row = rows::ScheduleRow {
                    id,
                    topic_id: thread.topic_id,
                    author_id: starter,
                    name: note
                        .lines()
                        .next()
                        .unwrap_or(note)
                        .chars()
                        .take(80)
                        .collect(),
                    target: encode(&InputScheduleTarget::Thread {
                        thread_id: thread.node_id,
                    })?,
                    content: encode(&vec![ContentPart::Text { text: note.into() }])?,
                    plan,
                    delivery: "queue".into(),
                    native_action: None,
                    definition_authority: Some(encode(&definition)?),
                    timing: encode(&timing)?,
                    state: "active".into(),
                    next_run_at: Some(next.due),
                    revision: 1,
                    topic_seq: 0,
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                };
                // Validate current ordinary input acceptance without queueing or issuing native work.
                let mut trial = tx.begin().await.map_err(DbError::from)?;
                let validation = validate(app, &mut trial, &row).await;
                trial.rollback().await.map_err(DbError::from)?;
                validation?;
                orch_engine::tools::source::check_call(&mut tx, call)
                    .await
                    .map_err(ApiError::engine)?;
                row.topic_seq = stamp(&mut tx, row.topic_id).await?;
                rows::insert(&mut tx, &row).await?;
                record(&mut tx,&row,Actor::Agent(thread.node_id),orch_db::audit::action!(AuditAction::ScheduleCreatedByAgent),&Origin::default(),json!({"agent":agent,"source_turn":call.turn,"until":until,"max_runs":max_runs}),now).await?;
                view(&row)?
            }
        }
        _ => {
            return Err(ApiError::invalid(
                "command",
                "choose create, list or cancel",
            ));
        }
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}
