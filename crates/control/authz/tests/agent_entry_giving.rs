//! An agent's shared access is extended only by someone who can still drive it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod support;
use chrono::Duration;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::Role;
use orch_authz::pages::{ChangeError, Target};
use orch_authz::{Caller, Policy};
use serde_json::json;
use support::World;

#[tokio::test]
async fn losing_the_right_to_drive_an_agent_prevents_extending_its_share() {
    let world = World::new().await;
    let sharer = world.member("sharer").await;
    world
        .name(
            world.thread,
            Permission::DocEdit,
            Holder::User { id: sharer },
        )
        .await;
    let role = world.give(world.topic, sharer, Role::Developer).await;
    let policy = Policy {
        holders_share: true,
        ..Policy::default()
    };
    let until = world.later(Duration::hours(1));
    world
        .change_as(
            Caller::session(sharer),
            Target::Live(world.thread),
            json!({
                "rows": [{"permission": "doc.edit", "add": [{
                    "holder": {"kind": "agents", "id": world.topic}, "expires_at": until
                }]}]
            }),
            policy,
        )
        .await
        .unwrap();
    let page = world.read(world.maker, Target::Live(world.thread)).await;
    let entry = page
        .rows
        .iter()
        .find(|row| row.permission == Permission::DocEdit)
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.holder == Holder::Agents { id: world.topic })
        .unwrap()
        .id;
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::take_role_away(&mut conn, role, world.boss, world.now.get())
        .await
        .unwrap();
    drop(conn);
    let change = json!({"rows": [{"permission": "doc.edit", "update": [{
        "entry_id": entry, "expires_at": null
    }]}]});
    let denied = world
        .change_as(
            Caller::session(sharer),
            Target::Live(world.thread),
            change.clone(),
            policy,
        )
        .await;
    assert!(matches!(denied, Err(ChangeError::Denied(_))), "{denied:?}");
    let after = world.read(world.maker, Target::Live(world.thread)).await;
    let saved = after
        .rows
        .iter()
        .find(|row| row.permission == Permission::DocEdit)
        .unwrap()
        .entries
        .iter()
        .find(|candidate| candidate.id == entry)
        .unwrap();
    assert_eq!(saved.expires_at, Some(until));
    let shortened = world.later(Duration::minutes(30));
    world
        .change_as(
            Caller::session(sharer),
            Target::Live(world.thread),
            json!({"rows": [{"permission": "doc.edit", "update": [{
                "entry_id": entry, "expires_at": shortened
            }]}]}),
            policy,
        )
        .await
        .unwrap();
    world
        .change_as(
            Caller::session(world.boss),
            Target::Live(world.thread),
            change,
            policy,
        )
        .await
        .unwrap();
}
