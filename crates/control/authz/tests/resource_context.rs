//! A selected work share is narrower than the person's ownership or management elsewhere.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a deciding case fails by panicking"
)]
mod support;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Options, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::{Act, Caller, Policy, resources};
use orch_db::queries::{access::testing::insert_member, hierarchy::testing::insert_node};
use orch_types::{Id, new_id};
use serde_json::json;
use support::World;

async fn other_root(w: &World) -> Id {
    let id = new_id();
    let mut conn = w.conn().await;
    insert_node(&mut conn, id, "org", None, "Resource home", w.boss)
        .await
        .unwrap();
    for person in [w.boss, w.maker] {
        insert_member(&mut conn, id, person, id).await.unwrap();
    }
    id
}

#[tokio::test]
async fn only_the_actual_contexts_work_share_supplies_options_even_to_an_owner_and_manager() {
    let w = World::new().await;
    let home = other_root(&w).await;
    let provider = w.provider(w.boss, home, "Shared provider").await;
    let unshared = w.provider(w.boss, home, "Other work's provider").await;
    let agent = w.thing("agent", w.boss, home, "Shared profile").await;
    let entry = w
        .put(
            "provider",
            provider,
            P::ProviderUse,
            Holder::Node { id: w.project },
            json!({"models":["small"],"efforts":["low"]}),
        )
        .await;
    w.grant(
        "provider",
        provider,
        P::ProviderManage,
        Holder::User { id: w.boss },
    )
    .await;
    w.grant("agent", agent, P::AgentUse, Holder::Node { id: w.project })
        .await;
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let global = resources::effective(
        &mut conn,
        &principal,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(
        global.owner
            && global
                .held
                .iter()
                .any(|held| held.permission == P::ProviderManage)
    );
    let grants = resources::context::among(
        &mut conn,
        &principal,
        w.topic,
        &[provider, unshared, agent],
        Policy::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        grants.len(),
        2,
        "global ownership does not add an unshared resource"
    );
    let provider_grant = grants
        .iter()
        .find(|grant| grant.resource.id == provider)
        .unwrap();
    assert_eq!(provider_grant.permission, P::ProviderUse);
    assert_eq!(
        provider_grant.resource.home,
        Some(home),
        "the resource's real home is not relabeled"
    );
    assert_eq!(
        provider_grant
            .sources
            .iter()
            .map(|source| source.0)
            .collect::<Vec<_>>(),
        [entry]
    );
    let Options::ProviderUse(options) = &provider_grant.options else {
        panic!("provider options")
    };
    assert_eq!(options.models.as_deref(), Some(&["small".to_owned()][..]));
    assert_eq!(options.efforts.as_deref(), Some(&["low".to_owned()][..]));
    assert!(
        grants
            .iter()
            .any(|grant| grant.resource.id == agent && grant.permission == P::AgentUse)
    );
    sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE id=$1")
        .bind(entry)
        .bind(w.now.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        resources::context::among(
            &mut conn,
            &principal,
            w.topic,
            &[provider],
            Policy::default()
        )
        .await
        .unwrap()
        .is_empty(),
        "revoking the work share cannot fall back to Manage"
    );
}

#[tokio::test]
async fn a_context_reader_needs_no_thread_creation_and_another_real_node_or_unreadable_node_refuses()
 {
    let w = World::new().await;
    let home = other_root(&w).await;
    let provider = w.provider(w.boss, home, "Work provider").await;
    w.give(w.project, w.maker, Role::Viewer).await;
    w.grant(
        "provider",
        provider,
        P::ProviderUse,
        Holder::Node { id: w.project },
    )
    .await;
    let reader = w.principal(Caller::session(w.maker)).await;
    let mut conn = w.conn().await;
    assert!(
        !orch_authz::nodes::check(
            &mut conn,
            &reader,
            w.topic,
            P::TopicStartThreads,
            Act::Use,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    let global = resources::effective(
        &mut conn,
        &reader,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(
        global.held.is_empty(),
        "no global resource permission is borrowed"
    );
    assert_eq!(
        resources::context::among(&mut conn, &reader, w.topic, &[provider], Policy::default())
            .await
            .unwrap()
            .len(),
        1
    );
    drop(conn);
    let another = w.node("project", w.root, "Another real context").await;
    w.give(another, w.maker, Role::Viewer).await;
    let reader = w.principal(Caller::session(w.maker)).await;
    assert!(
        resources::context::among(
            &mut *w.conn().await,
            &reader,
            another,
            &[provider],
            Policy::default()
        )
        .await
        .unwrap()
        .is_empty()
    );
    let stranger = w.user("context-stranger").await;
    w.grant(
        "provider",
        provider,
        P::ProviderManage,
        Holder::User { id: stranger },
    )
    .await;
    let stranger = w.principal(Caller::session(stranger)).await;
    assert!(
        resources::context::among(
            &mut *w.conn().await,
            &stranger,
            w.topic,
            &[provider],
            Policy::default()
        )
        .await
        .is_err(),
        "global Manage does not open an unreadable context"
    );
}
