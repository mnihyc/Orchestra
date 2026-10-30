//! Who may add, change and remove a usage cap; who may let a turn past one that is reached, by the row where that cap is defined; and who sees how much was used: one's own in full, a place's totals by its row, a place's reports.
//!
//! A cap is defined in one of three places, and the place decides everything
//! about it ([`Home`]): on a provider (a cap that names a provider, and may also
//! name a node), on a node (one that names a node and no provider), or on the
//! system (one that names neither). The person who may add, change and remove
//! it holds `provider.manage` on the provider (and `node.view` on the node it
//! also names), `usage.manage` on the node, or is a system admin. Reading the
//! caps defined in a place needs `usage.view`, `provider.view`, or being a
//! system admin. A turn is let past a cap that is reached by whoever holds
//! `provider.bypass` on its provider, `usage.bypass` on its node, or is a system
//! admin; being an admin lifts no cap by itself.
//!
//! How much of the use of a place a person may read is [`Depth`]: all of it with
//! `usage.view`, the figures without anybody else's records with
//! `usage.summary`, and one's own with nothing but seeing the place.
//!
//! Each question is asked of the same engine as any other, by the same
//! functions, so that a row of a page emptied or given is seen at once.

use crate::engine::{Frame, Graph};
use crate::{Act, Error, Policy, Principal, holders, nodes, resources};
use orch_access_contract::decisions::{Decision, ObjectRef, Refusal};
use orch_access_contract::permissions::{NodePermission, ObjectKind, Permission, PermissionName};
use orch_access_contract::roles::Role;
use orch_db::Conn;
use orch_db::queries::access;
use orch_types::{Id, Time};

/// The most system admins that are looked for when they are told of a cap.
const MOST_ADMINS: i64 = 1_000;

/// Where a cap is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// On a provider; it may name a node as well.
    Provider {
        /// The provider.
        provider: Id,
        /// The node it also names.
        node: Option<Id>,
    },
    /// On a node, naming no provider.
    Node(Id),
    /// On the system: it names neither.
    System,
}

impl Home {
    /// The place a cap with this node and this provider is defined in.
    #[must_use]
    pub const fn of(node: Option<Id>, provider: Option<Id>) -> Self {
        match (provider, node) {
            (Some(provider), node) => Self::Provider { provider, node },
            (None, Some(node)) => Self::Node(node),
            (None, None) => Self::System,
        }
    }
}

/// What an access question about a cap came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// They may.
    Allowed,
    /// They may not, and this is what they lack and who can give it.
    Refused(Box<Refusal>),
    /// Only a system admin may, and they are none.
    SystemAdminOnly,
}

impl Verdict {
    /// Whether they may.
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    fn of(decision: Decision) -> Self {
        match decision {
            Decision::Allowed { .. } => Self::Allowed,
            Decision::Refused(refusal) => Self::Refused(Box::new(refusal)),
        }
    }
}

fn provider_object(provider: Id) -> ObjectRef {
    ObjectRef {
        kind: ObjectKind::Provider,
        id: provider,
    }
}

/// Whether the person holds `permission` on the node.
async fn node_verdict(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    permission: Permission,
    policy: Policy,
) -> Result<Verdict, Error> {
    Ok(Verdict::of(
        nodes::check(conn, principal, node, permission, Act::Use, policy).await?,
    ))
}

/// Whether the person holds `permission` on the provider.
async fn provider_verdict(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    permission: Permission,
    policy: Policy,
) -> Result<Verdict, Error> {
    Ok(Verdict::of(
        resources::check(
            conn,
            principal,
            provider_object(provider),
            permission,
            policy,
        )
        .await?,
    ))
}

/// Whether the person may add, change or remove a cap defined in `home`.
pub async fn check_manage(
    conn: &mut Conn,
    principal: &Principal,
    home: Home,
    policy: Policy,
) -> Result<Verdict, Error> {
    match home {
        Home::Provider { provider, node } => {
            let held = provider_verdict(
                conn,
                principal,
                provider,
                Permission::ProviderManage,
                policy,
            )
            .await?;
            let Some(node) = node.filter(|_| held.is_allowed()) else {
                return Ok(held);
            };
            // Naming a node as well needs sight of it.
            Ok(Verdict::of(
                nodes::check_named(
                    conn,
                    principal,
                    node,
                    PermissionName::OfNode(NodePermission::View),
                    Act::Use,
                    policy,
                )
                .await?,
            ))
        }
        Home::Node(node) => {
            node_verdict(conn, principal, node, Permission::UsageManage, policy).await
        }
        Home::System => Ok(system(principal)),
    }
}

