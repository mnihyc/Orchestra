//! An outside creator's conditional access cannot supply the parent reach it requires.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::{Holder, StoredKind};
use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::Role;
use orch_authz::pages::{self, Target};
use orch_db::queries::access::{self, PageKey};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_types::new_id;
use serde_json::json;
use support::World;

#[tokio::test]
async fn a_thread_viewer_cycle_cannot_preserve_an_outside_creators_conditional_manage_entry() {
    let w = World::new().await;
    let creator = w.user("outside-creator").await;
    let way = w
        .name(w.topic, P::TopicInteract, Holder::User { id: creator })
        .await;
    assert!(w.can(creator, w.topic, P::TopicInteract).await);
    let thread = new_id();
    let mut conn = w.conn().await;
    // The same producer as outside_creation.rs selects a real member as owner and retains
    // the outsider's conditional Creation Manage entry. It does not make the outsider an owner.
    insert_node(
        &mut conn,
        thread,
        "thread",
        Some(w.topic),
        "Conditional creation source",
        creator,
    )
    .await
    .unwrap();
    pages::start(&mut conn, StoredKind::Node, thread, creator, w.now)
        .await
        .unwrap();
    let key = PageKey::new("node", thread, "live");
    let entries = access::entries_of(&mut conn, &key).await.unwrap();
    let creation = entries
        .iter()
        .find(|entry| {
            entry.permission == "thread.manage"
                && entry.source == "creation"
                && entry.holder_id == Some(creator)
                && entry.created_by == Some(creator)
        })
        .expect("the fixture must exercise the real conditional creation entry")
        .clone();
    drop(conn);
    assert!(w.can(w.boss, thread, P::AccessOwn).await);
    assert!(!w.can(creator, thread, P::AccessOwn).await);
    assert!(w.can(creator, thread, P::ThreadManage).await);
    w.change(
        w.boss,
        Target::Live(w.topic),
        json!({"rows":[{"permission":"topic.view","add":[{
            "holder":Holder::Role{role:Role::Viewer,at:Some(thread)}
        }]}]}),
    )
    .await
    .unwrap();
    assert!(w.can(creator, w.topic, P::TopicView).await);
    assert!(w.can(creator, thread, P::ThreadView).await);

    w.change(
        w.boss,
        Target::Live(w.topic),
        json!({"rows":[{"permission":"topic.interact","remove":[way]}]}),
    )
    .await
    .unwrap();
    assert!(
        !w.can(creator, w.topic, P::TopicView).await,
        "the parent cannot remain reachable only through its child's conditional creation grant"
    );
    assert!(!w.can(creator, thread, P::ThreadView).await);
    assert!(!w.can(creator, thread, P::ThreadManage).await);
    let mut conn = w.conn().await;
    let retained = access::entries_of(&mut conn, &key)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == creation.id)
        .unwrap();
    assert_eq!(
        retained, creation,
        "a failed conditional source is inactive, not erased or rewritten by a read"
    );
    drop(conn);

    w.name(w.topic, P::TopicInteract, Holder::User { id: creator })
        .await;
    assert!(w.can(creator, w.topic, P::TopicInteract).await);
    assert!(
        w.can(creator, thread, P::ThreadManage).await,
        "a new independent parent source reactivates the same creation entry"
    );
}
