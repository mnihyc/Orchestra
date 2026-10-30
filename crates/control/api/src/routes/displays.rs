//! Display requests share the same safe current rows with the unretained display feed.

pub(crate) use super::browsers::{holds, require};
use crate::middleware::authentication::Caller;
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, Guard};
use crate::registry::{Ctx, Registry};
use crate::state::App;
use orch_access_contract::permissions::Permission;
use orch_api_contract::browsers::EnvironmentBrowsersLimit;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::displays::*;
use orch_api_contract::environment::{EnvironmentRef, EnvironmentSupport, ThroughThreadQuery};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_authz::{Policy, Principal};
use orch_db::{Conn, queries::displays as rows};
use orch_engine::displays::{
    CLOSED_LIST_KEPT, DEFAULT_MAX, DisplayFailure, Displays, OpenDisplay, Requester,
};
use orch_types::{Id, Json, Time};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::Arc;

/// Installs the five declared display requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(EnvironmentDisplayList, list)
        .on(EnvironmentDisplayCreate, open)
        .on(DisplayClose, close)
        .on(DisplayTicketCreate, ticket)
        .on_socket(DisplayAttach, crate::realtime::displays::attach);
}
fn parse<T: DeserializeOwned>(value: &str) -> ApiResult<T> {
    serde_json::from_value(json!(value)).map_err(ApiError::internal)
}

