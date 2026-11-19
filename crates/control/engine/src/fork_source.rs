//! Exact saved file points and own-worktree choices in fork plans and jobs.

use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::forks::{ForkPlanWorktree, ForkRequest, PlanBlocker};
use orch_api_contract::source::WorktreeFrom;
use orch_api_contract::workspace::Snapshot;
use orch_authz::{Policy, Principal};
use orch_db::queries::environments::thread_engine::EnvironmentRow;
use orch_db::{
    Conn,
    queries::{storage, threads},
};
use orch_link_contract::commands::{FsStat, GitStatus, SerialKey};
use orch_types::{Time, new_id};
use serde_json::json;

use crate::{
    Engine, EngineError,
    commands::{Outcome, Outgoing},
    fork_plan::PlanError,
};

/// Source choices are independent of the conversation's history choice.
#[derive(Default)]
pub(crate) struct Planned {
    pub worktree: Option<ForkPlanWorktree>,
    pub files_at: Option<Snapshot>,
    pub blockers: Vec<PlanBlocker>,
}

impl Planned {
    fn blocked(code: &str, message: &str) -> Self {
        Self {
            blockers: vec![PlanBlocker {
                code: code.into(),
                message: message.into(),
                missing: None,
            }],
            ..Self::default()
        }
    }
}

fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn writable(conn: &mut Conn, environment: orch_types::Id) -> Result<bool, EngineError> {
    Ok(
        orch_db::queries::environments::workspace::environment(conn, environment)
            .await?
            .is_some_and(|row| row.remote_write),
    )
}

/// Includes the private repository's bytes in the same reviewed copy plan as its working files.
pub(crate) async fn independent_plan(
    engine: &Engine,
    environment: &EnvironmentRow,
    directory: &str,
    selection: orch_link_contract::commands::Selection,
) -> Result<orch_link_contract::source::GitWorktreeCopyPlanResult, EngineError> {
    let machine = environment
        .machine_id
        .ok_or_else(|| EngineError::unreadable("the source worktree has no connector"))?;
    let command = Outgoing::new(
        new_id(),
        machine,
        SerialKey::Environment(environment.id),
        &orch_link_contract::source::GitWorktreeCopyPlan {
            environment_id: environment.id,
            directory: directory.to_owned(),
            selection,
        },
    )
    .map_err(EngineError::from)?;
    let result = tokio::time::timeout(
        engine.config().hosting.attach_wait,
        crate::commands::run(engine.inner.commands.as_ref(), command),
    )
    .await
    .map_err(|_| EngineError::unreadable("the source worktree's repository measurement timed out"))?
    .map_err(EngineError::from)?;
    match result {
        Outcome::Succeeded(value) => serde_json::from_value(value).map_err(EngineError::from),
        _ => Err(EngineError::unreadable(
            "the source worktree could not provide a complete independent repository",
        )),
    }
}

/// A slow source copy repeats its current token, file reach, placement and target-write checks.
pub(crate) async fn authorize_copy(
    job: &crate::jobs::runner::Job,
    recipe: &crate::jobs::fork::Recipe,
    source: &EnvironmentRow,
    target: &EnvironmentRow,
    directory: &str,
) -> Result<(), crate::jobs::runner::Stop> {
    use crate::jobs::runner::Failure;
    let plan = job.plan().await?;
    let token = plan
        .get("file_caller_token")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(EngineError::from)?
        .flatten();
    let mut conn = job.engine().db().acquire().await?;
    let principal = crate::jobs::source::load_principal(&mut conn, job, recipe.by, token).await?;
    for (id, permission) in [
        (source.id, Permission::FilesView),
        (target.id, Permission::EnvironmentManage),
    ] {
        if !matches!(
            orch_authz::resources::check(
                &mut conn,
                &principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id
                },
                permission,
                Policy::default()
            )
            .await
            .map_err(EngineError::from)?,
            Decision::Allowed { .. }
        ) {
            return Err(Failure::new(
                "access.denied",
                "current permission no longer permits this file copy",
            )
            .into());
        }
    }
    for expected in [source, target] {
        if !orch_db::queries::environments::thread_engine::environment(&mut conn, expected.id)
            .await?
            .is_some_and(|current| {
                current.generation == expected.generation
                    && current.machine_id == expected.machine_id
                    && current.workspace_root == expected.workspace_root
            })
        {
            return Err(Failure::new(
                "conflict.stale_preflight",
                "the copy's environment changed during transfer",
            )
            .into());
        }
    }
    if !writable(&mut conn, target.id).await? {
        return Err(Failure::new(
            "workspace.remote_write_disabled",
            "the destination accepts no remote file writes",
        )
        .into());
    }
    if !threads::thread(&mut conn, recipe.source_thread)
        .await?
        .is_some_and(|thread| {
            thread.environment_id == Some(source.id) && thread.workspace_dir == directory
        })
    {
        return Err(Failure::new(
            "conflict.stale_preflight",
            "the thread's source directory changed during transfer",
        )
        .into());
    }
    Ok(())
}

