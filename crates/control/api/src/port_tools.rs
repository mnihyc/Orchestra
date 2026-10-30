//! The four port tools over the thread owner's current environment and authority.

use crate::middleware::authentication::{Authed, Caller, Via};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::{port_requests, ports};
use crate::state::{App, AppState};
use async_trait::async_trait;
use orch_agent_contract::tool_answers;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::ports::*;
use orch_api_contract::routes::Endpoint;
use orch_authz::{Principal, TokenGrant};
use orch_db::Tx;
use orch_db::queries::threads::ThreadRow;
use orch_db::queries::{identity, ports as rows, threads};
use orch_engine::ports as core;
use orch_engine::tools::definitions::{Code, Refusal};
use orch_engine::tools::ports::PortTools;
use orch_types::{Id, Json};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// A call waits one minute, leaving the adapters' seventy/eighty-second margins for its answer.
pub const DECISION_WAIT: Duration = Duration::from_secs(60);
/// A pending card is checked four times a second without a database lease between checks.
const DECISION_LOOK: Duration = Duration::from_millis(250);
/// One reconciliation pass handles at most this many outcomes, as the ordinary bounded page does.
const OUTCOME_BATCH: i64 = 100;

/// Attaches the API producer without retaining the application through its engine.
pub fn install(app: &App) {
    app.engine.attach_port_tools(Arc::new(Tools {
        app: Arc::downgrade(app),
    }));
}

/// The API implementation of the engine's port-tool hook.
pub struct Tools {
    app: Weak<AppState>,
}

fn refusal(error: ApiError) -> Refusal {
    let code = match error.code() {
        ErrorCode::NotFound => Code::NotFound,
        ErrorCode::PortsTooMany => Code::Limit,
        ErrorCode::RequestTooLarge => Code::TooLarge,
        ErrorCode::PortsNotAllowed | ErrorCode::AuthSuspended => Code::Disabled,
        ErrorCode::PortsUnavailable => Code::NotReachable,
        ErrorCode::ConflictStaleRevision | ErrorCode::PortsProgramConflict => Code::Changed,
        ErrorCode::Internal => Code::Unsupported,
        _ => Code::Invalid,
    };
    Refusal::new(
        code,
        format!(
            "{}: {}. list_ports shows the current state; a person who manages Ports can change the policy.",
            error.code().as_str(),
            error.problem.detail
        ),
    )
}

fn encode(value: impl serde::Serialize) -> ApiResult<Json> {
    serde_json::to_value(value).map_err(ApiError::internal)
}

fn arguments_only(arguments: &Json) -> ApiResult<Json> {
    let mut value = arguments.clone();
    let object = value
        .as_object_mut()
        .ok_or_else(|| ApiError::invalid("arguments", "give an object"))?;
    if let Some(bytes) = object.remove("max_bytes")
        && !bytes.as_i64().is_some_and(|bytes| {
            (orch_engine::tools::definitions::READ_BYTES_MIN
                ..=orch_engine::tools::definitions::READ_BYTES_MAX)
                .contains(&bytes)
        })
    {
        return Err(ApiError::invalid(
            "max_bytes",
            "use an integer from 1000 to 40000",
        ));
    }
    if let Some(ids) = object.remove("ids")
        && !ids.is_boolean()
    {
        return Err(ApiError::invalid("ids", "use true or false"));
    }
    Ok(value)
}

fn check_answer(tool: &str, arguments: &Json, value: impl serde::Serialize) -> ApiResult<()> {
    let value = encode(value)?;
    if tool_answers::fits(tool, arguments, &value) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::RequestTooLarge,
            "The complete port answer does not fit max_bytes. Narrow the port/filter or request more max_bytes (at most 40000); no new opening, card, close or link delivery was committed.",
        ))
    }
}

