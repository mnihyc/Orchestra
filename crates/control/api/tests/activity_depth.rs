//! Activity groups meaningful acts before paging and names current things without exposing defaults identifiers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use chrono::Duration;
use orch_api_contract::nodes::{
    ActivityDepth, NodeActivityList, NodeActivityListQuery, NodeUpdate, NodeUpdateBody,
};
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_types::new_id;
use places::{make_org, make_project, person};
use serde_json::json;
use support::World;

#[tokio::test]
async fn repeated_changes_are_one_summary_entry_with_the_current_name_and_do_not_repeat_on_the_next_page()
 {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Acme", "acme").await;
    let project = make_project(&world, &ada, org.base.id, "Web").await;
    world.advance(Duration::minutes(1));
    let mut revision = project.base.revision;
    for name in ["Site", "Website"] {
        let changed = world
            .app
            .call(NodeUpdate, &[&project.base.id])
            .token(&ada.token)
            .body(&NodeUpdateBody {
                name: Some(name.into()),
                description: None,
                color: None,
                custom_color: None,
                readme_document_id: None,
                expected_revision: revision,
            })
            .send()
            .await
            .ok();
        revision = match changed {
            orch_api_contract::nodes::Node::Project(project) => project.base.revision,
            other => panic!("{other:?}"),
        };
    }
    let first = world
        .app
        .call(NodeActivityList, &[&org.base.id])
        .token(&ada.token)
        .query(&NodeActivityListQuery {
            limit: Some(1),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].count, 2);
    assert_eq!(first.items[0].node.name.as_deref(), Some("Website"));
    assert!(first.items[0].title.contains("Website"));
    assert!(!first.items[0].title.contains(&project.base.id.to_string()));
    let rest = world
        .app
        .call(NodeActivityList, &[&org.base.id])
        .token(&ada.token)
        .query(&NodeActivityListQuery {
            cursor: first.next_cursor,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        !rest
            .items
            .iter()
            .any(|entry| entry.action.as_deref() == Some("node.updated"))
    );
    let every = world
        .app
        .call(NodeActivityList, &[&org.base.id])
        .token(&ada.token)
        .query(&NodeActivityListQuery {
            depth: Some(ActivityDepth::Every),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let changed: Vec<_> = every
        .items
        .iter()
        .filter(|entry| entry.action.as_deref() == Some("node.updated"))
        .collect();
    assert_eq!(changed.len(), 2);
    assert!(
        changed
            .iter()
            .all(|entry| entry.count == 1 && entry.title.contains("Website"))
    );
}

#[tokio::test]
async fn a_defaults_entry_is_absent_from_summary_and_every_change_says_what_changed_without_the_holder_identifier()
 {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Acme", "acme").await;
    let agent_thread = new_id();
    let stale = format!("ada named the agents of {agent_thread} in a defaults row");
    let mut entry = Entry::new(
        Actor::User(ada.id()),
        "permission.added",
        "permission",
        Some(new_id()),
        &stale,
    );
    entry.history = true;
    entry.node_id = Some(org.base.id);
    entry.org_id = Some(org.base.id);
    entry.details = json!({"page": "defaults", "permission": "thread.view", "holder": {"kind": "agents", "id": agent_thread}});
    audit::append(
        &mut *world.conn().await,
        &entry,
        &Origin::default(),
        world.now(),
    )
    .await
    .unwrap();
    let summary = world
        .app
        .call(NodeActivityList, &[&org.base.id])
        .token(&ada.token)
        .query(&NodeActivityListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        !summary
            .items
            .iter()
            .any(|line| line.action.as_deref() == Some("permission.added"))
    );
    let every = world
        .app
        .call(NodeActivityList, &[&org.base.id])
        .token(&ada.token)
        .query(&NodeActivityListQuery {
            depth: Some(ActivityDepth::Every),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let line = every
        .items
        .iter()
        .find(|line| line.action.as_deref() == Some("permission.added"))
        .unwrap();
    assert_eq!(
        line.title, "Acme",
        "only the object's display title crosses the Activity API"
    );
    assert_eq!(line.object.as_ref().unwrap().name.as_deref(), Some("Acme"));
    assert!(!line.title.contains(&agent_thread.to_string()));
    let words = line.wording.as_ref().unwrap();
    assert_eq!((words.version, words.verb.as_str()), (1, "agentDefaults"));
    assert_eq!(words.target.as_ref().unwrap().name.as_deref(), Some("Acme"));
    // The same projection is served to the History reader; Summary is filtered before its one-row page.
    let history = world
        .app
        .call(
            orch_api_contract::access::HistoryList,
            &[&"nodes", &org.base.id],
        )
        .token(&ada.token)
        .query(&orch_api_contract::access::HistoryListQuery {
            depth: Some(ActivityDepth::Summary),
            limit: Some(1),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        !history
            .items
            .iter()
            .any(|line| line.action == "permission.added")
    );
    let history = world
        .app
        .call(
            orch_api_contract::access::HistoryList,
            &[&"nodes", &org.base.id],
        )
        .token(&ada.token)
        .query(&orch_api_contract::access::HistoryListQuery {
            depth: Some(ActivityDepth::Every),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let line = history
        .items
        .iter()
        .find(|line| line.action == "permission.added")
        .unwrap();
    assert_eq!(line.wording.as_ref().unwrap().verb, "agentDefaults");
    assert_eq!(
        line.wording
            .as_ref()
            .unwrap()
            .target
            .as_ref()
            .unwrap()
            .name
            .as_deref(),
        Some("Acme")
    );
}

#[tokio::test]
async fn activity_names_the_actual_resource_and_its_place_without_a_server_language_sentence() {
    let s = rig::Scene::new().await;
    let org = s.tree.org.base.id;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let name: String = sqlx::query_scalar("SELECT name FROM providers WHERE id=$1")
        .bind(s.on.provider)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    // A provider's making, as the catalogue names it: an Activity lists the catalogue's actions alone.
    let mut entry = Entry::new(
        Actor::User(s.ada.id()),
        "provider.created",
        "provider",
        Some(s.on.provider),
        "Ada made this provider: this English audit sentence belongs to the export",
    );
    entry.history = true;
    entry.node_id = Some(org);
    entry.org_id = Some(org);
    audit::append(&mut conn, &entry, &Origin::default(), s.rig.world.now())
        .await
        .unwrap();
    drop(conn);
    let rows = s
        .rig
        .world
        .app
        .call(NodeActivityList, &[&org])
        .token(&s.ada.token)
        .query(&NodeActivityListQuery {
            depth: Some(ActivityDepth::Every),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let row = rows
        .items
        .iter()
        .find(|row| {
            row.action.as_deref() == Some("provider.created")
                && row
                    .object
                    .as_ref()
                    .is_some_and(|object| object.id == s.on.provider)
        })
        .unwrap();
    let object = row.object.as_ref().unwrap();
    assert_eq!(
        (
            object.id,
            object.kind.as_str(),
            object.name.as_deref(),
            object.redacted
        ),
        (s.on.provider, "provider", Some(name.as_str()), false)
    );
    assert_eq!(row.node.id, org);
    assert_eq!(row.title, name);
    assert_eq!(row.wording.as_ref().unwrap().verb, "created");
    assert!(
        matches!(&row.actor, Some(orch_api_contract::access::HistoryActor::User(user)) if user.id == s.ada.id())
    );
}
