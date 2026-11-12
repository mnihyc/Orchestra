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
use orch_authz::Caller;
use orch_authz::pages::{ChangeError, Target};
use orch_types::Id;
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
async fn a_manager_without_the_use_shortens_the_places_of_an_entry_but_lengthens_or_takes_them_away_only_by_giving()
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
    let mut places = Vec::new();
    for name in ["first", "second", "third"] {
        let environment = world
            .thing("environment", world.maker, world.topic, name)
            .await;
        // The manager sees all three, so that naming them is not what is refused.
        world
            .grant(
                "environment",
                environment,
                Permission::EnvironmentView,
                Holder::User { id: manager },
            )
            .await;
        places.push(json!({"kind": "environment", "id": environment}));
    }
    let before = json!({"where": [places[0], places[1]]});
    let entry = world
        .put(
            "provider",
            id,
            Permission::ProviderUse,
            Holder::User { id: recipient },
            before,
        )
        .await;
    let update = |options: serde_json::Value| {
        json!({"rows": [{"permission": "provider.use", "update": [{
            "entry_id": entry, "options": options
        }]}]})
    };
    world
        .change_thing(
            manager,
            "provider",
            id,
            update(json!({"where": [places[0]]})),
        )
        .await
        .unwrap();
    let page = world.read_thing(world.maker, "provider", id).await;
    assert_eq!(
        page.rows
            .iter()
            .find(|row| row.permission == Permission::ProviderUse)
            .unwrap()
            .entries[0]
            .options,
        json!({"where": [places[0]]})
    );
    for options in [
        json!({"where": [places[0], places[2]]}),
        json!({"where": [places[1]]}),
        json!({}),
    ] {
        let epoch = world.epoch().await;
        let error = world
            .change_thing(manager, "provider", id, update(options))
            .await
            .unwrap_err();
        missing(error, Permission::ProviderUse);
        assert_eq!(world.epoch().await, epoch);
    }
}

#[tokio::test]
async fn a_manager_without_the_use_cannot_add_a_type_in_which_the_credential_leaves_to_an_entry() {
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
    // An entry that names no type allows those in which the credential stays where it is kept.
    let entry = world
        .put(
            "provider",
            id,
            Permission::ProviderUse,
            Holder::User { id: recipient },
            json!({"models": ["small"]}),
        )
        .await;
    let update = |options: serde_json::Value| {
        json!({"rows": [{"permission": "provider.use", "update": [{
            "entry_id": entry, "options": options
        }]}]})
    };
    for options in [
        json!({"models": ["small"], "arrangements": ["credential_shared"]}),
        json!({"models": ["small"], "arrangements": ["separated", "handed_over"]}),
        json!({"models": ["small"], "arrangements": ["separated", "key_separated", "credential_shared"]}),
    ] {
        let epoch = world.epoch().await;
        let error = world
            .change_thing(manager, "provider", id, update(options))
            .await
            .unwrap_err();
        missing(error, Permission::ProviderUse);
        assert_eq!(world.epoch().await, epoch);
    }
    // Naming a type in which it stays narrows, and needs no use.
    world
        .change_thing(
            manager,
            "provider",
            id,
            update(json!({"models": ["small"], "arrangements": ["separated"]})),
        )
        .await
        .unwrap();
}

/// Changes the page of a provider as somebody who may share what they hold: the place lets holders
/// share, as shipped.
async fn share_as(
    world: &World,
    user: Id,
    provider: Id,
    change: serde_json::Value,
) -> Result<orch_authz::pages::Outcome, ChangeError> {
    let change: orch_access_contract::pages::PageChange =
        serde_json::from_value(change).expect("a change that reads");
    let principal = world.principal(Caller::session(user)).await;
    let mut tx = world.db.pool.begin().await.unwrap();
    let outcome = orch_authz::pages::change_resource_under(
        &mut tx,
        &principal,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Provider,
            id: provider,
        },
        &change,
        orch_authz::Policy {
            holders_share: true,
            ..orch_authz::Policy::default()
        },
        &orch_authz::usable::ResourcePolicy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await;
    if outcome.is_ok() {
        tx.commit().await.unwrap();
    }
    outcome
}

#[tokio::test]
async fn somebody_who_shares_a_provider_gives_at_most_the_entry_they_hold_when_adding_as_when_editing()
 {
    let world = World::new().await;
    let sharer = world.member("sharer").await;
    let friend = world.member("friend").await;
    let id = world.provider(world.maker, world.root, "Shared key").await;
    world
        .put(
            "provider",
            id,
            Permission::ProviderUse,
            Holder::User { id: sharer },
            json!({"models": ["small"]}),
        )
        .await;
    let add = |options: serde_json::Value| {
        json!({"rows": [{"permission": "provider.use", "add": [{
            "holder": user_holder(friend), "options": options
        }]}]})
    };
    // More than the sharer holds: another model, no models named at all, or a copy.
    for options in [
        json!({"models": ["large"]}),
        json!({}),
        json!({"models": ["small"], "arrangements": ["credential_shared"]}),
    ] {
        let epoch = world.epoch().await;
        let error = share_as(&world, sharer, id, add(options))
            .await
            .unwrap_err();
        missing(error, Permission::ProviderUse);
        assert_eq!(world.epoch().await, epoch);
    }
    // What the sharer holds, or less of it.
    share_as(
        &world,
        sharer,
        id,
        add(json!({"models": ["small"], "arrangements": ["separated"]})),
    )
    .await
    .unwrap();
    // The entry just given is changed under the same rule: narrowed freely, never widened.
    let page = world.read_thing(world.maker, "provider", id).await;
    let given = page
        .rows
        .iter()
        .find(|row| row.permission == Permission::ProviderUse)
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.holder == Holder::User { id: friend })
        .unwrap()
        .id;
    let edit = |options: serde_json::Value| {
        json!({"rows": [{"permission": "provider.use", "update": [{
            "entry_id": given, "options": options
        }]}]})
    };
    let error = share_as(
        &world,
        sharer,
        id,
        edit(json!({"models": ["small"], "arrangements": ["separated", "handed_over"]})),
    )
    .await
    .unwrap_err();
    missing(error, Permission::ProviderUse);
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
