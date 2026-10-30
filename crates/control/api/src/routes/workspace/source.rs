//! Guarded source preparation and reviewed Git publication through the existing file jobs.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, Registry};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::ThroughThreadQuery;
use orch_api_contract::source::{
    EnvironmentGitCommit, EnvironmentGitCommitBody, EnvironmentGitPush, EnvironmentGitPushBody,
    EnvironmentSourcePrepare, EnvironmentSourcePrepareBody, RepositorySource,
};
use orch_api_contract::workspace::Job;
use orch_authz::{Policy, Principal};
use orch_db::{
    Conn,
    queries::{identity, threads},
};
use orch_engine::jobs::{
    runner::MakeError,
    source::{self, Action, Plan},
};
use orch_link_contract::commands::Secret;
use orch_link_contract::source::{GitAuthor, GitCommit, GitPush};
use orch_types::{Id, new_id};

/// Binds the source and publication requests declared by the canonical API.
pub fn mount(registry: &mut Registry) {
    registry
        .on(EnvironmentSourcePrepare, prepare)
        .on(EnvironmentGitCommit, commit)
        .on(EnvironmentGitPush, push);
}

pub(crate) fn make_error(error: MakeError) -> ApiError {
    match error {
        MakeError::Running(job) => ApiError::new(
            ErrorCode::ConflictExists,
            "a job already uses this environment",
        )
        .with("job", source::reference(&job)),
        MakeError::NoEnvironment => ApiError::not_found(),
        MakeError::Db(error) => error.into(),
    }
}

async fn require(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    permission: Permission,
) -> ApiResult<()> {
    authorization::require_resource(
        conn,
        principal,
        ResourceKind::Environment,
        environment,
        permission,
        Policy::default(),
    )
    .await
}

async fn complete(
    ctx: &Ctx,
    mut tx: orch_db::Tx,
    plan: Plan,
    now: orch_types::Time,
) -> ApiResult<Job> {
    let row = source::begin(&mut tx, plan, now.get())
        .await
        .map_err(make_error)?;
    let shown = crate::routes::environments::jobs::view(&mut tx, &row).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app.engine.kick();
    Ok(shown)
}

async fn prepare(ctx: Ctx, (): (), body: EnvironmentSourcePrepareBody) -> ApiResult<Job> {
    let environment = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require(
        &mut tx,
        guard.principal(),
        environment,
        Permission::EnvironmentManage,
    )
    .await?;
    let door = super::door::open(&ctx, &mut tx, Some(body.thread_id)).await?;
    door.may_write()?;
    let machine = super::live::require_live(&ctx, &mut tx, &door.env).await?;
    let row = threads::thread(&mut tx, body.thread_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let frozen: RepositorySource = match &body.source {
        Some(accepted) => {
            let current = source::resolved(&mut tx, row.node_id)
                .await
                .map_err(ApiError::internal)?;
            if accepted != &current {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the resolved source changed before it was accepted",
                ));
            }
            current
        }
        None => match row.source.as_ref().and_then(|source| source.get("source")) {
            Some(value) => serde_json::from_value(value.clone()).map_err(ApiError::internal)?,
            None => source::resolved(&mut tx, row.node_id)
                .await
                .map_err(ApiError::internal)?,
        },
    };
    let worktree: Option<orch_api_contract::source::WorktreeChoice> = row
        .source
        .as_ref()
        .and_then(|source| source.get("worktree"))
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)?
        .flatten();
    let action = if let Some(choice) = worktree {
        let record_days = orch_settings::get::<i64>(
            &mut tx,
            orch_settings::Place::Environment(environment),
            orch_settings_contract::registry::keys::WORKSPACE_RECORD_DAYS,
        )
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("recording days are unset"))?;
        let max_bytes = orch_settings::get::<i64>(
            &mut tx,
            orch_settings::Place::Environment(environment),
            orch_settings_contract::registry::keys::WORKSPACE_RECORD_MAX_BYTES,
        )
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("recording bytes are unset"))?;
        Action::Worktree {
            body: orch_link_contract::source::GitWorktree {
                environment_id: environment,
                job_id: Id::nil(),
                choice,
                snapshot_id: None,
            },
            source: frozen,
            record_days,
            max_bytes,
        }
    } else {
        let credential = source::credential(&mut tx, row.topic_id, &frozen)
            .await
            .ok()
            .flatten();
        Action::Prepare {
            directory: row.workspace_dir,
            source: frozen,
            credential,
            destination: orch_api_contract::source::SourceDestination::MissingDirectory,
            effect_id: None,
        }
    };
    let action = if body.source.is_none() {
        match row
            .source
            .as_ref()
            .and_then(|source| source.get("native_plan"))
        {
            Some(value) => {
                let previous: Plan =
                    serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
                if previous.environment != environment
                    || previous.generation != door.env.generation
                    || previous.machine != machine
                {
                    return Err(ApiError::new(
                        ErrorCode::ConflictStaleRevision,
                        "the original source environment changed; accept its current source to start a new preparation",
                    ));
                }
                match previous.action {
                    Action::Prepare {
                        directory,
                        source: frozen,
                        mut credential,
                        destination,
                        effect_id,
                    } => {
                        if credential.is_none() && frozen.secret_name.is_some() {
                            let home = row
                                .source
                                .as_ref()
                                .and_then(|source| source.get("credential_home"))
                                .cloned()
                                .and_then(|value| serde_json::from_value::<Id>(value).ok());
                            if home != Some(row.topic_id) {
                                return Err(ApiError::new(
                                    ErrorCode::ConflictStaleRevision,
                                    "the original source credential scope changed; accept the current source before preparing it",
                                ));
                            }
                            credential = source::credential(&mut tx, row.topic_id, &frozen)
                                .await
                                .ok()
                                .flatten();
                        }
                        Action::Prepare {
                            directory,
                            source: frozen,
                            credential,
                            destination,
                            effect_id,
                        }
                    }
                    action @ Action::Worktree { .. } => action,
                    _ => {
                        return Err(ApiError::internal(
                            "the retained source effect has the wrong kind",
                        ));
                    }
                }
            }
            None => action,
        }
    } else {
        action
    };
    let plan = Plan {
        environment,
        generation: door.env.generation,
        machine,
        thread: Some(row.node_id),
        caller: guard.user().id(),
        token: guard
            .principal()
            .caller()
            .grant()
            .and_then(|grant| grant.token()),
        action,
    };
    complete(&ctx, tx, plan, guard.now()).await
}

