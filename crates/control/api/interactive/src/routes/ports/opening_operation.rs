//! Actual stored-opening governor, current returned resource sources and optional protected names.

use super::*;

use crate::key_operations::targets;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyAction;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::TargetResolver;

use orch_authz::Caller;

use orch_authz::CapturedCaller;
const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

const NAMES: NameRoles = NameRoles {
    groups: None,
    containers: None,
    nodes: [
        (ObjectKind::Org, "opening_names_orgs"),
        (ObjectKind::Project, "opening_names_projects"),
        (ObjectKind::Topic, "opening_names_topics"),
        (ObjectKind::Thread, "opening_names_threads"),
    ],
};

pub(super) struct PreparedOpening {
    pub(super) row: rows::Opening,
    pub(super) environment: EnvironmentFull,
    pub(super) audience_only: bool,
    pub(super) names: NameBounds,
}

fn slot(plan: &OperationPlan, role: &'static str, kind: KeyTargetKind) -> ApiResult<usize> {
    let mut slots = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.target == TargetResolver::ResolvedCollection(role));
    let Some((index, facet)) = slots.next() else {
        return Err(ApiError::internal("port projection source role is absent"));
    };
    if slots.next().is_some()
        || facet.action != KeyAction::Read
        || facet.usage
            != if plan.variant().operation == "PortOpeningGet" {
                FacetUsage::Required
            } else {
                FacetUsage::ResponseContent
            }
        || facet.resources != [kind]
    {
        return Err(ApiError::internal(
            "port projection source role metadata changed",
        ));
    }
    Ok(index)
}

async fn base(ctx: &Ctx, conn: &mut Conn, row: &rows::Opening) -> ApiResult<OperationPlan> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: row.environment_id,
        },
    )
    .await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    let indices = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| {
            facet.resources == [ENVIRONMENT]
                && facet.target == TargetResolver::Resolved("opening_environment")
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if indices.is_empty() {
        return Err(ApiError::internal("opening environment metadata is absent"));
    }
    for index in indices {
        plan.bind(index, ENVIRONMENT, std::slice::from_ref(&target))?;
    }
    Ok(plan)
}

fn bind(
    plan: &mut OperationPlan,
    role: &'static str,
    kind: KeyTargetKind,
    targets: Vec<KeyResolvedTarget>,
) -> ApiResult<()> {
    let index = slot(plan, role, kind)?;
    if targets.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, kind, &targets)
    }
}

/// These are identities from the very same current projection queries, not allocation hints.
/// Actual selected publisher/context facts, shared by stored and list projections.
pub(super) async fn foreign_sources(
    conn: &mut Conn,
    row: &rows::Opening,
) -> ApiResult<(Vec<KeyResolvedTarget>, Vec<KeyResolvedTarget>)> {
    let mut hosts = Vec::new();
    let mut publishers = Vec::new();
    if let Some(machine) = row.publish_machine_id {
        if !orch_db::queries::resources::routes::machine_names(conn, &[machine])
            .await?
            .is_empty()
        {
            publishers.push(targets::machine_subject(conn, machine).await?);
        }
        if row.kind == "host"
            && let Some(publisher) =
                orch_db::queries::environment_facts::publisher(conn, machine).await?
            && let Some(host) = publisher.host_id
        {
            hosts.push(
                targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: host,
                    },
                )
                .await?,
            );
        }
    }
    Ok((hosts, publishers))
}
orch_api_base::__api_bind_40d114e821!();

async fn returned_sources(
    conn: &mut Conn,
    plan: &mut OperationPlan,
    row: &rows::Opening,
) -> ApiResult<()> {
    let (hosts, publishers) = foreign_sources(conn, row).await?;
    bind(
        plan,
        "opening_hosts",
        KeyTargetKind::Object(ObjectKind::Host),
        hosts,
    )?;
    bind(
        plan,
        "opening_publishers",
        KeyTargetKind::Object(ObjectKind::Machine),
        publishers,
    )
}

pub(super) fn name_sources(row: &rows::Opening) -> NameSources {
    let mut names = NameSources::default();
    names.nodes.extend(
        [row.place_id, row.requested_by_thread]
            .into_iter()
            .flatten(),
    );
    if let Some(node) = row
        .admission
        .as_ref()
        .and_then(|source| source.get("node_id"))
        .and_then(Json::as_str)
        .and_then(|value| value.parse::<Id>().ok())
    {
        names.nodes.insert(node);
    }
    names
}
orch_api_base::__api_bind_f02d1b6741!();

/// Ordinary read eligibility for this stored opening. Audience access stays an
/// alternative to Ports View; caller scope is independently admitted by each producer.
pub(super) async fn read_door(
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::Opening,
) -> ApiResult<bool> {
    let door = core::holds(conn, principal, row.environment_id, Permission::PortsView)
        .await
        .map_err(ApiError::internal)?;
    if !door
        && !(row.kind == "place"
            && row.place_id.is_some()
            && core::place_view(
                conn,
                principal,
                row.place_id.ok_or_else(ApiError::not_found)?,
            )
            .await
            .unwrap_or(false))
    {
        return Err(ApiError::not_found());
    }
    Ok(!door)
}

