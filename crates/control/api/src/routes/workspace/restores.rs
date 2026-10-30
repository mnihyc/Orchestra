//! Current-authority restore plans bind immutable manifests, whole namespaces and visible writers.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, Registry};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::forks::{
    ActiveTurn, AppliedChangeOptions, ChangeOptions, PREFLIGHT_VALID_MINUTES,
};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::source::{
    EnvironmentFilesRestore, EnvironmentFilesRestoreBody, EnvironmentRestorePreflight,
    EnvironmentRestorePreflightBody, RestorePlan, RestoreWriter,
};
use orch_api_contract::workspace::Job;
use orch_authz::{Act, Policy};
use orch_db::queries::{jobs, source, storage, threads};
use orch_engine::jobs::source::{Action, Plan};
use orch_link_contract::commands::SerialKey;
use orch_link_contract::source::{FsRestore, FsRestorePlan};
use orch_settings::{Place, get};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Hash, Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Binds the whole restore preflight and confirmation on the owning environment.
pub fn mount(registry: &mut Registry) {
    registry
        .on(EnvironmentRestorePreflight, preflight)
        .on(EnvironmentFilesRestore, confirm);
}

fn unavailable(reason: &str) -> ApiError {
    ApiError::new(
        ErrorCode::WorkspaceRestoreUnavailable,
        "complete recorded files are unavailable; review and undo individual files instead",
    )
    .with("reason", reason)
}

fn decode<T: serde::de::DeserializeOwned>(value: &Json, name: &str) -> ApiResult<T> {
    serde_json::from_value(
        value
            .get(name)
            .cloned()
            .ok_or_else(|| ApiError::internal("the stored restore plan lacks a required member"))?,
    )
    .map_err(ApiError::internal)
}

async fn options(
    conn: &mut orch_db::Conn,
    environment: Id,
    given: Option<&ChangeOptions>,
) -> ApiResult<AppliedChangeOptions> {
    let place = Place::Environment(environment);
    let active_turn = match given.and_then(|options| options.active_turn) {
        Some(value) => value,
        None => {
            if get::<String>(conn, place, keys::SWITCH_ACTIVE_TURN)
                .await
                .map_err(ApiError::internal)?
                .as_deref()
                == Some("interrupt")
            {
                ActiveTurn::Interrupt
            } else {
                ActiveTurn::Wait
            }
        }
    };
    let wait_minutes = match given.and_then(|options| options.wait_minutes) {
        Some(value) => value,
        None => get::<i64>(conn, place, keys::SWITCH_WAIT_MINUTES)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("the wait default is missing"))?,
    };
    let verify_timeout_secs = match given.and_then(|options| options.verify_timeout_secs) {
        Some(value) => value,
        None => get::<i64>(conn, place, keys::SWITCH_VERIFY_TIMEOUT_SECS)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("the verification default is missing"))?,
    };
    for (key, value) in [
        (keys::SWITCH_WAIT_MINUTES, wait_minutes),
        (keys::SWITCH_VERIFY_TIMEOUT_SECS, verify_timeout_secs),
    ] {
        registry::validate(key, &json!(value))
            .map_err(|error| ApiError::invalid("options", &error.to_string()))?;
    }
    Ok(AppliedChangeOptions {
        active_turn,
        wait_minutes,
        verify_timeout_secs,
        keep_previous: given
            .and_then(|options| options.keep_previous)
            .unwrap_or(true),
        stop_processes: given
            .and_then(|options| options.stop_processes)
            .unwrap_or(false),
    })
}

#[derive(Serialize, Deserialize)]
struct Frozen {
    shown: RestorePlan,
    recipe: Plan,
    plan_blob: Hash,
}

