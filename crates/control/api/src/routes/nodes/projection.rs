//! Credential bounds around the existing native Node producer, independent of its selector.

use super::{node_id, read_sources, tree, view};
use crate::key_operations::targets;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan};
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::{HiddenItem, Listed, Page};
use orch_api_contract::nodes::Node;
use orch_authz::engine::Graph;
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::Conn;
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet};

const PRIMARY: usize = 0;
const SOURCES: usize = 1;

/// The existing native page plus the actual row that justified its `more` bit.
/// Only its identity is disclosed by that bit; its DTO is never returned as a row.
pub(crate) struct Window<T> {
    pub(crate) page: Page<Listed<T>>,
    pub(crate) more: Option<Id>,
}

/// A selector supplies actual candidate identities; this adapter neither queries for
/// candidates nor grants ordinary View, restoration or private-metadata authority.
pub(crate) struct Projection<'a> {
    ctx: &'a Ctx,
    caller: &'a Caller,
    plan: OperationPlan,
    sources: usize,
}

pub(crate) fn groups(
    rows: impl IntoIterator<Item = (KeyTargetKind, KeyResolvedTarget)>,
) -> Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for (kind, target) in rows {
        groups.entry(kind).or_default().push(target);
    }
    groups.into_iter().collect()
}

pub(crate) fn bind(
    plan: &mut OperationPlan,
    slot: usize,
    rows: Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>,
) -> ApiResult<()> {
    if rows.is_empty() {
        plan.bind_empty(slot)
    } else {
        plan.bind_collection(slot, &rows)
    }
}

impl<'a> Projection<'a> {
    pub(crate) fn new(ctx: &'a Ctx, caller: &'a Caller, selection: &str) -> ApiResult<Self> {
        Ok(Self {
            ctx,
            caller,
            plan: OperationPlan::new(ctx.route, selection)?,
            sources: SOURCES,
        })
    }

    /// A completed writer supplies its own already declared optional source slot.
    /// This grants neither its effect again nor another endpoint's Read/parent authority.
    pub(crate) fn response(
        ctx: &'a Ctx,
        caller: &'a Caller,
        plan: OperationPlan,
        sources: usize,
    ) -> Self {
        Self {
            ctx,
            caller,
            plan,
            sources,
        }
    }

    /// Complete only actual returned content sources; do not call the list selector's
    /// parent/inspection gate or re-admit the completed mutation's required effects.
    pub(crate) async fn finish_response(
        &self,
        conn: &mut Conn,
        nodes: &[Node],
    ) -> ApiResult<OperationPlan> {
        let mut objects = Vec::new();
        for node in nodes {
            objects.extend(
                read_sources::Sources::of(node)?
                    .objects()
                    .filter(|object| !(object.kind.node().is_some() && object.id == node_id(node))),
            );
        }
        let sources = read_sources::Sources::from_objects(objects);
        let mut plan = self.plan.clone();
        bind(
            &mut plan,
            self.sources,
            groups(
                sources
                    .targets(conn)
                    .await?
                    .into_iter()
                    .map(|(object, target)| (KeyTargetKind::Object(object.kind), target)),
            ),
        )?;
        Ok(plan)
    }

