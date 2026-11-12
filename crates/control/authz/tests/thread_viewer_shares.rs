//! Proposed new-feature test: deliberate document disclosure to a thread's current viewers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::documents;
use orch_authz::pages::Target;
use orch_authz::{Caller, Policy, Principal, TokenGrant};
use orch_db::queries::documents as rows;
use orch_types::{Id, Time, new_id};
use serde_json::json;
use support::World;

fn page(document: Id) -> Target {
    Target::Resource(ObjectRef {
        kind: ObjectKind::Document,
        id: document,
    })
}

#[tokio::test]
async fn native_administrator_view_does_not_turn_into_membership_of_a_thread_viewer_audience() {
    let w = World::new().await;
    let admin = w.member("admin").await;
    w.give(w.root, admin, Role::Admin).await;
    w.change(w.maker, Target::Live(w.thread), json!({"whole":"private"}))
        .await
        .unwrap();
    assert!(!w.can(admin, w.thread, P::ThreadView).await);
    assert!(
        w.can_as(
            Caller::session(admin).with_admin_mode(true),
            w.thread,
            P::ThreadView
        )
        .await
    );
    let document = private_document(&w).await;
    w.change(
        w.maker,
        page(document),
        json!({"rows":[{"permission":"doc.view","add":[{
            "holder":Holder::Role{role:Role::Viewer,at:Some(w.thread)}
        }]}]}),
    )
    .await
    .unwrap();
    let principal = w
        .principal(Caller::session(admin).with_admin_mode(true))
        .await;
    let mut conn = w.conn().await;
    let (graph, _, site) = documents::site(&mut conn, document).await.unwrap();
    let frame = orch_authz::engine::Frame::load(
        &mut conn,
        &principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[P::DocView],
    )
    .await
    .unwrap();
    assert!(
        frame.has_permission(&site, P::DocView, false),
        "native administration itself can read the private document"
    );
    assert!(
        frame.named(&site, P::DocView).is_empty(),
        "native source-thread View must not be laundered into an ordinary document audience"
    );
}

#[tokio::test]
async fn thread_viewer_disclosure_rechecks_expiry_cuts_and_cycles_but_keeps_independent_document_grants()
 {
    let w = World::new().await;
    let reader = w.member("reader").await;
    let peer = w.node("thread", w.topic, "Audience peer").await;
    for thread in [w.thread, peer] {
        w.change(w.maker, Target::Live(thread), json!({"whole":"private"}))
            .await
            .unwrap();
    }
    let document = private_document(&w).await;
    w.change(
        w.maker,
        page(document),
        json!({"rows":[{"permission":"doc.view","add":[{
            "holder":Holder::Role{role:Role::Viewer,at:Some(w.thread)}
        }]}]}),
    )
    .await
    .unwrap();
    for (target, source) in [(w.thread, peer), (peer, w.thread)] {
        w.change(
            w.maker,
            Target::Live(target),
            json!({"rows":[{"permission":"thread.view","add":[{
                "holder":Holder::Role{role:Role::Viewer,at:Some(source)}
            }]}]}),
        )
        .await
        .unwrap();
    }
    assert!(
        !document_holds(&w, reader, document, P::DocView).await,
        "a reference cycle has no seed"
    );
    let end = Time::new(w.now.get() + chrono::Duration::minutes(1));
    w.change(
        w.maker,
        Target::Live(peer),
        json!({"rows":[{"permission":"thread.view","add":[{
            "holder":Holder::User{id:reader},"expires_at":end
        }]}]}),
    )
    .await
    .unwrap();
    assert!(document_holds(&w, reader, document, P::DocView).await);
    assert!(
        audience_people(&w, document).await.contains(&reader),
        "the access-page audience uses the same current membership"
    );
    // The credential can read the document without carrying ThreadView; audience membership is
    // the person's current standing, while its actions remain narrowed by the token.
    let scopes = TokenGrant::from_stored(&["doc.view".into()], None).unwrap();
    let principal = w.principal(Caller::token(reader, scopes)).await;
    let mut conn = w.conn().await;
    assert!(matches!(
        documents::check(
            &mut conn,
            &principal,
            document,
            P::DocView,
            Policy::default()
        )
        .await
        .unwrap(),
        Decision::Allowed { .. }
    ));
    assert!(!matches!(
        documents::check(
            &mut conn,
            &principal,
            document,
            P::DocEdit,
            Policy::default()
        )
        .await
        .unwrap(),
        Decision::Allowed { .. }
    ));
    let late = Principal::load(
        &mut conn,
        Caller::session(reader),
        Time::new(end.get() + chrono::Duration::seconds(1)),
    )
    .await
    .unwrap();
    assert!(
        !matches!(
            documents::check(&mut conn, &late, document, P::DocView, Policy::default())
                .await
                .unwrap(),
            Decision::Allowed { .. }
        ),
        "expired seed must not survive through its reference cycle"
    );
    drop(conn);
    let seed = w
        .read(w.maker, Target::Live(peer))
        .await
        .rows
        .into_iter()
        .find(|row| row.permission == P::ThreadView)
        .unwrap()
        .entries
        .into_iter()
        .find(|entry| entry.holder == Holder::User { id: reader })
        .unwrap()
        .id;
    w.change(
        w.maker,
        Target::Live(peer),
        json!({"rows":[{"permission":"thread.view","remove":[seed]}]}),
    )
    .await
    .unwrap();
    assert!(!document_holds(&w, reader, document, P::DocView).await);
    assert!(!audience_people(&w, document).await.contains(&reader));
    w.name_default(
        Some(w.topic),
        P::ThreadView,
        "managed",
        Holder::User { id: reader },
    )
    .await;
    w.change(
        w.maker,
        Target::Live(peer),
        json!({"rows":[{"permission":"thread.view","inherit":true}]}),
    )
    .await
    .unwrap();
    assert!(document_holds(&w, reader, document, P::DocView).await);
    w.change(
        w.maker,
        Target::Live(peer),
        json!({"rows":[{"permission":"thread.view","inherit":false}]}),
    )
    .await
    .unwrap();
    assert!(
        !document_holds(&w, reader, document, P::DocView).await,
        "a current source cut ends its audience contribution"
    );
    w.change(
        w.maker,
        page(document),
        json!({"rows":[{"permission":"doc.view","add":[{
            "holder":Holder::User{id:reader}
        }]}]}),
    )
    .await
    .unwrap();
    assert!(
        document_holds(&w, reader, document, P::DocView).await,
        "an independent ordinary document grant remains"
    );
    assert!(!document_holds(&w, reader, document, P::DocEdit).await);
    assert!(!document_holds(&w, reader, document, P::DocManage).await);
}

