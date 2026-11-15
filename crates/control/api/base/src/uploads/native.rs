//! The shared browser session protocol with a native temporary-file carrier.

use super::*;
use axum::body::Body;
use orch_api_contract::threads::UploadSessionCompletion;
use orch_db::queries::uploads::targets::{self as rows, State};
use orch_engine::{
    commands::Outgoing,
    operation_scope::upload_session::{Cleanup, Piece, Verify},
};
use orch_link_contract::commands::{FsRelayAbort, FsRelayReceive, FsRelayVerify, SerialKey};
use orch_types::Time;
use std::sync::Arc;

struct PieceOwner {
    relay: Arc<crate::relay::Relay>,
    dispatcher: crate::dispatch::Dispatcher,
    id: Id,
}
impl Drop for PieceOwner {
    fn drop(&mut self) {
        self.relay.cancel();
        let (dispatcher, id) = (self.dispatcher.clone(), self.id);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = dispatcher.cancel(id).await;
            });
        }
    }
}

pub(super) async fn put(
    ctx: &Ctx,
    id: Id,
    offset: i64,
    body: Body,
    size: i64,
) -> ApiResult<UploadSessionPutReply> {
    if offset < 0 {
        return Err(ApiError::invalid(
            "offset",
            "an upload offset is not negative",
        ));
    }
    let options = crate::relay::Options::read(ctx).await?;
    let relay = ctx
        .app
        .blobs
        .relays()
        .create(
            options.concurrency,
            u64::try_from(size).map_err(ApiError::internal)?,
        )
        .await;
    // The session gate and global memory admission precede collecting this bounded browser body.
    let input = relay.input().await;
    let previous = {
        let (mut tx, guard) = transaction_guard::begin(ctx).await?;
        let state = rows::state(&mut tx, id, false)
            .await?
            .ok_or_else(ApiError::not_found)?;
        session_operation::check_lifetime(
            ctx,
            &mut tx,
            &state.session,
            guard.user().id(),
            guard.now(),
        )
        .await?;
        let target = rows::of(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        creation::check_destination(ctx, &mut tx, guard.principal(), &target.destination).await?;
        if target.verifying_command.is_some() || target.verified_hash.is_some() {
            return Err(out_of_place(state.session.received));
        }
        let previous = target.receiving_command;
        tx.commit().await.map_err(DbError::from)?;
        previous
    };
    if let Some(previous) = previous {
        wait_stopped(ctx, previous, options.retry_ms).await?;
    }
    let bytes = axum::body::to_bytes(
        body,
        usize::try_from(chunk_bytes()).unwrap_or(usize::MAX) + 1,
    )
    .await
    .map_err(|_| too_large(chunk_bytes()))?;
    if i64::try_from(bytes.len()).unwrap_or(i64::MAX) > chunk_bytes() {
        return Err(too_large(chunk_bytes()));
    }
    let end = offset
        .checked_add(i64::try_from(bytes.len()).map_err(ApiError::internal)?)
        .filter(|end| *end <= size)
        .ok_or_else(|| ApiError::invalid("", "the piece reaches past the upload's end"))?;
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let state = rows::state(&mut tx, id, true)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut target = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    session_operation::check_lifetime(ctx, &mut tx, &state.session, guard.user().id(), guard.now())
        .await?;
    creation::check_destination(ctx, &mut tx, guard.principal(), &target.destination).await?;
    if !target.destination.relayed || target.verified_hash.is_some() || target.written_job.is_some()
    {
        return Err(ApiError::new(
            ErrorCode::Gone,
            "this upload is no longer receiving",
        ));
    }
    if let Some(previous) = target.receiving_command
        && !stopped(&mut tx, previous).await?
    {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "the earlier native piece is still stopping",
        ));
    }
    let command = orch_types::new_id();
    guard.check_deadline(ctx)?;
    rows::receiving(&mut tx, id, command).await?;
    target.receiving_command = Some(command);
    let command = Outgoing::new(
        command,
        target
            .destination
            .machine_id
            .ok_or_else(ApiError::not_found)?,
        SerialKey::None,
        &FsRelayReceive {
            environment_id: target.destination.environment_id,
            path: target.destination.path.clone(),
            session_id: id,
            relay_id: relay.id(),
            size,
            end,
            idle_secs: options.idle_secs,
            retry_ms: options.retry_ms,
            expires_at: Time::new(state.session.expires_at),
            retention_secs: u64::try_from(state.session.idle_secs).map_err(ApiError::internal)?,
        },
    )
    .map_err(ApiError::internal)?
    .by(options.until);
    let owner = PieceOwner {
        relay: Arc::clone(&relay),
        dispatcher: ctx.app.link.dispatcher(),
        id: command.id,
    };
    native_command::issue::<Piece>(ctx, tx, &state, &target, command).await?;
    let offered = input.offer(u64::try_from(offset).map_err(ApiError::internal)?, bytes);
    let machine = async {
        let result = owner
            .dispatcher
            .wait_for::<FsRelayReceive>(owner.id)
            .await
            .map_err(|error| {
                crate::routes::workspace::live::command_failure(error)
                    .problem(crate::routes::workspace::problems::Doing::Writing)
            });
        if result.is_err() {
            relay.cancel();
        }
        result
    };
    let (offered, received) = tokio::join!(offered, machine);
    let actual = received?;
    let received = actual.received;
    if received < 0 || received > size {
        return Err(ApiError::internal(
            "the native prefix lies outside this upload",
        ));
    }
    let now = ctx.app.clock.now();
    let expires = actual.expires_at.get();
    let mut tx = ctx.app.db.begin().await?;
    rows::progress(&mut tx, id, received, now, expires).await?;
    tx.commit().await.map_err(DbError::from)?;
    let reply = UploadSessionPutReply {
        received,
        expires_at: Time::new(expires),
    };
    crate::key_operations::publication::receipt(ctx, &reply)?;
    match offered {
        Ok(ack) if i64::try_from(ack).ok() == Some(received) => Ok(reply),
        Err(crate::relay::RelayError::Offset { .. }) => Err(out_of_place(received)),
        Ok(_) => Err(ApiError::internal(
            "the native destination acknowledged another prefix",
        )),
        Err(_) => Err(ApiError::new(
            ErrorCode::SourceOffline,
            "the native upload ended before acknowledging this piece",
        )),
    }
}

