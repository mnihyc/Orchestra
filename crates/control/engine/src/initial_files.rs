//! Authorized initial workspace selections delivered before a new environment becomes runnable.

use crate::{
    EngineError,
    commands::Outgoing,
    ids,
    jobs::runner::{Done, Failure, Job, Stop},
    manifest::{self, EntryKind},
};
use orch_access_contract::decisions::{Decision, ObjectRef, Refusal};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::{binding::NewEnvironmentFiles, forks::FileSelection};
use orch_authz::{Act, Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn,
    queries::{
        environments::{thread_engine as environments, workspace},
        jobs, storage, threads, transaction_guard,
    },
};
use orch_link_contract::commands::{
    FsMaterialize, FsMaterializeResult, FsSnapshot, FsSnapshotResult, MaterializePurpose,
    Selection, SerialKey, validate_selection_pattern,
};
use orch_link_contract::source::{FsRecordRead, FsRecordReadResult};
use orch_types::{Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeSet, HashSet};

/// The accepted source identity and caller, retained with the provisioning job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    caller: Id,
    token: Option<Id>,
    thread: Id,
    environment: Id,
    generation: i32,
    directory: Option<String>,
    selection: Option<Selection>,
    empty: bool,
    snapshot: Option<Id>,
    manifest: Option<Hash>,
    local: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creator_origin: Option<CapturedOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creator_thread: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_environment: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_generation: Option<i32>,
}

impl Plan {
    /// The actual source Thread resolved by the ordinary initial-file planner.
    #[must_use]
    pub fn source_thread(&self) -> Id {
        self.thread
    }

    /// The recorded source Environment, including for a saved snapshot.
    #[must_use]
    pub fn source_environment(&self) -> Id {
        self.environment
    }

    /// Attach the creator's full admission at its last rollbackable checkpoint. The shared
    /// capture retains every credential boundary, rather than choosing a representative token.
    pub fn capture_creator(
        &mut self,
        caller: &Caller,
        created_thread: Id,
        target_environment: Id,
        target_generation: i32,
    ) -> Result<(), EngineError> {
        let origin = CapturedOperation::capture(&caller.clone().with_admin_mode(false))
            .map_err(EngineError::unreadable)?;
        if caller.user() != self.caller
            || !matches!(
                origin.operation(),
                "ThreadCreate" | "TopicCreate.with_thread"
            )
            || target_generation < 0
            || self
                .creator_origin
                .as_ref()
                .is_some_and(|old| old != &origin)
            || self.creator_thread.is_some_and(|old| old != created_thread)
            || self
                .target_environment
                .is_some_and(|old| old != target_environment)
            || self
                .target_generation
                .is_some_and(|old| old != target_generation)
        {
            return Err(EngineError::unreadable(
                "the initial-file plan does not belong to this admitted creator",
            ));
        }
        self.creator_origin = Some(origin);
        self.creator_thread = Some(created_thread);
        self.target_environment = Some(target_environment);
        self.target_generation = Some(target_generation);
        Ok(())
    }
}

/// Why no initial-file plan was accepted; refusal precedes any environment or job creation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No readable live source reference was found.
    #[error("the initial file source is not available")]
    NotFound,
    /// A source permission is missing.
    #[error("the initial file source is not authorized")]
    Denied(Box<Refusal>),
    /// A selection leaves its root or cannot be represented.
    #[error("{0}")]
    Invalid(String),
    /// A source reference is present but cannot supply complete files.
    #[error("{0}")]
    Unavailable(String),
    /// A control-plane dependency failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}
