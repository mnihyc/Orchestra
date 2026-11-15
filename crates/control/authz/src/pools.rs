//! Current two-sided lending on a real pool membership path.
//!
//! A contribution is a factual current decision, never a token or reusable admission. Native
//! consumers retain their actual source and repeat this check at their own first-effect boundary.
use crate::{
    Caller, Error, Frame, Policy, Principal,
    giving::{Acting, Giving},
    options, resources,
    usable::{self, Policies, Usable, Use},
};
use orch_access_contract::{
    decisions::{ObjectRef, Through},
    pages::{Origin, PageKind},
    permissions::{ObjectKind, Options, Permission, ProviderUsePurpose},
    pools::{LendingMatcher, PathMember, PoolUse},
    roles::PlaceRef,
};
use orch_db::queries::pools::{Leaf, MemberRule};
use orch_db::{
    Conn,
    queries::{access, pools as stored},
};
use orch_types::Id;

/// Native consumers supply their whole actual request, never a synthesized person or grant.
pub trait Demand: Sync {
    /// All relevant limits of one complete pool entry have to allow this same request.
    fn pool(&self, options: &PoolUse) -> bool;
    /// Member-side typed bounds, or a same-family inner pool's complete bounds.
    fn lent(&self, options: &Options) -> bool;
    /// Current ordinary native member Use, preserving provider one-entry checks.
    fn direct(&self, usage: &Use) -> bool;
}

/// Only the real native selection can obtain one, and it cannot be serialized as authority.
#[derive(Debug, Clone)]
pub struct Contribution {
    object: ObjectRef,
    actor: Id,
    entries: Vec<Id>,
    lenders: Vec<(Id, Id)>,
    show_names: bool,
}
impl Contribution {
    pub fn object(&self) -> ObjectRef {
        self.object
    }
    pub fn actor(&self) -> Id {
        self.actor
    }
    pub fn entries(&self) -> &[Id] {
        &self.entries
    }
    pub fn lenders(&self) -> &[(Id, Id)] {
        &self.lenders
    }
    pub fn shows_names(&self) -> bool {
        self.show_names
    }
}

fn permission(kind: ObjectKind) -> Option<Permission> {
    Some(match kind {
        ObjectKind::Provider => Permission::ProviderUse,
        ObjectKind::Host => Permission::HostUse,
        ObjectKind::Environment => Permission::EnvironmentUse,
        ObjectKind::Template => Permission::TemplateUse,
        ObjectKind::Pool => Permission::PoolUse,
        _ => return None,
    })
}
fn source_error(error: orch_settings::Error) -> Error {
    match error {
        orch_settings::Error::Db(error) => Error::Db(error),
        other => Error::Stored(other.to_string()),
    }
}

