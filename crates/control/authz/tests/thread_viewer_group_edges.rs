//! Referenced mixed groups agree across ownership, roles, explanations and ordinary audiences.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use std::collections::BTreeSet;

use chrono::Duration;
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::engine::Frame;
use orch_authz::pages::{self, Target};
use orch_authz::things::{self, Thing};
use orch_authz::{Caller, Policy, Principal, documents, holders};
use orch_db::queries::{access, documents as rows};
use orch_types::{Id, Time, new_id};
use serde_json::json;
use support::World;

fn object(document: Id) -> ObjectRef {
    ObjectRef {
        kind: ObjectKind::Document,
        id: document,
    }
}

fn viewers(node: Id) -> Holder {
    Holder::Role {
        role: Role::Viewer,
        at: Some(node),
    }
}

/// The group refers to a supported topic audience, whose sole reader source is a thread audience.
/// No reader seed is installed here, so each control decides when membership begins and ends.
async fn mixed_group() -> (World, Id, Id) {
    let w = World::new().await;
    let reader = w.member("mixed-reader").await;
    w.give(w.root, w.maker, Role::Viewer).await;
    w.change(w.maker, Target::Live(w.thread), json!({"whole":"private"}))
        .await
        .unwrap();
    w.change(w.boss, Target::Live(w.topic), json!({"whole":"private"}))
        .await
        .unwrap();
    // Naming an audience requires ordinary visibility of its private source.
    // This gives the author View only; it does not seed the tested reader.
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows":[{"permission":"thread.view","add":[{
            "holder":Holder::User{id:w.boss}
        }]}]}),
    )
    .await
    .unwrap();
    w.change(
        w.boss,
        Target::Live(w.topic),
        json!({"rows":[{"permission":"topic.view","add":[{
            "holder":viewers(w.thread)
        }]}]}),
    )
    .await
    .unwrap();
    let group = w.group(w.root, "Thread-derived topic viewers", &[]).await;
    let mut conn = w.conn().await;
    access::add_group_holder(
        &mut conn,
        group,
        &access::GroupHolderRow {
            user_id: None,
            member_group_id: None,
            member_node_id: Some(w.topic),
            member_org_id: None,
            member_role: Some("viewer".into()),
        },
        w.boss,
        w.now.get(),
    )
    .await
    .unwrap();
    drop(conn);
    assert!(!w.can(reader, w.thread, P::ThreadView).await);
    assert!(!w.can(reader, w.topic, P::TopicView).await);
    (w, reader, group)
}

async fn seed(w: &World, reader: Id, expires_at: Option<Time>) {
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows":[{"permission":"thread.view","add":[{
            "holder":Holder::User{id:reader},"expires_at":expires_at
        }]}]}),
    )
    .await
    .unwrap();
    assert!(w.can(reader, w.thread, P::ThreadView).await);
    assert!(w.can(reader, w.topic, P::TopicView).await);
}

async fn private_document(w: &World) -> Id {
    let id = new_id();
    let mut conn = w.conn().await;
    rows::containers::insert_document(
        &mut conn,
        &rows::NewDocument {
            id,
            node_id: w.topic,
            org_id: w.root,
            owner_id: w.maker,
            created_by_thread: None,
            created_by_agent: None,
            title: "Mixed group target",
            now: w.now.get(),
        },
        None,
        &format!("Mixed-group-target~{}.md", id.simple()),
    )
    .await
    .unwrap();
    documents::start(&mut conn, id, w.maker, w.now)
        .await
        .unwrap();
    drop(conn);
    w.change(
        w.maker,
        Target::Resource(object(id)),
        json!({"whole":"private"}),
    )
    .await
    .unwrap();
    id
}

async fn share_document(w: &World, document: Id, group: Id) {
    w.change(
        w.maker,
        Target::Resource(object(document)),
        json!({"rows":[{"permission":"doc.view","add":[{
            "holder":Holder::Group{id:group}
        }]}]}),
    )
    .await
    .unwrap();
}

async fn document_holds(w: &World, person: Id, document: Id, permission: P, at: Time) -> bool {
    let mut conn = w.conn().await;
    let principal = Principal::load(&mut conn, Caller::session(person), at)
        .await
        .unwrap();
    matches!(
        documents::check(
            &mut conn,
            &principal,
            document,
            permission,
            Policy::default()
        )
        .await
        .unwrap(),
        Decision::Allowed { .. }
    )
}

async fn audience(w: &World, document: Id, group: Id, at: Time) -> BTreeSet<Id> {
    let mut conn = w.conn().await;
    let principal = Principal::load(&mut conn, Caller::session(w.maker), at)
        .await
        .unwrap();
    let (graph, site) = things::site(&mut conn, Thing::Other(object(document)))
        .await
        .unwrap();
    let frame = Frame::load(
        &mut conn,
        &principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[P::DocView],
    )
    .await
    .unwrap();
    holders::people_of(&mut conn, &frame, &site, &Holder::Group { id: group })
        .await
        .unwrap()
        .into_iter()
        .collect()
}

