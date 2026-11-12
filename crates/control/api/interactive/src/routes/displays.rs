//! Display requests share the same safe current rows with the unretained display feed.

pub(crate) use super::browsers::holds;

pub(crate) use super::browsers::require;

use crate::middleware::authentication::Caller;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::Guard;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::Registry;

use crate::routes::access::page::NameBounds;

use crate::state::App;

use orch_access_contract::permissions::Permission;

use orch_api_contract::browsers::EnvironmentBrowsersLimit;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::displays::*;

use orch_api_contract::environment::EnvironmentRef;

use orch_api_contract::environment::EnvironmentSupport;

use orch_api_contract::environment::ThroughThreadQuery;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeRef;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::displays as rows;

use orch_engine::displays::CLOSED_LIST_KEPT;

use orch_engine::displays::DEFAULT_MAX;

use orch_engine::displays::DisplayFailure;

use orch_engine::displays::Displays;

use orch_engine::displays::OpenDisplay;

use orch_engine::displays::Requester;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde::de::DeserializeOwned;

use serde_json::json;

use std::sync::Arc;
/// Installs the five declared display requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(
            EnvironmentDisplayCreate,
            super::interactive_creation::display_binding(),
        )
        .on(EnvironmentDisplayList, list)
        .on_with_optional_operation(
            EnvironmentDisplayCreate,
            crate::key_operations::native::stored_deferred::<
                super::interactive_creation::Creation,
                _,
                _,
            >,
            open,
        )
        .on_with_operation(DisplayClose, close_operation, close)
        .creation(
            DisplayTicketCreate,
            super::interactive_tickets::creation::<super::interactive_tickets::DisplayTickets>(),
        )
        .on_with_operation(DisplayTicketCreate, ticket_operation, ticket)
        .on_socket(DisplayAttach, crate::realtime::displays::attach);
}
orch_api_base::__api_bind_0102d264ad!();

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
    current_with_names(app, conn, principal, environment, None).await
}
orch_api_base::__api_bind_c12234bdba!();

async fn current_with_names(
    app: &App,
    conn: &mut Conn,
    principal: Option<&Principal>,
    environment: Id,
    names: Option<&NameBounds>,
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
                let visible = match names {
                    Some(names) => names.refs(conn, p, &[id]).await?,
                    None => {
                        crate::routes::nodes::view::refs(conn, p, &[id], Policy::default()).await?
                    }
                };
                visible.get(&id).filter(|r| !r.redacted).cloned()
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
orch_api_base::__api_bind_3e10c98a8c!();

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

pub(super) async fn display_in(
    app: &App,
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    id: Id,
    names: Option<&NameBounds>,
) -> ApiResult<Display> {
    require(conn, principal, environment, Permission::VncView).await?;
    current_with_names(app, conn, Some(principal), environment, names)
        .await?
        .displays
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(ApiError::not_found)
}

async fn open(
    ctx: Ctx,
    query: ThroughThreadQuery,
    body: EnvironmentDisplayCreateBody,
) -> ApiResult<DisplayCreateResponse> {
    let environment = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let ctx = action_context(ctx, &guard);
    let caller = guard.principal().caller().clone();
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
            requester: Requester::Person(caller.clone()),
            thread: query.thread,
            name: body.name,
            geometry: body.geometry,
            note: None,
            origin: crate::sign_in::origin(&ctx),
        })
        .await;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    match row {
        Ok(row) => super::interactive_creation::display_created(&ctx, &caller, &row).await,
        Err(error) => Err(failure(&ctx.app, error).await),
    }
}

fn close_operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a orch_authz::Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let row = rows::get(conn, ctx.id("display_id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        Ok(super::interactive_close::plan(
            ctx,
            conn,
            caller,
            row.environment_id,
            row.thread_id,
            true,
        )
        .await?
        .0
        .with_data(row.environment_id))
    })
}

