//! Topic statistics disclose only the native reader's visible main threads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod support;
use orch_api_contract::nodes::{Node, NodeChildList, NodeChildListQuery, NodeGet, Topic};
use orch_db::queries::{
    access::{self, PageKey, testing},
    threads,
};
use orch_types::{Id, Time, new_id};
use places::{Who, join, make_org, make_project, make_topic, person};
use serde_json::json;
use support::World;
struct Fixture {
    world: World,
    owner: Who,
    viewer: Who,
    project: Id,
    topic: Id,
    visible: Id,
    hidden: Vec<Id>,
    old: chrono::DateTime<chrono::Utc>,
    latest: chrono::DateTime<chrono::Utc>,
}
async fn thread(
    world: &World,
    owner: Id,
    topic: Id,
    name: &str,
    state: &str,
    at: chrono::DateTime<chrono::Utc>,
    private: bool,
) -> Id {
    let id = new_id();
    let mut tx = world.app.app.db.begin().await.unwrap();
    assert!(
        threads::insert_thread(&mut tx, id, topic, name, owner, world.now())
            .await
            .unwrap()
    );
    testing::name_owner(&mut tx, id, ("user", owner), world.now())
        .await
        .unwrap();
    if private {
        testing::set_row(
            &mut tx,
            &PageKey::new("node", id, "live"),
            "any",
            "thread.view",
            false,
            false,
        )
        .await
        .unwrap();
    }
    sqlx::query("UPDATE threads SET state=$2,blocked_reason=CASE WHEN $2='blocked' THEN 'agent_offline' ELSE NULL END,last_activity_at=$3 WHERE node_id=$1")
        .bind(id)
        .bind(state)
        .bind(at)
        .execute(&mut *tx)
        .await
        .unwrap();
    access::bump_epoch(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    id
}
impl Fixture {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = person(&world, "topic-owner").await;
        let viewer = person(&world, "topic-viewer").await;
        let org = make_org(&world, &owner, "Topic privacy", "topic-privacy")
            .await
            .base
            .id;
        join(&world, org, org, &viewer, "viewer", &owner).await;
        let project = make_project(&world, &owner, org, "Reader counts")
            .await
            .base
            .id;
        let topic = make_topic(&world, &owner, project, "Visible topic")
            .await
            .base
            .id;
        let old = world.now() - chrono::Duration::hours(4);
        let latest = world.now() - chrono::Duration::minutes(1);
        let visible = thread(
            &world,
            owner.id(),
            topic,
            "Visible main",
            "idle",
            old,
            false,
        )
        .await;
        let mut hidden = Vec::new();
        for (state, minutes) in [("running", 3), ("waiting_approval", 2), ("blocked", 1)] {
            hidden.push(
                thread(
                    &world,
                    owner.id(),
                    topic,
                    "Private work",
                    state,
                    world.now() - chrono::Duration::minutes(minutes),
                    true,
                )
                .await,
            );
        }
        // A readable side remains excluded from main-thread counts and latest activity.
        let side = thread(
            &world,
            owner.id(),
            topic,
            "Private side",
            "running",
            world.now(),
            true,
        )
        .await;
        sqlx::query("UPDATE threads SET side_of=$2,side_reading='read_only' WHERE node_id=$1")
            .bind(side)
            .bind(visible)
            .execute(&world.db.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE nodes child SET parent_id=parent.id,path=parent.path||child.id FROM nodes parent WHERE child.id=$1 AND parent.id=$2").bind(side).bind(visible).execute(&world.db.pool).await.unwrap();
        Self {
            world,
            owner,
            viewer,
            project,
            topic,
            visible,
            hidden,
            old,
            latest,
        }
    }
    async fn get(&self, by: &str) -> Topic {
        match self
            .world
            .app
            .call(NodeGet, &[&self.topic])
            .token(by)
            .send()
            .await
            .ok()
        {
            Node::Topic(topic) => topic,
            _ => panic!("topic expected"),
        }
    }
    fn visible_only(&self, topic: &Topic) {
        assert_eq!(topic.thread_count, 1);
        assert_eq!(
            (
                topic.attention.running,
                topic.attention.waiting_approval,
                topic.attention.blocked
            ),
            (0, 0, 0)
        );
        assert_eq!(topic.last_activity_at, Some(Time::new(self.old)));
    }
}
#[tokio::test]
async fn topic_get_and_project_list_count_no_private_state_or_activity_but_owners_keep_their_view()
{
    let f = Fixture::new().await;
    f.visible_only(&f.get(&f.viewer.token).await);
    let page = f
        .world
        .app
        .call(NodeChildList, &[&f.project])
        .token(&f.viewer.token)
        .query(&NodeChildListQuery::default())
        .send()
        .await
        .ok();
    let listed = page
        .items
        .into_iter()
        .filter_map(|row| row.into_visible())
        .find_map(|node| match node {
            Node::Topic(topic) if topic.base.id == f.topic => Some(topic),
            _ => None,
        })
        .unwrap();
    f.visible_only(&listed);
    let own = f.get(&f.owner.token).await;
    assert_eq!(own.thread_count, 4);
    assert_eq!(
        (
            own.attention.running,
            own.attention.waiting_approval,
            own.attention.blocked
        ),
        (1, 1, 1)
    );
    assert_eq!(own.last_activity_at, Some(Time::new(f.latest)));
    // A modern key retains the same native ACL and assurance as its actual person.
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    let secret = new_token(API_TOKEN_PREFIX);
    let scope:orch_access_contract::key_scope::KeyScope=serde_json::from_value(json!({"rules":[{"target":{"kind":"topic","id":f.topic,"reach":"self"},"resources":["topic"],"allow":[{"operation":"NodeGet.read","actions":["read"]}]}]})).unwrap();
    orch_db::queries::identity::create_api_token_scoped(
        &mut *f.world.conn().await,
        &orch_db::queries::identity::NewApiToken {
            id: new_id(),
            user_id: f.viewer.id(),
            name: "Visible topic statistics",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: f.world.now(),
            expires_at: Some(f.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    f.visible_only(&f.get(&secret).await);
}
#[tokio::test]
async fn sharing_and_revoking_one_thread_changes_only_its_actual_reader_statistics() {
    let f = Fixture::new().await;
    let id = f.hidden[2];
    f.visible_only(&f.get(&f.viewer.token).await);
    let mut tx = f.world.app.app.db.begin().await.unwrap();
    let entry = testing::add_entry(
        &mut tx,
        &PageKey::new("node", id, "live"),
        "any",
        "thread.view",
        ("user", Some(f.viewer.id()), None),
        None,
        f.world.now(),
    )
    .await
    .unwrap();
    access::bump_epoch(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    let shared = f.get(&f.viewer.token).await;
    assert_eq!(shared.thread_count, 2);
    assert_eq!(
        (
            shared.attention.running,
            shared.attention.waiting_approval,
            shared.attention.blocked
        ),
        (0, 0, 1)
    );
    assert_eq!(shared.last_activity_at, Some(Time::new(f.latest)));
    let mut tx = f.world.app.app.db.begin().await.unwrap();
    access::remove_entry(&mut tx, entry, f.owner.id(), f.world.now())
        .await
        .unwrap();
    access::bump_epoch(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    f.visible_only(&f.get(&f.viewer.token).await);
}
#[tokio::test]
async fn an_empty_visible_set_has_no_latest_private_activity_and_admin_reach_needs_its_mode() {
    let f = Fixture::new().await;
    testing::set_row(
        &mut *f.world.conn().await,
        &PageKey::new("node", f.visible, "live"),
        "any",
        "thread.view",
        false,
        false,
    )
    .await
    .unwrap();
    access::bump_epoch(&mut *f.world.conn().await)
        .await
        .unwrap();
    let empty = f.get(&f.viewer.token).await;
    assert_eq!(empty.thread_count, 0);
    assert_eq!(empty.last_activity_at, None);
    assert_eq!(
        (
            empty.attention.running,
            empty.attention.waiting_approval,
            empty.attention.blocked
        ),
        (0, 0, 0)
    );
    f.world.make_admin(&f.viewer.person).await;
    assert_eq!(f.get(&f.viewer.token).await.thread_count, 0);
    places::admin_mode(&f.world, &f.viewer, true).await;
    let admin = f.get(&f.viewer.token).await;
    assert_eq!(admin.thread_count, 4);
    assert_eq!(admin.last_activity_at, Some(Time::new(f.latest)));
    places::admin_mode(&f.world, &f.viewer, false).await;
    assert_eq!(f.get(&f.viewer.token).await.thread_count, 0);
}
