//! A setting's actual visible descendants and overrides, with merge rules, locks and bounded paging over the real routes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use crate::support::FullPermissionPageReply;
use orch_api_contract::access::{PageChange, PermissionPageUpdate};
use orch_api_contract::admin::AdminSettingReachGet;
use orch_api_contract::common::{ErrorCode, PageQuery};
use orch_api_contract::settings::{
    NodeSettingReachGet, NodeSettingSet, NodeSettingSetBody, SettingReach,
};
use orch_types::{Id, Json, new_id};
use places::{Who, admin, join, make_org, make_project, make_topic, person};
use serde_json::json;
use support::{Statements, World};

async fn put(world: &World, who: &Who, at: Id, key: &str, value: Json, locked: bool) {
    world
        .app
        .call(NodeSettingSet, &[&at, &key])
        .token(&who.token)
        .body(&NodeSettingSetBody {
            model_choice: None,
            value,
            locked: Some(locked),
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}

async fn reach(world: &World, who: &Who, at: Id, key: &str, page: PageQuery) -> SettingReach {
    world
        .app
        .call(NodeSettingReachGet, &[&at, &key])
        .token(&who.token)
        .query(&page)
        .send()
        .await
        .ok()
}

async fn thread(world: &World, owner: &Who, topic: Id) -> Id {
    let id = new_id();
    let mut conn = world.conn().await;
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        id,
        "thread",
        Some(topic),
        "A thread",
        owner.id(),
    )
    .await
    .unwrap();
    // Define its read audience explicitly; the reach fixture does not need a running binding.
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", id, "live"),
        "any",
        "thread.view",
        ("role", Some(topic), Some("viewer")),
        None,
        world.now(),
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_viewer_can_inspect_reach_and_a_private_branch_leaks_neither_its_name_nor_its_count() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let viewer = person(&world, "viewer").await;
    let org = make_org(&world, &owner, "Team", "team").await.base.id;
    join(&world, org, org, &viewer, "viewer", &owner).await;
    let project = make_project(&world, &owner, org, "Web").await.base.id;
    let topic = make_topic(&world, &owner, project, "Login").await.base.id;
    thread(&world, &owner, topic).await;
    let hidden = make_topic(&world, &owner, project, "Secret override")
        .await
        .base
        .id;
    thread(&world, &owner, hidden).await;
    put(
        &world,
        &owner,
        org,
        "agents.subagents_at_once",
        json!(4),
        false,
    )
    .await;
    put(
        &world,
        &owner,
        hidden,
        "agents.subagents_at_once",
        json!(2),
        false,
    )
    .await;
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &hidden])
        .token(&owner.token)
        .body(&serde_json::from_value::<PageChange>(json!({"whole":"private"})).unwrap())
        .send()
        .await
        .full_permission_page()
        .ok();
    let shown = reach(
        &world,
        &viewer,
        org,
        "agents.subagents_at_once",
        PageQuery::default(),
    )
    .await;
    // Web, and the General project a team's org starts with.
    assert_eq!(
        (
            shown.applies.projects,
            shown.applies.topics,
            shown.applies.threads
        ),
        (2, 1, 1)
    );
    assert!(shown.overrides.items.is_empty());
    let owner_view = reach(
        &world,
        &owner,
        org,
        "agents.subagents_at_once",
        PageQuery::default(),
    )
    .await;
    assert_eq!(
        owner_view
            .overrides
            .items
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        vec![hidden]
    );
    let outsider = person(&world, "outsider").await;
    assert_eq!(
        world
            .app
            .call(NodeSettingReachGet, &[&org, &"agents.subagents_at_once"])
            .token(&outsider.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn an_ignored_wider_minimum_is_not_an_override_and_a_tighter_one_is() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Team", "team").await.base.id;
    let project = make_project(&world, &owner, org, "Web").await.base.id;
    let topic = make_topic(&world, &owner, project, "Login").await.base.id;
    thread(&world, &owner, topic).await;
    // The project's wider minimum was chosen before the org's ceiling, which only narrows it.
    put(
        &world,
        &owner,
        project,
        "agents.subagents_at_once",
        json!(8),
        false,
    )
    .await;
    put(
        &world,
        &owner,
        org,
        "agents.subagents_at_once",
        json!(4),
        false,
    )
    .await;
    let same = reach(
        &world,
        &owner,
        org,
        "agents.subagents_at_once",
        PageQuery::default(),
    )
    .await;
    // Web, and the General project a team's org starts with.
    assert_eq!(
        (
            same.applies.projects,
            same.applies.topics,
            same.applies.threads
        ),
        (2, 1, 1)
    );
    assert!(same.overrides.items.is_empty());
    put(
        &world,
        &owner,
        topic,
        "agents.subagents_at_once",
        json!(2),
        false,
    )
    .await;
    let tighter = reach(
        &world,
        &owner,
        org,
        "agents.subagents_at_once",
        PageQuery::default(),
    )
    .await;
    assert_eq!(
        (
            tighter.applies.projects,
            tighter.applies.topics,
            tighter.applies.threads
        ),
        (2, 0, 0)
    );
    assert_eq!(
        tighter
            .overrides
            .items
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        vec![topic]
    );
}

#[tokio::test]
async fn additive_values_keep_the_parent_contribution_and_locked_out_rows_are_not_overrides() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Team", "team").await.base.id;
    let project = make_project(&world, &owner, org, "Web").await.base.id;
    let topic = make_topic(&world, &owner, project, "Login").await.base.id;
    put(
        &world,
        &owner,
        org,
        "agent.instructions",
        json!("Team guidance"),
        false,
    )
    .await;
    put(
        &world,
        &owner,
        topic,
        "agent.instructions",
        json!("Topic guidance"),
        false,
    )
    .await;
    let added = reach(
        &world,
        &owner,
        org,
        "agent.instructions",
        PageQuery::default(),
    )
    .await;
    // Web, and the General project a team's org starts with.
    assert_eq!((added.applies.projects, added.applies.topics), (2, 1));
    assert!(added.overrides.items.is_empty());
    put(&world, &owner, topic, "auto.title", json!("off"), false).await;
    put(&world, &owner, org, "auto.title", json!("harness"), true).await;
    let locked = reach(&world, &owner, org, "auto.title", PageQuery::default()).await;
    assert_eq!((locked.applies.projects, locked.applies.topics), (2, 1));
    assert!(locked.overrides.items.is_empty());
}