impl From<orch_db::DbError> for Error {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}
impl From<orch_authz::Error> for Error {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

fn relative(path: &str) -> bool {
    path.len() <= 4096
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && (path.is_empty()
            || path
                .split('/')
                .all(|part| !part.is_empty() && !matches!(part, "." | "..")))
}

async fn rights(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    environment: Id,
) -> Result<(), Error> {
    if let Decision::Refused(why) = orch_authz::nodes::check(
        conn,
        principal,
        thread,
        Permission::ThreadView,
        Act::Use,
        Policy::default(),
    )
    .await?
    {
        return Err(Error::Denied(Box::new(why)));
    }
    if let Decision::Refused(why) = orch_authz::resources::check(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        Permission::FilesView,
        Policy::default(),
    )
    .await?
    {
        return Err(Error::Denied(Box::new(why)));
    }
    Ok(())
}

/// Resolve only the declared source's canonical identities. This performs no permission check
/// or effects: the creator binds these facts before the ordinary planner checks its caller.
pub async fn source_identity(
    conn: &mut Conn,
    requested: &NewEnvironmentFiles,
) -> Result<(Id, Id), Error> {
    match requested {
        NewEnvironmentFiles::FromThread { from_thread, .. } => {
            let thread = threads::thread(conn, *from_thread)
                .await?
                .ok_or(Error::NotFound)?;
            Ok((*from_thread, thread.environment_id.ok_or(Error::NotFound)?))
        }
        NewEnvironmentFiles::Snapshot { snapshot_id } => {
            let row = storage::snapshot(conn, *snapshot_id)
                .await?
                .ok_or(Error::NotFound)?;
            let thread = row.thread_id.ok_or_else(|| Error::Invalid(
                "a workspace export is for download; choose a thread snapshot to copy initial files".into(),
            ))?;
            Ok((thread, row.environment_id))
        }
    }
}

/// Authorizes the declared source, normalizes only its selection, and freezes no arbitrary blob claim.
pub async fn plan(
    conn: &mut Conn,
    principal: &Principal,
    requested: &NewEnvironmentFiles,
) -> Result<Plan, Error> {
    let caller = principal.user();
    let token = principal.caller().grant().and_then(|grant| grant.token());
    let (source_thread, source_environment) = source_identity(conn, requested).await?;
    match requested {
        NewEnvironmentFiles::FromThread {
            from_thread,
            selection,
        } => {
            let thread = threads::thread(conn, *from_thread)
                .await?
                .ok_or(Error::NotFound)?;
            let environment = thread
                .environment_id
                .filter(|id| *id == source_environment)
                .ok_or(Error::NotFound)?;
            rights(conn, principal, *from_thread, environment).await?;
            let env = environments::environment(conn, environment)
                .await?
                .ok_or(Error::NotFound)?;
            if env.status != "running" || env.machine_id.is_none() {
                return Err(Error::Unavailable(
                    "the source environment must be running to select its current files".into(),
                ));
            }
            if !relative(&thread.workspace_dir) {
                return Err(Error::Invalid(
                    "the source directory leaves its workspace".into(),
                ));
            }
            let empty = matches!(selection, Some(FileSelection::Nothing));
            let selected = match selection {
                Some(FileSelection::Selection { include, exclude }) => {
                    if include.is_empty() {
                        return Err(Error::Invalid(
                            "an explicit file selection must name a path".into(),
                        ));
                    }
                    Some(Selection {
                        include: include.clone(),
                        exclude: exclude.clone().unwrap_or_default(),
                    })
                }
                _ => None,
            };
            if let Some(selected) = &selected {
                for path in selected.include.iter().chain(&selected.exclude) {
                    validate_selection_pattern(path)
                        .map_err(|error| Error::Invalid(error.to_string()))?;
                }
                crate::selection::Matcher::new(selected)
                    .map_err(|error| Error::Invalid(error.to_string()))?;
            }
            let observed = workspace::environment(conn, environment)
                .await?
                .ok_or(Error::NotFound)?;
            if !empty
                && selected.is_none()
                && observed.facts.get("index").and_then(Json::as_str) == Some("live")
            {
                return Err(Error::Invalid(
                    "a live workspace needs an explicit initial file selection".into(),
                ));
            }
            Ok(Plan {
                caller,
                token,
                thread: *from_thread,
                environment,
                generation: env.generation,
                directory: Some(thread.workspace_dir),
                selection: selected,
                empty,
                snapshot: None,
                manifest: None,
                local: false,
                creator_origin: None,
                creator_thread: None,
                target_environment: None,
                target_generation: None,
            })
        }
        NewEnvironmentFiles::Snapshot { snapshot_id } => {
            let row = storage::snapshot(conn, *snapshot_id)
                .await?
                .ok_or(Error::NotFound)?;
            let thread = row
                .thread_id
                .filter(|id| *id == source_thread)
                .filter(|_| row.environment_id == source_environment)
                .ok_or(Error::NotFound)?;
            rights(conn, principal, thread, row.environment_id).await?;
            if row.state != "complete"
                || row.missing_count != 0
                || row.manifest_blob.is_none()
                || row.expires_at.is_some_and(|at| at <= principal.now().get())
            {
                return Err(Error::Unavailable(
                    "the selected snapshot is incomplete, expired or unavailable".into(),
                ));
            }
            if row
                .directory
                .as_deref()
                .is_some_and(|directory| !relative(directory))
            {
                return Err(Error::Invalid(
                    "the snapshot directory leaves its workspace".into(),
                ));
            }
            Ok(Plan {
                caller,
                token,
                thread,
                environment: row.environment_id,
                generation: row.env_generation,
                directory: row.directory,
                selection: None,
                empty: false,
                snapshot: Some(row.id),
                manifest: row.manifest_blob,
                local: row.storage == "environment",
                creator_origin: None,
                creator_thread: None,
                target_environment: None,
                target_generation: None,
            })
        }
    }
}

fn failure(message: impl Into<String>) -> Stop {
    Failure::new("initial_files_unavailable", message).into()
}

async fn current_at(
    conn: &mut Conn,
    plan: &Plan,
    now: Time,
) -> Result<environments::EnvironmentRow, Stop> {
    let principal = if let Some(origin) = &plan.creator_origin {
        let (Some(thread), Some(target), Some(_)) = (
            plan.creator_thread,
            plan.target_environment,
            plan.target_generation,
        ) else {
            return Err(failure("the initial file creator evidence is incomplete"));
        };
        if origin.user() != plan.caller {
            return Err(failure("the initial file creator identity changed"));
        }
        let caller = crate::operation_scope::creators::restore_initial_files(
            conn,
            origin,
            thread,
            target,
            plan.thread,
            plan.environment,
            now,
        )
        .await?
        .ok_or_else(|| failure("the accepted creator no longer permits this initial copy"))?;
        Principal::load(conn, caller, now)
            .await
            .map_err(EngineError::from)?
    } else {
        if plan.creator_thread.is_some()
            || plan.target_environment.is_some()
            || plan.target_generation.is_some()
        {
            return Err(failure("the initial file creator evidence is incomplete"));
        }
        let principal =
            crate::jobs::source::load_principal_at(conn, plan.caller, plan.token, now).await?;
        // Old plans remain readable, but a current modern key can never be reconstructed
        // through its legacy scopes after losing the creator's accepted operation.
        if let Some(token) = plan.token {
            let scoped =
                orch_db::queries::identity::api_token_is_operation_scoped(conn, token, plan.caller)
                    .await?;
            if scoped != Some(false) {
                return Err(failure(
                    "the scoped initial copy has no accepted creator origin",
                ));
            }
        }
        principal
    };
    rights(conn, &principal, plan.thread, plan.environment)
        .await
        .map_err(|error| failure(error.to_string()))?;
    let source = environments::environment(conn, plan.environment)
        .await?
        .ok_or_else(|| failure("the initial file source disappeared"))?;
    if let Some(snapshot) = plan.snapshot {
        let row = storage::snapshot(conn, snapshot)
            .await?
            .filter(|row| {
                row.thread_id == Some(plan.thread)
                    && row.environment_id == plan.environment
                    && row.env_generation == plan.generation
                    && row.directory == plan.directory
                    && row.state == "complete"
                    && row.missing_count == 0
                    && row.manifest_blob == plan.manifest
                    && !row.expires_at.is_some_and(|at| at <= now.get())
            })
            .ok_or_else(|| {
                failure("the initial file snapshot changed or is no longer available")
            })?;
        if (row.storage == "environment") != plan.local {
            return Err(failure("the initial file snapshot changed its storage"));
        }
    } else {
        let thread = threads::thread(conn, plan.thread)
            .await?
            .filter(|thread| {
                thread.environment_id == Some(plan.environment)
                    && plan.directory.as_deref() == Some(thread.workspace_dir.as_str())
            })
            .ok_or_else(|| {
                failure("the initial file source thread changed its directory or binding")
            })?;
        let _ = thread;
    }
    if (plan.snapshot.is_none() || plan.local)
        && (source.generation != plan.generation
            || source.status != "running"
            || source.machine_id.is_none())
    {
        return Err(failure(
            "the original source generation is no longer running",
        ));
    }
    Ok(source)
}

async fn current(job: &Job, plan: &Plan) -> Result<environments::EnvironmentRow, Stop> {
    let mut tx = transaction_guard::scope(false, job.engine().db().begin()).await?;
    transaction_guard::lock_action(&mut tx).await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if let Some(reason) = crate::jobs::runner::abort_reason(&owner)? {
        return Err(reason);
    }
    target_at(&mut tx, &owner, plan).await?;
    let source = current_at(&mut tx, plan, Time::new(job.now())).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(source)
}

// A new Environment has not published its connector yet. Only the owning provisioning
// job's actual enrolled candidate can receive this copy; no future generation is inferred.
async fn target_at(
    conn: &mut Conn,
    owner: &jobs::JobRow,
    plan: &Plan,
) -> Result<Option<(environments::EnvironmentRow, Id, Id)>, Stop> {
    let Some(target) = plan.target_environment else {
        return Ok(None);
    };
    let generation = plan
        .target_generation
        .ok_or_else(|| failure("the initial file target generation is absent"))?;
    if !owner.is_open()
        || owner.cancel_requested()
        || owner.kind != jobs::kind::ENVIRONMENT_PROVISION
        || owner.environment_id != Some(target)
        || owner.plan.get("mode").and_then(Json::as_str) != Some("make")
        || owner.plan.get("generation").and_then(Json::as_i64) != Some(i64::from(generation))
        || owner.plan.get("initial_files") != Some(&json!(plan))
    {
        return Err(failure(
            "the initial file target no longer belongs to this provisioning",
        ));
    }
    let runtime = owner
        .plan
        .get("runtime")
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
        .ok_or_else(|| failure("the initial file target has no enrolled runtime"))?;
    let candidate = orch_db::queries::environments::hosting::runtime(conn, runtime)
        .await?
        .ok_or_else(|| failure("the initial file target runtime disappeared"))?;
    let machine = candidate
        .machine_id
        .ok_or_else(|| failure("the initial file target connector has not enrolled"))?;
    let current = environments::environment(conn, target)
        .await?
        .filter(|env| env.generation == generation)
        .ok_or_else(|| failure("the accepted initial file target generation changed"))?;
    if crate::jobs::source::matching_provision_target(
        conn,
        owner,
        target,
        machine,
        candidate.generation,
        runtime,
    )
    .await?
    .is_none()
    {
        return Err(failure("the initial file target runtime changed"));
    }
    Ok(Some((current, runtime, machine)))
}

fn safe_link(path: &str, target: &str) -> bool {
    if target.starts_with('/')
        || target.contains('\\')
        || target.contains(':')
        || target.chars().any(char::is_control)
    {
        return false;
    }
    let mut depth = path.split('/').count().saturating_sub(1);
    for part in target.split('/') {
        match part {
            ".." => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            "" | "." => {}
            _ => depth += 1,
        }
    }
    true
}

fn snapshot_selection(plan: &Plan) -> Selection {
    let directory = plan.directory.as_deref().unwrap_or("");
    let below = |path: &str| {
        if directory.is_empty() {
            path.to_owned()
        } else if path.is_empty() {
            directory.to_owned()
        } else {
            format!("{directory}/{path}")
        }
    };
    plan.selection.as_ref().map_or_else(
        || Selection {
            include: vec![directory.to_owned()],
            exclude: vec![],
        },
        |selection| Selection {
            include: selection.include.iter().map(|path| below(path)).collect(),
            exclude: selection.exclude.iter().map(|path| below(path)).collect(),
        },
    )
}

/// The ordinary runner calls this after its WorkReport/deadline augmentation and active
/// command reservation. Retain that canonical body once, before issuing this native command.
pub(crate) async fn remember_creator_command(job: &Job, outgoing: &Outgoing) -> Result<(), Stop> {
    if !matches!(
        outgoing.kind.as_str(),
        "fs.snapshot" | "fs.record.read" | "fs.materialize"
    ) {
        return Ok(());
    }
    let mut tx = transaction_guard::scope(false, job.engine().db().begin()).await?;
    transaction_guard::lock_action(&mut tx).await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let Some(value) = owner.plan.get("initial_files") else {
        return Ok(());
    };
    let plan: Plan = serde_json::from_value(value.clone()).map_err(EngineError::from)?;
    if plan.creator_origin.is_none() {
        return Ok(());
    }
    let known = [
        "initial-files.snapshot",
        "initial-files.export",
        "initial-files.materialize",
    ]
    .into_iter()
    .any(|phase| outgoing.id == ids::derived(phase, &[owner.id.as_bytes()]));
    if !known {
        return Ok(());
    }
    if let Some(reason) = crate::jobs::runner::abort_reason(&owner)? {
        return Err(reason);
    }
    current_at(&mut tx, &plan, Time::new(job.now())).await?;
    let (_, runtime, _) = target_at(&mut tx, &owner, &plan)
        .await?
        .ok_or_else(|| failure("the initial copy has no accepted target"))?;
    let native = json!({
        "id": outgoing.id, "runtime": runtime, "machine": outgoing.machine,
        "kind": outgoing.kind.as_str(), "key": outgoing.key.wire(), "body": outgoing.body,
    });
    let mut journal = owner
        .plan
        .get("initial_files_commands")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let entries = journal
        .as_object_mut()
        .ok_or_else(|| failure("the initial file command journal is invalid"))?;
    if entries
        .get(&outgoing.id.to_string())
        .is_some_and(|old| old != &native)
    {
        return Err(failure("the accepted initial file command recipe changed"));
    }
    // There are only three stable phase IDs, none a Busy-retry kind. Retaining earlier
    // phases prevents late queued commands becoming ordinary when active_command moves.
    entries.insert(outgoing.id.to_string(), native);
    jobs::merge_plan(
        &mut tx,
        job.id(),
        &json!({"initial_files_commands": journal}),
        job.now(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Recheck a queued initial copy at the dispatcher's existing first-send action barrier.
/// No matching initial-file job means an unrelated ordinary command. Present creator evidence
/// must match the exact owner, active command, accepted recipe and currently enrolled runtime.
pub async fn command_authorized(
    conn: &mut Conn,
    command: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    now: Time,
) -> Result<bool, EngineError> {
    let owners = jobs::lock_initial_files_command_owners(conn, command).await?;
    if owners.is_empty() {
        return Ok(true);
    }
    if owners.len() != 1 {
        return Ok(false);
    }
    let Some(owner) = jobs::job(conn, owners[0]).await? else {
        return Ok(false);
    };
    let plan: Plan = serde_json::from_value(owner.plan["initial_files"].clone())?;
    if plan.creator_origin.is_none()
        && owner
            .plan
            .get("initial_files_commands")
            .is_some_and(|value| !value.is_null())
    {
        return Ok(false);
    }
    let checked = async {
        let source = current_at(conn, &plan, now).await?;
        if plan.creator_origin.is_none() {
            // Current legacy callers keep their existing path. current_at rejects a modern
            // credential whose old plan lacks the fully admitted creator origin.
            return Ok::<bool, Stop>(true);
        }
        let Some((target, runtime, target_machine)) = target_at(conn, &owner, &plan).await? else {
            return Ok(false);
        };
        let Some(native) = owner
            .plan
            .get("initial_files_commands")
            .and_then(|commands| commands.get(command.to_string()))
        else {
            return Ok(false);
        };
        if native.get("id") != Some(&json!(command))
            || native.get("runtime") != Some(&json!(runtime))
            || native.get("machine") != Some(&json!(machine))
            || native.get("kind").and_then(Json::as_str) != Some(kind)
            || native.get("key") != Some(&json!(key))
            || owner.plan.pointer("/active_command/id") != Some(&json!(command))
            || owner.plan.pointer("/active_command/machine") != Some(&json!(machine))
            || owner
                .plan
                .pointer("/active_command/kind")
                .and_then(Json::as_str)
                != Some(kind)
            || owner
                .plan
                .pointer("/active_command/phase")
                .and_then(Json::as_str)
                != Some("files")
            || owner.plan.get("initial_files_done").and_then(Json::as_bool) == Some(true)
        {
            return Ok(false);
        }
        let (expected_id, expected_machine, expected_environment, expected) = match kind {
            "fs.snapshot" if plan.snapshot.is_none() && !plan.empty => (
                ids::derived("initial-files.snapshot", &[owner.id.as_bytes()]),
                source
                    .machine_id
                    .ok_or_else(|| failure("the initial source connector disappeared"))?,
                source.id,
                json!(FsSnapshot {
                    work: None,
                    environment_id: source.id,
                    job_id: owner.id,
                    selection: snapshot_selection(&plan)
                }),
            ),
            "fs.record.read" if plan.local && plan.manifest.is_some() => (
                ids::derived("initial-files.export", &[owner.id.as_bytes()]),
                source
                    .machine_id
                    .ok_or_else(|| failure("the initial source connector disappeared"))?,
                source.id,
                json!(FsRecordRead {
                    environment_id: source.id,
                    snapshot_id: plan
                        .snapshot
                        .ok_or_else(|| failure("the initial snapshot disappeared"))?,
                    path: None,
                    export: true
                }),
            ),
            "fs.materialize" if !plan.empty => (
                ids::derived("initial-files.materialize", &[owner.id.as_bytes()]),
                target_machine,
                target.id,
                json!(FsMaterialize {
                    work: None,
                    environment_id: target.id,
                    job_id: Some(owner.id),
                    manifest_blob: serde_json::from_value::<Hash>(
                        owner.plan["initial_files_manifest"].clone()
                    )
                    .map_err(EngineError::from)?,
                    target_root: target.workspace_root,
                    purpose: MaterializePurpose::Move
                }),
            ),
            _ => return Ok(false),
        };
        if command != expected_id
            || machine != expected_machine
            || SerialKey::Environment(expected_environment)
                .wire()
                .as_deref()
                != key
            || native.get("body") != Some(body)
        {
            return Ok(false);
        }
        let mut actual = body.clone();
        if matches!(kind, "fs.snapshot" | "fs.materialize") {
            let work: orch_link_contract::commands::WorkReport =
                serde_json::from_value(actual.get("work").cloned().unwrap_or(Json::Null))
                    .map_err(EngineError::from)?;
            if work.job_id != owner.id || work.phase != "files" {
                return Ok(false);
            }
            if let Some(fields) = actual.as_object_mut() {
                fields.remove("work");
            }
        }
        Ok(actual == expected)
    }
    .await;
    match checked {
        Ok(allowed) => Ok(allowed),
        Err(Stop::Failed(error)) if error.code == crate::jobs::runner::INTERNAL => Err(
            EngineError::unreadable("initial copy authority could not be checked"),
        ),
        Err(Stop::Failed(_) | Stop::Cancelled | Stop::Lost) => Ok(false),
    }
}

/// Writes the selected contents at the new workspace root, before source init and running publication.
/// Stable native command identities and the retained pin ledger survive retries and process restart.
pub async fn deliver(job: &Job, environment: Id, machine: Id) -> Result<Option<Done>, Stop> {
    let remembered = job.plan().await?;
    let Some(value) = remembered.get("initial_files") else {
        return Ok(None);
    };
    let plan: Plan = serde_json::from_value(value.clone()).map_err(EngineError::from)?;
    if remembered.get("initial_files_done").and_then(Json::as_bool) == Some(true) {
        return Ok(Some(Done::Finished));
    }
    let source = current(job, &plan).await?;
    if plan.empty {
        job.remember(&json!({"initial_files_done":true})).await?;
        return Ok(Some(Done::Finished));
    }
    let hash = if let Some(value) = remembered.get("initial_files_manifest") {
        serde_json::from_value::<Hash>(value.clone()).map_err(EngineError::from)?
    } else {
        let hash = if let Some(hash) = plan.manifest {
            if plan.local {
                let value = job
                    .command(
                        Outgoing::new(
                            ids::derived("initial-files.export", &[job.id().as_bytes()]),
                            source
                                .machine_id
                                .ok_or_else(|| failure("the source connector is unavailable"))?,
                            SerialKey::Environment(source.id),
                            &FsRecordRead {
                                environment_id: source.id,
                                snapshot_id: plan.snapshot.ok_or_else(|| {
                                    failure("the source snapshot identity is absent")
                                })?,
                                path: None,
                                export: true,
                            },
                        )
                        .map_err(EngineError::from)?,
                    )
                    .await?;
                let result: FsRecordReadResult =
                    serde_json::from_value(value).map_err(EngineError::from)?;
                if result.blob != hash {
                    return Err(failure("the exported snapshot changed its manifest"));
                }
            }
            hash
        } else {
            let selection = snapshot_selection(&plan);
            let value = job
                .command(
                    Outgoing::new(
                        ids::derived("initial-files.snapshot", &[job.id().as_bytes()]),
                        source
                            .machine_id
                            .ok_or_else(|| failure("the source connector is unavailable"))?,
                        SerialKey::Environment(source.id),
                        &FsSnapshot {
                            work: None,
                            environment_id: source.id,
                            job_id: job.id(),
                            selection,
                        },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            let result: FsSnapshotResult =
                serde_json::from_value(value).map_err(EngineError::from)?;
            if result.missing != 0 {
                return Err(failure("not every selected initial file could be read"));
            }
            result.manifest_blob
        };
        current(job, &plan).await?;
        let original = job.engine().read_manifest_checked(hash).await?;
        if original.header().generation != i64::from(plan.generation) {
            return Err(failure(
                "the initial manifest belongs to another source generation",
            ));
        }
        // A saved snapshot keeps its own namespace when the live environment moves.
        // Only a live selection is rebased against the current workspace and thread directory.
        let base = if plan.snapshot.is_none() {
            let workspace_root = crate::sessions::workspace_root(
                &source.workspace_root,
                &source.facts,
                plan.generation,
            );
            let source_root = workspace_root
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_owned();
            let root = original
                .header()
                .root
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_owned();
            let suffix = if root == source_root {
                ""
            } else {
                root.strip_prefix(&format!("{source_root}/"))
                    .ok_or_else(|| {
                        failure("the initial file manifest leaves its source workspace")
                    })?
            };
            if !relative(suffix) {
                return Err(failure("the initial file manifest has an unsafe root"));
            }
            plan.directory
                .as_deref()
                .map(|directory| crate::sessions::absolute_directory(workspace_root, directory))
        } else {
            None
        };
        let mut tx = transaction_guard::scope(false, job.engine().db().begin()).await?;
        transaction_guard::lock_action(&mut tx).await?;
        let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if let Some(reason) = crate::jobs::runner::abort_reason(&owner)? {
            return Err(reason);
        }
        target_at(&mut tx, &owner, &plan).await?;
        current_at(&mut tx, &plan, Time::new(job.now())).await?;
        let hash = if let Some(base) = base {
            crate::selection::rebase_manifest(job.engine(), &mut tx, &hash, &base).await?
        } else {
            hash
        };
        let original = job.engine().read_manifest_checked(hash).await?;
        let matcher = plan
            .selection
            .as_ref()
            .map(crate::selection::Matcher::new)
            .transpose()?;
        let selected: Vec<_> = original
            .entries()
            .iter()
            .filter(|entry| {
                matcher
                    .as_ref()
                    .is_none_or(|matcher| matcher.matches(&entry.path))
            })
            .cloned()
            .collect();
        let mut paths = HashSet::new();
        for entry in &selected {
            paths.insert(entry.path.clone());
            let mut path = entry.path.as_str();
            while let Some((parent, _)) = path.rsplit_once('/') {
                paths.insert(parent.to_owned());
                path = parent;
            }
        }
        let entries = original
            .entries()
            .iter()
            .filter(|entry| paths.contains(&entry.path))
            .cloned()
            .collect::<Vec<_>>();
        if entries.iter().any(|entry| {
            !relative(&entry.path)
                || entry.kind == EntryKind::Other
                || (entry.kind == EntryKind::File && entry.hash.is_none())
                || (entry.kind == EntryKind::Link
                    && entry
                        .link
                        .as_deref()
                        .is_none_or(|target| !safe_link(&entry.path, target)))
        }) {
            return Err(failure(
                "the initial selection contains incomplete or unsafe file references",
            ));
        }
        let written =
            manifest::build(original.header(), entries.clone()).map_err(EngineError::unreadable)?;
        let kept = job.engine().put_manifest(&mut tx, written.bytes).await?;
        let mut hashes = entries
            .iter()
            .filter_map(|entry| entry.hash)
            .collect::<BTreeSet<_>>();
        hashes.insert(kept.hash);
        let hashes: Vec<_> = hashes.into_iter().collect();
        let store = job
            .engine()
            .blobs()
            .ok_or_else(|| failure("the initial file store is unavailable"))?;
        for hash in &hashes {
            let size = store
                .size(hash)
                .await
                .map_err(EngineError::unreadable)?
                .ok_or_else(|| failure("an initial file's contents are no longer available"))?;
            storage::register_blob(&mut tx, hash, size, job.now()).await?;
        }
        let row = jobs::lock(&mut tx, job.id())
            .await?
            .ok_or_else(|| failure("the initial file job disappeared"))?;
        if !row
            .plan
            .get("initial_files_pins")
            .is_some_and(Json::is_array)
        {
            storage::pin_blobs(&mut tx, &hashes).await?;
            jobs::merge_plan(
                &mut tx,
                job.id(),
                &json!({"initial_files_manifest":kept.hash,"initial_files_pins":hashes}),
                job.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        kept.hash
    };
    current(job, &plan).await?;
    let target = {
        let mut conn = job.engine().db().acquire().await?;
        environments::environment(&mut conn, environment)
            .await?
            .ok_or_else(|| failure("the new environment disappeared"))?
    };
    let manifest = job.engine().read_manifest_checked(hash).await?;
    let files = i64::try_from(
        manifest
            .entries()
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .count(),
    )
    .unwrap_or(i64::MAX);
    let value = job
        .command(
            Outgoing::new(
                ids::derived("initial-files.materialize", &[job.id().as_bytes()]),
                machine,
                SerialKey::Environment(environment),
                &FsMaterialize {
                    work: None,
                    environment_id: environment,
                    job_id: Some(job.id()),
                    manifest_blob: hash,
                    target_root: target.workspace_root,
                    purpose: MaterializePurpose::Move,
                },
            )
            .map_err(EngineError::from)?,
        )
        .await?;
    let applied: FsMaterializeResult = serde_json::from_value(value).map_err(EngineError::from)?;
    if applied.files.saturating_add(applied.skipped) < files {
        return Err(failure(
            "the new environment did not receive every selected file",
        ));
    }
    job.remember(&json!({"initial_files_done":true,"initial_files_result":applied}))
        .await?;
    Ok(Some(Done::Finished))
}

/// Releases this job's individual temporary pins once, on commit or rollback of its fresh unit.
pub(crate) async fn release(conn: &mut Conn, job: Id, now: Time) -> Result<(), EngineError> {
    let Some(row) = jobs::lock(conn, job).await? else {
        return Ok(());
    };
    let Some(value) = row
        .plan
        .get("initial_files_pins")
        .filter(|value| value.is_array())
    else {
        return Ok(());
    };
    let hashes: Vec<Hash> = serde_json::from_value(value.clone())?;
    storage::unpin_blobs(conn, &hashes, now.get()).await?;
    jobs::merge_plan(conn, job, &json!({"initial_files_pins":null}), now.get()).await?;
    Ok(())
}

/// Live first-send identity/time export around the unchanged immutable copy validator.
pub async fn command_current_callers(
    conn: &mut Conn,
    command: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    clock: &dyn orch_types::Clock,
) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
    if !command_authorized(conn, command, machine, key, kind, body, clock.time()).await? {
        return Ok(None);
    }
    let owners = jobs::lock_initial_files_command_owners(conn, command).await?;
    if owners.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if owners.len() != 1 {
        return Ok(None);
    }
    let Some(owner) = jobs::job(conn, owners[0]).await? else {
        return Ok(None);
    };
    let plan: Plan = serde_json::from_value(owner.plan["initial_files"].clone())?;
    let caller = if let Some(origin) = &plan.creator_origin {
        let Some(caller) = origin.restore_current(conn, clock).await? else {
            return Ok(None);
        };
        caller
    } else {
        let principal = match crate::jobs::source::load_principal_at(
            conn,
            plan.caller,
            plan.token,
            clock.time(),
        )
        .await
        {
            Ok(principal) => principal,
            Err(Stop::Failed(failure)) if failure.code == crate::jobs::runner::INTERNAL => {
                return Err(EngineError::unreadable(
                    "initial copy authority could not be checked",
                ));
            }
            Err(_) => return Ok(None),
        };
        let Some(caller) = orch_authz::CapturedCaller::capture(principal.caller())
            .restore_current(conn, clock)
            .await?
        else {
            return Ok(None);
        };
        caller
    };
    let deadline = if let Some(snapshot) = plan.snapshot {
        let Some(row) = storage::snapshot(conn, snapshot).await? else {
            return Ok(None);
        };
        if row.expires_at.is_some_and(|end| end <= clock.now()) {
            return Ok(None);
        }
        row.expires_at.map(Time::new)
    } else {
        None
    };
    Ok(Some(vec![(Some(caller), deadline)]))
}
