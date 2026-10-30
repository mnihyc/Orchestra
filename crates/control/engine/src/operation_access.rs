//! Loss and relocation check ordinary authority and make reviewed administrative self-grants through permission pages.

use crate::{EngineError, updates::UpdateError};
use orch_access_contract::{decisions::ObjectRef, permissions::Permission};
use orch_api_contract::loss::OperationAdminGrant;
use orch_authz::{Policy, Principal, engine::Frame};
use orch_db::Conn;

/// Ordinary access, an eligible explicit administrative bypass, or refusal. Tokens gain no bypass.
pub async fn review(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    permission: Permission,
) -> Result<Option<OperationAdminGrant>, UpdateError> {
    let (graph, resource, site) = orch_authz::resources::site(conn, object)
        .await
        .map_err(EngineError::from)?;
    let mut asked = vec![permission, Permission::AccessManage];
    asked.extend(permission.held_through());
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &asked,
    )
    .await
    .map_err(EngineError::from)?;
    if frame.ordinary_holds(&site, permission) {
        return Ok(None);
    }
    let allowed = principal.caller().as_admin()
        && principal.caller().grant().is_none()
        && principal.caller().permits(permission)
        && frame.administers(&site)
        && orch_authz::admins::closed_to_everybody(&frame, &site, permission).is_none()
        && !frame.ways(&site, Permission::AccessManage).is_empty();
    if !allowed {
        return Err(UpdateError::Denied);
    }
    Ok(Some(OperationAdminGrant {
        object,
        permission,
        name: resource.name,
    }))
}

/// Confirming a reviewed bypass names the actor on each actual resource page, atomically with the act.
pub async fn accept(
    conn: &mut Conn,
    principal: &Principal,
    grants: &[OperationAdminGrant],
    origin: &orch_db::audit::Origin,
) -> Result<(), UpdateError> {
    use orch_access_contract::pages::{EntryAdd, Holder, PageChange, RowChange};
    for grant in grants {
        if review(conn, principal, grant.object, grant.permission)
            .await?
            .is_none()
        {
            continue;
        }
        let (_, resource, _) = orch_authz::resources::site(conn, grant.object)
            .await
            .map_err(EngineError::from)?;
        let place = resource
            .home
            .map_or(orch_settings::Place::System, orch_settings::Place::Node);
        let external = orch_settings::get::<bool>(
            conn,
            place,
            orch_settings_contract::registry::keys::SHARING_ALLOW_EXTERNAL,
        )
        .await
        .map_err(EngineError::from)?
        .unwrap_or(true);
        let policy = orch_authz::usable::ResourcePolicy {
            owners_name_outsiders: external,
            shared_subscriptions: None,
        };
        let change = PageChange {
            rows: Some(vec![RowChange {
                permission: grant.permission.as_str().into(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![EntryAdd {
                    holder: Holder::User {
                        id: principal.user(),
                    },
                    options: None,
                    expires_at: None,
                    note: Some("Administrative operation".into()),
                }]),
                update: None,
                remove: None,
            }]),
            ..Default::default()
        };
        orch_authz::pages::change_resource(conn, principal, grant.object, &change, &policy, origin)
            .await
            .map_err(|error| match error {
                orch_authz::pages::ChangeError::Denied(_) => UpdateError::Denied,
                other => UpdateError::Internal(EngineError::unreadable(other)),
            })?;
    }
    Ok(())
}

/// Applies prospective page grants only inside a preview transaction which its caller must roll back.
/// This lets the ordinary scheduler measure the same host/template access that confirmation will grant.
pub async fn preview_recovery(
    conn: &mut Conn,
    principal: &Principal,
    environment: orch_types::Id,
    named: Option<orch_types::Id>,
) -> Result<Vec<OperationAdminGrant>, UpdateError> {
    use orch_access_contract::permissions::ObjectKind;
    let mut grants = Vec::new();
    if let Some(grant) = review(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        Permission::EnvironmentManage,
    )
    .await?
    {
        grants.push(grant);
    }
    let state = orch_db::queries::environments::hosting::environment(conn, environment)
        .await?
        .ok_or(UpdateError::NotFound)?;
    let template = match state.template_id {
        Some(id) => Some(id),
        None => orch_settings::get::<orch_types::Id>(
            conn,
            orch_settings::Place::Environment(environment),
            orch_settings_contract::registry::keys::ENV_TEMPLATE_ID,
        )
        .await
        .map_err(EngineError::from)?,
    };
    if let Some(id) = template {
        match review(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Template,
                id,
            },
            Permission::TemplateUse,
        )
        .await
        {
            Ok(Some(grant)) => grants.push(grant),
            Ok(None) | Err(UpdateError::Denied) => {}
            Err(error) => return Err(error),
        }
    }
    for host in
        orch_db::queries::resources::hosting::hosts(conn, named.as_ref().map(std::slice::from_ref))
            .await?
    {
        match review(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host.id,
            },
            Permission::HostUse,
        )
        .await
        {
            Ok(Some(grant)) => grants.push(grant),
            Ok(None) | Err(UpdateError::Denied) => {}
            Err(error) => return Err(error),
        }
    }
    accept(conn, principal, &grants, &orch_db::audit::Origin::default()).await?;
    Ok(grants)
}

/// Ownership-only lifecycle acts use the same enabled-session, page and lock limits as a permission bypass.
pub async fn can_bypass(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    permission: Permission,
) -> Result<bool, EngineError> {
    if !principal.caller().as_admin() || principal.caller().grant().is_some() {
        return Ok(false);
    }
    let (graph, _, site) = orch_authz::resources::site(conn, object).await?;
    let mut asked = vec![permission, Permission::AccessManage];
    asked.extend(permission.held_through());
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &asked,
    )
    .await?;
    Ok(frame.administers(&site)
        && principal.caller().permits(permission)
        && !frame.ways(&site, Permission::AccessManage).is_empty()
        && orch_authz::admins::closed_to_everybody(&frame, &site, permission).is_none())
}