pub(super) fn prepare_get<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let row = rows::opening(conn, ctx.id("opening_id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let environment = super::environment(conn, row.environment_id).await?;
        let mut plan = base(ctx, conn, &row).await?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let audience_only = read_door(conn, &principal, &row).await?;
        returned_sources(conn, &mut plan, &row).await?;
        let names = projection_names(conn, caller, &mut plan, name_sources(&row), &NAMES).await?;
        Ok(plan.with_data(PreparedOpening {
            row,
            environment,
            audience_only,
            names,
        }))
    })
}

/// Only current stored effect targets are admitted before mutation. Existing content sources
/// stay unresolved until the response checkpoint; Manage never becomes a Read prerequisite.
pub(super) fn prepare_write<'a, B>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    _body: B,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let row = rows::opening(conn, ctx.id("opening_id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        base(ctx, conn, &row).await
    })
}

/// Same ordinary PortsManage decision as the writer, keeping names out of a restricted error.
pub(super) async fn require_manage(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
) -> ApiResult<()> {
    authorization::require_resource_projected(
        conn,
        principal,
        ResourceKind::Environment,
        environment,
        Permission::PortsManage,
        Policy::default(),
        ctx.key_receipt_required(),
    )
    .await
}

pub(super) async fn reply(
    ctx: &Ctx,
    accepted: &Caller,
    opening: Id,
    environment: Id,
    revision: i64,
) -> ApiResult<PortOpeningWriteResponse> {
    let receipt = || {
        PortOpeningWriteResponse::Receipt(PortOpeningWriteReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            opening_id: opening,
            environment_id: environment,
            revision,
        })
    };
    if ctx.disclosure() == crate::key_operations::resolver::Disclosure::Denied {
        return Ok(receipt());
    }
    let full = crate::key_operations::completed_projection::run(ctx, async {
        let mut tx = ctx.app.db.begin_snapshot().await?;
        let full = reply_in(ctx, &mut tx, accepted, opening).await;
        let full = match full {
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
                None
            }
            other => other?,
        };
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(full)
    })
    .await?;
    Ok(match full {
        Some(full) => PortOpeningWriteResponse::Full(Box::new(full)),
        None => receipt(),
    })
}

async fn reply_in(
    ctx: &Ctx,
    conn: &mut Conn,
    accepted: &Caller,
    opening: Id,
) -> ApiResult<Option<PortOpening>> {
    let row = rows::opening(conn, opening)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let environment = super::environment(conn, row.environment_id).await?;
    let mut plan = base(ctx, conn, &row).await?;
    let now = Time::new(ctx.app.clock.now());
    let Some(current) = CapturedCaller::capture(accepted)
        .restore(conn, now)
        .await
        .map_err(authorization::restoration_error)?
    else {
        ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
        return Ok(None);
    };
    // Source preparation admits only its known primary Read, not the already committed Manage.
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: row.environment_id,
        },
    )
    .await?;
    let primary = plan
        .variant()
        .facets
        .iter()
        .find(|facet| {
            facet.resources == [ENVIRONMENT] && facet.usage == FacetUsage::ResponseContent
        })
        .ok_or_else(|| ApiError::internal("opening response Read is absent"))?;
    let facts =
        orch_engine::operation_scope::facets(ctx.route.name, primary, ENVIRONMENT, &[target])
            .map_err(crate::key_operations::admission::binding_error)?;
    let reader = match current.admit_facets(&facts) {
        Ok(reader) => reader.into_caller(),
        Err(_) => {
            ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
            return Ok(None);
        }
    };
    let principal = authorization::load_principal(conn, reader, now).await?;
    let principal = crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?;
    require_manage(ctx, conn, &principal, environment.id).await?;
    returned_sources(conn, &mut plan, &row).await?;
    let names = projection_names(
        conn,
        principal.caller(),
        &mut plan,
        name_sources(&row),
        &NAMES,
    )
    .await?;
    let projection = ctx.clone();
    plan.read_response(ctx, conn, accepted, move |conn, reader| {
        Box::pin(async move {
            // Account PortsManage is the same existing response authority; scope Read is additional.
            require_manage(&projection, conn, reader, environment.id).await?;
            super::opening_view_with_names(
                &projection,
                conn,
                reader,
                &row,
                &environment,
                false,
                Some(&names),
            )
            .await
        })
    })
    .await
}

pub(super) async fn get(ctx: Ctx) -> ApiResult<PortOpening> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<PreparedOpening>()?;
    let result = super::opening_view_with_names(
        &ctx,
        &mut tx,
        &principal,
        &prepared.row,
        &prepared.environment,
        prepared.audience_only,
        Some(&prepared.names),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(result)
}
