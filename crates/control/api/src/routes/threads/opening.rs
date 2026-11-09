//! One screen read under its own operation, with native components on the same snapshot.

use crate::key_operations::{native, targets};
use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
};
use crate::registry::{Answer, Ctx, Registry};
use crate::routes::access::page::NameBounds;
use crate::routes::settings::readers::{self, PreparedSettings};
use crate::routes::usage::caps::{self, PreparedReached};
use futures::future::BoxFuture;
use http::header::CACHE_CONTROL;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::FacetUsage;
use orch_api_contract::thread_sources::{self as sources, Source};
use orch_api_contract::threads::{
    ThreadLimits, ThreadOpening, ThreadOpeningGet, ThreadOpeningGetQuery, ThreadOutlineGetQuery,
    ThreadSyncGetQuery,
};
use orch_authz::{Act, Caller, Policy, Principal, caps::Home};
use orch_db::{
    Conn,
    queries::{thread_continuations, threads},
};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        ThreadOpeningGet,
        native::selected::<Opening>,
        opening,
    );
}

fn available<T>(result: ApiResult<T>) -> ApiResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.problem.status == 403 => Ok(None),
        Err(error) => Err(error),
    }
}

async fn require_view(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
    authorization::require_projected(
        conn,
        principal,
        ctx.id("id")?,
        authorization::exact(Permission::ThreadView),
        Act::Use,
        Policy::default(),
        native::modern(principal.caller()),
    )
    .await
}

fn collection(source: Source, targets: Vec<KeyResolvedTarget>) -> native::Facts<Source> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    let mut seen = BTreeSet::new();
    for target in targets {
        if seen.insert((target.object.kind, target.object.id)) {
            groups.entry(target.object.kind).or_default().push(target);
        }
    }
    native::Facts::collection(source, groups.into_iter().collect())
}

fn permitted(
    ctx: &Ctx,
    plan: &crate::registry::OperationPlan,
    caller: &Caller,
    source: Source,
    targets: &[KeyResolvedTarget],
) -> ApiResult<bool> {
    if !native::modern(caller) {
        return Ok(true);
    }
    for target in targets {
        if !native::permits_usage(
            plan,
            &sources::OPENING,
            caller,
            &source,
            FacetUsage::ResponseContent,
            target.object.kind,
            target,
        )? {
            return Ok(false);
        }
    }
    // The route remains the enclosing native opening at every source selection.
    if ctx.route.name != sources::OPENING.operation {
        return Err(ApiError::internal(
            "the screen sources have another native operation",
        ));
    }
    Ok(true)
}

/// Immutable branch, pane and source identities; mutable usage counters are not an authority witness.
#[derive(PartialEq, Eq)]
struct Witness {
    viewed: Id,
    active: Id,
    execution_revision: i64,
    projection_revision: i64,
    viewed_segment: Option<Id>,
    current_segment: Option<Id>,
    draft: bool,
    parameters: bool,
    provider: bool,
    controls: Vec<(KeyTargetKind, Option<Id>)>,
    names: BTreeSet<Id>,
    caps: Option<Vec<Id>>,
    schedules: Option<Vec<(Id, i64)>>,
}

struct Prepared {
    query: ThreadOpeningGetQuery,
    settings: PreparedSettings,
    reached: Option<PreparedReached>,
    provider: Option<crate::routes::resources::providers::PreparedProjection>,
    bounds: NameBounds,
    offer_modes: bool,
    pending: Option<crate::input_schedules::PreparedThreadPending>,
    witness: Witness,
}

struct Opening;

