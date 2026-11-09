//! Ordinary Orch calls into the shared Ask readers, under a current native owner delegation.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::state::{App, AppState};
use async_trait::async_trait;
use orch_agent_contract::events::OrchTool;
use orch_api_contract::common::ErrorCode;
use orch_authz::engine::Graph;
use orch_db::{
    Conn, DbError,
    queries::{threads, transaction_guard},
};
use orch_engine::{
    EngineError,
    tools::{
        CallContext,
        definitions::{Code, Refusal},
        read_tools::{ReadTools, SHARED_POINTER},
        source,
    },
};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, Weak};

pub(crate) mod sharing;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerRead {
    owner: Id,
    root: Id,
    #[serde(default)]
    completed: bool,
}

/// A native source proof constructed only after this call's current delegation and audience.
/// It is not an HTTP session, token or reusable permission lease.
#[derive(Clone)]
pub(crate) struct OwnerReadPermit {
    thread: Id,
    owner: Id,
    root: Id,
    call: CallContext,
}

impl OwnerReadPermit {
    pub(crate) fn thread(&self) -> Id {
        self.thread
    }
    pub(crate) fn owner(&self) -> Id {
        self.owner
    }
    pub(crate) fn root(&self) -> Id {
        self.root
    }
    pub(crate) fn call(&self) -> &CallContext {
        &self.call
    }

    pub(crate) async fn check(&self, conn: &mut Conn, app: &App) -> ApiResult<()> {
        transaction_guard::lock_action(conn).await?;
        source::check_call(conn, &self.call)
            .await
            .map_err(ApiError::engine)?;
        let thread = current(app, conn, self.thread, self.owner, self.root).await?;
        if !thread {
            return Err(ApiError::new(ErrorCode::AuthzDenied, SHARED_POINTER));
        }
        Ok(())
    }
}

async fn current(app: &App, conn: &mut Conn, id: Id, owner: Id, root: Id) -> ApiResult<bool> {
    let thread = threads::lock(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.archived_at.is_none());
    let Some(thread) = thread else {
        return Ok(false);
    };
    if thread.owner_id != owner
        || !orch_settings::get::<bool>(conn, Place::Node(id), keys::ORCH_TOOLS_OWNER_READS)
            .await
            .map_err(ApiError::internal)?
            .unwrap_or(false)
    {
        return Ok(false);
    }
    let graph = Graph::load(conn, &[id]).await.map_err(ApiError::internal)?;
    if graph.get(id).is_none_or(|node| node.root != root) {
        return Ok(false);
    }
    orch_engine::tools::read_tools::owner_is_only_viewer(conn, &thread, app.clock.time())
        .await
        .map_err(ApiError::engine)
}

fn refusal(error: ApiError) -> Refusal {
    let code = match error.code() {
        ErrorCode::AuthzDenied | ErrorCode::AuthSuspended => Code::NotYours,
        ErrorCode::NotFound => Code::NotFound,
        ErrorCode::ConflictStaleRevision
        | ErrorCode::ConflictIdempotencyMismatch
        | ErrorCode::ConflictConfirmationRequired => Code::Changed,
        ErrorCode::RequestTooLarge => Code::TooLarge,
        _ => Code::Invalid,
    };
    Refusal::new(code, error.problem.detail)
}

pub fn install(app: &App) {
    let tools: Arc<dyn ReadTools> = Arc::new(Tools {
        app: Arc::downgrade(app),
    });
    app.engine.attach_read_tools(Arc::clone(&tools));
    app.link.serve_read_tools(tools);
}

struct Tools {
    app: Weak<AppState>,
}

