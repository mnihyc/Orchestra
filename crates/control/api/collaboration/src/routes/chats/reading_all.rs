//! A selected native collection, a rollbackable state-row prefix, then the current guarded write.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::chat_sources as declarations;

use orch_api_contract::chat_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use std::sync::Arc;
const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);

pub(super) struct Reads;

fn work(ctx: &Ctx, principal: &Principal) -> Work {
    Work {
        projection_ctx: ctx.clone(),
        principal: principal.clone(),
        origin: origin(ctx),
        now: principal.now().get(),
    }
}

async fn require_org(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
    if !native::modern(principal.caller()) {
        return authorization::require_on_path_in(ctx, conn, principal, "node.view").await;
    }
    // The operation has no existing-content response. Keep its native permission decision,
    // without constructing a named permission page merely to refuse a write-only caller.
    match orch_authz::nodes::check_named(
        conn,
        principal,
        ctx.id("id")?,
        authorization::VIEW,
        Act::Use,
        Policy::default(),
    )
    .await
    {
        Ok(decision) if decision.is_allowed() => Ok(()),
        Ok(_) => Err(denied_for(
            None,
            "node.view",
            "the caller lacks the required permission",
        )),
        Err(orch_authz::Error::NotFound(_)) => Err(ApiError::not_found()),
        Err(orch_authz::Error::Suspended { org }) => {
            Err(failed_projected(conn, principal, None, ChatError::Suspended { org }, true).await)
        }
        Err(orch_authz::Error::Archived { node }) => {
            Err(failed_projected(conn, principal, None, ChatError::Archived { node }, true).await)
        }
        Err(error) => Err(ApiError::internal(error)),
    }
}

async fn refuse<T>(
    conn: &mut Conn,
    principal: &Principal,
    result: Result<T, ChatError>,
) -> ApiResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => Err(failed_projected(
            conn,
            principal,
            None,
            error,
            native::modern(principal.caller()),
        )
        .await),
    }
}

impl native::Selected for Reads {
    type Source = Source;
    type Query = ();
    type Value = Arc<following::AllReadSelection>;
    type Reply = ();
    const DECLARATION: &'static Declaration<Source> = &declarations::READ_ALL;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let base = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let principal = native::preparation_principal(ctx, conn, caller, &base).await?;
            require_org(ctx, conn, &principal).await?;
            let id = ctx.id("id")?;
            let root = hierarchy::with_ancestors(conn, &[id])
                .await?
                .into_iter()
                .find(|node| node.id == id && node.kind == "org")
                .map(|node| node.org_id)
                .ok_or_else(ApiError::not_found)?;
            let acting = work(ctx, &principal);
            let result = match ctx.optional_retained_source::<following::AllReadSelection>()? {
                Some(original) => {
                    if original.root() != root {
                        return Err(ApiError::not_found());
                    }
                    following::refresh_all_read(conn, &acting.acting(), &original).await
                }
                None => following::select_all_read(conn, &acting.acting(), root).await,
            };
            let selected = refuse(conn, &principal, result).await?;
            let rows: Vec<_> = selected.chats().cloned().collect();
            let actual = targets::from_chat_rows(conn, &rows)
                .await?
                .into_values()
                .collect::<Vec<_>>();
            if actual.len() != rows.len() {
                return Err(ApiError::not_found());
            }
            let facts = native::Facts::collection(
                Source::AllReadChats,
                if actual.is_empty() {
                    Vec::new()
                } else {
                    vec![(CHAT, actual)]
                },
            );
            Ok((Arc::new(selected), vec![facts]))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require_org(ctx, conn, principal).await })
    }

    // This factory supplies operands to the native action guard, never a read-only projection path.
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
}

async fn selected(
    ctx: &Ctx,
    conn: &mut Conn,
    guard: &transaction_guard::Guard,
) -> ApiResult<Arc<following::AllReadSelection>> {
    if native::modern(guard.principal().caller()) {
        return Ok(Arc::clone(
            &guard
                .operation_data::<native::Prepared<Arc<following::AllReadSelection>>>()?
                .value,
        ));
    }
    Ok(
        <Reads as native::Selected>::select(ctx, conn, guard.principal().caller(), &())
            .await?
            .0,
    )
}

struct Prefix {
    source: Arc<following::AllReadSelection>,
    principal: Principal,
    identity: transaction_guard::CurrentHttpIdentity,
}

/// Admit the actual selection, then release credential locks before the native state-row wait.
/// Only the following action transaction may consume the rollbackable prefix after a fresh guard.
async fn prefix(ctx: &Ctx) -> ApiResult<Prefix> {
    let mut tx = ctx.app.db.begin().await?;
    let identity = transaction_guard::current_http_identity(
        ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    transaction_guard::guard_user(&identity.user)?;
    let (source, facts) =
        <Reads as native::Selected>::select(ctx, &mut tx, &identity.caller, &()).await?;
    let plan = native::bind(ctx, &declarations::READ_ALL, &facts)?;
    let principal = native::preparation_principal(ctx, &mut tx, &identity.caller, &plan).await?;
    <Reads as native::Selected>::require(ctx, &mut tx, &principal).await?;
    identity.check_deadline(&ctx.app)?;
    tx.commit().await.map_err(DbError::from)?;
    identity.check_deadline(&ctx.app)?;
    Ok(Prefix {
        source,
        principal,
        identity,
    })
}

pub(super) async fn apply(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let initial = prefix(&ctx).await?;
    let ctx = ctx.with_operation_ceiling(initial.principal.caller().clone())?;
    let mut tx = ctx.app.db.begin().await?;
    // These are genuine native operands, not target admission. Every later resolver must reload
    // current governors and ordinary authority for the same IDs and original sequence bounds.
    ctx.retain_source(initial.source.as_ref().clone())?;
    let acting = work(&ctx, &initial.principal);
    let staged = following::stage_all_read(&mut tx, &acting.acting(), &initial.source).await;
    let ready = refuse(&mut tx, &initial.principal, staged).await?;
    initial.identity.check_deadline(&ctx.app)?;
    let (current, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let source = selected(&current, &mut tx, &guard).await?;
    if !ready.matches(&source) {
        return Err(ApiError::not_found());
    }
    initial.identity.check_deadline(&current.app)?;
    guard.check_deadline(&current)?;
    let acting = work(&current, guard.principal());
    let result = following::apply_all_read(&mut tx, &acting.acting(), &ready, &source).await;
    refuse(&mut tx, guard.principal(), result).await?;
    initial.identity.check_deadline(&current.app)?;
    guard.check_deadline(&current)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
