//! Guarded future-input definitions, private drafts, safe occurrence views and the API's due sweep.

mod runner;
mod timing;

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::view;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::input_schedules::*;
use orch_api_contract::threads::{ContentPart, Delivery};
use orch_api_contract::users::UserRef;
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::{input_schedules as rows, storage, threads};
use orch_db::{Conn, DbError};
use orch_engine::inputs::InputRequest;
use orch_types::{Id, Json, Seq, Time, new_id};
use serde::de::DeserializeOwned;
use serde_json::json;
use sqlx::Acquire;

pub use runner::{run_due, start};

/// Binds the ordinary schedule metadata, authored edit and controller routes.
pub fn mount(registry: &mut Registry) {
    registry
        .on(TopicInputScheduleList, list)
        .on(TopicInputScheduleCreate, create)
        .on(InputScheduleGet, get)
        .on(InputScheduleRunList, runs)
        .on(InputScheduleUpdate, update)
        .on(InputScheduleDelete, delete);
}

fn decode<T: DeserializeOwned>(raw: Json) -> ApiResult<T> {
    serde_json::from_value(raw).map_err(ApiError::internal)
}
fn encode<T: serde::Serialize>(value: &T) -> ApiResult<Json> {
    serde_json::to_value(value).map_err(ApiError::internal)
}
fn target(row: &rows::ScheduleRow) -> ApiResult<InputScheduleTarget> {
    decode(row.target.clone())
}
fn content(row: &rows::ScheduleRow) -> ApiResult<Vec<ContentPart>> {
    decode(row.content.clone())
}
fn timing_of(row: &rows::ScheduleRow) -> ApiResult<InputTiming> {
    decode(row.timing.clone())
}

fn name(value: &str) -> ApiResult<String> {
    let name = value.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::invalid("name", "use 1 to 80 characters"));
    }
    Ok(name.to_owned())
}

fn browser(ctx: &Ctx) -> ApiResult<()> {
    if !ctx.user()?.is_session() {
        return Err(ApiError::new(
            ErrorCode::AuthSessionRequired,
            "a browser sign-in is needed to create, edit or resume future work",
        ));
    }
    Ok(())
}

async fn stamp(conn: &mut Conn, topic: Id) -> ApiResult<Seq> {
    let seq: Seq = sqlx::query_scalar(
        "UPDATE topics SET topic_seq=topic_seq+1 WHERE node_id=$1 RETURNING topic_seq",
    )
    .bind(topic)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?
    .ok_or_else(ApiError::not_found)?;
    orch_db::sequences::notify_topic(conn, topic, seq).await?;
    Ok(seq)
}

/// Authorship survives a lost page grant, while token scopes/subtrees and suspensions still narrow it.
async fn author_reaches(
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::ScheduleRow,
    permission: Permission,
) -> ApiResult<bool> {
    if principal.user() != row.author_id {
        return Ok(false);
    }
    let destination = match target(row)? {
        InputScheduleTarget::Thread { thread_id } => thread_id,
        InputScheduleTarget::NewThread { .. } => row.topic_id,
    };
    let path: Option<Vec<Id>> = sqlx::query_scalar("SELECT path FROM nodes WHERE id=$1")
        .bind(destination)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
    Ok(principal.caller().permits(permission)
        && principal.caller().reaches(path.as_deref())
        && principal
            .suspension()
            .closing(path.as_deref().unwrap_or_default())
            .is_none())
}

async fn allowed(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    permission: Permission,
) -> ApiResult<bool> {
    match orch_authz::nodes::check(conn, principal, id, permission, Act::Use, Policy::default())
        .await
    {
        Ok(decision) => Ok(decision.is_allowed()),
        Err(
            orch_authz::Error::NotFound(_)
            | orch_authz::Error::Archived { .. }
            | orch_authz::Error::Suspended { .. },
        ) => Ok(false),
        Err(error) => Err(ApiError::internal(error)),
    }
}

async fn readable(
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::ScheduleRow,
) -> ApiResult<bool> {
    let (destination, permission) = match target(row)? {
        InputScheduleTarget::Thread { thread_id } => (thread_id, Permission::ThreadView),
        InputScheduleTarget::NewThread { .. } => (row.topic_id, Permission::TopicView),
    };
    Ok(author_reaches(conn, principal, row, permission).await?
        || allowed(conn, principal, destination, permission).await?)
}

