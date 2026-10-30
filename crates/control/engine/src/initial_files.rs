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
use orch_authz::{Act, Policy, Principal};
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

/// Authorizes the declared source, normalizes only its selection, and freezes no arbitrary blob claim.
pub async fn plan(
    conn: &mut Conn,
    principal: &Principal,
    requested: &NewEnvironmentFiles,
) -> Result<Plan, Error> {
    let caller = principal.user();
    let token = principal.caller().grant().and_then(|grant| grant.token());
    match requested {
        NewEnvironmentFiles::FromThread {
            from_thread,
            selection,
        } => {
            let thread = threads::thread(conn, *from_thread)
                .await?
                .ok_or(Error::NotFound)?;
            let environment = thread.environment_id.ok_or(Error::NotFound)?;
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
            })
        }
        NewEnvironmentFiles::Snapshot { snapshot_id } => {
            let row = storage::snapshot(conn, *snapshot_id)
                .await?
                .ok_or(Error::NotFound)?;
            let thread=row.thread_id.ok_or_else(||Error::Invalid("a workspace export is for download; choose a thread snapshot to copy initial files".into()))?;
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
            })
        }
    }
}

fn failure(message: impl Into<String>) -> Stop {
    Failure::new("initial_files_unavailable", message).into()
}

async fn current(job: &Job, plan: &Plan) -> Result<environments::EnvironmentRow, Stop> {
    let mut tx = transaction_guard::scope(false, job.engine().db().begin()).await?;
    transaction_guard::lock_action(&mut tx).await?;
    let principal =
        crate::jobs::source::load_principal(&mut tx, job, plan.caller, plan.token).await?;
    rights(&mut tx, &principal, plan.thread, plan.environment)
        .await
        .map_err(|error| failure(error.to_string()))?;
    let source = environments::environment(&mut tx, plan.environment)
        .await?
        .ok_or_else(|| failure("the initial file source disappeared"))?;
    if let Some(snapshot) = plan.snapshot {
        let row = storage::snapshot(&mut tx, snapshot)
            .await?
            .filter(|row| {
                row.thread_id == Some(plan.thread)
                    && row.environment_id == plan.environment
                    && row.state == "complete"
                    && row.missing_count == 0
                    && row.manifest_blob == plan.manifest
                    && !row.expires_at.is_some_and(|at| at <= job.now())
            })
            .ok_or_else(|| {
                failure("the initial file snapshot changed or is no longer available")
            })?;
        if (row.storage == "environment") != plan.local {
            return Err(failure("the initial file snapshot changed its storage"));
        }
    } else {
        let thread = threads::thread(&mut tx, plan.thread)
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
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(source)
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
            let selection = plan.selection.as_ref().map_or_else(
                || Selection {
                    include: vec![directory.to_owned()],
                    exclude: vec![],
                },
                |selection| Selection {
                    include: selection.include.iter().map(|path| below(path)).collect(),
                    exclude: selection.exclude.iter().map(|path| below(path)).collect(),
                },
            );
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
