//! Common native lifecycle source facts and retained-original resolver composition.
use crate::{
    key_operations::{
        native::{self, effects},
        targets,
    },
    middleware::requests::{ApiError, ApiResult},
    registry::Ctx,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::environment_sources::lifecycle::Source;
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::Conn;
use orch_engine::jobs::environment_move::preparation::{self, Origin, Scope};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn error(e: orch_engine::EngineError) -> ApiError {
    match e {
        orch_engine::EngineError::BeforeEffect(_) => ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "The original native lifecycle source no longer permits this work.",
        ),
        other => ApiError::engine(other),
    }
}
pub(super) fn source_error(e: orch_engine::operation_scope::continuing::Error) -> ApiError {
    match e {
        orch_engine::operation_scope::continuing::Error::Native(e) => error(e),
        orch_engine::operation_scope::continuing::Error::Refused
        | orch_engine::operation_scope::continuing::Error::Binding(
            orch_engine::operation_scope::BindingError::Refused,
        ) => error(orch_engine::EngineError::BeforeEffect(Box::new(e))),
        other => ApiError::internal(other),
    }
}
pub(super) async fn objects(
    conn: &mut Conn,
    source: Source,
    objects: impl IntoIterator<Item = (ObjectKind, Id)>,
) -> ApiResult<native::Facts<Source>> {
    let mut groups = BTreeMap::new();
    for (kind, id) in objects.into_iter().collect::<BTreeSet<_>>() {
        let target = match kind {
            ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread => {
                targets::node_identity(conn, id).await?
            }
            ObjectKind::Machine => targets::machine_subject(conn, id).await?,
            _ => targets::resource_identity(conn, ObjectRef { kind, id }).await?,
        };
        groups
            .entry(KeyTargetKind::Object(kind))
            .or_insert_with(Vec::new)
            .push(target);
    }
    Ok(native::Facts::collection(
        source,
        groups.into_iter().collect(),
    ))
}
pub(super) async fn source(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    kind: preparation::Kind,
    request: &orch_types::Json,
    environments: &[Id],
    review: Option<&orch_db::queries::jobs::forks::PreflightRow>,
) -> ApiResult<Scope> {
    let captured = CapturedOperation::capture(caller).map_err(ApiError::internal)?;
    let http = native::http_origin::capture_operation(ctx, conn, &captured).await?;
    let request = orch_types::Hash::of(&serde_json::to_vec(request).map_err(ApiError::internal)?);
    let origin = Origin::capture(conn, http, kind, request, environments, review)
        .await
        .map_err(error)?;
    Ok(Scope::new(origin))
}
pub(super) async fn content(
    conn: &mut Conn,
    origin: &Origin,
    extra: impl IntoIterator<Item = (ObjectKind, Id)>,
) -> ApiResult<native::Facts<Source>> {
    let environments = origin.environments().collect::<Vec<_>>();
    let extra = extra
        .into_iter()
        .chain(
            origin
                .destinations()
                .map(|(object, _)| (object.kind, object.id)),
        )
        .collect::<Vec<_>>();
    content_for(conn, &environments, extra).await
}
/// Factual names emitted by the native Environment formatter, independent of a spent confirmation.
pub(super) async fn content_for(
    conn: &mut Conn,
    environments: &[Id],
    extra: impl IntoIterator<Item = (ObjectKind, Id)>,
) -> ApiResult<native::Facts<Source>> {
    let mut selected = extra.into_iter().collect::<BTreeSet<_>>();
    for &id in environments {
        selected.insert((ObjectKind::Environment, id));
        let row = orch_db::queries::environments::hosting::environment(conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let home = targets::node_identity(conn, row.home_node_id).await?;
        if let KeyTargetKind::Object(kind) = home.object.kind {
            selected.insert((kind, row.home_node_id));
        }
        if let Some(id) = row.machine_id {
            selected.insert((ObjectKind::Machine, id));
        }
        if let Some(id) = row.template_id {
            selected.insert((ObjectKind::Template, id));
        }
        if let Some(runtime) =
            orch_db::queries::environments::hosting::live_runtime(conn, row.id).await?
        {
            selected.insert((ObjectKind::Host, runtime.host_id));
        }
    }
    let mut names = BTreeSet::new();
    for &(kind, id) in &selected {
        let target = match kind {
            ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread => {
                targets::node_identity(conn, id).await?
            }
            ObjectKind::Machine => targets::machine_subject(conn, id).await?,
            _ => targets::resource_identity(conn, ObjectRef { kind, id }).await?,
        };
        for parent in target.node_ancestors {
            if let (KeyTargetKind::Object(kind), Some(id)) = (parent.kind, parent.id) {
                names.insert((kind, id));
            }
        }
    }
    selected.extend(names);
    objects(conn, Source::Content, selected).await
}
pub(super) async fn ordinary(
    conn: &mut Conn,
    principal: &Principal,
    environments: impl IntoIterator<Item = Id>,
) -> ApiResult<()> {
    for id in environments {
        if !orch_engine::updates::may_manage(
            conn,
            principal,
            orch_engine::updates::Target::Environment(id),
        )
        .await
        .map_err(ApiError::engine)?
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "Environment management is required.",
            ));
        }
    }
    Ok(())
}
/// A once-only native observation retains its actual original/current minima for the common resolver.
pub(super) trait Observed {
    fn origin(&self) -> &Origin;
}
pub(super) fn prepare<'a, F>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: F::Query,
    body: F::Body,
) -> crate::registry::OperationFuture<'a>
where
    F: effects::Initial,
    F::Fetched: Observed,
{
    Box::pin(async move {
        let observed = effects::initial_value::<F>(ctx)?;
        ctx.publication.retain_deadline(observed.origin().until())?;
        let plan = effects::prepare::<F>(ctx, conn, caller, query, body).await?;
        plan.with_origin(ctx, conn, caller, observed.origin().captured())
            .await
    })
}
pub(super) fn current<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    origin: &'a Origin,
) -> BoxFuture<'a, ApiResult<()>> {
    Box::pin(async move {
        if origin.environments().next().is_none() {
            return Ok(());
        }
        let check = origin
            .checkpoint(conn, ctx.app.clock.as_ref())
            .await
            .map_err(error)?;
        ctx.publication.retain_deadline(check.until())?;
        ctx.publication.retain_ceiling(check.caller())?;
        check.check(ctx.app.clock.as_ref()).map_err(error)
    })
}

/// Name obligations come from the native ordinary reader, not an arbitrary walk over response UUIDs.
pub(super) async fn visible_threads(
    conn: &mut Conn,
    p: &Principal,
    ids: &[Id],
) -> ApiResult<Vec<Id>> {
    Ok(orch_authz::nodes::holding(
        conn,
        p,
        ids,
        orch_access_contract::permissions::PermissionName::OfNode(
            orch_access_contract::permissions::NodePermission::View,
        ),
        orch_authz::Act::Use,
        orch_authz::Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|n| n.id)
    .collect())
}
pub(super) async fn require_names(conn: &mut Conn, p: &Principal, ids: &[Id]) -> ApiResult<()> {
    for id in ids {
        crate::middleware::authorization::require(
            conn,
            p,
            *id,
            crate::middleware::authorization::VIEW,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await?;
    }
    Ok(())
}