fn link_delivery(
    ctx: &Ctx,
    row: &rows::Opening,
    view: &PortOpening,
) -> ApiResult<Option<PortLinkDelivery>> {
    if row.state == "waiting" || row.link_recipient.as_deref() != Some("thread") {
        return Ok(None);
    }
    let Some(sealed) = &row.secret_sealed else {
        return Ok(None);
    };
    let proof = crate::routes::resources::sealing::master_key()?
        .open(sealed, row.id.as_bytes())
        .map_err(ApiError::internal)?;
    let proof = String::from_utf8(proof).map_err(ApiError::internal)?;
    let helper = format!(
        "{}/api/v1/ports/links/{}#{proof}",
        ctx.app.config.system.public_url.trim_end_matches('/'),
        row.id
    );
    let browser = if view.entry_methods.contains(&PortEntryMethod::Browser) {
        ports::address(ctx, row.environment_id, row.port as u16)
            .map(|address| format!("{address}/?orch_token={proof}"))
    } else {
        None
    };
    Ok(Some(PortLinkDelivery {
        recipient: PortLinkRecipient::Thread,
        url_once: Some(helper.clone()),
        browser_url_once: browser,
        helper_link_once: view
            .entry_methods
            .contains(&PortEntryMethod::Helper)
            .then_some(helper),
        delivered_to: "this thread, once; readable to its viewers".into(),
    }))
}

/// Checks the actual current opening/card and prospective once-only link before its creation commits.
pub(crate) fn check_open_budget(
    ctx: &Ctx,
    row: &rows::Opening,
    reply: &PortOpenReply,
    arguments: &Json,
) -> ApiResult<()> {
    let mut shown = reply.clone();
    shown.link = link_delivery(ctx, row, &shown.opening)?;
    check_answer("open_port", arguments, &shown)
}