async fn close_content(
    ctx: &Ctx,
    caller: &orch_authz::Caller,
    environment: Id,
    id: Id,
) -> ApiResult<Option<Display>> {
    if ctx.key_receipt_required() {
        return Ok(None);
    }
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let now = orch_types::Time::new(ctx.app.clock.now());
    let Some(current) = crate::key_operations::admission::response_caller(
        ctx,
        &mut conn,
        caller,
        ctx.route.name,
        now,
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(row) = rows::get(&mut conn, id).await? else {
        ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
        return Ok(None);
    };
    let (plan, names) = match super::interactive_close::plan(
        ctx,
        &mut conn,
        &current.caller,
        environment,
        row.thread_id,
        false,
    )
    .await
    {
        Ok(plan) => plan,
        Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
            ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    #[cfg(any(test, feature = "test-support"))]
    crate::realtime::history_read_tests::close_name_sources_cut(&plan).await?;
    let app = ctx.app.clone();
    let result = plan
        .read_response(ctx, &mut conn, caller, move |conn, reader| {
            Box::pin(
                async move { display_in(&app, conn, reader, environment, id, Some(&names)).await },
            )
        })
        .await;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    super::interactive_close::projection(ctx, result)
}

async fn close(ctx: Ctx, (): (), (): ()) -> ApiResult<DisplayCloseResponse> {
    let id = ctx.id("display_id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let caller = guard.principal().caller().clone();
    let environment = *guard.operation_data::<Id>()?;
    let ctx = action_context(ctx, &guard).with_admitted_caller(caller.clone())?;
    let closer = ctx.app.engine.displays();
    let result = closer
        .close_in(
            &mut tx,
            id,
            "closed_by_user",
            Requester::Person(caller.clone()),
            &crate::sign_in::origin(&ctx),
            Some(environment),
        )
        .await;
    let closed = match result {
        Ok(closed) => {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            closed
        }
        Err(error) => {
            tx.rollback().await.map_err(orch_db::DbError::from)?;
            let disclose = super::interactive_close::may_disclose(
                &ctx,
                &caller,
                environment,
                Permission::VncView,
            )
            .await;
            return Err(super::interactive_close::error(
                &ctx,
                failure(&ctx.app, error).await,
                disclose,
            ));
        }
    };
    // The actual effect and its audit are committed before best-effort native cleanup.
    // No database lease or later key revocation can strand this accepted close.
    let Some((row, site)) = closed else {
        return Err(ApiError::not_found());
    };
    let receipt = DisplayCloseReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        display_id: id,
        environment_id: environment,
        closed: true,
    };
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    closer.finish_close(id, &site, row.is_some()).await;
    let projected = crate::key_operations::completed_projection::run(
        &ctx,
        close_content(&ctx, &caller, environment, id),
    )
    .await;
    match super::interactive_close::projection(&ctx, projected)? {
        Some(full) if !ctx.key_receipt_required() => Ok(DisplayCloseResponse::Full(Box::new(full))),
        _ => Ok(DisplayCloseResponse::Receipt(receipt)),
    }
}

fn ticket_operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a orch_authz::Caller,
    (): (),
    body: DisplayTicketCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let row = rows::get(conn, ctx.id("display_id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let selection = if body.control.unwrap_or(false) {
            "DisplayTicketCreate.control"
        } else {
            "DisplayTicketCreate.view"
        };
        super::interactive_tickets::plan(ctx, conn, selection, row.environment_id).await
    })
}

async fn ticket(
    ctx: Ctx,
    (): (),
    body: DisplayTicketCreateBody,
) -> ApiResult<DisplayTicketCreateResponse> {
    let id = ctx.id("display_id")?;
    let control = body.control.unwrap_or(false);
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let ctx =
        action_context(ctx, &guard).with_admitted_caller(guard.principal().caller().clone())?;
    let row = rows::get(&mut tx, id)
        .await?
        .filter(|r| r.state == "open")
        .ok_or_else(ApiError::not_found)?;
    if row.environment_id != *guard.operation_data::<Id>()? {
        return Err(ApiError::not_found());
    }
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
    let mut ticket = ctx.app.realtime.displays().ticket_pending(
        &ctx,
        id,
        control,
        row.environment_id,
        machine,
        site.generation,
    )?;
    super::interactive_tickets::record::<super::interactive_tickets::DisplayTickets>(
        &mut ticket,
        &ctx,
        &mut tx,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(DisplayTicketCreateResponse::Full(Box::new(
        ticket.publish()?,
    )))
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
