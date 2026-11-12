//! The ordinary trial, stored fingerprint and native move stay on one guarded transaction.

pub use super::*;

use crate::key_operations::native::effects::Apply;

use crate::key_operations::native::effects::Outcome;

pub use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::HttpIdentityMode;

use futures::future::BoxFuture;

pub use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::thread_move::ThreadMoveReceipt;

use orch_authz::CapturedOperation;
orch_api_base::api_family_callback! {
    bind __api_bind_3d9ed592e3, __api_install_3d9ed592e3, __api_callback_3d9ed592e3;
    slot __ApiCallback3d9ed592e3, __API_CALLBACK_3D9ED592E3, __api_set_3d9ed592e3;
    base orch_api_base::routes::nodes::moves::thread::writing;

    fn boxed(arg0: orch_api_base::middleware::requests::ApiError) -> orch_engine::EngineError;
}

impl Apply for operation::Preview {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a operation::Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Reviewed>>> {
        Box::pin(async move {
            let mut first = Some(before);
            let id = new_id();
            let until = Time::new(ctx.now + PLAN_LIFETIME);
            let (plan, content) = review(
                ctx,
                conn,
                principal,
                &value.base,
                ReviewStamp { id, until },
                &mut first,
                None,
            )
            .await?;
            let sources = operation::ReviewSources::of(&value.base, content);
            let origin = operation::Origin {
                operation: CapturedOperation::capture(principal.caller())
                    .map_err(ApiError::internal)?,
                session: match ctx.user()?.via {
                    crate::middleware::authentication::Via::Session { session_id } => {
                        Some(session_id)
                    }
                    crate::middleware::authentication::Via::Token { .. }
                    | crate::middleware::authentication::Via::OwnerRead { .. } => None,
                },
            };
            let kept = operation::Kept {
                fingerprint: fingerprint(conn, &plan).await?,
                review: plan.clone(),
                sources: Some(sources.clone()),
                origin: Some(origin),
            };
            // The trial was fully rolled back. Recheck the real HTTP row lifetime before
            // keeping this review, independently of the optional content projection.
            let identity =
                transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Action)
                    .await?;
            identity.check_deadline(&ctx.app)?;
            operation::check_first(ctx, &mut first, None).map_err(ApiError::engine)?;
            jobs::forks::save_preflight(
                conn,
                &jobs::forks::PreflightRow {
                    id,
                    kind: KIND.to_owned(),
                    node_id: Some(value.base.row.node_id),
                    environment_id: None,
                    created_by: principal.user(),
                    plan: serde_json::to_value(&kept).map_err(ApiError::internal)?,
                    expires_at: until.get(),
                    created_at: ctx.now,
                },
            )
            .await?;
            identity.check_deadline(&ctx.app)?;
            Ok(Outcome::Applied(Reviewed {
                id,
                actor: principal.user(),
                plan,
                sources,
            }))
        })
    }
}

impl Apply for operation::Move {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a operation::Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<MovedThread>>> {
        Box::pin(async move {
            let confirmation = value
                .confirmation
                .as_ref()
                .ok_or_else(|| ApiError::internal("a confirmed native move has no owned review"))?;
            let original = Some(operation::original(ctx, conn, confirmation).await?);
            let mut first = Some(before);
            let (plan, _) = review(
                ctx,
                conn,
                principal,
                &value.base,
                ReviewStamp {
                    id: confirmation.body.preflight_id,
                    until: confirmation.until,
                },
                &mut first,
                original.as_ref(),
            )
            .await?;
            if let Some(blocker) = plan.movement.blockers.first() {
                let code = if blocker.code == "thread.not_idle" {
                    ErrorCode::ThreadNotIdle
                } else {
                    ErrorCode::NodeWrongParent
                };
                return Err(ApiError::new(code, blocker.message.clone()));
            }
            if confirmation.body.expected_revision != plan.expected_revision
                || confirmation.kept.fingerprint != fingerprint(conn, &plan).await?
            {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the access or inherited settings of this move changed; review them again",
                ));
            }
            let by = orch_engine::views::user_ref_json(conn, principal.user())
                .await
                .map_err(ApiError::engine)?;
            let marker = thread_move::marker(
                serde_json::to_value(&plan.movement.from).map_err(ApiError::internal)?,
                serde_json::to_value(&plan.movement.to).map_err(ApiError::internal)?,
                by,
            );
            let identity =
                transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Action)
                    .await?;
            thread_move::write_before(
                conn,
                principal,
                &value.base.lanes,
                value.base.row.node_id,
                value.base.target,
                marker,
                &format!("thread-move:{}", confirmation.body.preflight_id),
                || {
                    identity.check_deadline(&ctx.app).map_err(boxed)?;
                    operation::check_first(ctx, &mut first, original.as_ref())
                },
            )
            .await
            .map_err(ApiError::engine)?;
            epoch::bump(conn).await.map_err(ApiError::internal)?;
            let row = &value.base.row;
            let who = super::super::actor_name(ctx)?;
            let summary = format!("{who} moved {} to another topic", row.name);
            let details = json!({"thread_id":row.node_id,"from":row.topic_id,"to":value.base.target,"owner_id":row.owner_id,"environment_id":row.environment_id,"directory":row.workspace_dir});
            crate::routes::nodes::record_about(
                ctx,
                conn,
                orch_db::audit::action!(AuditAction::ThreadMoved),
                row.node_id,
                crate::routes::nodes::Placed {
                    id: row.node_id,
                    org: row.org_id,
                },
                &summary,
                details,
            )
            .await?;
            jobs::forks::forget_preflight(conn, confirmation.body.preflight_id).await?;
            let revision = hierarchy::with_ancestors(conn, &[row.node_id])
                .await?
                .into_iter()
                .find(|row| row.id == value.base.row.node_id)
                .ok_or_else(ApiError::not_found)?
                .revision;
            identity.check_deadline(&ctx.app)?;
            if let Some(original) = &original {
                original.check(ctx)?;
            }
            Ok(Outcome::Applied(MovedThread {
                receipt: ThreadMoveReceipt {
                    projection: ReceiptProjection::Receipt,
                    thread_id: row.node_id,
                    parent_id: value.base.target,
                    revision,
                },
                original,
            }))
        })
    }
}
