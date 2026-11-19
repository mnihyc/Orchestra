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
use std::collections::HashMap;

mod cancel;
mod read;
pub(crate) mod subject;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(
            api::JobGet,
            crate::key_operations::native::selected::<read::Reader>,
            crate::key_operations::native::selected_read::<read::Reader>,
        )
        .on_with_operation(
            api::JobCancel,
            crate::key_operations::native::effects::prepare::<cancel::Cancel>,
            cancel::run,
        );
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
    view_with_machine_sources(conn, row, None).await
}

pub(crate) fn waiting_machine(row: &JobRow) -> Option<Id> {
    row.plan
        .pointer("/active_command/machine")
        .and_then(orch_types::Json::as_str)
        .and_then(|value| value.parse().ok())
}

/// Actual native waiting-name sources, before any job DTO is produced.
pub(crate) async fn machine_name_sources(
    conn: &mut Conn,
    rows: &[JobRow],
) -> ApiResult<HashMap<Id, String>> {
    let ids = rows.iter().filter_map(waiting_machine).collect::<Vec<_>>();
    Ok(
        orch_db::queries::resources::routes::machine_names(conn, &ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row.name))
            .collect(),
    )
}

/// The same native projection, with an optional trusted bound on emitted machine names.
/// Ordinary callers retain their original current/historical progress presentation.
pub(crate) async fn view_with_machine_sources(
    conn: &mut Conn,
    row: &JobRow,
    names: Option<&HashMap<Id, String>>,
) -> ApiResult<Job> {
    let (mut progress, log) = read_progress(row);
    let waiting_for = match waiting_machine(row) {
        Some(machine) => match names {
            Some(names) => names.get(&machine).cloned(),
            None => orch_db::queries::resources::machine(conn, machine)
                .await?
                .map(|machine| machine.name),
        },
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
    if names.is_some()
        && matches!(
            activity.waiting_for.as_ref(),
            Some(api::JobWaiting::Machine { .. })
        )
    {
        // An old cached label is not evidence for the currently selected source.
        activity.waiting_for = None;
    }
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

/// The original native read door, backed by the same factual branch producer.
async fn may_see(conn: &mut Conn, principal: &Principal, row: &JobRow) -> ApiResult<bool> {
    subject::read(conn, principal, row, None)
        .await
        .map(|choice| choice.cut)
}

/// Whether the caller could have started the job: whoever manages its environment or its provider, or
/// acts in its thread. A snapshot and a delivery are started by whoever writes the thread's files.
async fn could_have_started(conn: &mut Conn, principal: &Principal, row: &JobRow) -> ApiResult<()> {
    subject::cancel(conn, principal, row, None)
        .await
        .map(|_| ())
}

async fn row_of(conn: &mut Conn, id: Id) -> ApiResult<JobRow> {
    rows::job(conn, id).await?.ok_or_else(ApiError::not_found)
}

/// The exact job read and progress-only cut, shared by HTTP and live delivery.
pub(crate) async fn read_for(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<Job> {
    prepare_read_for(conn, principal, id)
        .await?
        .render(conn, None)
        .await
}

/// The exact native job row and its ordinary progress-only cut, before public formatting.
pub(crate) struct PreparedRead {
    row: JobRow,
    cut: bool,
}

impl PreparedRead {
    pub(crate) fn row(&self) -> &JobRow {
        &self.row
    }
    pub(crate) fn progress_only(&self) -> bool {
        self.cut
    }
    pub(crate) fn machine_source(&self) -> Option<Id> {
        waiting_machine(&self.row)
    }

    /// Native identity and field authority, without comparing changing progress content.
    pub(crate) fn preserves(&self, fresh: &Self) -> bool {
        self.row.id == fresh.row.id
            && self.row.kind == fresh.row.kind
            && self.row.environment_id == fresh.row.environment_id
            && self.row.node_id == fresh.row.node_id
            && self.row.provider_id == fresh.row.provider_id
            && self.row.batch_id == fresh.row.batch_id
            && self.row.created_at == fresh.row.created_at
            && self.row.created_by == fresh.row.created_by
            && (self.cut || !fresh.cut)
    }

    pub(crate) async fn require_current(
        &self,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<()> {
        let cut = may_see(conn, principal, &self.row).await?;
        if !self.cut && cut {
            return Err(ApiError::not_found());
        }
        Ok(())
    }

    /// The original native DTO and its unchanged explicit progress cut. A source bound
    /// only restricts the existing named machine field, never stored JSON by inspection.
    pub(crate) async fn render(
        &self,
        conn: &mut Conn,
        names: Option<&HashMap<Id, String>>,
    ) -> ApiResult<Job> {
        let mut shown = view_with_machine_sources(conn, &self.row, names).await?;
        if self.cut {
            progress_only(&mut shown);
        }
        Ok(shown)
    }
}

/// The same ordinary job reader's selection and gate, reusable by stored native composites.
pub(crate) async fn prepare_read_for(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<PreparedRead> {
    let row = row_of(conn, id).await?;
    let cut = may_see(conn, principal, &row).await?;
    Ok(PreparedRead { row, cut })
}

fn progress_only(shown: &mut Job) {
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

fn measurement_subject(row: &JobRow) -> ApiResult<orch_access_contract::decisions::ObjectRef> {
    use orch_access_contract::permissions::ObjectKind;
    let subject = &row.plan["subject"];
    [
        ("environment_id", ObjectKind::Environment),
        ("host_id", ObjectKind::Host),
        ("mount_id", ObjectKind::Mount),
    ]
    .into_iter()
    .find_map(|(field, kind)| {
        subject[field]
            .as_str()
            .and_then(|value| value.parse::<Id>().ok())
            .map(|id| orch_access_contract::decisions::ObjectRef { kind, id })
    })
    .ok_or_else(ApiError::not_found)
}

async fn measurement_access(
    conn: &mut Conn,
    principal: &Principal,
    row: &JobRow,
    cancel: bool,
) -> ApiResult<()> {
    let object = measurement_subject(row)?;
    let id = object.id;
    let (kind, permission) = match object.kind {
        orch_access_contract::permissions::ObjectKind::Environment => (
            ResourceKind::Environment,
            if cancel {
                Permission::EnvironmentManage
            } else {
                Permission::FilesView
            },
        ),
        orch_access_contract::permissions::ObjectKind::Host => {
            (ResourceKind::Host, Permission::HostManage)
        }
        orch_access_contract::permissions::ObjectKind::Mount => (
            ResourceKind::Mount,
            if cancel {
                Permission::MountManage
            } else {
                Permission::MountUse
            },
        ),
        _ => return Err(ApiError::not_found()),
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
