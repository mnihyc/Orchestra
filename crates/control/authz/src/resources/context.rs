//! Read a resource's grant to work in one real place, without borrowing a person's global rights.
use super::*;
use crate::options;
use orch_access_contract::pages::Origin;
use orch_access_contract::permissions::{NodePermission, Options, PermissionName};

/// Native selected share facts. This is not serializable as permission or a public full resource.
#[derive(Debug, Clone, PartialEq)]
pub struct Grant {
    /// Actual resource identity and home, never relabeled as the selected context.
    pub resource: Resource,
    /// The actual work permission on that resource family.
    pub permission: Permission,
    /// Only the selected work entries' combined native options.
    pub options: Options,
    /// Exact current row suppliers, for a retained projection's association check.
    pub sources: Vec<(Id, Origin)>,
}

/// Current native place View and the same resource entry matcher used by thread binding.
/// Personal, group, role, owner and administrator rights cannot raise this projection.
/// A caller's global Manage never removes the selected work entries' options.
pub async fn among(
    conn: &mut Conn,
    principal: &Principal,
    context: Id,
    ids: &[Id],
    policy: Policy,
) -> Result<Vec<Grant>, Error> {
    let decision = crate::nodes::check_named(
        conn,
        principal,
        context,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        policy,
    )
    .await?;
    if !decision.is_allowed() {
        return Err(Error::NotFound(context));
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut graph = Graph::load(conn, &[context]).await?;
    let path = graph
        .get(context)
        .ok_or(Error::NotFound(context))?
        .path
        .clone();
    let resources: Vec<_> = read_into(conn, &mut graph, ids)
        .await?
        .into_iter()
        .filter(|resource| {
            !resource.deleted
                && matches!(resource.kind, ResourceKind::Provider | ResourceKind::Agent)
        })
        .collect();
    if resources.is_empty() {
        return Ok(Vec::new());
    }
    let sites: Vec<_> = resources
        .iter()
        .filter_map(|resource| site_of(&graph, resource))
        .collect();
    let mut permissions = Vec::new();
    for resource in &resources {
        if let Some(permission) = use_permission(resource.kind)
            && !permissions.contains(&permission)
        {
            permissions.push(permission);
        }
    }
    let frame = Frame::load(conn, principal, policy, graph, &sites, &permissions).await?;
    let mut grants = Vec::new();
    for resource in resources {
        if resource.deleted
            || !matches!(resource.kind, ResourceKind::Provider | ResourceKind::Agent)
        {
            continue;
        }
        let Some(permission) = use_permission(resource.kind) else {
            continue;
        };
        let Some(mut site) = sites.iter().find(|site| site.id == resource.id).cloned() else {
            continue;
        };
        // Match ordinary credential reach on the genuine selected work, as usable::access does.
        // The resource's own home, inheritance and external-sharing bounds stay unchanged.
        site.path = Some(path.clone());
        let view = match resource.kind {
            ResourceKind::Provider => Permission::ProviderView,
            ResourceKind::Agent => Permission::AgentView,
            _ => continue,
        };
        if !principal.caller().permits(view)
            || !principal.caller().reaches(Some(&path))
            || principal.suspension().applies(&site.orgs)
        {
            continue;
        }
        let matched: Vec<_> = frame
            .matching(&site, permission, Some(&path))
            .into_iter()
            .filter(|named| named.entry.holder.is_work_on(&path))
            .collect();
        if matched.is_empty() {
            continue;
        }
        let options = options::combine(permission, matched.iter().map(|named| named.entry))?
            .unwrap_or_else(|| options::unlimited(permission));
        let mut sources: Vec<_> = matched
            .iter()
            .map(|named| (named.entry.id, named.from))
            .collect();
        sources.sort_by_key(|source| source.0);
        sources.dedup();
        grants.push(Grant {
            resource,
            permission,
            options,
            sources,
        });
    }
    Ok(grants)
}
