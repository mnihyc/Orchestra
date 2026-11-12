//! Path locations use the same native operands and current result Read as volume locations.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
async fn key(world: &World, owner: Id, rules: Vec<serde_json::Value>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: orch_types::new_id(),
            user_id: owner,
            name: "Native mount location",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    token
}
fn rule(kind: &str, id: Id, actions: &[&str]) -> serde_json::Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":"MountLocationSet","actions":actions}]})
}
#[tokio::test]
async fn location_set_requires_both_typed_roots_and_reads_completed_inventory_only_with_its_own_read()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.boss, c.acme, "scope-path")
        .await
        .base
        .id;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut *world.conn().await,
        c.boss.id(),
        "source-machine",
    )
    .await
    .unwrap();
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv".into()],
        ..Default::default()
    };
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(machine)
        .bind(json!(policy))
        .execute(&world.db.pool)
        .await
        .unwrap();
    for rules in [
        vec![rule("mount", mount, &["write"])],
        vec![rule("machine", machine, &["manage"])],
        vec![
            rule("mount", mount, &["read"]),
            rule("machine", machine, &["manage"]),
        ],
    ] {
        let token = key(&world, c.boss.id(), rules).await;
        let request = world
            .app
            .call(MountLocationSet, &[&mount, &machine])
            .token(&token)
            .body(&MountLocationSetBody::Path {
                source: "/srv/private".into(),
            });
        assert_eq!(
            request.send_bytes().await.status,
            http::StatusCode::FORBIDDEN
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM mount_locations WHERE mount_id=$1")
                .bind(mount)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
    let token = key(
        &world,
        c.boss.id(),
        vec![
            rule("mount", mount, &["write"]),
            rule("machine", machine, &["manage"]),
        ],
    )
    .await;
    let request = world
        .app
        .call(MountLocationSet, &[&mount, &machine])
        .token(&token)
        .body(&MountLocationSetBody::Path {
            source: "/srv/private".into(),
        });
    let reply = request.send_bytes().await;
    assert_eq!(
        reply.status,
        http::StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&reply.raw).unwrap(),
        json!({"projection":"receipt","mount_id":mount,"machine_id":machine})
    );
    let token = key(
        &world,
        c.boss.id(),
        vec![
            rule("mount", mount, &["write", "read"]),
            rule("machine", machine, &["manage", "read"]),
        ],
    )
    .await;
    let request = world
        .app
        .call(MountLocationSet, &[&mount, &machine])
        .token(&token)
        .body(&MountLocationSetBody::Path {
            source: "/srv/private".into(),
        });
    let reply = request
        .send()
        .await
        .ok()
        .into_full()
        .expect("the completed current reader gets its native inventory");
    assert_eq!(reply.base.id, mount);
    assert_eq!(reply.locations.len(), 1);
}

#[tokio::test]
async fn location_delete_requires_its_stored_machine_and_keeps_the_other_location_unchanged() {
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.boss, c.acme, "delete-path")
        .await
        .base
        .id;
    let mut machines = Vec::new();
    for name in ["selected-machine", "unrelated-machine"] {
        let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
            &mut *world.conn().await,
            c.boss.id(),
            name,
        )
        .await
        .unwrap();
        let policy = orch_link_contract::policy::LocalPolicy {
            environment_roots: vec!["/srv".into()],
            ..Default::default()
        };
        sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
            .bind(machine)
            .bind(json!(policy))
            .execute(&world.db.pool)
            .await
            .unwrap();
        let request = world
            .app
            .call(MountLocationSet, &[&mount, &machine])
            .token(&c.boss.token)
            .body(&MountLocationSetBody::Path {
                source: format!("/srv/{name}"),
            });
        request.send().await.ok().into_full().unwrap();
        machines.push(machine);
    }
    let selected = machines[0];
    let other = machines[1];
    for rules in [
        vec![rule("mount", mount, &["write"])],
        vec![rule("machine", selected, &["manage"])],
        vec![
            rule("mount", mount, &["write"]),
            rule("machine", other, &["manage"]),
        ],
    ] {
        let rules = rules
            .into_iter()
            .map(|mut rule| {
                rule["allow"][0]["operation"] = json!("MountLocationDelete");
                rule
            })
            .collect();
        let token = key(&world, c.boss.id(), rules).await;
        let request = world
            .app
            .call(MountLocationDelete, &[&mount, &selected])
            .token(&token)
            .query(&MountLocationDeleteQuery { confirm: None });
        assert_eq!(
            request.send_bytes().await.status,
            http::StatusCode::FORBIDDEN
        );
    }
    let rules = vec![
        rule("mount", mount, &["write"]),
        rule("machine", selected, &["manage"]),
    ]
    .into_iter()
    .map(|mut rule| {
        rule["allow"][0]["operation"] = json!("MountLocationDelete");
        rule
    })
    .collect();
    let token = key(&world, c.boss.id(), rules).await;
    let request = world
        .app
        .call(MountLocationDelete, &[&mount, &selected])
        .token(&token)
        .query(&MountLocationDeleteQuery { confirm: None });
    assert_eq!(
        request.send_bytes().await.status,
        http::StatusCode::NO_CONTENT
    );
    let current =
        orch_db::queries::resources::mounts::locations(&mut *world.conn().await, &[mount])
            .await
            .unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].machine_id, other);
    assert_eq!(current[0].source, "/srv/unrelated-machine");
}