/// Complete native pool allowances, before any model, service or effort fragments can be combined.
pub async fn pool_allowances(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    node: Option<Id>,
    policies: &Policies,
) -> Result<Vec<(Option<Id>, PoolUse)>, Error> {
    let usage = match usable::usable(conn, principal, id, node, policies).await {
        Ok(Usable::Yes(usage)) => usage,
        Ok(_) | Err(Error::NotFound(_)) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    if usage
        .through
        .iter()
        .any(|way| !matches!(way, Through::Entry(_)))
    {
        return Ok(vec![(None, PoolUse::unlimited())]);
    }
    let entries = access::entries_by_id(conn, &usage.entries).await?;
    let mut result = Vec::new();
    for entry in entries {
        if entry.permission == Permission::PoolManage.as_str() {
            result.push((Some(entry.id), PoolUse::unlimited()));
            continue;
        }
        if entry.permission != Permission::PoolUse.as_str() {
            continue;
        }
        if let Options::PoolUse(options) = Options::parse(Permission::PoolUse, &entry.options)
            .map_err(|faults| Error::Stored(format!("invalid pool allowance: {faults:?}")))?
        {
            result.push((Some(entry.id), *options));
        }
    }
    Ok(result)
}

async fn native_use(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    node: Option<Id>,
    policies: &Policies,
    demand: &dyn Demand,
) -> Result<Option<Vec<Id>>, Error> {
    if object.kind == ObjectKind::Environment {
        let allowed = resources::check(
            conn,
            principal,
            object,
            Permission::EnvironmentUse,
            policies.sharing_at(None),
        )
        .await?
        .is_allowed();
        return Ok((allowed && demand.lent(&Options::None)).then(Vec::new));
    }
    match usable::usable(conn, principal, object.id, node, policies).await {
        Ok(Usable::Yes(usage)) => Ok(demand.direct(&usage).then_some(usage.entries)),
        Ok(_) | Err(Error::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn matcher(
    conn: &mut Conn,
    matcher: &LendingMatcher,
    pool: &stored::Row,
    now: orch_types::Time,
) -> Result<bool, Error> {
    Ok(match matcher{
        LendingMatcher::Pool{pool_id}=>*pool_id==pool.id,
        LendingMatcher::Home{home_id}=>sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1 AND $2=ANY(path) AND deleted_at IS NULL)")
            .bind(pool.home_id).bind(home_id).fetch_one(conn).await.map_err(orch_db::DbError::from)?,
        LendingMatcher::Managers{manager_ids}=>{
            for manager in access::active_givers(conn,manager_ids).await?{
                let principal=Principal::load(conn,Caller::session(manager).with_admin_mode(false),now).await?;
                if resources::check(conn,&principal,ObjectRef{kind:ObjectKind::Pool,id:pool.id},Permission::PoolManage,Policy::default()).await?.is_allowed(){return Ok(true);}
            }
            false
        },
    })
}

struct Loan {
    entry: Id,
    giver: Id,
    show_names: bool,
    bounds: Options,
}
async fn lend(
    conn: &mut Conn,
    principal: &Principal,
    parent: &stored::Row,
    object: ObjectRef,
    node: Option<Id>,
    policies: &Policies,
    demand: &dyn Demand,
) -> Result<Option<Loan>, Error> {
    if parent
        .definition
        .get("lend_members")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Ok(None);
    }
    let Some(use_permission) = permission(object.kind) else {
        return Ok(None);
    };
    let (graph, resource, site) = resources::site(conn, object).await?;
    if resource.disabled
        || resource.deleted
        || policies.at(resource.home).keeps_to_its_owner(&resource)
            && resource.first_owner != Some(principal.user())
    {
        return Ok(None);
    }
    let permissions: Vec<_> = object.kind.permissions().collect();
    let actor_frame = Frame::load(
        conn,
        principal,
        policies.sharing_at(resource.home),
        graph.clone(),
        std::slice::from_ref(&site),
        &permissions,
    )
    .await?;
    let scopes: Vec<_> = actor_frame
        .chain(&site, use_permission)
        .origins()
        .into_iter()
        .filter_map(|origin| match origin {
            Origin::Own => Some((false, object.id)),
            Origin::Defaults(PlaceRef::Node(id)) => Some((true, id)),
            Origin::Defaults(PlaceRef::System) => Some((true, Id::nil())),
            _ => None,
        })
        .collect();
    let consents =
        stored::lending_for(conn, object.kind.as_str(), &scopes, principal.now().get()).await?;
    let active: std::collections::HashSet<_> = access::active_givers(
        conn,
        &consents
            .iter()
            .map(|entry| entry.giver_id)
            .collect::<Vec<_>>(),
    )
    .await?
    .into_iter()
    .collect();
    for consent in consents {
        if !active.contains(&consent.giver_id) {
            continue;
        }
        let selected: LendingMatcher = serde_json::from_value(consent.matcher)
            .map_err(|error| Error::Stored(error.to_string()))?;
        if !matcher(conn, &selected, parent, principal.now()).await? {
            continue;
        }
        let options = Options::parse(use_permission, &consent.options)
            .map_err(|faults| Error::Stored(format!("invalid lending allowance: {faults:?}")))?;
        if !demand.lent(&options) {
            continue;
        }
        let giver = match Principal::load(
            conn,
            Caller::session(consent.giver_id).with_admin_mode(false),
            principal.now(),
        )
        .await
        {
            Ok(giver) => giver,
            Err(Error::NoSuchUser(_)) => continue,
            Err(error) => return Err(error),
        };
        let frame = Frame::load(
            conn,
            &giver,
            policies.sharing_at(resource.home),
            graph.clone(),
            std::slice::from_ref(&site),
            &permissions,
        )
        .await?;
        let giving = Giving::of(&frame, Some(&site), PageKind::Live, Acting::Share)
            .with_policy(policies.sharing_at(resource.home));
        if !giving.can_give(use_permission, frame.graph().profile(&site, use_permission)) {
            continue;
        }
        if (resource.root != Some(parent.org_id)
            || resource
                .root
                .is_some_and(|root| !principal.is_in_tree(root)))
            && !crate::external::may_name_outside(&frame, &site, policies.sharing_at(resource.home))
        {
            continue;
        }
        let outside = resource.root != Some(parent.org_id)
            || resource
                .root
                .is_some_and(|root| !principal.is_in_tree(root));
        if outside {
            let lifetime = crate::external::LifeBounds::load(conn).await?;
            if lifetime
                .at(&crate::external::way_of(&site))
                .is_some_and(|bound| {
                    !bound.admits(
                        consent.expires_at.map(orch_types::Time::new),
                        orch_types::Time::new(consent.renewed_at.unwrap_or(consent.created_at)),
                    )
                })
            {
                continue;
            }
        }
        let allowed = if object.kind == ObjectKind::Pool {
            pool_allowances(conn, &giver, object.id, node, policies)
                .await?
                .iter()
                .any(|(_, options)| demand.pool(options))
        } else {
            native_use(conn, &giver, object, node, policies, demand)
                .await?
                .is_some()
        };
        if allowed {
            return Ok(Some(Loan {
                entry: consent.id,
                giver: consent.giver_id,
                show_names: consent.show_names,
                bounds: options,
            }));
        }
    }
    Ok(None)
}

/// Verify every current membership and both lending authorities for one actual leaf resource.
#[allow(
    clippy::too_many_arguments,
    reason = "the original actor, current membership, exact native request and policies remain separate"
)]
pub async fn member_choice(
    conn: &mut Conn,
    principal: &Principal,
    path: &[PathMember],
    leaf: &Leaf,
    object: ObjectRef,
    node: Option<Id>,
    policies: &Policies,
    demand: &dyn Demand,
) -> Result<Option<Contribution>, Error> {
    let intended = match leaf {
        Leaf::Model { backend } => backend.offering.as_ref().map(|offering| ObjectRef {
            kind: ObjectKind::Provider,
            id: offering.provider,
        }),
        Leaf::Provider { id } => Some(ObjectRef {
            kind: ObjectKind::Provider,
            id: *id,
        }),
        Leaf::Host { id } => Some(ObjectRef {
            kind: ObjectKind::Host,
            id: *id,
        }),
        Leaf::Environment { id } => Some(ObjectRef {
            kind: ObjectKind::Environment,
            id: *id,
        }),
        Leaf::EnvironmentTemplate {
            template_id,
            host_id,
        } => Some(if object.kind == ObjectKind::Template {
            ObjectRef {
                kind: ObjectKind::Template,
                id: *template_id,
            }
        } else {
            ObjectRef {
                kind: ObjectKind::Host,
                id: *host_id,
            }
        }),
    };
    if intended != Some(object) {
        return Ok(None);
    }
    let mut pools = Vec::new();
    let mut visited = std::collections::HashSet::new();
    for (index, hop) in path.iter().enumerate() {
        if !visited.insert(hop.pool_id) {
            return Ok(None);
        }
        let Some(row) = stored::get(conn, hop.pool_id).await?.filter(|row| {
            row.deleted_at.is_none()
                && row.kind == leaf.kind().as_str()
                && row
                    .definition
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
        }) else {
            return Ok(None);
        };
        let depth =
            orch_settings::get::<u64>(conn, orch_settings::Place::Pool(row.id), "pools.max_depth")
                .await
                .map_err(source_error)?
                .ok_or_else(|| Error::Stored("the pool depth has no registered default".into()))?;
        if (path.len() - index) as u64 > depth {
            return Ok(None);
        }
        let Some(member) = stored::member(conn, row.id, hop.member_id)
            .await?
            .filter(|member| member.enabled)
        else {
            return Ok(None);
        };
        let rule: MemberRule = serde_json::from_value(member.rule)
            .map_err(|error| Error::Stored(error.to_string()))?;
        if let Some(next) = path.get(index + 1) {
            if !matches!(rule,MemberRule::Pool{id,kind}if id==next.pool_id&&kind==leaf.kind()) {
                return Ok(None);
            }
        } else if !stored::rule_contains(conn, row.org_id, &rule, leaf).await? {
            return Ok(None);
        }
        pools.push(row);
    }
    let mut result = Contribution {
        object,
        actor: principal.user(),
        entries: Vec::new(),
        lenders: Vec::new(),
        show_names: false,
    };
    for (index, pool) in pools.iter().enumerate() {
        let entries = pool_allowances(conn, principal, pool.id, node, policies).await?;
        if let Some((entry, _)) = entries.iter().find(|(_, options)| demand.pool(options)) {
            result.entries.extend(*entry);
            continue;
        }
        let Some(parent) = index.checked_sub(1).and_then(|index| pools.get(index)) else {
            return Ok(None);
        };
        let Some(loan) = lend(
            conn,
            principal,
            parent,
            ObjectRef {
                kind: ObjectKind::Pool,
                id: pool.id,
            },
            node,
            policies,
            demand,
        )
        .await?
        else {
            return Ok(None);
        };
        if !matches!(&loan.bounds,Options::PoolUse(bounds)if demand.pool(bounds)) {
            return Ok(None);
        }
        result.lenders.push((loan.entry, loan.giver));
    }
    if let Some(entries) = native_use(conn, principal, object, node, policies, demand).await? {
        result.entries.extend(entries);
        result.show_names = resources::check(
            conn,
            principal,
            object,
            crate::engine::view_permission(object.kind),
            policies.sharing_at(None),
        )
        .await?
        .is_allowed();
        return Ok(Some(result));
    }
    let Some(parent) = pools.last() else {
        return Ok(None);
    };
    let Some(loan) = lend(conn, principal, parent, object, node, policies, demand).await? else {
        return Ok(None);
    };
    result.lenders.push((loan.entry, loan.giver));
    result.show_names = loan.show_names;
    Ok(Some(result))
}

