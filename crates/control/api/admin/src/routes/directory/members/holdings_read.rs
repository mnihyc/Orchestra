//! Membership preview facts and count/ID summaries on the shared native read adapter.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use crate::routes::nodes::read_sources::Sources;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyEmptyFacet;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::ownership::HoldingsObjectId;

use orch_api_contract::ownership::HoldingsRegistrationSummary;

use orch_api_contract::ownership::HoldingsResourceSummary;

use orch_api_contract::ownership::HoldingsResponse;

use orch_api_contract::ownership::HoldingsSummary;

use orch_api_contract::ownership_sources as sources;

use orch_api_contract::ownership_sources::Source;

use orch_authz::Caller;

use orch_authz::Principal;

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use std::collections::HashMap;
pub(super) struct Members;

pub(super) struct Selected {
    found: Found,
    holdings: PreparedHoldings,
    metadata: bool,
}

type Object = (ObjectKind, Id);

fn object(row: &orch_db::queries::resources::AccessFacts) -> ApiResult<Object> {
    Ok((row.kind.parse().map_err(ApiError::internal)?, row.id))
}

fn grouped(
    objects: &BTreeSet<Object>,
    targets: &HashMap<ObjectRef, KeyResolvedTarget>,
) -> ApiResult<Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>> {
    let mut groups = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
    for &(kind, id) in objects {
        let object = ObjectRef { kind, id };
        groups.entry(KeyTargetKind::Object(kind)).or_default().push(
            targets
                .get(&object)
                .cloned()
                .ok_or_else(ApiError::not_found)?,
        );
    }
    Ok(groups.into_iter().collect())
}

async fn require_member(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    member: &Found,
) -> ApiResult<()> {
    let result = may_end(conn, principal, member).await;
    let _ = ctx;
    result.map_err(|error| {
        if native::modern(principal.caller()) {
            authorization::problem_without_content(error)
        } else {
            error
        }
    })
}

