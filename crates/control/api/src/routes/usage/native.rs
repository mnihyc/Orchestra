//! Native usage subjects consumed by the common stored-source adapter.

use super::{caps, scope};
use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::usage::UsageHealthGetQuery;
use orch_api_contract::usage_sources::{self as sources, Source};
use orch_authz::{Principal, caps::Home};
use orch_db::{
    Conn,
    usage::caps::{self as rows, CapRow},
};
use orch_types::Id;

pub(super) enum Subject {
    Provider(Id),
    Cap(Box<CapRow>),
}

pub(super) struct Subjects;

pub(super) async fn home(conn: &mut Conn, home: Home) -> ApiResult<KeyResolvedTarget> {
    match home {
        Home::Node(id) => targets::node(conn, id).await,
        Home::Provider { provider, .. } => {
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: provider,
                },
            )
            .await
        }
        Home::System => Ok(targets::system()),
    }
}

impl native::Stored for Subjects {
    type Source = Source;
    type Value = Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::HEALTH, sources::CAP_DELETE];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            match declaration.bindings.first().map(|binding| binding.source) {
                Some(Source::CapHome) => {
                    let cap = rows::get(conn, ctx.id("id")?)
                        .await?
                        .ok_or_else(ApiError::not_found)?;
                    let target = home(conn, orch_engine::caps::home_of(&cap)).await?;
                    Ok((
                        Subject::Cap(Box::new(cap)),
                        vec![native::Facts::one(
                            Source::CapHome,
                            target.object.kind,
                            target,
                        )],
                    ))
                }
                _ => Err(ApiError::internal(
                    "the native usage subject needs its typed query",
                )),
            }
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match value {
                Subject::Provider(id) => {
                    scope::open(conn, principal, scope::Scope::Provider(*id)).await?;
                    Ok(())
                }
                Subject::Cap(cap) => caps::require(
                    conn,
                    principal,
                    orch_engine::caps::home_of(cap),
                    caps::Need::Manage,
                )
                .await
                .map_err(crate::middleware::authorization::problem_without_content),
            }
        })
    }
}

impl native::StoredQuery for Subjects {
    type Query = UsageHealthGetQuery;

    fn load_query<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
        query: UsageHealthGetQuery,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: query.provider_id,
                },
            )
            .await?;
            Ok((
                Subject::Provider(query.provider_id),
                vec![native::Facts::one(
                    Source::Provider,
                    KeyTargetKind::Object(ObjectKind::Provider),
                    target,
                )],
            ))
        })
    }
}

/// Group native facts by their actual business kind; no family is inferred from a reply.
pub(super) fn collection(source: Source, targets: Vec<KeyResolvedTarget>) -> native::Facts<Source> {
    let mut groups = std::collections::BTreeMap::new();
    for target in targets {
        groups
            .entry(target.object.kind)
            .or_insert_with(Vec::new)
            .push(target);
    }
    native::Facts::collection(source, groups.into_iter().collect())
}