#[tokio::test]
async fn overrides_page_without_changing_the_counts_and_the_query_count_does_not_grow_with_children()
 {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Team", "team").await.base.id;
    let project = make_project(&world, &owner, org, "Web").await.base.id;
    put(&world, &owner, org, "auto.title", json!("harness"), false).await;
    let mut topics = Vec::new();
    let mut one_child_statements = None;
    for index in 0..11 {
        let topic = make_topic(&world, &owner, project, &format!("Topic {index}"))
            .await
            .base
            .id;
        put(&world, &owner, topic, "auto.title", json!("off"), false).await;
        topics.push(topic);
        if index == 0 {
            let before = Statements::now();
            reach(&world, &owner, org, "auto.title", PageQuery::default()).await;
            one_child_statements = Some(Statements::now() - before);
        }
    }
    topics.sort_unstable();
    let start = Statements::now();
    let first = reach(
        &world,
        &owner,
        org,
        "auto.title",
        PageQuery {
            limit: Some(2),
            cursor: None,
        },
    )
    .await;
    let statements = Statements::now() - start;
    assert_eq!(
        first
            .overrides
            .items
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        topics[..2]
    );
    let next = reach(
        &world,
        &owner,
        org,
        "auto.title",
        PageQuery {
            limit: Some(200),
            cursor: first.overrides.next_cursor,
        },
    )
    .await;
    assert_eq!(
        next.overrides
            .items
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        topics[2..]
    );
    assert_eq!(first.applies, next.applies);
    assert_eq!(
        statements,
        one_child_statements.unwrap(),
        "one frame and one settings read for one or eleven children"
    );
}

#[tokio::test]
async fn system_reach_has_the_system_settings_read_gate_and_unknown_keys_are_not_settings() {
    let world = World::new().await;
    let ordinary = person(&world, "ordinary").await;
    let boss = admin(&world, "boss").await;
    assert_eq!(
        world
            .app
            .call(AdminSettingReachGet, &[&"agents.subagents_at_once"])
            .token(&ordinary.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    world
        .app
        .call(AdminSettingReachGet, &[&"agents.subagents_at_once"])
        .token(&boss.token)
        .send()
        .await
        .ok();
    assert_eq!(
        world
            .app
            .call(AdminSettingReachGet, &[&"not.a.setting"])
            .token(&boss.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn an_environment_setting_reaches_the_environments_homed_below_that_take_it_and_their_threads()
 {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Team", "team").await.base.id;
    let project = make_project(&world, &owner, org, "Web").await.base.id;
    let topic = make_topic(&world, &owner, project, "Login").await.base.id;
    let taking = thread(&world, &owner, topic).await;
    let keeping = thread(&world, &owner, topic).await;
    let elsewhere = thread(&world, &owner, topic).await;
    let idle = thread(&world, &owner, topic).await;
    let own = {
        let mut conn = world.conn().await;
        orch_db::queries::hierarchy::testing::put_thread_in_environment(
            &mut conn,
            taking,
            "connected",
            owner.id(),
            topic,
        )
        .await
        .unwrap();
        let own = orch_db::queries::hierarchy::testing::put_thread_in_environment(
            &mut conn,
            keeping,
            "connected",
            owner.id(),
            topic,
        )
        .await
        .unwrap();
        // Homed above the topic: the topic's value is not its own.
        orch_db::queries::hierarchy::testing::put_thread_in_environment(
            &mut conn,
            elsewhere,
            "connected",
            owner.id(),
            org,
        )
        .await
        .unwrap();
        orch_db::queries::settings::put(
            &mut conn,
            Some(topic),
            "env.idle_stop_minutes",
            &orch_db::queries::settings::Stored {
                value: &json!(30),
                locked: false,
                by: Some(owner.id()),
                at: world.now(),
            },
            None,
        )
        .await
        .unwrap();
        own
    };
    // Its own value replaces the topic's.
    sqlx::query("UPDATE environments SET config=$2 WHERE id=$1")
        .bind(own)
        .bind(json!({"env.idle_stop_minutes": 10}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let shown = reach(
        &world,
        &owner,
        topic,
        "env.idle_stop_minutes",
        PageQuery::default(),
    )
    .await;
    // One environment takes the topic's value, with its one thread; the thread with no environment is no part of it.
    assert_eq!(
        (
            shown.applies.environments,
            shown.applies.environment_threads
        ),
        (1, 1),
        "{idle} has no environment"
    );
    // Nothing is set at the project: of the two environments below it, one takes the topic's value and one its own.
    let above = reach(
        &world,
        &owner,
        project,
        "env.idle_stop_minutes",
        PageQuery::default(),
    )
    .await;
    assert_eq!(above.applies.environments, 0);
    // A key no environment takes reaches none.
    let agents = reach(
        &world,
        &owner,
        topic,
        "agents.subagents_at_once",
        PageQuery::default(),
    )
    .await;
    assert_eq!(
        (
            agents.applies.environments,
            agents.applies.environment_threads
        ),
        (0, 0)
    );
}
