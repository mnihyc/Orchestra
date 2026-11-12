//! Accepted author-definition boundaries, separate from completed controller actions.
//! Each origin is restored and admitted independently against the current definition.

use super::content;

use super::operation;

use super::rows;

use super::target;

use crate::key_operations::admission::binding_error;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::state::AppState;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::input_schedules::InputScheduleTarget;

use orch_authz::Act;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use orch_authz::Policy;

use orch_db::Conn;

use orch_engine::operation_scope::blob_sources;

use orch_engine::operation_scope::targets::TargetError;

use orch_types::Id;

use orch_types::Json;
const CREATE: &str = "TopicInputScheduleCreate";

const EDIT: &str = "InputScheduleUpdate.definition";

fn invalid_origin() -> ApiError {
    ApiError::new(
        ErrorCode::AuthInvalidToken,
        "the schedule's authored credential is unavailable",
    )
}

fn decode(row: &rows::ScheduleRow) -> ApiResult<Vec<CapturedOperation>> {
    let raw = row
        .definition_authority
        .as_ref()
        .ok_or_else(invalid_origin)?;
    let origins: Vec<CapturedOperation> =
        serde_json::from_value(raw.clone()).map_err(|_| invalid_origin())?;
    if origins.is_empty()
        || origins.iter().any(|origin| {
            origin.user() != row.author_id || !matches!(origin.operation(), CREATE | EDIT)
        })
    {
        return Err(invalid_origin());
    }
    Ok(origins)
}

/// Capture every newly accepted author definition, including browser and legacy-key
/// callers. A state controller neither becomes its author nor lends it future authority.
pub(super) fn capture(row: &rows::ScheduleRow, caller: &Caller) -> ApiResult<Json> {
    if caller.user() != row.author_id {
        return Err(invalid_origin());
    }
    // Future work has always used ordinary author rights, never browser As-admin.
    // Removing that mode keeps the exact accepted selection and all key restrictions.
    let ordinary = caller.clone().with_admin_mode(false);
    let origin = CapturedOperation::capture(&ordinary).map_err(|_| invalid_origin())?;
    if !matches!(origin.operation(), CREATE | EDIT) {
        return Err(invalid_origin());
    }
    let mut origins = if row.definition_authority.is_some() {
        decode(row)?
    } else {
        Vec::new()
    };
    if !origins.contains(&origin) {
        origins.push(origin);
    }
    serde_json::to_value(origins).map_err(ApiError::internal)
}

/// Only historical rows whose session-only producer stored no origin use the old
/// ordinary-author path. Present but invalid evidence never selects that path.
pub enum Restored {
    Historical(Box<Caller>),
    Captured {
        origins: Vec<CapturedOperation>,
        callers: Vec<Caller>,
    },
}

impl Restored {
    pub(super) fn callers(&self) -> &[Caller] {
        match self {
            Self::Historical(caller) => std::slice::from_ref(caller.as_ref()),
            Self::Captured { callers, .. } => callers,
        }
    }
}

