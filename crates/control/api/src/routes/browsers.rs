//! Browser requests and the whole current list shared with the unretained browser feed.

use crate::middleware::authentication::Caller;
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, Guard};
use crate::registry::{Answer, Ctx, Registry};
use crate::state::App;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::browsers::*;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{EnvironmentRef, EnvironmentSupport, ThroughThreadQuery};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::queries::browsers::{self as rows, BrowserRow};
use orch_engine::browsers::{BrowserFailure, Browsers, CLOSED_LIST_KEPT, OpenBrowser, Requester};
use orch_link_contract::browser::BrowserProfileAction;
use orch_types::{Id, Json, Time};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::Arc;

/// A place may replace the shipped limit of four owned browsers.
const DEFAULT_MAX: i64 = 4;

/// Installs the eight declared browser and profile requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(EnvironmentBrowserList, list)
        .on(EnvironmentBrowserCreate, open)
        .on(BrowserClose, close)
        .on(BrowserTicketCreate, ticket)
        .on(BrowserProfileCreate, create_profile)
        .on(BrowserProfileUpdate, rename_profile)
        .on(BrowserProfileDelete, delete_profile)
        .on_socket(BrowserAttach, crate::realtime::browsers::attach);
}

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
                crate::routes::nodes::view::refs(conn, principal, &[id], Policy::default())
                    .await?
                    .get(&id)
                    .filter(|r| !r.redacted)
                    .cloned()
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

async fn browser(ctx: &Ctx, row: &BrowserRow) -> ApiResult<Browser> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    current(&ctx.app, &mut conn, Some(&principal), row.environment_id)
        .await?
        .browsers
        .into_iter()
        .find(|b| b.id == row.id)
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
) -> ApiResult<Answer<Browser>> {
    let environment = ctx.id("id")?;
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let ctx = action_context(ctx, &guard);
    let principal = guard.principal();
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
            requester: Requester::Person(authorization::caller_of(ctx.user()?)?),
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
    Ok(Answer::from(browser(&ctx, &opened.row).await?).status(status))
}

async fn close(ctx: Ctx, (): (), (): ()) -> ApiResult<Browser> {
    let id = ctx.id("browser_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let ctx = action_context(ctx, &guard);
    let requester = Requester::Person(authorization::caller_of(ctx.user()?)?);
    let result = ctx
        .app
        .engine
        .browsers()
        .close(id, "quit", requester, &crate::sign_in::origin(&ctx))
        .await;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    match result {
        Ok(Some(row)) => browser(&ctx, &row).await,
        Ok(None) => {
            let mut conn = ctx.app.db.acquire().await?;
            let row = rows::get(&mut conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            drop(conn);
            browser(&ctx, &row).await
        }
        Err(error) => Err(failure(&ctx.app, error).await),
    }
}

async fn ticket(
    ctx: Ctx,
    (): (),
    body: BrowserTicketCreateBody,
) -> ApiResult<BrowserTicketCreateReply> {
    let id = ctx.id("browser_id")?;
    let control = body.control.unwrap_or(false);
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let ctx = action_context(ctx, &guard);
    let row = rows::get(&mut conn, id)
        .await?
        .filter(|r| r.state == "open")
        .ok_or_else(ApiError::not_found)?;
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
    let result = ctx.app.realtime.browsers().ticket(&ctx, id, control)?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(result)
}

async fn change_profile(
    ctx: &Ctx,
    action: BrowserProfileAction,
    name: String,
    to: Option<String>,
) -> ApiResult<Vec<BrowserProfile>> {
    let environment = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(ctx).await?;
    let ctx = action_context(ctx.clone(), &guard);
    let result = ctx
        .app
        .engine
        .browsers()
        .profile(
            environment,
            Requester::Person(authorization::caller_of(ctx.user()?)?),
            action,
            name.clone(),
            to,
            &crate::sign_in::origin(&ctx),
        )
        .await;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    match result {
        Ok(result) => Ok(result
            .profiles
            .into_iter()
            .map(|p| BrowserProfile {
                name: p.name,
                size_bytes: p.size_bytes,
                used_at: p.used_at,
                browser: p.browser,
            })
            .collect()),
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
) -> ApiResult<BrowserProfile> {
    change_profile(&ctx, BrowserProfileAction::Create, body.name.clone(), None)
        .await?
        .into_iter()
        .find(|p| p.name == body.name)
        .ok_or_else(ApiError::not_found)
}

async fn rename_profile(
    ctx: Ctx,
    (): (),
    body: BrowserProfileUpdateBody,
) -> ApiResult<BrowserProfile> {
    change_profile(
        &ctx,
        BrowserProfileAction::Rename,
        ctx.param("name")?.to_owned(),
        Some(body.name.clone()),
    )
    .await?
    .into_iter()
    .find(|p| p.name == body.name)
    .ok_or_else(ApiError::not_found)
}

async fn delete_profile(ctx: Ctx, query: BrowserProfileDeleteQuery, (): ()) -> ApiResult<()> {
    let name = ctx.param("name")?.to_owned();
    if query.confirm != name {
        return Err(ApiError::invalid("confirm", "must be the profile's name"));
    }
    change_profile(&ctx, BrowserProfileAction::Delete, name, None).await?;
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