async fn require(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
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

/// Included semantic hashes are compared with the complete end map, preserving unknown coverage.
fn later(paths: &mut [orch_api_contract::source::RestorePath], current: &Json, end: Option<&Json>) {
    let Some(end) = end.and_then(Json::as_object) else {
        return;
    };
    let mut now = std::collections::BTreeMap::new();
    for entry in current.as_array().into_iter().flatten() {
        let Some(path) = entry.get("p").and_then(Json::as_str) else {
            continue;
        };
        let hash = if entry.get("t").and_then(Json::as_str) == Some("l") {
            entry
                .get("l")
                .and_then(Json::as_str)
                .map(|link| json!(Hash::of(link.as_bytes())))
                .unwrap_or(Json::Null)
        } else {
            entry.get("h").cloned().unwrap_or(Json::Null)
        };
        now.insert(
            path,
            json!({ "h": hash, "t": entry["t"], "x": entry["x"].as_u64().unwrap_or(0) & 0o111 }),
        );
    }
    for path in paths {
        path.later_change = Some(now.get(path.path.as_str()) != end.get(&path.path));
    }
}

fn attribute(
    paths: &mut [orch_api_contract::source::RestorePath],
    directory: &str,
    writer_directory: &str,
    before: &Json,
    end: &Json,
    writer: &NodeRef,
) {
    let (Some(before), Some(end)) = (before.as_object(), end.as_object()) else {
        return;
    };
    for path in paths {
        let absolute = if directory.is_empty() {
            path.path.clone()
        } else {
            format!("{directory}/{}", path.path)
        };
        let relative = if writer_directory.is_empty() {
            Some(absolute.as_str())
        } else {
            absolute
                .strip_prefix(writer_directory)
                .and_then(|rest| rest.strip_prefix('/'))
        };
        let Some(relative) = relative else { continue };
        if before.get(relative) != end.get(relative)
            && !path.later_writers.iter().any(|known| known.id == writer.id)
        {
            path.later_writers.push(writer.clone());
        }
    }
}

/// Names only a visible later turn whose complete before/end maps prove an included write.
/// Separate threads at the same clock instant have unknown order; one thread's ordinals do not.
async fn known_writers(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    environment: Id,
    directory: &str,
    target: Option<&threads::TurnRow>,
    writers: &[RestoreWriter],
    paths: &mut [orch_api_contract::source::RestorePath],
) -> ApiResult<()> {
    let Some(target) = target.filter(|turn| turn.completed_at.is_some()) else {
        return Ok(());
    };
    for writer in writers.iter().filter_map(|writer| writer.thread.as_ref()) {
        let mut before_ordinal: Option<i32> = None;
        loop {
            let turns: Vec<(i32, Id, Json)> = sqlx::query_as(
                "SELECT t.ordinal, t.before_snapshot_id, t.files_end_manifest FROM turns t
                 JOIN snapshots s ON s.id = t.before_snapshot_id JOIN segments g ON g.id = t.segment_id
                 WHERE t.thread_id = $1 AND t.completed_at IS NOT NULL AND t.files_end_manifest IS NOT NULL
                   AND s.environment_id = $2 AND g.environment_id = $2 AND s.env_generation = g.env_generation
                   AND s.storage = 'environment' AND s.state = 'complete' AND s.manifest_blob IS NOT NULL
                   AND (t.started_at > $3 OR t.thread_id = $4 AND t.ordinal > $5)
                   AND ($6::int IS NULL OR t.ordinal < $6) ORDER BY t.ordinal DESC LIMIT $7")
                .bind(writer.id).bind(environment).bind(target.completed_at).bind(target.thread_id).bind(target.ordinal)
                .bind(before_ordinal).bind(i64::from(orch_api_contract::common::MAX_LIMIT))
                .fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
            if turns.is_empty() {
                break;
            }
            before_ordinal = turns.last().map(|(ordinal, _, _)| *ordinal);
            for (_, snapshot, end) in turns {
                let Some(recorded) = storage::snapshot(conn, snapshot).await? else {
                    continue;
                };
                let (Some(hash), Some(writer_directory)) =
                    (recorded.manifest_blob, recorded.directory)
                else {
                    continue;
                };
                // Missing/expired old coverage cannot establish a writer. It leaves the original
                // later_change value alone; neither an unknown map nor a hidden name becomes proof.
                let Ok(bytes) = ctx
                    .app
                    .blobs
                    .read_whole(
                        &hash,
                        i64::try_from(orch_engine::manifest::MANIFEST_MAX_EXPANDED_BYTES)
                            .map_err(ApiError::internal)?,
                    )
                    .await
                else {
                    continue;
                };
                if Hash::of(&bytes) != hash {
                    continue;
                }
                let Ok(manifest) = orch_engine::manifest::Manifest::parse(&bytes) else {
                    continue;
                };
                if manifest.header().generation != i64::from(recorded.env_generation) {
                    continue;
                }
                attribute(
                    paths,
                    directory,
                    &writer_directory,
                    &orch_engine::source::end_namespace(&manifest),
                    &end,
                    writer,
                );
            }
        }
    }
    Ok(())
}

async fn writer_ids(
    conn: &mut orch_db::Conn,
    environment: Id,
    directory: &str,
) -> ApiResult<Vec<Id>> {
    Ok(sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
        WHERE t.environment_id = $1 AND n.deleted_at IS NULL AND t.side_of IS NULL
          AND (t.current_turn_id IS NOT NULL OR $2 = '' OR t.workspace_dir = '' OR t.workspace_dir = $2 OR starts_with(t.workspace_dir, $2 || '/') OR starts_with($2, t.workspace_dir || '/')) ORDER BY t.node_id")
        .bind(environment).bind(directory).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?)
}

async fn preflight(
    mut ctx: Ctx,
    (): (),
    body: EnvironmentRestorePreflightBody,
) -> ApiResult<RestorePlan> {
    let environment = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require(
        &mut tx,
        guard.principal(),
        environment,
        Permission::FilesManage,
    )
    .await?;
    let door = super::door::open(&ctx, &mut tx, None).await?;
    door.may_write()?;
    let machine = super::live::require_live(&ctx, &mut tx, &door.env).await?;
    let snapshot = storage::snapshot(&mut tx, body.snapshot_id)
        .await?
        .filter(|row| row.environment_id == environment)
        .ok_or_else(ApiError::not_found)?;
    if snapshot.storage != "environment" || snapshot.state != "complete" {
        return Err(unavailable(
            snapshot
                .unavailable_reason
                .as_deref()
                .unwrap_or("incomplete"),
        ));
    }
    if snapshot
        .expires_at
        .is_none_or(|expires| expires <= guard.now().get())
    {
        return Err(unavailable("expired"));
    }
    let directory = snapshot
        .directory
        .clone()
        .ok_or_else(|| unavailable("unknown_directory"))?;
    let record_days = get::<i64>(
        &mut tx,
        Place::Environment(environment),
        keys::WORKSPACE_RECORD_DAYS,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| unavailable("off"))?;
    let max_bytes = get::<i64>(
        &mut tx,
        Place::Environment(environment),
        keys::WORKSPACE_RECORD_MAX_BYTES,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| unavailable("off"))?;
    if record_days <= 0 {
        return Err(unavailable("off"));
    }
    let applied = options(&mut tx, environment, body.options.as_ref()).await?;
    let target_turn: Option<Id> = sqlx::query_scalar(
        "SELECT id FROM turns WHERE before_snapshot_id = $1 ORDER BY ordinal DESC LIMIT 1",
    )
    .bind(snapshot.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(orch_db::DbError::from)?;
    let target = match target_turn {
        Some(turn) => threads::turn(&mut tx, turn).await?,
        None => None,
    };
    // The machine and its blob upload need the same database pool. Hold no transaction or
    // connection while asking them, and decide again before keeping or returning the plan.
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let mut measured = super::live::run(
        &ctx,
        machine,
        SerialKey::Environment(environment),
        &FsRestorePlan {
            environment_id: environment,
            snapshot_id: snapshot.id,
            directory: directory.clone(),
            turn_id: target_turn,
            record_days: Some(record_days),
            max_bytes: Some(max_bytes),
        },
    )
    .await
    .map_err(|failure| match failure.code() {
        Some("changed" | "conflict") => ApiError::new(
            ErrorCode::WorkspaceRestoreConflict,
            "the recorded coverage or current files changed; make a new plan",
        ),
        Some("unsupported" | "too_large" | "not_found" | "ignored") => {
            unavailable("store_unavailable")
        }
        _ => super::live::refused(failure, super::problems::Doing::Reading),
    })?;
    let bytes = ctx
        .app
        .blobs
        .read_whole(
            &measured.plan_blob,
            i64::try_from(orch_engine::manifest::MANIFEST_MAX_EXPANDED_BYTES)
                .map_err(ApiError::internal)?,
        )
        .await
        .map_err(ApiError::internal)?;
    if Hash::of(&bytes) != measured.plan_blob {
        return Err(unavailable("store_changed"));
    }
    let private: Json = serde_json::from_slice(&bytes).map_err(ApiError::internal)?;
    let target_manifest: Hash = decode(&private, "target_manifest")?;
    if Some(target_manifest) != snapshot.manifest_blob
        || decode::<Id>(&private, "snapshot_id")? != snapshot.id
        || decode::<String>(&private, "directory")? != directory
        || decode::<i64>(&private, "generation")? != i64::from(door.env.generation)
    {
        return Err(unavailable("store_changed"));
    }
    later(
        &mut measured.paths,
        &private["current"],
        target
            .as_ref()
            .and_then(|turn| turn.files_end_manifest.as_ref()),
    );
    {
        // Attribution reads old content objects too, on a bare connection rather than under
        // an authority transaction. Names are admitted afresh below before any are published.
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        let mut candidates = Vec::new();
        for id in writer_ids(&mut conn, environment, &directory).await? {
            match authorization::require(
                &mut conn,
                &principal,
                id,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await
            {
                Ok(()) => candidates.push(RestoreWriter {
                    thread: Some(NodeRef {
                        thread_display: None,
                        id,
                        kind: NodeKind::Thread,
                        name: None,
                        redacted: true,
                    }),
                    active: false,
                }),
                Err(error) if error.code() == ErrorCode::Internal => return Err(error),
                Err(_) => {}
            }
        }
        known_writers(
            &ctx,
            &mut conn,
            environment,
            &directory,
            target.as_ref(),
            &candidates,
            &mut measured.paths,
        )
        .await?;
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.caller =
        crate::middleware::authentication::Caller::User(std::sync::Arc::new(guard.user().clone()));
    ctx.now = guard.now().get();
    require(
        &mut tx,
        guard.principal(),
        environment,
        Permission::FilesManage,
    )
    .await?;
    let current_door = super::door::open(&ctx, &mut tx, None).await?;
    current_door.may_write()?;
    let current_machine = super::live::require_live(&ctx, &mut tx, &current_door.env).await?;
    let current_snapshot = storage::snapshot(&mut tx, snapshot.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if current_door.env.generation != door.env.generation
        || current_machine != machine
        || current_door.env.workspace_root != door.env.workspace_root
        || current_snapshot.environment_id != environment
        || current_snapshot.storage != "environment"
        || current_snapshot.state != "complete"
        || current_snapshot.env_generation != snapshot.env_generation
        || current_snapshot.manifest_blob != snapshot.manifest_blob
        || current_snapshot.tree_ref != snapshot.tree_ref
        || current_snapshot.directory != snapshot.directory
        || current_snapshot.thread_id != snapshot.thread_id
    {
        return Err(ApiError::new(
            ErrorCode::WorkspaceRestoreConflict,
            "the environment or recorded files changed; make a new plan",
        ));
    }
    if current_snapshot
        .expires_at
        .is_none_or(|expires| expires <= guard.now().get())
    {
        return Err(unavailable("expired"));
    }
    if get::<i64>(
        &mut tx,
        Place::Environment(environment),
        keys::WORKSPACE_RECORD_DAYS,
    )
    .await
    .map_err(ApiError::internal)?
        != Some(record_days)
        || get::<i64>(
            &mut tx,
            Place::Environment(environment),
            keys::WORKSPACE_RECORD_MAX_BYTES,
        )
        .await
        .map_err(ApiError::internal)?
            != Some(max_bytes)
        || options(&mut tx, environment, body.options.as_ref()).await? != applied
    {
        return Err(ApiError::new(
            ErrorCode::WorkspaceRestoreConflict,
            "the restore settings changed; make a new plan",
        ));
    }
    let ids = writer_ids(&mut tx, environment, &directory).await?;
    let mut writers = Vec::new();
    let mut blockers = Vec::new();
    let can_suspend = require(
        &mut tx,
        guard.principal(),
        environment,
        Permission::TerminalManage,
    )
    .await;
    let suspend = match can_suspend {
        Ok(()) => true,
        Err(error) if error.code() == ErrorCode::Internal => return Err(error),
        Err(_) => false,
    };
    for id in &ids {
        let row = threads::thread(&mut tx, *id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let active = row.current_turn_id.is_some();
        let visible = match authorization::require(
            &mut tx,
            guard.principal(),
            *id,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
        )
        .await
        {
            Ok(()) => Some(NodeRef {
                thread_display: None,
                id: *id,
                kind: NodeKind::Thread,
                name: Some(row.name),
                redacted: false,
            }),
            Err(error) if error.code() == ErrorCode::Internal => return Err(error),
            Err(_) => None,
        };
        if active
            && applied.active_turn == ActiveTurn::Interrupt
            && require(
                &mut tx,
                guard.principal(),
                environment,
                Permission::EnvironmentManage,
            )
            .await
            .is_err()
            && authorization::require(
                &mut tx,
                guard.principal(),
                *id,
                authorization::exact(Permission::ThreadInteract),
                Act::Use,
                Policy::default(),
            )
            .await
            .is_err()
        {
            blockers.push("interrupt_not_allowed".to_owned());
        }
        writers.push(RestoreWriter {
            thread: visible,
            active,
        });
    }
    if measured.writers_active && !writers.iter().any(|writer| writer.active) {
        writers.push(RestoreWriter {
            thread: None,
            active: true,
        });
    }
    let names: std::collections::HashMap<_, _> = writers
        .iter()
        .filter_map(|writer| {
            writer
                .thread
                .as_ref()
                .map(|thread| (thread.id, thread.clone()))
        })
        .collect();
    for path in &mut measured.paths {
        path.later_writers = path
            .later_writers
            .iter()
            .filter_map(|writer| names.get(&writer.id).cloned())
            .collect();
    }
    if !measured.undo_available {
        blockers.push("complete_undo_unavailable".to_owned());
    }
    let shown_snapshot = super::snapshots::views(&mut tx, vec![current_snapshot], guard.now())
        .await?
        .remove(0);
    let preflight = new_id();
    let shown = RestorePlan {
        preflight_id: preflight,
        expires_at: Time::new(
            guard.now().get() + chrono::Duration::minutes(PREFLIGHT_VALID_MINUTES),
        ),
        environment: shown_snapshot.environment.clone(),
        snapshot: shown_snapshot,
        generation: i64::from(door.env.generation),
        directory: directory.clone(),
        namespace_fingerprint: decode(&private, "namespace_fingerprint")?,
        ignore_fingerprint: decode(&private, "ignore_fingerprint")?,
        paths: measured.paths,
        writers,
        ignored: vec![".git".to_owned()],
        undo_available: measured.undo_available,
        options: applied.clone(),
        blockers,
    };
    let recipe = Plan {
        environment,
        generation: door.env.generation,
        machine,
        thread: snapshot.thread_id,
        caller: guard.user().id(),
        token: guard
            .principal()
            .caller()
            .grant()
            .and_then(|grant| grant.token()),
        action: Action::Restore {
            body: FsRestore {
                work: None,
                environment_id: environment,
                job_id: new_id(),
                snapshot_id: snapshot.id,
                undo_snapshot_id: new_id(),
                plan_blob: measured.plan_blob,
                generation: i64::from(door.env.generation),
            },
            writers: ids,
            options: applied,
            suspend,
            directory,
            record_days,
            max_bytes,
        },
    };
    let frozen = Frozen {
        shown: shown.clone(),
        recipe,
        plan_blob: measured.plan_blob,
    };
    storage::register_blob(
        &mut tx,
        &measured.plan_blob,
        bytes.len() as i64,
        guard.now().get(),
    )
    .await?;
    storage::pin_blobs(&mut tx, &[measured.plan_blob]).await?;
    jobs::forks::save_preflight(
        &mut tx,
        &jobs::forks::PreflightRow {
            id: preflight,
            kind: jobs::kind::WORKSPACE_RESTORE.to_owned(),
            node_id: snapshot.thread_id,
            environment_id: Some(environment),
            created_by: guard.user().id(),
            plan: serde_json::to_value(frozen).map_err(ApiError::internal)?,
            expires_at: shown.expires_at.get(),
            created_at: guard.now().get(),
        },
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(shown)
}

async fn confirm(ctx: Ctx, (): (), body: EnvironmentFilesRestoreBody) -> ApiResult<Job> {
    let environment = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require(
        &mut tx,
        guard.principal(),
        environment,
        Permission::FilesManage,
    )
    .await?;
    let saved = jobs::changes::lock_plan(&mut tx, body.preflight_id)
        .await?
        .filter(|row| {
            row.kind == jobs::kind::WORKSPACE_RESTORE
                && row.environment_id == Some(environment)
                && row.created_by == guard.user().id()
        })
        .ok_or_else(ApiError::not_found)?;
    let frozen: Frozen = serde_json::from_value(saved.plan).map_err(ApiError::internal)?;
    let existing: Option<Id> = sqlx::query_scalar("SELECT id FROM jobs WHERE kind = 'workspace_restore' AND plan ->> 'preflight_id' = $1 ORDER BY created_at LIMIT 1")
        .bind(body.preflight_id.to_string()).fetch_optional(&mut *tx).await.map_err(orch_db::DbError::from)?;
    if let Some(id) = existing {
        let row = jobs::job(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let shown = crate::routes::environments::jobs::view(&mut tx, &row).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(shown);
    }
    if saved.expires_at <= guard.now().get() {
        return Err(ApiError::new(
            ErrorCode::PreflightExpired,
            "the restore plan expired; make a new plan",
        ));
    }
    if !frozen.shown.blockers.is_empty() {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "the restore plan has unresolved blockers",
        )
        .with("blockers", frozen.shown.blockers));
    }
    let door = super::door::open(&ctx, &mut tx, None).await?;
    door.may_write()?;
    if door.env.generation != frozen.recipe.generation
        || door.env.machine_id != Some(frozen.recipe.machine)
    {
        return Err(ApiError::new(
            ErrorCode::WorkspaceRestoreConflict,
            "the environment changed; make a new plan",
        ));
    }
    let target = storage::snapshot(&mut tx, frozen.shown.snapshot.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if target.state != "complete"
        || target
            .expires_at
            .is_none_or(|expires| expires <= guard.now().get())
    {
        return Err(unavailable("expired"));
    }
    if let Action::Restore {
        suspend,
        writers,
        options,
        ..
    } = &frozen.recipe.action
    {
        if *suspend {
            require(
                &mut tx,
                guard.principal(),
                environment,
                Permission::TerminalManage,
            )
            .await?;
        }
        if options.active_turn == ActiveTurn::Interrupt {
            for thread in writers {
                if require(
                    &mut tx,
                    guard.principal(),
                    environment,
                    Permission::EnvironmentManage,
                )
                .await
                .is_err()
                {
                    authorization::require(
                        &mut tx,
                        guard.principal(),
                        *thread,
                        authorization::exact(Permission::ThreadInteract),
                        Act::Use,
                        Policy::default(),
                    )
                    .await?;
                }
            }
        }
    }
    let row = orch_engine::jobs::source::begin(&mut tx, frozen.recipe, guard.now().get())
        .await
        .map_err(super::source::make_error)?;
    jobs::merge_plan(
        &mut tx,
        row.id,
        &json!({ "preflight_id": body.preflight_id }),
        guard.now().get(),
    )
    .await?;
    let recipe: Plan = serde_json::from_value(row.plan.clone()).map_err(ApiError::internal)?;
    if let Action::Restore {
        body, directory, ..
    } = &recipe.action
    {
        source::begin_recorded(
            &mut tx,
            &storage::NewSnapshot {
                id: body.undo_snapshot_id,
                thread_id: target.thread_id,
                environment_id: environment,
                env_generation: recipe.generation,
                label: "Before restore",
                selection: &json!({ "include": [directory], "exclude": [] }),
                created_by: Some(guard.user().id()),
                at: guard.now().get(),
            },
            directory,
            "restore",
        )
        .await?;
    }
    storage::pin_blobs(&mut tx, &[frozen.plan_blob]).await?;
    let shown = crate::routes::environments::jobs::view(&mut tx, &row).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app.engine.kick();
    Ok(shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn later_writer_names_follow_proved_paths_across_overlapping_directories() {
        use orch_api_contract::source::{
            ExpectedAbsent, RestoreAction, RestoreExpected, RestorePath,
        };
        let writer = NodeRef {
            thread_display: None,
            id: new_id(),
            kind: NodeKind::Thread,
            name: Some("Own attempt".into()),
            redacted: false,
        };
        let make = |path: &str| RestorePath {
            path: path.into(),
            action: RestoreAction::Change,
            expected: RestoreExpected::Absent(ExpectedAbsent::None),
            later_change: None,
            later_writers: Vec::new(),
        };
        let mut paths = vec![
            make("nested/code"),
            make("removed"),
            make("unchanged"),
            make("nested-sibling/code"),
        ];
        let before = json!({ "code": { "h": Hash::of(b"old"), "t": "f", "x": 0 } });
        let after = json!({ "code": { "h": Hash::of(b"old"), "t": "f", "x": 0o111 } });
        attribute(&mut paths, "web", "web/nested", &before, &after, &writer);
        attribute(&mut paths, "web", "web/nested", &before, &after, &writer);
        assert_eq!(
            paths[0].later_writers,
            vec![writer.clone()],
            "a mode-only write is known once"
        );
        assert!(
            paths
                .iter()
                .skip(1)
                .all(|path| path.later_writers.is_empty()),
            "a prefix sibling is outside the writer's directory"
        );
        attribute(
            &mut paths,
            "web",
            "web",
            &json!({ "removed": { "h": Hash::of(b"gone"), "t": "f", "x": 0 } }),
            &json!({}),
            &writer,
        );
        assert_eq!(paths[1].later_writers, vec![writer]);
        assert!(paths[2].later_writers.is_empty());
        assert!(
            paths.iter().all(|path| path.later_change.is_none()),
            "a writer never fills an unknown end comparison"
        );
    }
    #[test]
    fn later_changes_compare_added_removed_link_type_and_mode_without_guessing_unknowns() {
        use orch_api_contract::source::{ExpectedAbsent, RestoreAction, RestoreExpected};
        let mut paths = vec![orch_api_contract::source::RestorePath {
            path: "code".to_owned(),
            action: RestoreAction::Change,
            expected: RestoreExpected::Absent(ExpectedAbsent::None),
            later_change: None,
            later_writers: Vec::new(),
        }];
        let bytes = Hash::of(b"content");
        let current = json!([{ "p": "code", "t": "f", "x": 0o755, "h": bytes, "l": null }]);
        later(&mut paths, &current, None);
        assert_eq!(paths[0].later_change, None);
        later(
            &mut paths,
            &current,
            Some(&json!({ "code": { "h": bytes, "t": "f", "x": 0o111 } })),
        );
        assert_eq!(paths[0].later_change, Some(false));
        later(
            &mut paths,
            &current,
            Some(&json!({ "code": { "h": bytes, "t": "f", "x": 0 } })),
        );
        assert_eq!(paths[0].later_change, Some(true));
        later(&mut paths, &current, Some(&json!({})));
        assert_eq!(paths[0].later_change, Some(true));
    }
}