async fn controller(
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::ScheduleRow,
) -> ApiResult<()> {
    let (destination, permission) = match target(row)? {
        InputScheduleTarget::Thread { thread_id } => (thread_id, Permission::ThreadInteract),
        InputScheduleTarget::NewThread { .. } => (row.topic_id, Permission::TopicManage),
    };
    if author_reaches(conn, principal, row, permission).await?
        || allowed(conn, principal, destination, permission).await?
    {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "only the author or the current destination controller controls this schedule",
        ))
    }
}

async fn run_view(
    conn: &mut Conn,
    principal: &Principal,
    row: rows::RunRow,
) -> ApiResult<InputScheduleRun> {
    let refs = view::refs(
        conn,
        principal,
        &row.thread_id.into_iter().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let thread = row
        .thread_id
        .and_then(|id| refs.get(&id).filter(|item| !item.redacted).cloned());
    let mut state = row.state;
    let mut reason = row.reason;
    let mut turn_id = row.turn_id;
    let mut started_at = row.started_at;
    if let Some(input) = row.input_id
        && let Some(input) = threads::input(conn, input).await?
    {
        turn_id = input.turn_id;
        if let Some(turn) = input.turn_id
            && let Some(turn) = threads::turn(conn, turn).await?
            && let Some(at) = turn.started_at
        {
            state = "started".to_owned();
            started_at = Some(at);
        }
        if input.state == "cancelled" || input.state == "failed" {
            state = "failed".to_owned();
            reason = Some(input.state_detail.unwrap_or(input.state));
        } else if input.state == "pending" && input.state_detail.is_some() {
            state = "blocked".to_owned();
            reason = input.state_detail;
        }
    }
    Ok(InputScheduleRun {
        id: row.id,
        occurrence: row.occurrence,
        scheduled_for: row.scheduled_for.map(Time::new),
        state: decode(json!(state))?,
        thread,
        input_id: row.input_id,
        turn_id,
        reason,
        created_at: Time::new(row.created_at),
        started_at: started_at.map(Time::new),
    })
}

async fn present(
    conn: &mut Conn,
    principal: &Principal,
    row: rows::ScheduleRow,
) -> ApiResult<InputSchedule> {
    let selected = target(&row)?;
    let thread = match selected {
        InputScheduleTarget::Thread { thread_id } => Some(thread_id),
        InputScheduleTarget::NewThread { .. } => None,
    };
    let ids: Vec<Id> = std::iter::once(row.topic_id).chain(thread).collect();
    let refs = view::refs(conn, principal, &ids, Policy::default()).await?;
    let definition = if row.author_id == principal.user() {
        Some(InputScheduleDefinition {
            target: target(&row)?,
            content: content(&row)?,
            native_action: row.native_action.clone().map(decode).transpose()?,
        })
    } else {
        None
    };
    let author: UserRef = decode(
        orch_engine::views::user_ref_json(conn, row.author_id)
            .await
            .map_err(ApiError::internal)?,
    )?;
    let last_run = match rows::runs(conn, row.id).await?.into_iter().next() {
        Some(run) => Some(run_view(conn, principal, run).await?),
        None => None,
    };
    Ok(InputSchedule {
        id: row.id,
        topic: refs
            .get(&row.topic_id)
            .filter(|item| !item.redacted)
            .cloned(),
        author,
        name: row.name,
        destination: InputScheduleDestination {
            thread: thread.and_then(|id| refs.get(&id).filter(|item| !item.redacted).cloned()),
            new_thread: thread.is_none(),
        },
        definition,
        timing: decode(row.timing)?,
        state: decode(json!(row.state))?,
        next_run_at: row.next_run_at.map(Time::new),
        revision: row.revision,
        last_run,
        created_at: Time::new(row.created_at),
        updated_at: Time::new(row.updated_at),
        topic_seq: row.topic_seq,
    })
}

fn page<T>(items: Vec<T>, query: &PageQuery, id: impl Fn(&T) -> Id) -> ApiResult<Page<T>> {
    let start = match &query.cursor {
        None => 0,
        Some(cursor) => {
            let after: Id = cursor
                .parse()
                .map_err(|_| ApiError::invalid("cursor", "use the returned cursor"))?;
            items
                .iter()
                .position(|item| id(item) == after)
                .map_or(items.len(), |position| position + 1)
        }
    };
    let limit = usize::try_from(query.limit()).unwrap_or(usize::MAX);
    let more = items.len() > start.saturating_add(limit);
    let selected: Vec<T> = items.into_iter().skip(start).take(limit).collect();
    let next_cursor = if more {
        selected.last().map(|item| id(item).to_string())
    } else {
        None
    };
    Ok(Page {
        items: selected,
        next_cursor,
    })
}

/// The same metadata cut used by the initial topic snapshot and later feed updates.
pub(crate) async fn for_topic(
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
    since: Option<Seq>,
) -> ApiResult<Vec<InputSchedule>> {
    let mut items = Vec::new();
    for row in rows::topic(conn, topic, since).await? {
        if readable(conn, principal, &row).await? {
            items.push(present(conn, principal, row).await?);
        }
    }
    Ok(items)
}

async fn list(ctx: Ctx, query: InputScheduleListQuery, (): ()) -> ApiResult<Page<InputSchedule>> {
    let topic = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let items = for_topic(&mut conn, &principal, topic, None)
        .await?
        .into_iter()
        .filter(|row| query.state.is_none_or(|state| row.state == state))
        .collect();
    page(items, &query.page, |row| row.id)
}

async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<InputSchedule> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let row = rows::schedule(&mut conn, ctx.id("id")?, false)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !readable(&mut conn, &principal, &row).await? {
        return Err(ApiError::not_found());
    }
    present(&mut conn, &principal, row).await
}

