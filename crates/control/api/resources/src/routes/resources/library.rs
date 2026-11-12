//! Resource pages read the same batched library figures as inventory rows, after ordinary View checks.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Registry;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::library as api;

use orch_engine::resource_facts;

use std::collections::HashSet;
/// Binds the ordinary resource-page library read.
pub fn mount(registry: &mut Registry) {
    registry.on_snapshot_body_read_with_optional_operation(
        api::ResourceLibraryRead,
        crate::key_operations::native::selected_body::<
            super::figures::Library,
            (),
            api::LibraryReadBody,
        >,
        crate::key_operations::native::selected_body_read::<
            super::figures::Library,
            (),
            api::LibraryReadBody,
        >,
    );
    registry.on_snapshot_body_read_with_optional_operation(
        api::ResourceLibraryPeopleRead,
        crate::key_operations::native::selected_body::<people::People, (), api::LibraryPeopleBody>,
        people::get,
    );
}

/// An environment owns the authority of its own tool definitions; presets retain their own View.
pub(crate) async fn readable(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    objects: &[ObjectRef],
) -> ApiResult<HashSet<ObjectRef>> {
    let ids: Vec<_> = objects.iter().map(|o| o.id).collect();
    let mut allowed: HashSet<_> = resource_facts::authorities(conn, principal, &ids)
        .await
        .map_err(crate::key_operations::snapshot_read::engine_error)?
        .into_iter()
        .filter(|row| row.readable)
        .map(|row| row.resource.object())
        .collect();
    let own:Vec<(orch_types::Id,orch_types::Id)>=sqlx::query_as("SELECT id,environment_id FROM mcp_servers WHERE id=ANY($1) AND environment_id IS NOT NULL AND deleted_at IS NULL").bind(&ids).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let environments: Vec<_> = own.iter().map(|(_, env)| *env).collect();
    if !environments.is_empty() {
        let visible: HashSet<_> = orch_authz::resources::holding(
            conn,
            principal,
            &environments,
            orch_access_contract::permissions::Permission::EnvironmentView,
            Default::default(),
        )
        .await
        .map_err(crate::middleware::authorization::restoration_error)?
        .into_iter()
        .map(|env| env.id)
        .collect();
        for (id, env) in own {
            if visible.contains(&env) {
                allowed.insert(ObjectRef {
                    kind: ObjectKind::McpServer,
                    id,
                });
            }
        }
    }
    Ok(allowed)
}
orch_api_base::__api_bind_eead74b020!();

/// Actual immutable version ownership, shared by batched figures and the people window.
pub(super) async fn version_memberships(
    conn: &mut orch_db::Conn,
    ids: &[orch_types::Id],
) -> ApiResult<HashSet<(orch_types::Id, orch_types::Id)>> {
    Ok(
        sqlx::query_as("SELECT id,skill_id FROM skill_versions WHERE id=ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await
            .map_err(orch_db::DbError::from)?
            .into_iter()
            .collect(),
    )
}

pub(super) fn matches_version(
    target: &api::LibraryTarget,
    memberships: &HashSet<(orch_types::Id, orch_types::Id)>,
) -> bool {
    target.version_id.is_none_or(|version| {
        target.object.kind == ObjectKind::Skill
            && memberships.contains(&(version, target.object.id))
    })
}

/// The original all-or-nothing resource/version door, shared by both library readers.
pub(super) async fn require_targets(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    targets: &[api::LibraryTarget],
) -> ApiResult<()> {
    let objects: Vec<_> = targets.iter().map(|target| target.object).collect();
    let allowed = readable(conn, principal, &objects).await?;
    let versions: Vec<_> = targets
        .iter()
        .filter_map(|target| target.version_id)
        .collect();
    let memberships = version_memberships(conn, &versions).await?;
    if targets
        .iter()
        .any(|target| !allowed.contains(&target.object) || !matches_version(target, &memberships))
    {
        return Err(ApiError::not_found());
    }
    Ok(())
}

#[path = "library/people.rs"]
pub mod people;

#[path = "library/subjects.rs"]
pub mod subjects;
