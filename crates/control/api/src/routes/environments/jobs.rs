//! The jobs of environments over HTTP: reading one with how far it has come, and asking that one be stopped.
//!
//! A job is work that takes a while and is carried out apart from the request that asked for it: an
//! environment being made or started, a unit being removed. Its row is the truth about how far it has
//! come, so that a client that reads it after the API restarted sees the same bar. A job is read by
//! those who may see what it is about (the thread, the environment or the provider), and stopped by
//! those who could have started it, until its commit phase; a removal is not stopped, it is withdrawn
//! before it begins.

use crate::middleware::authorization::{self, VIEW, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::JobState;
use orch_api_contract::workspace::{self as api, Job, JobError};
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::jobs::{self as rows, JobRow, kind};
use orch_engine::jobs::runner::read_progress;
use orch_types::{Id, Time};

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(api::JobGet, get).on(api::JobCancel, cancel);
}

/// The state of a job as the API writes it.
fn state_of(word: &str) -> JobState {
    match word {
        "queued" => JobState::Queued,
        "running" => JobState::Running,
        "succeeded" => JobState::Succeeded,
        "cancelled" => JobState::Cancelled,
        "lost" => JobState::Lost,
        _ => JobState::Failed,
    }
}

/// The job as the API shows it.
pub(crate) async fn view(conn: &mut Conn, row: &JobRow) -> ApiResult<Job> {
    let (mut progress, log) = read_progress(row);
    let waiting_for = match row
        .plan
        .pointer("/active_command/machine")
        .and_then(orch_types::Json::as_str)
        .and_then(|value| value.parse::<Id>().ok())
    {
        Some(machine) => orch_db::queries::resources::machine(conn, machine)
            .await?
            .map(|machine| machine.name),
        None => None,
    };
    let stall_after_seconds = orch_settings::get::<i64>(
        conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::JOB_STALL_SECS,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or_default();
    let activity = progress.activity.get_or_insert(api::JobActivity {
        waiting_for: None,
        updated_at: Time::new(row.updated_at),
        stall_after_seconds,
        directories_done: None,
        stopping: false,
    });
    activity.stall_after_seconds = stall_after_seconds;
    activity.stopping = row.is_open() && row.cancel_requested();
    if activity.stopping {
        activity.stall_after_seconds = orch_settings::get::<i64>(
            conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::JOB_STOP_WAIT_SECS,
        )
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_default();
        if let Some(at) = row
            .plan
            .pointer("/cancel/at")
            .and_then(orch_types::Json::as_str)
            .and_then(|value| value.parse::<Time>().ok())
        {
            activity.updated_at = at;
        }
    }
    if let Some(name) = waiting_for {
        activity.waiting_for = Some(api::JobWaiting::Machine { name });
    }
    let by = match row.created_by {
        Some(person) => refusals::user_refs(conn, &[person]).await?.remove(&person),
        None => None,
    };
    let error = row.error.as_ref().map(|failed| JobError {
        code: text(failed, "code"),
        message: orch_api_contract::messages::for_failure(&text(failed, "code")),
        phase: text(failed, "phase"),
        undone: failed
            .get("undone")
            .and_then(orch_types::Json::as_bool)
            .unwrap_or(false),
    });
    let call_recovery = if row.kind == orch_engine::jobs::fork::KIND && progress.phase == "history"
    {
        if let Some(node) = row.node_id {
            orch_engine::calls::read_workflow(
                conn,
                node,
                orch_provider_contract::calls::Purpose::Summary,
                row.id,
            )
            .await
            .map_err(ApiError::engine)?
            .filter(|recovery| {
                recovery.context["workflow_id"] == serde_json::json!(row.id)
                    && !matches!(recovery.state.as_str(), "completed" | "dismissed")
            })
            .map(|recovery| orch_engine::calls::presentation(&recovery))
            .transpose()
            .map_err(ApiError::engine)?
        } else {
            None
        }
    } else {
        None
    };
    Ok(Job {
        call_recovery,
        id: row.id,
        kind: row.kind.clone(),
        node_id: row.node_id,
        environment_id: row.environment_id,
        provider_id: row.provider_id,
        batch_id: row.batch_id,
        state: state_of(&row.state),
        progress,
        result: row.result.clone(),
        error,
        log,
        created_by: by,
        created_at: Time::new(row.created_at),
        started_at: row.started_at.map(Time::new),
        finished_at: row.finished_at.map(Time::new),
    })
}

fn text(failed: &orch_types::Json, member: &str) -> String {
    failed
        .get(member)
        .and_then(orch_types::Json::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// A job that writes an archive is its person's alone: nobody else reads how far it has come or stops it, and for
/// anybody else it is not there.
fn makers_own(principal: &Principal, row: &JobRow) -> ApiResult<()> {
    if row.created_by == Some(principal.user()) {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

/// Whether the caller may see what the job is about: its thread, its environment or its provider.
/// A snapshot and a delivery are about the files of a thread, and the workspace decides.
async fn may_see(conn: &mut Conn, principal: &Principal, row: &JobRow) -> ApiResult<bool> {
    if row.kind == orch_engine::jobs::cleanup::KIND || row.kind == orch_engine::resource_pause::KIND
    {
        return makers_own(principal, row).map(|()| false);
    }
    if row.kind == kind::DELIVERY && row.plan.get("workspace_upload").is_some() {
        authorization::require_resource(
            conn,
            principal,
            ResourceKind::Environment,
            row.environment_id.ok_or_else(ApiError::not_found)?,
            Permission::FilesView,
            Policy::default(),
        )
        .await?;
        return Ok(false);
    }
    if row.kind == orch_engine::jobs::host_delete::KIND {
        let host = serde_json::from_value::<orch_types::Id>(row.plan["cascade"]["host"].clone())
            .map_err(ApiError::internal)?;
        authorization::require_resource(
            conn,
            principal,
            ResourceKind::Host,
            host,
            Permission::HostView,
            Policy::default(),
        )
        .await?;
        return Ok(false);
    }
    if row.kind == orch_engine::jobs::measurement::KIND {
        measurement_access(conn, principal, row, false).await?;
        return Ok(false);
    }
    if row.kind == orch_db::queries::thread_continuations::JOB_KIND {
        authorization::require(
            conn,
            principal,
            row.node_id.ok_or_else(ApiError::not_found)?,
            VIEW,
            Act::Use,
            Policy::default(),
        )
        .await?;
        return Ok(false);
    }
    if kind::SOURCE.contains(&row.kind.as_str()) {
        let environment = row.environment_id.ok_or_else(ApiError::not_found)?;
        if authorization::require_resource(
            conn,
            principal,
            ResourceKind::Environment,
            environment,
            Permission::FilesView,
            Policy::default(),
        )
        .await
        .is_ok()
        {
            return Ok(false);
        }
        super::reading::decide(conn, principal, environment).await?;
        return Ok(true);
    }
    if kind::WORKSPACE.contains(&row.kind.as_str()) {
        return crate::routes::workspace::snapshots::may_read_job(conn, principal, row)
            .await
            .map(|()| false);
    }
    if row.kind == orch_engine::jobs::document_notices::KIND {
        let document = serde_json::from_value(row.plan["notice"]["document"].clone())
            .map_err(ApiError::internal)?;
        authorization::require_document(
            conn,
            principal,
            document,
            Permission::DocView,
            Policy::default(),
        )
        .await?;
        return makers_own(principal, row).map(|()| false);
    }
    if row.kind == kind::EXPORT || row.kind == kind::IMPORT {
        return makers_own(principal, row).map(|()| false);
    }
    if matches!(
        row.kind.as_str(),
        rows::retained::RESTORE | rows::retained::PURGE
    ) {
        super::retained::may_read(
            conn,
            principal,
            row.environment_id.ok_or_else(ApiError::not_found)?,
        )
        .await?;
        return Ok(false);
    }
    let mut first: Option<ApiError> = None;
    if let Some(environment) = row.environment_id {
        match super::reading::decide(conn, principal, environment).await {
            Ok(super::reading::Reading::Whole) => return Ok(false),
            Ok(super::reading::Reading::ThroughAThread) => {
                let current = rows::open_of_environment(conn, environment).await?;
                if current.first().is_some_and(|current| current.id == row.id) {
                    return Ok(true);
                }
                return Err(ApiError::not_found());
            }
            Err(error) => first = first.or(Some(error)),
        }
    }
    if let Some(thread) = row.node_id {
        match authorization::require(conn, principal, thread, VIEW, Act::Use, Policy::default())
            .await
        {
            Ok(()) => return Ok(false),
            Err(error) => first = first.or(Some(error)),
        }
    }
    if let Some(provider) = row.provider_id {
        match authorization::require_resource(
            conn,
            principal,
            ResourceKind::Provider,
            provider,
            Permission::ProviderManage,
            Policy::default(),
        )
        .await
        {
            Ok(()) => return Ok(false),
            Err(error) => first = first.or(Some(error)),
        }
    }
    Err(first.unwrap_or_else(ApiError::not_found))
}

/// Whether the caller could have started the job: whoever manages its environment or its provider, or
/// acts in its thread. A snapshot and a delivery are started by whoever writes the thread's files.
async fn could_have_started(conn: &mut Conn, principal: &Principal, row: &JobRow) -> ApiResult<()> {
    if row.kind == orch_engine::jobs::cleanup::KIND || row.kind == orch_engine::resource_pause::KIND
    {
        return makers_own(principal, row);
    }
    if row.kind == kind::DELIVERY && row.plan.get("workspace_upload").is_some() {
        return authorization::require_resource(
            conn,
            principal,
            ResourceKind::Environment,
            row.environment_id.ok_or_else(ApiError::not_found)?,
            Permission::FilesManage,
            Policy::default(),
        )
        .await;
    }
    if row.kind == orch_engine::jobs::host_delete::KIND {
        let host = serde_json::from_value::<orch_types::Id>(row.plan["cascade"]["host"].clone())
            .map_err(ApiError::internal)?;
        return authorization::require_resource(
            conn,
            principal,
            ResourceKind::Host,
            host,
            Permission::HostManage,
            Policy::default(),
        )
        .await;
    }
    if row.kind == orch_engine::jobs::measurement::KIND {
        return measurement_access(conn, principal, row, true).await;
    }
    if row.kind == orch_db::queries::thread_continuations::JOB_KIND {
        let thread = row.node_id.ok_or_else(ApiError::not_found)?;
        authorization::require(conn, principal, thread, VIEW, Act::Use, Policy::default()).await?;
        // This records cancellation only. The claimed runner still reconciles possibly
        // started native work and restores its exact original lane before finishing.
        // No fresh provider use or environment-management authority is borrowed for cleanup.
        return authorization::require(
            conn,
            principal,
            thread,
            PermissionName::OfNode(NodePermission::Interact),
            Act::Use,
            Policy::default(),
        )
        .await;
    }
    if kind::SOURCE.contains(&row.kind.as_str()) {
        return authorization::require_resource(
            conn,
            principal,
            ResourceKind::Environment,
            row.environment_id.ok_or_else(ApiError::not_found)?,
            if row.kind == kind::WORKSPACE_PREPARE {
                Permission::EnvironmentManage
            } else {
                Permission::FilesManage
            },
            Policy::default(),
        )
        .await;
    }
    if kind::WORKSPACE.contains(&row.kind.as_str()) {
        return crate::routes::workspace::snapshots::may_cancel_job(conn, principal, row).await;
    }
    if row.kind == kind::EXPORT
        || row.kind == kind::IMPORT
        || row.kind == orch_engine::jobs::document_notices::KIND
    {
        return makers_own(principal, row);
    }
    // The part that forks and switches: whoever confirmed the job may stop it, and whoever could have
    // started it: they manage the thread it changes or the environment it lands in.
    if row.kind == "thread_fork" {
        if row.created_by == Some(principal.user()) {
            return Ok(());
        }
        let mut first: Option<ApiError> = None;
        if let Some(thread) = row.node_id {
            match authorization::require(
                conn,
                principal,
                thread,
                orch_access_contract::permissions::PermissionName::OfNode(NodePermission::Manage),
                Act::Use,
                Policy::default(),
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => first = first.or(Some(error)),
            }
        }
        if let Some(environment) = row.environment_id {
            match authorization::require_resource(
                conn,
                principal,
                ResourceKind::Environment,
                environment,
                Permission::EnvironmentManage,
                Policy::default(),
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => first = first.or(Some(error)),
            }
        }
        return Err(first.unwrap_or_else(ApiError::not_found));
    }
    if let Some(environment) = row.environment_id {
        return authorization::require_resource(
            conn,
            principal,
            ResourceKind::Environment,
            environment,
            Permission::EnvironmentManage,
            Policy::default(),
        )
        .await;
    }
    if let Some(provider) = row.provider_id {
        return authorization::require_resource(
            conn,
            principal,
            ResourceKind::Provider,
            provider,
            Permission::ProviderManage,
            Policy::default(),
        )
        .await;
    }
    if let Some(thread) = row.node_id {
        return authorization::require(
            conn,
            principal,
            thread,
            PermissionName::OfNode(NodePermission::Interact),
            Act::Use,
            Policy::default(),
        )
        .await;
    }
    Err(ApiError::not_found())
}

async fn row_of(conn: &mut Conn, id: Id) -> ApiResult<JobRow> {
    rows::job(conn, id).await?.ok_or_else(ApiError::not_found)
}

/// Reads a job.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Job> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    read_for(&mut conn, &principal, id).await
}

/// The exact job read and progress-only cut, shared by HTTP and live delivery.
pub(crate) async fn read_for(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<Job> {
    let row = row_of(conn, id).await?;
    let cut = may_see(conn, principal, &row).await?;
    let mut shown = view(conn, &row).await?;
    if cut {
        shown.node_id = None;
        shown.provider_id = None;
        shown.batch_id = None;
        shown.result = None;
        shown.error = None;
        shown.log = None;
        shown.created_by = None;
        shown.started_at = None;
        shown.finished_at = None;
        shown.progress.phases.clear();
        shown.progress.files_done = 0;
        shown.progress.files_total = None;
        shown.progress.bytes_done = 0;
        shown.progress.bytes_total = None;
        shown.progress.rate_bps = None;
        shown.progress.eta_seconds = None;
        shown.progress.message = None;
        shown.progress.activity = None;
    }
    Ok(shown)
}

async fn measurement_access(
    conn: &mut Conn,
    principal: &Principal,
    row: &JobRow,
    cancel: bool,
) -> ApiResult<()> {
    let subject = &row.plan["subject"];
    let object = |field: &str| {
        subject[field]
            .as_str()
            .and_then(|value| value.parse::<Id>().ok())
    };
    let (kind, id, permission) = if let Some(id) = object("environment_id") {
        (
            ResourceKind::Environment,
            id,
            if cancel {
                Permission::EnvironmentManage
            } else {
                Permission::FilesView
            },
        )
    } else if let Some(id) = object("host_id") {
        (ResourceKind::Host, id, Permission::HostManage)
    } else if let Some(id) = object("mount_id") {
        (
            ResourceKind::Mount,
            id,
            if cancel {
                Permission::MountManage
            } else {
                Permission::MountUse
            },
        )
    } else {
        return Err(ApiError::not_found());
    };
    // The maker needs current read access; the cancellation reply contains the live count too.
    if cancel && row.created_by == Some(principal.user()) {
        let read = match kind {
            ResourceKind::Environment => Permission::FilesView,
            ResourceKind::Host => Permission::HostManage,
            _ => Permission::MountUse,
        };
        return authorization::require_resource(conn, principal, kind, id, read, Policy::default())
            .await;
    }
    authorization::require_resource(conn, principal, kind, id, permission, Policy::default()).await
}

/// Asks that a job be stopped: it is undone as a failure is, if it has not passed its commit phase.
/// A job that is over is shown as it is; a removal is not stopped, and a job that passed its commit
/// phase is not either.
async fn cancel(ctx: Ctx, (): (), (): ()) -> ApiResult<Job> {
    let id = ctx.id("id")?;
    // Whether the caller may cancel and the cancellation itself are one transaction, which asked the
    // caller's credential again; the runner is woken once it is committed.
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let row = row_of(&mut tx, id).await?;
    could_have_started(&mut tx, &principal, &row).await?;
    let mut asked = false;
    if row.is_open() {
        if row.kind == kind::ENVIRONMENT_REMOVE || row.kind == rows::retained::PURGE {
            return Err(ApiError::new(
                ErrorCode::PlanBlocked,
                "a removal is not stopped once it is under way; keep the environment before it begins",
            ));
        }
        asked = rows::request_cancel(&mut tx, id, principal.user(), ctx.now).await?;
        if !asked {
            return Err(ApiError::new(
                ErrorCode::PlanBlocked,
                "the job is past the point after which nothing is undone",
            ));
        }
    }
    let row = row_of(&mut tx, id).await?;
    let shown = if row.kind == orch_db::queries::thread_continuations::JOB_KIND {
        read_for(&mut tx, &principal, row.id).await?
    } else {
        view(&mut tx, &row).await?
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if asked {
        ctx.app
            .engine
            .stop_job_command(id)
            .await
            .map_err(ApiError::internal)?;
        ctx.app.engine.kick();
    }
    Ok(shown)
}
