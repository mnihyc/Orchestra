//! An effect-free move preview retains its native body, sources and ordinary one-side gate.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::ownership::MemberMovePreviewResponse;

use orch_api_contract::ownership::MemberMovePreviewSummary;

use orch_api_contract::ownership_sources as sources;

use orch_api_contract::ownership_sources::Source;

use orch_authz::Caller;

use orch_authz::Principal;

use std::collections::BTreeMap;

use std::collections::BTreeSet;
pub(super) struct Preview;

pub(super) struct Selected {
    found: Found,
    to: Placed,
    wanted: Role,
    role: Role,
    loses: i64,
    gains: i64,
    owned: HoldingsNodes,
    named: i64,
    roles: Vec<access::RoleRow>,
    holds: [bool; 2],
    metadata: bool,
    places: Vec<KeyResolvedTarget>,
    visible: BTreeSet<Id>,
    displays: BTreeSet<Id>,
}

const ORG: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Org);

const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);

async fn holds(conn: &mut Conn, principal: &Principal, from: Id, to: Id) -> ApiResult<[bool; 2]> {
    let mut holds = [false; 2];
    for (index, id) in [from, to].into_iter().enumerate() {
        holds[index] = match nodes::check(
            conn,
            principal,
            id,
            Permission::MembersManage,
            Act::Use,
            Policy::default(),
        )
        .await
        {
            Ok(decision) => decision.is_allowed(),
            Err(error) => {
                let error = refusals::problem(conn, error).await;
                return Err(if native::modern(principal.caller()) {
                    authorization::problem_without_content(error)
                } else {
                    error
                });
            }
        };
    }
    if !holds.iter().any(|held| *held) {
        authorization::require_projected(
            conn,
            principal,
            from,
            exact(Permission::MembersManage),
            Act::Use,
            Policy::default(),
            native::modern(principal.caller()),
        )
        .await?;
    }
    Ok(holds)
}

