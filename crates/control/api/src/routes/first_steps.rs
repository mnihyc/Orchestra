//! Current readable setup observations and atomic portable first-steps choices.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{invalid, view};
use orch_access_contract::pages::PageKind;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::{PlaceRef, Role};
use orch_api_contract::first_steps::*;
use orch_api_contract::nodes::{Node, NodeKind, NodeRef};
use orch_api_contract::notifications::OutsideWayState;
use orch_api_contract::personal::{PERSONAL_STATE_MAX_BYTES, PersonalState};
use orch_authz::engine::{Frame, PageRef};
use orch_authz::giving::{Acting, Giving};
use orch_authz::things::{self, Thing};
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::resources::routes as resources;
use orch_db::queries::{first_steps as rows, hierarchy, identity};
use orch_types::{Id, Json, Time};
use std::collections::HashSet;

/// Registers both declared first-steps requests.
pub fn mount(registry: &mut Registry) {
    registry.on(MeFirstStepsGet, get).on(MeFirstStepsSet, set);
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
            start_topics: held.contains(&Permission::ProjectInteract),
            start_threads: held.contains(&Permission::TopicInteract),
            send_inputs: held.contains(&Permission::ThreadInteract),
            answer_requests: held.contains(&Permission::ThreadManage),
            model_access_required: true,
        }
    })
    .collect())
}