impl native::Selected for Opening {
    type Source = Source;
    type Query = ThreadOpeningGetQuery;
    type Value = Prepared;
    type Reply = ThreadOpening;
    const DECLARATION: &'static Declaration<Source> = &sources::OPENING;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let target = targets::node(conn, id).await?;
            if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
                return Err(ApiError::not_found());
            }
            let mut facts = vec![native::Facts::one(
                Source::Thread,
                target.object.kind,
                target,
            )];
            let base = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &base).await?;
            require_view(ctx, conn, &principal).await?;
            let row = threads::thread(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let projection = thread_continuations::projection(conn, id, query.continuation_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let settings = readers::prepare_settings(conn, &principal, id).await?;
            let draft = available(
                authorization::require_projected(
                    conn,
                    &principal,
                    id,
                    authorization::exact(Permission::ThreadInteract),
                    Act::Use,
                    Policy::default(),
                    native::modern(caller),
                )
                .await,
            )?
            .is_some();

            // These are actual current native binding rows, before descriptor/model projection.
            let mut binding = Vec::new();
            let mut binding_available = true;
            for (kind, resource) in [
                (ObjectKind::Agent, row.agent_id),
                (ObjectKind::Provider, row.provider_id),
                (ObjectKind::Environment, row.environment_id),
            ] {
                if let Some(resource) = resource {
                    match targets::page_object(conn, ObjectRef { kind, id: resource }).await {
                        Ok(target) => binding.push(target),
                        Err(error) if matches!(error.problem.status, 404 | 410) => {
                            binding_available = false;
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            let offer_modes = (!native::modern(caller) || binding_available)
                && permitted(ctx, &base, caller, Source::OpeningControls, &binding)?;
            let parameters = offer_modes;
            let mut controls = if offer_modes { binding } else { Vec::new() };
            let mut provider = None;
            if parameters
                && native::modern(caller)
                && let Some(id) = row.provider_id
            {
                let mut prepared = available(
                    crate::routes::resources::providers::prepare_readable_projection(
                        ctx, conn, &principal, id,
                    )
                    .await,
                )?;
                if let Some(value) = &prepared {
                    let groups = crate::routes::access::operations::group_targets(
                        conn,
                        &value.name_sources().groups.into_iter().collect::<Vec<_>>(),
                    )
                    .await?;
                    if permitted(ctx, &base, caller, Source::OpeningControls, &groups)? {
                        controls.extend(groups);
                    } else {
                        prepared = None;
                    }
                }
                provider = prepared;
            }

            let mut reached =
                Some(caps::prepare_reached_for_thread(ctx, conn, &principal, id, None).await?);
            let prepared_caps = reached.as_ref().ok_or_else(ApiError::not_found)?;
            let mut cap_sources = Vec::new();
            for home in prepared_caps.homes() {
                cap_sources.push(match home {
                    Home::Node(node) => targets::node(conn, node).await?,
                    Home::Provider { provider, .. } => {
                        targets::page_object(
                            conn,
                            ObjectRef {
                                kind: ObjectKind::Provider,
                                id: provider,
                            },
                        )
                        .await?
                    }
                    Home::System => targets::system(),
                });
            }
            cap_sources.extend(
                crate::routes::access::operations::group_targets(
                    conn,
                    &prepared_caps
                        .names
                        .groups
                        .iter()
                        .copied()
                        .collect::<Vec<_>>(),
                )
                .await?,
            );
            if permitted(ctx, &base, caller, Source::OpeningControls, &cap_sources)? {
                controls.extend(cap_sources);
            } else {
                // A limits pane is complete or unavailable; a key does not silently remove a cap.
                reached = None;
            }

            let mut name_ids: BTreeSet<_> = settings.source_nodes().into_iter().collect();
            if let Some(provider) = &provider {
                name_ids.extend(provider.name_sources().nodes);
            }
            if let Some(reached) = &reached {
                name_ids.extend(&reached.names.nodes);
                for target in &controls {
                    name_ids.extend(target.node_ancestors.iter().filter_map(|node| node.id));
                }
            }
            let graph = orch_authz::engine::Graph::load(
                conn,
                &name_ids.iter().copied().collect::<Vec<_>>(),
            )
            .await
            .map_err(ApiError::internal)?;
            let paths: BTreeSet<_> = name_ids
                .iter()
                .flat_map(|id| {
                    graph
                        .get(*id)
                        .into_iter()
                        .flat_map(|node| node.path.iter().copied())
                })
                .collect();
            let mut bounds = NameBounds {
                limited: native::modern(caller),
                ..Default::default()
            };
            let mut names = Vec::new();
            for node in paths {
                let Some(target) =
                    orch_engine::operation_scope::targets::from_node_identity(&graph, node)
                else {
                    continue;
                };
                if !matches!(
                    target.object.kind,
                    KeyTargetKind::Object(
                        ObjectKind::Org
                            | ObjectKind::Project
                            | ObjectKind::Topic
                            | ObjectKind::Thread
                            | ObjectKind::Document
                    )
                ) {
                    continue;
                }
                if permitted(
                    ctx,
                    &base,
                    caller,
                    Source::OpeningNames,
                    std::slice::from_ref(&target),
                )? {
                    bounds.nodes.insert(node);
                    names.push(target);
                }
            }
            let mut control_ids = controls
                .iter()
                .map(|target| (target.object.kind, target.object.id))
                .collect::<Vec<_>>();
            control_ids.sort_unstable();
            control_ids.dedup();
            let mut cap_ids = reached.as_ref().map(PreparedReached::ids);
            if let Some(ids) = &mut cap_ids {
                ids.sort_unstable();
            }
            let mut pending = if projection.follows_active {
                available(
                    crate::input_schedules::prepare_thread_pending(
                        conn,
                        &principal,
                        row.topic_id,
                        id,
                    )
                    .await,
                )?
            } else {
                None
            };
            if let Some(selected) = &pending
                && !permitted(
                    ctx,
                    &base,
                    caller,
                    Source::OpeningSchedules,
                    &selected.sources,
                )?
            {
                pending = None;
            }
            let witness = Witness {
                viewed: projection.viewed_continuation_id,
                active: projection.active_continuation_id,
                execution_revision: projection.execution_revision,
                projection_revision: projection.projection_revision,
                viewed_segment: projection.viewed_segment_id,
                current_segment: row.current_segment_id,
                draft,
                parameters,
                provider: provider.is_some(),
                controls: control_ids,
                names: bounds.nodes.clone(),
                caps: cap_ids,
                schedules: pending.as_ref().map(|selected| selected.witness.clone()),
            };
            if let Some(provider) = &mut provider {
                provider.bound_names(bounds.clone());
            }
            facts.push(collection(Source::OpeningControls, controls));
            facts.push(collection(Source::OpeningNames, names));
            facts.push(collection(
                Source::OpeningSchedules,
                pending
                    .as_ref()
                    .map_or_else(Vec::new, |selected| selected.sources.clone()),
            ));
            Ok((
                Prepared {
                    query: query.clone(),
                    settings,
                    reached,
                    provider,
                    bounds,
                    offer_modes,
                    pending,
                    witness,
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
        Box::pin(require_view(ctx, conn, principal))
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<ThreadOpening>> {
        Box::pin(async move {
            let query = &value.query;
            let sync = super::read::sync_in(
                ctx,
                conn,
                ThreadSyncGetQuery {
                    limit: query.limit,
                    continuation_id: query.continuation_id,
                    projection_revision: query.projection_revision,
                    ..Default::default()
                },
            )
            .await?;
            let outline = super::read::outline_in(
                ctx,
                conn,
                ThreadOutlineGetQuery {
                    limit: query.limit,
                    continuation_id: query.continuation_id,
                    projection_revision: query.projection_revision,
                    ..Default::default()
                },
            )
            .await?;
            let draft = if value.witness.draft {
                super::drive::draft_in(ctx, conn).await?
            } else {
                None
            };
            let parameters = if value.witness.parameters {
                available(
                    super::parameters::parameters_in(
                        ctx,
                        conn,
                        principal,
                        &value.bounds,
                        if native::modern(principal.caller()) {
                            super::parameters::ProviderView::Selected(value.provider.as_ref())
                        } else {
                            super::parameters::ProviderView::Native
                        },
                    )
                    .await,
                )?
            } else {
                None
            };
            let limits = match &value.reached {
                Some(reached) => {
                    let reached = caps::show_reached_for_thread(
                        ctx,
                        conn,
                        principal,
                        reached,
                        None,
                        Some(&value.bounds),
                    )
                    .await?;
                    Some(ThreadLimits {
                        bypass_all: reached.iter().any(|limit| limit.passed_by.is_none())
                            && reached
                                .iter()
                                .filter(|limit| limit.passed_by.is_none())
                                .all(|limit| limit.can_bypass),
                        reached,
                    })
                }
                None => None,
            };
            let settings = readers::show_settings_with_controls(
                ctx,
                conn,
                principal,
                &value.settings,
                &value.bounds,
                value.offer_modes,
            )
            .await?;
            let follows_active = sync
                .projection
                .as_ref()
                .is_none_or(|projection| projection.follows_active);
            let gateway = if follows_active {
                Some(
                    match crate::routes::gateways::thread_in(ctx, conn, ctx.id("id")?).await {
                        Ok(data) => orch_api_contract::nodes::OverviewPart::Ready { data },
                        Err(error) => {
                            let error = if native::modern(principal.caller()) {
                                crate::middleware::authorization::problem_without_content(error)
                            } else {
                                error
                            };
                            let (problem, retry_after) = error.into_parts_for(&ctx.request_id);
                            orch_api_contract::nodes::OverviewPart::Failed {
                                problem,
                                retry_after,
                            }
                        }
                    },
                )
            } else {
                None
            };
            let pending_agent_schedules = if follows_active {
                Some(match &value.pending {
                    Some(selected) => orch_api_contract::nodes::OverviewPart::Ready {
                        data: selected.value.clone(),
                    },
                    None => {
                        let (problem, retry_after) = ApiError::new(
                            ErrorCode::AuthzDenied,
                            "the pending schedule sources are not readable",
                        )
                        .into_parts_for(&ctx.request_id);
                        orch_api_contract::nodes::OverviewPart::Failed {
                            problem,
                            retry_after,
                        }
                    }
                })
            } else {
                None
            };
            Ok(ThreadOpening {
                sync,
                draft,
                parameters,
                limits,
                settings,
                outline,
                gateway,
                pending_agent_schedules,
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Prepared,
        fresh: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.witness == fresh.witness) })
    }
}

async fn opening(
    ctx: Ctx,
    query: ThreadOpeningGetQuery,
    (): (),
) -> ApiResult<Answer<ThreadOpening>> {
    Ok(
        Answer::from(native::selected_read::<Opening>(ctx, query, ()).await?)
            .header(CACHE_CONTROL, crate::sign_in::NOT_KEPT),
    )
}
