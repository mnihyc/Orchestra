//! Page managers may reduce another person's entry without giving a permission they do not hold.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission;
use orch_authz::pages::{ChangeError, Target};
use serde_json::json;
use support::{World, user_holder};

fn missing(error: ChangeError, permission: Permission) {
    match error {
        ChangeError::Denied(refusal) => assert!(
            refusal
                .missing
                .iter()
                .any(|lack| lack.permissions.contains(&permission))
        ),
        other => panic!("expected a permission refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_manager_without_the_permission_shortens_anothers_entry_but_cannot_lengthen_it() {
    let world = World::new().await;
    let manager = world.member("manager").await;
    let recipient = world.member("recipient").await;
    let target = Target::Live(world.thread);
    world
        .name(
            world.thread,
            Permission::AccessManage,
            Holder::User { id: manager },
        )
        .await;
    assert!(
        !world
            .can_at(manager, world.thread, Permission::ThreadManage, world.now)
            .await
    );
    world
        .change(
            world.maker,
            target,
            json!({"rows": [{"permission": "thread.manage", "add": [{
                "holder": user_holder(recipient), "expires_at": world.later(Duration::hours(2))
            }]}]}),
        )
        .await
        .unwrap();
    let page = world.read(world.maker, target).await;
    let entry = page
        .rows
        .iter()
        .find(|row| row.permission == Permission::ThreadManage)
        .unwrap()
        .entries[0]
        .id;
    world
        .change(
            manager,
            target,
            json!({"rows": [{"permission": "thread.manage", "update": [{
                "entry_id": entry, "expires_at": world.later(Duration::hours(1))
            }]}]}),
        )
        .await
        .unwrap();
    assert!(
        world
            .can_at(
                recipient,
                world.thread,
                Permission::ThreadManage,
                world.later(Duration::minutes(30))
            )
            .await
    );
    assert!(
        !world
            .can_at(
                recipient,
                world.thread,
                Permission::ThreadManage,
                world.later(Duration::minutes(90))
            )
            .await
    );
    let epoch = world.epoch().await;
    let error = world
        .change(
            manager,
            target,
            json!({"rows": [{"permission": "thread.manage", "update": [{
                "entry_id": entry, "expires_at": world.later(Duration::hours(2))
            }]}]}),
        )
        .await
        .unwrap_err();
    missing(error, Permission::ThreadManage);
    assert_eq!(world.epoch().await, epoch);
    let error = world
        .change(
            manager,
            target,
            json!({"rows": [{"permission": "thread.manage", "update": [{
                "entry_id": entry, "expires_at": null
            }]}]}),
        )
        .await
        .unwrap_err();
    missing(error, Permission::ThreadManage);
}

#[tokio::test]
async fn narrower_models_efforts_and_arrangements_need_management_but_a_swap_or_any_wider_member_needs_use()
 {
    let world = World::new().await;
    let manager = world.member("manager").await;
    let recipient = world.member("recipient").await;
    let id = world.provider(world.maker, world.root, "Shared key").await;
    world
        .grant(
            "provider",
            id,
            Permission::AccessManage,
            Holder::User { id: manager },
        )
        .await;
    let before = json!({"models": ["small", "large"], "efforts": ["low", "high"], "arrangements": ["separated", "key_separated"]});
    let narrowed = json!({"models": ["small"], "efforts": ["low"], "arrangements": ["separated"]});
    let entry = world
        .put(
            "provider",
            id,
            Permission::ProviderUse,
            Holder::User { id: recipient },
            before,
        )
        .await;
    world.change_thing(manager, "provider", id, json!({"rows": [{"permission": "provider.use", "update": [{
        "entry_id": entry, "options": narrowed, "expires_at": world.later(Duration::hours(1))
    }]}]})).await.unwrap();
    let page = world.read_thing(world.maker, "provider", id).await;
    assert_eq!(
        page.rows
            .iter()
            .find(|row| row.permission == Permission::ProviderUse)
            .unwrap()
            .entries[0]
            .options,
        narrowed
    );
    for options in [
        json!({"models": ["other"], "efforts": ["low"], "arrangements": ["separated"]}),
        json!({"models": ["small"], "efforts": ["low", "high"], "arrangements": ["separated"]}),
        json!({"models": ["small"], "efforts": ["low"], "arrangements": ["separated", "key_separated"]}),
        json!({}),
    ] {
        let epoch = world.epoch().await;
        let error = world
            .change_thing(
                manager,
                "provider",
                id,
                json!({"rows": [{"permission": "provider.use", "update": [{
                    "entry_id": entry, "options": options
                }]}]}),
            )
            .await
            .unwrap_err();
        missing(error, Permission::ProviderUse);
        assert_eq!(world.epoch().await, epoch);
    }
    let error = world.change_thing(manager, "provider", id, json!({"rows": [{"permission": "provider.use", "update": [{
        "entry_id": entry, "options": {"models": [], "efforts": ["low"], "arrangements": ["separated"]},
        "expires_at": world.later(Duration::hours(2))
    }]}]})).await.unwrap_err();
    missing(error, Permission::ProviderUse);
    world
        .change_thing(
            manager,
            "provider",
            id,
            json!({"rows": [{"permission": "provider.use", "remove": [entry]}]}),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn reducing_host_and_template_caps_is_management_and_removing_their_caps_is_giving() {
    let world = World::new().await;
    let manager = world.member("manager").await;
    let recipient = world.member("recipient").await;
    for (kind, permission, before, narrowed) in [
        (
            "host",
            Permission::HostUse,
            json!({"max_environments": 2, "storages": ["one", "two"], "resources_max": {"cpu_millis": 4000, "memory_mb": 2048}}),
            json!({"max_environments": 1, "storages": ["one"], "resources_max": {"cpu_millis": 2000, "memory_mb": 1024}}),
        ),
        (
            "template",
            Permission::TemplateUse,
            json!({"capabilities": ["one", "two"], "resources_max": {"cpu_millis": 4000, "memory_mb": 2048}}),
            json!({"capabilities": ["one"], "resources_max": {"cpu_millis": 2000, "memory_mb": 1024}}),
        ),
    ] {
        let id = world
            .thing(kind, world.maker, world.root, "Shared resource")
            .await;
        world
            .grant(
                kind,
                id,
                Permission::AccessManage,
                Holder::User { id: manager },
            )
            .await;
        let entry = world
            .put(kind, id, permission, Holder::User { id: recipient }, before)
            .await;
        world
            .change_thing(
                manager,
                kind,
                id,
                json!({"rows": [{"permission": permission.as_str(), "update": [{
                    "entry_id": entry, "options": narrowed
                }]}]}),
            )
            .await
            .unwrap();
        let page = world.read_thing(world.maker, kind, id).await;
        assert_eq!(
            page.rows
                .iter()
                .find(|row| row.permission == permission)
                .unwrap()
                .entries[0]
                .options,
            narrowed
        );
        let error = world
            .change_thing(
                manager,
                kind,
                id,
                json!({"rows": [{"permission": permission.as_str(), "update": [{
                    "entry_id": entry, "options": {}
                }]}]}),
            )
            .await
            .unwrap_err();
        missing(error, permission);
    }
}
