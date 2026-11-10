//! One bounded native opening. Selection, ordinary visibility and credential
//! scope precede each projection; all suppliers share the current snapshot.

use super::node_id;

pub use super::projection;

use super::read_sources;

pub use crate::key_operations::targets;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use crate::registry::OperationPlan;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

pub use orch_access_contract::key_scope::KeyTargetKind;

pub use orch_access_contract::permissions::ObjectKind;

pub use orch_api_contract::nodes::Node;

use orch_api_contract::nodes::NodeRef;

pub use orch_authz::Caller;

pub use orch_db::Conn;

pub use orch_types::Id;

pub use std::collections::BTreeSet;

pub use std::sync::Mutex;
/// Factual sources of this exact declaration, shared with the native readers.
/// No wider principal is reconstructed and no serialized response is scrubbed.
pub struct Reading<'a> {
    pub caller: &'a Caller,
    pub delivery: bool,
    pub plan: OperationPlan,
    pub nodes: projection::Projection<'a>,
    pub primary: Mutex<Vec<KeyResolvedTarget>>,
    pub sources: Mutex<Vec<(KeyTargetKind, KeyResolvedTarget)>>,
}

impl Reading<'_> {
    pub fn delivery_allowed(&self) -> bool {
        self.delivery
    }
    pub fn remember(rows: &Mutex<Vec<KeyResolvedTarget>>, row: KeyResolvedTarget) -> ApiResult<()> {
        rows.lock()
            .map_err(|_| ApiError::internal("the read sources are unavailable"))?
            .push(row);
        Ok(())
    }

    pub async fn select_nodes(&self, conn: &mut Conn, ids: &[Id]) -> ApiResult<BTreeSet<Id>> {
        let graph = orch_authz::engine::Graph::load(conn, ids)
            .await
            .map_err(ApiError::internal)?;
        let live: Vec<_> = ids
            .iter()
            .copied()
            .filter(|id| graph.site_of(*id).is_some())
            .collect();
        Ok(self
            .nodes
            .selected(conn, &live)
            .await?
            .iter()
            .filter_map(|row| row.object.id)
            .collect())
    }

    pub async fn remember_node_ids(&self, conn: &mut Conn, ids: &[Id]) -> ApiResult<()> {
        for id in ids {
            Self::remember(&self.primary, targets::node(conn, *id).await?)?;
        }
        Ok(())
    }

    pub async fn remember_nodes(&self, conn: &mut Conn, nodes: &[Node]) -> ApiResult<()> {
        self.remember_node_ids(conn, &nodes.iter().map(node_id).collect::<Vec<_>>())
            .await?;
        for node in nodes {
            for (object, target) in read_sources::Sources::of(node)?.targets(conn).await? {
                self.remember_source(KeyTargetKind::Object(object.kind), target)?;
            }
        }
        Ok(())
    }

    pub async fn visible(&self, conn: &mut Conn, ids: &[Id]) -> ApiResult<Vec<Node>> {
        let nodes = self.nodes.visible(conn, ids).await?;
        self.remember_nodes(conn, &nodes).await?;
        Ok(nodes)
    }

    pub fn remember_source(
        &self,
        family: KeyTargetKind,
        target: KeyResolvedTarget,
    ) -> ApiResult<()> {
        self.sources
            .lock()
            .map_err(|_| ApiError::internal("the read sources are unavailable"))?
            .push((family, target));
        Ok(())
    }

    pub fn source_target(
        &self,
        family: KeyTargetKind,
        target: &KeyResolvedTarget,
    ) -> ApiResult<bool> {
        let allowed =
            self.plan
                .permits_binding(self.caller, 1, family, std::slice::from_ref(target))?;
        if allowed {
            self.remember_source(family, target.clone())?;
        }
        Ok(allowed)
    }

    /// The native selector supplies an actual typed identity before reading its
    /// protected fields. Scope is checked before its ordinary projection.
    pub async fn object(&self, conn: &mut Conn, kind: ObjectKind, id: Id) -> ApiResult<bool> {
        let target = match targets::object(
            conn,
            KeyObjectRef {
                kind: KeyTargetKind::Object(kind),
                id: Some(id),
            },
        )
        .await
        {
            Ok(target) => target,
            Err(error) if matches!(error.problem.status, 404 | 410) => return Ok(false),
            Err(error) => return Err(error),
        };
        self.source_target(KeyTargetKind::Object(kind), &target)
    }

    pub async fn node_source(&self, conn: &mut Conn, id: Id) -> ApiResult<bool> {
        let target = match targets::node(conn, id).await {
            Ok(target) => target,
            Err(error) if matches!(error.problem.status, 404 | 410) => return Ok(false),
            Err(error) => return Err(error),
        };
        let allowed = self.plan.permits_binding(
            self.caller,
            1,
            target.object.kind,
            std::slice::from_ref(&target),
        )?;
        if allowed {
            self.remember_source(target.object.kind, target)?;
        }
        Ok(allowed)
    }

    /// Resolve neutral current paths first; the native name producer receives
    /// this bound before it reads protected labels, never a broader DTO to scrub.
    pub async fn name_bounds(
        &self,
        conn: &mut Conn,
        ids: &[Id],
    ) -> ApiResult<crate::routes::access::page::NameBounds> {
        let limited = self
            .caller
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some());
        let mut bounds = crate::routes::access::page::NameBounds {
            limited,
            ..Default::default()
        };
        if !limited {
            return Ok(bounds);
        }
        let graph = orch_authz::engine::Graph::load(conn, ids)
            .await
            .map_err(ApiError::internal)?;
        let all: BTreeSet<_> = ids
            .iter()
            .flat_map(|id| {
                graph
                    .get(*id)
                    .into_iter()
                    .flat_map(|node| node.path.iter().copied())
            })
            .collect();
        for id in all {
            let Some(site) = graph.site_of(id) else {
                continue;
            };
            let target = targets::from_site(&graph, &site)?;
            if self.plan.permits_binding(
                self.caller,
                1,
                target.object.kind,
                std::slice::from_ref(&target),
            )? {
                bounds.nodes.insert(id);
            }
        }
        Ok(bounds)
    }

    /// Record the actual reference selected by the bounded native producer.
    pub async fn reference(&self, conn: &mut Conn, node: &NodeRef) -> ApiResult<()> {
        if node.redacted || (node.name.is_none() && node.thread_display.is_none()) {
            return Ok(());
        }
        let target = targets::node(conn, node.id).await?;
        if !self.plan.permits_binding(
            self.caller,
            1,
            target.object.kind,
            std::slice::from_ref(&target),
        )? {
            return Err(ApiError::internal(
                "the native reference exceeded its declared source",
            ));
        }
        self.remember_source(target.object.kind, target)
    }

    /// This section is an ordinary administrative read, not browser private access.
    /// Bind its actual Manage source before asking the existing native role guard.
    pub async fn outside_principal(
        &mut self,
        ctx: &Ctx,
        conn: &mut Conn,
        target: &KeyResolvedTarget,
        applicable: bool,
    ) -> ApiResult<Option<orch_authz::Principal>> {
        if applicable {
            let mut plan = self.plan.clone();
            plan.bind(4, target.object.kind, std::slice::from_ref(target))?;
            let principal = plan.preparation_principal(ctx, conn, self.caller).await?;
            match ctx
                .ordinary_node_administration(conn, &principal, ctx.id("id")?)
                .await
            {
                Ok(()) => {
                    self.plan = plan;
                    return Ok(Some(principal));
                }
                Err(error) if error.problem.status == 403 => {}
                Err(error) => return Err(error),
            }
        }
        self.plan.bind_empty(4)?;
        Ok(None)
    }

    pub fn finish(&self) -> ApiResult<OperationPlan> {
        let mut plan = self.plan.clone();
        let primary = self
            .primary
            .lock()
            .map_err(|_| ApiError::internal("the read sources are unavailable"))?;
        let mut seen = BTreeSet::new();
        projection::bind(
            &mut plan,
            0,
            projection::groups(
                primary
                    .iter()
                    .filter(|row| seen.insert((row.object.kind, row.object.id)))
                    .cloned()
                    .map(|row| (row.object.kind, row)),
            ),
        )?;
        let sources = self
            .sources
            .lock()
            .map_err(|_| ApiError::internal("the read sources are unavailable"))?;
        let mut seen = BTreeSet::new();
        projection::bind(
            &mut plan,
            1,
            projection::groups(
                sources
                    .iter()
                    .filter(|(family, row)| seen.insert((*family, row.object.kind, row.object.id)))
                    .cloned(),
            ),
        )?;
        Ok(plan)
    }
}
