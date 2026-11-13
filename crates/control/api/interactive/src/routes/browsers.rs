//! Browser requests and the whole current list shared with the unretained browser feed.

use crate::middleware::authentication::Caller;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::Guard;

use crate::registry::Answer;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::Registry;

use crate::routes::access::page::NameBounds;

use crate::state::App;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::browsers::*;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::environment::EnvironmentRef;

use orch_api_contract::environment::EnvironmentSupport;

use orch_api_contract::environment::ThroughThreadQuery;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeRef;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::browsers as rows;

use orch_engine::browsers::BrowserFailure;

use orch_engine::browsers::Browsers;

use orch_engine::browsers::CLOSED_LIST_KEPT;

use orch_engine::browsers::OpenBrowser;

use orch_engine::browsers::Requester;

use orch_link_contract::browser::BrowserProfileAction;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde::de::DeserializeOwned;

use serde_json::json;

use std::sync::Arc;
/// A place may replace the shipped limit of four owned browsers.
const DEFAULT_MAX: i64 = 4;

/// Installs the eight declared browser and profile requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(
            EnvironmentBrowserCreate,
            super::interactive_creation::browser_binding(),
        )
        .on(EnvironmentBrowserList, list)
        .on_with_optional_operation(
            EnvironmentBrowserCreate,
            crate::key_operations::native::stored_deferred::<
                super::interactive_creation::Creation,
                _,
                _,
            >,
            open,
        )
        .on_with_operation(BrowserClose, close_operation, close)
        .creation(
            BrowserTicketCreate,
            super::interactive_tickets::creation::<super::interactive_tickets::BrowserTickets>(),
        )
        .on_with_operation(BrowserTicketCreate, ticket_operation, ticket)
        .creation(BrowserProfileCreate, profiles::binding())
        .on(BrowserProfileCreate, create_profile)
        .on(BrowserProfileUpdate, rename_profile)
        .on(BrowserProfileDelete, delete_profile)
        .on_socket(BrowserAttach, crate::realtime::browsers::attach);
}
orch_api_base::__api_bind_ea12023e1e!();

fn parse<T: DeserializeOwned>(value: &str) -> ApiResult<T> {
    serde_json::from_value(json!(value)).map_err(ApiError::internal)
}

/// Checks one door on the environment, preserving the caller's token limits.
pub(crate) async fn holds(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    permission: Permission,
) -> ApiResult<bool> {
    orch_authz::resources::check(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        permission,
        Policy::default(),
    )
    .await
    .map(|d| d.is_allowed())
    .map_err(ApiError::internal)
}
orch_api_base::__api_bind_0ebb32af85!();

