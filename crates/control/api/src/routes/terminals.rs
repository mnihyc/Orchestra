//! Shells that are their opener's alone: sessions, the ticket that takes one to another screen, the choice of which to close at a person's limit, and closing by whoever manages the environment.
//!
//! A shell is opened in an environment by somebody who holds `terminal.manage` on the environment, which
//! whoever manages it holds without being named, when no gate that no permission lifts is shut and the
//! person is below their limit; a thread the request names says only beside which one the shell is opened
//! and gives nothing. At the limit the request names the shells of theirs they give up, and the
//! answer to one that does not is the list of their own to choose from. A shell is its opener's alone: a
//! ticket to attach to it is given to its opener and to nobody else, whatever anybody else holds, and
//! whoever manages the environment sees that it exists and may close it. What a closed shell last printed
//! is read by its opener, by whoever manages the environment and by whoever holds `audit.view` on the
//! thread it was opened from; the read of anybody but the opener is a line of the environment's history.
//! The agent's own terminal is watched with a ticket for `terminal.view` on the thread's environment, and
//! typed into with one for `terminal.manage` there, where the agent declares one. The sockets themselves
//! are in the realtime module.

use super::workspace::door;
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::realtime::terminals::{Door, Told, agent_socket, shell_socket};
use crate::registry::{Ctx, Registry};
use crate::sign_in::origin;
use axum::body::Body;
use axum::response::Response;
use chrono::{DateTime, Utc};
use http::{HeaderValue, StatusCode, header};
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_agent_contract::descriptor::AgentTerminal;
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::environment::ThroughThreadQuery;
use orch_api_contract::terminals::{
    self as api, AgentTerminalTicketCreateBody, EnvironmentTerminalCreateBody,
    EnvironmentTerminalCreateReply, EnvironmentTerminalListQuery, TerminalListState,
    TerminalNotAllowedReason, TerminalSession, TerminalState, TerminalTicket,
};
use orch_api_contract::users::UserRef;
use orch_authz::environments::{Shell, may_close_shell, may_open_shell, may_read_recording};
use orch_authz::{Policy, Principal, resources};
use orch_db::Conn;
use orch_db::queries::terminals::{self as rows, After, SessionRow, Which};
use orch_db::queries::threads;
use orch_engine::shells::{Ending, Holding, Keeping, Limit, OpenError, OpenRequest, Shells};
use orch_link_contract::commands::ErrorCode as MachineCode;
use orch_realtime_contract::terminal::TerminalClosedReason;
use orch_types::{Id, Time};
use std::collections::HashMap;

/// The largest screen, in either direction, that a shell is opened with: what the machine takes.
const SIZE_MAX: u16 = 1_000;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::EnvironmentTerminalList, list)
        .on(api::EnvironmentTerminalCreate, create)
        .on(api::TerminalTicketCreate, ticket)
        .on(api::TerminalClose, close)
        .on(api::AgentTerminalTicketCreate, agent_ticket)
        .on_raw(api::TerminalRecordingGet, recording)
        .on_socket(api::TerminalAttach, shell_socket)
        .on_socket(api::AgentTerminalAttach, agent_socket);
}

// ---------------------------------------------------------------- the view

/// A shell as the API shows it, from the row and what its place says of how it is kept.
fn view(row: &SessionRow, opener: UserRef, keeping: Keeping, busy: bool) -> TerminalSession {
    let state = match row.state.as_str() {
        rows::state::OPENING => TerminalState::Opening,
        rows::state::OPEN => TerminalState::Open,
        _ => TerminalState::Closed,
    };
    let left_alone = state == TerminalState::Open && !row.attached;
    let closes_at = row.detached_at.filter(|_| left_alone).and_then(|since| {
        chrono::Duration::try_minutes(keeping.detached_minutes).map(|kept| Time::new(since + kept))
    });
    TerminalSession {
        id: row.id,
        thread_id: row.thread_id,
        environment_id: row.environment_id,
        opened_by: opener,
        shell: row.shell.clone(),
        cwd: row.cwd.clone(),
        cols: u16::try_from(row.cols).unwrap_or(1),
        rows: u16::try_from(row.rows).unwrap_or(1),
        state,
        close_reason: row.close_reason.clone(),
        exit_code: row.exit_code.map(i64::from),
        attached: row.attached,
        detached_at: row.detached_at.map(Time::new),
        closes_at,
        busy: state == TerminalState::Open && busy,
        recorded: if state == TerminalState::Closed {
            row.recording_blob.is_some()
        } else {
            keeping.record
        },
        opened_at: Time::new(row.opened_at),
        last_io_at: Time::new(row.last_io_at),
        closed_at: row.closed_at.map(Time::new),
    }
}