struct ProviderDemand<'a, 'b> {
    wanted: &'a options::Wanted<'b>,
    purpose: ProviderUsePurpose,
}
impl Demand for ProviderDemand<'_, '_> {
    fn pool(&self, options: &PoolUse) -> bool {
        options::entry_allows_for(&options.provider, self.wanted, self.purpose)
    }
    fn lent(&self, options: &Options) -> bool {
        match options {
            Options::ProviderUse(options) => {
                options::entry_allows_for(options, self.wanted, self.purpose)
            }
            Options::PoolUse(options) => self.pool(options),
            _ => false,
        }
    }
    fn direct(&self, usage: &Use) -> bool {
        usage.allows_for(self.wanted, self.purpose)
    }
}
/// The provider's ordinary complete-entry rule, with the same current pool path for every consumer.
#[allow(
    clippy::too_many_arguments,
    reason = "the source's provider request and original current caller cannot be inferred from identifiers"
)]
pub async fn provider_choice(
    conn: &mut Conn,
    principal: &Principal,
    path: &[PathMember],
    leaf: &Leaf,
    node: Option<Id>,
    wanted: &options::Wanted<'_>,
    purpose: ProviderUsePurpose,
    policies: &Policies,
) -> Result<Option<Contribution>, Error> {
    let provider = match leaf {
        Leaf::Model { backend } => backend.offering.as_ref().and_then(|offering| {
            let exact = wanted
                .model
                .is_some_and(|(model, _)| model == offering.offering)
                || wanted
                    .service
                    .is_some_and(|(service, _)| service == offering.offering);
            exact.then_some(offering.provider)
        }),
        Leaf::Provider { id } => Some(*id),
        _ => None,
    };
    let Some(provider) = provider else {
        return Ok(None);
    };
    member_choice(
        conn,
        principal,
        path,
        leaf,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
        node,
        policies,
        &ProviderDemand { wanted, purpose },
    )
    .await
}