pub(crate) async fn require(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    permission: Permission,
) -> ApiResult<()> {
    let decision = orch_authz::resources::check(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        permission,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    refusals::settle(conn, decision).await
}

/// Builds exactly the current browser/profile rows for HTTP and for publication.
pub(crate) async fn current(
    app: &App,
    conn: &mut Conn,
    principal: Option<&Principal>,
    environment: Id,
) -> ApiResult<EnvironmentBrowsers> {
    current_with_names(app, conn, principal, environment, None).await
}
orch_api_base::__api_bind_3f4b14d8fe!();

async fn current_with_names(
    app: &App,
    conn: &mut Conn,
    principal: Option<&Principal>,
    environment: Id,
    names: Option<&NameBounds>,
) -> ApiResult<EnvironmentBrowsers> {
    let site = rows::site(conn, environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let manages = match principal {
        Some(p) => holds(conn, p, environment, Permission::BrowserManage).await?,
        None => true,
    };
    let off = Browsers::off(conn, &site)
        .await
        .map_err(ApiError::internal)?;
    let support = serde_json::from_value(site.facts["browser"].clone())
        .unwrap_or_else(|_| EnvironmentSupport::nothing_found().browser);
    let max = orch_settings::get::<i64>(
        conn,
        orch_settings::Place::Environment(environment),
        "browser.max_per_environment",
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(DEFAULT_MAX);
    let count = rows::owned_count(conn, environment).await?;
    let profiles = app
        .engine
        .browsers()
        .memory()
        .profiles(environment)
        .unwrap_or_default();
    let profiles = profiles
        .into_iter()
        .map(|p| BrowserProfile {
            name: p.name,
            size_bytes: p.size_bytes,
            used_at: p.used_at,
            browser: p.browser,
        })
        .collect();
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
    let exposed = crate::routes::ports::exposures(
        conn,
        &[environment],
        orch_types::Time::new(app.clock.now()),
    )
    .await?;
    let env = EnvironmentRef {
        activity: None,
        ports_exposed: exposed.contains(&environment),
        id: environment,
        name: site.name.clone(),
        owner,
        address: addresses.get(&environment).cloned().unwrap_or_default(),
        placement: parse(&site.placement)?,
        status: parse(&site.status)?,
    };
    let mut browsers = Vec::new();
    for row in found {
        let thread = match (row.thread_id, principal) {
            (Some(id), Some(principal)) => {
                let visible = match names {
                    Some(names) => names.refs(conn, principal, &[id]).await?,
                    None => {
                        crate::routes::nodes::view::refs(conn, principal, &[id], Policy::default())
                            .await?
                    }
                };
                visible.get(&id).filter(|r| !r.redacted).cloned()
            }
            (Some(id), None) => {
                let name = rows::node_name(conn, id).await?;
                name.map(|name| NodeRef {
                    thread_display: None,
                    id,
                    kind: NodeKind::Thread,
                    name: Some(name),
                    redacted: false,
                })
            }
            _ => None,
        };
        let watching = app.engine.browsers().memory().watching(row.id);
        let driving = app.realtime.browsers().driving(row.id, app.clock.now());
        let drivers = refusals::user_refs(conn, &driving).await?;
        browsers.push(Browser {
            id: row.id,
            environment: env.clone(),
            profile: row.profile,
            origin: parse(&row.origin)?,
            opened_by: row.opened_by.and_then(|p| people.get(&p).cloned()),
            thread,
            preset: row.preset.as_deref().map(parse).transpose()?,
            display: row.display_id,
            endpoint: (manages && !row.endpoint.is_empty()).then_some(row.endpoint),
            program: row.program,
            note: row.note,
            tabs: app.engine.browsers().memory().tabs(row.id),
            watching,
            driving: driving
                .iter()
                .filter_map(|p| drivers.get(p).cloned())
                .collect(),
            state: parse(&row.state)?,
            close_reason: row.close_reason,
            opened_at: Time::new(row.opened_at),
            last_used_at: Time::new(row.last_used_at),
            closed_at: row.closed_at.map(Time::new),
        });
    }
    let presets = vec![
        EnvironmentBrowsersPreset {
            name: BrowserPreset::Headless,
            available: !support.programs.is_empty(),
            reason: support
                .programs
                .is_empty()
                .then(|| "No supported browser was found.".to_owned()),
        },
        EnvironmentBrowsersPreset {
            name: BrowserPreset::Window,
            available: !support.programs.is_empty() && support.window,
            reason: (!support.window || support.programs.is_empty())
                .then(|| "No display for a browser window was found.".to_owned()),
        },
    ];
    let profiles_dir = manages
        .then(|| {
            site.facts
                .get("browser_profiles_dir")
                .and_then(Json::as_str)
                .map(str::to_owned)
        })
        .flatten();
    Ok(EnvironmentBrowsers {
        off,
        support,
        presets,
        profiles: if off.is_none() { profiles } else { Vec::new() },
        profiles_dir,
        browsers: if off.is_none() { browsers } else { Vec::new() },
        limit: EnvironmentBrowsersLimit { open: count, max },
    })
}

/// Cuts a canonical publication down for this feed subscriber before it is sent.
pub(crate) async fn filter_list(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    mut data: Json,
) -> ApiResult<Json> {
    let manages = holds(conn, principal, environment, Permission::BrowserManage).await?;
    let mut refs = Vec::new();
    if let Some(browsers) = data["browsers"].as_array() {
        for browser in browsers {
            if let Some(id) = browser["thread"]["id"]
                .as_str()
                .and_then(|id| id.parse::<Id>().ok())
            {
                refs.push(id);
            }
        }
    }
    let seen = crate::routes::nodes::view::refs(conn, principal, &refs, Policy::default()).await?;
    if let Some(browsers) = data["browsers"].as_array_mut() {
        for browser in browsers {
            if !manages {
                browser["endpoint"] = Json::Null;
            }
            if let Some(id) = browser["thread"]["id"]
                .as_str()
                .and_then(|id| id.parse::<Id>().ok())
                && !seen.get(&id).is_some_and(|r| !r.redacted)
            {
                browser["thread"] = Json::Null;
            }
        }
    }
    Ok(data)
}
orch_api_base::__api_bind_66bd6d39aa!();

async fn list(
    ctx: Ctx,
    query: EnvironmentBrowserListQuery,
    (): (),
) -> ApiResult<EnvironmentBrowsers> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(&mut conn, &principal, id, Permission::BrowserView).await?;
    if query.look == Some(true) || ctx.app.engine.browsers().memory().profiles(id).is_none() {
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
    let current = current(&ctx.app, &mut conn, Some(&principal), id).await?;
    drop(conn);
    ctx.app.engine.browsers().changed(id).await;
    Ok(current)
}

pub(super) async fn browser_in(
    app: &App,
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    id: Id,
    names: Option<&NameBounds>,
) -> ApiResult<Browser> {
    current_with_names(app, conn, Some(principal), environment, names)
        .await?
        .browsers
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(ApiError::not_found)
}

// Browser operations use the refreshed identity while a shared guard protects the engine's transactions.
fn action_context(mut ctx: Ctx, guard: &Guard) -> Ctx {
    ctx.caller = Caller::User(Arc::new(guard.user().clone()));
    ctx.now = guard.now().into();
    ctx
}

async fn open(
    ctx: Ctx,
    query: ThroughThreadQuery,
    body: EnvironmentBrowserCreateBody,
) -> ApiResult<Answer<BrowserCreateResponse>> {
    let environment = ctx.id("id")?;
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let ctx = action_context(ctx, &guard);
    let principal = guard.principal();
    let caller = principal.caller().clone();
    require(&mut conn, principal, environment, Permission::BrowserManage).await?;
    if let Some(thread) = query.thread {
        crate::routes::workspace::door::beside(&mut conn, principal, environment, thread).await?;
    }
    if body
        .url
        .as_deref()
        .is_some_and(|u| url::Url::parse(u).is_ok_and(|url| url.scheme() == "file"))
    {
        require(&mut conn, principal, environment, Permission::FilesView).await?;
    }
    let result = ctx
        .app
        .engine
        .browsers()
        .open(OpenBrowser {
            environment,
            requester: Requester::Person(caller.clone()),
            thread: query.thread,
            profile: body.profile,
            create: body.create.unwrap_or(false),
            preset: body.preset,
            url: body.url,
            note: None,
            origin: crate::sign_in::origin(&ctx),
        })
        .await;
    let opened = match result {
        Ok(value) => value,
        Err(error) => return Err(failure(&ctx.app, error).await),
    };
    conn.commit().await.map_err(orch_db::DbError::from)?;
    let status = if opened.started {
        http::StatusCode::CREATED
    } else {
        http::StatusCode::OK
    };
    Ok(Answer::from(
        super::interactive_creation::browser_created(&ctx, &caller, &opened.row).await?,
    )
    .status(status))
}

fn close_operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a orch_authz::Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let row = rows::get(conn, ctx.id("browser_id")?)
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
) -> ApiResult<Option<Browser>> {
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
            Box::pin(async move {
                require(conn, reader, environment, Permission::BrowserView).await?;
                browser_in(&app, conn, reader, environment, id, Some(&names)).await
            })
        })
        .await;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    super::interactive_close::projection(ctx, result)
}