async fn pin_export(
    job: &crate::jobs::runner::Job,
    manifest: orch_types::Hash,
) -> Result<(), crate::jobs::runner::Stop> {
    let parsed = job.engine().read_manifest_checked(manifest).await?;
    let store = job.engine().blobs().ok_or_else(|| {
        crate::jobs::runner::Failure::new("internal", "the saved-file content store is unavailable")
    })?;
    let mut hashes = parsed.hashes().collect::<Vec<_>>();
    hashes.push(manifest);
    hashes.sort();
    hashes.dedup();
    let mut sizes = Vec::new();
    for hash in &hashes {
        let size = store
            .size(hash)
            .await
            .map_err(EngineError::unreadable)?
            .ok_or_else(|| {
                crate::jobs::runner::Failure::new(
                    "workspace.restore_unavailable",
                    "an exported saved-file object is missing",
                )
            })?;
        sizes.push((*hash, size));
    }
    let mut tx = job.engine().db().begin().await?;
    let row = orch_db::queries::jobs::lock(&mut tx, job.id())
        .await?
        .ok_or_else(|| {
            crate::jobs::runner::Failure::new(
                "not_found",
                "the fork disappeared during its saved-file transfer",
            )
        })?;
    if row.plan.get("fork_file_pins").is_none() {
        for (hash, size) in sizes {
            storage::register_blob(&mut tx, &hash, size, job.now()).await?;
        }
        storage::pin_blobs(&mut tx, &hashes).await?;
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({ "fork_file_pins": hashes }),
            job.now(),
        )
        .await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Releases exactly this fork's exported object pins after its native copy or terminal outcome.
pub(crate) async fn release_pins(
    engine: &Engine,
    id: orch_types::Id,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let mut tx = engine.db().begin().await?;
    let Some(row) = orch_db::queries::jobs::lock(&mut tx, id).await? else {
        return Ok(());
    };
    if row
        .plan
        .get("fork_file_pins_released")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return Ok(());
    }
    if let Some(pins) = row.plan.get("fork_file_pins") {
        let pins: Vec<orch_types::Hash> = serde_json::from_value(pins.clone())?;
        storage::unpin_blobs(&mut tx, &pins, now).await?;
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            id,
            &json!({ "fork_file_pins_released": true }),
            now,
        )
        .await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Resolves only an explicitly chosen source act; ordinary forks make no extra native call.
#[allow(
    clippy::too_many_arguments,
    reason = "a source choice is bound to its caller, thread, environment and target directory"
)]
pub(crate) async fn plan(
    engine: &Engine,
    conn: &mut Conn,
    principal: &Principal,
    request: &ForkRequest,
    thread: &threads::ThreadRow,
    environment: &EnvironmentRow,
    directory: &str,
    target: Option<&EnvironmentRow>,
) -> Result<Planned, PlanError> {
    if request.worktree.is_none() && request.files_at.is_none() {
        return Ok(Planned::default());
    }
    if directory.is_empty()
        || target
            .is_some_and(|target| target.id == environment.id && directory == thread.workspace_dir)
    {
        return Err(PlanError::invalid(
            "directory",
            "an own attempt or saved point uses another unused directory",
        ));
    }
    if request.worktree.is_some() && !writable(conn, environment.id).await? {
        return Ok(Planned::blocked(
            "workspace.remote_write_disabled",
            "the environment permits no remote file writes",
        ));
    }
    let Some(machine) = environment
        .machine_id
        .filter(|_| environment.status == "running")
    else {
        return Ok(Planned::blocked(
            "workspace.source_failed",
            "connect the environment to review its source and unused directory",
        ));
    };
    // Existing directories, including empty ones, are never accepted for exact materialization.
    if let Some(target) = target {
        if !writable(conn, target.id).await? {
            return Ok(Planned::blocked(
                "workspace.remote_write_disabled",
                "the destination permits no remote file writes",
            ));
        }
        let Some(target_machine) = target.machine_id.filter(|_| target.status == "running") else {
            return Ok(Planned::blocked(
                "workspace.source_failed",
                "connect the destination to verify its unused directory",
            ));
        };
        let command = Outgoing::new(
            new_id(),
            target_machine,
            SerialKey::Environment(target.id),
            &FsStat {
                environment_id: target.id,
                mount: None,
                path: directory.into(),
            },
        )
        .map_err(EngineError::from)?;
        match engine.ask_bounded(command).await? {
            Some(Outcome::Failed(error)) if error.code == "not_found" => {}
            Some(Outcome::Succeeded(_)) => {
                return Ok(Planned::blocked(
                    "workspace.source_conflict",
                    "the target directory already exists",
                ));
            }
            _ => {
                return Ok(Planned::blocked(
                    "workspace.source_failed",
                    "the unused target directory could not be verified",
                ));
            }
        }
    }
    let mut planned = Planned::default();
    if let Some(point) = &request.files_at {
        let row = storage::snapshot(conn, point.snapshot_id)
            .await?
            .filter(|row| row.thread_id == Some(thread.node_id))
            .ok_or(PlanError::NotFound)?;
        let access = orch_authz::resources::check(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: row.environment_id,
            },
            Permission::FilesView,
            Policy::default(),
        )
        .await?;
        if !matches!(access, Decision::Allowed { .. }) {
            return Ok(Planned::blocked(
                "access",
                "Files View on the recorded state's owning environment is required",
            ));
        }
        if row.storage != "environment"
            || row.state != "complete"
            || row.missing_count != 0
            || row
                .expires_at
                .is_some_and(|at| at <= engine.inner.clock.now())
            || row.manifest_blob.is_none()
            || row.directory.is_none()
        {
            return Ok(Planned::blocked(
                "workspace.restore_unavailable",
                "the chosen file point is incomplete, expired or unavailable",
            ));
        }
        if request.worktree.is_some() && row.environment_id != environment.id {
            return Ok(Planned::blocked(
                "workspace.restore_unavailable",
                "the chosen local worktree files belong to another environment",
            ));
        }
        let owning =
            orch_db::queries::environments::thread_engine::environment(conn, row.environment_id)
                .await?
                .ok_or(PlanError::NotFound)?;
        let hash = row
            .manifest_blob
            .ok_or_else(|| EngineError::unreadable("the complete saved state has no manifest"))?;
        let manifest = engine.read_manifest_checked(hash).await?;
        if let Some(turn) = request.history.as_ref().and_then(|history| history.to_turn) {
            // A history ending after an agent answer needs exactly its saved end, rather than
            // the before-state of that turn or the files that happen to be there today.
            let end: Option<(Option<orch_types::Json>, orch_types::Id, i32, Option<String>)> = sqlx::query_as(
                "SELECT t.files_end_manifest, sg.environment_id, sg.env_generation, s.directory FROM turns t
                 JOIN segments sg ON sg.id = t.segment_id LEFT JOIN snapshots s ON s.id = t.before_snapshot_id
                 WHERE t.thread_id = $1 AND t.ordinal = $2 AND t.completed_at IS NOT NULL")
                .bind(thread.node_id).bind(turn).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
            if !end.is_some_and(|(end, environment, generation, directory)| {
                end.as_ref() == Some(&crate::source::end_namespace(&manifest))
                    && environment == row.environment_id
                    && generation == row.env_generation
                    && directory == row.directory
            }) {
                return Ok(Planned::blocked(
                    "workspace.restore_unavailable",
                    "no complete saved state equals the selected answer's end observation",
                ));
            }
        }
        if manifest.header().generation != i64::from(row.env_generation) {
            return Ok(Planned::blocked(
                "workspace.restore_unavailable",
                "the saved file point no longer has its original coverage",
            ));
        }
        planned.files_at = Some(serde_json::from_value(
            json!({ "id": row.id, "thread_id": row.thread_id,
            "environment": crate::views::environment_ref_json(conn, &owning, Time::new(engine.inner.clock.now())).await?,
            "label": row.label, "kind": row.kind, "storage": row.storage, "repository_head": row.repository_head,
            "directory": row.directory, "expires_at": row.expires_at.map(Time::new), "unavailable_reason": row.unavailable_reason,
            "state": row.state, "selection": row.selection, "file_count": row.file_count, "total_bytes": row.total_bytes,
            "missing_count": row.missing_count, "env_generation": row.env_generation,
            "created_by": match row.created_by { Some(user) => Some(crate::views::user_ref_json(conn, user).await?), None => None },
            "created_at": Time::new(row.created_at), "completed_at": row.completed_at.map(Time::new) }),
        )?);
    }
    if let Some(choice) = &request.worktree {
        let command = Outgoing::new(
            new_id(),
            machine,
            SerialKey::Environment(environment.id),
            &GitStatus {
                environment_id: environment.id,
                path: Some(choice.repository_dir.clone()),
                watch_id: None,
            },
        )
        .map_err(EngineError::from)?;
        let Some(Outcome::Succeeded(value)) = engine.ask_bounded(command).await? else {
            return Ok(Planned::blocked(
                "git.unavailable",
                "the worktree's repository could not be reviewed",
            ));
        };
        let status: orch_api_contract::workspace::GitStatus = serde_json::from_value(value)?;
        if !status.available
            || status.repository_dir.as_deref() != Some(choice.repository_dir.as_str())
        {
            return Ok(Planned::blocked(
                "git.unavailable",
                "choose the repository directory shown by Git status",
            ));
        }
        let (base_commit, changes_carried) = match &choice.from {
            WorktreeFrom::Commit { commit } if oid(commit) => (commit.clone(), false),
            WorktreeFrom::Commit { .. } => {
                return Err(PlanError::invalid(
                    "worktree.from.commit",
                    "choose a resolved Git commit hash",
                ));
            }
            WorktreeFrom::Now { .. } => {
                let head = planned
                    .files_at
                    .as_ref()
                    .and_then(|snapshot| snapshot.repository_head.clone())
                    .or(status.head.clone())
                    .filter(|head| oid(head))
                    .ok_or_else(|| {
                        PlanError::invalid(
                            "worktree.from",
                            "a complete current state needs a known base commit",
                        )
                    })?;
                if planned
                    .files_at
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.repository_head.is_none())
                {
                    return Ok(Planned::blocked(
                        "workspace.restore_unavailable",
                        "the chosen saved state has no recorded repository HEAD",
                    ));
                }
                (
                    head,
                    planned.files_at.is_some() || !status.changes.is_empty(),
                )
            }
        };
        if status.branch.as_deref() == Some(choice.branch.as_str()) {
            return Ok(Planned::blocked(
                "workspace.source_conflict",
                "the requested worktree branch already exists",
            ));
        }
        planned.worktree = Some(ForkPlanWorktree {
            choice: choice.clone(),
            base_commit,
            changes_carried,
        });
    }
    Ok(planned)
}

