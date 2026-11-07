//! Current credential collection facts and their shared row/reference projection.
//!
//! Account collections retain their real Account root; an org inventory keeps
//! its separate governance root and reuses the same Key metadata/name projection.
//! Each returned credential needs its operation's Read; Account self never
//! substitutes for a Key/Session. The consuming producer retains the existing
//! ordinary owner, service-management or org-management authorization.

use super::{admission::Prepared, targets};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::auth::{ApiTokenInfo, SessionInfo};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::{Caller, Principal, engine::Graph};
use orch_db::{Conn, queries::identity};
use orch_types::{Id, Time};
use std::collections::HashMap;

const ROOT: usize = 0;
const ROWS: usize = 1;
const NAME_START: usize = 2;
const NAME_KINDS: [ObjectKind; 4] = [
    ObjectKind::Org,
    ObjectKind::Project,
    ObjectKind::Topic,
    ObjectKind::Thread,
];

fn key_names_declared(plan: &Prepared) -> bool {
    let facets = &plan.variant().facets;
    facets.len() == NAME_START + NAME_KINDS.len()
        && facets
            .iter()
            .skip(NAME_START)
            .zip(NAME_KINDS)
            .all(|(facet, kind)| {
                facet.usage == FacetUsage::ResponseContent
                    && facet.action == KeyAction::Read
                    && facet.resources.as_slice() == [KeyTargetKind::Object(kind)]
                    && matches!(facet.target, TargetResolver::ResolvedCollection(_))
            })
}

pub(crate) fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the credential collection is outside this key's current scope",
    )
}

pub(crate) fn project_error(ctx: &Ctx, error: ApiError) -> ApiError {
    if ctx.key_receipt_required() {
        authorization::problem_without_content(error)
    } else {
        error
    }
}

/// One bare metadata producer for the existing personal, service and org inventories.
/// Restriction names follow their native metadata entitlement; scope bounds are separate.
pub(crate) fn key_info(row: identity::ApiTokenListed) -> ApiResult<ApiTokenInfo> {
    let (rules, constraints, excluded_places) =
        crate::routes::auth::tokens::scope_info(row.operation_scope)?
            .map(|scope| (Some(scope.rules), scope.constraints, scope.excluded_places))
            .unwrap_or_default();
    Ok(ApiTokenInfo {
        id: row.id,
        name: row.name,
        scopes: row.scopes,
        rules,
        constraints,
        excluded_places,
        node: row.node_id.map(|id| NodeRef {
            thread_display: None,
            id,
            kind: row
                .node_kind
                .as_deref()
                .and_then(authorization::refusals::node_kind)
                .unwrap_or(NodeKind::Org),
            name: row.node_name,
            redacted: false,
        }),
        created_at: Time::new(row.created_at),
        last_used_at: row.last_used_at.map(Time::new),
        expires_at: row.expires_at.map(Time::new),
    })
}

/// The existing six-facet key collection's optional restriction labels. Root
/// governance and whole credential rows are bound by the consuming producer.
/// A legacy node limit is configuration, never Key placement or an account witness.
pub(crate) async fn key_names(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut Prepared,
    info: &mut [ApiTokenInfo],
) -> ApiResult<()> {
    if !key_names_declared(plan) {
        return Err(ApiError::internal(
            "the key restriction-name sources are undeclared",
        ));
    }
    let mut node_ids: Vec<_> = info
        .iter()
        .filter_map(|info| info.node.as_ref())
        .filter(|node| node.name.is_some())
        .map(|node| node.id)
        .collect();
    node_ids.sort_unstable();
    node_ids.dedup();
    let graph = if node_ids.is_empty() {
        None
    } else {
        Some(
            Graph::load(conn, &node_ids)
                .await
                .map_err(ApiError::internal)?,
        )
    };
    let mut names = HashMap::new();
    let mut named: [Vec<KeyResolvedTarget>; 4] = std::array::from_fn(|_| Vec::new());
    for id in node_ids {
        let Some(graph) = &graph else { continue };
        let Some(target) = orch_engine::operation_scope::targets::from_node_identity(graph, id)
        else {
            continue;
        };
        let KeyTargetKind::Object(kind) = target.object.kind else {
            continue;
        };
        let Some(index) = NAME_KINDS.iter().position(|known| *known == kind) else {
            continue;
        };
        if plan.permits_binding(
            caller,
            NAME_START + index,
            target.object.kind,
            std::slice::from_ref(&target),
        )? {
            names.insert(id, ());
            named[index].push(target);
        }
    }
    for (index, targets) in named.into_iter().enumerate() {
        if targets.is_empty() {
            plan.bind_empty(NAME_START + index)?;
        } else {
            plan.bind(
                NAME_START + index,
                KeyTargetKind::Object(NAME_KINDS[index]),
                &targets,
            )?;
        }
    }
    for info in info {
        if let Some(node) = &mut info.node
            && node.name.is_some()
            && !names.contains_key(&node.id)
        {
            node.name = None;
            node.redacted = true;
        }
    }
    Ok(())
}

/// Root admission is established before a service producer performs its ordinary
/// management check. No child credential query or activity fact occurs here.
pub(crate) struct Collection {
    plan: Prepared,
    account: KeyResolvedTarget,
    principal: Principal,
    family: KeyTargetKind,
}