    fn limited(&self) -> bool {
        self.caller
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some())
    }

    async fn targets(&self, conn: &mut Conn, ids: &[Id]) -> ApiResult<Vec<KeyResolvedTarget>> {
        let graph = Graph::load(conn, ids).await.map_err(ApiError::internal)?;
        ids.iter()
            .map(|id| {
                if self.ctx.inspect_deleted {
                    orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
                        .ok_or_else(ApiError::not_found)
                } else {
                    graph
                        .site_of(*id)
                        .ok_or_else(ApiError::not_found)
                        .and_then(|site| targets::from_site(&graph, &site))
                }
            })
            .collect()
    }

    /// Filter typed, current candidates before any native window is filled.
    pub(crate) async fn selected(
        &self,
        conn: &mut Conn,
        ids: &[Id],
    ) -> ApiResult<Vec<KeyResolvedTarget>> {
        self.targets(conn, ids)
            .await?
            .into_iter()
            .filter_map(|target| {
                match self.plan.permits_binding(
                    self.caller,
                    PRIMARY,
                    target.object.kind,
                    std::slice::from_ref(&target),
                ) {
                    Ok(true) => Some(Ok(target)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect()
    }

    pub(crate) async fn principal(
        &self,
        conn: &mut Conn,
        rows: &[KeyResolvedTarget],
    ) -> ApiResult<Principal> {
        let mut partial = self.plan.clone();
        bind(
            &mut partial,
            PRIMARY,
            groups(rows.iter().cloned().map(|row| (row.object.kind, row))),
        )?;
        if rows.is_empty() {
            partial.bind_empty(self.sources)?;
        }
        let principal = partial
            .preparation_principal(self.ctx, conn, self.caller)
            .await?;
        // Keep the route's original requested-place View even for a scope-filtered empty
        // list. The key needs no invented Read of that parent or its protected name.
        if self.ctx.inspect_deleted
            && crate::middleware::inspection::prepare(self.ctx, conn, &principal)
                .await?
                .is_some()
        {
            return Ok(principal);
        }
        authorization::require_projected(
            conn,
            &principal,
            self.ctx.id("id")?,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
            self.limited(),
        )
        .await?;
        Ok(principal)
    }

    /// Existing visibility and complete native payloads; optional references alone
    /// may be bounded. A mandatory source refusal drops the row, never its fields.
    pub(crate) async fn visible(&self, conn: &mut Conn, ids: &[Id]) -> ApiResult<Vec<Node>> {
        let selected = self.selected(conn, ids).await?;
        if selected.is_empty() {
            return Ok(Vec::new());
        }
        let principal = self.principal(conn, &selected).await?;
        let ids: Vec<_> = selected
            .iter()
            .filter_map(|target| target.object.id)
            .collect();
        let nodes = view::visible(conn, &principal, &ids, self.ctx.now, Policy::default()).await?;
        self.project(conn, &principal, nodes).await
    }

    pub(crate) async fn private_rows(
        &self,
        conn: &mut Conn,
        ids: &[Id],
    ) -> ApiResult<Vec<HiddenItem>> {
        let selected = self.selected(conn, ids).await?;
        if selected.is_empty() {
            return Ok(Vec::new());
        }
        let principal = self.principal(conn, &selected).await?;
        let ids: Vec<_> = selected
            .iter()
            .filter_map(|target| target.object.id)
            .collect();
        tree::private_rows(conn, &principal, &ids).await
    }

    pub(crate) async fn project(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        nodes: Vec<Node>,
    ) -> ApiResult<Vec<Node>> {
        if !self.limited() || nodes.is_empty() {
            return Ok(nodes);
        }
        let ids: Vec<_> = nodes.iter().map(node_id).collect();
        let intrinsic: BTreeSet<_> = ids.iter().copied().collect();
        let mut objects = Vec::new();
        for node in &nodes {
            objects.extend(
                read_sources::Sources::of(node)?
                    .objects()
                    .filter(|object| !(object.kind.node().is_some() && object.id == node_id(node))),
            );
        }
        let sources = read_sources::Sources::from_objects(objects);
        let resolved = sources.targets(conn).await?;
        let mut names = intrinsic.clone();
        let mut readmes = BTreeSet::new();
        for (object, target) in &resolved {
            if self.plan.permits_binding(
                self.caller,
                self.sources,
                KeyTargetKind::Object(object.kind),
                std::slice::from_ref(target),
            )? {
                if object.kind.node().is_some() {
                    names.insert(object.id);
                }
                if object.kind == ObjectKind::Document {
                    readmes.insert(object.id);
                }
            }
        }
        let projected = view::build_with_read_sources(
            conn,
            principal,
            &ids,
            self.ctx.now,
            Policy::default(),
            &names,
            &readmes,
        )
        .await?;
        let mut allowed = Vec::new();
        for built in projected {
            let mut complete = true;
            for object in read_sources::Sources::of(&built.node)?.objects() {
                if object.kind.node().is_some() && object.id == node_id(&built.node) {
                    continue;
                }
                let target = resolved.get(&object).ok_or_else(|| {
                    ApiError::internal("a native node projection introduced an uncaptured source")
                })?;
                if !self.plan.permits_binding(
                    self.caller,
                    self.sources,
                    KeyTargetKind::Object(object.kind),
                    std::slice::from_ref(target),
                )? {
                    complete = false;
                    break;
                }
            }
            if complete {
                allowed.push(built.node);
            }
        }
        Ok(allowed)
    }

    /// Bind the final native window, private rows, and the real look-ahead identity
    /// disclosed by a next cursor. Its unreturned payload contributes no source facets.
    pub(crate) async fn finish(
        &self,
        conn: &mut Conn,
        nodes: &[Node],
        hidden: &[HiddenItem],
        more: Option<Id>,
    ) -> ApiResult<OperationPlan> {
        let ids: Vec<_> = nodes
            .iter()
            .map(node_id)
            .chain(hidden.iter().map(|row| row.id))
            .chain(more)
            .collect();
        let primary = self.targets(conn, &ids).await?;
        let _principal = self.principal(conn, &primary).await?;
        let mut plan = self.plan.clone();
        bind(
            &mut plan,
            PRIMARY,
            groups(primary.into_iter().map(|row| (row.object.kind, row))),
        )?;
        let mut objects = Vec::new();
        for node in nodes {
            objects.extend(
                read_sources::Sources::of(node)?
                    .objects()
                    .filter(|object| !(object.kind.node().is_some() && object.id == node_id(node))),
            );
        }
        let sources = read_sources::Sources::from_objects(objects);
        let resolved = sources.targets(conn).await?;
        bind(
            &mut plan,
            self.sources,
            groups(
                resolved
                    .into_iter()
                    .map(|(object, target)| (KeyTargetKind::Object(object.kind), target)),
            ),
        )?;
        Ok(plan)
    }
}

/// Pure readers recheck their complete declared operation, including privileged
/// inspection and checked-empty proof, at the shared fresh response boundary.
/// Accepted mutation effects/receipts use the ordinary response helper instead.
pub(super) async fn disclose_read<T: Send + 'static>(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    response: OperationPlan,
    body: T,
    missing: &'static str,
) -> ApiResult<T> {
    let complete = response.shared().clone();
    response
        .read_response(ctx, conn, caller, move |_, reader| {
            Box::pin(async move {
                complete
                    .admit(reader.caller(), true)
                    .map_err(crate::key_operations::admission::binding_error)?;
                Ok(body)
            })
        })
        .await?
        .ok_or_else(|| ApiError::new(orch_api_contract::common::ErrorCode::AuthzDenied, missing))
}