async fn private_document(w: &World) -> Id {
    let id = new_id();
    let mut conn = w.conn().await;
    // Same document fixture producer as tests/documents.rs, including a unique basename.
    rows::containers::insert_document(
        &mut conn,
        &rows::NewDocument {
            id,
            node_id: w.topic,
            org_id: w.root,
            owner_id: w.maker,
            created_by_thread: None,
            created_by_agent: None,
            title: "Deliberately disclosed original",
            now: w.now.get(),
        },
        None,
        &format!("Thread-share~{}.md", id.simple()),
    )
    .await
    .unwrap();
    documents::start(&mut conn, id, w.maker, w.now)
        .await
        .unwrap();
    drop(conn);
    w.change(w.maker, page(id), json!({"whole":"private"}))
        .await
        .unwrap();
    id
}

async fn document_holds(w: &World, person: Id, document: Id, permission: P) -> bool {
    let principal = w.principal(Caller::session(person)).await;
    let mut conn = w.conn().await;
    matches!(
        documents::check(
            &mut conn,
            &principal,
            document,
            permission,
            Policy::default(),
        )
        .await
        .unwrap(),
        Decision::Allowed { .. }
    )
}

async fn audience_people(w: &World, document: Id) -> Vec<Id> {
    let principal = w.principal(Caller::session(w.maker)).await;
    let mut conn = w.conn().await;
    let (graph, _, site) = documents::site(&mut conn, document).await.unwrap();
    let frame = orch_authz::engine::Frame::load(
        &mut conn,
        &principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[P::DocView],
    )
    .await
    .unwrap();
    orch_authz::holders::people_of(
        &mut conn,
        &frame,
        &site,
        &Holder::Role {
            role: Role::Viewer,
            at: Some(w.thread),
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_place_viewer_derived_from_thread_view_has_the_same_document_access_and_holder_chain() {
    let w = World::new().await;
    let reader = w.member("reader").await;
    w.change(w.maker, Target::Live(w.thread), json!({"whole":"private"}))
        .await
        .unwrap();
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows":[{"permission":"thread.view","add":[{
            "holder":Holder::User{id:reader}
        }]}]}),
    )
    .await
    .unwrap();
    // The topic writer must be able to name this private source; View gives no control.
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows":[{"permission":"thread.view","add":[{"holder":Holder::User{id:w.boss}}]}]}),
    )
    .await
    .unwrap();
    w.change(
        w.boss,
        Target::Live(w.topic),
        json!({"rows":[{"permission":"topic.view","inherit":false,"add":[{
            "holder":Holder::Role{role:Role::Viewer,at:Some(w.thread)}
        }]}]}),
    )
    .await
    .unwrap();
    assert!(w.can(reader, w.topic, P::TopicView).await);
    let document = private_document(&w).await;
    let holder = Holder::Role {
        role: Role::Viewer,
        at: Some(w.topic),
    };
    w.change(
        w.maker,
        page(document),
        json!({"rows":[{"permission":"doc.view","add":[{
            "holder":holder
        }]}]}),
    )
    .await
    .unwrap();
    assert!(document_holds(&w, reader, document, P::DocView).await);
    let principal = w.principal(Caller::session(reader)).await;
    let mut conn = w.conn().await;
    let chain = orch_authz::holders::chain(&mut conn, &principal, holder, None)
        .await
        .unwrap();
    assert_eq!(chain.first(), Some(&holder));
    assert!(chain.contains(&Holder::Role {
        role: Role::Viewer,
        at: Some(w.thread)
    }));
    assert_eq!(chain.last(), Some(&Holder::User { id: reader }));
}