async fn commit(
    ctx: Ctx,
    query: ThroughThreadQuery,
    body: EnvironmentGitCommitBody,
) -> ApiResult<Job> {
    let path = super::paths::clean("path", &body.path, true)?;
    let paths = body
        .paths
        .iter()
        .map(|path| super::paths::clean("paths", path, false))
        .collect::<ApiResult<Vec<_>>>()?;
    if paths.is_empty() || body.message.trim().is_empty() || body.message.contains('\0') {
        return Err(ApiError::invalid(
            "message",
            "select changed paths and give a commit message",
        ));
    }
    let environment = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require(
        &mut tx,
        guard.principal(),
        environment,
        Permission::FilesManage,
    )
    .await?;
    let door = super::door::open(&ctx, &mut tx, query.thread).await?;
    door.may_write()?;
    let machine = super::live::require_live(&ctx, &mut tx, &door.env).await?;
    let person = identity::user_by_id(&mut tx, guard.user().id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let email = if person.email_confirmed() {
        person.email.clone().unwrap_or_default()
    } else {
        format!("{}@orch.invalid", person.id)
    };
    let command = GitCommit {
        environment_id: environment,
        job_id: new_id(),
        path,
        paths,
        message: body.message,
        expected_head: body.expected_head,
        expected_status: body.expected_status,
        author: GitAuthor {
            name: person.display_name,
            email,
        },
    };
    let plan = Plan {
        environment,
        generation: door.env.generation,
        machine,
        thread: door.thread,
        caller: guard.user().id(),
        token: guard
            .principal()
            .caller()
            .grant()
            .and_then(|grant| grant.token()),
        action: Action::Commit { body: command },
    };
    complete(&ctx, tx, plan, guard.now()).await
}

async fn push(ctx: Ctx, query: ThroughThreadQuery, body: EnvironmentGitPushBody) -> ApiResult<Job> {
    let path = super::paths::clean("path", &body.path, true)?;
    if !orch_settings_contract::registry::is_git_branch(&body.branch)
        || !orch_settings_contract::registry::is_repository_address(&body.expected_remote_url)
    {
        return Err(ApiError::invalid(
            "branch",
            "select a safe remote and a valid Git branch",
        ));
    }
    let environment = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require(
        &mut tx,
        guard.principal(),
        environment,
        Permission::FilesManage,
    )
    .await?;
    let door = super::door::open(&ctx, &mut tx, query.thread).await?;
    door.may_write()?;
    let machine = super::live::require_live(&ctx, &mut tx, &door.env).await?;
    let mut credential = None;
    let mut original = None;
    if let Some(thread) = door.thread
        && let Some(row) = threads::thread(&mut tx, thread).await?
        && let Some(value) = row.source.as_ref().and_then(|source| source.get("source"))
    {
        let frozen: RepositorySource =
            serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
        if frozen.repository_url.as_deref().is_some_and(|original| {
            original == body.expected_remote_url
                || url::Url::parse(original)
                    .ok()
                    .zip(url::Url::parse(&body.expected_remote_url).ok())
                    .is_some_and(|(original, reviewed)| original == reviewed)
        }) {
            credential = source::credential(&mut tx, row.topic_id, &frozen)
                .await
                .map_err(ApiError::internal)?
                .map(Secret::Ref);
            original = frozen.repository_url;
        }
    }
    let command = GitPush {
        environment_id: environment,
        job_id: new_id(),
        path,
        remote: body.remote,
        expected_remote_url: body.expected_remote_url,
        branch: body.branch,
        expected_head: body.expected_head,
        set_upstream: body.set_upstream.unwrap_or(false),
        credential,
        credential_repository_url: original,
    };
    let plan = Plan {
        environment,
        generation: door.env.generation,
        machine,
        thread: door.thread,
        caller: guard.user().id(),
        token: guard
            .principal()
            .caller()
            .grant()
            .and_then(|grant| grant.token()),
        action: Action::Push { body: command },
    };
    complete(&ctx, tx, plan, guard.now()).await
}
