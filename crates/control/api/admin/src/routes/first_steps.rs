//! Current readable setup observations and atomic portable first-steps choices.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::authorization;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::invalid;

use crate::routes::nodes::view;

use orch_access_contract::pages::PageKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::roles::PlaceRef;

use orch_access_contract::roles::Role;

use orch_api_contract::first_steps::*;

use orch_api_contract::nodes::Node;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::personal::PERSONAL_STATE_MAX_BYTES;

use orch_api_contract::personal::PersonalState;

use orch_authz::engine::Frame;

use orch_authz::engine::PageRef;

use orch_authz::giving::Acting;

use orch_authz::giving::Giving;

use orch_authz::things;

use orch_authz::things::Thing;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::first_steps as rows;

use orch_db::queries::identity;

use orch_types::Id;

pub use orch_api_base::routes::first_steps::ObservedFirstSteps;

pub use orch_api_base::routes::first_steps::read_in;
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
orch_api_base::__api_bind_f635db2fdc!();

async fn state(conn: &mut Conn, user: Id) -> ApiResult<PersonalState> {
    Ok(identity::interface_state(conn, user)
        .await?
        .into_iter()
        .collect())
}
orch_api_base::__api_bind_712bb19250!();

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
orch_api_base::__api_bind_3b673ad843!();

fn action(url: String, allowed: bool) -> Option<FirstStepsAction> {
    Some(FirstStepsAction {
        url,
        allowed,
        reason: None,
    })
}
orch_api_base::__api_bind_ed8b0e6ce1!();

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
orch_api_base::__api_bind_8bf6e24f02!();

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
orch_api_base::__api_bind_c22f2bb4e0!();

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
orch_api_base::__api_bind_293ce96b17!();

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

#[path = "first_steps/read.rs"]
pub mod read;