#[derive(Clone)]
pub(crate) struct Read<T> {
    pub plan: Prepared,
    pub data: T,
}

/// Finish disclosure only at the real reader's checkpoint. A returned complete
/// plan cannot by itself turn Pending into Full, and a denied Read stays denied.
pub(crate) async fn complete<T: Clone + Send + Sync + 'static>(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    read: &Read<T>,
) -> ApiResult<T> {
    let data = read.data.clone();
    read.plan
        .clone()
        .read_response(ctx, conn, principal.caller(), move |_, _| {
            Box::pin(async move { Ok(data) })
        })
        .await?
        .ok_or_else(refused)
}

impl Collection {
    pub(crate) async fn begin(
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
        owner: Id,
        kind: KeyIdentityKind,
    ) -> ApiResult<Self> {
        if !matches!(kind, KeyIdentityKind::Key | KeyIdentityKind::Session) {
            return Err(ApiError::internal(
                "a credential collection needs a credential family",
            ));
        }
        let family = KeyTargetKind::Identity(kind);
        let mut plan = Prepared::new(ctx.route, ctx.route.name)?;
        let facets = &plan.variant().facets;
        let expected = if kind == KeyIdentityKind::Key { 6 } else { 2 };
        if facets.len() != expected
            || facets[ROOT].usage != FacetUsage::Required
            || facets[ROOT].action != KeyAction::Read
            || facets[ROOT].resources.as_slice() != [family]
            || facets[ROWS].usage != FacetUsage::ResponseContent
            || facets[ROWS].action != KeyAction::Read
            || facets[ROWS].resources.as_slice() != [family]
            || !matches!(facets[ROWS].target, TargetResolver::ResolvedCollection(_))
            || (kind == KeyIdentityKind::Key && !key_names_declared(&plan))
        {
            return Err(ApiError::internal(
                "the credential collection metadata is inconsistent",
            ));
        }
        let roots = targets::resolve(
            ctx,
            conn,
            &facets[ROOT],
            &targets::TargetBindings::default(),
        )
        .await?;
        let [account] = roots.as_slice() else {
            return Err(refused());
        };
        if account.object
            != (KeyObjectRef {
                kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
                id: Some(owner),
            })
        {
            return Err(refused());
        }
        let account = account.clone();
        plan.bind(ROOT, family, std::slice::from_ref(&account))?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        Ok(Self {
            plan,
            account,
            principal,
            family,
        })
    }

    pub(crate) fn principal(&self) -> &Principal {
        &self.principal
    }

    fn owner(&self) -> ApiResult<Id> {
        self.account
            .object
            .id
            .ok_or_else(|| ApiError::internal("the account collection has no identity"))
    }

    fn bind_rows(&mut self, rows: &[KeyResolvedTarget]) -> ApiResult<()> {
        if rows.is_empty() {
            self.plan.bind_empty(ROWS)
        } else {
            self.plan.bind(ROWS, self.family, rows)
        }
    }

    /// Read the original owner-filtered query only after its ordinary authorization.
    /// The same stored owner field supplies the Account-subtree witness for every row.
    pub(crate) async fn keys(mut self, conn: &mut Conn) -> ApiResult<Read<Vec<ApiTokenInfo>>> {
        if self.family != KeyTargetKind::Identity(KeyIdentityKind::Key) {
            return Err(ApiError::internal("the collection is not a key collection"));
        }
        let mut kept = Vec::new();
        let mut row_targets = Vec::new();
        for row in identity::api_tokens_of(conn, self.owner()?).await? {
            let target = targets::from_key_subject(&self.account, row.id, row.user_id)?;
            if self.plan.permits_binding(
                self.principal.caller(),
                ROWS,
                self.family,
                std::slice::from_ref(&target),
            )? {
                row_targets.push(target);
                kept.push(row);
            }
        }
        self.bind_rows(&row_targets)?;

        let mut data = kept
            .into_iter()
            .map(key_info)
            .collect::<ApiResult<Vec<_>>>()?;
        key_names(conn, self.principal.caller(), &mut self.plan, &mut data).await?;
        Ok(Read {
            plan: self.plan,
            data,
        })
    }

    pub(crate) async fn sessions(
        mut self,
        conn: &mut Conn,
        current: Option<Id>,
    ) -> ApiResult<Read<Vec<SessionInfo>>> {
        if self.family != KeyTargetKind::Identity(KeyIdentityKind::Session) {
            return Err(ApiError::internal(
                "the collection is not a session collection",
            ));
        }
        let mut data = Vec::new();
        let mut targets = Vec::new();
        for row in identity::live_sessions(conn, self.owner()?, self.principal.now().get()).await? {
            let target = targets::from_session_subject(&self.account, row.id, row.user_id)?;
            if self.plan.permits_binding(
                self.principal.caller(),
                ROWS,
                self.family,
                std::slice::from_ref(&target),
            )? {
                targets.push(target);
                data.push(SessionInfo {
                    id: row.id,
                    current: Some(row.id) == current,
                    user_agent: row.user_agent,
                    ip: row.ip,
                    created_at: Time::new(row.created_at),
                    last_used_at: Time::new(row.last_used_at),
                    expires_at: Time::new(row.expires_at),
                });
            }
        }
        self.bind_rows(&targets)?;
        Ok(Read {
            plan: self.plan,
            data,
        })
    }
}