impl native::Selected for Members {
    type Source = Source;
    type Query = ();
    type Value = Selected;
    type Reply = HoldingsResponse;
    const DECLARATION: &'static Declaration<Source> = &sources::MEMBER_HOLDINGS;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let found = found(ctx, conn).await?;
            let modern = native::modern(caller);
            let domain = if modern {
                let target = targets::node(conn, found.member.domain_id).await?;
                if target.object.kind != KeyTargetKind::Object(ObjectKind::Org) {
                    return Err(ApiError::internal("a membership domain is not an org"));
                }
                Some(native::Facts::one(
                    Source::MemberDomain,
                    target.object.kind,
                    target,
                ))
            } else {
                None
            };
            let plan = match &domain {
                Some(domain) => {
                    native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(domain))?
                }
                None => OperationPlan::new(ctx.route, Self::DECLARATION.operation)?,
            };
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            require_member(ctx, conn, &principal, &found).await?;
            let mut current = ctx.clone();
            current.now = principal.now().get();
            let holdings = prepare_holdings(&current, conn, found.org.org, found.user).await?;
            if !modern {
                return Ok((
                    Selected {
                        found,
                        holdings,
                        metadata: true,
                    },
                    Vec::new(),
                ));
            }
            let mut resources = BTreeSet::new();
            let mut machines = BTreeSet::new();
            for row in holdings.affected.resources() {
                resources.insert(object(&row.source)?);
            }
            for row in holdings.affected.registrations() {
                resources.insert(object(&row.source)?);
                let machine = row
                    .source
                    .machine_id
                    .ok_or_else(|| ApiError::internal("a native registration has no machine"))?;
                machines.insert((ObjectKind::Machine, machine));
            }
            let all = Sources::from_objects(
                resources
                    .iter()
                    .chain(&machines)
                    .map(|&(kind, id)| ObjectRef { kind, id }),
            )
            .identity_targets(conn)
            .await?;
            let mut metadata = true;
            for (source, objects) in [
                (Source::HoldingsResources, &resources),
                (Source::HoldingsMachines, &machines),
            ] {
                for &(kind, id) in objects {
                    let object = ObjectRef { kind, id };
                    metadata &= native::permits_usage(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &source,
                        FacetUsage::ResponseContent,
                        KeyTargetKind::Object(kind),
                        all.get(&object).ok_or_else(ApiError::not_found)?,
                    )?;
                }
            }
            if resources.is_empty() && machines.is_empty() {
                // An actual empty metadata selection still distinguishes explicit own-operation Read
                // from a Manage-only aggregate request. No object or source is manufactured.
                let kinds = sources::MEMBER_HOLDINGS_FACETS
                    .iter()
                    .filter(|facet| facet.usage == FacetUsage::ResponseContent)
                    .flat_map(|facet| facet.resources.iter().copied())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                let action = sources::MEMBER_HOLDINGS_FACETS
                    .iter()
                    .find(|facet| facet.usage == FacetUsage::ResponseContent)
                    .ok_or_else(|| {
                        ApiError::internal("the holdings metadata declaration is missing")
                    })?
                    .action;
                metadata = caller.permits_empty_operation(
                    Self::DECLARATION.operation,
                    &[KeyEmptyFacet {
                        action,
                        resources: kinds,
                    }],
                );
            }
            let mut facts =
                vec![domain.ok_or_else(|| ApiError::internal("a membership source is missing"))?];
            // The aggregate formatter emits only the inspection's intrinsic counts and IDs.
            // Full child metadata is materialized only when these actual native sources admit it.
            facts.push(native::Facts::collection(
                Source::HoldingsResources,
                if metadata {
                    grouped(&resources, &all)?
                } else {
                    Vec::new()
                },
            ));
            facts.push(native::Facts::collection(
                Source::HoldingsMachines,
                if metadata {
                    grouped(&machines, &all)?
                } else {
                    Vec::new()
                },
            ));
            Ok((
                Selected {
                    found,
                    holdings,
                    metadata,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let member = found(ctx, conn).await?;
            require_member(ctx, conn, principal, &member).await
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        selected: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<HoldingsResponse>> {
        Box::pin(async move {
            if selected.metadata {
                return Ok(HoldingsResponse::Full(
                    render_holdings(ctx, conn, &selected.holdings).await?,
                ));
            }
            let prepared = &selected.holdings;
            let shared = prepared.shared_by_kind.orgs
                + prepared.shared_by_kind.projects
                + prepared.shared_by_kind.topics
                + prepared.shared_by_kind.threads
                + prepared.shared_by_kind.documents
                + prepared.shared_by_kind.resources;
            Ok(HoldingsResponse::Receipt(HoldingsSummary {
                projection: orch_api_contract::common::ReceiptProjection::Receipt,
                org_id: selected.found.org.org,
                domain_id: selected.found.member.domain_id,
                user_id: selected.found.user,
                nodes: prepared.nodes.clone(),
                shared,
                shared_by_kind: prepared.shared_by_kind.clone(),
                resources: prepared
                    .affected
                    .resources()
                    .iter()
                    .map(|row| HoldingsResourceSummary {
                        object: HoldingsObjectId {
                            kind: row.source.kind.clone(),
                            id: row.source.id,
                        },
                        shared_with: row.shared_with,
                        threads: row.threads,
                    })
                    .collect(),
                machine_registrations: prepared
                    .affected
                    .registrations()
                    .iter()
                    .map(|row| {
                        Ok(HoldingsRegistrationSummary {
                            object: HoldingsObjectId {
                                kind: row.source.kind.clone(),
                                id: row.source.id,
                            },
                            machine_id: row.source.machine_id.ok_or_else(|| {
                                ApiError::internal("a native registration has no machine")
                            })?,
                            threads: row.threads,
                        })
                    })
                    .collect::<ApiResult<Vec<_>>>()?,
                entries: prepared.entries,
                pending: prepared.affected.pending,
            }))
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a (),
        opened: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            if opened.found.user != fresh.found.user
                || opened.found.org.org != fresh.found.org.org
                || opened.found.member.domain_id != fresh.found.member.domain_id
                || opened.metadata && !fresh.metadata
            {
                return Ok(false);
            }
            let associations = |value: &Selected| {
                value
                    .holdings
                    .affected
                    .resources()
                    .iter()
                    .map(|row| (false, &row.source))
                    .chain(
                        value
                            .holdings
                            .affected
                            .registrations()
                            .iter()
                            .map(|row| (true, &row.source)),
                    )
                    .map(|(registration, row)| {
                        (
                            registration,
                            row.kind.clone(),
                            row.id,
                            row.home_id,
                            row.owner_id,
                            row.machine_id,
                        )
                    })
                    .collect::<BTreeSet<_>>()
            };
            Ok(associations(opened).is_subset(&associations(fresh)))
        })
    }
}