impl native::Selected for Preview {
    type Source = Source;
    type Query = ((), MemberMoveBody);
    type Value = Selected;
    type Reply = MemberMovePreviewResponse;
    const DECLARATION: &'static Declaration<Source> = &sources::MEMBER_MOVE_PREVIEW;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (_, body): &'a ((), MemberMoveBody),
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (found, to, wanted) = move_inputs(ctx, conn, body).await?;
            let from = targets::node(conn, found.member.domain_id).await?;
            let destination = targets::node(conn, to.id).await?;
            let account = targets::account(conn, found.user).await?;
            let mut facts = vec![
                native::Facts::one(Source::MemberDomain, ORG, from),
                native::Facts::one(Source::MemberDestination, ORG, destination),
            ];
            let known = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let holds = holds(conn, &principal, found.member.domain_id, to.id).await?;
            let existing: Option<Role> = access::role_at(conn, to.id, found.user)
                .await?
                .and_then(|word| serde_json::from_value(json!(word)).ok());
            let role = existing.map_or(wanted, |held| {
                if rank(held) > rank(wanted) {
                    held
                } else {
                    wanted
                }
            });
            let root = found.org.org;
            let user = found.user;
            let domain = found.member.domain_id;
            let loses =
                access::reached_only_through_domain(conn, root, user, domain, domain, to.id)
                    .await?;
            let gains =
                access::reached_only_through_domain(conn, root, user, domain, to.id, domain)
                    .await?;
            let owned = access::owned_counts(conn, root, user, Some(domain), false).await?;
            let documents =
                access::owned_document_count(conn, root, user, Some(domain), false).await?;
            let (_, named) = access::named_counts(conn, root, user).await?;
            let roles = access::roles_elsewhere(conn, root, user, domain).await?;
            let mut ids = BTreeSet::from([domain, to.id]);
            ids.extend(roles.iter().map(|row| row.node_id));
            let graph =
                orch_authz::engine::Graph::load(conn, &ids.iter().copied().collect::<Vec<_>>())
                    .await
                    .map_err(ApiError::internal)?;
            for id in ids.clone() {
                let source = orch_engine::operation_scope::targets::from_node_identity(&graph, id)
                    .ok_or_else(ApiError::not_found)?;
                ids.extend(
                    source
                        .node_ancestors
                        .iter()
                        .filter_map(|ancestor| ancestor.id),
                );
            }
            let ids = ids.into_iter().collect::<Vec<_>>();
            let visible =
                view::reference_visibility(conn, &principal, &ids, Policy::default()).await?;
            let displays =
                view::reference_thread_visibility(conn, &principal, &ids, Policy::default())
                    .await?;
            let mut places = Vec::new();
            let mut grouped = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
            let modern = native::modern(caller);
            let mut metadata = !modern
                || native::permits_usage(
                    &known,
                    Self::DECLARATION,
                    caller,
                    &Source::PreviewAccount,
                    FacetUsage::ResponseContent,
                    ACCOUNT,
                    &account,
                )?;
            for id in ids {
                let target = orch_engine::operation_scope::targets::from_node_identity(&graph, id)
                    .ok_or_else(ApiError::not_found)?;
                metadata &= !modern
                    || native::permits_usage(
                        &known,
                        Self::DECLARATION,
                        caller,
                        &Source::PreviewPlaces,
                        FacetUsage::ResponseContent,
                        target.object.kind,
                        &target,
                    )?;
                grouped
                    .entry(target.object.kind)
                    .or_default()
                    .push(target.clone());
                places.push(target);
            }
            facts.push(native::Facts::collection(
                Source::PreviewAccount,
                if metadata {
                    vec![(ACCOUNT, vec![account])]
                } else {
                    Vec::new()
                },
            ));
            facts.push(native::Facts::collection(
                Source::PreviewPlaces,
                if metadata {
                    grouped.into_iter().collect()
                } else {
                    Vec::new()
                },
            ));
            Ok((
                Selected {
                    found,
                    to,
                    wanted,
                    role,
                    loses,
                    gains,
                    owned: nodes_of(&owned, documents),
                    named,
                    roles,
                    holds,
                    metadata,
                    places,
                    visible,
                    displays,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        // Both source and destination are native body inputs. Selection gates those exact inputs
        // before counts; render repeats that same ordinary gate on the guarded principal.
        Box::pin(async { Ok(()) })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        selected: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<MemberMovePreviewResponse>> {
        Box::pin(async move {
            let domain = selected.found.member.domain_id;
            holds(conn, principal, domain, selected.to.id).await?;
            let reach = |nodes| MemberMoveReach {
                nodes,
                resources: 0,
                documents: 0,
            };
            if !selected.metadata {
                return Ok(MemberMovePreviewResponse::Receipt(Box::new(
                    MemberMovePreviewSummary {
                        projection: orch_api_contract::common::ReceiptProjection::Receipt,
                        org_id: selected.found.org.org,
                        user_id: selected.found.user,
                        from: domain,
                        to: selected.to.id,
                        loses: reach(selected.loses),
                        gains: reach(selected.gains),
                        stays: selected.owned.clone(),
                        resources: 0,
                        role_ids: selected.roles.iter().map(|row| row.id).collect(),
                        named: selected.named,
                        missing: [domain, selected.to.id]
                            .into_iter()
                            .zip(selected.holds)
                            .filter_map(|(id, held)| (!held).then_some(id))
                            .collect(),
                    },
                )));
            }
            let mut missing = Vec::new();
            for id in [domain, selected.to.id] {
                missing.extend(lacking(conn, principal, id).await?.1);
            }
            let roles =
                roles_held(conn, principal, &selected.roles, selected.found.org.org).await?;
            let mut people = refusals::user_refs(conn, &[selected.found.user]).await?;
            let refs = view::refs(
                conn,
                principal,
                &[domain, selected.to.id],
                Policy::default(),
            )
            .await?;
            let node = |id| {
                refs.get(&id)
                    .cloned()
                    .ok_or_else(|| ApiError::internal("a place of a move is not there"))
            };
            Ok(MemberMovePreviewResponse::Full(Box::new(MemberMovePlan {
                user: people
                    .remove(&selected.found.user)
                    .ok_or_else(|| ApiError::internal("a member is not there"))?,
                from: node(domain)?,
                to: node(selected.to.id)?,
                role: selected.role,
                loses: reach(selected.loses),
                gains: reach(selected.gains),
                stays: MemberMoveStays {
                    nodes: selected.owned.clone(),
                    resources: 0,
                    roles,
                    named: selected.named,
                },
                waiting: Vec::new(),
                missing,
            })))
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _body: &'a ((), MemberMoveBody),
        opened: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.found.user == fresh.found.user
                && opened.found.org.org == fresh.found.org.org
                && opened.found.member.domain_id == fresh.found.member.domain_id
                && opened.to.id == fresh.to.id
                && opened.wanted == fresh.wanted
                && opened.role == fresh.role
                && opened.roles == fresh.roles
                && opened.holds == fresh.holds
                && opened.places == fresh.places
                && (!opened.metadata
                    || (fresh.metadata
                        && opened.visible.is_subset(&fresh.visible)
                        && opened.displays.is_subset(&fresh.displays))))
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    body: MemberMoveBody,
) -> ApiResult<MemberMovePreviewResponse> {
    native::selected_body_read::<Preview, (), MemberMoveBody>(ctx, (), body).await
}
