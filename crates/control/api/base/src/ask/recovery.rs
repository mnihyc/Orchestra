//! An expired runner ends its accepted work; new work needs a fresh user request.

use super::*;

use std::time::Duration;

use tokio_util::sync::CancellationToken;
/// Four missed thirty-second heartbeats distinguish a lost runner from ordinary scheduling delay.
const LEASE_SECONDS: i64 = orch_engine::ask::command::CONTROLLER_LEASE_SECONDS;

const HEARTBEAT: Duration = Duration::from_secs(30);

/// Matches the ordinary supervisor's bounded lifetime page.
const BATCH: i64 = 200;

pub(super) fn alive(
    heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    orch_engine::ask::command::controller_alive(heartbeat, now)
}

pub(crate) fn install(app: &crate::state::App) {
    if !app.engine.install_ask_runtime() {
        return;
    }
    let weak = Arc::downgrade(app);
    tokio::spawn(async move {
        loop {
            let Some(app) = weak.upgrade() else {
                return;
            };
            if app.engine.ask_runtime_stopped() {
                return;
            }
            if let Err(error) = run_due(&app).await {
                tracing::warn!(%error,"Ask runner cleanup could not complete");
            }
            drop(app);
            tokio::time::sleep(HEARTBEAT).await;
        }
    });
}

pub(super) fn watch(
    app: crate::state::App,
    thread: Id,
    turn: Id,
    owner: Id,
    done: CancellationToken,
    lost: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! { () = done.cancelled() => return, () = tokio::time::sleep(HEARTBEAT) => {} }
            if app.engine.ask_runtime_stopped() {
                lost.cancel();
                return;
            }
            let renewed = async {
                let mut conn = app.db.acquire().await?;
                let now = app.clock.now();
                rows::heartbeat(
                    &mut conn,
                    thread,
                    turn,
                    owner,
                    now,
                    now - chrono::Duration::seconds(LEASE_SECONDS),
                )
                .await
            }
            .await;
            match renewed {
                Ok(true) => {}
                Ok(false) => {
                    lost.cancel();
                    return;
                }
                Err(error) => {
                    tracing::warn!(%error,"Ask runner heartbeat ended");
                    lost.cancel();
                    return;
                }
            }
        }
    });
}

/// Bounded native cleanup. It neither restores a user nor starts another model step.
pub(crate) async fn run_due(app: &crate::state::App) -> ApiResult<usize> {
    let now = app.clock.now();
    let mut tx = app.db.begin().await?;
    let ids = rows::orphaned(
        &mut tx,
        now - chrono::Duration::seconds(LEASE_SECONDS),
        BATCH,
    )
    .await?;
    let mut calls = Vec::new();
    let mut closed = 0;
    for (id, turn) in ids {
        calls.extend(rows::calls(&mut tx, turn).await?);
        let seq = orch_db::sequences::next(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let error = json!({"code":"ask_controller_unavailable","message":"The Ask controller became unavailable. Send a follow-up to continue.","retryable":true});
        threads::finish_turn(&mut tx, turn, "interrupted", Some(&error), now, seq).await?;
        threads::settle_items_of_turn(&mut tx, turn, "cancelled", now, seq).await?;
        rows::finish(&mut tx, id, turn).await?;
        rows::emit(
            &mut tx,
            id,
            &json!({"kind":"notice","code":"controller_unavailable","message":error["message"]}),
        )
        .await?;
        rows::emit(&mut tx, id, &json!({"kind":"reset"})).await?;
        closed += 1;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    app.engine
        .cancel_ask_calls(&calls)
        .await
        .map_err(ApiError::engine)?;
    Ok(closed)
}
