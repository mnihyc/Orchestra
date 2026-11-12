//! A native opening carries its summaries under that opening's own declared source roles.

use super::*;

use crate::registry::OperationPlan;

use std::sync::Arc;

pub use orch_api_base::routes::summaries::embedded::Embedded;
/// Bind only the selected summary sources. An existing primary role can stay bound.
pub(crate) fn bind_sources(
    plan: &mut OperationPlan,
    slots: [Option<usize>; 3],
    selected: Option<&Embedded>,
) -> ApiResult<()> {
    for (slot, facts) in slots
        .into_iter()
        .zip(selected.map(Embedded::facts).unwrap_or_default())
    {
        let Some(index) = slot else { continue };
        let mut groups = BTreeMap::<_, Vec<_>>::new();
        for fact in facts {
            let group = groups.entry(fact.object.kind).or_default();
            if !group.contains(&fact) {
                group.push(fact);
            }
        }
        if groups.is_empty() {
            plan.bind_empty(index)?;
        } else {
            plan.bind_collection(index, &groups.into_iter().collect::<Vec<_>>())?;
        }
    }
    Ok(())
}

pub(crate) async fn prepare_target(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &Prepared,
    target: SummaryTarget,
    slots: [usize; 3],
) -> ApiResult<Option<Arc<Embedded>>> {
    let selected = optional_view(ctx, conn, caller, target, None, plan, slots).await?;
    Ok(selected.map(|view| {
        Arc::new(Embedded {
            views: vec![view],
            range: None,
            slots,
        })
    }))
}

/// The selected chat's catch-up and title, and the opening's own place (its topic or its thread) when it has
/// one. Each view has its own admission, so a view that is left out never drops its siblings.
pub(crate) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &Prepared,
    chat: Option<Id>,
    place: Option<SummaryTarget>,
    slots: [usize; 3],
) -> ApiResult<Option<Arc<Embedded>>> {
    if chat.is_none() && place.is_none() {
        return Ok(None);
    }
    let principal = Principal::load(
        conn,
        plan.preparation_caller(caller).map_err(source_error)?,
        ctx.app.clock.time(),
    )
    .await
    .map_err(ApiError::internal)?;
    let range = match chat {
        Some(chat) => sources::disclosure_range(conn, &principal, chat).await.ok(),
        None => None,
    };
    let mut targets = Vec::new();
    if let Some(chat) = chat {
        if let Some(range) = range {
            targets.push((SummaryTarget::Chat { id: chat }, Some(range)));
        }
        targets.push((SummaryTarget::ChatTitle { id: chat }, None));
    }
    if let Some(place) = place {
        targets.push((place, None));
    }
    let mut views = Vec::new();
    for (target, wanted) in targets {
        // Each optional source has its own admission; a missing view never drops its siblings.
        if let Ok(Some(view)) = optional_view(ctx, conn, caller, target, wanted, plan, slots).await
        {
            views.push(view);
        }
    }
    Ok((!views.is_empty()).then(|| {
        Arc::new(Embedded {
            views,
            range,
            slots,
        })
    }))
}
orch_api_base::__api_bind_e93ed15ce0!();

async fn optional_view(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    target: SummaryTarget,
    wanted: Option<SummaryChatRange>,
    plan: &Prepared,
    slots: [usize; 3],
) -> ApiResult<Option<View>> {
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
    read_view(ctx, conn, caller, target, wanted, plan, slots)
        .await
        .map(Some)
}

/// The actual original opening caller and every source are restored at the final byte boundary.
pub(crate) fn retain(ctx: &Ctx, principal: &Principal, selected: &Arc<Embedded>) -> ApiResult<()> {
    let caller = principal.caller();
    let selection = caller
        .admitted_operation()
        .unwrap_or(ctx.route.name)
        .to_owned();
    publication::retain(
        ctx,
        caller,
        (Arc::clone(selected), selection),
        |ctx, conn, current, (old, selection)| {
            Box::pin(async move {
                let plan = Prepared::new(ctx.route, selection).map_err(source_error)?;
                for view in &old.views {
                    let target = view.identity.target();
                    let wanted = if matches!(target, SummaryTarget::Chat { .. }) {
                        old.range
                    } else {
                        None
                    };
                    let fresh = optional_view(
                        ctx,
                        conn,
                        current.caller(),
                        target,
                        wanted,
                        &plan,
                        old.slots,
                    )
                    .await?
                    .ok_or_else(ApiError::not_found)?;
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
