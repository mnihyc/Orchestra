//! Saved start-plan edits become ordinary conditional workspace writes before a first session.

use super::*;
use crate::commands::Outcome;
use orch_db::queries::storage;
use orch_link_contract::commands::{ExpectedHash, FsWrite};
use orch_types::Hash;

pub fn validate(edits: &[WorkspaceFileEdit]) -> Result<(), &'static str> {
    let mut paths = std::collections::BTreeSet::new();
    for edit in edits {
        if edit.path.is_empty()
            || !crate::binding::directory_is_valid(&edit.path)
            || !paths.insert(&edit.path)
        {
            return Err("Choose distinct file paths relative to the workspace directory");
        }
    }
    Ok(())
}

fn directory(plan: &Plan) -> Option<&str> {
    match &plan.action {
        Action::Prepare { directory, .. } => Some(directory),
        Action::Worktree { body, .. } => Some(&body.choice.directory),
        _ => None,
    }
}

fn write(plan: &Plan, edit: &WorkspaceFileEdit) -> Option<FsWrite> {
    let root = directory(plan)?.trim_end_matches('/');
    Some(FsWrite {
        work: None,
        environment_id: plan.environment,
        path: if root.is_empty() {
            edit.path.clone()
        } else {
            format!("{root}/{}", edit.path)
        },
        blob: Hash::of(edit.text.as_bytes()),
        mode: None,
        expected_hash: Some(
            edit.expected_hash
                .map_or(ExpectedHash::Absent, ExpectedHash::Is),
        ),
    })
}

pub(super) fn command_matches(plan: &Plan, job: Id, body: &Json) -> Result<bool, EngineError> {
    if validate(&plan.workspace_edits).is_err() {
        return Ok(false);
    }
    let actual: FsWrite = match serde_json::from_value(body.clone()) {
        Ok(value) => value,
        Err(_) => return Ok(false),
    };
    let Some(work) = actual
        .work
        .filter(|work| work.job_id == job && work.phase == "files")
    else {
        return Ok(false);
    };
    for edit in &plan.workspace_edits {
        if let Some(mut expected) = write(plan, edit) {
            expected.work = Some(work.clone());
            if serde_json::to_value(expected)? == *body {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) async fn apply(preparing: &Preparing, job: &Job) -> Result<Done, Stop> {
    if preparing.plan.workspace_edits.is_empty() {
        return Ok(Done::Finished);
    }
    validate(&preparing.plan.workspace_edits)
        .map_err(|why| Failure::new("request.invalid", why))?;
    let thread = preparing
        .plan
        .thread
        .ok_or_else(|| Failure::new("internal", "The workspace file plan has no thread"))?;
    // A source conflict is reviewed in the prepared workspace; cleanup must keep both texts.
    job.remember(&json!({"workspace_file_review_keep":true}))
        .await?;
    check_with_write(job, &preparing.plan, true, preparing.provisioning_runtime).await?;
    let store = preparing.engine.blobs().ok_or_else(|| {
        Failure::new(
            "workspace.source_failed",
            "The saved file content store is unavailable",
        )
    })?;
    for edit in &preparing.plan.workspace_edits {
        let body = write(&preparing.plan, edit)
            .ok_or_else(|| Failure::new("internal", "The file plan has no workspace directory"))?;
        let size = i64::try_from(edit.text.len()).map_err(|_| {
            Failure::new(
                "request.too_large",
                "The saved file exceeds its byte representation",
            )
        })?;
        store
            .put(
                &body.blob,
                Some(size),
                orch_blob_contract::store::stream_of(vec![edit.text.clone().into_bytes().into()]),
            )
            .await
            .map_err(EngineError::unreadable)?;
        let mut tx = preparing.engine.db().begin().await?;
        let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if preparing.lock_attempt(&mut tx, &owner).await?.is_none() {
            return Err(Failure::new(
                "conflict.stale_preflight",
                "The saved file plan changed before writing",
            )
            .into());
        }
        storage::register_blob(&mut tx, &body.blob, size, job.now()).await?;
        sqlx::query("INSERT INTO workspace_start_file_blobs(job_id,thread_id,path,blob) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(job.id()).bind(thread).bind(&edit.path).bind(body.blob.as_bytes().as_slice()).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
        rows::merge_plan(
            &mut tx,
            job.id(),
            &json!({"workspace_file_bytes":true}),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let id = crate::ids::derived(
            "workspace.start-file",
            &[
                job.id().as_bytes(),
                thread.as_bytes(),
                edit.path.as_bytes(),
                body.blob.as_bytes(),
            ],
        );
        let command = Outgoing::new(
            id,
            preparing.plan.machine,
            SerialKey::Environment(preparing.plan.environment),
            &body,
        )
        .map_err(EngineError::from)?;
        match preparing.command(job, command).await {
            Ok(value) => {
                let stat: orch_api_contract::workspace::FileStat =
                    serde_json::from_value(value).map_err(EngineError::from)?;
                if stat.hash != Some(body.blob) {
                    return Err(Failure::new(
                        "workspace.source_failed",
                        "The machine did not report the saved file version",
                    )
                    .into());
                }
            }
            Err(Stop::Failed(failure)) if failure.code == "conflict" => {
                let applied = preparing
                    .engine
                    .inner
                    .commands
                    .status(id)
                    .await
                    .map_err(EngineError::from)?
                    .and_then(|state| state.outcome)
                    .is_some_and(|outcome| match outcome {
                        Outcome::Failed(error) => error
                            .details
                            .as_ref()
                            .and_then(|value| value.pointer("/current/hash"))
                            .is_some_and(|value| *value == json!(body.blob)),
                        _ => false,
                    });
                if !applied {
                    return Err(Failure::new(
                        "workspace.source_conflict",
                        "A source file changed before the saved edit could be applied",
                    )
                    .into());
                }
            }
            Err(stop) => return Err(stop),
        }
    }
    Ok(Done::Finished)
}

pub(crate) async fn release(engine: &Engine, job: Id) -> Result<(), EngineError> {
    let mut tx = engine.db().begin().await?;
    sqlx::query("DELETE FROM workspace_start_file_blobs WHERE job_id=$1")
        .bind(job)
        .execute(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