async fn runs(ctx: Ctx, query: PageQuery, (): ()) -> ApiResult<Page<InputScheduleRun>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let row = rows::schedule(&mut conn, ctx.id("id")?, false)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !readable(&mut conn, &principal, &row).await? {
        return Err(ApiError::not_found());
    }
    let mut items = Vec::new();
    for run in rows::runs(&mut conn, row.id).await? {
        items.push(run_view(&mut conn, &principal, run).await?);
    }
    page(items, &query, |row| row.id)
}

async fn record(
    conn: &mut Conn,
    row: &rows::ScheduleRow,
    actor: Actor,
    action: AuditAction,
    origin: &Origin,
    extra: Json,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<()> {
    let summary = match action {
        AuditAction::InputScheduleCreated => "Created a scheduled input",
        AuditAction::InputScheduleUpdated => "Changed a scheduled input",
        AuditAction::InputSchedulePaused => "Paused future inputs",
        AuditAction::InputScheduleResumed => "Resumed future inputs",
        AuditAction::InputScheduleDeleted => "Removed future inputs",
        AuditAction::InputScheduleRan => "Queued the scheduled input",
        AuditAction::InputScheduleCannotStart => "The scheduled input could not start",
        _ => action.records(),
    };
    let mut entry = Entry::new(
        actor,
        action.as_str(),
        "input_schedule",
        Some(row.id),
        summary,
    );
    entry.node_id = Some(row.topic_id);
    entry.history = true;
    entry.details =
        json!({"schedule":row.id,"author":row.author_id,"name":row.name,"revision":row.revision});
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            entry.details[key] = value.clone();
        }
    }
    audit::append(conn, &entry, origin, now).await?;
    Ok(())
}

async fn creation_check(
    app: &crate::state::AppState,
    tx: &mut orch_db::Tx,
    caller: &Caller,
    row: &rows::ScheduleRow,
) -> ApiResult<()> {
    let mut trial = tx.begin().await.map_err(DbError::from)?;
    let result = runner::accept(app, &mut trial, caller, row, "validation", None).await;
    trial.rollback().await.map_err(DbError::from)?;
    result.map(|_| ())
}