async fn restore_origin(
    app: &AppState,
    conn: &mut Conn,
    origin: &CapturedOperation,
) -> ApiResult<Caller> {
    match origin.restore(conn, app.clock.time()).await {
        Ok(Some(caller)) => Ok(caller.with_admin_mode(false)),
        Ok(None) | Err(orch_authz::Error::Stored(_)) => Err(invalid_origin()),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

async fn source_failed(conn: &mut Conn, error: blob_sources::Error) -> ApiError {
    match error {
        blob_sources::Error::Binding(error) => binding_error(error),
        blob_sources::Error::Missing | blob_sources::Error::Target(TargetError::Missing) => {
            ApiError::not_found()
        }
        blob_sources::Error::Target(TargetError::Db(error)) => error.into(),
        blob_sources::Error::Target(TargetError::Authz(error)) => {
            refusals::problem(conn, error).await
        }
        blob_sources::Error::Source(orch_chat::Error::Db(error)) => error.into(),
        blob_sources::Error::Source(orch_chat::Error::NotFound) => ApiError::not_found(),
        blob_sources::Error::Source(orch_chat::Error::Invalid { path, message }) => {
            ApiError::invalid(&path, &message)
        }
        blob_sources::Error::Source(error) => ApiError::internal(error),
        blob_sources::Error::Declaration(error) => ApiError::internal(error),
    }
}

/// Rebind each original author operation to the same current definition and resolve
/// one complete readable witness for every hash under that origin alone.
pub(super) async fn restore(
    app: &AppState,
    conn: &mut Conn,
    row: &rows::ScheduleRow,
) -> ApiResult<Restored> {
    if row.definition_authority.is_none() {
        return Ok(Restored::Historical(Box::new(Caller::session(
            row.author_id,
        ))));
    }
    let origins = decode(row)?;
    let hashes = orch_chat::following::content_blobs(&content(row)?);
    let mut callers = Vec::with_capacity(origins.len());
    for origin in &origins {
        let caller = restore_origin(app, conn, origin).await?;
        // Keep native classification behind the same known destination and
        // ordinary authority prefix as HTTP definition preparation.
        let known = operation::definition_base(conn, row, origin.operation()).await?;
        let preparing = known.preparation_caller(&caller).map_err(binding_error)?;
        let (destination, permission) = match target(row)? {
            InputScheduleTarget::Thread { thread_id } => (thread_id, Permission::ThreadInteract),
            InputScheduleTarget::NewThread { .. } => (row.topic_id, Permission::TopicStartThreads),
        };
        require(app, conn, &preparing, destination, permission).await?;
        let mut plan = operation::definition_plan(app, conn, row, origin.operation()).await?;
        let preparing = plan.preparation_caller(&caller).map_err(binding_error)?;
        let principal = authorization::load_principal(conn, preparing, app.clock.time()).await?;
        let sources = match blob_sources::prepare(conn, &principal, &plan, &hashes, &[]).await {
            Ok(sources) => sources,
            Err(error) => return Err(source_failed(conn, error).await),
        };
        if let Err(error) = blob_sources::bind(&mut plan, &sources.bindings) {
            return Err(source_failed(conn, error).await);
        }
        callers.push(plan.admit(&caller, false).map_err(binding_error)?);
    }
    Ok(Restored::Captured { origins, callers })
}

async fn require(
    app: &AppState,
    conn: &mut Conn,
    caller: &Caller,
    node: Id,
    permission: Permission,
) -> ApiResult<()> {
    let principal = authorization::load_principal(conn, caller.clone(), app.clock.time()).await?;
    let decision = match orch_authz::nodes::check(
        conn,
        &principal,
        node,
        permission,
        Act::Use,
        Policy::default(),
    )
    .await
    {
        Ok(decision) => decision,
        Err(error) => return Err(refusals::problem(conn, error).await),
    };
    if let orch_access_contract::decisions::Decision::Refused(refusal) = decision {
        return Err(refusals::denied(conn, refusal).await?);
    }
    Ok(())
}

/// Check the original creation permission under every independent current caller
/// before the one ordinary thread writer runs. Input acceptance then checks all again.
pub(super) async fn require_creation(
    app: &AppState,
    conn: &mut Conn,
    restored: &Restored,
    topic: Id,
) -> ApiResult<()> {
    for caller in restored.callers() {
        require(app, conn, caller, topic, Permission::TopicStartThreads).await?;
    }
    Ok(())
}

/// A native catalogue refresh is effect-free preparation, not definition execution.
/// Admit its known subject/destination and ordinary Interact before that external wait;
/// the locked claim rebinds native management and all blob sources afterwards.
pub(super) async fn before_refresh(
    app: &AppState,
    conn: &mut Conn,
    row: &rows::ScheduleRow,
) -> ApiResult<()> {
    let InputScheduleTarget::Thread { thread_id } = target(row)? else {
        return Ok(());
    };
    if row.definition_authority.is_none() {
        return require(
            app,
            conn,
            &Caller::session(row.author_id),
            thread_id,
            Permission::ThreadInteract,
        )
        .await;
    }
    for origin in decode(row)? {
        let caller = restore_origin(app, conn, &origin).await?;
        let plan = operation::definition_base(conn, row, origin.operation()).await?;
        let caller = plan.preparation_caller(&caller).map_err(binding_error)?;
        require(app, conn, &caller, thread_id, Permission::ThreadInteract).await?;
    }
    Ok(())
}
