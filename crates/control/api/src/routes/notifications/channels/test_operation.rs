//! Actual sample sources, current before-delivery admission and exact accepted reconciliation.

use super::*;
use crate::key_operations::{native, publication};
use crate::middleware::authentication::Via;
use crate::middleware::transaction_guard::Guard;
use futures::future::BoxFuture;
use orch_authz::Caller;
use orch_notifier_contract::delivery::Delivery;
use orch_worker_core::channels::{TestOutcome, TestPrepared};
use std::sync::Arc;

struct Sample {
    row: ChannelRow,
    recipient_timezone: String,
    route: Option<route_rows::RouteRow>,
    body: NotificationChannelTestBody,
    editor: bool,
}

pub(super) async fn require(
    conn: &mut Conn,
    principal: &Principal,
    row: &ChannelRow,
    to: Option<&str>,
) -> ApiResult<bool> {
    let result = async {
        let me = principal.user();
        if row.user_id.is_some_and(|person| person != me) {
            return Err(ApiError::not_found());
        }
        let editor = may_change(conn, principal, row, &mut HashMap::new()).await?;
        if !editor && !pages::is_routed(conn, me, row.id).await? {
            require_change(conn, principal, row).await?;
            return Err(not_for_the_caller(row.id));
        }
        if to.is_some() && !editor {
            return Err(ApiError::invalid(
                "to",
                "is given by whoever looks after the channel; for you it writes to you",
            ));
        }
        Ok(editor)
    }
    .await;
    result.map_err(|error| {
        if native::modern(principal.caller())
            && matches!(error.problem.status, 401 | 403 | 404 | 410)
        {
            authorization::problem_without_content(error)
        } else {
            error
        }
    })
}

async fn value(ctx: &Ctx, conn: &mut Conn, guard: &Guard) -> ApiResult<ChannelRow> {
    if native::modern(guard.principal().caller()) {
        Ok(guard
            .operation_data::<native::Prepared<ChannelRow>>()?
            .value
            .clone())
    } else {
        channel_of(conn, ctx).await
    }
}

fn same_channel(old: &ChannelRow, fresh: &ChannelRow) -> bool {
    old.id == fresh.id
        && old.org_id == fresh.org_id
        && old.user_id == fresh.user_id
        && old.kind == fresh.kind
        && old.name == fresh.name
        && old.config == fresh.config
        && old.templates == fresh.templates
        && old.created_by == fresh.created_by
        && old.created_at == fresh.created_at
}

fn same_route(old: &route_rows::RouteRow, fresh: &route_rows::RouteRow) -> bool {
    old.id == fresh.id
        && old.user_id == fresh.user_id
        && old.channel_id == fresh.channel_id
        && old.address == fresh.address
        && old.linked_at == fresh.linked_at
}

fn closed() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the original sample source is no longer current",
    )
}

async fn locks(
    conn: &mut Conn,
    channel: Id,
    actor: Id,
    route: Option<Id>,
    delivery: Option<&Delivery>,
) -> ApiResult<()> {
    rows::lock_channel_shared(conn, channel)
        .await?
        .ok_or_else(ApiError::not_found)?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR SHARE")
        .bind(actor)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    if let Some(route) = route {
        sqlx::query("SELECT id FROM notification_routes WHERE id=$1 FOR SHARE")
            .bind(route)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
    }
    // These are native prepared address operands, not scoped business targets.
    if let Some(device) = delivery
        .and_then(|delivery| delivery.to.get("id"))
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
    {
        sqlx::query("SELECT id FROM push_subscriptions WHERE id=$1 AND user_id=$2 FOR SHARE")
            .bind(device)
            .bind(actor)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
    }
    Ok(())
}

