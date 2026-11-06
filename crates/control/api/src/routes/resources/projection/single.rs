//! Native singleton cardinality, including individual styles and metadata generation cuts.

use super::*;
use crate::routes::{document_tree, documents, environments};
use orch_access_contract::permissions::ObjectKind as K;
use orch_api_contract::resources::Resource;
use orch_types::Time;
use std::collections::BTreeMap;
use std::sync::Arc;

enum Body {
    Resource(Box<Prepared>),
    Document(Box<documents::metadata_projection::Prepared>),
    Folder(orch_engine::document_tree::TreeRead),
}

pub(crate) struct Single {
    object: ObjectRef,
    body: Body,
}

/// The consumer's declared source slots; these are catalogue roles, never permissions.
pub(crate) struct Roles {
    pub(crate) names: NameRoles,
    pub(crate) sources: &'static str,
}

/// Actual native content origins, independent of response DTOs and operation admission.
#[derive(Clone, Copy)]
struct Origin {
    object: ObjectRef,
    retained: bool,
}

pub(crate) struct Bounds {
    names: NameSources,
    origins: Vec<Origin>,
    roles: &'static Roles,
}

pub(crate) struct Snapshot {
    pub(crate) value: Resource,
    object: ObjectRef,
    cut: Option<document_tree::metadata::Cut>,
    native: Option<Arc<Prepared>>,
}

/// The original metadata generation without retaining or cloning the rendered body.
#[derive(Clone)]
#[expect(dead_code, reason = "consumed by the resources cohort of scope-map-a")]
pub(crate) struct Validation {
    cut: Option<document_tree::metadata::Cut>,
    object: ObjectRef,
    native: Option<Arc<Prepared>>,
}

/// An individual projection's native subject may differ from the ownership page.
pub(crate) async fn subject(conn: &mut Conn, object: ObjectRef) -> ApiResult<KeyResolvedTarget> {
    if object.kind == K::Machine {
        return targets::machine_subject(conn, object.id).await;
    }
    if object.kind == K::McpServer {
        let selected = orch_db::queries::resources::mcp::subject(conn, object.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if let Some(id) = selected.environment_id {
            return targets::page_object(
                conn,
                ObjectRef {
                    kind: K::Environment,
                    id,
                },
            )
            .await;
        }
    }
    targets::page_object(conn, object).await
}

async fn seen(conn: &mut Conn, principal: &Principal, object: ObjectRef) -> ApiResult<Seen> {
    let (_, resource, _) = orch_authz::resources::site(conn, object)
        .await
        .map_err(ApiError::internal)?;
    let held =
        orch_authz::resources::effective(conn, principal, object, orch_authz::Policy::default())
            .await
            .map_err(ApiError::internal)?;
    Ok(Seen {
        resource,
        admin_place: held.admin_place,
        admin_only: held.admin_only,
        admin_access: held.admin_access,
        participating: held.participating,
        owner: held.owner,
        administers: held.administers,
        held: held.held,
    })
}

/// Select the existing native individual style; every resource reuses the common factual handoff.
pub(crate) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
) -> ApiResult<Single> {
    let id = object.id;
    let body = match object.kind {
        K::Document => Body::Document(Box::new(
            documents::metadata_projection::prepare(conn, principal, id).await?,
        )),
        K::DocumentContainer => {
            let read = orch_engine::document_tree::read_in(
                conn,
                orch_engine::document_tree::Actor::Person(principal),
                orch_engine::document_tree::Item::Container(id),
                principal.now(),
            )
            .await;
            Body::Folder(document_tree::result(conn, read).await?)
        }
        kind => {
            let view = match kind {
                K::Provider => View::IndividualProvider(Box::new(
                    providers::prepare_handover_projection(ctx, conn, principal, id).await?,
                )),
                K::Agent => View::Agent(agents::prepare_projection(conn, principal, id).await?),
                K::Template => {
                    View::Template(templates::prepare_projection(conn, principal, id).await?)
                }
                K::Environment => {
                    let native = seen(conn, principal, object).await?;
                    View::IndividualEnvironment(
                        environments::prepare_inventory(ctx, conn, principal, &[native]).await?,
                        id,
                    )
                }
                K::Host => {
                    let native = seen(conn, principal, object).await?;
                    View::Host(hosts::prepare_inventory(ctx, conn, principal, &[native]).await?)
                }
                K::Machine => {
                    let row = orch_db::queries::resources::machine(conn, id)
                        .await?
                        .ok_or_else(ApiError::not_found)?;
                    View::Machine(machines::prepare_inventory(conn, principal, vec![row]).await?)
                }
                K::McpServer => View::Mcp(
                    mcp::prepare(conn, principal, &[id], None, mcp::Style::Individual).await?,
                    NameBounds::default(),
                ),
                K::Mount => {
                    View::Mount(mounts::prepare_inventory(ctx, conn, principal, &[id]).await?)
                }
                K::Skill => {
                    View::Skill(skills::prepare_single_projection(conn, principal, id).await?)
                }
                _ => return Err(ApiError::not_found()),
            };
            Body::Resource(Box::new(Prepared::from_view(conn, view).await?))
        }
    };
    Ok(Single { object, body })
}