/// Optional labels use the existing name-source scan and current typed node graph.
/// Public actor identities stay intrinsic; domain-derived addresses remain bounded.
pub(super) async fn names(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &orch_authz::Caller,
    declaration: &'static Declaration<Source>,
    mut names: crate::routes::access::page::NameSources,
) -> ApiResult<(
    crate::routes::access::page::NameBounds,
    native::Facts<Source>,
)> {
    use crate::routes::access::page::NameBounds;
    let groups = crate::routes::access::operations::group_targets(
        conn,
        &names.groups.iter().copied().collect::<Vec<_>>(),
    )
    .await?;
    names.nodes.extend(
        groups
            .iter()
            .flat_map(|target| target.node_ancestors.iter().filter_map(|node| node.id)),
    );
    names.nodes.extend(
        orch_db::queries::access::holder_user_addresses(
            conn,
            &names.users.iter().copied().collect::<Vec<_>>(),
        )
        .await?
        .into_iter()
        .map(|(_, _, domain, _)| domain),
    );
    let graph =
        orch_authz::engine::Graph::load(conn, &names.nodes.iter().copied().collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
    let plan = native::bind_known(ctx, declaration, &[])?;
    let mut bounds = NameBounds {
        limited: native::modern(caller),
        ..NameBounds::default()
    };
    let mut selected = Vec::new();
    for node in graph.nodes() {
        let Some(target) =
            orch_engine::operation_scope::targets::from_node_identity(&graph, node.id)
        else {
            continue;
        };
        if !bounds.limited
            || native::permits_usage(
                &plan,
                declaration,
                caller,
                &Source::Names,
                declaration
                    .bindings
                    .iter()
                    .find(|binding| binding.source == Source::Names)
                    .ok_or_else(|| ApiError::internal("the usage name source is undeclared"))?
                    .usage,
                target.object.kind,
                &target,
            )?
        {
            bounds.nodes.insert(node.id);
            selected.push(target);
        }
    }
    Ok((bounds, collection(Source::Names, selected)))
}

/// Retain the exact health source; a new provider/window cannot authorize an older body.
pub(super) fn retain_health(
    ctx: &Ctx,
    accepted: &orch_authz::Caller,
    provider: Id,
) -> ApiResult<()> {
    if !native::modern(accepted) {
        return Ok(());
    }
    crate::key_operations::publication::retain(
        ctx,
        accepted,
        provider,
        |ctx, conn, reader, provider| {
            Box::pin(async move {
                use native::StoredQuery;
                let (_, facts) = Subjects::load_query(
                    ctx,
                    conn,
                    &sources::HEALTH,
                    UsageHealthGetQuery {
                        provider_id: *provider,
                    },
                )
                .await?;
                let plan = native::bind(ctx, &sources::HEALTH, &facts)?;
                let provider = *provider;
                plan.read_response(ctx, conn, reader.caller(), move |conn, principal| {
                    Box::pin(async move {
                        scope::open(conn, principal, scope::Scope::Provider(provider)).await?;
                        Ok(())
                    })
                })
                .await?
                .ok_or_else(|| {
                    ApiError::new(
                        orch_api_contract::common::ErrorCode::AuthzDenied,
                        "the health source is no longer readable",
                    )
                })
            })
        },
    )
}

/// A report is owned by its native usage scope. All fields keep its native reader's privacy cut.
pub(super) struct Report<T> {
    pub answer: T,
    pub disclosure: Disclosure,
}
#[derive(Clone)]
pub(super) struct Disclosure {
    pub scope: scope::Scope,
    pub depth: orch_api_contract::usage::UsageDepth,
    pub names: super::names::Cut,
    pub records: Vec<orch_db::usage::ledger::EventRow>,
}
impl Disclosure {
    pub(super) async fn require(&self, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
        use orch_api_contract::usage::UsageDepth::{Own, Reports, Summary};
        let current = scope::open(conn, principal, self.scope).await?;
        let enough = matches!(
            (self.depth, current.depth),
            (Own, _) | (Summary, Summary | Reports) | (Reports, Reports)
        );
        let all_places = scope::all_record_places(conn, principal, &current).await?;
        let visible =
            scope::visible_records(conn, principal, all_places, self.records.clone()).await?;
        if !enough
            || visible.len() != self.records.len()
            || !self.names.current(conn, principal).await?
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the original usage projection is no longer readable",
            ));
        }
        Ok(())
    }
}
async fn scope_facts(
    conn: &mut Conn,
    caller: &orch_authz::Caller,
    scope: scope::Scope,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = match scope {
        scope::Scope::Me => targets::account(conn, caller.user()).await?,
        scope::Scope::Node(id) => targets::node(conn, id).await?,
        scope::Scope::Provider(id) => {
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id,
                },
            )
            .await?
        }
    };
    Ok(vec![native::Facts::one(
        Source::UsageScope,
        target.object.kind,
        target,
    )])
}
pub(super) async fn report_reader(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &orch_authz::Caller,
    declaration: &'static Declaration<Source>,
    scope: &str,
) -> ApiResult<(Principal, Vec<native::Facts<Source>>)> {
    let facts = scope_facts(conn, caller, scope::parse(scope)?).await?;
    let plan = native::bind(ctx, declaration, &facts)?;
    Ok((
        native::preparation_principal(ctx, conn, caller, &plan).await?,
        facts,
    ))
}