async fn before(ctx: &Ctx, sample: &Sample, delivery: &Delivery) -> ApiResult<()> {
    let (mut tx, first) = transaction_guard::begin(ctx).await?;
    locks(
        &mut tx,
        sample.row.id,
        first.principal().user(),
        sample.route.as_ref().map(|row| row.id),
        Some(delivery),
    )
    .await?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let row = value(&current, &mut tx, &guard).await?;
    require(&mut tx, guard.principal(), &row, sample.body.to.as_deref()).await?;
    if !same_channel(&sample.row, &row) {
        return Err(closed());
    }
    let actor = guard.principal().user();
    let recipient = people::recipient(&mut tx, actor)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let route = route_rows::route_for(&mut tx, actor, row.id).await?;
    if recipient.timezone != sample.recipient_timezone {
        return Err(closed());
    }
    let descriptor = notifiers()
        .get(&row.kind)
        .ok_or_else(ApiError::not_found)?
        .describe();
    if matches!(
        descriptor.reaches,
        orch_notifier_contract::descriptor::Reaches::Link
    ) && !matches!((&sample.route, &route), (Some(old), Some(fresh)) if same_route(old, fresh))
    {
        return Err(closed());
    }
    let around = surroundings(&current);
    if !channels::test_delivery_is_current(
        &mut tx,
        &toolbox(master_key()?, &around),
        &TestFor {
            channel: &row,
            caller: &recipient,
            route_address: route.as_ref().map(|route| &route.address),
            to: sample.body.to.as_deref(),
            device: sample.body.device_id,
        },
        delivery,
    )
    .await
    .map_err(internal)?
    {
        return Err(closed());
    }
    guard.check_deadline(&current)?;
    tx.commit().await.map_err(DbError::from)?;
    guard.check_deadline(&current)
}

async fn remember(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    sample: &Sample,
    outcome: &TestOutcome,
    route_taken: bool,
) -> ApiResult<bool> {
    let Some(row) = rows::channel(conn, sample.row.id).await? else {
        return Ok(false);
    };
    if !same_channel(&sample.row, &row) {
        return Ok(false);
    }
    let mut changed = false;
    if sample.editor && may_change(conn, principal, &row, &mut HashMap::new()).await? {
        match &outcome.error {
            Some(error) => rows::record_channel_error(conn, row.id, error).await?,
            None => rows::record_channel_ok(conn, row.id, ctx.now).await?,
        }
        changed = true;
    }
    if route_taken
        && let Some(old) = &sample.route
        && let Some(fresh) = route_rows::route_for(conn, principal.user(), row.id).await?
        && same_route(old, &fresh)
    {
        route_rows::record_test_taken(conn, fresh.id, ctx.now).await?;
        changed = true;
    }
    Ok(changed)
}

