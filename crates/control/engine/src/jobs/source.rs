//! Durable source preparation and Git publication jobs.

use async_trait::async_trait;
use orch_access_contract::decisions::Decision;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::nodes::{JobRef, JobState};
use orch_api_contract::source::{
    RepositorySource, SourceDestination, SourceInfo, SourceKind, SourceState,
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal, TokenGrant};
use orch_db::queries::jobs::{JobRow, NewJob, kind};
use orch_db::{
    Conn,
    queries::{environments, jobs as rows, source, storage, threads},
};
use orch_link_contract::commands::{FsMkdir, FsStat, Secret, SerialKey};
use orch_link_contract::source::{
    FsRecord, FsRecordResult, GitCommit, GitPrepare, GitPush, GitWorktree, RecordKind, RecordState,
};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::runner::{Done, Failure, Job, MakeError, Phase, Phased, Stop, Undone, initial_progress};
use crate::{Engine, EngineError, commands::Outgoing, unit::Unit};

pub mod operation;
pub mod publication_authority;

/// The ordering is shared by the API and automatic first-input preparation.
pub const PHASES: [Phase; 4] = [
    Phase {
        name: "validate",
        weight: 5,
    },
    Phase {
        name: "apply",
        weight: 85,
    },
    Phase {
        name: "verify",
        weight: 5,
    },
    Phase {
        name: "commit",
        weight: 5,
    },
];

/// Safe progress never contains URLs, file names, credentials, logs or errors.
#[must_use]
pub fn reference(row: &JobRow) -> JobRef {
    let (progress, _) = super::runner::read_progress(row);
    JobRef {
        id: row.id,
        kind: row.kind.clone(),
        state: match row.state.as_str() {
            "queued" => JobState::Queued,
            "running" => JobState::Running,
            "succeeded" => JobState::Succeeded,
            "cancelled" => JobState::Cancelled,
            _ => JobState::Failed,
        },
        phase: progress.phase,
        percent: progress.percent,
    }
}

/// Makes a new-request link for known forges from a safe remote; it calls no forge service.
#[must_use]
pub fn pull_request_url(remote: &str, branch: &str, base: Option<&str>) -> Option<String> {
    if !orch_settings_contract::registry::is_git_branch(branch) {
        return None;
    }
    let remote = if remote.starts_with("git@") {
        let (host, path) = remote.strip_prefix("git@")?.split_once(':')?;
        format!("https://{host}/{path}")
    } else {
        remote.to_owned()
    };
    let remote = url::Url::parse(&remote).ok()?;
    if !matches!(remote.scheme(), "https" | "ssh")
        || remote.password().is_some()
        || remote.query().is_some()
        || remote.fragment().is_some()
        || remote.port().is_some()
        || (!remote.username().is_empty() && remote.username() != "git")
    {
        return None;
    }
    let host = remote.host_str()?;
    if !matches!(host, "github.com" | "gitlab.com") {
        return None;
    }
    let path = remote
        .path()
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(remote.path().trim_end_matches('/'));
    if path
        .trim_matches('/')
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return None;
    }
    let mut address = url::Url::parse(&format!("https://{host}{path}/")).ok()?;
    if host == "github.com" {
        let base = base.filter(|base| {
            *base != branch && orch_settings_contract::registry::is_git_branch(base)
        })?;
        address
            .path_segments_mut()
            .ok()?
            .pop_if_empty()
            .push("compare")
            .push(&format!("{base}...{branch}"));
        address.query_pairs_mut().append_pair("quick_pull", "1");
    } else {
        address
            .path_segments_mut()
            .ok()?
            .pop_if_empty()
            .push("merge_requests")
            .push("new");
        address
            .query_pairs_mut()
            .append_pair("merge_request[source_branch]", branch);
    }
    Some(address.to_string())
}

/// Resolves the place once; a retry uses these frozen names rather than newer defaults.
pub async fn resolved(conn: &mut Conn, thread: Id) -> Result<RepositorySource, EngineError> {
    Ok(RepositorySource {
        repository_url: get::<Option<String>>(
            conn,
            Place::Node(thread),
            keys::SOURCE_REPOSITORY_URL,
        )
        .await?
        .flatten(),
        branch: get::<Option<String>>(conn, Place::Node(thread), keys::SOURCE_BRANCH)
            .await?
            .flatten(),
        secret_name: get::<Option<String>>(conn, Place::Node(thread), keys::SOURCE_SECRET_NAME)
            .await?
            .flatten(),
    })
}

/// Resolves a name to an opaque stored reference; neither jobs nor callers receive its value.
pub async fn credential(
    conn: &mut Conn,
    topic: Id,
    source: &RepositorySource,
) -> Result<Option<Id>, EngineError> {
    let reference = credential_if_present(conn, topic, source).await?;
    if source.secret_name.is_some() && reference.is_none() {
        return Err(EngineError::unreadable(
            "the named source credential is unavailable",
        ));
    }
    Ok(reference)
}

// A missing name is deferred until stat proves a fetch is required. Database failures and
// invalid credential schemes still fail rather than being mistaken for an absent secret.
async fn credential_if_present(
    conn: &mut Conn,
    topic: Id,
    source: &RepositorySource,
) -> Result<Option<Id>, EngineError> {
    let Some(name) = &source.secret_name else {
        return Ok(None);
    };
    if source
        .repository_url
        .as_deref()
        .is_none_or(|url| !url.starts_with("https://"))
    {
        return Err(EngineError::unreadable(
            "a named source credential requires HTTPS",
        ));
    }
    let names = orch_settings::secrets::list(conn, orch_settings::secrets::Owner::Node(topic))
        .await
        .map_err(EngineError::unreadable)?;
    let Some(owner) = names
        .iter()
        .find(|secret| &secret.name == name)
        .and_then(|secret| secret.node)
    else {
        return Ok(None);
    };
    Ok(
        orch_db::queries::settings::secret(conn, "setting", owner, name)
            .await?
            .map(|row| row.id),
    )
}

/// Shared interpretation of the existing source-preparation choice. Scheduling and creator
/// admission use this same predicate before the source writer stores the frozen choice.
#[must_use]
pub fn preparation_pending(
    prepare: Option<bool>,
    worktree: bool,
    repository: bool,
    running: bool,
) -> bool {
    worktree || (prepare.unwrap_or(true) && (repository || !running || prepare == Some(true)))
}

/// Stores the creation choice before its first input, without contacting a machine.
pub async fn freeze(
    unit: &mut Unit<'_>,
    caller: &Caller,
    thread: Id,
    environment: Id,
    directory: &str,
    prepare: Option<bool>,
    worktree: Option<&orch_api_contract::source::WorktreeChoice>,
) -> Result<(), EngineError> {
    let env = environments::thread_engine::environment(&mut *unit.conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("the source environment disappeared"))?;
    let frozen = resolved(&mut *unit.conn, thread).await?;
    let preparing = preparation_pending(
        prepare,
        worktree.is_some(),
        frozen.repository_url.is_some(),
        env.status == "running",
    );
    let info = SourceInfo {
        kind: if worktree.is_some() {
            SourceKind::Worktree
        } else if preparing && frozen.repository_url.is_some() {
            SourceKind::Repository
        } else {
            SourceKind::Directory
        },
        directory: directory.to_owned(),
        repository_url: preparing.then(|| frozen.repository_url.clone()).flatten(),
        branch: preparing.then(|| frozen.branch.clone()).flatten(),
        head: None,
        state: if preparing {
            SourceState::Pending
        } else {
            SourceState::Ready
        },
        job: None,
        reason: (prepare == Some(false)).then(|| "source_skipped".to_owned()),
    };
    let seq = unit.stamp(thread).await?;
    let creator_origin = caller
        .admitted_operation()
        .filter(|selection| {
            matches!(
                *selection,
                "ThreadCreate"
                    | "TopicCreate.with_thread"
                    | "TopicInputScheduleCreate"
                    | "InputScheduleUpdate.definition"
            )
        })
        .map(|_| CapturedOperation::capture(&caller.clone().with_admin_mode(false)))
        .transpose()
        .map_err(EngineError::unreadable)?;
    let mut value = with_creator_origin(
        json!({ "info": info, "source": frozen, "prepare": preparing, "worktree": worktree,
            "caller": caller.user(), "token": caller.grant().and_then(|grant| grant.token()) }),
        creator_origin.as_ref(),
        None,
        None,
    );
    if creator_origin.as_ref().is_some_and(|origin| {
        matches!(
            origin.operation(),
            "ThreadCreate" | "TopicCreate.with_thread"
        )
    }) {
        value["creator_environment_generation"] = json!(env.generation);
    }
    source::set_source(&mut *unit.conn, thread, &value, seq).await?;
    Ok(())
}

impl Engine {
    /// Schedules a frozen creation choice once; failures wait for explicit Retry without losing input.
    pub(crate) async fn prepare_pending_source(
        &self,
        thread: &threads::ThreadRow,
    ) -> Result<(), EngineError> {
        let Some(frozen) = thread
            .source
            .as_ref()
            .filter(|source| source.get("source").is_some())
        else {
            return Ok(());
        };
        let info: SourceInfo =
            serde_json::from_value(frozen.get("info").unwrap_or(frozen).clone())?;
        if info.state != SourceState::Pending {
            return Ok(());
        }
        let Some(environment) = thread.environment_id else {
            return Ok(());
        };
        let mut tx = self.db().begin().await?;
        let env = environments::hosting::lock_environment(&mut tx, environment)
            .await?
            .ok_or_else(|| EngineError::unreadable("the source environment disappeared"))?;
        let Some(machine) = env.machine_id else {
            return Ok(());
        };
        if !rows::retained::open_of_environment(&mut tx, environment)
            .await?
            .is_empty()
        {
            return Ok(());
        }
        let current = threads::lock(&mut tx, thread.node_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the source thread disappeared"))?;
        if current.source != thread.source || current.environment_id != Some(environment) {
            return Ok(());
        }
        if accepted_creator_generation(frozen)?.is_some_and(|accepted| accepted != env.generation) {
            return Err(EngineError::unreadable(
                "The accepted source environment was replaced before preparation.",
            ));
        }
        let choice: RepositorySource = serde_json::from_value(frozen["source"].clone())?;
        let worktree: Option<orch_api_contract::source::WorktreeChoice> = frozen
            .get("worktree")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .flatten();
        // Freeze a reference in the original topic's scope. A missing reference matters only
        // after stat proves a fetch is needed; an existing checkout needs no credential.
        let reference = if worktree.is_none() {
            credential_if_present(&mut tx, current.topic_id, &choice).await?
        } else {
            None
        };
        let action = if let Some(choice_tree) = worktree {
            let record_days = get::<i64>(
                &mut tx,
                Place::Environment(environment),
                keys::WORKSPACE_RECORD_DAYS,
            )
            .await?
            .ok_or_else(|| EngineError::unreadable("recording days are unset"))?;
            let max_bytes = get::<i64>(
                &mut tx,
                Place::Environment(environment),
                keys::WORKSPACE_RECORD_MAX_BYTES,
            )
            .await?
            .ok_or_else(|| EngineError::unreadable("recording bytes are unset"))?;
            Action::Worktree {
                body: GitWorktree {
                    environment_id: environment,
                    job_id: Id::nil(),
                    choice: choice_tree,
                    snapshot_id: None,
                },
                source: choice,
                record_days,
                max_bytes,
            }
        } else {
            Action::Prepare {
                directory: current.workspace_dir,
                source: choice,
                credential: reference,
                destination: SourceDestination::MissingDirectory,
                effect_id: None,
            }
        };
        let plan = Plan {
            environment,
            generation: env.generation,
            machine,
            thread: Some(current.node_id),
            caller: frozen
                .get("caller")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or(current.owner_id),
            token: frozen
                .get("token")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok()),
            creator_origin: creator_origin(frozen)?,
            scheduled_input: scheduled_input(frozen)?,
            action,
        };
        begin(&mut tx, plan, self.inner.clock.now())
            .await
            .map_err(EngineError::unreadable)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.kick();
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub environment: Id,
    pub generation: i32,
    pub machine: Id,
    pub thread: Option<Id>,
    pub caller: Id,
    pub token: Option<Id>,
    /// The accepted creator's immutable boundary; absent on historical and unrelated file jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_origin: Option<CapturedOperation>,
    /// Actual accepted scheduled input; its stored authority retains every independent origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_input: Option<Id>,
    pub action: Action,
}

// An absent historical origin keeps its existing path. A present invalid origin never becomes
// a session or a legacy token reconstructed without the accepted operation's policy ceiling.
fn creator_origin(value: &Json) -> Result<Option<CapturedOperation>, EngineError> {
    value
        .get("creator_origin")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}

fn scheduled_input(value: &Json) -> Result<Option<Id>, EngineError> {
    value
        .get("scheduled_input")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}

fn with_creator_origin(
    mut value: Json,
    origin: Option<&CapturedOperation>,
    input: Option<Id>,
    previous: Option<&Json>,
) -> Json {
    if let Some(origin) = origin {
        value["creator_origin"] = json!(origin);
    }
    if let Some(input) = input {
        value["scheduled_input"] = json!(input);
    }
    if origin.is_some() || input.is_some() {
        for field in [
            "creator_environment_generation",
            "scheduled_environment_generation",
        ] {
            if let Some(previous) = previous.and_then(|previous| previous.get(field)) {
                value[field] = previous.clone();
            }
        }
    }
    value
}

fn accepted_creator_generation(value: &Json) -> Result<Option<i32>, EngineError> {
    value
        .get("creator_environment_generation")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Prepare {
        directory: String,
        source: RepositorySource,
        credential: Option<Id>,
        destination: SourceDestination,
        #[serde(default)]
        effect_id: Option<Id>,
    },
    Commit {
        body: GitCommit,
    },
    Worktree {
        body: GitWorktree,
        source: RepositorySource,
        record_days: i64,
        max_bytes: i64,
    },
    Push {
        body: GitPush,
    },
    Restore {
        body: orch_link_contract::source::FsRestore,
        writers: Vec<Id>,
        options: orch_api_contract::forks::AppliedChangeOptions,
        suspend: bool,
        directory: String,
        record_days: i64,
        max_bytes: i64,
    },
}

impl Action {
    fn kind(&self) -> &'static str {
        match self {
            Self::Prepare { .. } | Self::Worktree { .. } => kind::WORKSPACE_PREPARE,
            Self::Commit { .. } => kind::GIT_COMMIT,
            Self::Push { .. } => kind::GIT_PUSH,
            Self::Restore { .. } => kind::WORKSPACE_RESTORE,
        }
    }
}