async fn stopped(conn: &mut orch_db::Conn, id: Id) -> ApiResult<bool> {
    Ok(sqlx::query_scalar::<_,bool>("SELECT state='succeeded' OR (state IN ('failed','cancelled','expired') AND stop_confirmed_at IS NOT NULL) FROM commands WHERE id=$1")
        .bind(id).fetch_optional(conn).await.map_err(DbError::from)?.unwrap_or(true))
}

async fn wait_stopped(ctx: &Ctx, id: Id, retry_ms: u64) -> ApiResult<()> {
    let dispatcher = ctx.app.link.dispatcher();
    if dispatcher.stopped(id).await.map_err(ApiError::internal)? {
        return Ok(());
    }
    dispatcher.cancel(id).await.map_err(ApiError::internal)?;
    while !dispatcher.stopped(id).await.map_err(ApiError::internal)? {
        tokio::time::sleep(std::time::Duration::from_millis(retry_ms)).await;
    }
    Ok(())
}

fn ready(state: &State, hash: orch_types::Hash) -> UploadSessionCompletion {
    UploadSessionCompletion::Ready {
        file: UploadedBlob {
            blob: hash,
            relay: Some(state.session.id),
            size: state.session.size,
            name: state.session.name.clone(),
            media_type: state
                .session
                .media_type
                .clone()
                .unwrap_or_else(|| crate::media::media_type(&state.session.name, &[]).to_owned()),
        },
    }
}