impl Single {
    async fn names(&self, conn: &mut Conn) -> ApiResult<NameSources> {
        Ok(match &self.body {
            Body::Resource(native) => native.name_sources().clone(),
            Body::Document(native) => native.names(conn).await?,
            Body::Folder(native) => NameSources {
                containers: native
                    .ancestors
                    .iter()
                    .filter(|row| row.title.is_some() || row.basename.is_some())
                    .map(|row| row.id)
                    .collect(),
                ..Default::default()
            },
        })
    }
    fn origins(&self) -> Vec<Origin> {
        match &self.body {
            Body::Resource(native) => native
                .foreign
                .iter()
                .map(|object| Origin {
                    object: *object,
                    retained: native.retained.contains(&(object.kind, object.id)),
                })
                .collect(),
            Body::Document(_) | Body::Folder(_) => Vec::new(),
        }
    }
    pub(crate) async fn bind(
        &mut self,
        conn: &mut Conn,
        caller: &Caller,
        plan: &mut OperationPlan,
        roles: &'static Roles,
    ) -> ApiResult<Bounds> {
        let names = self.names(conn).await?;
        let bounds = projection_names(conn, caller, plan, names.clone(), &roles.names).await?;
        let used = NameSources {
            groups: names.groups,
            nodes: bounds.nodes.clone(),
            containers: bounds.containers.clone(),
            ..Default::default()
        };
        match &mut self.body {
            Body::Resource(native) => native.bound_names(bounds),
            Body::Document(native) => native.bound_names(bounds),
            Body::Folder(native) if bounds.limited => {
                for ancestor in &mut native.ancestors {
                    if !bounds.containers.contains(&ancestor.id) {
                        ancestor.title = None;
                        ancestor.basename = None;
                    }
                }
            }
            Body::Folder(_) => {}
        }
        let origins = self.origins();
        bind_origins(conn, plan, roles.sources, &origins).await?;
        Ok(Bounds {
            names: used,
            origins,
            roles,
        })
    }
    pub(crate) async fn render(
        self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<Snapshot> {
        let mut current = ctx.clone();
        current.now = principal.now().get();
        let ctx = &current;
        let (value, node, native) = match self.body {
            Body::Resource(native) => {
                let native = Arc::new(*native);
                (
                    resource(native.render(ctx, conn, principal).await?)?,
                    None,
                    Some(native),
                )
            }
            Body::Document(native) => {
                let node = native.node();
                (
                    Resource::Document(native.render(conn, principal, Time::new(ctx.now)).await?),
                    Some(node),
                    None,
                )
            }
            Body::Folder(native) => {
                let node = native
                    .entries
                    .first()
                    .map(|row| row.entry.node)
                    .ok_or_else(ApiError::not_found)?;
                (
                    Resource::DocumentContainer(document_tree::convert(native)?),
                    Some(node),
                    None,
                )
            }
        };
        let cut = match node {
            Some(node) => Some(
                document_tree::metadata::cut(conn, principal, node, ctx.app.clock.time()).await?,
            ),
            None => None,
        };
        Ok(Snapshot {
            value,
            object: self.object,
            cut,
            native,
        })
    }
}

impl Bounds {
    /// Refresh only the exact sources already emitted; current bounds cannot relabel old content.
    pub(crate) async fn refresh(
        &self,
        conn: &mut Conn,
        caller: &Caller,
        plan: &mut OperationPlan,
    ) -> ApiResult<bool> {
        let fresh =
            projection_names(conn, caller, plan, self.names.clone(), &self.roles.names).await?;
        if !self.names.nodes.is_subset(&fresh.nodes)
            || !self.names.containers.is_subset(&fresh.containers)
        {
            return Ok(false);
        }
        bind_origins(conn, plan, self.roles.sources, &self.origins).await?;
        Ok(true)
    }
}
impl Snapshot {
    #[expect(dead_code, reason = "consumed by the resources cohort of scope-map-a")]
    pub(crate) fn validation(&self) -> Validation {
        Validation {
            cut: self.cut.clone(),
            object: self.object,
            native: self.native.clone(),
        }
    }
    pub(crate) fn object(&self) -> ObjectRef {
        self.object
    }
    pub(crate) fn has_cut(&self) -> bool {
        self.cut.is_some()
    }
    pub(crate) async fn current(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        now: Time,
    ) -> ApiResult<bool> {
        current_cut(conn, principal, self.cut.as_ref(), now).await
    }
    /// A metadata snapshot is committed before its current-generation validation begins.
    pub(crate) async fn after_snapshot(&self) {
        #[cfg(test)]
        tests::after_snapshot(self).await;
    }
}

impl Validation {
    #[expect(dead_code, reason = "consumed by the resources cohort of scope-map-a")]
    pub(crate) async fn current(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
        now: Time,
    ) -> ApiResult<bool> {
        if let Some(opened) = &self.native {
            let fresh = prepare(ctx, conn, principal, self.object).await?;
            let Body::Resource(fresh) = fresh.body else {
                return Ok(false);
            };
            if !opened.preserves_sources(&fresh) {
                return Ok(false);
            }
        }
        current_cut(conn, principal, self.cut.as_ref(), now).await
    }
}

async fn current_cut(
    conn: &mut Conn,
    principal: &Principal,
    cut: Option<&document_tree::metadata::Cut>,
    now: Time,
) -> ApiResult<bool> {
    let Some(cut) = cut else {
        return Ok(true);
    };
    match document_tree::metadata::unchanged(conn, principal, cut, now).await {
        Ok(()) => Ok(true),
        Err(error)
            if error.code() == orch_api_contract::common::ErrorCode::ConflictStaleRevision =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

async fn bind_origins(
    conn: &mut Conn,
    plan: &mut OperationPlan,
    role: &'static str,
    origins: &[Origin],
) -> ApiResult<()> {
    let index = plan
        .variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
        .ok_or_else(|| ApiError::internal("the native singleton source role is undeclared"))?;
    let mut groups = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
    for origin in origins {
        let target = if origin.object.kind == K::Machine {
            targets::machine_subject(conn, origin.object.id).await?
        } else if origin.retained {
            targets::resource_identity(conn, origin.object).await?
        } else {
            targets::page_object(conn, origin.object).await?
        };
        groups
            .entry(KeyTargetKind::Object(origin.object.kind))
            .or_default()
            .push(target);
    }
    if groups.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind_collection(index, &groups.into_iter().collect::<Vec<_>>())
    }
}
fn one<T>(rows: Vec<T>) -> ApiResult<T> {
    let mut rows = rows.into_iter();
    let value = rows.next().ok_or_else(ApiError::not_found)?;
    if rows.next().is_some() {
        return Err(ApiError::internal(
            "the native singleton returned several rows",
        ));
    }
    Ok(value)
}
fn resource(rows: Rendered) -> ApiResult<Resource> {
    Ok(match rows {
        Rendered::Agent(rows) => Resource::Agent(one(rows)?),
        Rendered::Provider(rows) => Resource::Provider(one(rows)?),
        Rendered::Template(rows) => Resource::Template(one(rows)?),
        Rendered::Mcp(rows) => Resource::McpServer(one(rows)?),
        Rendered::Host(rows) => Resource::Host(one(rows)?),
        Rendered::Environment(rows) => Resource::Environment(one(rows)?),
        Rendered::Machine(rows) => Resource::Machine(one(rows)?),
        Rendered::Mount(rows) => Resource::Mount(one(rows)?),
        Rendered::Skill(rows) => Resource::Skill(one(rows)?),
    })
}

#[cfg(test)]
mod tests;
