//! Current Ask root governance shared by the HTTP entry and retained model commands.
use crate::EngineError;
use orch_access_contract::{decisions::Decision, permissions::Permission};
use orch_authz::{Act, Policy, Principal};
use orch_db::{Conn, queries::hierarchy};
use orch_settings::{Place, get as setting};
use orch_types::Id;

/// Ordinary root access, without projecting an administrator inspection.
pub enum RootAccess {
    /// The exact root no longer exists.
    Missing,
    /// The current person cannot enter this knowledge boundary.
    Refused,
    /// Current membership or ordinary root View permits this boundary.
    Allowed,
}

/// The same sharing policy used by the ordinary node gate.
pub async fn page_policy(conn: &mut Conn, node: Id) -> Result<Policy, EngineError> {
    Ok(Policy {
        owners_name_outsiders: setting::<bool>(conn, Place::Node(node), "sharing.allow_external")
            .await?
            .unwrap_or(true),
        holders_share: setting::<bool>(conn, Place::Node(node), "sharing.allow_holders")
            .await?
            .unwrap_or(true),
    })
}

/// Membership in a department admits its root boundary, never unreadable source contents.
pub async fn root_access(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
) -> Result<RootAccess, EngineError> {
    let Some(node) = hierarchy::node(conn, root)
        .await?
        .filter(|node| node.kind == "org" && node.org_id == root)
    else {
        return Ok(RootAccess::Missing);
    };
    if principal.memberships().is_in_tree(root)
        && !principal.suspension().applies(&[root])
        && principal.caller().permits(Permission::OrgView)
        && principal.caller().reaches(Some(&node.path))
    {
        return Ok(RootAccess::Allowed);
    }
    let policy = page_policy(conn, root).await?;
    Ok(
        match orch_authz::nodes::check(conn, principal, root, Permission::OrgView, Act::Use, policy)
            .await?
        {
            Decision::Allowed { .. } => RootAccess::Allowed,
            _ => RootAccess::Refused,
        },
    )
}
