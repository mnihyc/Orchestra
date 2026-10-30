//! Settings along a path that holds several projects: the nearest one wins, and a lock in an outer project decides what is set in the folders inside it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_settings::write::WriteError;
use orch_settings::{Place, Target};
use orch_settings_contract::provenance::{LockScope, SourceScope};
use orch_settings_contract::registry::keys;
use serde_json::json;
use support::World;

#[tokio::test]
async fn of_two_projects_on_a_path_the_inner_one_is_nearer() {
    let w = World::new().await;
    let folder = w.node("project", w.project, "Checkout").await;
    let cart = w.node("topic", folder, "Cart").await;
    let fix = w.node("thread", cart, "Fix").await;
    w.set(Target::Node(w.project), keys::AGENT_MODEL, json!("model-a"))
        .await;
    w.set(Target::Node(folder), keys::AGENT_MODEL, json!("model-b"))
        .await;

    let at_cart = w.view(Place::Node(cart), keys::AGENT_MODEL).await;
    assert_eq!(at_cart.value(), Some(&json!("model-b")));
    assert_eq!(at_cart.resolved.source.scope, SourceScope::Project);
    assert_eq!(
        at_cart.resolved.source.node,
        Some(folder),
        "the folder, not the project above it"
    );
    assert_eq!(
        w.value(Place::Node(fix), keys::AGENT_MODEL).await,
        Some(json!("model-b")),
        "and so does a thread of that topic"
    );
    // A topic beside the folder takes the outer project's, and the folder itself its own.
    let login = w.view(Place::Node(w.topic), keys::AGENT_MODEL).await;
    assert_eq!(login.value(), Some(&json!("model-a")));
    assert_eq!(login.resolved.source.node, Some(w.project));
    assert_eq!(
        w.value(Place::Node(folder), keys::AGENT_MODEL).await,
        Some(json!("model-b"))
    );
    assert_eq!(
        w.value(Place::Node(w.project), keys::AGENT_MODEL).await,
        Some(json!("model-a")),
        "what is set in a folder does not reach up"
    );
    // The folder takes its row away: the project's holds again.
    w.try_clear(Target::Node(folder), keys::AGENT_MODEL, None, true)
        .await
        .unwrap();
    assert_eq!(
        w.value(Place::Node(cart), keys::AGENT_MODEL).await,
        Some(json!("model-a"))
    );
}

#[tokio::test]
async fn a_lock_in_a_project_decides_what_is_set_in_the_folders_inside_it_and_the_outer_one_decides_over_the_inner()
 {
    let w = World::new().await;
    let folder = w.node("project", w.project, "Checkout").await;
    let cart = w.node("topic", folder, "Cart").await;
    w.set(Target::Node(folder), keys::APPROVAL_MODE, json!("auto"))
        .await;
    w.lock(
        Target::Node(folder),
        keys::APPROVAL_MODE,
        json!("on_request"),
    )
    .await;
    // The folder's lock holds the topics in it, and nothing beside.
    let inside = w.view(Place::Node(cart), keys::APPROVAL_MODE).await;
    assert_eq!(inside.value(), Some(&json!("on_request")));
    let lock = inside.resolved.locked_by.clone().unwrap();
    assert_eq!((lock.scope, lock.node), (LockScope::Project, Some(folder)));
    assert_ne!(
        w.value(Place::Node(w.topic), keys::APPROVAL_MODE).await,
        Some(json!("on_request")),
        "a topic of the project beside the folder is not held by it"
    );

    // The project above locks the same key: its lock decides over the folder's.
    w.lock(Target::Node(w.project), keys::APPROVAL_MODE, json!("ask"))
        .await;
    let both = w.view(Place::Node(cart), keys::APPROVAL_MODE).await;
    assert_eq!(both.value(), Some(&json!("ask")));
    let lock = both.resolved.locked_by.clone().unwrap();
    assert_eq!(
        (lock.scope, lock.node),
        (LockScope::Project, Some(w.project)),
        "the upper one decides"
    );
    // A write in the folder is refused naming the project, and so is clearing its row.
    let Err(WriteError::Locked { by }) = w
        .try_set(
            Target::Node(folder),
            keys::APPROVAL_MODE,
            json!("auto"),
            None,
            None,
            true,
        )
        .await
    else {
        panic!("a write under a lock was accepted");
    };
    assert_eq!((by.scope, by.node), (LockScope::Project, Some(w.project)));
    // Lifted above, the folder's own lock decides again.
    w.try_set(
        Target::Node(w.project),
        keys::APPROVAL_MODE,
        json!("ask"),
        Some(false),
        None,
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        w.value(Place::Node(cart), keys::APPROVAL_MODE).await,
        Some(json!("on_request"))
    );
}
