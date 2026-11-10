//! A native opening carries its summaries under that opening's own declared source roles.

use super::*;

use std::sync::Arc;

pub use orch_api_base::routes::summaries::embedded::Embedded;
pub(crate) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &Prepared,
    chat: Id,
    slots: [usize; 3],
) -> ApiResult<Option<Arc<Embedded>>> {
    let target = SummaryTarget::Chat { id: chat };
    let identity = sources::identify(conn, target)
        .await
        .map_err(source_error)?;
    if !plan
        .permits_binding(
            caller,
            slots[0],
            identity.fact().object.kind,
            std::slice::from_ref(identity.fact()),
        )
        .map_err(source_error)?
    {
        return Ok(None);
    }
    let principal = Principal::load(
        conn,
        plan.preparation_caller(caller).map_err(source_error)?,
        ctx.app.clock.time(),
    )
    .await
    .map_err(ApiError::internal)?;
    let range = sources::disclosure_range(conn, &principal, chat)
        .await
        .map_err(source_error)?;
    let mut views = Vec::new();
    for target in [target, SummaryTarget::ChatTitle { id: chat }] {
        let wanted = matches!(target, SummaryTarget::Chat { .. }).then_some(range);
        views.push(read_view(ctx, conn, caller, target, wanted, plan, slots).await?);
    }
    Ok(Some(Arc::new(Embedded {
        views,
        range,
        slots,
    })))
}
orch_api_base::__api_bind_e93ed15ce0!();

/// The actual original opening caller and every source are restored at the final byte boundary.
pub(crate) fn retain(ctx: &Ctx, principal: &Principal, selected: &Arc<Embedded>) -> ApiResult<()> {
    publication::retain(
        ctx,
        principal.caller(),
        Arc::clone(selected),
        |ctx, conn, current, old| {
            Box::pin(async move {
                let plan = Prepared::new(ctx.route, ctx.route.name).map_err(source_error)?;
                for view in &old.views {
                    let target = view.identity.target();
                    let wanted = matches!(target, SummaryTarget::Chat { .. }).then_some(old.range);
                    let fresh = read_view(
                        ctx,
                        conn,
                        current.caller(),
                        target,
                        wanted,
                        &plan,
                        old.slots,
                    )
                    .await?;
                    if !same_view(view, &fresh) {
                        return Err(ApiError::not_found());
                    }
                }
                Ok(())
            })
        },
    )
}
orch_api_base::__api_bind_7e5709b692!();

/// Every inspected chat contributing words keeps the ordinary once-only audit door.
pub(crate) async fn note_read(
    conn: &mut Conn,
    principal: &Principal,
    selected: &Embedded,
) -> ApiResult<()> {
    let facts = selected
        .views
        .iter()
        .flat_map(|view| {
            std::iter::once(view.identity.fact().clone()).chain(view.inputs.iter().cloned())
        })
        .collect::<Vec<_>>();
    note_chat_openings(conn, principal, &facts).await
}
orch_api_base::__api_bind_face5ea742!();