async fn permit(
    app: &App,
    call: &CallContext,
    expected: &threads::ThreadRow,
) -> ApiResult<OwnerReadPermit> {
    let mut tx = app.db.begin().await?;
    transaction_guard::lock_action(&mut tx).await?;
    source::check_call(&mut tx, call)
        .await
        .map_err(ApiError::engine)?;
    let graph = Graph::load(&mut tx, &[call.thread])
        .await
        .map_err(ApiError::internal)?;
    let root = graph.get(call.thread).ok_or_else(ApiError::not_found)?.root;
    let permit = OwnerReadPermit {
        thread: call.thread,
        owner: expected.owner_id,
        root,
        call: call.clone(),
    };
    permit.check(&mut tx, app).await?;
    let answer = orch_engine::ids::tool_result(call.thread, &call.call_ref);
    let (plan, completed): (Json, bool) = sqlx::query_as("SELECT plan,completed FROM thread_tool_calls WHERE answer_id=$1 AND NOT retired FOR UPDATE")
        .bind(answer).fetch_one(&mut *tx).await.map_err(DbError::from)?;
    let record = OwnerRead {
        owner: permit.owner,
        root: permit.root,
        completed: false,
    };
    if let Some(old) = plan.get("owner_read") {
        let old: OwnerRead = serde_json::from_value(old.clone()).map_err(ApiError::internal)?;
        if old != record || completed {
            return Err(ApiError::new(
                ErrorCode::ConflictIdempotencyMismatch,
                "This logical read already belongs to a completed or different owner context. Start a new tool call.",
            ));
        }
    } else {
        sqlx::query("UPDATE thread_tool_calls SET plan=jsonb_set(plan,'{owner_read}',$2) WHERE answer_id=$1")
            .bind(answer).bind(serde_json::to_value(record).map_err(ApiError::internal)?)
            .execute(&mut *tx).await.map_err(DbError::from)?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(permit)
}

#[async_trait]
impl ReadTools for Tools {
    async fn documentation(&self, arguments: &Json) -> Result<Json, Refusal> {
        crate::ask::tools::documentation(arguments).map_err(refusal)
    }

    async fn owner_read(
        &self,
        call: &CallContext,
        thread: &threads::ThreadRow,
    ) -> Result<Json, Refusal> {
        let app = self.app.upgrade().ok_or_else(|| {
            Refusal::new(
                Code::Unsupported,
                "The server's read tools are unavailable.",
            )
        })?;
        let permit = permit(&app, call, thread).await.map_err(refusal)?;
        let name = match call.tool {
            OrchTool::Open => "open",
            OrchTool::Search => "search",
            OrchTool::FindOperation => "find_operation",
            OrchTool::Read => "read",
            _ => return Err(Refusal::invalid("This is not a person-scoped read tool.")),
        };
        let result = crate::ask::tools::execute_thread(&app, &permit, name, &call.arguments)
            .await
            .map_err(refusal)?;
        let mut tx = app.db.begin().await.map_err(|e| refusal(e.into()))?;
        permit.check(&mut tx, &app).await.map_err(refusal)?;
        tx.commit()
            .await
            .map_err(|e| refusal(DbError::from(e).into()))?;
        Ok(result)
    }

    async fn prepare_result(
        &self,
        conn: &mut Conn,
        answer: Id,
        body: &mut Json,
    ) -> Result<bool, EngineError> {
        let saved: Option<(Id, String, Json)> =
            sqlx::query_as("SELECT thread_id,tool,plan FROM thread_tool_calls WHERE answer_id=$1")
                .bind(answer)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
        let Some((thread, tool, plan)) = saved else {
            return Ok(false);
        };
        if body
            .get("thread_id")
            .and_then(Json::as_str)
            .and_then(|text| text.parse::<Id>().ok())
            != Some(thread)
            || body
                .get("call_ref")
                .and_then(Json::as_str)
                .is_none_or(|call| orch_engine::ids::tool_result(thread, call) != answer)
        {
            return Err(EngineError::StaleToolSource);
        }
        if !OrchTool::parse(&tool).is_some_and(OrchTool::is_owner_read)
            || body.get("result").is_none()
            || body.get("error").is_some_and(|v| !v.is_null())
        {
            return Ok(false);
        }
        let Some(record) = plan.get("owner_read") else {
            return Ok(false);
        };
        let record: OwnerRead = serde_json::from_value(record.clone())?;
        if record.completed {
            return Ok(false);
        }
        let app = self
            .app
            .upgrade()
            .ok_or_else(|| EngineError::unreadable("The server's read tools are unavailable."))?;
        transaction_guard::lock_action(conn).await?;
        let valid = current(&app, conn, thread, record.owner, record.root)
            .await
            .map_err(|e| EngineError::BeforeEffect(Box::new(e)))?;
        if valid {
            return Ok(true);
        }
        body["result"] = Json::Null;
        body["error"] = json!({"code":"not_yours","message":SHARED_POINTER});
        Ok(false)
    }
}