/// File choices finish before the fork opens a session; every read is checked on its owning environment.
pub(crate) async fn copy(
    job: &crate::jobs::runner::Job,
    recipe: &crate::jobs::fork::Recipe,
) -> Result<crate::jobs::runner::Done, crate::jobs::runner::Stop> {
    use crate::jobs::runner::{Done, Failure};
    use orch_db::queries::{environments::thread_engine as environments, source};
    use orch_link_contract::commands::{FsMaterialize, FsMaterializeResult, MaterializePurpose};
    use orch_link_contract::source::{
        FsRecord, FsRecordRead, FsRecordReadResult, FsRecordResult, GitWorktree, RecordKind,
        RecordState,
    };
    use orch_settings::{Place, get};
    use orch_settings_contract::registry::keys;
    let fail = |message: &str| Failure::new("workspace.restore_unavailable", message);
    let plan = job.plan().await?;
    if plan
        .get("fork_source_complete")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        release_pins(job.engine(), job.id(), job.now()).await?;
        return Ok(Done::Finished);
    }
    let target_id = recipe
        .to_environment
        .or_else(|| {
            plan.get("new_environment")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
        })
        .ok_or_else(|| fail("the fork's destination environment is unavailable"))?;
    let selected = recipe.forkplan.files_at.as_ref();
    let source_id = selected.map_or(recipe.from_environment, |snapshot| snapshot.environment.id);
    let mut conn = job.engine().db().acquire().await?;
    let source_env = environments::environment(&mut conn, source_id)
        .await?
        .ok_or_else(|| fail("the source environment disappeared"))?;
    if !writable(&mut conn, target_id).await? {
        return Err(fail("the destination accepts no remote file writes").into());
    }
    let target = environments::environment(&mut conn, target_id)
        .await?
        .ok_or_else(|| fail("the destination accepts no remote file writes"))?;
    let source_machine = source_env
        .machine_id
        .ok_or_else(|| fail("the saved state has no source connector"))?;
    let target_machine = target
        .machine_id
        .ok_or_else(|| fail("the target has no connector"))?;
    let token = plan
        .get("file_caller_token")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(EngineError::from)?
        .flatten();
    let principal = crate::jobs::source::load_principal(&mut conn, job, recipe.by, token).await?;
    for (environment, permission) in [
        (target_id, Permission::EnvironmentManage),
        (source_id, Permission::FilesView),
    ] {
        if !matches!(
            orch_authz::resources::check(
                &mut conn,
                &principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: environment
                },
                permission,
                Policy::default()
            )
            .await
            .map_err(EngineError::from)?,
            Decision::Allowed { .. }
        ) {
            return Err(Failure::new(
                "access.denied",
                "current permission no longer permits the fork's files",
            )
            .into());
        }
    }
    let snapshot = if let Some(saved) = selected {
        let current = storage::snapshot(&mut conn, saved.id)
            .await?
            .filter(|row| {
                row.state == "complete"
                    && row.environment_id == source_id
                    && row.expires_at.is_none_or(|at| at > job.now())
            })
            .ok_or_else(|| fail("the selected exact file point expired or changed"))?;
        Some((
            current.id,
            current
                .manifest_blob
                .ok_or_else(|| fail("the selected exact file manifest is unavailable"))?,
        ))
    } else {
        None
    };
    let record_days = get::<i64>(
        &mut conn,
        Place::Environment(recipe.from_environment),
        keys::WORKSPACE_RECORD_DAYS,
    )
    .await
    .map_err(EngineError::from)?
    .ok_or_else(|| fail("recording days are unset"))?;
    let max_bytes = get::<i64>(
        &mut conn,
        Place::Environment(recipe.from_environment),
        keys::WORKSPACE_RECORD_MAX_BYTES,
    )
    .await
    .map_err(EngineError::from)?
    .ok_or_else(|| fail("recording bytes are unset"))?;
    drop(conn);
    if let Some(worktree) = &recipe.forkplan.worktree {
        if target_id != recipe.from_environment {
            return Err(fail("an own worktree must stay in its repository's environment").into());
        }
        let snapshot_id = if let Some((snapshot, _)) = snapshot {
            Some(snapshot)
        } else if matches!(worktree.choice.from, WorktreeFrom::Now { .. }) {
            let snapshot =
                crate::ids::derived("fork.worktree.current-files", &[job.id().as_bytes()]);
            job.remember(&json!({ "fork_source_snapshot": snapshot }))
                .await?;
            let mut tx = job.engine().db().begin().await?;
            source::begin_recorded(&mut tx, &storage::NewSnapshot { id: snapshot, thread_id: Some(recipe.source_thread),
                environment_id: recipe.from_environment, env_generation: source_env.generation, label: "Current files for own attempt",
                selection: &json!({ "include": [worktree.choice.repository_dir], "exclude": [] }), created_by: Some(recipe.by), at: job.now() },
                &worktree.choice.repository_dir, "turn").await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            let value = job
                .command(
                    Outgoing::new(
                        crate::ids::derived("fork.worktree.record", &[job.id().as_bytes()]),
                        source_machine,
                        SerialKey::Environment(recipe.from_environment),
                        &FsRecord {
                            environment_id: recipe.from_environment,
                            snapshot_id: snapshot,
                            directory: worktree.choice.repository_dir.clone(),
                            kind: RecordKind::Turn,
                            record_days,
                            max_bytes,
                        },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            let recorded: FsRecordResult =
                serde_json::from_value(value).map_err(EngineError::from)?;
            if recorded.state != RecordState::Complete
                || recorded.repository_head.as_deref() != Some(&worktree.base_commit)
            {
                return Err(fail("From now could not retain a complete state with the reviewed base commit; make a new plan").into());
            }
            let hash = recorded
                .manifest_blob
                .ok_or_else(|| fail("the complete current file manifest is missing"))?;
            let store = job
                .engine()
                .blobs()
                .ok_or_else(|| fail("the manifest store is unavailable"))?;
            job.engine().read_manifest_checked(hash).await?;
            let size = store
                .size(&hash)
                .await
                .map_err(EngineError::unreadable)?
                .ok_or_else(|| fail("the complete current file manifest is missing"))?;
            let mut tx = job.engine().db().begin().await?;
            storage::register_blob(&mut tx, &hash, size, job.now()).await?;
            if source::finish_recorded(&mut tx, snapshot, &recorded, job.now()).await? {
                storage::pin_blobs(&mut tx, &[hash]).await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Some(snapshot)
        } else {
            None
        };
        let body = GitWorktree {
            environment_id: target_id,
            job_id: job.id(),
            choice: worktree.choice.clone(),
            snapshot_id,
        };
        job.remember(&json!({ "fork_worktree_original": body, "fork_worktree_machine": target_machine, "fork_worktree_generation": target.generation })).await?;
        let value = job
            .command(
                Outgoing::new(
                    crate::ids::derived("fork.git.worktree", &[job.id().as_bytes()]),
                    target_machine,
                    SerialKey::Environment(target_id),
                    &body,
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        let info: orch_api_contract::source::SourceInfo =
            serde_json::from_value(value).map_err(EngineError::from)?;
        if info.head.as_deref() != Some(&worktree.base_commit) {
            return Err(fail("the worktree did not retain its reviewed base commit").into());
        }
        job.remember(&json!({ "fork_source_complete": true, "fork_source_info": info }))
            .await?;
        return Ok(Done::Finished);
    }
    let (snapshot, hash) = snapshot.ok_or_else(|| fail("the exact file choice has no snapshot"))?;
    let value = job
        .command(
            Outgoing::new(
                crate::ids::derived("fork.files-at.export", &[job.id().as_bytes()]),
                source_machine,
                SerialKey::Environment(source_id),
                &FsRecordRead {
                    environment_id: source_id,
                    snapshot_id: snapshot,
                    path: None,
                    export: true,
                },
            )
            .map_err(EngineError::from)?,
        )
        .await?;
    let exported: FsRecordReadResult = serde_json::from_value(value).map_err(EngineError::from)?;
    if exported.blob != hash {
        return Err(fail("the saved point changed its original manifest").into());
    }
    pin_export(job, hash).await?;
    // Recheck after a slow transfer before any destination bytes are written.
    let mut conn = job.engine().db().acquire().await?;
    let principal = crate::jobs::source::load_principal(&mut conn, job, recipe.by, token).await?;
    for (environment, permission) in [
        (source_id, Permission::FilesView),
        (target_id, Permission::EnvironmentManage),
    ] {
        if !matches!(
            orch_authz::resources::check(
                &mut conn,
                &principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: environment
                },
                permission,
                Policy::default()
            )
            .await
            .map_err(EngineError::from)?,
            Decision::Allowed { .. }
        ) {
            return Err(Failure::new(
                "access.denied",
                "file permission was withdrawn during the saved-point transfer",
            )
            .into());
        }
    }
    if !environments::environment(&mut conn, target_id)
        .await?
        .is_some_and(|current| {
            current.generation == target.generation && current.machine_id == Some(target_machine)
        })
        || !writable(&mut conn, target_id).await?
    {
        return Err(Failure::new(
            "conflict.stale_preflight",
            "the saved-point destination changed during transfer",
        )
        .into());
    }
    drop(conn);
    let target_root =
        crate::sessions::absolute_directory(&target.workspace_root, &recipe.forkplan.to.directory);
    let original = FsMaterialize {
        work: None,
        environment_id: target_id,
        job_id: Some(job.id()),
        manifest_blob: hash,
        target_root,
        purpose: MaterializePurpose::Exact,
    };
    // Retain the attempt before dispatch: a lost answer can still have published its directory.
    job.remember(&json!({ "fork_exact_original": original, "fork_exact_machine": target_machine, "fork_exact_generation": target.generation })).await?;
    let value = job
        .command(
            Outgoing::new(
                crate::ids::derived("fork.files-at.materialize", &[job.id().as_bytes()]),
                target_machine,
                SerialKey::Environment(target_id),
                &original,
            )
            .map_err(EngineError::from)?,
        )
        .await?;
    let materialized: FsMaterializeResult =
        serde_json::from_value(value).map_err(EngineError::from)?;
    if selected.is_some_and(|snapshot| {
        materialized.files.saturating_add(materialized.skipped) < snapshot.file_count
    }) {
        return Err(fail("the target did not receive the complete saved point").into());
    }
    job.remember(&json!({ "fork_source_complete": true, "files_at_result": materialized }))
        .await?;
    release_pins(job.engine(), job.id(), job.now()).await?;
    Ok(Done::Finished)
}

/// Failure of a fork cleans only the unchanged worktree made by its original native request.
pub(crate) async fn cleanup_worktree(
    job: &crate::jobs::runner::Job,
) -> Result<Option<orch_link_contract::source::GitWorktreeCleanupResult>, crate::jobs::runner::Stop>
{
    let plan = job.plan().await?;
    let Some(original) = plan.get("fork_worktree_original") else {
        return Ok(None);
    };
    let original: orch_link_contract::source::GitWorktree =
        serde_json::from_value(original.clone()).map_err(EngineError::from)?;
    let machine: orch_types::Id =
        serde_json::from_value(plan["fork_worktree_machine"].clone()).map_err(EngineError::from)?;
    let generation: i32 = serde_json::from_value(plan["fork_worktree_generation"].clone())
        .map_err(EngineError::from)?;
    let current = {
        let mut conn = job.engine().db().acquire().await?;
        orch_db::queries::environments::thread_engine::environment(
            &mut conn,
            original.environment_id,
        )
        .await?
    };
    if current.is_none_or(|environment| {
        environment.machine_id != Some(machine) || environment.generation != generation
    }) {
        return Err(crate::jobs::runner::Failure::new(
            "conflict.stale_preflight",
            "the fork's original worktree unit is unavailable for cleanup",
        )
        .into());
    }
    let value = job
        .command_to_undo(
            Outgoing::new(
                crate::ids::derived("fork.worktree.cleanup", &[job.id().as_bytes()]),
                machine,
                SerialKey::Environment(original.environment_id),
                &orch_link_contract::source::GitWorktreeCleanup { original },
            )
            .map_err(EngineError::from)?,
        )
        .await?;
    let result: orch_link_contract::source::GitWorktreeCleanupResult =
        serde_json::from_value(value).map_err(EngineError::from)?;
    job.remember(&json!({ "fork_worktree_cleanup": result }))
        .await?;
    Ok(Some(result))
}