async fn create(ctx: Ctx, (): (), body: InputScheduleCreateBody) -> ApiResult<InputSchedule> {
    browser(&ctx)?;
    if body.native_action.is_some()
        && let InputScheduleTarget::Thread { thread_id } = &body.target
    {
        crate::routes::threads::native::refresh_for_input(&ctx, *thread_id).await?;
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard.into_principal();
    let now = principal.now().get();
    let next = timing::next(&body.timing, now)?;
    let mut row = rows::ScheduleRow {
        id: new_id(),
        topic_id: ctx.id("id")?,
        author_id: principal.user(),
        name: name(body.name.as_deref().unwrap_or("Scheduled input"))?,
        target: encode(&body.target)?,
        content: encode(&body.content)?,
        native_action: body.native_action.as_ref().map(encode).transpose()?,
        timing: encode(&body.timing)?,
        state: "active".to_owned(),
        next_run_at: Some(next.due),
        revision: 1,
        topic_seq: 0,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };
    creation_check(&ctx.app, &mut tx, principal.caller(), &row).await?;
    row.topic_seq = stamp(&mut tx, row.topic_id).await?;
    rows::insert(&mut tx, &row).await?;
    storage::schedule_pins(
        &mut tx,
        row.id,
        &orch_chat::following::content_blobs(&body.content),
        now,
    )
    .await?;
    record(
        &mut tx,
        &row,
        Actor::User(principal.user()),
        orch_db::audit::action!(AuditAction::InputScheduleCreated),
        &crate::sign_in::origin(&ctx),
        json!({}),
        now,
    )
    .await?;
    let result = present(&mut tx, &principal, row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn update(ctx: Ctx, (): (), body: InputScheduleUpdateBody) -> ApiResult<InputSchedule> {
    let definition = body.name.is_some()
        || body.target.is_some()
        || body.content.is_some()
        || body.native_action.is_some()
        || body.timing.is_some();
    if definition || body.state == Some(InputScheduleState::Active) {
        browser(&ctx)?;
    }
    if body.state.is_some_and(|state| {
        !matches!(
            state,
            InputScheduleState::Active | InputScheduleState::Paused
        )
    }) {
        return Err(ApiError::invalid("state", "choose active or paused"));
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard.into_principal();
    let now = principal.now().get();
    let mut row = rows::schedule(&mut tx, ctx.id("id")?, true)
        .await?
        .ok_or_else(ApiError::not_found)?;
    controller(&mut tx, &principal, &row).await?;
    if row.revision != body.revision {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the schedule changed since it was read",
        )
        .with("current", present(&mut tx, &principal, row).await?));
    }
    if row.state == "deleted" {
        return Err(ApiError::not_found());
    }
    if definition && row.author_id != principal.user() {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "only its author edits the prepared definition",
        ));
    }
    let previous = row.state.clone();
    if let Some(value) = body.name {
        row.name = name(&value)?;
    }
    if let Some(value) = body.target {
        row.target = encode(&value)?;
    }
    if let Some(value) = body.content {
        row.content = encode(&value)?;
    }
    if let Some(value) = body.native_action {
        row.native_action = value.as_ref().map(encode).transpose()?;
    }
    if let Some(value) = body.timing {
        row.timing = encode(&value)?;
    }
    if let Some(state) = body.state {
        row.state = match state {
            InputScheduleState::Active => "active",
            _ => "paused",
        }
        .to_owned();
    }
    if definition || (previous != "active" && row.state == "active") {
        let next = timing::next(&timing_of(&row)?, now)?;
        row.next_run_at = Some(next.due);
        creation_check(&ctx.app, &mut tx, principal.caller(), &row).await?;
    }
    if row.state != "active" {
        row.next_run_at = None;
    }
    row.revision += 1;
    row.updated_at = now;
    row.topic_seq = stamp(&mut tx, row.topic_id).await?;
    rows::update(&mut tx, &row).await?;
    storage::schedule_pins(
        &mut tx,
        row.id,
        &orch_chat::following::content_blobs(&content(&row)?),
        now,
    )
    .await?;
    if definition {
        record(
            &mut tx,
            &row,
            Actor::User(principal.user()),
            orch_db::audit::action!(AuditAction::InputScheduleUpdated),
            &crate::sign_in::origin(&ctx),
            json!({}),
            now,
        )
        .await?;
    }
    if previous != row.state {
        let action = if row.state == "active" {
            orch_db::audit::action!(AuditAction::InputScheduleResumed)
        } else {
            orch_db::audit::action!(AuditAction::InputSchedulePaused)
        };
        record(
            &mut tx,
            &row,
            Actor::User(principal.user()),
            action,
            &crate::sign_in::origin(&ctx),
            json!({}),
            now,
        )
        .await?;
    }
    let result = present(&mut tx, &principal, row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard.into_principal();
    let now = principal.now().get();
    let mut row = rows::schedule(&mut tx, ctx.id("id")?, true)
        .await?
        .ok_or_else(ApiError::not_found)?;
    controller(&mut tx, &principal, &row).await?;
    if row.state != "deleted" {
        row.state = "deleted".to_owned();
        row.next_run_at = None;
        row.deleted_at = Some(now);
        row.updated_at = now;
        row.revision += 1;
        row.topic_seq = stamp(&mut tx, row.topic_id).await?;
        rows::update(&mut tx, &row).await?;
        storage::schedule_pins(&mut tx, row.id, &[], now).await?;
        record(
            &mut tx,
            &row,
            Actor::User(principal.user()),
            orch_db::audit::action!(AuditAction::InputScheduleDeleted),
            &crate::sign_in::origin(&ctx),
            json!({}),
            now,
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