#[tokio::test]
async fn a_document_shared_with_thread_viewers_reaches_current_and_future_readers_without_editing_or_an_agent_entry()
 {
    let w = World::new().await;
    let direct = w.member("direct").await;
    let grouped = w.member("grouped").await;
    let by_role = w.member("by_role").await;
    let future = w.member("future").await;
    let unrelated = w.member("unrelated").await;
    // The giver can name this in-tree group and role source through their visible org.
    w.give(w.root, w.maker, Role::Viewer).await;
    let group = w.group(w.root, "Current reviewers", &[grouped]).await;
    w.give(w.project, by_role, Role::Viewer).await;
    w.change(w.maker, Target::Live(w.thread), json!({"whole":"private"}))
        .await
        .unwrap();
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows":[{"permission":"thread.view","add":[
            {"holder":Holder::User{id:direct}},
            {"holder":Holder::Group{id:group}},
            {"holder":Holder::Role{role:Role::Viewer,at:Some(w.project)}}
        ]}]}),
    )
    .await
    .unwrap();
    for reader in [direct, grouped, by_role] {
        assert!(w.can(reader, w.thread, P::ThreadView).await);
        assert!(!w.can(reader, w.thread, P::ThreadInteract).await);
    }
    for nonviewer in [future, unrelated] {
        assert!(!w.can(nonviewer, w.thread, P::ThreadView).await);
    }
    let document = private_document(&w).await;
    for reader in [direct, grouped, by_role, future, unrelated] {
        assert!(!document_holds(&w, reader, document, P::DocView).await);
    }
    // Normal Share/page writer, not a raw permission-entry insertion or a new ACL.
    // bd1 production is expected to reject this existing holder at the thread.
    let shared = w
        .change(
            w.maker,
            page(document),
            json!({"rows":[{"permission":"doc.view","add":[{
                "holder":Holder::Role{role:Role::Viewer,at:Some(w.thread)}
            }]}]}),
        )
        .await;
    assert!(
        shared.is_ok(),
        "an authorized deliberate share must accept current thread viewers: {shared:?}"
    );
    for reader in [direct, grouped, by_role] {
        assert!(document_holds(&w, reader, document, P::DocView).await);
        assert!(!document_holds(&w, reader, document, P::DocEdit).await);
        assert!(!document_holds(&w, reader, document, P::DocManage).await);
    }
    assert!(!document_holds(&w, unrelated, document, P::DocView).await);
    assert!(!document_holds(&w, future, document, P::DocView).await);
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows":[{"permission":"thread.view","add":[{
            "holder":Holder::User{id:future}
        }]}]}),
    )
    .await
    .unwrap();
    assert!(document_holds(&w, future, document, P::DocView).await);
    assert!(!document_holds(&w, future, document, P::DocEdit).await);
    assert!(!document_holds(&w, future, document, P::DocManage).await);
    let mut conn = w.conn().await;
    let agent_entries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permission_entries WHERE object_kind='document' AND object_id=$1
         AND holder_kind='agents' AND removed_at IS NULL",
    )
    .bind(document)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        agent_entries, 0,
        "human disclosure must not invent an agent grant"
    );
}