/// The shells `found` as the API shows them, in the same order.
async fn views(
    conn: &mut Conn,
    shells: &Shells,
    found: &[SessionRow],
) -> ApiResult<Vec<TerminalSession>> {
    views_observed(conn, shells, found, None).await
}

async fn views_observed(
    conn: &mut Conn,
    shells: &Shells,
    found: &[SessionRow],
    observed: Option<&HashMap<Id, bool>>,
) -> ApiResult<Vec<TerminalSession>> {
    let busy = match observed {
        Some(observed) => observed.clone(),
        None => shells
            .busy(conn, found)
            .await
            .map_err(|error| ApiError::new(ErrorCode::EnvironmentUnavailable, error.to_string()))?,
    };
    let people: Vec<Id> = found.iter().map(|row| row.opened_by).collect();
    let people = refusals::user_refs(conn, &people).await?;
    let mut kept: HashMap<(Option<Id>, Id), Keeping> = HashMap::new();
    let mut shown = Vec::with_capacity(found.len());
    for row in found {
        let opener = people
            .get(&row.opened_by)
            .cloned()
            .ok_or_else(|| ApiError::internal("the opener of a shell is not there"))?;
        let key = (row.thread_id, row.environment_id);
        let keeping = match kept.get(&key) {
            Some(keeping) => *keeping,
            None => {
                let keeping = shells
                    .keeping(conn, row)
                    .await
                    .map_err(ApiError::internal)?;
                kept.insert(key, keeping);
                keeping
            }
        };
        shown.push(view(
            row,
            opener,
            keeping,
            row.state == rows::state::OPEN && busy.get(&row.id).copied().unwrap_or(false),
        ));
    }
    Ok(shown)
}

/// The refusal that no permission lifts, as a problem.
fn not_allowed(reason: TerminalNotAllowedReason) -> ApiError {
    ApiError::new(ErrorCode::TerminalNotAllowed, "a shell is not allowed here")
        .with("reason", reason)
}

// ---------------------------------------------------------------- who may

/// How the caller stands to the shells of an environment.
enum Standing {
    /// They manage the environment: everybody's shells are shown to them, and any may be closed.
    Manager,
    /// They hold `terminal.manage` there, and no more: their own shells.
    Own,
}

