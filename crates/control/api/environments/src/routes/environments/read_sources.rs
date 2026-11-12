//! The individual native whole, through-thread and retained Environment reading shapes.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::group_targets;

use crate::routes::access::operations::projection_names;

use crate::routes::nodes::read_sources::Sources;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::environment_sources as sources;

use orch_api_contract::environment_sources::GetSource as Source;

use orch_api_contract::nodes::DeletedInspectionQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller as AuthCaller;

use orch_authz::engine::Graph;

use std::collections::BTreeMap;

pub use orch_api_base::routes::environments::read_sources::Shape;

pub use orch_api_base::routes::environments::read_sources::Prepared;
const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

const NAMES: NameRoles = NameRoles {
    groups: Some("environment_read_groups"),
    nodes: [
        (ObjectKind::Org, "environment_read_orgs"),
        (ObjectKind::Project, "environment_read_projects"),
        (ObjectKind::Topic, "environment_read_topics"),
        (ObjectKind::Thread, "environment_read_threads"),
    ],
    containers: None,
};

pub(super) struct EnvironmentRead;

fn declaration(query: &DeletedInspectionQuery) -> &'static Declaration<Source> {
    if query.inspect_deleted == Some(true) {
        &sources::GET_INSPECT
    } else {
        &sources::GET_READ
    }
}

fn collection(
    source: Source,
    groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>,
) -> native::Facts<Source> {
    native::Facts::collection(
        source,
        groups
            .into_iter()
            .filter(|(_, values)| !values.is_empty())
            .collect(),
    )
}

/// The same native individual-view source factory under its consuming operation row.
pub(super) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &AuthCaller,
    query: &DeletedInspectionQuery,
    row: &'static Declaration<Source>,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let id = ctx.id("id")?;
    let state = hosting::environment(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let kept = query.inspect_deleted == Some(true) && state.deleted_at.is_some();
    let modern = native::modern(caller);
    let mut facts = Vec::new();
    if modern {
        let object = ObjectRef {
            kind: ObjectKind::Environment,
            id,
        };
        let target = if kept {
            targets::resource_identity(conn, object).await?
        } else {
            targets::page_object(conn, object).await?
        };
        facts.push(native::Facts::one(Source::Environment, ENVIRONMENT, target));
    }
    let partial = native::bind_known(ctx, row, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &partial).await?;
    let shape: ApiResult<Shape> = async {
        Ok(if kept {
            Shape::Retained(retained::prepare_read(ctx, conn, &principal, id).await?)
        } else {
            let reading = reading::decide(conn, &principal, id).await?;
            Shape::Live {
                projection: prepare_shown(conn, &principal, id).await?,
                reading,
            }
        })
    }
    .await;
    let shape = if modern {
        shape.map_err(authorization::problem_without_content)?
    } else {
        shape?
    };
    let mut value = Prepared {
        shape,
        disclosure: node_view::ReferenceDisclosure {
            names: BTreeSet::new(),
            displays: BTreeSet::new(),
        },
    };
    if !modern {
        return Ok((value, facts));
    }
    let objects = value
        .projection()
        .content_sources(conn)
        .await?
        .remove(&id)
        .unwrap_or_default();
    let actual = Sources::from_objects(objects)
        .identity_targets(conn)
        .await?;
    let mut foreign = BTreeMap::<_, Vec<_>>::new();
    for (object, target) in actual {
        foreign
            .entry(KeyTargetKind::Object(object.kind))
            .or_default()
            .push(target);
    }
    facts.push(collection(Source::Foreign, foreign));
    let names = value.projection().name_sources().clone();
    let groups = group_targets(conn, &names.groups.iter().copied().collect::<Vec<_>>()).await?;
    let mut partial = native::bind_known(ctx, row, &facts)?;
    // The name producer binds this slot once; retain its facts for final admission afterward.
    let bounds = projection_names(conn, caller, &mut partial, names, &NAMES).await?;
    facts.push(collection(
        Source::Groups,
        BTreeMap::from([(KeyTargetKind::Identity(KeyIdentityKind::Group), groups)]),
    ));
    let graph = Graph::load(conn, &bounds.nodes.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    for (kind, source) in [
        (ObjectKind::Org, Source::Orgs),
        (ObjectKind::Project, Source::Projects),
        (ObjectKind::Topic, Source::Topics),
        (ObjectKind::Thread, Source::Threads),
    ] {
        let mut selected = Vec::new();
        for id in &bounds.nodes {
            let target = orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
                .ok_or_else(ApiError::not_found)?;
            if target.object.kind == KeyTargetKind::Object(kind) {
                selected.push(target);
            }
        }
        facts.push(collection(
            source,
            BTreeMap::from([(KeyTargetKind::Object(kind), selected)]),
        ));
    }
    value.disclosure = node_view::reference_disclosure(
        conn,
        &principal,
        &bounds.nodes.iter().copied().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    value.projection_mut().bound_names(bounds);
    Ok((value, facts))
}
orch_api_base::__api_bind_07c8ba3b15!();

impl native::Selected for EnvironmentRead {
    type Source = Source;
    type Query = DeletedInspectionQuery;
    type Value = Prepared;
    type Reply = Environment;
    const DECLARATION: &'static Declaration<Source> = &sources::GET_READ;

    fn declaration(
        query: &DeletedInspectionQuery,
        _value: &Prepared,
    ) -> &'static Declaration<Source> {
        declaration(query)
    }

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a AuthCaller,
        query: &'a DeletedInspectionQuery,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, query, declaration(query)))
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal(
                    "the native Environment reader changed account",
                ));
            }
            Ok(())
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<Environment>> {
        Box::pin(value.render(ctx, conn, principal))
    }

    fn after_read<'a>(
        ctx: &'a Ctx,
        _accepted: &'a AuthCaller,
        _query: &'a DeletedInspectionQuery,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if let Shape::Retained(row) = &value.shape {
                retained::record_inspected(ctx, row.id).await?;
            }
            Ok(())
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a DeletedInspectionQuery,
        opened: &'a Prepared,
        fresh: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.preserves(fresh)) })
    }
}