async fn close(ctx: Ctx, (): (), (): ()) -> ApiResult<BrowserCloseResponse> {
    let id = ctx.id("browser_id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let caller = guard.principal().caller().clone();
    let environment = *guard.operation_data::<Id>()?;
    let ctx = action_context(ctx, &guard).with_admitted_caller(caller.clone())?;
    let closer = ctx.app.engine.browsers();
    let result = closer
        .close_in(
            &mut tx,
            id,
            "quit",
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
                Permission::BrowserView,
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
    let receipt = BrowserCloseReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        browser_id: id,
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
        Some(full) if !ctx.key_receipt_required() => Ok(BrowserCloseResponse::Full(Box::new(full))),
        _ => Ok(BrowserCloseResponse::Receipt(receipt)),
    }
}

fn ticket_operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a orch_authz::Caller,
    (): (),
    body: BrowserTicketCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let row = rows::get(conn, ctx.id("browser_id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let selection = if body.control.unwrap_or(false) {
            "BrowserTicketCreate.control"
        } else {
            "BrowserTicketCreate.view"
        };
        super::interactive_tickets::plan(ctx, conn, selection, row.environment_id).await
    })
}

async fn ticket(
    ctx: Ctx,
    (): (),
    body: BrowserTicketCreateBody,
) -> ApiResult<BrowserTicketCreateResponse> {
    let id = ctx.id("browser_id")?;
    let control = body.control.unwrap_or(false);
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let ctx =
        action_context(ctx, &guard).with_admitted_caller(guard.principal().caller().clone())?;
    let row = rows::get(&mut conn, id)
        .await?
        .filter(|r| r.state == "open")
        .ok_or_else(ApiError::not_found)?;
    if row.environment_id != *guard.operation_data::<Id>()? {
        return Err(ApiError::not_found());
    }
    let principal = guard.principal();
    require(
        &mut conn,
        principal,
        row.environment_id,
        Permission::BrowserView,
    )
    .await?;
    if control {
        require(
            &mut conn,
            principal,
            row.environment_id,
            Permission::BrowserManage,
        )
        .await?;
    }
    let site = rows::site(&mut conn, row.environment_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if let Some(off) = Browsers::off(&mut conn, &site)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(
            ApiError::new(ErrorCode::BrowserNotAllowed, "browsers are off here")
                .with("reason", off),
        );
    }
    let mut result = ctx.app.realtime.browsers().ticket_pending(
        &ctx,
        id,
        control,
        row.environment_id,
        site.machine_id,
        row.env_generation,
    )?;
    super::interactive_tickets::record::<super::interactive_tickets::BrowserTickets>(
        &mut result,
        &ctx,
        &mut conn,
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(BrowserTicketCreateResponse::Full(Box::new(
        result.publish()?,
    )))
}

async fn change_profile(
    ctx: &Ctx,
    command: Option<Id>,
    action: BrowserProfileAction,
    name: String,
    to: Option<String>,
) -> ApiResult<(Vec<BrowserProfile>, orch_authz::Caller)> {
    let environment = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(ctx).await?;
    let accepted = guard.principal().caller().clone();
    let ctx = action_context(ctx.clone(), &guard);
    let result = ctx
        .app
        .engine
        .browsers()
        .profile_with_id(
            command.unwrap_or_else(orch_types::new_id),
            environment,
            Requester::Person(accepted.clone()),
            action,
            name.clone(),
            to,
            &crate::sign_in::origin(&ctx),
        )
        .await;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    match result {
        Ok(result) => Ok((
            result
                .profiles
                .into_iter()
                .map(|p| BrowserProfile {
                    name: p.name,
                    size_bytes: p.size_bytes,
                    used_at: p.used_at,
                    browser: p.browser,
                })
                .collect(),
            accepted,
        )),
        Err(error) => {
            let in_use =
                matches!(&error,BrowserFailure::Command(command) if command.code=="in_use");
            let mut problem = failure(&ctx.app, error).await;
            if in_use {
                let mut conn = ctx.app.db.acquire().await?;
                let browser = rows::on_profile(&mut conn, environment, &name)
                    .await?
                    .map(|row| row.id);
                problem = problem.with("browser", browser);
            }
            Err(problem)
        }
    }
}

async fn create_profile(
    ctx: Ctx,
    (): (),
    body: BrowserProfileCreateBody,
) -> ApiResult<BrowserProfileCreateResponse> {
    let command = orch_types::new_id();
    let (profiles, accepted) = change_profile(
        &ctx,
        Some(command),
        BrowserProfileAction::Create,
        body.name.clone(),
        None,
    )
    .await?;
    let profile = profiles
        .into_iter()
        .find(|p| p.name == body.name)
        .ok_or_else(ApiError::not_found)?;
    profiles::first(&ctx, &accepted, command, profile).await
}

async fn rename_profile(
    ctx: Ctx,
    (): (),
    body: BrowserProfileUpdateBody,
) -> ApiResult<BrowserProfile> {
    change_profile(
        &ctx,
        None,
        BrowserProfileAction::Rename,
        ctx.param("name")?.to_owned(),
        Some(body.name.clone()),
    )
    .await?
    .0
    .into_iter()
    .find(|p| p.name == body.name)
    .ok_or_else(ApiError::not_found)
}

async fn delete_profile(ctx: Ctx, query: BrowserProfileDeleteQuery, (): ()) -> ApiResult<()> {
    let name = ctx.param("name")?.to_owned();
    if query.confirm != name {
        return Err(ApiError::invalid("confirm", "must be the profile's name"));
    }
    change_profile(&ctx, None, BrowserProfileAction::Delete, name, None).await?;
    Ok(())
}

pub(crate) async fn failure(app: &App, error: BrowserFailure) -> ApiError {
    match error {
        BrowserFailure::NotFound => ApiError::not_found(),
        BrowserFailure::Invalid(message) => ApiError::invalid("browser", &message),
        BrowserFailure::Off(reason) => {
            ApiError::new(ErrorCode::BrowserNotAllowed, "browsers are off here")
                .with("reason", reason)
        }
        BrowserFailure::DisplayOff(reason) => ApiError::new(
            ErrorCode::VncNotAllowed,
            "The requested browser window needs displays switched on.",
        )
        .with("reason", reason),
        BrowserFailure::Unsupported => ApiError::new(
            ErrorCode::BrowserNotAllowed,
            "no supported program was found",
        )
        .with("reason", "unsupported"),
        BrowserFailure::TooMany { open, max } => ApiError::new(
            ErrorCode::BrowserTooMany,
            "the environment's browser limit is reached",
        )
        .with("open", open)
        .with("limit", max),
        BrowserFailure::DisplayTooMany { open, max } => {
            ApiError::new(ErrorCode::VncTooMany, "The shared display limit is full.")
                .with("open", open)
                .with("limit", max)
        }
        BrowserFailure::Permission(decision) => match app.db.acquire().await {
            Ok(mut conn) => refusals::settle(&mut conn, *decision)
                .await
                .err()
                .unwrap_or_else(|| ApiError::internal("a refusal was allowed")),
            Err(e) => ApiError::internal(e),
        },
        BrowserFailure::Command(error) => match error.code.as_str() {
            "not_found" => ApiError::not_found(),
            "conflict" => ApiError::new(ErrorCode::ConflictExists, "that profile exists"),
            "in_use" => ApiError::new(
                ErrorCode::BrowserProfileInUse,
                "a browser uses that profile",
            ),
            "invalid" => ApiError::invalid("browser", &error.detail),
            "unsupported" => ApiError::new(ErrorCode::BrowserNotAllowed, error.detail)
                .with("reason", "unsupported"),
            _ => ApiError::new(ErrorCode::BrowserStartFailed, "the browser did not come up")
                .with("detail", error.detail),
        },
        BrowserFailure::Unreachable(detail) => {
            ApiError::new(ErrorCode::SourceOffline, "the environment did not answer")
                .with("detail", detail)
        }
        BrowserFailure::Engine(error) => ApiError::internal(error),
    }
}

#[path = "browsers/profiles.rs"]
pub mod profiles;
