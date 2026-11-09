//! Ordinary provider moves use the provider owner's current destination rights.
use super::*;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_authz::{Caller, Policy, Principal};

async fn allowed(
    world: &World,
    user: Id,
    kind: ObjectKind,
    id: Id,
    permission: Permission,
) -> bool {
    let mut conn = world.conn().await;
    let principal = Principal::load(
        &mut conn,
        Caller::session(user).with_admin_mode(false),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    orch_authz::resources::check(
        &mut conn,
        &principal,
        ObjectRef { kind, id },
        permission,
        Policy::default(),
    )
    .await
    .unwrap()
    .is_allowed()
}

async fn preview(world: &World, team: &Team) -> serde_json::Value {
    let reply = world
        .app
        .call(provider_api::ProviderRuntimeUpdateBatchPreflight, &[])
        .token(&team.c.boss.token)
        .body(&provider_api::ProviderRuntimeUpdateBatchPreflightBody {
            provider_ids: vec![team.provider],
        })
        .send_bytes()
        .await;
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn a_provider_manager_cannot_move_its_runtime_where_only_the_manager_has_host_use() {
    let world = World::new().await;
    let team = team(&world).await;
    let owner = team.c.admin.id();
    let manager = team.c.boss.id();
    assert_ne!(owner, manager);
    resources::name_person(
        &world,
        "provider",
        team.provider,
        "provider.manage",
        manager,
    )
    .await;
    let template: Id = sqlx::query_scalar("SELECT template_id FROM runtimes WHERE provider_id=$1")
        .bind(team.provider)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    for user in [owner, manager] {
        resources::name_person(&world, "template", template, "template.use", user).await;
        assert!(
            allowed(
                &world,
                user,
                ObjectKind::Template,
                template,
                Permission::TemplateUse
            )
            .await
        );
    }
    assert!(
        allowed(
            &world,
            manager,
            ObjectKind::Provider,
            team.provider,
            Permission::ProviderManage
        )
        .await
    );
    let mut conn = world.conn().await;
    let destination = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(manager),
        Some(team.c.acme),
        "Manager's private host",
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Host,
            id: destination,
        },
        manager,
        orch_authz::ownership::NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(destination)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    assert!(
        allowed(
            &world,
            manager,
            ObjectKind::Host,
            destination,
            Permission::HostUse
        )
        .await
    );
    assert!(
        !allowed(
            &world,
            owner,
            ObjectKind::Host,
            destination,
            Permission::HostUse
        )
        .await
    );

    // Desired values are a whole-map ProviderUpdate, using its current revision. A stored
    // preference is not proof that its owner currently may use the selected host.
    let (mut config, revision): (serde_json::Value, i64) =
        sqlx::query_as("SELECT runtime_config,revision FROM providers WHERE id=$1")
            .bind(team.provider)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    config
        .as_object_mut()
        .unwrap()
        .insert("runtime.host_id".into(), json!(destination));
    let reply = world
        .app
        .call(provider_api::ProviderUpdate, &[&team.provider])
        .token(&team.c.boss.token)
        .body(&provider_api::ProviderUpdateBody {
            service_offerings: None,
            model_group_options: None,
            name: None,
            upstreams: None,
            config: None,
            arrangements: None,
            runtime_config: Some(config),
            expected_revision: revision,
        })
        .send_bytes()
        .await;
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );

    // Prove that the same destination and template are executable when the owner has
    // Use, before removing exactly that grant for the causal negative control.
    let grant = resources::name_person(&world, "host", destination, "host.use", owner).await;
    assert!(
        allowed(
            &world,
            owner,
            ObjectKind::Host,
            destination,
            Permission::HostUse
        )
        .await
    );
    let permitted = preview(&world, &team).await;
    assert_eq!(permitted["executable"], 1, "{permitted}");
    assert!(
        orch_db::queries::access::remove_entry(
            &mut *world.conn().await,
            grant,
            manager,
            world.now(),
        )
        .await
        .unwrap()
    );
    assert!(
        !allowed(
            &world,
            owner,
            ObjectKind::Host,
            destination,
            Permission::HostUse
        )
        .await
    );
    assert!(
        allowed(
            &world,
            manager,
            ObjectKind::Host,
            destination,
            Permission::HostUse
        )
        .await
    );
    let refused = preview(&world, &team).await;
    assert_eq!(
        refused["executable"], 0,
        "the provider owner's current HostUse is required even when its manager may use the destination: {refused}"
    );
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='provider_recreate'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(queued, 0, "previews never start runtime work");
}

#[tokio::test]
async fn a_provider_manager_can_preview_a_move_when_only_its_owner_has_destination_host_use() {
    let world = World::new().await;
    let team = team(&world).await;
    let owner = team.c.admin.id();
    let manager = team.c.boss.id();
    // The existing company fixture makes this third person an active org member.
    // Starting their host as OnlyMe avoids lending either A or B implicit host rights.
    let host_owner = team.c.viewer.id();
    assert_ne!(owner, manager);
    assert_ne!(owner, host_owner);
    assert_ne!(manager, host_owner);
    resources::name_person(
        &world,
        "provider",
        team.provider,
        "provider.manage",
        manager,
    )
    .await;
    let template: Id = sqlx::query_scalar("SELECT template_id FROM runtimes WHERE provider_id=$1")
        .bind(team.provider)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    for user in [owner, manager] {
        resources::name_person(&world, "template", template, "template.use", user).await;
        assert!(
            allowed(
                &world,
                user,
                ObjectKind::Template,
                template,
                Permission::TemplateUse
            )
            .await
        );
    }
    assert!(
        allowed(
            &world,
            manager,
            ObjectKind::Provider,
            team.provider,
            Permission::ProviderManage
        )
        .await
    );
    let mut conn = world.conn().await;
    let destination = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(host_owner),
        Some(team.c.acme),
        "Third member's private host",
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Host,
            id: destination,
        },
        host_owner,
        orch_authz::ownership::NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(destination)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    resources::name_person(&world, "host", destination, "host.use", owner).await;
    for (user, permission, expected) in [
        (host_owner, Permission::HostManage, true),
        (owner, Permission::HostUse, true),
        (owner, Permission::HostManage, false),
        (manager, Permission::HostUse, false),
        (manager, Permission::HostManage, false),
    ] {
        assert_eq!(
            allowed(&world, user, ObjectKind::Host, destination, permission).await,
            expected,
            "destination rights for {user}, {permission:?}"
        );
    }

    // Store the selected destination through the normal versioned desired-runtime writer.
    let (mut config, revision): (serde_json::Value, i64) =
        sqlx::query_as("SELECT runtime_config,revision FROM providers WHERE id=$1")
            .bind(team.provider)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    config
        .as_object_mut()
        .unwrap()
        .insert("runtime.host_id".into(), json!(destination));
    let reply = world
        .app
        .call(provider_api::ProviderUpdate, &[&team.provider])
        .token(&team.c.boss.token)
        .body(&provider_api::ProviderUpdateBody {
            service_offerings: None,
            model_group_options: None,
            name: None,
            upstreams: None,
            config: None,
            arrangements: None,
            runtime_config: Some(config),
            expected_revision: revision,
        })
        .send_bytes()
        .await;
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let permitted = preview(&world, &team).await;
    assert_eq!(
        permitted["executable"], 1,
        "the owner's current HostUse suffices without giving its manager HostUse or HostManage: {permitted}"
    );
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='provider_recreate'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(queued, 0, "previews never start runtime work");
}