async fn start_here(
    conn: &mut Conn,
    principal: &Principal,
    org: Id,
    root: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Option<orch_api_contract::documents::DocumentRef>> {
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
    Ok(crate::routes::documents::build::refs(
        conn,
        principal,
        &[(row, held)],
        now,
        Policy::default(),
    )
    .await?
    .pop())
}

async fn get(ctx: Ctx, query: FirstStepsGetQuery, (): ()) -> ApiResult<FirstSteps> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let saved = state(&mut conn, principal.user()).await?;
    let choice: FirstStepsChoice = saved
        .get("help")
        .and_then(|help| help.get("first_steps"))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let personal = identity::user_by_id(&mut conn, principal.user())
        .await?
        .and_then(|user| user.personal_org_id);
    let last = saved
        .get("sidebar")
        .and_then(|sidebar| sidebar.get("last_org_id"))
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok());
    let wanted = query.org_id.or(choice.org_id).or(last);
    let mut org = None;
    if let Some(id) = wanted {
        match readable_org(&ctx, &mut conn, &principal, id).await {
            Ok(found) => org = Some(found),
            Err(error)
                if query.org_id.is_none()
                    && matches!(
                        error.code(),
                        orch_api_contract::common::ErrorCode::AuthzDenied
                            | orch_api_contract::common::ErrorCode::NotFound
                            | orch_api_contract::common::ErrorCode::OrgSuspended
                    ) => {}
            Err(error) => return Err(error),
        }
    }
    if org.is_none() && query.org_id.is_none() {
        let roots = hierarchy::root_orgs_of(&mut conn, principal.user()).await?;
        let mut teams: Vec<_> =
            view::visible(&mut conn, &principal, &roots, ctx.now, Policy::default())
                .await?
                .into_iter()
                .filter_map(|node| match node {
                    Node::Org(org) if org.org_kind == orch_api_contract::nodes::OrgType::Team => {
                        Some(org)
                    }
                    _ => None,
                })
                .collect();
        if teams.len() == 1 {
            org = teams.pop();
        } else if let Some(id) = personal {
            org = Some(readable_org(&ctx, &mut conn, &principal, id).await?);
        }
    }
    let Some(org) = org else {
        return Ok(FirstSteps {
            org: None,
            work_mode: None,
            steps: Vec::new(),
            invitation_roles: Vec::new(),
            start_here: None,
            as_of: Time::new(ctx.now),
        });
    };
    let id = org.base.id;
    let org_ref = NodeRef {
        thread_display: None,
        id,
        kind: NodeKind::Org,
        name: Some(org.base.name.clone()),
        redacted: false,
    };
    let mode = if choice.org_id == Some(id) {
        choice.work_mode
    } else {
        None
    };
    let permits = |permission: &str| {
        org.base
            .access
            .permissions
            .iter()
            .any(|held| held == permission)
    };
    let mut provider_ids = rows::providers(&mut conn, id).await?;
    let default = orch_settings::get::<Id>(
        &mut conn,
        orch_settings::Place::Node(id),
        orch_settings_contract::registry::keys::AGENT_DEFAULT_PROVIDER_ID,
    )
    .await
    .map_err(ApiError::internal)?;
    if let Some(default) = default
        && !provider_ids.contains(&default)
    {
        provider_ids.push(default);
    }
    let standing = orch_authz::resources::effective_among(
        &mut conn,
        &principal,
        &provider_ids,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let visible: HashSet<Id> = standing
        .iter()
        .filter(|row| {
            row.held
                .iter()
                .any(|held| held.permission == Permission::ProviderView)
        })
        .map(|row| row.object.id)
        .collect();
    let usable: HashSet<Id> = standing
        .iter()
        .filter(|row| {
            row.held
                .iter()
                .any(|held| held.permission == Permission::ProviderUse)
        })
        .map(|row| row.object.id)
        .collect();
    provider_ids.retain(|candidate| {
        visible.contains(candidate) && (Some(*candidate) != default || usable.contains(candidate))
    });
    let providers = resources::providers(&mut conn, &provider_ids).await?;
    let secrets = resources::provider_secret_names(&mut conn, &provider_ids).await?;
    let models = resources::models_of(&mut conn, &provider_ids).await?;
    let configured = providers.iter().find_map(|provider| {
        let yields = provider
            .facts
            .pointer("/credential/yields")
            .and_then(Json::as_str);
        let fact = if secrets
            .iter()
            .any(|(owner, name)| *owner == provider.id && name == "api_key")
            || yields == Some("key")
        {
            FirstStepsFact::KeySaved
        } else if matches!(yields, Some("login" | "token")) {
            FirstStepsFact::SignedIn
        } else if provider.upstreams.as_array().is_some_and(|upstreams| {
            upstreams.iter().any(|endpoint| {
                endpoint.pointer("/options/auth").and_then(Json::as_str) == Some("none")
            })
        }) {
            FirstStepsFact::EndpointConfigured
        } else {
            return None;
        };
        Some((provider, fact))
    });
    let with_models = providers.iter().find(|provider| {
        usable.contains(&provider.id)
            && models
                .iter()
                .any(|model| model.provider_id == provider.id && model.enabled)
    });
    let candidates = rows::answers(&mut conn, id).await?;
    let candidate_ids: Vec<_> = candidates.iter().map(|(thread, _)| *thread).collect();
    let answers = view::visible(
        &mut conn,
        &principal,
        &candidate_ids,
        ctx.now,
        Policy::default(),
    )
    .await?;
    let answer = answers.iter().find_map(|node| {
        if let Node::Thread(thread) = node {
            Some(thread.base.id)
        } else {
            None
        }
    });
    let scripted = answer.is_some_and(|answer| {
        candidates
            .iter()
            .any(|(id, scripted)| *id == answer && *scripted)
    });
    let domain_ids = rows::member_domains(&mut conn, id, principal.user()).await?;
    let joined = !view::visible(
        &mut conn,
        &principal,
        &domain_ids,
        ctx.now,
        Policy::default(),
    )
    .await?
    .is_empty();
    let waiting =
        permits("members.manage") && rows::invitation_waiting(&mut conn, id, ctx.now).await?;
    let mut steps = vec![
        step(
            FirstStepsId::Team,
            match mode {
                None => FirstStepsFact::ChooseOrg,
                Some(FirstStepsMode::Solo) => FirstStepsFact::SoloChosen,
                Some(FirstStepsMode::Team) => FirstStepsFact::TeamChosen,
            },
            Some(org_ref.clone()),
            None,
            false,
        ),
        step(
            FirstStepsId::Provider,
            configured.map_or(FirstStepsFact::NotDone, |(_, fact)| fact),
            None,
            action(
                configured.map_or_else(
                    || format!("/resources/providers?org_id={id}"),
                    |(provider, _)| format!("/resources/providers/{}", provider.id),
                ),
                configured.is_some() || permits("settings.manage"),
            ),
            false,
        ),
        step(
            FirstStepsId::Models,
            if with_models.is_some() {
                FirstStepsFact::ModelsEnabled
            } else {
                FirstStepsFact::NotDone
            },
            None,
            action(
                with_models.map_or_else(
                    || format!("/resources/providers?org_id={id}"),
                    |provider| format!("/resources/providers/{}?tab=models", provider.id),
                ),
                with_models.is_some() || permits("settings.manage"),
            ),
            false,
        ),
        step(
            FirstStepsId::Work,
            if answer.is_some() {
                FirstStepsFact::AnsweredOnce
            } else {
                FirstStepsFact::NotDone
            },
            None,
            action(
                answer.map_or_else(
                    || format!("/org/{}", org.address),
                    |thread| format!("/thread/{thread}"),
                ),
                answer.is_some() || permits("org.interact"),
            ),
            scripted,
        ),
    ];
    if answer.is_some() {
        let outside = crate::routes::notifications::inbox::outside_status(
            &mut conn,
            &principal,
            Some(id),
            ctx.now,
        )
        .await?;
        steps.push(step(
            FirstStepsId::Away,
            if outside
                .ways
                .iter()
                .any(|way| way.state == OutsideWayState::Ready)
            {
                FirstStepsFact::OutwardEnabled
            } else {
                FirstStepsFact::NotDone
            },
            None,
            action("/settings/notifications".into(), true),
            false,
        ));
    }
    if mode != Some(FirstStepsMode::Solo) {
        steps.push(step(
            FirstStepsId::Teammate,
            if joined {
                FirstStepsFact::MemberJoined
            } else if waiting {
                FirstStepsFact::InvitationWaiting
            } else if !permits("members.manage") {
                FirstStepsFact::Unknown
            } else {
                FirstStepsFact::NotDone
            },
            Some(org_ref.clone()),
            action(
                format!("/org/{}/settings/members", org.address),
                permits("members.manage"),
            ),
            false,
        ));
    }
    let invitation_roles = role_summaries(&mut conn, &principal, id).await?;
    let start_here = start_here(&mut conn, &principal, id, org.base.org_id, ctx.now).await?;
    Ok(FirstSteps {
        org: Some(org_ref),
        work_mode: mode,
        steps,
        invitation_roles,
        start_here,
        as_of: Time::new(ctx.now),
    })
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