/// Builds HTTP and canonical feed rows without ever serializing a password.
pub(crate) async fn current(
    app: &App,
    conn: &mut Conn,
    principal: Option<&Principal>,
    environment: Id,
) -> ApiResult<EnvironmentDisplays> {
    let site = rows::site(conn, environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let manages = match principal {
        Some(p) => holds(conn, p, environment, Permission::VncManage).await?,
        None => true,
    };
    let off = Displays::off(conn, &site)
        .await
        .map_err(ApiError::internal)?;
    let support = serde_json::from_value(site.facts["vnc"].clone())
        .unwrap_or_else(|_| EnvironmentSupport::nothing_found().vnc);
    let max = orch_settings::get::<i64>(
        conn,
        orch_settings::Place::Environment(environment),
        "vnc.max_per_environment",
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(DEFAULT_MAX);
    let count = rows::owned_count(conn, environment).await?;
    let found = rows::list(conn, environment, app.clock.now() - CLOSED_LIST_KEPT).await?;
    let people: Vec<_> = found
        .iter()
        .filter_map(|r| r.opened_by)
        .chain(std::iter::once(site.owner_id))
        .collect();
    let people = refusals::user_refs(conn, &people).await?;
    let owner = people
        .get(&site.owner_id)
        .cloned()
        .ok_or_else(|| ApiError::internal("the environment's owner is missing"))?;
    let addresses = crate::routes::resources::view::addresses(conn, &[environment]).await?;
    let exposed =
        crate::routes::ports::exposures(conn, &[environment], Time::new(app.clock.now())).await?;
    let env = EnvironmentRef {
        activity: None,
        id: environment,
        name: site.name,
        owner,
        address: addresses.get(&environment).cloned().unwrap_or_default(),
        placement: parse(&site.placement)?,
        status: parse(&site.status)?,
        ports_exposed: exposed.contains(&environment),
    };
    let mut displays = Vec::new();
    for row in found {
        let thread = match (row.thread_id, principal) {
            (Some(id), Some(p)) => {
                crate::routes::nodes::view::refs(conn, p, &[id], Policy::default())
                    .await?
                    .get(&id)
                    .filter(|r| !r.redacted)
                    .cloned()
            }
            (Some(id), None) => {
                orch_db::queries::browsers::node_name(conn, id)
                    .await?
                    .map(|name| NodeRef {
                        thread_display: None,
                        id,
                        kind: NodeKind::Thread,
                        name: Some(name),
                        redacted: false,
                    })
            }
            _ => None,
        };
        let driving = app.realtime.displays().driving(row.id, app.clock.now());
        let drivers = refusals::user_refs(conn, &driving).await?;
        displays.push(Display {
            id: row.id,
            environment: env.clone(),
            name: row.name,
            origin: parse(&row.origin)?,
            opened_by: row.opened_by.and_then(|id| people.get(&id).cloned()),
            thread,
            endpoint: (manages && !row.endpoint.is_empty()).then_some(row.endpoint),
            x_display: row.x_display,
            geometry: row.geometry,
            watching: app.realtime.displays().watching(row.id),
            driving: driving
                .iter()
                .filter_map(|id| drivers.get(id).cloned())
                .collect(),
            note: row.note,
            browsers: rows::browsers(conn, row.id).await?,
            state: parse(&row.state)?,
            close_reason: row.close_reason,
            opened_at: Time::new(row.opened_at),
            last_used_at: Time::new(row.last_used_at),
            closed_at: row.closed_at.map(Time::new),
        });
    }
    Ok(EnvironmentDisplays {
        off,
        support,
        displays: if off.is_none() { displays } else { Vec::new() },
        limit: EnvironmentBrowsersLimit { open: count, max },
    })
}

/// Applies each subscriber's current Manage and thread visibility before publication.
pub(crate) async fn filter_list(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    mut data: Json,
) -> ApiResult<Json> {
    let manages = holds(conn, principal, environment, Permission::VncManage).await?;
    let refs: Vec<_> = data["displays"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            row["thread"]["id"]
                .as_str()
                .and_then(|id| id.parse::<Id>().ok())
        })
        .collect();
    let seen = crate::routes::nodes::view::refs(conn, principal, &refs, Policy::default()).await?;
    if let Some(displays) = data["displays"].as_array_mut() {
        for row in displays {
            if !manages {
                row["endpoint"] = Json::Null;
            }
            if let Some(id) = row["thread"]["id"]
                .as_str()
                .and_then(|id| id.parse::<Id>().ok())
                && !seen.get(&id).is_some_and(|r| !r.redacted)
            {
                row["thread"] = Json::Null;
            }
        }
    }
    Ok(data)
}

async fn list(
    ctx: Ctx,
    query: EnvironmentDisplayListQuery,
    (): (),
) -> ApiResult<EnvironmentDisplays> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(&mut conn, &principal, id, Permission::VncView).await?;
    if query.look == Some(true) {
        let site = rows::site(&mut conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if site.status == "running" && site.machine_online == Some(true) {
            drop(conn);
            ctx.app
                .engine
                .browsers()
                .look(id)
                .await
                .map_err(ApiError::internal)?;
            conn = ctx.app.db.acquire().await?;
        }
    }
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(&mut conn, &principal, id, Permission::VncView).await?;
    current(&ctx.app, &mut conn, Some(&principal), id).await
}

fn action_context(mut ctx: Ctx, guard: &Guard) -> Ctx {
    ctx.caller = Caller::User(Arc::new(guard.user().clone()));
    ctx.now = guard.now().into();
    ctx
}
async fn display(ctx: &Ctx, row: &rows::DisplayRow) -> ApiResult<Display> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    require(
        &mut conn,
        &principal,
        row.environment_id,
        Permission::VncView,
    )
    .await?;
    current(&ctx.app, &mut conn, Some(&principal), row.environment_id)
        .await?
        .displays
        .into_iter()
        .find(|d| d.id == row.id)
        .ok_or_else(ApiError::not_found)
}
async fn open(
    ctx: Ctx,
    query: ThroughThreadQuery,
    body: EnvironmentDisplayCreateBody,
) -> ApiResult<Display> {
    let environment = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let ctx = action_context(ctx, &guard);
    require(
        &mut tx,
        guard.principal(),
        environment,
        Permission::VncManage,
    )
    .await?;
    if let Some(thread) = query.thread {
        crate::routes::workspace::door::beside(&mut tx, guard.principal(), environment, thread)
            .await?;
    }
    let row = ctx
        .app
        .engine
        .displays()
        .open(OpenDisplay {
            environment,
            requester: Requester::Person(authorization::caller_of(ctx.user()?)?),
            thread: query.thread,
            name: body.name,
            geometry: body.geometry,
            note: None,
            origin: crate::sign_in::origin(&ctx),
        })
        .await;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    match row {
        Ok(row) => display(&ctx, &row).await,
        Err(error) => Err(failure(&ctx.app, error).await),
    }
}
async fn close(ctx: Ctx, (): (), (): ()) -> ApiResult<Display> {
    let id = ctx.id("display_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let ctx = action_context(ctx, &guard);
    let result = ctx
        .app
        .engine
        .displays()
        .close(
            id,
            "closed_by_user",
            Requester::Person(authorization::caller_of(ctx.user()?)?),
            &crate::sign_in::origin(&ctx),
        )
        .await;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let row = match result {
        Ok(Some(row)) => row,
        Ok(None) => {
            let mut conn = ctx.app.db.acquire().await?;
            rows::get(&mut conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?
        }
        Err(error) => return Err(failure(&ctx.app, error).await),
    };
    display(&ctx, &row).await
}
async fn ticket(
    ctx: Ctx,
    (): (),
    body: DisplayTicketCreateBody,
) -> ApiResult<DisplayTicketCreateReply> {
    let id = ctx.id("display_id")?;
    let control = body.control.unwrap_or(false);
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let ctx = action_context(ctx, &guard);
    let row = rows::get(&mut tx, id)
        .await?
        .filter(|r| r.state == "open")
        .ok_or_else(ApiError::not_found)?;
    require(
        &mut tx,
        guard.principal(),
        row.environment_id,
        Permission::VncView,
    )
    .await?;
    if control {
        require(
            &mut tx,
            guard.principal(),
            row.environment_id,
            Permission::VncManage,
        )
        .await?;
    }
    let site = rows::site(&mut tx, row.environment_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if let Some(off) = Displays::off(&mut tx, &site)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(failure(&ctx.app, DisplayFailure::Off(off)).await);
    }
    if row.env_generation != site.generation {
        return Err(ApiError::not_found());
    }
    let machine = site.machine_id.ok_or_else(ApiError::not_found)?;
    let ticket = ctx
        .app
        .realtime
        .displays()
        .ticket(&ctx, id, control, machine, site.generation)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(ticket)
}

pub(crate) async fn failure(app: &App, error: DisplayFailure) -> ApiError {
    match error {
        DisplayFailure::Permission(decision) => match app.db.acquire().await {
            Ok(mut conn) => refusals::settle(&mut conn, *decision)
                .await
                .err()
                .unwrap_or_else(|| ApiError::internal("a refusal was allowed")),
            Err(error) => ApiError::internal(error),
        },
        DisplayFailure::Off(off) | DisplayFailure::DisplayOff(off) => {
            ApiError::new(ErrorCode::VncNotAllowed, "Displays are off here.").with("reason", off)
        }
        DisplayFailure::Unsupported => ApiError::new(
            ErrorCode::VncNotAllowed,
            "No supported display server was found.",
        )
        .with("reason", "unsupported"),
        DisplayFailure::TooMany { open, max } | DisplayFailure::DisplayTooMany { open, max } => {
            ApiError::new(
                ErrorCode::VncTooMany,
                "The environment's display limit is full.",
            )
            .with("open", open)
            .with("limit", max)
        }
        DisplayFailure::NotFound => ApiError::not_found(),
        DisplayFailure::Invalid(detail) => ApiError::new(ErrorCode::RequestInvalid, detail),
        DisplayFailure::Command(error) if error.code == "unsupported" => ApiError::new(
            ErrorCode::VncNotAllowed,
            "No supported display server was found.",
        )
        .with("reason", "unsupported"),
        DisplayFailure::Command(_) | DisplayFailure::Unreachable(_) => ApiError::new(
            ErrorCode::VncStartFailed,
            "The display could not be started or reached.",
        ),
        DisplayFailure::Engine(error) => ApiError::internal(error),
    }
}
