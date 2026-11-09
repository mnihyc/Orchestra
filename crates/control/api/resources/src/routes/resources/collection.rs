//! Shared factual candidate and scoped-window selection for typed resource collections.
//!
//! Raw callers only discover stored identities. Ordinary visibility uses a current admitted
//! principal, and per-operation scope filtering happens before filling the returned window.

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::ResourceKind;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::engine::Graph;

use orch_authz::nodes::Page;

use orch_authz::resources;

use orch_db::Conn;

use orch_types::Id;

use std::collections::HashMap;
pub(super) type Filter<Q> = for<'a> fn(
    &'a mut Conn,
    &'a Principal,
    Vec<resources::Seen>,
    &'a Q,
) -> BoxFuture<'a, ApiResult<Vec<resources::Seen>>>;

pub(super) struct Window {
    pub after: Option<Id>,
    pub limit: usize,
}

pub(super) struct Query<'a, Q> {
    pub window: Window,
    pub value: &'a Q,
    pub filter: Filter<Q>,
}

pub(super) struct Selected {
    pub items: Vec<resources::Seen>,
    pub next_cursor: Option<String>,
}

/// Resource families are server-owned endpoint declarations, never inferred from a URL.
#[derive(Clone, Copy)]
pub(super) struct Collection {
    kind: ResourceKind,
}

impl Collection {
    pub const fn new(kind: ResourceKind) -> Self {
        Self { kind }
    }
    fn business(self) -> KeyTargetKind {
        KeyTargetKind::Object(ObjectKind::from(self.kind))
    }

    pub async fn targets(
        self,
        conn: &mut Conn,
        ids: &[Id],
    ) -> ApiResult<HashMap<Id, KeyResolvedTarget>> {
        let mut graph = Graph::default();
        let rows = resources::read_into(conn, &mut graph, ids)
            .await
            .map_err(ApiError::internal)?;
        let mut found = HashMap::new();
        for row in rows.into_iter().filter(|row| row.kind == self.kind) {
            if let Some(site) = resources::site_of(&graph, &row) {
                found.insert(row.id, targets::from_site(&graph, &site)?);
            }
        }
        Ok(found)
    }

    /// Existing list declarations bind their required result collection at slot zero.
    /// A current real target permits only effect-free ordinary preparation of that list.
    pub async fn principal_for(
        self,
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
        plan: &OperationPlan,
    ) -> ApiResult<Option<Principal>> {
        plan.shared()
            .compatible(caller)
            .map_err(crate::key_operations::admission::binding_error)?;
        let raw = authorization::load_principal(
            conn,
            caller.clone(),
            orch_types::Time::new(ctx.app.clock.now()),
        )
        .await?;
        if !modern(caller) {
            return Ok(Some(
                crate::sign_in::second_step::hold(conn, ctx.user()?, raw).await?,
            ));
        }
        let mut cursor = None;
        loop {
            // Same bounded Looker identities as the ordinary producer; no raw presentation.
            let candidates = resources::candidates(conn, &raw, self.kind, cursor)
                .await
                .map_err(ApiError::internal)?;
            if candidates.is_empty() {
                return Ok(None);
            }
            let next = candidates
                .last()
                .map(|row| row.id)
                .ok_or_else(|| ApiError::internal("a candidate batch has no last identity"))?;
            if cursor.is_some_and(|old| next <= old) {
                return Err(ApiError::internal(
                    "the ordinary resource candidate cursor did not advance",
                ));
            }
            let ids: Vec<_> = candidates.iter().map(|row| row.id).collect();
            let mut targets = self.targets(conn, &ids).await?;
            for id in ids {
                let Some(target) = targets.remove(&id) else {
                    continue;
                };
                if plan.permits_binding(
                    caller,
                    0,
                    self.business(),
                    std::slice::from_ref(&target),
                )? {
                    let mut initial = OperationPlan::new(ctx.route, ctx.route.name)?;
                    initial.bind(0, self.business(), &[target])?;
                    return initial
                        .preparation_principal(ctx, conn, caller)
                        .await
                        .map(Some);
                }
            }
            cursor = Some(next);
        }
    }

    /// Drain the same scoped windows for an existing exhaustive Vec endpoint. This adds
    /// neither a wire cursor nor a budget-created empty/exhausted result.
    pub async fn all<Q: Sync>(
        self,
        conn: &mut Conn,
        principal: &Principal,
        caller: &Caller,
        plan: &OperationPlan,
        query: Query<'_, Q>,
    ) -> ApiResult<Vec<resources::Seen>> {
        let Query {
            window: Window { mut after, limit },
            value,
            filter,
        } = query;
        let mut items = Vec::new();
        loop {
            let page = self
                .selected(
                    conn,
                    principal,
                    caller,
                    plan,
                    Query {
                        window: Window { after, limit },
                        value,
                        filter,
                    },
                )
                .await?;
            items.extend(page.items);
            let Some(next) = page.next_cursor else {
                break;
            };
            // selected creates this cursor from an actual Id; it is not a caller's target.
            let next: Id = next.parse().map_err(ApiError::internal)?;
            if after.is_some_and(|old| next <= old) {
                return Err(ApiError::internal(
                    "the exhaustive resource cursor did not advance",
                ));
            }
            after = Some(next);
        }
        Ok(items)
    }

    pub async fn selected<Q: Sync>(
        self,
        conn: &mut Conn,
        principal: &Principal,
        caller: &Caller,
        plan: &OperationPlan,
        query: Query<'_, Q>,
    ) -> ApiResult<Selected> {
        let Window { after, limit } = query.window;
        if !modern(caller) {
            // Keep the original ordinary first-page/filter order, including its cursor.
            let seen = resources::visible(
                conn,
                principal,
                self.kind,
                Page { after, limit },
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            let next_cursor = seen.next.map(|id| id.to_string());
            let items = (query.filter)(conn, principal, seen.items, query.value).await?;
            return Ok(Selected { items, next_cursor });
        }
        let mut cursor = after;
        let mut accepted = Vec::new();
        loop {
            let seen = resources::visible(
                conn,
                principal,
                self.kind,
                Page {
                    after: cursor,
                    limit,
                },
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            let next = seen.next;
            let seen = (query.filter)(conn, principal, seen.items, query.value).await?;
            let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
            let mut targets = self.targets(conn, &ids).await?;
            for row in seen {
                let target = targets
                    .remove(&row.resource.id)
                    .ok_or_else(ApiError::not_found)?;
                if plan.permits_binding(caller, 0, self.business(), &[target])? {
                    accepted.push(row);
                    if accepted.len() > limit {
                        break;
                    }
                }
            }
            if accepted.len() > limit || next.is_none() {
                break;
            }
            if cursor.zip(next).is_some_and(|(old, new)| new <= old) {
                return Err(ApiError::internal(
                    "the ordinary resource page cursor did not advance",
                ));
            }
            cursor = next;
        }
        let more = accepted.len() > limit;
        accepted.truncate(limit);
        let next_cursor = more
            .then(|| accepted.last().map(|row| row.resource.id.to_string()))
            .flatten();
        Ok(Selected {
            items: accepted,
            next_cursor,
        })
    }
}

pub(super) fn modern(caller: &Caller) -> bool {
    caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}
