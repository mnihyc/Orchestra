//! Raw editor saves use the same native carrier under their original environment file operation.

use super::*;
use axum::body::Body;
use orch_api_contract::{threads::UploadSessionCompletion, workspace::FileStat};
use orch_blob_contract::range::BlobMeta;
use orch_db::queries::uploads::targets::{self as rows, Destination};
use orch_engine::{commands::Outgoing, operation_scope::upload_session::Editor};
use orch_link_contract::commands::{
    CommandBody, CommandKind, ExpectedHash, FsRelayAbort, FsRelayCommit, FsRelayPrepare, SerialKey,
};
use orch_types::Time;

pub async fn save(
    ctx: &Ctx,
    door: &crate::routes::workspace::door::Door,
    path: String,
    expected: Option<ExpectedHash>,
    body: Body,
) -> ApiResult<FileStat> {
    let options = crate::relay::Options::read(ctx).await?;
    let limit = i64::try_from(orch_api_contract::common::MAX_BODY_BYTES).unwrap_or(i64::MAX);
    let relay = ctx
        .app
        .blobs
        .relays()
        .create(
            options.concurrency,
            u64::try_from(limit).map_err(ApiError::internal)?,
        )
        .await;
    let input = relay.input().await;
    let bytes = axum::body::to_bytes(body, usize::try_from(limit).unwrap_or(usize::MAX) + 1)
        .await
        .map_err(|_| too_large(limit))?;
    if i64::try_from(bytes.len()).unwrap_or(i64::MAX) > limit {
        return Err(too_large(limit));
    }
    drop(input);
    let hash = orch_types::Hash::of(&bytes);
    let size = i64::try_from(bytes.len()).map_err(ApiError::internal)?;
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let (fresh, machine) = crate::routes::workspace::door::writing(
        ctx,
        &mut tx,
        door.env.id,
        door.thread,
        guard.principal().clone(),
    )
    .await?;
    if fresh.env.machine_id != door.env.machine_id
        || fresh.env.generation != door.env.generation
        || fresh.env.workspace_root != door.env.workspace_root
    {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the editor destination changed before its save",
        ));
    }
    let hours = orch_settings::get::<i64>(
        &mut tx,
        Place::System,
        orch_settings_contract::registry::keys::STORAGE_UPLOAD_SESSION_HOURS,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::internal("the upload idle default is missing"))?;
    let idle = Duration::try_hours(hours).ok_or_else(|| {
        ApiError::invalid(
            "storage.upload_session_hours",
            "the duration cannot be represented",
        )
    })?;
    let now = ctx.app.clock.now();
    let id = new_id();
    let session = SessionRow {
        id,
        user_id: guard.user().id(),
        name: crate::routes::workspace::paths::name_of(&path).to_owned(),
        media_type: None,
        size,
        idle_secs: idle.num_seconds(),
        received: 0,
        created_at: now,
        updated_at: now,
        expires_at: now
            .checked_add_signed(idle)
            .unwrap_or(DateTime::<Utc>::MAX_UTC),
    };
    guard.check_deadline(ctx)?;
    storage::begin_session(&mut tx, &session).await?;
    rows::insert(
        &mut tx,
        id,
        &Destination {
            environment_id: fresh.env.id,
            machine_id: Some(machine),
            generation: fresh.env.generation,
            workspace_root: fresh.env.workspace_root,
            path: path.clone(),
            thread_id: door.thread,
            relayed: true,
        },
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    let result = async {
        native::put_reserved(ctx, id, 0, Body::from(bytes), size, Some((options, relay))).await?;
        let verified = loop {
            match native::complete(ctx, id).await? {
                UploadSessionCompletion::Ready { file } => {
                    break BlobMeta {
                        hash: file.blob,
                        size: file.size,
                    };
                }
                UploadSessionCompletion::Verifying { retry_after_ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(retry_after_ms)).await
                }
            }
        };
        if verified.hash != hash || verified.size != size {
            return Err(ApiError::internal(
                "the destination verified different editor bytes",
            ));
        }
        run(
            ctx,
            id,
            machine,
            SerialKey::None,
            &FsRelayPrepare {
                environment_id: door.env.id,
                receive_path: path.clone(),
                path: path.clone(),
                session_id: id,
                work: None,
            },
            options.until,
        )
        .await?;
        run(
            ctx,
            id,
            machine,
            SerialKey::Environment(door.env.id),
            &FsRelayCommit {
                environment_id: door.env.id,
                receive_path: path.clone(),
                path: path.clone(),
                session_id: id,
                hash,
                size,
                mode: None,
                expected_hash: expected,
                work: None,
            },
            options.until,
        )
        .await
    }
    .await;
    // Only private staging is cleaned; an accepted native publication is never undone here.
    let cleanup = run(
        ctx,
        id,
        machine,
        SerialKey::None,
        &FsRelayAbort {
            environment_id: door.env.id,
            receive_path: path,
            session_id: id,
        },
        options.until,
    )
    .await;
    if let Err(error) = cleanup {
        tracing::warn!(%id,code=?error.code(),"editor staging remains held for native expiry");
    }
    let mut tx = ctx.app.db.begin().await?;
    sqlx::query("UPDATE upload_sessions SET completed_at=COALESCE(completed_at,$2) WHERE id=$1")
        .bind(id)
        .bind(ctx.app.clock.now())
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    tx.commit().await.map_err(DbError::from)?;
    result
}

async fn run<B: CommandBody + Sync>(
    ctx: &Ctx,
    id: Id,
    machine: Id,
    key: SerialKey,
    body: &B,
    until: Time,
) -> ApiResult<B::Result> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let state = rows::state(&mut tx, id, true)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut target = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    check_session_for(ctx, &state.session, guard.user().id(), guard.now().get())?;
    creation::check_destination(ctx, &mut tx, guard.principal(), &target.destination).await?;
    let command = new_id();
    guard.check_deadline(ctx)?;
    if B::KIND == CommandKind::FsRelayAbort {
        rows::aborting(&mut tx, id, command).await?;
        target.aborting_command = Some(command);
    } else {
        rows::receiving(&mut tx, id, command).await?;
        target.receiving_command = Some(command);
    }
    let command = Outgoing::new(command, machine, key, body)
        .map_err(ApiError::internal)?
        .by(until);
    let id = native_command::issue::<Editor>(ctx, tx, &state, &target, command).await?;
    ctx.app
        .link
        .dispatcher()
        .wait_for::<B>(id)
        .await
        .map_err(|error| {
            let failure = crate::routes::workspace::live::command_failure(error);
            let current = match &failure {
                crate::routes::workspace::problems::Failure::Machine(error)
                    if error.code == "conflict" =>
                {
                    error
                        .details
                        .as_ref()
                        .and_then(|detail| detail.get("current"))
                        .cloned()
                }
                _ => None,
            };
            let problem = failure.problem(crate::routes::workspace::problems::Doing::Writing);
            match current {
                Some(current) => problem.with("current", current),
                None => problem,
            }
        })
}