/// Pins the thread and its owner's ordinary authority before any environment or opening lock.
async fn begin(app: &App, id: Id) -> ApiResult<(Tx, ThreadRow, Principal, Ctx)> {
    let mut tx = app.db.begin().await?;
    orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
    let thread = threads::lock(&mut tx, id)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    orch_engine::tools::source::check(&mut tx)
        .await
        .map_err(|error| match error {
            orch_engine::error::EngineError::StaleToolSource => {
                ApiError::new(ErrorCode::ConflictStaleRevision, error.to_string())
            }
            error => ApiError::internal(error),
        })?;
    let user = identity::user_by_id(&mut tx, thread.owner_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if user.suspended() {
        return Err(ApiError::new(
            ErrorCode::AuthSuspended,
            "the thread's owner is suspended",
        ));
    }
    // Agent work carries ordinary holdings, never a human's system-admin override. This context
    // is for read projections only; no synthetic credential passes an HTTP write guard.
    let principal = Principal::load(
        &mut tx,
        orch_authz::Caller::token(thread.owner_id, TokenGrant::everything()),
        app.clock.time(),
    )
    .await
    .map_err(ApiError::internal)?;
    let caller = Caller::User(Arc::new(Authed {
        user,
        as_admin: false,
        via: Via::Token {
            token_id: Id::nil(),
            scopes: vec!["*".to_owned()],
            node_id: None,
        },
    }));
    let ctx = Ctx::for_tool(
        app.clone(),
        caller,
        EnvironmentPortList::ROUTE,
        thread.environment_id.unwrap_or(id),
    );
    Ok((tx, thread, principal, ctx))
}

fn environment_id(thread: &ThreadRow) -> ApiResult<Id> {
    thread.environment_id.ok_or_else(|| {
        ApiError::new(
            ErrorCode::PortsUnavailable,
            "this thread has no environment; a person can give it one",
        )
    })
}

/// Reads an outcome under the same guard and consumes only a link intended for this thread.
async fn outcome(
    app: &App,
    thread: Id,
    opening: Id,
    consume: bool,
    arguments: &Json,
) -> ApiResult<PortOpenReply> {
    let (mut tx, _, principal, ctx) = begin(app, thread).await?;
    let row = rows::opening(&mut tx, opening)
        .await?
        .filter(|row| row.requested_by_thread == Some(thread))
        .ok_or_else(ApiError::not_found)?;
    let env = ports::environment(&mut tx, row.environment_id).await?;
    let view = ports::opening_view(&ctx, &mut tx, &principal, &row, &env, false).await?;
    let request = match row.approval_id {
        Some(id) => Some(port_requests::request_ref(&mut tx, &principal, id, &env).await?),
        None => None,
    };
    let link = if consume {
        link_delivery(&ctx, &row, &view)?
    } else {
        None
    };
    let mut reply = PortOpenReply {
        opening: view,
        request,
        link,
    };
    let fits = tool_answers::fits("open_port", arguments, &encode(&reply)?);
    if consume && fits {
        if reply.link.is_some() {
            rows::delivered(&mut tx, row.id).await?;
        }
        if row.state != "waiting" {
            let _ = rows::outcome_told(&mut tx, row.id).await?;
        }
    } else if consume && reply.link.is_some() {
        // A human can change the final view while the call waits. Keep the actual opening
        // visible and the secret unconsumed; the ordinary late thread item will deliver it.
        reply.link=Some(PortLinkDelivery {recipient:PortLinkRecipient::Thread,url_once:None,browser_url_once:None,helper_link_once:None,
            delivered_to:"The opening exists; its once-only link is held for the thread's result item because this answer's byte bound is too small.".into()});
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(reply)
}

impl Tools {
    fn app(&self) -> ApiResult<App> {
        self.app
            .upgrade()
            .ok_or_else(|| ApiError::new(ErrorCode::PortsUnavailable, "the system is stopping"))
    }

    async fn list(&self, thread: Id, arguments: &Json) -> ApiResult<Json> {
        let app = self.app()?;
        let (tx, current, _, ctx) = begin(&app, thread).await?;
        let id = environment_id(&current)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let mut query: EnvironmentPortListQuery =
            serde_json::from_value(arguments_only(arguments)?)
                .map_err(|error| ApiError::invalid("arguments", &error.to_string()))?;
        query.look = Some(true);
        let result = loop {
            let result = ports::list(ctx.clone(), query.clone(), ()).await?;
            if check_answer("list_ports", arguments, &result).is_ok() {
                break result;
            }
            let maximum = result.targets.len();
            if maximum <= 1 {
                return check_answer("list_ports", arguments, &result)
                    .and_then(|()| encode(result));
            }
            query.limit = Some(u32::try_from((maximum / 2).max(1)).unwrap_or(1));
            query.look = Some(false);
        };
        let mut conn = app.db.acquire().await?;
        if threads::thread(&mut conn, thread)
            .await?
            .is_none_or(|current| {
                current.environment_id != Some(id) || current.deleted_at.is_some()
            })
        {
            return Err(ApiError::not_found());
        }
        encode(result)
    }

    async fn open(&self, thread: Id, arguments: &Json) -> ApiResult<Json> {
        let app = self.app()?;
        let body: PortOpeningRequest = serde_json::from_value(arguments_only(arguments)?)
            .map_err(|error| ApiError::invalid("arguments", &error.to_string()))?;
        let (tx, current, principal, ctx) = begin(&app, thread).await?;
        environment_id(&current)?;
        let made = port_requests::request_for_thread_bounded(
            &ctx,
            tx,
            &principal,
            thread,
            body,
            Some(arguments),
        )
        .await?;
        let id = made.opening.id;
        if made.opening.state != PortOpeningState::Waiting {
            return encode(outcome(&app, thread, id, true, arguments).await?);
        }
        let decided = tokio::time::timeout(DECISION_WAIT, async {
            loop {
                let reply = outcome(&app, thread, id, false, arguments).await?;
                if reply.opening.state != PortOpeningState::Waiting {
                    return Ok::<_, ApiError>(reply);
                }
                tokio::time::sleep(DECISION_LOOK).await;
            }
        })
        .await;
        match decided {
            Ok(Ok(_)) => encode(outcome(&app, thread, id, true, arguments).await?),
            Ok(Err(error)) => Err(error),
            Err(_) => encode(outcome(&app, thread, id, true, arguments).await?),
        }
    }

    async fn close(&self, thread: Id, arguments: &Json) -> ApiResult<Json> {
        let _ = arguments_only(arguments)?;
        let id = arguments
            .get("opening")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
            .ok_or_else(|| {
                ApiError::invalid("opening", "give the opening reference list_ports returned")
            })?;
        let app = self.app()?;
        let (mut tx, _, principal, ctx) = begin(&app, thread).await?;
        let row = rows::opening(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if row.requested_by_thread != Some(thread) {
            return Err(ApiError::new(
                ErrorCode::PortsNotAllowed,
                "this thread did not request this opening; only its requester or a Ports manager can close it",
            ));
        }
        let closed = core::close_requested(&mut tx, thread, id, app.clock.time())
            .await
            .map_err(ApiError::internal)?;
        let env = ports::environment(&mut tx, row.environment_id).await?;
        let view = ports::opening_view(&ctx, &mut tx, &principal, &closed, &env, false).await?;
        if row.state != "closed" {
            check_answer("close_port", arguments, &view)?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        app.cache.ports().close_opening(id);
        app.link.terminals().close_port_opening(id);
        crate::host_ports::unpublish_in_background(&app, &[id]);
        encode(view)
    }

    async fn connections(&self, thread: Id, arguments: &Json) -> ApiResult<Json> {
        let app = self.app()?;
        let (tx, current, _, ctx) = begin(&app, thread).await?;
        environment_id(&current)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let mut query = arguments_only(arguments)?;
        if let Some(object) = query.as_object_mut()
            && let Some(opening) = object.remove("opening")
        {
            object.insert("opening_id".to_owned(), opening);
        }
        let mut query: EnvironmentPortConnectionListQuery = serde_json::from_value(query)
            .map_err(|error| ApiError::invalid("arguments", &error.to_string()))?;
        loop {
            let result = ports::connections(ctx.clone(), query.clone(), ()).await?;
            if check_answer("list_port_connections", arguments, &result).is_ok() {
                return encode(result);
            }
            let maximum = result.connections.as_ref().map_or(0, Vec::len);
            if maximum <= 1 {
                return check_answer("list_port_connections", arguments, &result)
                    .and_then(|()| encode(result));
            }
            query.limit = Some(u32::try_from((maximum / 2).max(1)).unwrap_or(1));
        }
    }
}

/// Reconciles decided requests after their one-minute call window, once even across API processes and restarts.
pub async fn tell_late(app: &App) -> ApiResult<()> {
    let since = app.clock.now() - chrono::Duration::seconds(DECISION_WAIT.as_secs() as i64);
    let pending: Vec<(Id, Id)> = {
        let mut conn = app.db.acquire().await?;
        sqlx::query_as("SELECT DISTINCT o.id,o.requested_by_thread FROM port_openings o JOIN port_open_requests r ON r.opening_id=o.id JOIN nodes n ON n.id=o.requested_by_thread WHERE o.state<>'waiting' AND NOT r.outcome_delivered AND r.created_at<=$1 AND n.deleted_at IS NULL ORDER BY o.id LIMIT $2")
            .bind(since).bind(OUTCOME_BATCH).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
    };
    for (opening, thread_id) in pending {
        let (mut tx, thread, principal, ctx) = begin(app, thread_id).await?;
        let row = rows::opening(&mut tx, opening)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if row.state == "waiting" || !rows::outcome_told(&mut tx, opening).await? {
            continue;
        }
        let env = ports::environment(&mut tx, row.environment_id).await?;
        let view = ports::opening_view(&ctx, &mut tx, &principal, &row, &env, false).await?;
        let mut location = view
            .entries
            .iter()
            .filter_map(|entry| entry.address.as_deref())
            .collect::<Vec<_>>()
            .join("; ");
        let recipient = if row.link_recipient.as_deref() == Some("person") {
            "The deciding person kept the link; this thread receives none.".to_owned()
        } else if row.link_recipient.as_deref() == Some("thread")
            && row.secret_sealed.is_some()
            && !matches!(row.state.as_str(), "closed" | "refused")
        {
            let proof = crate::routes::resources::sealing::master_key()?
                .open(
                    row.secret_sealed.as_deref().unwrap_or_default(),
                    row.id.as_bytes(),
                )
                .map_err(ApiError::internal)?;
            let proof = String::from_utf8(proof).map_err(ApiError::internal)?;
            let helper = format!(
                "{}/api/v1/ports/links/{}#{proof}",
                app.config.system.public_url.trim_end_matches('/'),
                row.id
            );
            location.push_str(&format!(" Link: {helper}."));
            if view.entry_methods.contains(&PortEntryMethod::Browser)
                && let Some(address) = ports::address(&ctx, env.id, row.port as u16)
            {
                location.push_str(&format!(" Webhook: {address}/?orch_token={proof}."));
            }
            rows::delivered(&mut tx, row.id).await?;
            "The link was given to this thread and is readable here.".to_owned()
        } else {
            String::new()
        };
        let settings = orch_engine::tools::ToolSettings::load(&mut tx, thread_id)
            .await
            .map_err(ApiError::internal)?;
        let text = core::port_opening_decided(
            settings
                .messages
                .get("port_opening_decided")
                .map_or(core::PORT_OPENING_DECIDED, String::as_str),
            &core::PortDecided {
                port: row.port as u16,
                environment: &env.name,
                audience: &row.label,
                state: &row.state,
                r#where: &location,
                reason: row.reason.as_deref().unwrap_or_default(),
                recipient: &recipient,
            },
        );
        let mut unit = orch_engine::unit::Unit::new(&mut tx, app.clock.now());
        app.engine
            .port_notice_in(&mut unit, &thread, row.id, &text)
            .await
            .map_err(ApiError::internal)?;
        let finished = unit.finish().await.map_err(ApiError::internal)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        for thread in finished.wakes {
            app.engine.wake(thread);
        }
    }
    Ok(())
}

#[async_trait]
impl PortTools for Tools {
    async fn list_ports(&self, thread: &ThreadRow, arguments: &Json) -> Result<Json, Refusal> {
        self.list(thread.node_id, arguments).await.map_err(refusal)
    }
    async fn open_port(&self, thread: &ThreadRow, arguments: &Json) -> Result<Json, Refusal> {
        self.open(thread.node_id, arguments).await.map_err(refusal)
    }
    async fn close_port(&self, thread: &ThreadRow, arguments: &Json) -> Result<Json, Refusal> {
        self.close(thread.node_id, arguments)
            .await
            .map_err(|error| {
                if error
                    .problem
                    .detail
                    .starts_with("this thread did not request")
                {
                    Refusal::new(Code::NotYours, error.problem.detail.clone())
                } else {
                    refusal(error)
                }
            })
    }
    async fn list_port_connections(
        &self,
        thread: &ThreadRow,
        arguments: &Json,
    ) -> Result<Json, Refusal> {
        self.connections(thread.node_id, arguments)
            .await
            .map_err(refusal)
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn supplied_marks_and_multibyte_text_count_before_a_change_is_allowed() {
        let value = json!({"label":"界<orch".repeat(180)});
        let raw = value.to_string().len();
        assert!(raw >= 1000);
        let arguments = json!({"max_bytes":raw,"ids":false,"port":3000});
        assert_eq!(arguments_only(&arguments).unwrap(), json!({"port":3000}));
        let error = check_answer("open_port", &arguments, &value).unwrap_err();
        assert_eq!(
            error.code(),
            ErrorCode::RequestTooLarge,
            "escaped delivered bytes, not the raw JSON alone"
        );
        assert!(arguments_only(&json!({"max_bytes":"40000"})).is_err());
    }
}
