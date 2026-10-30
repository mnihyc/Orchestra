//! What a role held in a place, or a share of it with what is in it, reaches: the places, threads and documents inside that follow the place, and those made private or cut loose throughout, counted as far as the caller may see them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::Role;
use orch_authz::covers::{self, Counts, Covers};
use orch_authz::{Caller, Error, Policy};
use orch_db::queries::documents as rows;
use orch_types::{Id, new_id};
use support::{Statements, World};

async fn document_beside(w: &World, node: Id, title: &str) -> Id {
    let mut conn = w.conn().await;
    let id = new_id();
    rows::insert(
        &mut conn,
        &rows::NewDocument {
            id,
            node_id: node,
            org_id: w.root,
            owner_id: w.boss,
            created_by_thread: None,
            created_by_agent: None,
            title,
            now: w.now.get(),
        },
    )
    .await
    .unwrap();
    orch_authz::documents::start(&mut conn, id, w.boss, w.now)
        .await
        .unwrap();
    id
}

async fn covers_for(w: &World, user: Id, place: Id) -> Option<Covers> {
    let principal = w.principal(Caller::session(user)).await;
    let mut conn = w.conn().await;
    covers::of_place(&mut conn, &principal, place, Policy::default())
        .await
        .unwrap()
}

const fn counts(places: i64, threads: i64, documents: i64) -> Counts {
    Counts {
        places,
        threads,
        documents,
    }
}

/// The project Web with its topic Login and the thread Fix, and beside them: a topic that follows with
/// two threads, a folder with a topic and a thread in it, a topic made private throughout with a
/// thread in it, a thread made private inside a topic that follows, and documents of each kind.
struct Place {
    folder_topic_thread: Id,
}

async fn build(w: &World) -> Place {
    let open = w.node("topic", w.project, "Open").await;
    w.node("thread", open, "One").await;
    w.node("thread", open, "Two").await;
    let private_thread = w.node_owned_by("thread", open, "Closed", w.boss).await;
    w.row(private_thread, P::ThreadView, false).await;
    let folder = w.node("project", w.project, "Folder").await;
    let folder_topic = w.node("topic", folder, "Deep").await;
    let folder_topic_thread = w.node("thread", folder_topic, "Deeper").await;
    let private = w.node("topic", w.project, "Secret").await;
    w.row(private, P::TopicView, false).await;
    w.row(private, P::DocView, false).await;
    w.default_row(private, P::ThreadView, "managed", false, false)
        .await;
    w.node_owned_by("thread", private, "Hidden", w.boss).await;
    // Documents: one beside the project that follows and one made private, one beside a topic that
    // follows and one made private, one beside the topic that is private throughout.
    document_beside(w, w.project, "Project handbook").await;
    let made_private = document_beside(w, w.project, "Project secrets").await;
    w.thing_row("document", made_private, P::DocView, false)
        .await;
    document_beside(w, open, "Notes").await;
    let made_private = document_beside(w, open, "Drafts").await;
    w.thing_row("document", made_private, P::DocView, false)
        .await;
    document_beside(w, private, "Minutes").await;
    Place {
        folder_topic_thread,
    }
}

#[tokio::test]
async fn a_place_says_how_many_places_threads_and_documents_follow_it_and_how_many_do_not() {
    let w = World::new().await;
    let built = build(&w).await;
    assert!(
        w.can(w.boss, built.folder_topic_thread, P::ThreadView)
            .await
    );
    // The owner of the place sees all of it: four places follow (Login, Open, Folder, Deep), one does
    // not (Secret); four threads (Fix, One, Two, Deeper), two do not (Closed, Hidden); two documents
    // follow (the project's handbook, the notes), three do not.
    let seen = covers_for(&w, w.boss, w.project).await.unwrap();
    assert_eq!(seen.follows, counts(4, 4, 2), "{seen:?}");
    assert_eq!(seen.apart, counts(1, 2, 3), "{seen:?}");
}

#[tokio::test]
async fn what_the_caller_may_not_see_is_counted_in_neither_count() {
    let w = World::new().await;
    build(&w).await;
    let viewer = w.member("viewer").await;
    w.give(w.project, viewer, Role::Viewer).await;
    // A Viewer of the project is not let into what was made private: it is not theirs to count.
    let seen = covers_for(&w, viewer, w.project).await.unwrap();
    assert_eq!(seen.follows, counts(4, 4, 2), "{seen:?}");
    assert_eq!(seen.apart, Counts::default(), "{seen:?}");
    // The owner of four threads sees those four, and not the places, the documents and the two threads
    // that the owner of the place made private.
    let alone = covers_for(&w, w.maker, w.project).await.unwrap();
    assert_eq!(alone.follows, counts(0, 4, 0), "{alone:?}");
    assert_eq!(alone.apart, Counts::default());
}

#[tokio::test]
async fn orgs_inside_orgs_are_walked_by_what_lies_inside_and_not_by_a_fixed_depth() {
    let w = World::new().await;
    let d = w.deep().await;
    // Research holds ML, Vision and Ops (three orgs), Detect (a project), its two topics, and the
    // thread Train; the two projects of other departments are beside it and not inside.
    let seen = covers_for(&w, w.boss, d.d1).await.unwrap();
    assert_eq!(seen.follows, counts(6, 1, 0), "{seen:?}");
    assert_eq!(seen.apart, Counts::default());
    // The root org holds all of it and Sales with its project, and the original project and topic.
    let all = covers_for(&w, w.boss, w.root).await.unwrap();
    assert_eq!(all.follows.threads, 2, "Fix and Train");
    assert_eq!(
        all.follows.places,
        5 + 3 + 3,
        "five orgs, three projects and three topics"
    );
}

#[tokio::test]
async fn a_thread_is_no_place_and_has_nothing_inside() {
    let w = World::new().await;
    assert!(covers_for(&w, w.boss, w.thread).await.is_none());
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    assert!(matches!(
        covers::of_place(&mut conn, &principal, new_id(), Policy::default()).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn counting_a_place_takes_the_same_statements_for_a_thousand_threads_as_for_a_few() {
    let w = World::new().await;
    let few = w.node("topic", w.project, "Few").await;
    for number in 0..3 {
        w.node("thread", few, &format!("t{number}")).await;
    }
    let before = Statements::now();
    let small = covers_for(&w, w.boss, few).await.unwrap();
    let small_statements = Statements::now() - before;
    assert_eq!(small.follows.threads, 3);

    let many = w.node("topic", w.project, "Many").await;
    let mut conn = w.conn().await;
    // A thousand threads in one go, as the statements of the world's own helper, to keep the test quick.
    for number in 0..1000 {
        let id = new_id();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            id,
            "thread",
            Some(many),
            &format!("thread {number}"),
            w.maker,
        )
        .await
        .unwrap();
    }
    drop(conn);
    let started = std::time::Instant::now();
    let before = Statements::now();
    let big = covers_for(&w, w.boss, many).await.unwrap();
    let statements = Statements::now() - before;
    eprintln!(
        "covers of a place with 1000 threads: {statements} statements, {:?}",
        started.elapsed()
    );
    assert_eq!(big.follows.threads, 1000);
    assert_eq!(
        statements, small_statements,
        "a thousand threads cost the statements of three"
    );
}
