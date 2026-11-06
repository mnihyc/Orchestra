//! Current readable setup observations and atomic portable first-steps choices.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{invalid, view};
use orch_access_contract::pages::PageKind;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::{PlaceRef, Role};
use orch_api_contract::first_steps::*;
use orch_api_contract::nodes::{Node, NodeRef};
use orch_api_contract::personal::{PERSONAL_STATE_MAX_BYTES, PersonalState};
use orch_authz::engine::{Frame, PageRef};
use orch_authz::giving::{Acting, Giving};
use orch_authz::things::{self, Thing};
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::{first_steps as rows, identity};
use orch_types::Id;

mod read;

/// Registers both declared first-steps requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(
            MeFirstStepsGet,
            crate::key_operations::native::selected::<read::Setup>,
            crate::key_operations::native::selected_read::<read::Setup>,
        )
        .on(MeFirstStepsSet, set);
}

async fn state(conn: &mut Conn, user: Id) -> ApiResult<PersonalState> {
    Ok(identity::interface_state(conn, user)
        .await?
        .into_iter()
        .collect())
}

async fn readable_org(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<orch_api_contract::nodes::Org> {
    authorization::require(
        conn,
        principal,
        id,
        authorization::VIEW,
        Act::Use,
        Policy::default(),
    )
    .await?;
    match view::visible(conn, principal, &[id], ctx.now, Policy::default())
        .await?
        .pop()
    {
        Some(Node::Org(org)) => Ok(org),
        _ => Err(ApiError::not_found()),
    }
}

fn action(url: String, allowed: bool) -> Option<FirstStepsAction> {
    Some(FirstStepsAction {
        url,
        allowed,
        reason: None,
    })
}
fn step(
    id: FirstStepsId,
    fact: FirstStepsFact,
    source: Option<NodeRef>,
    action: Option<FirstStepsAction>,
    scripted: bool,
) -> FirstStepsStep {
    let state = match fact {
        FirstStepsFact::Unknown => FirstStepsState::Unknown,
        FirstStepsFact::ChooseOrg | FirstStepsFact::NotDone | FirstStepsFact::InvitationWaiting => {
            FirstStepsState::Open
        }
        _ => FirstStepsState::Done,
    };
    FirstStepsStep {
        id,
        state,
        scripted,
        fact,
        source,
        action,
    }
}

async fn role_summaries(
    conn: &mut Conn,
    principal: &Principal,
    org: Id,
) -> ApiResult<Vec<InvitationRoleSummary>> {
    let (graph, site) = things::site(conn, Thing::Node(org))
        .await
        .map_err(ApiError::internal)?;
    let frame = Frame::load_with(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[PageRef::defaults_of(PlaceRef::Node(org))],
        Permission::ALL,
    )
    .await
    .map_err(ApiError::internal)?;
    let giving = Giving::of(&frame, Some(&site), PageKind::Live, Acting::Ordinary);
    Ok([
        (orch_api_contract::access::Role::Viewer, Role::Viewer),
        (
            orch_api_contract::access::Role::Interactor,
            Role::Interactor,
        ),
        (orch_api_contract::access::Role::Developer, Role::Developer),
        (orch_api_contract::access::Role::Admin, Role::Admin),
    ]
    .into_iter()
    .map(|(role, native)| {
        let held = giving.role_permissions(native);
        InvitationRoleSummary {
            role,
            start_topics: held.contains(&Permission::ProjectCreateTopics),
            start_threads: held.contains(&Permission::TopicStartThreads),
            send_inputs: held.contains(&Permission::ThreadInteract),
            answer_requests: held.contains(&Permission::ThreadManage),
            model_access_required: true,
        }
    })
    .collect())
}

async fn start_here_row(
    conn: &mut Conn,
    principal: &Principal,
    org: Id,
    root: Id,
) -> ApiResult<
    Option<(
        orch_db::queries::documents::DocumentRow,
        orch_authz::documents::Listed,
    )>,
> {
    let key = "org.start_here_document_id";
    if orch_settings_contract::registry::get(key).is_none() {
        return Ok(None);
    }
    let Some(id) = orch_settings::get::<Id>(conn, orch_settings::Place::Node(org), key)
        .await
        .map_err(ApiError::internal)?
    else {
        return Ok(None);
    };
    let held = orch_authz::documents::listed(conn, principal, &[id], Policy::default())
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .find(|held| held.holds(Permission::DocView));
    let Some(held) = held else {
        return Ok(None);
    };
    let Some(row) = orch_db::queries::documents::get(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.org_id == root)
    else {
        return Ok(None);
    };
    Ok(Some((row, held)))
}

/// Internal observation handoff; the standalone reader retains its existing DTO.
pub(crate) struct ObservedFirstSteps {
    pub(crate) facts: FirstSteps,
    pub(crate) outside: Option<orch_api_contract::notifications::OutsideStatus>,
}

/// The overview consumes the same corrected observation cohort under its own scope.
pub(crate) async fn read_in(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: FirstStepsGetQuery,
    initial: Option<orch_api_contract::nodes::Org>,
    reading: Option<&crate::routes::nodes::overview::Reading<'_>>,
) -> ApiResult<ObservedFirstSteps> {
    let reading = reading.ok_or_else(|| {
        ApiError::internal("a composite setup observation requires its declared reader")
    })?;
    read::for_overview(ctx, conn, principal, query, initial, reading).await
}

async fn set(ctx: Ctx, (): (), choice: FirstStepsChoice) -> ApiResult<PersonalState> {
    if choice.work_mode.is_some() && choice.org_id.is_none() {
        return Err(invalid("org_id", "choose an org before its work mode"));
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard.principal();
    if let Some(org) = choice.org_id {
        readable_org(&ctx, &mut tx, principal, org).await?;
    }
    let value = serde_json::to_value(&choice).map_err(ApiError::internal)?;
    if rows::save_choice(
        &mut tx,
        principal.user(),
        &value,
        choice.org_id,
        guard.now().get(),
    )
    .await?
    .iter()
    .any(|size| *size > PERSONAL_STATE_MAX_BYTES)
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::RequestTooLarge,
            "the interface state is too large",
        ));
    }
    // Only changed fields travel: unrelated portable state can exceed pg_notify's payload bound.
    orch_db::sequences::notify_user_event(
        &mut tx,
        &[principal.user()],
        &serde_json::json!({"what":"state","key":"help","child":"first_steps","value":value}),
    )
    .await?;
    if let Some(org) = choice.org_id {
        orch_db::sequences::notify_user_event(
            &mut tx,
            &[principal.user()],
            &serde_json::json!({"what":"state","key":"sidebar","child":"last_org_id","value":org}),
        )
        .await?;
    }
    let saved = state(&mut tx, principal.user()).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(saved)
}