/// Whether the person may read the caps defined in `home`, and their use.
pub async fn check_view(
    conn: &mut Conn,
    principal: &Principal,
    home: Home,
    policy: Policy,
) -> Result<Verdict, Error> {
    match home {
        Home::Provider { provider, .. } => {
            provider_verdict(conn, principal, provider, Permission::ProviderView, policy).await
        }
        Home::Node(node) => {
            node_verdict(conn, principal, node, Permission::UsageView, policy).await
        }
        Home::System => Ok(system(principal)),
    }
}

/// Whether the person may let a turn past a cap defined in `home` that is
/// reached: by the row where the cap is defined, which the managers of the place
/// hold as well. What they lack is in the verdict.
pub async fn check_pass(
    conn: &mut Conn,
    principal: &Principal,
    home: Home,
    policy: Policy,
) -> Result<Verdict, Error> {
    match home {
        Home::Provider { provider, .. } => {
            provider_verdict(
                conn,
                principal,
                provider,
                Permission::ProviderBypass,
                policy,
            )
            .await
        }
        Home::Node(node) => {
            node_verdict(conn, principal, node, Permission::UsageBypass, policy).await
        }
        Home::System => Ok(system(principal)),
    }
}

/// Whether the person may let a turn past a cap defined in `home`. A person
/// who is suspended, or for whom the place is archived or gone, may not.
pub async fn may_pass(
    conn: &mut Conn,
    principal: &Principal,
    home: Home,
    policy: Policy,
) -> Result<bool, Error> {
    match check_pass(conn, principal, home, policy).await {
        Ok(verdict) => Ok(verdict.is_allowed()),
        Err(Error::Suspended { .. } | Error::Archived { .. } | Error::NotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

fn system(principal: &Principal) -> Verdict {
    if principal.is_system_admin() {
        Verdict::Allowed
    } else {
        Verdict::SystemAdminOnly
    }
}

/// How much of the use of a place a person may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Depth {
    /// Their own use there.
    Own,
    /// The figures of the place, without anybody else's records.
    Summary,
    /// Everybody's use there, and every record.
    Reports,
}

/// How deep the person may read the use of the node: everything with
/// `usage.view`, the figures with `usage.summary`, their own with sight of the
/// node; none for a person who does not see the node.
pub async fn depth_at(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    policy: Policy,
) -> Result<Option<Depth>, Error> {
    if node_verdict(conn, principal, node, Permission::UsageView, policy)
        .await?
        .is_allowed()
    {
        return Ok(Some(Depth::Reports));
    }
    if node_verdict(conn, principal, node, Permission::UsageSummary, policy)
        .await?
        .is_allowed()
    {
        return Ok(Some(Depth::Summary));
    }
    let sees = nodes::check_named(
        conn,
        principal,
        node,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        policy,
    )
    .await?;
    Ok(sees.is_allowed().then_some(Depth::Own))
}

/// The role the person holds at the node, which a cap for a role covers: the
/// highest they hold on the way down to it. None where they hold none.
pub async fn role_at(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
) -> Result<Option<Role>, Error> {
    let graph = Graph::load(conn, &[node]).await?;
    let Some(site) = graph.site_of(node) else {
        return Ok(None);
    };
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[],
    )
    .await?;
    Ok(frame.role_for(&site).map(|held| held.role))
}

/// The people who may change a cap defined in `home`, who are told when it
/// warns and when it is reached: the holders of `usage.manage` on its node, of
/// `provider.manage` on its provider, or the system's admins.
pub async fn managers(conn: &mut Conn, home: Home, now: Time) -> Result<Vec<Id>, Error> {
    match home {
        Home::System => Ok(access::system_admin_people(conn, MOST_ADMINS).await?),
        Home::Node(node) => {
            let Some(owner) = orch_db::usage::caps::node_owner(conn, node).await? else {
                return Ok(Vec::new());
            };
            let principal = Principal::load(conn, crate::Caller::session(owner), now).await?;
            let graph = Graph::load(conn, &[node]).await?;
            let Some(site) = graph.site_of(node) else {
                return Ok(Vec::new());
            };
            let frame = Frame::load(
                conn,
                &principal,
                Policy::default(),
                graph,
                std::slice::from_ref(&site),
                &[Permission::UsageManage],
            )
            .await?;
            holders::who_holds(conn, &frame, &site, Permission::UsageManage).await
        }
        Home::Provider { provider, .. } => {
            let Some(owner) = orch_db::usage::caps::provider_owner(conn, provider).await? else {
                return Ok(Vec::new());
            };
            let principal = Principal::load(conn, crate::Caller::session(owner), now).await?;
            let (graph, _, site) = resources::site(conn, provider_object(provider)).await?;
            let frame = Frame::load(
                conn,
                &principal,
                Policy::default(),
                graph,
                std::slice::from_ref(&site),
                &[Permission::ProviderManage],
            )
            .await?;
            holders::who_holds(conn, &frame, &site, Permission::ProviderManage).await
        }
    }
}