/// Inserts one serialized file job while the API's current action guard still holds.
pub async fn begin(
    conn: &mut Conn,
    plan: Plan,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<JobRow, MakeError> {
    begin_before(conn, plan, now, || Ok(())).await
}

/// The same native writer with a live cut after its complete native preparation and before INSERT.
pub async fn begin_before(
    conn: &mut Conn,
    mut plan: Plan,
    now: chrono::DateTime<chrono::Utc>,
    before: impl FnOnce() -> Result<(), Box<dyn std::error::Error + Send + Sync>>,
) -> Result<JobRow, MakeError> {
    environments::hosting::lock_environment(conn, plan.environment)
        .await?
        .ok_or(MakeError::NoEnvironment)?;
    if let Some(open) = rows::retained::open_of_environment(conn, plan.environment)
        .await?
        .into_iter()
        .next()
    {
        return Err(MakeError::Running(Box::new(open)));
    }
    let id = new_id();
    match &mut plan.action {
        Action::Commit { body } => body.job_id = id,
        Action::Push { body } => body.job_id = id,
        Action::Prepare { effect_id, .. } => {
            if effect_id.is_none() {
                *effect_id = Some(id);
            }
        }
        Action::Worktree { body, .. } => {
            if body.job_id.is_nil() {
                body.job_id = id;
            }
            // Job identity and an absent current-files capture are independently completed on retry.
            if body.snapshot_id.is_none()
                && matches!(
                    body.choice.from,
                    orch_api_contract::source::WorktreeFrom::Now { .. }
                )
            {
                body.snapshot_id = Some(crate::ids::derived(
                    "worktree.current-files",
                    &[body.job_id.as_bytes()],
                ));
            }
        }
        Action::Restore { body, .. } => {
            body.job_id = id;
            body.undo_snapshot_id = crate::ids::derived("restore.undo", &[id.as_bytes()]);
        }
    }
    if let Action::Worktree { body, .. } = &mut plan.action
        && let Some(snapshot) = body.snapshot_id
        && storage::snapshot(conn, snapshot)
            .await?
            .is_some_and(|row| row.state == "unavailable")
    {
        // No git.worktree command is issued after unavailable capture, so its branch
        // and destination are still frozen but Retry may ask for a newly ready state.
        body.snapshot_id = Some(crate::ids::derived(
            "worktree.current-files.retry",
            &[body.job_id.as_bytes(), id.as_bytes()],
        ));
    }
    let mut value = serde_json::to_value(&plan)
        .map_err(|error| orch_db::DbError::Other(sqlx::Error::Decode(error.into())))?;
    if let Some(thread) = plan.thread
        && let Some(source) = threads::thread(conn, thread)
            .await?
            .and_then(|row| row.source)
    {
        let original = source.get("native_plan");
        if original.is_some_and(|original| {
            original.get("environment") == value.get("environment")
                && original.get("generation") == value.get("generation")
                && original.get("machine") == value.get("machine")
                && original.get("action") == value.get("action")
        }) {
            if let Some(result) = source
                .get("native_result")
                .filter(|result| !result.is_null())
            {
                value["file_result"] = result.clone();
                value["file_source_attempt"] = json!(plan);
            }
            if source.get("native_started").and_then(Json::as_bool) == Some(true) {
                value["file_prepare_started"] = json!(plan);
            }
        }
    }
    let progress = serde_json::to_value(initial_progress(
        if matches!(plan.action, Action::Restore { .. }) {
            &super::restore::PHASES
        } else {
            &PHASES
        },
    ))
    .map_err(|error| orch_db::DbError::Other(sqlx::Error::Decode(error.into())))?;
    before().map_err(EngineError::BeforeEffect)?;
    let job = rows::insert(
        conn,
        &NewJob {
            id,
            kind: plan.action.kind(),
            node_id: plan.thread,
            environment_id: Some(plan.environment),
            provider_id: None,
            plan: &value,
            progress: &progress,
            created_by: Some(plan.caller),
        },
        now,
    )
    .await?;
    if let (
        Some(thread),
        Action::Prepare {
            directory,
            source: frozen,
            ..
        },
    ) = (plan.thread, &plan.action)
    {
        let mut unit = Unit::new(conn, now);
        let row = threads::lock(&mut *unit.conn, thread)
            .await?
            .ok_or(MakeError::NoEnvironment)?;
        let info = SourceInfo {
            kind: if frozen.repository_url.is_some() {
                SourceKind::Repository
            } else {
                SourceKind::Directory
            },
            directory: directory.clone(),
            repository_url: frozen.repository_url.clone(),
            branch: frozen.branch.clone(),
            head: None,
            state: SourceState::Preparing,
            job: Some(reference(&job)),
            reason: None,
        };
        let seq = unit
            .stamp(thread)
            .await
            .map_err(|error| orch_db::DbError::Other(sqlx::Error::Decode(error.into())))?;
        source::set_source(&mut *unit.conn, row.node_id, &with_creator_origin(json!({ "info": info, "source": frozen, "prepare": true,
            "caller": plan.caller, "token": plan.token, "credential_home": row.topic_id, "native_plan": plan }), plan.creator_origin.as_ref(), plan.scheduled_input, row.source.as_ref()), seq).await?;
        unit.finish()
            .await
            .map_err(|error| orch_db::DbError::Other(sqlx::Error::Decode(error.into())))?;
    }
    if let (
        Some(thread),
        Action::Worktree {
            body,
            source: frozen,
            ..
        },
    ) = (plan.thread, &plan.action)
    {
        let mut unit = Unit::new(conn, now);
        let row = threads::lock(&mut *unit.conn, thread)
            .await?
            .ok_or(MakeError::NoEnvironment)?;
        let info = SourceInfo {
            kind: SourceKind::Worktree,
            directory: body.choice.directory.clone(),
            repository_url: None,
            branch: Some(body.choice.branch.clone()),
            head: None,
            state: SourceState::Preparing,
            job: Some(reference(&job)),
            reason: None,
        };
        let seq = unit
            .stamp(thread)
            .await
            .map_err(|error| orch_db::DbError::Other(sqlx::Error::Decode(error.into())))?;
        source::set_source(
            &mut *unit.conn,
            thread,
            &with_creator_origin(
                json!({ "info": info, "source": frozen, "prepare": true, "worktree": body.choice,
            "caller": plan.caller, "token": plan.token, "native_plan": plan }),
                plan.creator_origin.as_ref(),
                plan.scheduled_input,
                row.source.as_ref(),
            ),
            seq,
        )
        .await?;
        unit.finish()
            .await
            .map_err(|error| orch_db::DbError::Other(sqlx::Error::Decode(error.into())))?;
    }
    Ok(job)
}

/// Rechecks current caller rights and the frozen binding before a native effect.
pub async fn principal(conn: &mut Conn, job: &Job, plan: &Plan) -> Result<Principal, Stop> {
    let row = job.row().await?;
    if operation::is_restricted(&row) {
        return operation::principal_at(conn, &row, plan, Time::new(job.now())).await;
    }
    principal_at(conn, plan, Time::new(job.now())).await
}

async fn principal_at(conn: &mut Conn, plan: &Plan, now: Time) -> Result<Principal, Stop> {
    if plan.scheduled_input.is_some() {
        return Err(Failure::new(
            "access.denied",
            "Scheduled source authority needs every original boundary.",
        )
        .into());
    }
    if let Some(origin) = &plan.creator_origin {
        let thread = plan.thread.ok_or_else(|| {
            Failure::new(
                "access.denied",
                "The accepted creator source has no thread.",
            )
        })?;
        if origin.user() != plan.caller
            || !matches!(
                plan.action,
                Action::Prepare { .. } | Action::Worktree { .. }
            )
        {
            return Err(
                Failure::new("access.denied", "The accepted creator source is invalid.").into(),
            );
        }
        let caller = crate::operation_scope::creator_files::restore(
            conn,
            origin,
            thread,
            plan.environment,
            matches!(plan.action, Action::Worktree { .. }),
            now,
        )
        .await?
        .ok_or_else(|| {
            Failure::new(
                "access.denied",
                "The accepted creator no longer permits this file operation.",
            )
        })?;
        return Principal::load(conn, caller, now)
            .await
            .map_err(EngineError::from)
            .map_err(Into::into);
    }
    // The existing loader pins person then token. Inspect the modern marker only after those
    // locks, so a legacy-to-modern scope change cannot slip between detection and restoration.
    let principal = load_principal_at(conn, plan.caller, plan.token, now).await?;
    if let Some(token) = plan.token {
        let scoped: Option<bool> = sqlx::query_scalar(
            "SELECT operation_scope IS NOT NULL FROM api_tokens WHERE id=$1 AND user_id=$2",
        )
        .bind(token)
        .bind(plan.caller)
        .fetch_optional(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
        if scoped == Some(true) {
            return Err(Failure::new(
                "access.denied",
                "This scoped file operation has no accepted creator origin.",
            )
            .into());
        }
    }
    Ok(principal)
}

/// Loads the current token row for a frozen file act, including a fork's recorded-state copy.
pub(crate) async fn load_principal(
    conn: &mut Conn,
    job: &Job,
    user: Id,
    token: Option<Id>,
) -> Result<Principal, Stop> {
    load_principal_at(conn, user, token, Time::new(job.now())).await
}

pub(crate) async fn load_principal_at(
    conn: &mut Conn,
    user: Id,
    token: Option<Id>,
    now: Time,
) -> Result<Principal, Stop> {
    // Personal token writers use this person's UPDATE lock, not the policy
    // barrier. Pin the person before reading their frozen act's current token.
    let present: Option<Id> =
        sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR KEY SHARE")
            .bind(user)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    if present.is_none() {
        return Err(Failure::new(
            "auth.invalid_token",
            "The account that requested this file operation no longer exists.",
        )
        .into());
    }
    let caller = if let Some(id) = token {
        let row = source::job_token(conn, id, user, now.get())
            .await?
            .ok_or_else(|| {
                Failure::new(
                    "auth.invalid_token",
                    "The access token used for this file operation is no longer valid.",
                )
            })?;
        Caller::token(
            user,
            TokenGrant::from_stored(&row.scopes, row.node_id)
                .map_err(EngineError::unreadable)?
                .for_token(id),
        )
    } else {
        Caller::session(user)
    };
    Principal::load(conn, caller, now)
        .await
        .map_err(EngineError::from)
        .map_err(Into::into)
}

pub async fn check(job: &Job, plan: &Plan) -> Result<(), Stop> {
    if operation::received(job, plan).await? || publication_authority::received(job, plan).await? {
        return Ok(());
    }
    // An existing directory needs only a read. The effect rechecks write authority after stat
    // when the directory is missing; a machine refusing writes may still keep an existing checkout.
    let checked = check_with_write(job, plan, requires_write(plan), None).await;
    if matches!(&checked,Err(Stop::Failed(failure)) if failure.code!=super::runner::INTERNAL)
        && publication_authority::received(job, plan).await?
    {
        // The actual ACK may have arrived while the current forward gate was waiting.
        // Reconciliation still proves the exact owned frame and ordinary ending fences.
        return Ok(());
    }
    checked
}

fn requires_write(plan: &Plan) -> bool {
    !matches!(
        plan.action,
        Action::Prepare {
            destination: SourceDestination::MissingDirectory,
            ..
        }
    )
}

// The initial files run before running publication. Only that provisioning job can use its
// exact enrolled candidate; an ordinary file job still requires the published environment identity.
async fn target(
    conn: &mut Conn,
    job: Id,
    plan: &Plan,
    pending: Option<Id>,
) -> Result<environments::workspace::EnvironmentRow, Stop> {
    let stale = || {
        Failure::new(
            "conflict.stale_preflight",
            "The environment changed while its files were being prepared. Review it and try again.",
        )
    };
    let owner = if pending.is_some() {
        rows::job(conn, job).await?
    } else {
        None
    };
    if owner.as_ref().is_some_and(JobRow::cancel_requested) {
        return Err(Stop::Cancelled);
    }
    matching_target(conn, plan, owner.as_ref().zip(pending))
        .await?
        .filter(|_| pending.is_none() || owner.is_some())
        .ok_or_else(|| stale().into())
}

// This identity check also serves undo. An owned cancellation may publish failure, whereas
// the pre-command target check above must honour cancellation before doing more work.
async fn matching_target(
    conn: &mut Conn,
    plan: &Plan,
    pending: Option<(&JobRow, Id)>,
) -> Result<Option<environments::workspace::EnvironmentRow>, EngineError> {
    if let Some((owner, runtime)) = pending {
        return matching_provision_target(
            conn,
            owner,
            plan.environment,
            plan.machine,
            plan.generation,
            runtime,
        )
        .await;
    }
    Ok(environments::workspace::environment(conn, plan.environment)
        .await?
        .filter(|env| env.generation == plan.generation && env.machine_id == Some(plan.machine)))
}

/// Current candidate facts shared by source and initial-copy jobs. This is the original pending
/// target check, not authorization: callers separately restore every accepted credential origin.
pub(crate) async fn matching_provision_target(
    conn: &mut Conn,
    owner: &JobRow,
    environment: Id,
    machine: Id,
    generation: i32,
    runtime: Id,
) -> Result<Option<environments::workspace::EnvironmentRow>, EngineError> {
    let Some(env) = environments::workspace::environment(conn, environment).await? else {
        return Ok(None);
    };
    let matches = {
        // The publication transaction pins this candidate's identity through its metadata write.
        // The pre-command check uses the same short lock and commits before any machine wait.
        let exists: Option<Id> =
            sqlx::query_scalar("SELECT id FROM runtimes WHERE id=$1 FOR UPDATE")
                .bind(runtime)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        if exists.is_none() {
            return Ok(None);
        }
        let owning = owner.kind == kind::ENVIRONMENT_PROVISION
            && owner.is_open()
            && owner.environment_id == Some(environment)
            && owner
                .plan
                .get("runtime")
                .and_then(Json::as_str)
                .and_then(|id| id.parse::<Id>().ok())
                == Some(runtime)
            && owner.plan.get("generation").and_then(Json::as_i64)
                == Some(i64::from(env.generation))
            && match owner.plan.get("mode").and_then(Json::as_str) {
                Some("make") => env.generation.checked_add(1) == Some(generation),
                Some("start") => env.generation == generation,
                _ => false,
            };
        if env.status != "provisioning" || !owning {
            false
        } else {
            let candidate = environments::hosting::live_runtime(conn, environment).await?;
            candidate.is_some_and(|row| {
                row.id == runtime
                    && row.environment_id == Some(environment)
                    && row.machine_id == Some(machine)
                    && row.generation == generation
                    && matches!(row.state.as_str(), "creating" | "running")
            })
        }
    };
    Ok(matches.then_some(env))
}

async fn check_with_write(
    job: &Job,
    plan: &Plan,
    write: bool,
    pending: Option<Id>,
) -> Result<(), Stop> {
    let owner = job.row().await?;
    if operation::is_restricted(&owner) {
        if operation::received(job, plan).await? {
            return Ok(());
        }
        return operation::check(job, plan, write).await;
    }
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    check_in(&mut tx, job, plan, write, pending).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

// The publication uses this same policy and identity check on its existing connection.
async fn check_in(
    conn: &mut Conn,
    job: &Job,
    plan: &Plan,
    write: bool,
    pending: Option<Id>,
) -> Result<(), Stop> {
    let owner = rows::job(conn, job.id()).await?.ok_or(Stop::Lost)?;
    if operation::is_restricted(&owner) {
        if operation::reconcile_at(conn, &owner, plan).await? {
            return Ok(());
        }
        return operation::before_mutation(conn, job, plan, write).await;
    }
    check_at(conn, job.id(), Time::new(job.now()), plan, write, pending).await
}

async fn check_at(
    conn: &mut Conn,
    job: Id,
    now: Time,
    plan: &Plan,
    write: bool,
    pending: Option<Id>,
) -> Result<(), Stop> {
    let principal = if let Some(input) = plan.scheduled_input {
        if !crate::inputs::scheduled_source_authorized(
            conn,
            input,
            plan.thread.ok_or_else(|| {
                Failure::new("access.denied", "The scheduled source has no thread.")
            })?,
            plan.environment,
            matches!(plan.action, Action::Worktree { .. }),
            now,
            plan.caller,
            plan.creator_origin.as_ref(),
        )
        .await?
        {
            return Err(Failure::new(
                "access.denied",
                "The scheduled source is no longer authorized.",
            )
            .into());
        }
        None
    } else {
        let owner = rows::job(conn, job).await?;
        if let Some(owner) = owner.as_ref().filter(|row| operation::is_restricted(row)) {
            Some(operation::principal_at(conn, owner, plan, now).await?)
        } else if let Some(owner) = owner.filter(publication_authority::is_restricted) {
            Some(publication_authority::principal(conn, &owner, plan, now).await?)
        } else {
            Some(principal_at(conn, plan, now).await?)
        }
    };
    check_native_at(conn, job, plan, write, pending, principal.as_ref()).await
}

pub(crate) async fn check_native_at(
    conn: &mut Conn,
    job: Id,
    plan: &Plan,
    write: bool,
    pending: Option<Id>,
    principal: Option<&Principal>,
) -> Result<(), Stop> {
    let permission = if matches!(
        plan.action,
        Action::Prepare { .. } | Action::Worktree { .. }
    ) {
        Permission::EnvironmentManage
    } else {
        Permission::FilesManage
    };
    if let Some(principal) = &principal {
        let allowed = orch_authz::resources::check(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: plan.environment,
            },
            permission,
            Policy::default(),
        )
        .await
        .map_err(EngineError::from)?;
        if !matches!(allowed, Decision::Allowed { .. }) {
            return Err(Failure::new(
                "access.denied",
                "You no longer have permission to complete this file operation.",
            )
            .into());
        }
    }
    let env = target(conn, job, plan, pending).await?;
    if write && !env.remote_write {
        return Err(Failure::new(
            "workspace.remote_write_disabled",
            "this environment or its machine does not allow Orch to write files here",
        )
        .into());
    }
    if write
        && orch_db::queries::environment_facts::refuses(
            conn,
            plan.environment,
            "environment.remote_write",
        )
        .await?
    {
        return Err(Failure::new(
            "policy.remote_write_disabled",
            "the hosting machine's policy does not allow Orch to write files here",
        )
        .into());
    }
    if let Some(thread) = plan
        .thread
        .filter(|_| !matches!(plan.action, Action::Restore { .. }))
    {
        let row = threads::thread(conn, thread).await?.filter(|row| row.environment_id == Some(plan.environment))
            .ok_or_else(|| Failure::new("conflict.stale_preflight", "The thread changed environment while its files were being prepared. Review it and try again."))?;
        if row
            .source
            .as_ref()
            .map(accepted_creator_generation)
            .transpose()?
            .flatten()
            .is_some_and(|accepted| {
                accepted != plan.generation
                    && !(pending.is_some() && accepted == 0 && env.generation == 0)
            })
        {
            return Err(Failure::new(
                "conflict.stale_preflight",
                "The accepted source environment generation changed.",
            )
            .into());
        }
        if let Action::Prepare { directory, .. } = &plan.action
            && row.workspace_dir != *directory
        {
            return Err(
                Failure::new("conflict.stale_preflight", "the source directory changed").into(),
            );
        }
        if let Action::Worktree { body, .. } = &plan.action
            && row.workspace_dir != body.choice.directory
        {
            return Err(Failure::new(
                "conflict.stale_preflight",
                "the own attempt's directory changed",
            )
            .into());
        }
    }
    Ok(())
}

/// Existing durable job ownership is the only association used by the dispatcher. A command
/// without creator evidence retains its ordinary path; present creator evidence must be exact.
/// Callers hold the normal action barrier and re-read the unacknowledged command under lock.
pub async fn creator_command_authorized(
    conn: &mut Conn,
    command: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    now: Time,
) -> Result<bool, EngineError> {
    let owners = rows::lock_source_command_owners(conn, command).await?;
    if owners.is_empty() {
        return Ok(true);
    }
    if owners.len() != 1 {
        return Ok(false);
    }
    let Some(owner) = rows::job(conn, owners[0]).await? else {
        return Ok(false);
    };
    if publication_authority::is_restricted(&owner) {
        return publication_authority::command_authorized(
            conn, &owner, command, machine, key, kind, body, now,
        )
        .await;
    }
    if matches!(owner.kind.as_str(), kind::GIT_COMMIT | kind::GIT_PUSH) {
        // Historical ordinary jobs retain their path, while a modernized token with no
        // accepted original operation must never acquire an unrestricted native dispatch.
        let plan: Plan = serde_json::from_value(owner.plan.clone())?;
        return match principal_at(conn, &plan, now).await {
            Ok(_) => Ok(true),
            Err(Stop::Failed(failure)) if failure.code == super::runner::INTERNAL => Err(
                EngineError::unreadable("Git publication authority could not be checked."),
            ),
            Err(Stop::Failed(_) | Stop::Cancelled | Stop::Lost) => Ok(false),
        };
    }
    let native = owner.plan.get("creator_command");
    let expected_creator = owner.kind == kind::WORKSPACE_PREPARE
        && ["creator_origin", "scheduled_input"]
            .iter()
            .any(|field| owner.plan.get(*field).is_some_and(|value| !value.is_null()));
    let Some(native) = native.filter(|value| !value.is_null()) else {
        if owner.kind == kind::WORKSPACE_PREPARE
            && let Some(token) = owner
                .plan
                .get("token")
                .and_then(Json::as_str)
                .and_then(|token| token.parse::<Id>().ok())
        {
            let Some(user) = owner
                .plan
                .get("caller")
                .and_then(Json::as_str)
                .and_then(|user| user.parse::<Id>().ok())
            else {
                return Ok(false);
            };
            let present: Option<Id> =
                sqlx::query_scalar("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
                    .bind(user)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            if present.is_none() {
                return Ok(false);
            }
            let scoped: Option<bool> = sqlx::query_scalar(
                "SELECT operation_scope IS NOT NULL FROM api_tokens WHERE id=$1 AND user_id=$2 FOR SHARE",
            )
            .bind(token)
            .bind(user)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
            if scoped != Some(false) {
                return Ok(false);
            }
        }
        return Ok(!expected_creator);
    };
    if native.get("id") != Some(&json!(command)) {
        // Another phase may have completed and already reserved the next command. Its private
        // evidence cannot be lent to an older, still unacknowledged command.
        return Ok(false);
    }
    let plan: Plan = serde_json::from_value(native.get("plan").cloned().unwrap_or(Json::Null))?;
    if plan.creator_origin.is_none() && plan.scheduled_input.is_none() {
        return Ok(false);
    }
    let runtime: Option<Id> =
        serde_json::from_value(native.get("runtime").cloned().unwrap_or(Json::Null))?;
    if !owner.is_open()
        || owner.cancel_requested()
        || owner.environment_id != Some(plan.environment)
        || plan.machine != machine
        || SerialKey::Environment(plan.environment).wire().as_deref() != key
        || owner.plan.pointer("/active_command/id") != Some(&json!(command))
        || owner.plan.pointer("/active_command/machine") != Some(&json!(machine))
        || owner
            .plan
            .pointer("/active_command/kind")
            .and_then(Json::as_str)
            != Some(kind)
        || !creator_body(&plan, owner.id, kind, body)?
    {
        return Ok(false);
    }
    let Some(thread) = plan.thread else {
        return Ok(false);
    };
    let Some(current) = threads::lock(conn, thread).await? else {
        return Ok(false);
    };
    let Some(source) = current.source.as_ref() else {
        return Ok(false);
    };
    let info: SourceInfo = serde_json::from_value(source.get("info").unwrap_or(source).clone())?;
    if source.get("native_plan") != Some(&json!(plan))
        || !matches!(info.state, SourceState::Pending | SourceState::Preparing)
    {
        return Ok(false);
    }
    let owned = if let Some(runtime) = runtime {
        let effect = crate::ids::derived(
            "source.provision.files",
            &[owner.id.as_bytes(), thread.as_bytes()],
        );
        owner.kind == kind::ENVIRONMENT_PROVISION
            && match &plan.action {
                Action::Prepare { effect_id, .. } => *effect_id == Some(effect),
                Action::Worktree { body, .. } => body.job_id == effect,
                _ => false,
            }
            && matching_target(conn, &plan, Some((&owner, runtime)))
                .await?
                .is_some()
    } else {
        owner.kind == kind::WORKSPACE_PREPARE
            && owner.node_id == Some(thread)
            && info.job.as_ref().map(|job| job.id) == Some(owner.id)
            && serde_json::to_value(serde_json::from_value::<Plan>(owner.plan.clone())?)?
                == json!(plan)
    };
    if !owned {
        return Ok(false);
    }
    match check_at(conn, owner.id, now, &plan, requires_write(&plan), runtime).await {
        Ok(()) => Ok(true),
        // A failed authority, attempt or lifecycle check refuses publication rather than
        // rebuilding a less restricted caller. The dispatcher emits its existing safe refusal.
        Err(Stop::Failed(failure)) if failure.code == super::runner::INTERNAL => Err(
            EngineError::unreadable("Creator source authority could not be checked."),
        ),
        Err(Stop::Failed(_)) | Err(Stop::Cancelled | Stop::Lost) => Ok(false),
    }
}

/// Export actual current identities only after the exact existing first-send recipe check.
/// Historical/system paths retain the existing loader; present scoped provenance never falls back.
pub async fn creator_command_current_callers(
    conn: &mut Conn,
    command: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    clock: &dyn orch_types::Clock,
) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
    let known = rows::lock_source_command_owners(conn, command).await?;
    if known.len() == 1 {
        if let Some(owner) = rows::job(conn, known[0]).await?
            && operation::is_restricted(&owner)
        {
            let Some(private) =
                orch_db::queries::link::command_with_authority(conn, command).await?
            else {
                return Ok(None);
            };
            if private.command.machine_id != machine
                || private.command.serial_key() != key
                || private.command.kind != kind
                || private.command.body != *body
            {
                return Ok(None);
            }
            let Some(value) = private.authority else {
                return Ok(None);
            };
            let authority: crate::commands::CommandAuthority = serde_json::from_value(value)?;
            if let crate::commands::CommandAuthority::SourcePreparation(authority) = authority {
                return authority
                    .current_callers(conn, &private.command, clock)
                    .await;
            }
            return Ok(None);
        }
    }
    if !creator_command_authorized(conn, command, machine, key, kind, body, clock.time()).await? {
        return Ok(None);
    }
    let owners = rows::lock_source_command_owners(conn, command).await?;
    if owners.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if owners.len() != 1 {
        return Ok(None);
    }
    let Some(owner) = rows::job(conn, owners[0]).await? else {
        return Ok(None);
    };
    if publication_authority::is_restricted(&owner) {
        return publication_authority::current_callers(conn, &owner, clock).await;
    }
    let native = owner
        .plan
        .get("creator_command")
        .filter(|value| !value.is_null());
    if native.is_none()
        && !matches!(
            owner.kind.as_str(),
            kind::WORKSPACE_PREPARE | kind::GIT_COMMIT | kind::GIT_PUSH
        )
    {
        // Another job family's ordinary frame is not a source recipe or caller loan.
        return Ok(Some(Vec::new()));
    }
    let plan: Plan = if let Some(native) = native {
        serde_json::from_value(native.get("plan").cloned().unwrap_or(Json::Null))?
    } else {
        serde_json::from_value(owner.plan.clone())?
    };
    if let Some(input) = plan.scheduled_input {
        let Some(thread) = plan.thread else {
            return Ok(None);
        };
        return crate::inputs::scheduled_source_current_callers(
            conn,
            input,
            thread,
            plan.caller,
            plan.creator_origin.as_ref(),
            clock,
        )
        .await;
    }
    if let Some(origin) = &plan.creator_origin {
        return Ok(origin
            .restore_current(conn, clock)
            .await?
            .map(|caller| vec![(Some(caller), None)]));
    }
    let principal = match principal_at(conn, &plan, clock.time()).await {
        Ok(principal) => principal,
        Err(Stop::Failed(failure)) if failure.code == super::runner::INTERNAL => {
            return Err(EngineError::unreadable(
                "Creator source authority could not be checked.",
            ));
        }
        Err(_) => return Ok(None),
    };
    Ok(orch_authz::CapturedCaller::capture(principal.caller())
        .restore_current(conn, clock)
        .await?
        .map(|caller| vec![(Some(caller), None)]))
}

fn creator_body(plan: &Plan, job: Id, kind: &str, body: &Json) -> Result<bool, EngineError> {
    let expected = match &plan.action {
        Action::Prepare {
            directory,
            source,
            credential,
            destination,
            effect_id,
        } => match kind {
            "fs.stat" => json!(FsStat {
                environment_id: plan.environment,
                mount: None,
                path: directory.clone()
            }),
            "fs.mkdir" if !directory.is_empty() => json!(FsMkdir {
                environment_id: plan.environment,
                path: directory.clone()
            }),
            "git.prepare" => {
                let Some(url) = &source.repository_url else {
                    return Ok(false);
                };
                json!(GitPrepare {
                    environment_id: plan.environment,
                    job_id: effect_id.unwrap_or(job),
                    directory: directory.clone(),
                    repository_url: url.clone(),
                    branch: source.branch.clone(),
                    destination: destination.clone(),
                    credential: credential.map(Secret::Ref)
                })
            }
            _ => return Ok(false),
        },
        Action::Worktree {
            body: worktree,
            record_days,
            max_bytes,
            ..
        } => match kind {
            "git.worktree" => json!(worktree),
            "fs.record" => {
                let Some(snapshot_id) = worktree.snapshot_id else {
                    return Ok(false);
                };
                json!(FsRecord {
                    environment_id: plan.environment,
                    snapshot_id,
                    directory: worktree.choice.repository_dir.clone(),
                    kind: RecordKind::Turn,
                    record_days: *record_days,
                    max_bytes: *max_bytes
                })
            }
            _ => return Ok(false),
        },
        _ => return Ok(false),
    };
    Ok(*body == expected)
}

pub(crate) async fn lock_attempt_at(
    conn: &mut Conn,
    owner: &JobRow,
    plan: &Plan,
    runtime: Option<Id>,
) -> Result<Option<threads::ThreadRow>, EngineError> {
    let (frozen, directory, worktree) = match &plan.action {
        Action::Prepare {
            source, directory, ..
        } => (source, directory, None),
        Action::Worktree { source, body, .. } => {
            (source, &body.choice.directory, Some(&body.choice))
        }
        _ => return Ok(None),
    };
    let Some(thread) = plan.thread else {
        return Ok(None);
    };
    let Some(row) = threads::lock(conn, thread).await? else {
        return Ok(None);
    };
    let Some(value) = row.source.as_ref() else {
        return Ok(None);
    };
    let info: SourceInfo = serde_json::from_value(value.get("info").unwrap_or(value).clone())?;
    let native_plan = serde_json::to_value(&plan)?;
    if row.environment_id != Some(plan.environment)
        || row.workspace_dir != *directory
        || !matches!(info.state, SourceState::Pending | SourceState::Preparing)
        || value.get("source") != Some(&json!(frozen))
        || value.get("worktree").filter(|value| !value.is_null())
            != worktree.map(|choice| json!(choice)).as_ref()
        || value.get("native_plan") != Some(&native_plan)
    {
        return Ok(None);
    }
    let pending = if let Some(runtime) = runtime {
        // Inline preparation belongs to the provisioning parent and its derived native effect;
        // it legitimately has no standalone source job in SourceInfo.
        let effect = crate::ids::derived(
            "source.provision.files",
            &[owner.id.as_bytes(), thread.as_bytes()],
        );
        let owns_effect = match &plan.action {
            Action::Prepare { effect_id, .. } => *effect_id == Some(effect),
            Action::Worktree { body, .. } => body.job_id == effect,
            _ => false,
        };
        if !owns_effect {
            return Ok(None);
        }
        Some((owner, runtime))
    } else {
        if owner.kind != kind::WORKSPACE_PREPARE
            || owner.node_id != Some(thread)
            || owner.environment_id != Some(plan.environment)
            || info.job.as_ref().map(|job| job.id) != Some(owner.id)
            || serde_json::to_value(serde_json::from_value::<Plan>(owner.plan.clone())?)?
                != native_plan
        {
            return Ok(None);
        }
        None
    };
    if matching_target(conn, &plan, pending).await?.is_none() {
        return Ok(None);
    }
    Ok(Some(row))
}

#[derive(Debug, Clone)]
pub struct Preparing {
    engine: Engine,
    plan: Plan,
    provisioning_runtime: Option<Id>,
    operation: Option<operation::Request>,
}
impl Preparing {
    pub fn of(engine: Engine, row: &JobRow) -> Result<Self, EngineError> {
        let plan = serde_json::from_value::<Plan>(row.plan.clone())?;
        Ok(Self {
            engine,
            plan,
            provisioning_runtime: None,
            operation: operation::Request::from_job(row)?,
        })
    }

    /// Keep the original source recipe attached to the exact job command before the ordinary
    /// job runner records active_command and makes it visible to the durable dispatcher.
    async fn command(&self, job: &Job, command: Outgoing) -> Result<Json, Stop> {
        if self.plan.creator_origin.is_some() || self.plan.scheduled_input.is_some() {
            check_with_write(
                job,
                &self.plan,
                requires_write(&self.plan),
                self.provisioning_runtime,
            )
            .await?;
            let mut tx = self.engine.db().begin().await?;
            let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
            if let Some(reason) = super::runner::abort_reason(&owner)? {
                return Err(reason);
            }
            if self.lock_attempt(&mut tx, &owner).await?.is_none() {
                return Err(Failure::new(
                    "conflict.stale_preflight",
                    "The creator source changed before its command.",
                )
                .into());
            }
            rows::merge_plan(
                &mut tx,
                job.id(),
                &json!({"creator_command": {
                    "id": command.id, "plan": self.plan, "runtime": self.provisioning_runtime,
                }}),
                job.now(),
            )
            .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        let mut command = job.prepare_command(command).await?;
        if let Some(origin) = &self.operation {
            let row = job.row().await?;
            let authority = operation::SourceCommand::capture(&row, origin, &command)?;
            command.authority = Some(crate::commands::CommandAuthority::source_preparation(
                authority,
            ));
        }
        job.command(command).await
    }

    /// Pins the current source attempt after the caller has locked its authoritative job claim.
    /// A retry may keep the same native effect, so the explicit job reference is also required.
    async fn lock_attempt(
        &self,
        conn: &mut Conn,
        owner: &JobRow,
    ) -> Result<Option<threads::ThreadRow>, EngineError> {
        lock_attempt_at(conn, owner, &self.plan, self.provisioning_runtime).await
    }

    // No transaction survives the native cleanup wait. Publication checks this again afterwards.
    async fn owns_attempt(&self, job: &Job) -> Result<bool, EngineError> {
        let mut tx = self.engine.db().begin().await?;
        let Some(owner) = job.lock_owned(&mut tx).await? else {
            return Ok(false);
        };
        let owned = if self.plan.thread.is_some() {
            self.lock_attempt(&mut tx, &owner).await?.is_some()
        } else {
            matching_target(&mut tx, &self.plan, None).await?.is_some()
        };
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(owned)
    }

    async fn apply(&self, job: &Job) -> Result<Done, Stop> {
        check(job, &self.plan).await?;
        // Publication is an irreversible explicit effect. Crossing commit before the native
        // command prevents cancellation from falsely promising to undo an accepted commit/push.
        if !matches!(
            self.plan.action,
            Action::Prepare { .. } | Action::Worktree { .. }
        ) {
            job.pass_commit().await?;
        }
        self.effect(job).await
    }

    async fn prepare_was_issued(&self, job: &Job, effect: Id) -> Result<bool, Stop> {
        let saved = job.plan().await?;
        if saved.get("file_prepare_started") == Some(&json!(self.plan))
            || (saved
                .get("file_result")
                .and_then(|result| result.get("kind"))
                .and_then(Json::as_str)
                == Some("repository")
                && (self.provisioning_runtime.is_none()
                    || saved.get("file_source_attempt") == Some(&json!(self.plan))))
        {
            return Ok(true);
        }
        // A retained Directory answer can be a read-only observation. It is not proof of
        // native preparation and must still work with both write gates off after a crash.
        // Also recognize a retained pre-marker command on upgrade, including a standalone
        // explicit retry's original effect. An unequal parent/effect alone proves nothing.
        for id in [
            crate::ids::derived("git.prepare", &[effect.as_bytes(), job.id().as_bytes()]),
            crate::ids::derived("git.prepare", &[effect.as_bytes(), effect.as_bytes()]),
            crate::ids::derived("source.mkdir", &[effect.as_bytes()]),
        ] {
            if job
                .engine()
                .inner
                .commands
                .status(id)
                .await
                .map_err(EngineError::from)?
                .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn mark_prepare_started(&self, job: &Job) -> Result<(), Stop> {
        let mut tx = self.engine.db().begin().await?;
        let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if let Some(reason) = super::runner::abort_reason(&owner)? {
            return Err(reason);
        }
        if self.operation.is_some() {
            if owner.plan.get("file_prepare_started") == Some(&json!(self.plan)) {
                return Ok(());
            }
            operation::before_mutation(&mut tx, job, &self.plan, true).await?;
        }
        if self.plan.thread.is_some() && self.lock_attempt(&mut tx, &owner).await?.is_none() {
            return Err(Failure::new(
                "conflict.stale_preflight",
                "The source attempt changed before its files could be prepared.",
            )
            .into());
        }
        rows::merge_plan(
            &mut tx,
            job.id(),
            &json!({ "file_prepare_started": self.plan }),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    async fn effect(&self, job: &Job) -> Result<Done, Stop> {
        if self.provisioning_runtime.is_some() {
            check_with_write(
                job,
                &self.plan,
                requires_write(&self.plan),
                self.provisioning_runtime,
            )
            .await?;
        }
        let mut result = match &self.plan.action {
            Action::Prepare {
                directory,
                source: frozen,
                credential,
                destination,
                effect_id,
            } => {
                let effect = effect_id.unwrap_or(job.id());
                let existing = self
                    .command(
                        job,
                        Outgoing::new(
                            if self.operation.is_some() {
                                crate::ids::derived(
                                    "source.stat",
                                    &[effect.as_bytes(), job.id().as_bytes()],
                                )
                            } else {
                                crate::ids::derived("source.stat", &[effect.as_bytes()])
                            },
                            self.plan.machine,
                            SerialKey::Environment(self.plan.environment),
                            &FsStat {
                                environment_id: self.plan.environment,
                                mount: None,
                                path: directory.clone(),
                            },
                        )
                        .map_err(EngineError::from)?,
                    )
                    .await;
                let owned_root = matches!(destination, SourceDestination::OwnedEmptyRoot { .. });
                let missing = match existing {
                    Ok(_) if owned_root => true,
                    Ok(value) => {
                        let entry: orch_api_contract::workspace::FileStat =
                            serde_json::from_value(value).map_err(EngineError::from)?;
                        if entry.entry.kind != orch_api_contract::workspace::DirEntryKind::Dir {
                            return Err(Failure::new(
                                "workspace.source_conflict",
                                "the source destination is not a directory",
                            )
                            .into());
                        }
                        false
                    }
                    Err(Stop::Failed(failure)) if failure.code == "not_found" => true,
                    Err(stop) => return Err(stop),
                };
                let info = if missing || self.prepare_was_issued(job, effect).await? {
                    check_with_write(job, &self.plan, true, self.provisioning_runtime).await?;
                    if let Some(url) = &frozen.repository_url {
                        if credential.is_none() && frozen.secret_name.is_some() {
                            return Err(Failure::new(
                                "workspace.source_failed",
                                "the named HTTPS source credential is unavailable",
                            )
                            .into());
                        }
                        let command = GitPrepare {
                            environment_id: self.plan.environment,
                            job_id: effect,
                            directory: directory.clone(),
                            repository_url: url.clone(),
                            branch: frozen.branch.clone(),
                            destination: destination.clone(),
                            credential: credential.map(Secret::Ref),
                        };
                        self.mark_prepare_started(job).await?;
                        let value = self
                            .command(
                                job,
                                Outgoing::new(
                                    crate::ids::derived(
                                        "git.prepare",
                                        &[effect.as_bytes(), job.id().as_bytes()],
                                    ),
                                    self.plan.machine,
                                    SerialKey::Environment(self.plan.environment),
                                    &command,
                                )
                                .map_err(EngineError::from)?,
                            )
                            .await?;
                        serde_json::from_value(value).map_err(EngineError::from)?
                    } else {
                        self.mark_prepare_started(job).await?;
                        if !directory.is_empty() {
                            self.command(
                                job,
                                Outgoing::new(
                                    if self.operation.is_some() {
                                        crate::ids::derived(
                                            "source.mkdir",
                                            &[effect.as_bytes(), job.id().as_bytes()],
                                        )
                                    } else {
                                        crate::ids::derived("source.mkdir", &[effect.as_bytes()])
                                    },
                                    self.plan.machine,
                                    SerialKey::Environment(self.plan.environment),
                                    &FsMkdir {
                                        environment_id: self.plan.environment,
                                        path: directory.clone(),
                                    },
                                )
                                .map_err(EngineError::from)?,
                            )
                            .await?;
                        }
                        SourceInfo {
                            kind: SourceKind::Directory,
                            directory: directory.clone(),
                            repository_url: None,
                            branch: None,
                            head: None,
                            state: SourceState::Ready,
                            job: None,
                            reason: Some("no_repository".to_owned()),
                        }
                    }
                } else {
                    SourceInfo {
                        kind: SourceKind::Directory,
                        directory: directory.clone(),
                        repository_url: None,
                        branch: None,
                        head: None,
                        state: SourceState::Ready,
                        job: None,
                        reason: None,
                    }
                };
                serde_json::to_value(info).map_err(EngineError::from)?
            }
            Action::Commit { body } => {
                self.command(
                    job,
                    Outgoing::new(
                        crate::ids::derived("git.commit", &[job.id().as_bytes()]),
                        self.plan.machine,
                        SerialKey::Environment(self.plan.environment),
                        body,
                    )
                    .map_err(EngineError::from)?,
                )
                .await?
            }
            Action::Worktree {
                body,
                record_days,
                max_bytes,
                ..
            } => {
                if body.job_id != job.id() && body.snapshot_id.is_some() {
                    let mut conn = self.engine.db().acquire().await?;
                    let saved = storage::snapshot(
                        &mut conn,
                        body.snapshot_id.ok_or_else(|| {
                            Failure::new("internal", "the original worktree capture disappeared")
                        })?,
                    )
                    .await?;
                    drop(conn);
                    if saved
                        .is_some_and(|saved| matches!(saved.state.as_str(), "complete" | "expired"))
                    {
                        // Ask the original native identity first. A completed worktree can
                        // return its frozen answer after capture expiry; no new capture is made.
                        let result = self
                            .command(
                                job,
                                Outgoing::new(
                                    crate::ids::derived(
                                        "git.worktree.retry",
                                        &[body.job_id.as_bytes(), job.id().as_bytes()],
                                    ),
                                    self.plan.machine,
                                    SerialKey::Environment(self.plan.environment),
                                    body,
                                )
                                .map_err(EngineError::from)?,
                            )
                            .await?;
                        job.remember(
                            &json!({ "file_result": result, "file_source_attempt": self.plan }),
                        )
                        .await?;
                        return Ok(Done::Finished);
                    }
                }
                if let Some(snapshot) = body.snapshot_id {
                    if *record_days <= 0 {
                        return Err(Failure::new(
                            "workspace.restore_unavailable",
                            "From now needs a complete recorded current state; recording is off",
                        )
                        .into());
                    }
                    let thread = self
                        .plan
                        .thread
                        .ok_or_else(|| Failure::new("internal", "the own attempt has no thread"))?;
                    let mut tx = self.engine.db().begin().await?;
                    source::begin_recorded(&mut tx, &storage::NewSnapshot { id: snapshot, thread_id: Some(thread), environment_id: self.plan.environment,
                        env_generation: self.plan.generation, label: "Current files for own attempt", selection: &json!({ "include": [body.choice.repository_dir], "exclude": [] }),
                        created_by: Some(self.plan.caller), at: job.now() }, &body.choice.repository_dir, "turn").await?;
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    let value = self
                        .command(
                            job,
                            Outgoing::new(
                                crate::ids::derived(
                                    "worktree.record-current",
                                    &[body.job_id.as_bytes(), job.id().as_bytes()],
                                ),
                                self.plan.machine,
                                SerialKey::Environment(self.plan.environment),
                                &FsRecord {
                                    environment_id: self.plan.environment,
                                    snapshot_id: snapshot,
                                    directory: body.choice.repository_dir.clone(),
                                    kind: RecordKind::Turn,
                                    record_days: *record_days,
                                    max_bytes: *max_bytes,
                                },
                            )
                            .map_err(EngineError::from)?,
                        )
                        .await?;
                    let recorded: FsRecordResult =
                        serde_json::from_value(value).map_err(EngineError::from)?;
                    if recorded.state != RecordState::Complete {
                        return Err(Failure::new(
                            "workspace.restore_unavailable",
                            "complete current files could not be recorded; choose a commit",
                        )
                        .into());
                    }
                    let hash = recorded.manifest_blob.ok_or_else(|| {
                        Failure::new("internal", "the recorded current manifest is missing")
                    })?;
                    let manifest = self.engine.read_manifest_checked(hash).await?;
                    if manifest.header().generation != i64::from(self.plan.generation)
                        || manifest
                            .entries()
                            .iter()
                            .any(|entry| entry.kind == crate::manifest::EntryKind::Other)
                    {
                        return Err(Failure::new(
                            "workspace.restore_unavailable",
                            "the current file coverage is incomplete",
                        )
                        .into());
                    }
                    let size = self
                        .engine
                        .blobs()
                        .ok_or_else(|| {
                            Failure::new("internal", "the current manifest store is unavailable")
                        })?
                        .size(&hash)
                        .await
                        .map_err(EngineError::unreadable)?
                        .ok_or_else(|| {
                            Failure::new("internal", "the current manifest is unavailable")
                        })?;
                    let mut tx = self.engine.db().begin().await?;
                    storage::register_blob(&mut tx, &hash, size, job.now()).await?;
                    if source::finish_recorded(&mut tx, snapshot, &recorded, job.now()).await? {
                        storage::pin_blobs(&mut tx, &[hash]).await?;
                    }
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                }
                self.command(
                    job,
                    Outgoing::new(
                        crate::ids::derived(
                            "git.worktree",
                            &[body.job_id.as_bytes(), job.id().as_bytes()],
                        ),
                        self.plan.machine,
                        SerialKey::Environment(self.plan.environment),
                        body,
                    )
                    .map_err(EngineError::from)?,
                )
                .await?
            }
            Action::Push { body } => {
                self.command(
                    job,
                    Outgoing::new(
                        crate::ids::derived("git.push", &[job.id().as_bytes()]),
                        self.plan.machine,
                        SerialKey::Environment(self.plan.environment),
                        body,
                    )
                    .map_err(EngineError::from)?,
                )
                .await?
            }
            Action::Restore { .. } => {
                return Err(Failure::new(
                    "internal",
                    "a restore needs its dedicated recovery runner",
                )
                .into());
            }
        };
        if let Action::Push { body } = &self.plan.action {
            let url = pull_request_url(
                &body.expected_remote_url,
                &body.branch,
                result.get("base_branch").and_then(Json::as_str),
            );
            if let Some(object) = result.as_object_mut() {
                object.remove("base_branch");
            }
            result["pull_request_url"] = json!(url);
        }
        // A provisioning parent can prepare several threads. Tie its last retained answer to
        // the exact native recipe rather than mistaking the preceding thread's answer for this one.
        let remembered = if matches!(
            self.plan.action,
            Action::Prepare { .. } | Action::Worktree { .. }
        ) {
            json!({ "file_result": result, "file_source_attempt": self.plan })
        } else {
            json!({ "file_result": result })
        };
        job.remember(&remembered).await?;
        Ok(Done::Finished)
    }

    async fn commit(&self, job: &Job) -> Result<Done, Stop> {
        check(job, &self.plan).await?;
        job.pass_commit().await?;
        self.publish(job).await
    }

    async fn publish(&self, job: &Job) -> Result<Done, Stop> {
        let publication = self
            .plan
            .thread
            .map_or_else(|| "git".to_owned(), |thread| thread.to_string());
        if let (
            Some(thread),
            Action::Prepare { source: frozen, .. } | Action::Worktree { source: frozen, .. },
        ) = (self.plan.thread, &self.plan.action)
        {
            let mut tx = self.engine.db().begin().await?;
            let saved = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
            if let Some(reason) = super::runner::abort_reason(&saved)? {
                return Err(reason);
            }
            let mut publications = saved
                .plan
                .get("file_publications")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if publications.get(&publication).and_then(Json::as_bool) == Some(true) {
                return Ok(Done::Finished);
            }
            let row = self.lock_attempt(&mut tx, &saved).await?.ok_or_else(|| {
                Failure::new(
                    "conflict.stale_preflight",
                    "The source attempt changed before its result could be published.",
                )
            })?;
            if self.provisioning_runtime.is_some()
                && saved.plan.get("file_source_attempt") != Some(&json!(self.plan))
            {
                return Err(Failure::new(
                    "internal",
                    "the source result does not belong to this provisioning attempt",
                )
                .into());
            }
            let result = saved
                .plan
                .get("file_result")
                .ok_or_else(|| Failure::new("internal", "the file result was not retained"))?;
            let mut unit = Unit::new(&mut tx, job.now());
            check_in(
                &mut *unit.conn,
                job,
                &self.plan,
                requires_write(&self.plan),
                self.provisioning_runtime,
            )
            .await?;
            let mut info: SourceInfo =
                serde_json::from_value(result.clone()).map_err(EngineError::from)?;
            info.job = Some(reference(&saved));
            let seq = unit.stamp(thread).await?;
            let worktree = match &self.plan.action {
                Action::Worktree { body, .. } => Some(&body.choice),
                _ => None,
            };
            source::set_source(
                &mut *unit.conn,
                thread,
                &with_creator_origin(
                    json!({ "info": info, "source": frozen, "prepare": true, "worktree": worktree,
                "caller": self.plan.caller, "token": self.plan.token, "native_plan": self.plan }),
                    self.plan.creator_origin.as_ref(),
                    self.plan.scheduled_input,
                    row.source.as_ref(),
                ),
                seq,
            )
            .await?;
            if let Some(segment) = row.current_segment_id {
                let by = crate::views::user_ref_json(&mut *unit.conn, self.plan.caller).await?;
                crate::ingest::add_marker(&mut unit, thread, segment, None,
                    &json!({ "marker": "source_prepared", "data": { "job_id": job.id(), "directory": info.directory,
                        "branch": info.branch, "head": info.head, "by": by } }), &format!("source:{}:{thread}", job.id())).await?;
            }
            self.append_audit(&mut *unit.conn, job, result, Some(row.org_id))
                .await?;
            publications[&publication] = json!(true);
            rows::merge_plan(
                &mut *unit.conn,
                job.id(),
                &json!({ "file_publications": publications }),
                job.now(),
            )
            .await?;
            unit.wake(thread);
            let finished = unit.finish().await?;
            self.engine.commit(tx, finished).await?;
        } else {
            let directory = match &self.plan.action {
                Action::Commit { body } => &body.path,
                Action::Push { body } => &body.path,
                _ => {
                    return Err(Failure::new(
                        "internal",
                        "the source job has no publication directory",
                    )
                    .into());
                }
            };
            if publication_authority::reconciliation_current(job, &self.plan).await? {
                self.engine
                    .refresh_git_repository(self.plan.environment, directory, job.id())
                    .await?;
            }
            let mut tx = self.engine.db().begin().await?;
            let saved = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
            if let Some(reason) = super::runner::abort_reason(&saved)? {
                return Err(reason);
            }
            let result = saved
                .plan
                .get("file_result")
                .ok_or_else(|| Failure::new("internal", "the file result was not retained"))?;
            let mut publications = saved
                .plan
                .get("file_publications")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if publications.get(&publication).and_then(Json::as_bool) != Some(true) {
                let org = match self.plan.thread {
                    Some(thread) => threads::thread(&mut tx, thread)
                        .await?
                        .map(|row| row.org_id),
                    None => None,
                };
                self.append_audit(&mut tx, job, result, org).await?;
                publications[&publication] = json!(true);
                rows::merge_plan(
                    &mut tx,
                    job.id(),
                    &json!({ "file_publications": publications }),
                    job.now(),
                )
                .await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        Ok(Done::Finished)
    }

    async fn append_audit(
        &self,
        conn: &mut Conn,
        job: &Job,
        result: &Json,
        org: Option<Id>,
    ) -> Result<(), Stop> {
        let (action, summary, details) = match &self.plan.action {
            Action::Prepare {
                source, directory, ..
            } => (
                orch_db::audit::name!("workspace.source_prepared"),
                "Prepared the thread's source",
                json!({ "repository": source.repository_url, "directory": directory, "branch": result.get("branch"), "commit": result.get("head") }),
            ),
            Action::Worktree { body, .. } => (
                orch_db::audit::name!("workspace.worktree_created"),
                "Created an own worktree",
                json!({ "directory": body.choice.directory, "branch": body.choice.branch, "base": result.get("head") }),
            ),
            Action::Commit { body } => (
                orch_db::audit::name!("git.committed"),
                "Committed the selected files",
                json!({ "commit": result.get("head"), "caller": self.plan.caller, "paths": body.paths, "directory": body.path }),
            ),
            Action::Push { body } => (
                orch_db::audit::name!("git.pushed"),
                "Pushed the reviewed commit",
                json!({ "remote": body.expected_remote_url, "branch": body.branch, "commit": body.expected_head }),
            ),
            Action::Restore { .. } => {
                return Err(
                    Failure::new("internal", "a restore has its own audit publication").into(),
                );
            }
        };
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(self.plan.caller),
            action,
            "environment",
            Some(self.plan.environment),
            summary,
        );
        entry.environment_id = Some(self.plan.environment);
        entry.node_id = self.plan.thread;
        entry.org_id = org;
        entry.history = true;
        entry.details = details;
        entry.details["job_id"] = json!(job.id());
        orch_db::audit::append(conn, &entry, &orch_db::audit::Origin::default(), job.now()).await?;
        Ok(())
    }
}

/// Publish only the first successfully activated candidate as the accepted creator's
/// environment generation. The enclosing provision commit already owns the lifecycle barrier;
/// this helper rechecks its claim, exact runtime and all source-attempt identities under lock.
/// It never advances scheduled source authority or a previously activated generation.
pub(crate) async fn publish_creator_generation(
    conn: &mut Conn,
    job: &Job,
    environment: Id,
    runtime: Id,
    machine: Id,
    previous_generation: i32,
) -> Result<usize, EngineError> {
    if previous_generation != 0 {
        return Ok(0);
    }
    let owner = job
        .lock_owned(conn)
        .await?
        .ok_or_else(|| EngineError::unreadable("The first activation claim was lost."))?;
    if owner.kind != kind::ENVIRONMENT_PROVISION
        || !owner.is_open()
        || owner.cancel_requested()
        || owner.environment_id != Some(environment)
        || owner.plan.get("mode").and_then(Json::as_str) != Some("make")
        || owner.plan.get("generation").and_then(Json::as_i64) != Some(0)
        || owner.plan.get("runtime") != Some(&json!(runtime))
    {
        return Err(EngineError::unreadable(
            "The first activation no longer owns its candidate.",
        ));
    }
    let published = environments::workspace::environment(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("The activated environment disappeared."))?;
    let candidate = environments::hosting::live_runtime(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("The activated candidate disappeared."))?;
    if published.status != "running"
        || published.machine_id != Some(machine)
        || candidate.id != runtime
        || candidate.kind != "environment"
        || candidate.environment_id != Some(environment)
        || candidate.machine_id != Some(machine)
        || candidate.state != "running"
        || candidate.generation <= 0
        || candidate.generation != published.generation
    {
        return Err(EngineError::unreadable(
            "The published first activation does not match its candidate.",
        ));
    }
    let ids = source::lock_creator_generation_threads(conn, environment).await?;
    let mut count = 0;
    for thread in ids {
        let current = threads::thread(conn, thread)
            .await?
            .ok_or_else(|| EngineError::unreadable("The activated source thread disappeared."))?;
        let Some(value) = current.source.as_ref() else {
            continue;
        };
        // Existing/scheduled definitions never acquire a fresh generation from this publication.
        let (Ok(None), Ok(Some(0)), Ok(Some(origin))) = (
            scheduled_input(value),
            accepted_creator_generation(value),
            creator_origin(value),
        ) else {
            continue;
        };
        if !matches!(
            origin.operation(),
            "ThreadCreate" | "TopicCreate.with_thread"
        ) {
            continue;
        }
        let Ok(info) =
            serde_json::from_value::<SourceInfo>(value.get("info").unwrap_or(value).clone())
        else {
            continue;
        };
        if current.environment_id != Some(environment)
            || current.workspace_dir != info.directory
            || !matches!(info.state, SourceState::Ready)
        {
            continue;
        }
        if let Some(raw) = value.get("native_plan") {
            let Ok(plan) = serde_json::from_value::<Plan>(raw.clone()) else {
                continue;
            };
            let effect = crate::ids::derived(
                "source.provision.files",
                &[owner.id.as_bytes(), thread.as_bytes()],
            );
            let same_source = match &plan.action {
                Action::Prepare {
                    effect_id,
                    source,
                    directory,
                    ..
                } => {
                    *effect_id == Some(effect)
                        && *directory == current.workspace_dir
                        && value.get("source") == Some(&json!(source))
                }
                Action::Worktree { body, source, .. } => {
                    body.job_id == effect
                        && body.choice.directory == current.workspace_dir
                        && value.get("source") == Some(&json!(source))
                        && value.get("worktree") == Some(&json!(body.choice))
                }
                _ => false,
            };
            if plan.environment != environment
                || plan.machine != machine
                || plan.generation != candidate.generation
                || plan.thread != Some(thread)
                || plan.scheduled_input.is_some()
                || plan.creator_origin.as_ref() != Some(&origin)
                || plan.caller != origin.user()
                || !same_source
                || info.job.as_ref().map(|reference| reference.id) != Some(owner.id)
                || owner
                    .plan
                    .pointer(&format!("/file_publications/{thread}"))
                    .and_then(Json::as_bool)
                    != Some(true)
            {
                continue;
            }
        } else if value.get("prepare").and_then(Json::as_bool) != Some(false)
            || value
                .get("worktree")
                .is_some_and(|worktree| !worktree.is_null())
        {
            // An explicit skipped preparation has no native recipe. Pending/default source work
            // must have the exact completed native recipe above before the unit is committed.
            continue;
        }
        source::set_creator_environment_generation(conn, thread, candidate.generation).await?;
        count += 1;
    }
    Ok(count)
}

/// The provisioning job proves the fresh workspace volume and prepares it before init.
/// A start on retained volumes uses missing children only and keeps every existing directory.
pub async fn provision_files(
    job: &Job,
    environment: Id,
    machine: Id,
    generation: i32,
    runtime: Id,
    workspace_volume: &str,
    fresh: bool,
) -> Result<Done, Stop> {
    let candidates: Vec<Id> = {
        let mut conn = job.engine().db().acquire().await?;
        // Threads are resolved on the owning environment; names never enter a weaker job result.
        sqlx::query_scalar("SELECT node_id FROM threads WHERE environment_id = $1 AND side_of IS NULL ORDER BY node_id")
            .bind(environment).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
    };
    for thread in candidates {
        let row = {
            let mut conn = job.engine().db().acquire().await?;
            threads::thread(&mut conn, thread)
                .await?
                .ok_or_else(|| Failure::new("not_found", "a source thread disappeared"))?
        };
        let Some(value) = &row.source else { continue };
        let info: SourceInfo = serde_json::from_value(value.get("info").unwrap_or(value).clone())
            .map_err(EngineError::from)?;
        if !matches!(info.state, SourceState::Pending | SourceState::Preparing) {
            continue;
        }
        let frozen: RepositorySource =
            serde_json::from_value(value.get("source").cloned().unwrap_or_else(|| {
                json!({
            "repository_url": info.repository_url, "branch": info.branch, "secret_name": null })
            }))
            .map_err(EngineError::from)?;
        let worktree: Option<orch_api_contract::source::WorktreeChoice> = value
            .get("worktree")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(EngineError::from)?
            .flatten();
        let mut destination = SourceDestination::MissingDirectory;
        if fresh && row.workspace_dir.is_empty() && !workspace_volume.is_empty() {
            let plan = job.plan().await?;
            let owner = plan
                .get("source_root_owner")
                .cloned()
                .and_then(|value| serde_json::from_value::<Id>(value).ok());
            if owner.is_none() || owner == Some(thread) {
                job.remember(&json!({ "source_root_owner": thread }))
                    .await?;
                destination = SourceDestination::OwnedEmptyRoot {
                    provisioning_job_id: job.id(),
                    workspace_volume: workspace_volume.to_owned(),
                    generation: i64::from(generation),
                };
            }
        }
        let effect = crate::ids::derived(
            "source.provision.files",
            &[job.id().as_bytes(), thread.as_bytes()],
        );
        let action = if let Some(choice) = worktree {
            let mut conn = job.engine().db().acquire().await?;
            let record_days = get::<i64>(
                &mut conn,
                Place::Environment(environment),
                keys::WORKSPACE_RECORD_DAYS,
            )
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| EngineError::unreadable("recording days are unset"))?;
            let max_bytes = get::<i64>(
                &mut conn,
                Place::Environment(environment),
                keys::WORKSPACE_RECORD_MAX_BYTES,
            )
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| EngineError::unreadable("recording bytes are unset"))?;
            let snapshot_id = matches!(
                choice.from,
                orch_api_contract::source::WorktreeFrom::Now { .. }
            )
            .then(|| crate::ids::derived("worktree.current-files", &[effect.as_bytes()]));
            Action::Worktree {
                body: GitWorktree {
                    environment_id: environment,
                    job_id: effect,
                    choice,
                    snapshot_id,
                },
                source: frozen,
                record_days,
                max_bytes,
            }
        } else {
            let mut conn = job.engine().db().acquire().await?;
            let secret = credential_if_present(&mut conn, row.topic_id, &frozen).await?;
            Action::Prepare {
                directory: row.workspace_dir,
                source: frozen,
                credential: secret,
                destination,
                effect_id: Some(effect),
            }
        };
        let mut preparing = Preparing {
            engine: job.engine().clone(),
            provisioning_runtime: Some(runtime),
            operation: None,
            plan: Plan {
                environment,
                generation,
                machine,
                thread: Some(thread),
                caller: value
                    .get("caller")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or(row.owner_id),
                token: value
                    .get("token")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok()),
                creator_origin: creator_origin(value)?,
                scheduled_input: scheduled_input(value)?,
                action,
            },
        };
        if let Some(previous) = value.get("native_plan") {
            let previous: Plan =
                serde_json::from_value(previous.clone()).map_err(EngineError::from)?;
            let same_effect = match &previous.action {
                Action::Prepare { effect_id, .. } => *effect_id == Some(effect),
                Action::Worktree { body, .. } => body.job_id == effect,
                _ => false,
            };
            if same_effect {
                let same_choice = match (&previous.action, &preparing.plan.action) {
                    (
                        Action::Prepare {
                            directory: old_dir,
                            source: old_source,
                            destination: old_destination,
                            ..
                        },
                        Action::Prepare {
                            directory,
                            source,
                            destination,
                            ..
                        },
                    ) => {
                        old_dir == directory
                            && json!(old_source) == json!(source)
                            && old_destination == destination
                    }
                    (
                        Action::Worktree {
                            body: old_body,
                            source: old_source,
                            ..
                        },
                        Action::Worktree { body, source, .. },
                    ) => old_body.choice == body.choice && json!(old_source) == json!(source),
                    _ => false,
                };
                if previous.environment != environment
                    || previous.machine != machine
                    || previous.generation != generation
                    || previous.thread != Some(thread)
                    || previous.caller != preparing.plan.caller
                    || previous.token != preparing.plan.token
                    || previous.creator_origin != preparing.plan.creator_origin
                    || previous.scheduled_input != preparing.plan.scheduled_input
                    || !same_choice
                {
                    return Err(Failure::new(
                        "conflict.stale_preflight",
                        "The frozen source recipe changed during provisioning.",
                    )
                    .into());
                }
                // Credentials, recording limits and capture identity belong to the accepted
                // recipe, even if their current defaults or secret availability have changed.
                preparing.plan = previous;
            }
        }
        // Preserve the native effect before the first command so an explicit Retry reconciles
        // this same stage, branch and capture rather than accepting partially installed files.
        let mut saved = value.clone();
        saved["native_plan"] = serde_json::to_value(&preparing.plan).map_err(EngineError::from)?;
        let mut tx = job.engine().db().begin().await?;
        let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if let Some(reason) = super::runner::abort_reason(&owner)? {
            return Err(reason);
        }
        let current = threads::lock(&mut tx, thread)
            .await?
            .ok_or_else(|| Failure::new("not_found", "a source thread disappeared"))?;
        let candidate = matching_target(&mut tx, &preparing.plan, Some((&owner, runtime))).await?;
        if current.source.as_ref() != Some(value)
            || current.environment_id != Some(environment)
            || current.workspace_dir != info.directory
            || candidate.is_none()
            || accepted_creator_generation(value)?.is_some_and(|accepted| {
                accepted != generation
                    && !(accepted == 0 && candidate.as_ref().is_some_and(|env| env.generation == 0))
            })
        {
            return Err(Failure::new(
                "conflict.stale_preflight",
                "The source changed before provisioning could prepare it.",
            )
            .into());
        }
        let mut unit = Unit::new(&mut tx, job.now());
        let seq = unit.stamp(thread).await?;
        source::set_source(&mut *unit.conn, thread, &saved, seq).await?;
        let finished = unit.finish().await?;
        job.engine().commit(tx, finished).await?;
        preparing.effect(job).await?;
        preparing.publish(job).await?;
    }
    Ok(Done::Finished)
}

/// Terminal provisioning failure owns inline source work through its current parent claim.
/// Recover the durable recipe before the provisioning undo retires its candidate runtime.
pub(crate) async fn undo_provision_sources(job: &Job, why: &Stop) -> Result<(), EngineError> {
    let attempts = {
        let mut tx = job.engine().db().begin().await?;
        let Some(owner) = job.lock_owned(&mut tx).await? else {
            return Ok(());
        };
        let Some(environment) = owner
            .environment_id
            .filter(|_| owner.kind == kind::ENVIRONMENT_PROVISION)
        else {
            return Ok(());
        };
        let Some(runtime) = owner
            .plan
            .get("runtime")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
        else {
            return Ok(());
        };
        let values: Vec<Json> = sqlx::query_scalar(
            "SELECT source FROM threads WHERE environment_id = $1
            AND source #>> '{info,state}' IN ('pending','preparing')
            AND jsonb_typeof(source -> 'native_plan') = 'object' ORDER BY node_id",
        )
        .bind(environment)
        .fetch_all(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
        let mut attempts = Vec::new();
        for value in values {
            let plan: Plan = serde_json::from_value(value["native_plan"].clone())?;
            let preparing = Preparing {
                engine: job.engine().clone(),
                plan,
                provisioning_runtime: Some(runtime),
                operation: None,
            };
            if preparing.lock_attempt(&mut tx, &owner).await?.is_some() {
                attempts.push(preparing);
            }
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        attempts
    };
    for preparing in attempts {
        if let Undone::Partly(detail) = preparing.undo(job, why).await {
            tracing::debug!(job = %job.id(), %detail, "provisioning source cleanup retained files or metadata");
        }
    }
    Ok(())
}

#[async_trait]
impl Phased for Preparing {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "validate" => {
                check(job, &self.plan).await?;
                Ok(Done::Finished)
            }
            "apply" => self.apply(job).await,
            "verify" => {
                check(job, &self.plan).await?;
                Ok(Done::Finished)
            }
            "commit" => self.commit(job).await,
            _ => Err(Failure::new(
                "internal",
                "The saved file operation cannot continue. Please try it again.",
            )
            .into()),
        }
    }
    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
        let stored_reason = if matches!(
            self.plan.action,
            Action::Prepare { .. } | Action::Worktree { .. }
        ) {
            match self.owns_attempt(job).await {
                Ok(true) => {}
                Ok(false) => return Undone::Partly("the source attempt is no longer owned by this runner; its files and result were kept".to_owned()),
                Err(_) => return Undone::Partly("source failure metadata awaits recovery".to_owned()),
            }
            // Direct users of this phase share the runner's durable undo direction too.
            let Ok(Some((_, reason))) = job.begin_undo(why).await else {
                return Undone::Partly("source cleanup awaits its owned undo intent".to_owned());
            };
            reason
        } else {
            why.clone()
        };
        let why = &stored_reason;
        let mut retry_plan = self.plan.clone();
        let mut cleanup = None;
        if let Action::Prepare {
            directory,
            source,
            destination,
            effect_id,
            ..
        } = &self.plan.action
            && let Some(url) = &source.repository_url
        {
            let cleaned = async {
                let original = GitPrepare {
                    environment_id: self.plan.environment,
                    job_id: effect_id.unwrap_or(job.id()),
                    directory: directory.clone(),
                    repository_url: url.clone(),
                    branch: source.branch.clone(),
                    destination: destination.clone(),
                    credential: None,
                };
                let command = Outgoing::new(
                    crate::ids::derived(
                        "git.prepare.cleanup",
                        &[original.job_id.as_bytes(), job.id().as_bytes()],
                    ),
                    self.plan.machine,
                    SerialKey::Environment(self.plan.environment),
                    &orch_link_contract::source::GitPrepareCleanup { original },
                )
                .map_err(EngineError::from)?;
                let result: orch_link_contract::source::GitPrepareCleanupResult =
                    serde_json::from_value(job.command_to_undo(command).await?)
                        .map_err(EngineError::from)?;
                job.remember(&json!({ "file_source_cleanup": result }))
                    .await?;
                Ok::<_, Stop>(orch_link_contract::source::GitWorktreeCleanupResult {
                    removed: result.removed,
                    reason: result.reason,
                })
            }
            .await;
            if cleaned.as_ref().is_ok_and(|result| result.removed)
                && let Action::Prepare { effect_id, .. } = &mut retry_plan.action
            {
                *effect_id = None;
            }
            cleanup = Some(cleaned);
        }
        if let Action::Worktree { body, .. } = &self.plan.action {
            let cleaned = async {
                let command = Outgoing::new(
                    crate::ids::derived(
                        "git.worktree.cleanup",
                        &[body.job_id.as_bytes(), job.id().as_bytes()],
                    ),
                    self.plan.machine,
                    SerialKey::Environment(self.plan.environment),
                    &orch_link_contract::source::GitWorktreeCleanup {
                        original: body.clone(),
                    },
                )
                .map_err(EngineError::from)?;
                let result: orch_link_contract::source::GitWorktreeCleanupResult =
                    serde_json::from_value(job.command_to_undo(command).await?)
                        .map_err(EngineError::from)?;
                job.remember(&json!({ "file_worktree_cleanup": result }))
                    .await?;
                Ok::<_, Stop>(result)
            }
            .await;
            if let Ok(result) = &cleaned
                && result.removed
                && let Action::Worktree { body, .. } = &mut retry_plan.action
            {
                body.job_id = Id::nil();
                body.snapshot_id = None;
            }
            cleanup = Some(cleaned);
        }
        let choice = match &self.plan.action {
            Action::Prepare {
                source, directory, ..
            } => Some((source, directory, None)),
            Action::Worktree { source, body, .. } => {
                Some((source, &body.choice.directory, Some(&body.choice)))
            }
            _ => None,
        };
        if let (Some(thread), Some((frozen, directory, worktree))) = (self.plan.thread, choice) {
            let saved = async {
                let mut tx = self.engine.db().begin().await?;
                let Some(owner) = job.lock_owned(&mut tx).await? else { return Ok::<_, EngineError>(()); };
                let Some(row) = self.lock_attempt(&mut tx, &owner).await? else { return Ok(()); };
                let retained = if self.provisioning_runtime.is_none() || owner.plan.get("file_source_attempt") == Some(&json!(self.plan)) {
                    owner.plan.get("file_result")
                } else { row.source.as_ref().and_then(|source| source.get("native_result")) };
                let native_result = if cleanup.as_ref().is_some_and(|result| result.as_ref().is_ok_and(|result| result.removed)) { Json::Null }
                    else { retained.cloned().unwrap_or(Json::Null) };
                let native_started = !cleanup.as_ref().is_some_and(|result| result.as_ref().is_ok_and(|result| result.removed))
                    && (owner.plan.get("file_prepare_started") == Some(&json!(self.plan))
                        || native_result.get("kind").and_then(Json::as_str).is_some_and(|kind| matches!(kind, "repository" | "worktree"))
                        || row.source.as_ref().and_then(|source| source.get("native_started")).and_then(Json::as_bool) == Some(true));
                let job_ref = reference(&owner);
                let mut unit = Unit::new(&mut tx, job.now());
                let info = SourceInfo { kind: if worktree.is_some() { SourceKind::Worktree } else if frozen.repository_url.is_some() { SourceKind::Repository } else { SourceKind::Directory }, directory: directory.clone(), repository_url: frozen.repository_url.clone(),
                    branch: frozen.branch.clone(), head: None, state: SourceState::Failed, job: Some(job_ref), reason: Some(match why {
                        Stop::Failed(failure) if matches!(failure.code.as_str(), "workspace.remote_write_disabled" | "policy.remote_write_disabled") => "workspace.remote_write_disabled",
                        _ => "workspace.source_failed",
                    }.to_owned()) };
                let seq = unit.stamp(thread).await?;
                source::set_source(&mut *unit.conn, thread, &with_creator_origin(json!({ "info": info, "source": frozen, "prepare": true, "worktree": worktree,
                    "caller": self.plan.caller, "token": self.plan.token, "credential_home": row.source.as_ref().and_then(|source|source.get("credential_home")).cloned().unwrap_or_else(||json!(row.topic_id)),
                    "native_plan": retry_plan, "native_result": native_result, "native_started": native_started }),
                    self.plan.creator_origin.as_ref(), self.plan.scheduled_input, row.source.as_ref()), seq).await?;
                unit.wake(thread);
                let finished = unit.finish().await?; self.engine.commit(tx, finished).await?; Ok::<_, EngineError>(())
            }.await;
            if saved.is_err() {
                return Undone::Partly("source failure metadata awaits recovery".to_owned());
            }
        }
        match cleanup {
            Some(Ok(result)) if result.removed => Undone::Everything,
            Some(Ok(result)) => Undone::Partly(result.reason.unwrap_or_else(|| "prepared files are kept".to_owned())),
            Some(Err(_)) => Undone::Partly("owned source cleanup awaits its original unit; files and any branch are kept".to_owned()),
            None => Undone::Partly("any job-owned source stage is retained for recovery; existing files were not replaced".to_owned()),
        }
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        job.plan().await.ok()?.get("file_result").cloned()
    }
}

#[cfg(test)]
mod attempt_tests;

#[cfg(test)]
mod creator_generation_tests;

#[cfg(test)]
mod publication_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::unwrap_used, reason = "a control fails by panicking")]
    async fn an_existing_environment_generation_is_frozen_before_the_source_plan_is_built() {
        use orch_access_contract::key_scope::{KeyAction, KeyFacet, KeyTargetKind};
        for replaced in [false, true] {
            let world = crate::test_common::World::observed().await;
            let thread = world.thread("Accepted source generation").await;
            let engine = world.engine();
            engine.stop();
            let mut tx = world.pool.begin().await.unwrap();
            let environment = environments::workspace::environment(&mut tx, world.environment)
                .await
                .unwrap()
                .unwrap();
            let parent = crate::operation_scope::targets::node(&mut tx, world.topic)
                .await
                .unwrap();
            let caller = Caller::session(world.ada)
                .admit_facets(&[KeyFacet {
                    operation: "ThreadCreate".into(),
                    action: KeyAction::Write,
                    resource_kind: KeyTargetKind::Object(ObjectKind::Thread),
                    target: parent,
                }])
                .unwrap()
                .into_caller();
            let mut unit = Unit::new(&mut tx, world.clock.now());
            freeze(
                &mut unit,
                &caller,
                thread,
                world.environment,
                "web",
                Some(true),
                None,
            )
            .await
            .unwrap();
            unit.finish().await.unwrap();
            tx.commit().await.unwrap();
            if replaced {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(world.environment)
                    .execute(world.pool.pool())
                    .await
                    .unwrap();
            }
            let mut conn = world.pool.acquire().await.unwrap();
            let frozen = threads::thread(&mut conn, thread).await.unwrap().unwrap();
            assert_eq!(
                frozen.source.as_ref().unwrap()["creator_environment_generation"],
                json!(environment.generation)
            );
            drop(conn);
            let result = engine.prepare_pending_source(&frozen).await;
            assert_eq!(result.is_ok(), !replaced);
            let mut conn = world.pool.acquire().await.unwrap();
            let jobs = rows::open_of_environment(&mut conn, world.environment)
                .await
                .unwrap();
            assert_eq!(jobs.len(), usize::from(!replaced));
            assert_eq!(world.commands.issued_of("fs.stat"), 0);
            assert_eq!(world.commands.issued_of("git.prepare"), 0);
            assert_eq!(world.commands.issued_of("git.worktree"), 0);
        }
    }

    #[test]
    fn request_links_encode_branches_and_unknown_hosts_stay_manual() {
        assert_eq!(
            pull_request_url("git@github.com:team/repo.git", "fix/a#b", Some("main")).as_deref(),
            Some("https://github.com/team/repo/compare/main...fix%2Fa%23b?quick_pull=1")
        );
        assert_eq!(
            pull_request_url("https://gitlab.com/team/sub/repo.git", "fix/next", None).as_deref(),
            Some(
                "https://gitlab.com/team/sub/repo/merge_requests/new?merge_request%5Bsource_branch%5D=fix%2Fnext"
            )
        );
        assert!(pull_request_url("https://github.com/team/repo.git", "work", None).is_none());
        assert!(
            pull_request_url("https://github.com/team/repo.git", "work", Some("work")).is_none()
        );
        assert!(
            pull_request_url("https://code.example.test/team/repo.git", "work", None).is_none()
        );
        assert!(
            pull_request_url(
                "https://token@github.com/team/repo.git",
                "work",
                Some("main")
            )
            .is_none()
        );
        assert!(
            pull_request_url(
                "https://github.com/team/repo.git?token=secret",
                "work",
                Some("main")
            )
            .is_none()
        );
    }
}
