//! Exact native copy operands at queue and dispatch, using the existing copy prerequisites.
use super::*;
use orch_authz::Principal;
use orch_db::Conn;
use orch_types::Clock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Environment {
    id: Id,
    generation: i32,
    machine: Option<Id>,
    root: String,
}
impl Environment {
    fn of(row: &environment_rows::EnvironmentRow) -> Self {
        Self {
            id: row.id,
            generation: row.generation,
            machine: row.machine_id,
            root: row.workspace_root.clone(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Copy {
    source: Environment,
    target: Environment,
    directory: String,
    snapshot: Option<(Id, orch_types::Hash)>,
}
fn refused() -> EngineError {
    EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused))
}
fn native(error: Stop) -> EngineError {
    match error {
        Stop::Failed(error) if error.code == INTERNAL => EngineError::unreadable(error.message),
        _ => refused(),
    }
}
impl Copy {
    pub(super) async fn selected(
        conn: &mut Conn,
        row: &JobRow,
        recipe: &Recipe,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        let selected = recipe.forkplan.files_at.as_ref();
        let source_id = selected
            .map(|snapshot| snapshot.environment.id)
            .or_else(|| recipe.source_execution.environment())
            .ok_or_else(refused)?;
        let target_id = recipe
            .to_environment
            .or_else(|| member::<Id>(&row.plan, made::ENVIRONMENT))
            .ok_or_else(refused)?;
        let source = environment_rows::environment(conn, source_id)
            .await?
            .ok_or_else(refused)?;
        let target = environment_rows::environment(conn, target_id)
            .await?
            .ok_or_else(refused)?;
        let thread = threads::thread(conn, recipe.source_thread)
            .await?
            .ok_or_else(refused)?;
        if selected.is_none()
            && recipe.forkplan.worktree.is_none()
            && thread.environment_id != Some(source_id)
        {
            return Err(refused());
        }
        let snapshot = if let Some(saved) = selected {
            let actual = orch_db::queries::storage::snapshot(conn, saved.id)
                .await?
                .filter(|actual| {
                    actual.state == "complete"
                        && actual.environment_id == source_id
                        && actual.expires_at.is_none_or(|until| until > clock.now())
                })
                .ok_or_else(refused)?;
            Some((actual.id, actual.manifest_blob.ok_or_else(refused)?))
        } else {
            None
        };
        Ok(Self {
            source: Environment::of(&source),
            target: Environment::of(&target),
            directory: thread.workspace_dir,
            snapshot,
        })
    }
    pub(super) async fn lock(&self, conn: &mut Conn) -> Result<(), EngineError> {
        for id in [self.source.id, self.target.id]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
        {
            hosting_rows::lock_environment(conn, id)
                .await?
                .ok_or_else(refused)?;
        }
        Ok(())
    }
    pub(super) async fn current(
        &self,
        conn: &mut Conn,
        row: &JobRow,
        recipe: &Recipe,
        principal: &Principal,
        clock: &dyn Clock,
    ) -> Result<(), EngineError> {
        self.lock(conn).await?;
        if *self != Self::selected(conn, row, recipe, clock).await? {
            return Err(refused());
        }
        crate::fork_source::copy_grants(
            conn,
            principal,
            self.source.id,
            self.target.id,
            "current permission no longer permits this file copy",
        )
        .await
        .map_err(native)?;
        if !orch_db::queries::environments::workspace::environment(conn, self.target.id)
            .await?
            .is_some_and(|row| row.remote_write)
        {
            return Err(refused());
        }
        Ok(())
    }
    pub(super) fn covers(
        &self,
        recipe: &Recipe,
        job: Id,
        command: &Outgoing,
    ) -> Result<bool, EngineError> {
        use orch_link_contract::commands::CommandKind;
        use orch_link_contract::source::{
            FsRecord, FsRecordRead, GitWorktree, GitWorktreeSnapshot, GitWorktreeStandalone,
        };
        let (environment, source) = match command.kind {
            CommandKind::FsSnapshot => {
                let body: FsSnapshot = serde_json::from_value(command.body.clone())?;
                if body.job_id != job {
                    return Ok(false);
                }
                (body.environment_id, true)
            }
            CommandKind::GitWorktreeSnapshot => {
                let body: GitWorktreeSnapshot = serde_json::from_value(command.body.clone())?;
                if body.job_id != job || body.directory != self.directory {
                    return Ok(false);
                }
                (body.environment_id, true)
            }
            CommandKind::FsMaterialize => {
                let body: FsMaterialize = serde_json::from_value(command.body.clone())?;
                if body.job_id != Some(job)
                    || !matches!(
                        body.purpose,
                        MaterializePurpose::Move | MaterializePurpose::Exact
                    )
                {
                    return Ok(false);
                }
                (body.environment_id, false)
            }
            CommandKind::GitWorktreeStandalone => {
                let body: GitWorktreeStandalone = serde_json::from_value(command.body.clone())?;
                if body.job_id != job {
                    return Ok(false);
                }
                (body.environment_id, false)
            }
            CommandKind::FsRecord => {
                let body: FsRecord = serde_json::from_value(command.body.clone())?;
                if recipe.forkplan.worktree.is_none()
                    || body.snapshot_id
                        != crate::ids::derived("fork.worktree.current-files", &[job.as_bytes()])
                {
                    return Ok(false);
                }
                (body.environment_id, true)
            }
            CommandKind::FsRecordRead => {
                let body: FsRecordRead = serde_json::from_value(command.body.clone())?;
                if !self.snapshot.is_some_and(|(id, _)| id == body.snapshot_id)
                    || body.path.is_some()
                    || !body.export
                {
                    return Ok(false);
                }
                (body.environment_id, true)
            }
            CommandKind::GitWorktree => {
                let body: GitWorktree = serde_json::from_value(command.body.clone())?;
                if body.job_id != job
                    || !recipe
                        .forkplan
                        .worktree
                        .as_ref()
                        .is_some_and(|plan| plan.choice == body.choice)
                {
                    return Ok(false);
                }
                (body.environment_id, false)
            }
            _ => return Ok(false),
        };
        let expected = if source { &self.source } else { &self.target };
        Ok(environment == expected.id
            && expected.machine == Some(command.machine)
            && command.key == SerialKey::Environment(expected.id))
    }
}