/// This accepts only an actual completed TestRun's outcome from run below. Its original
/// actor is evidence for exact owned health reconciliation, never authority for another send.
async fn accepted_cleanup(
    ctx: &Ctx,
    accepted: &Caller,
    sample: &Sample,
    outcome: &TestOutcome,
    route_taken: bool,
) -> ApiResult<()> {
    let mut tx = ctx.app.db.begin().await?;
    if rows::lock_channel_for_write(&mut tx, sample.row.id)
        .await?
        .is_none()
    {
        return Ok(());
    }
    let mut caller = accepted.clone();
    if let Via::Session { session_id } = &ctx.user()?.via {
        let session =
            orch_db::queries::tokens::lock_session_state(&mut tx, *session_id, accepted.user())
                .await?;
        let admin = accepted.as_admin()
            && session.is_some_and(|(mode, end)| mode && end > ctx.app.clock.now());
        caller = caller.with_admin_mode(admin);
    }
    let principal = match Principal::load(&mut tx, caller, ctx.app.clock.time()).await {
        Ok(principal) => principal,
        Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => {
            return Ok(());
        }
        Err(error) => return Err(internal(error)),
    };
    let mut current = ctx.clone();
    current.now = ctx.app.clock.now();
    remember(&current, &mut tx, &principal, sample, outcome, route_taken).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

pub(super) async fn run(
    ctx: Ctx,
    body: NotificationChannelTestBody,
) -> ApiResult<NotificationTestReply> {
    let (prepared, sample, accepted, ctx) = {
        let (mut tx, first) = transaction_guard::begin(&ctx).await?;
        let row = value(&ctx, &mut tx, &first).await?;
        require(&mut tx, first.principal(), &row, body.to.as_deref()).await?;
        let actor = first.principal().user();
        let route = route_rows::route_for(&mut tx, actor, row.id).await?;
        locks(
            &mut tx,
            row.id,
            actor,
            route.as_ref().map(|row| row.id),
            None,
        )
        .await?;
        let (mut current, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
        current.now = guard.now().get();
        let row = value(&current, &mut tx, &guard).await?;
        let editor = require(&mut tx, guard.principal(), &row, body.to.as_deref()).await?;
        let accepted = guard.principal().caller().clone();
        let recipient = people::recipient(&mut tx, actor)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let route = route_rows::route_for(&mut tx, actor, row.id).await?;
        let around = surroundings(&current);
        let prepared = channels::prepare_test(
            &mut tx,
            &toolbox(master_key()?, &around),
            &TestFor {
                channel: &row,
                caller: &recipient,
                route_address: route.as_ref().map(|route| &route.address),
                to: body.to.as_deref(),
                device: body.device_id,
            },
            current.now,
        )
        .await
        .map_err(|error| match error {
            channels::ChannelError::UnknownKind(kind) => {
                ApiError::invalid("kind", &format!("there is no notifier named {kind}"))
            }
            other => internal(other),
        })?;
        let sample = Arc::new(Sample {
            row,
            recipient_timezone: recipient.timezone,
            route,
            body,
            editor,
        });
        guard.check_deadline(&current)?;
        tx.commit().await.map_err(DbError::from)?;
        guard.check_deadline(&current)?;
        (
            prepared,
            sample,
            accepted.clone(),
            current.with_operation_ceiling(accepted)?,
        )
    };
    match prepared {
        TestPrepared::Ready(ready) => {
            let checking = ctx.clone();
            let original = Arc::clone(&sample);
            let run =
                channels::run_test_before(ready, move |delivery| -> BoxFuture<'_, ApiResult<()>> {
                    let ctx = checking.clone();
                    let source = Arc::clone(&original);
                    Box::pin(async move { before(&ctx, &source, delivery).await })
                })
                .await?;
            let reply = NotificationTestReply {
                ok: run.outcome().ok,
                error: run.outcome().error.clone(),
            };
            publication::receipt(&ctx, &reply)?;
            // Accepted notifier work is never replayed because its health observer failed.
            if let Err(error) = accepted_cleanup(
                &ctx,
                &accepted,
                &sample,
                run.attempted_outcome(),
                run.outcome().ok,
            )
            .await
            {
                tracing::warn!(%error, "a completed sample result could not be recorded");
            }
            Ok(reply)
        }
        TestPrepared::Done(outcome) => {
            let (mut tx, _) = transaction_guard::begin(&ctx).await?;
            rows::lock_channel_for_write(&mut tx, sample.row.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let (mut current, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
            current.now = guard.now().get();
            let row = value(&current, &mut tx, &guard).await?;
            require(&mut tx, guard.principal(), &row, sample.body.to.as_deref()).await?;
            guard.check_deadline(&current)?;
            let changed = remember(
                &current,
                &mut tx,
                guard.principal(),
                &sample,
                &outcome,
                outcome.ok,
            )
            .await?;
            guard.check_deadline(&current)?;
            tx.commit().await.map_err(DbError::from)?;
            let reply = NotificationTestReply {
                ok: outcome.ok,
                error: outcome.error,
            };
            if changed {
                publication::receipt(&ctx, &reply)?;
            } else {
                publication::retain(
                    &ctx,
                    &accepted,
                    Arc::clone(&sample),
                    |ctx, conn, principal, sample| {
                        Box::pin(async move {
                            let row = channel_of(conn, ctx).await?;
                            if !same_channel(&sample.row, &row) {
                                return Err(closed());
                            }
                            let (family, owner) = operation::owner(conn, &row).await?;
                            let plan = native::bind_required(
                                ctx,
                                &orch_api_contract::notification_sources::CHANNEL_TEST,
                                &[native::Facts::one(
                                    orch_api_contract::notification_sources::Source::ChannelOwner,
                                    family,
                                    owner,
                                )],
                            )?;
                            let current =
                                native::preparation_principal(ctx, conn, principal.caller(), &plan)
                                    .await?;
                            require(conn, &current, &row, sample.body.to.as_deref()).await?;
                            Ok(())
                        })
                    },
                )?;
            }
            Ok(reply)
        }
    }
}