#[tokio::test]
async fn a_mixed_viewer_group_owns_only_while_its_ordinary_reader_source_counts() {
    let (w, reader, group) = mixed_group().await;
    let end = w.later(Duration::minutes(1));
    seed(&w, reader, Some(end)).await;
    let document = private_document(&w).await;
    assert!(!document_holds(&w, reader, document, P::DocManage, w.now).await);
    w.change(
        w.maker,
        Target::Resource(object(document)),
        json!({"owners":{"add":[Holder::Group{id:group}]}}),
    )
    .await
    .unwrap();
    assert!(
        document_holds(&w, reader, document, P::DocManage, w.now).await,
        "the current member of a named owner group is an ordinary owner"
    );
    let mut conn = w.conn().await;
    let principal = Principal::load(&mut conn, Caller::session(reader), w.now)
        .await
        .unwrap();
    let page = pages::read(
        &mut conn,
        &principal,
        Target::Resource(object(document)),
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(page.writable && page.owners_writable);
    assert!(
        page.owners
            .iter()
            .any(|entry| entry.holder == Holder::Group { id: group })
    );
    assert!(
        !principal.groups().contains(&group),
        "the thread-derived result belongs to this request, not the global holder cache"
    );
    drop(conn);
    let late = Time::new(end.get() + Duration::seconds(1));
    assert!(!document_holds(&w, reader, document, P::DocManage, late).await);
    assert!(!document_holds(&w, reader, document, P::DocView, late).await);
}

#[tokio::test]
async fn a_role_given_to_a_mixed_viewer_group_tracks_the_source_without_a_self_supporting_role() {
    let (w, reader, group) = mixed_group().await;
    let end = w.later(Duration::minutes(1));
    seed(&w, reader, Some(end)).await;
    // A sibling recipient prevents its granted role from supplying the source topic's View.
    let recipient_topic = w.node("topic", w.project, "Role recipient").await;
    let recipient = w.node("thread", recipient_topic, "Role target").await;
    assert!(!w.can(reader, recipient, P::ThreadManage).await);
    let mut conn = w.conn().await;
    access::testing::give_role(
        &mut conn,
        recipient_topic,
        "group",
        group,
        "developer",
        w.boss,
        None,
    )
    .await
    .unwrap();
    drop(conn);
    assert!(
        w.can(reader, recipient, P::ThreadManage).await,
        "role-subject matching must use the requested group's current mixed source"
    );
    assert!(!w.can(reader, w.thread, P::ThreadManage).await);
    let late = Time::new(end.get() + Duration::seconds(1));
    assert!(!w.can_at(reader, w.thread, P::ThreadView, late).await);
    assert!(!w.can_at(reader, recipient, P::ThreadManage, late).await);
}

#[tokio::test]
async fn mixed_group_provenance_names_every_source_and_ends_with_the_expired_reader_seed() {
    let (w, reader, group) = mixed_group().await;
    let end = w.later(Duration::minutes(1));
    seed(&w, reader, Some(end)).await;
    let mut conn = w.conn().await;
    let principal = Principal::load(&mut conn, Caller::session(reader), w.now)
        .await
        .unwrap();
    let path = holders::chain(&mut conn, &principal, Holder::Group { id: group }, None)
        .await
        .unwrap();
    assert_eq!(
        path,
        vec![
            Holder::Group { id: group },
            viewers(w.topic),
            viewers(w.thread),
            Holder::User { id: reader },
        ],
        "a complete explanation must retain the group, topic, thread and real person"
    );
    let late = Principal::load(
        &mut conn,
        Caller::session(reader),
        Time::new(end.get() + Duration::seconds(1)),
    )
    .await
    .unwrap();
    assert!(
        holders::chain(&mut conn, &late, Holder::Group { id: group }, None)
            .await
            .unwrap()
            .is_empty(),
        "expired source paths cannot remain as current provenance"
    );
}

#[tokio::test]
async fn mixed_group_people_excludes_native_only_viewers_and_preserves_direct_members() {
    let (w, reader, group) = mixed_group().await;
    let direct = w.member("direct-member").await;
    let admin = w.member("native-only").await;
    let mut conn = w.conn().await;
    access::testing::add_to_group(&mut conn, group, direct)
        .await
        .unwrap();
    access::testing::make_system_admin(&mut conn, admin)
        .await
        .unwrap();
    drop(conn);
    // Retain the shipped source and prove native View without assuming the inverse query includes it.
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows":[{"permission":"thread.view","inherit":true}]}),
    )
    .await
    .unwrap();
    seed(&w, reader, None).await;
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
    share_document(&w, document, group).await;
    let mut conn = w.conn().await;
    let candidates = access::group_people(&mut conn, &[group], i64::MAX, w.now.get())
        .await
        .unwrap();
    assert!(candidates.contains(&(group, reader)));
    assert!(candidates.contains(&(group, direct)));
    // Candidate discovery may already exclude the native-only account. The authoritative
    // membership, access and lending results below must exclude it either way.
    drop(conn);
    let people = audience(&w, document, group, w.now).await;
    assert!(people.contains(&reader));
    assert!(people.contains(&direct));
    assert!(
        !people.contains(&admin),
        "native reach cannot make someone a member of an ordinary viewer-derived group"
    );
    assert!(document_holds(&w, direct, document, P::DocView, w.now).await);
    assert!(!document_holds(&w, admin, document, P::DocView, w.now).await);
    let native = Caller::session(admin).with_admin_mode(true);
    let page = w
        .read_as(native.clone(), Target::Resource(object(document)))
        .await;
    assert!(
        page.writable,
        "native human administration remains available"
    );
    assert!(
        page.gives.agent_permissions.is_empty(),
        "native-only source View is not a lendable group right"
    );
    let loan = w
        .change_as(
            native,
            Target::Resource(object(document)),
            json!({"rows":[{
                "permission":"doc.view", "add":[{"holder":Holder::Agents{id:w.thread}}]
            }]}),
            Policy::default(),
        )
        .await;
    assert!(
        loan.is_err(),
        "a Viewer-derived group cannot turn native-only ThreadView into an agent loan"
    );
    let mut conn = w.conn().await;
    let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='agents' AND removed_at IS NULL")
        .bind(document).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(entries, 0);
}

