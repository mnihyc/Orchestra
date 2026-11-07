//! The account's placement grants inside one already-admitted ProviderCreate.
//!
//! Host/Template Use is an internal dependency of creating this provider, not another
//! token endpoint. The original provider/home credential gate remains mandatory. This
//! never constructs another Caller, changes its grant, or exposes an ignore-scopes API.
use super::*;
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::WhenArchived;

async fn admitted_home(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
) -> Result<bool, Error> {
    let caller = principal.caller();
    if caller.admitted_operation() != Some("ProviderCreate") {
        return Ok(false);
    }
    let object = ObjectRef {
        kind: ObjectKind::Provider,
        id: provider,
    };
    let (_, resource, site) = match site(conn, object).await {
        Ok(current) => current,
        Err(Error::NotFound(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    if resource.deleted || resource.disabled {
        return Ok(false);
    }
    let Some(home) = site.role_place else {
        return Ok(false);
    };
    let mut homes = caller
        .admitted_facets()
        .unwrap_or_default()
        .iter()
        .filter(|facet| {
            facet.operation == "ProviderCreate"
                && facet.action == KeyAction::Manage
                && facet.resource_kind == KeyTargetKind::Object(ObjectKind::Provider)
                && facet.target.object.kind == KeyTargetKind::Object(ObjectKind::Org)
                && facet.target.object.id == Some(home)
        });
    if homes.next().is_none() || homes.next().is_some() {
        return Ok(false);
    }
    Ok(needs(
        conn,
        principal,
        &Needs {
            required: Permission::ProviderManage,
            anchor: object,
            through: Some(home),
            items: vec![Need {
                object,
                permissions: vec![Permission::ProviderManage],
                options: None,
            }],
        },
        Policy::default(),
    )
    .await?
    .is_allowed())
}

fn account_has(frame: &Frame<'_>, site: &Site, permission: Permission) -> bool {
    // Resource sites retain the existing archive semantics; do not inherit a new
    // archive denial from the resource's home. The full frame retains holder/ACL
    // locks, expiry, outside membership and suspension/second-step conditions.
    (!site.is_archived() || permission.when_archived() != WhenArchived::Withheld)
        && !frame.account_ways(site, permission).is_empty()
}

async fn dependency(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    object: ObjectRef,
    permission: Permission,
) -> Result<bool, Error> {
    if !admitted_home(conn, principal, provider).await? {
        return Ok(false);
    }
    let (graph, resource, site) = match site(conn, object).await {
        Ok(current) => current,
        Err(Error::NotFound(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    if resource.deleted || resource.disabled {
        return Ok(false);
    }
    let mut wanted = vec![permission];
    wanted.extend(permission.held_through());
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &wanted,
    )
    .await?;
    Ok(account_has(&frame, &site, permission))
}

/// The current account's exact selected host Use, solely within this ProviderCreate.
pub async fn provider_creation_host_use(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    host: Id,
) -> Result<bool, Error> {
    dependency(
        conn,
        principal,
        provider,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
        Permission::HostUse,
    )
    .await
}

/// The current account's exact selected template Use, solely within this ProviderCreate.
pub async fn provider_creation_template_use(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    template: Id,
) -> Result<bool, Error> {
    dependency(
        conn,
        principal,
        provider,
        ObjectRef {
            kind: ObjectKind::Template,
            id: template,
        },
        Permission::TemplateUse,
    )
    .await
}

/// The same first page the ordinary owner-based chooser used, before its HostUse filter.
/// Page membership remains ordinary account visibility in identifier order; only IDs
/// with HostUse survive. Physical/template/driver/privacy checks stay with the chooser.
pub async fn provider_creation_hosts(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    limit: usize,
) -> Result<Vec<Id>, Error> {
    if !admitted_home(conn, principal, provider).await? {
        return Ok(Vec::new());
    }
    let mut cursor = None;
    let mut visible = 0usize;
    let mut hosts = Vec::new();
    while visible < limit {
        let batch = Batch::read(
            conn,
            principal,
            ResourceKind::Host,
            cursor,
            Policy::default(),
        )
        .await?;
        for (resource, site) in &batch.resources {
            let seen = ObjectKind::Host
                .permissions()
                .filter(|permission| *permission != Permission::AccessOwn)
                .any(|permission| account_has(&batch.frame, site, permission));
            if !seen {
                continue;
            }
            visible += 1;
            if account_has(&batch.frame, site, Permission::HostUse) {
                hosts.push(resource.id);
            }
            if visible == limit {
                break;
            }
        }
        cursor = batch.last;
        if batch.exhausted || batch.last.is_none() {
            break;
        }
    }
    Ok(hosts)
}