/// Whether the caller holds `permission` on the environment, quietly.
async fn holds_on_environment(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    permission: Permission,
) -> ApiResult<bool> {
    let object = ObjectRef {
        kind: ObjectKind::Environment,
        id: environment,
    };
    match resources::check(conn, principal, object, permission, Policy::default()).await {
        Ok(decision) => Ok(decision.is_allowed()),
        Err(orch_authz::Error::NotFound(_)) => Err(ApiError::not_found()),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

/// What the caller may see of the shells of `environment`: all of them if they manage it, their own
/// otherwise. The request's declaration already holds them to `terminal.manage` there.
async fn standing(conn: &mut Conn, principal: &Principal, environment: Id) -> ApiResult<Standing> {
    if holds_on_environment(conn, principal, environment, Permission::EnvironmentManage).await? {
        Ok(Standing::Manager)
    } else {
        Ok(Standing::Own)
    }
}

/// The cursor of a list: when the last shell of the page was opened, and which it was.
fn cursor_text(row: &SessionRow) -> String {
    format!("{}|{}", row.opened_at.timestamp_micros(), row.id)
}

fn cursor_of(text: Option<&str>) -> ApiResult<Option<After>> {
    let Some(text) = text else { return Ok(None) };
    let bad = || ApiError::invalid("cursor", "is not a cursor of this list");
    let (micros, id) = text.split_once('|').ok_or_else(bad)?;
    Ok(Some(After {
        at: micros
            .parse::<i64>()
            .ok()
            .and_then(DateTime::<Utc>::from_timestamp_micros)
            .ok_or_else(bad)?,
        id: id.parse().map_err(|_| bad())?,
    }))
}

// ---------------------------------------------------------------- the requests

/// The caller's own shells in an environment, whichever thread each was opened beside, attached or not,
/// newest first; whoever manages the environment sees everybody's.
async fn list(
    ctx: Ctx,
    query: EnvironmentTerminalListQuery,
    (): (),
) -> ApiResult<Page<TerminalSession>> {
    let environment = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    list_for(&ctx, &mut conn, &principal, environment, query, None).await
}

/// One terminal-list projection for HTTP and live delivery. Live foreground observations are
/// supplied by the native report, so a change never calls the machine once per recipient.
pub(crate) async fn list_for(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    query: EnvironmentTerminalListQuery,
    busy: Option<&HashMap<Id, bool>>,
) -> ApiResult<Page<TerminalSession>> {
    if rows::site(conn, environment).await?.is_none() {
        return Err(ApiError::not_found());
    }
    let opener = match standing(conn, principal, environment).await? {
        Standing::Manager => None,
        Standing::Own => Some(principal.user()),
    };
    let which = match query.state {
        None => Which::Any,
        Some(TerminalListState::Open) => Which::Live,
        Some(TerminalListState::Closed) => Which::Ended,
    };
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = i64::from(page.limit());
    let after = cursor_of(query.cursor.as_deref())?;
    let mut found = rows::list(conn, environment, opener, which, after, limit + 1).await?;
    let more = i64::try_from(found.len()).unwrap_or(i64::MAX) > limit;
    if more {
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    let next_cursor = more.then(|| found.last().map(cursor_text)).flatten();
    let items = views_observed(conn, &ctx.app.engine.shells(), &found, busy).await?;
    Ok(Page { items, next_cursor })
}

/// Why the machine did not start a shell, as the person is told.
fn refused_by_machine(error: &orch_link_contract::commands::CommandError) -> ApiError {
    match error.code.as_str() {
        code if code == MachineCode::Unsupported.as_str() => {
            not_allowed(TerminalNotAllowedReason::MachinePolicy)
        }
        code if code == MachineCode::NotFound.as_str() => {
            ApiError::invalid("cwd", "is not a directory of the workspace")
        }
        _ => ApiError::new(
            ErrorCode::EnvironmentUnavailable,
            "the environment's machine would not open a shell",
        ),
    }
}

/// An open that did not come to a shell, as the person is told.
async fn open_failed(conn: &mut Conn, shells: &Shells, error: OpenError) -> ApiResult<ApiError> {
    Ok(match error {
        OpenError::NoEnvironment | OpenError::WrongThread => ApiError::not_found(),
        OpenError::NotAllowed(reason) => not_allowed(reason),
        OpenError::BadDirectory(why) => ApiError::invalid("cwd", &why),
        OpenError::NotYours(_) => {
            ApiError::invalid("close", "names a shell that is not yours here")
        }
        OpenError::AtLimit(at) => {
            let mine = views(conn, shells, &at.mine).await?;
            ApiError::new(
                ErrorCode::ConflictExists,
                "this many shells are open already; close one to open another",
            )
            .with("mine", mine)
            .with(
                "limit",
                match at.limit {
                    Limit::Person => "person",
                    Limit::Environment => "environment",
                },
            )
            .with("open", at.open)
            .with("people", at.people)
        }
        OpenError::Unreachable(why) => ApiError::new(
            ErrorCode::SourceOffline,
            format!("the machine could not be reached: {why}"),
        ),
        OpenError::Refused(error) => refused_by_machine(&error),
        OpenError::Engine(error) => ApiError::internal(error),
    })
}

/// Opens a shell in an environment, closing some of the caller's own first when they are at the limit,
/// and gives the ticket to attach to it.
async fn create(
    ctx: Ctx,
    query: ThroughThreadQuery,
    body: EnvironmentTerminalCreateBody,
) -> ApiResult<EnvironmentTerminalCreateReply> {
    let environment = ctx.id("id")?;
    let person = ctx.user()?.id();
    if !(1..=SIZE_MAX).contains(&body.cols) {
        return Err(ApiError::invalid(
            "cols",
            "is the width of the screen, 1 to 1000",
        ));
    }
    if !(1..=SIZE_MAX).contains(&body.rows) {
        return Err(ApiError::invalid(
            "rows",
            "is the height of the screen, 1 to 1000",
        ));
    }
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    if rows::site(&mut conn, environment).await?.is_none() {
        return Err(ApiError::not_found());
    }
    match may_open_shell(&mut conn, &principal, environment, Policy::default()).await {
        Ok(Decision::Allowed { .. }) => {}
        Ok(Decision::Refused(refusal)) => return Err(refusals::denied(&mut conn, refusal).await?),
        Err(error) => return Err(refusals::problem(&mut conn, error).await),
    }
    // The thread it is opened beside gives nothing: it has to be one that runs there and that the
    // caller may view, and says where the shell starts.
    if let Some(thread) = query.thread {
        door::beside(&mut conn, &principal, environment, thread).await?;
    }
    drop(conn);
    let shells = ctx.app.engine.shells();
    let request = OpenRequest {
        person,
        environment,
        through: query.thread,
        cols: body.cols,
        rows: body.rows,
        directory: body.cwd,
        close: body.close.unwrap_or_default(),
        origin: origin(&ctx),
    };
    // A screen that has a shell the person gives up is told why before it goes.
    let sockets = ctx.app.realtime.terminals();
    let about = |row: &SessionRow, _: Ending| {
        sockets.tell(row.id, Told::Over(TerminalClosedReason::ClosedByUser));
    };
    let opened = match shells.open(request, &about).await {
        Ok(opened) => opened,
        Err(error) => {
            let mut conn = ctx.app.db.acquire().await?;
            return Err(open_failed(&mut conn, &shells, error).await?);
        }
    };
    let mut conn = ctx.app.db.acquire().await?;
    let site = rows::site(&mut conn, opened.session.environment_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let TerminalTicket {
        ticket,
        ticket_expires_at,
    } = ctx.app.realtime.terminals().issue(
        ctx.app.clock.now(),
        ctx.user()?,
        Door::Shell(opened.session.id),
        opened.session.environment_id,
        opened.session.env_generation,
        site.machine_id,
    );
    let session = views(&mut conn, &shells, std::slice::from_ref(&opened.session))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a shell that was opened is not shown"))?;
    Ok(EnvironmentTerminalCreateReply {
        session,
        ticket,
        ticket_expires_at,
        closed: opened.closed,
    })
}

/// What a person who lost what a shell was opened by is told: a shell that is lost is ended at once, as
/// the check of each move of the epoch would end it, and the refusal says what is missing.
async fn lost(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &SessionRow,
    holding: Holding,
) -> ApiResult<ApiError> {
    ctx.app
        .realtime
        .terminals()
        .tell(row.id, Told::Over(TerminalClosedReason::Revoked));
    let _ = ctx
        .app
        .engine
        .shells()
        .finish(row.id, Ending::REVOKED, &origin(ctx))
        .await;
    match holding {
        Holding::Gate(reason) => Ok(not_allowed(reason)),
        Holding::Permission | Holding::Yes => {
            match may_open_shell(conn, principal, row.environment_id, Policy::default()).await {
                Ok(Decision::Refused(refusal)) => Ok(refusals::denied(conn, refusal).await?),
                Ok(Decision::Allowed { .. }) => Ok(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "you no longer hold what this shell was opened by",
                )),
                Err(error) => Ok(refusals::problem(conn, error).await),
            }
        }
    }
}

/// Gives a new ticket to attach to a shell again, from this browser or another. Only its opener has
/// one, and only while they still pass the gates it was opened under.
async fn ticket(ctx: Ctx, (): (), (): ()) -> ApiResult<TerminalTicket> {
    let id = ctx.id("terminal_id")?;
    let person = ctx.user()?.id();
    let mut conn = ctx.app.db.acquire().await?;
    let row = rows::get(&mut conn, id)
        .await?
        .filter(|row| row.opened_by == person && row.state == rows::state::OPEN)
        .ok_or_else(ApiError::not_found)?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let holding = ctx
        .app
        .engine
        .shells()
        .holds(&mut conn, &row)
        .await
        .map_err(ApiError::internal)?;
    if holding != Holding::Yes {
        return Err(lost(&ctx, &mut conn, &principal, &row, holding).await?);
    }
    authorization::require_resource(
        &mut conn,
        &principal,
        ResourceKind::Environment,
        row.environment_id,
        Permission::TerminalManage,
        Policy::default(),
    )
    .await?;
    let site = rows::site(&mut conn, row.environment_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(ctx.app.realtime.terminals().issue(
        ctx.app.clock.now(),
        ctx.user()?,
        Door::Shell(id),
        row.environment_id,
        row.env_generation,
        site.machine_id,
    ))
}

/// Closes a shell: its opener may, and so may whoever manages its environment. Closing one that is
/// closed answers with it as it is.
async fn close(ctx: Ctx, (): (), (): ()) -> ApiResult<TerminalSession> {
    let id = ctx.id("terminal_id")?;
    let person = ctx.user()?.id();
    let mut conn = ctx.app.db.acquire().await?;
    let row = rows::get(&mut conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.opened_by != person {
        let principal = authorization::principal(&ctx, &mut conn).await?;
        let shell = Shell {
            environment: row.environment_id,
            thread: row.thread_id,
            opened_by: row.opened_by,
        };
        let may = may_close_shell(&mut conn, &principal, &shell, Policy::default())
            .await
            .map_err(ApiError::internal)?;
        if !may {
            return Err(ApiError::not_found());
        }
    }
    drop(conn);
    let shells = ctx.app.engine.shells();
    if row.is_live() {
        // The screen is told why before the shell goes, so that it says what was done and not what it
        // sees first of the shell going away.
        ctx.app
            .realtime
            .terminals()
            .tell(id, Told::Over(TerminalClosedReason::ClosedByUser));
        shells
            .finish(id, Ending::CLOSED, &origin(&ctx))
            .await
            .map_err(ApiError::internal)?;
    }
    let mut conn = ctx.app.db.acquire().await?;
    let row = rows::get(&mut conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    views(&mut conn, &shells, std::slice::from_ref(&row))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a shell that was closed is not shown"))
}

/// What a shell last printed, as it was kept when it closed: for its opener, for whoever manages its
/// environment, and for whoever holds `audit.view` on the thread it was opened from.
async fn recording(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    let id = ctx.id("terminal_id")?;
    let person = ctx.user()?.id();
    let mut conn = ctx.app.db.acquire().await?;
    let row = rows::get(&mut conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let shell = Shell {
        environment: row.environment_id,
        thread: row.thread_id,
        opened_by: row.opened_by,
    };
    let reading = may_read_recording(&mut conn, &principal, &shell, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    if !reading.allowed || row.is_live() || row.recording().is_none() {
        return Err(ApiError::not_found());
    }
    drop(conn);
    let shells = ctx.app.engine.shells();
    let content = shells
        .recording(&row)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    if reading.audited {
        shells
            .note_read(&row, person, &origin(&ctx))
            .await
            .map_err(ApiError::internal)?;
    }
    let mut response = Response::new(Body::from(content));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// A ticket for the agent's own terminal of a thread: to watch it with `terminal.view` on the thread's
/// environment, and to type into it with `terminal.manage` there, where the agent declares a terminal
/// that can be.
async fn agent_ticket(
    ctx: Ctx,
    (): (),
    body: AgentTerminalTicketCreateBody,
) -> ApiResult<TerminalTicket> {
    let thread = ctx.id("id")?;
    let control = body.control.unwrap_or(false);
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let row = threads::thread(&mut conn, thread)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let environment = row.environment_id.ok_or_else(ApiError::not_found)?;
    authorization::require_resource(
        &mut conn,
        &principal,
        ResourceKind::Environment,
        environment,
        Permission::TerminalView,
        Policy::default(),
    )
    .await?;
    if control {
        authorization::require_resource(
            &mut conn,
            &principal,
            ResourceKind::Environment,
            environment,
            Permission::TerminalManage,
            Policy::default(),
        )
        .await?;
    }
    let screen = ctx
        .app
        .engine
        .shells()
        .agent_screen(&mut conn, thread)
        .await
        .map_err(ApiError::internal)?;
    let declared = screen.as_ref().map(|screen| screen.declared);
    let fits = match declared {
        Some(AgentTerminal::Interactive) => true,
        Some(AgentTerminal::View) => !control,
        Some(AgentTerminal::None) | None => false,
    };
    if !fits {
        return Err(ApiError::new(
            ErrorCode::CapabilityUnsupported,
            if control {
                "the agent declares no terminal that can be typed into"
            } else {
                "the agent declares no terminal of its own"
            },
        ));
    }
    let site = rows::site(&mut conn, environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(ctx.app.realtime.terminals().issue(
        ctx.app.clock.now(),
        ctx.user()?,
        Door::Agent {
            thread,
            control,
            segment: row.current_segment_id,
        },
        environment,
        site.generation,
        screen.and_then(|screen| screen.machine),
    ))
}
