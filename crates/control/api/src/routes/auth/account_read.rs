//! Native account authority and ordinary root-organization source preparation.

use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan};
use crate::routes::nodes::{node_id, projection, read_sources, view};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::account_sources::{self as sources, Source};
use orch_api_contract::auth::{AdminMode, MeAuthorityReply};
use orch_api_contract::nodes::{Node, Org};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::engine::Graph;
use orch_authz::{Caller, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::{hierarchy, identity};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet};

const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);
const ORG: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Org);
type ObjectKey = (ObjectKind, Id);

pub(super) struct Organizations {
    pub(super) rows: Vec<Org>,
    pub(super) facts: Vec<native::Facts<Source>>,
    authority: Vec<orch_authz::nodes::Effective>,
    associations: BTreeMap<Id, BTreeSet<ObjectKey>>,
}

impl Organizations {
    pub(super) fn preserves(&self, old: &Self) -> bool {
        old.associations.iter().all(|(id, sources)| {
            self.associations
                .get(id)
                .is_some_and(|current| sources.is_subset(current))
        }) && old.authority.iter().all(|previous| {
            self.authority
                .iter()
                .any(|current| crate::routes::profiles::node_authority_preserved(previous, current))
        })
    }
}

pub(crate) async fn account(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<native::Facts<Source>> {
    account_for(ctx, conn, caller, Source::CurrentAccount).await
}

pub(super) async fn account_for(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    source: Source,
) -> ApiResult<native::Facts<Source>> {
    if ctx.user()?.id() != caller.user() {
        return Err(ApiError::internal("the account observation changed actor"));
    }
    Ok(native::Facts::one(
        source,
        ACCOUNT,
        targets::account(conn, caller.user()).await?,
    ))
}

/// The original visible_root_orgs(false) selector, before its native full node projection.
pub(super) async fn organizations(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    root: &native::Facts<Source>,
) -> ApiResult<Organizations> {
    let plan = OperationPlan::new(ctx.route, declaration.operation)?;
    let ids = hierarchy::visible_root_orgs(conn, caller.user(), false).await?;
    let graph = Graph::load(conn, &ids).await.map_err(ApiError::internal)?;
    let mut selected = BTreeMap::new();
    for id in &ids {
        if let Some(target) = orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
            && target.object.kind == ORG
            && (!native::modern(caller)
                || native::permits_usage(
                    &plan,
                    declaration,
                    caller,
                    &Source::Organizations,
                    FacetUsage::ResponseContent,
                    ORG,
                    &target,
                )?)
        {
            selected.insert(*id, target);
        }
    }
    let known = native::bind_known(
        ctx,
        declaration,
        &[
            root.clone(),
            native::Facts::collection(
                Source::Organizations,
                projection::groups(selected.values().cloned().map(|target| (ORG, target))),
            ),
        ],
    )?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    let ids: Vec<_> = ids
        .into_iter()
        .filter(|id| selected.contains_key(id))
        .collect();
    let nodes = view::visible(conn, &principal, &ids, ctx.now, Policy::default()).await?;
    let slot = known
        .variant()
        .facets
        .iter()
        .position(|facet| {
            facet.usage == FacetUsage::ResponseContent
                && facet.target
                    == TargetResolver::ResolvedCollection(sources::ORGANIZATION_CONTEXT_ROLE)
        })
        .ok_or_else(|| {
            ApiError::internal("the account organization context declaration is absent")
        })?;
    let projector = projection::Projection::response(ctx, caller, known, slot);
    let nodes = projector.project(conn, &principal, nodes).await?;
    let authority = orch_authz::nodes::effective_many(
        conn,
        &principal,
        &nodes.iter().map(node_id).collect::<Vec<_>>(),
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let mut context = Vec::new();
    let mut associations = BTreeMap::new();
    let mut rows = Vec::new();
    for node in nodes {
        if let Node::Org(org) = &node {
            let actual: BTreeSet<_> = read_sources::Sources::of(&node)?
                .objects()
                .filter(|source| source.kind != ObjectKind::Org || source.id != org.base.id)
                .map(|source| (source.kind, source.id))
                .collect();
            context.extend(
                actual
                    .iter()
                    .map(|&(kind, id)| orch_access_contract::decisions::ObjectRef { kind, id }),
            );
            associations.insert(org.base.id, actual);
        }
        if let Node::Org(org) = node {
            rows.push(org);
        }
    }
    let targets = read_sources::Sources::from_objects(context)
        .identity_targets(conn)
        .await?;
    let facts = vec![
        native::Facts::collection(
            Source::Organizations,
            projection::groups(rows.iter().map(|org| (ORG, selected[&org.base.id].clone()))),
        ),
        native::Facts::collection(
            Source::OrganizationContext,
            projection::groups(
                targets
                    .into_iter()
                    .map(|(object, target)| (KeyTargetKind::Object(object.kind), target)),
            ),
        ),
    ];
    Ok(Organizations {
        rows,
        facts,
        authority,
        associations,
    })
}

pub(super) struct Authority;
pub(super) struct AuthoritySelection {
    account: Id,
    orgs: Organizations,
    system_admin: bool,
    admin_available: bool,
}

impl native::Selected for Authority {
    type Source = Source;
    type Query = ();
    type Value = AuthoritySelection;
    type Reply = MeAuthorityReply;
    const DECLARATION: &'static Declaration<Source> = &sources::AUTHORITY;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(AuthoritySelection, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let root = account(ctx, conn, caller).await?;
            let orgs = organizations(ctx, conn, caller, Self::DECLARATION, &root).await?;
            let mut facts = vec![root];
            facts.extend(orgs.facts.iter().cloned());
            let person = identity::user_by_id(conn, caller.user())
                .await?
                .ok_or_else(ApiError::not_found)?;
            let admin_available =
                identity::admin_mode_available(conn, caller.user(), ctx.now).await?;
            Ok((
                AuthoritySelection {
                    account: person.id,
                    system_admin: person.is_system_admin,
                    admin_available,
                    orgs,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal(
                    "the authority observation changed actor",
                ));
            }
            Ok(())
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a AuthoritySelection,
    ) -> BoxFuture<'a, ApiResult<MeAuthorityReply>> {
        Box::pin(async move {
            let row = identity::user_by_id(conn, principal.user())
                .await?
                .ok_or_else(ApiError::not_found)?;
            let person = crate::sign_in::people::me(conn, &row, ctx.now).await?;
            let admin_mode = AdminMode {
                as_admin: ctx.user()?.as_admin,
                available: value.admin_available,
            };
            let meta = crate::routes::meta::on_connection(ctx, conn, Some(ctx.user()?)).await?;
            Ok(MeAuthorityReply {
                person,
                orgs: value.orgs.rows.clone(),
                admin_mode,
                meta,
            })
        })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a (),
        old: &'a AuthoritySelection,
        fresh: &'a AuthoritySelection,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.account == fresh.account
                && (!old.system_admin || fresh.system_admin)
                && (!old.admin_available || fresh.admin_available)
                && fresh.orgs.preserves(&old.orgs))
        })
    }
}