#[tokio::test]
async fn nonviewer_role_groups_preserve_native_membership_and_provenance_without_ordinary_authority()
 {
    let w = World::new().await;
    let admin = w.member("native-role-member").await;
    let group = w.group(w.root, "Administrators of this place", &[]).await;
    let mut conn = w.conn().await;
    access::testing::make_system_admin(&mut conn, admin)
        .await
        .unwrap();
    access::add_group_holder(
        &mut conn,
        group,
        &access::GroupHolderRow {
            user_id: None,
            member_group_id: None,
            member_node_id: Some(w.project),
            member_org_id: None,
            member_role: Some("admin".into()),
        },
        w.boss,
        w.now.get(),
    )
    .await
    .unwrap();
    let ordinary = Principal::load(&mut conn, Caller::session(admin), w.now)
        .await
        .unwrap();
    let native = Principal::load(
        &mut conn,
        Caller::session(admin).with_admin_mode(true),
        w.now,
    )
    .await
    .unwrap();
    assert!(!ordinary.groups().contains(&group));
    assert!(
        native.groups().contains(&group),
        "the pre-existing holder graph has native role identity"
    );
    assert!(
        holders::chain(&mut conn, &ordinary, Holder::Group { id: group }, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        holders::chain(&mut conn, &native, Holder::Group { id: group }, None)
            .await
            .unwrap(),
        vec![
            Holder::Group { id: group },
            Holder::Role {
                role: Role::Admin,
                at: Some(w.project)
            },
            Holder::User { id: admin },
        ]
    );
    drop(conn);
    w.give(w.root, w.maker, Role::Viewer).await;
    let document = private_document(&w).await;
    share_document(&w, document, group).await;
    assert!(audience(&w, document, group, w.now).await.contains(&admin));
    assert!(
        !document_holds(&w, admin, document, P::DocView, w.now).await,
        "native reporting does not create ordinary permission authority"
    );
}

#[tokio::test]
async fn a_mixed_group_cycle_needs_a_live_seed_for_both_access_and_people() {
    let (w, reader, group) = mixed_group().await;
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows":[{"permission":"thread.view","add":[{
            "holder":Holder::Group{id:group}
        }]}]}),
    )
    .await
    .unwrap();
    let document = private_document(&w).await;
    share_document(&w, document, group).await;
    assert!(!document_holds(&w, reader, document, P::DocView, w.now).await);
    assert!(!audience(&w, document, group, w.now).await.contains(&reader));
    let end = w.later(Duration::minutes(1));
    seed(&w, reader, Some(end)).await;
    assert!(
        document_holds(&w, reader, document, P::DocView, w.now).await,
        "an independent current reader seed activates the positive cycle"
    );
    assert!(audience(&w, document, group, w.now).await.contains(&reader));
    let late = Time::new(end.get() + Duration::seconds(1));
    assert!(
        !document_holds(&w, reader, document, P::DocView, late).await,
        "the cycle cannot preserve an expired independent reader seed"
    );
    assert!(!audience(&w, document, group, late).await.contains(&reader));
}