pub(super) async fn complete(ctx: &Ctx, id: Id) -> ApiResult<UploadSessionCompletion> {
    let options = crate::relay::Options::read(ctx).await?;
    let needs_probe = {
        let mut conn = ctx.app.db.acquire().await?;
        let state = rows::state(&mut conn, id, false)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let target = rows::of(&mut conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        target.verified_hash.is_none()
            && target.verifying_command.is_none()
            && (state.session.received != state.session.size || target.receiving_command.is_none())
    };
    if needs_probe {
        probe(ctx, id, options).await?;
    }
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let state = rows::state(&mut tx, id, true)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut target = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    session_operation::check_lifetime(ctx, &mut tx, &state.session, guard.user().id(), guard.now())
        .await?;
    creation::check_destination(ctx, &mut tx, guard.principal(), &target.destination).await?;
    if let Some(hash) = target.verified()? {
        return Ok(ready(&state, hash));
    }
    if state.session.received != state.session.size {
        return Err(out_of_place(state.session.received));
    }
    if let Some(previous) = target.verifying_command {
        let row = orch_db::queries::link::command(&mut tx, previous)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if !row.is_over() || !stopped(&mut tx, previous).await? {
            return Ok(UploadSessionCompletion::Verifying {
                retry_after_ms: options.retry_ms,
            });
        }
        // A native success is retained by the acknowledgement hook, independently of this HTTP read.
        if row.state == "succeeded" {
            return Err(ApiError::internal(
                "native verification ended without a matching upload receipt",
            ));
        }
        if let Some(error) = row.error {
            let error: orch_link_contract::commands::CommandError =
                serde_json::from_value(error).map_err(ApiError::internal)?;
            return Err(crate::routes::workspace::problems::Failure::Machine(error)
                .problem(crate::routes::workspace::problems::Doing::Writing));
        }
    }
    let command = orch_types::new_id();
    guard.check_deadline(ctx)?;
    rows::verifying(&mut tx, id, command).await?;
    target.verifying_command = Some(command);
    let seconds = u64::try_from(state.session.idle_secs).map_err(ApiError::internal)?;
    let command = Outgoing::new(
        command,
        target
            .destination
            .machine_id
            .ok_or_else(ApiError::not_found)?,
        SerialKey::None,
        &FsRelayVerify {
            environment_id: target.destination.environment_id,
            receive_path: target.destination.path.clone(),
            session_id: id,
            retention_secs: seconds,
        },
    )
    .map_err(ApiError::internal)?
    .by(options.until);
    native_command::issue::<Verify>(ctx, tx, &state, &target, command).await?;
    Ok(UploadSessionCompletion::Verifying {
        retry_after_ms: options.retry_ms,
    })
}

async fn probe(ctx: &Ctx, id: Id, options: crate::relay::Options) -> ApiResult<()> {
    let size = {
        let mut conn = ctx.app.db.acquire().await?;
        rows::state(&mut conn, id, false)
            .await?
            .ok_or_else(ApiError::not_found)?
            .session
            .size
    };
    let relay = ctx
        .app
        .blobs
        .relays()
        .create(
            options.concurrency,
            u64::try_from(size).map_err(ApiError::internal)?,
        )
        .await;
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let state = rows::state(&mut tx, id, true)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut target = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    session_operation::check_lifetime(ctx, &mut tx, &state.session, guard.user().id(), guard.now())
        .await?;
    creation::check_destination(ctx, &mut tx, guard.principal(), &target.destination).await?;
    if let Some(previous) = target.receiving_command
        && !stopped(&mut tx, previous).await?
    {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "the earlier native receive has not stopped",
        ));
    }
    let command = orch_types::new_id();
    guard.check_deadline(ctx)?;
    rows::receiving(&mut tx, id, command).await?;
    target.receiving_command = Some(command);
    let command = Outgoing::new(
        command,
        target
            .destination
            .machine_id
            .ok_or_else(ApiError::not_found)?,
        SerialKey::None,
        &FsRelayReceive {
            environment_id: target.destination.environment_id,
            path: target.destination.path.clone(),
            session_id: id,
            relay_id: relay.id(),
            size,
            end: 0,
            idle_secs: options.idle_secs,
            retry_ms: options.retry_ms,
            expires_at: Time::new(state.session.expires_at),
            retention_secs: u64::try_from(state.session.idle_secs).map_err(ApiError::internal)?,
        },
    )
    .map_err(ApiError::internal)?
    .by(options.until);
    let owner = PieceOwner {
        relay: Arc::clone(&relay),
        dispatcher: ctx.app.link.dispatcher(),
        id: command.id,
    };
    native_command::issue::<Verify>(ctx, tx, &state, &target, command).await?;
    let progress = owner
        .dispatcher
        .wait_for::<FsRelayReceive>(owner.id)
        .await
        .map_err(|error| {
            crate::routes::workspace::live::command_failure(error)
                .problem(crate::routes::workspace::problems::Doing::Writing)
        })?;
    if progress.received < 0 || progress.received > size {
        return Err(ApiError::internal(
            "the native destination reported an invalid prefix",
        ));
    }
    let now = ctx.app.clock.now();
    let expires = progress.expires_at.get();
    let mut tx = ctx.app.db.begin().await?;
    rows::progress(&mut tx, id, progress.received, now, expires).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Cancels the accepted native work and joins its actual private-file cleanup before answering.
pub(super) async fn cancel(ctx: &Ctx, id: Id) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let mut state = rows::state(&mut tx, id, true)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut target = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    check_session_for(ctx, &state.session, guard.user().id(), guard.now().get())?;
    if state.completed_at.is_some() && state.cancelled_at.is_some() {
        return Ok(());
    }
    let dispatcher = ctx.app.link.dispatcher();
    let previous = target.aborting_command;
    let reuse = if let Some(previous) = previous {
        let row = orch_db::queries::link::command(&mut tx, previous).await?;
        row.is_some_and(|row| !row.is_over() || row.state == "succeeded")
    } else {
        false
    };
    let queued = if reuse {
        tx.commit().await.map_err(DbError::from)?;
        previous
            .ok_or_else(|| ApiError::internal("the native cleanup lost its retained command"))?
    } else {
        let now = ctx.app.clock.now();
        guard.check_deadline(ctx)?;
        sqlx::query(
            "UPDATE upload_sessions SET cancelled_at=COALESCE(cancelled_at,$2) WHERE id=$1",
        )
        .bind(id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
        state.cancelled_at = Some(state.cancelled_at.unwrap_or(now));
        let command = orch_types::new_id();
        rows::aborting(&mut tx, id, command).await?;
        target.aborting_command = Some(command);
        let command = Outgoing::new(
            command,
            target
                .destination
                .machine_id
                .ok_or_else(ApiError::not_found)?,
            SerialKey::None,
            &FsRelayAbort {
                environment_id: target.destination.environment_id,
                receive_path: target.destination.path.clone(),
                session_id: id,
            },
        )
        .map_err(ApiError::internal)?;
        native_command::issue::<Cleanup>(ctx, tx, &state, &target, command).await?
    };
    for command in [target.receiving_command, target.verifying_command]
        .into_iter()
        .flatten()
    {
        dispatcher
            .cancel(command)
            .await
            .map_err(ApiError::internal)?;
    }
    dispatcher
        .wait_for::<FsRelayAbort>(queued)
        .await
        .map_err(|error| {
            crate::routes::workspace::live::command_failure(error)
                .problem(crate::routes::workspace::problems::Doing::Writing)
        })?;
    let mut tx = ctx.app.db.begin().await?;
    sqlx::query("UPDATE upload_sessions SET completed_at=COALESCE(completed_at,$2) WHERE id=$1 AND cancelled_at IS NOT NULL")
        .bind(id).bind(ctx.app.clock.now()).execute(&mut *tx).await.map_err(DbError::from)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
