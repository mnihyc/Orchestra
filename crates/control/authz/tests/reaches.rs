//! A thing, or a place with what is in it: what a role held in a place, a name on a place's own page and a name on one document each reach, for a person, for a group and for somebody of another root org.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{ObjectKind, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::pages::Target;
use orch_authz::{Caller, Policy, documents};
use orch_db::queries::access::testing::give_role;
use orch_db::queries::documents as rows;
use orch_types::{Id, new_id};
use serde_json::json;
use support::{World, group_holder, user_holder};

/// The three kinds of holder a page can name or a role can be given to: a person of the tree, a group
/// of it, and somebody of another root org.
#[derive(Clone, Copy, Debug)]
enum Holder {
    Person,
    Group,
    Outsider,
}

const HOLDERS: [Holder; 3] = [Holder::Person, Holder::Group, Holder::Outsider];

/// The user who holds as `holder` does, and what names them on a page or gives them a role.
struct Named {
    user: Id,
    /// How a page change names them.
    holder: serde_json::Value,
    /// The subject kind and identifier a role is given to.
    subject: (&'static str, Id),
}

async fn named(w: &World, holder: Holder, name: &str) -> Named {
    match holder {
        Holder::Person => {
            let user = w.member(name).await;
            Named {
                user,
                holder: user_holder(user),
                subject: ("user", user),
            }
        }
        Holder::Group => {
            let user = w.member(name).await;
            let group = w.group(w.root, &format!("{name} team"), &[user]).await;
            Named {
                user,
                holder: group_holder(group),
                subject: ("group", group),
            }
        }
        Holder::Outsider => {
            let user = w.user(name).await;
            Named {
                user,
                holder: user_holder(user),
                subject: ("user", user),
            }
        }
    }
}

async fn give(w: &World, place: Id, who: &Named, role: Role) {
    let mut conn = w.conn().await;
    give_role(
        &mut conn,
        place,
        who.subject.0,
        who.subject.1,
        role.as_str(),
        w.boss,
        None,
    )
    .await
    .unwrap();
}

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
    documents::start(&mut conn, id, w.boss, w.now)
        .await
        .unwrap();
    id
}

async fn reads(w: &World, user: Id, document: Id) -> bool {
    let principal = w.principal(Caller::session(user)).await;
    let mut conn = w.conn().await;
    matches!(
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
    )
}

fn page_of(document: Id) -> Target {
    Target::Resource(ObjectRef {
        kind: ObjectKind::Document,
        id: document,
    })
}

#[tokio::test]
async fn a_role_given_in_a_place_reaches_what_is_made_inside_later_and_not_what_was_made_private() {
    for holder in HOLDERS {
        let w = World::new().await;
        let who = named(&w, holder, "pia").await;
        // What is inside the project already: a private topic with a thread and a document in it.
        let private = w.node("topic", w.project, "Private").await;
        w.row(private, P::TopicView, false).await;
        w.default_row(private, P::ThreadView, "managed", false, false)
            .await;
        let hidden = w.node("thread", private, "Hidden").await;
        let secret = document_beside(&w, private, "Secret").await;
        w.thing_row("document", secret, P::DocView, false).await;
        give(&w, w.project, &who, Role::Viewer).await;
        // What is made afterwards follows the defaults, and the role reaches it: a topic, a thread, a document.
        let later_topic = w.node("topic", w.project, "Later").await;
        let later_thread = w.node("thread", later_topic, "Fresh").await;
        let later_document = document_beside(&w, later_topic, "Minutes").await;
        let label = format!("{holder:?}");
        assert!(
            w.can(who.user, w.topic, P::TopicView).await,
            "{label}: a topic that was there"
        );
        assert!(
            w.can(who.user, w.thread, P::ThreadView).await,
            "{label}: a thread that was there"
        );
        assert!(
            w.can(who.user, later_topic, P::TopicView).await,
            "{label}: a topic made later"
        );
        assert!(
            w.can(who.user, later_thread, P::ThreadView).await,
            "{label}: a thread made later"
        );
        assert!(
            reads(&w, who.user, later_document).await,
            "{label}: a document made later"
        );
        assert!(
            !w.can(who.user, private, P::TopicView).await,
            "{label}: a private topic is not reached"
        );
        assert!(
            !w.can(who.user, hidden, P::ThreadView).await,
            "{label}: nor the thread in it"
        );
        assert!(
            !reads(&w, who.user, secret).await,
            "{label}: nor a document made private"
        );
        // The role is a Viewer's: it reaches no further than what the pages give a Viewer.
        assert!(
            !w.can(who.user, later_thread, P::ThreadInteract).await,
            "{label}"
        );
    }
}

#[tokio::test]
async fn naming_somebody_on_a_places_own_page_opens_nothing_inside_it() {
    for holder in HOLDERS {
        let w = World::new().await;
        let who = named(&w, holder, "pia").await;
        let beside_the_project = document_beside(&w, w.project, "Handbook").await;
        let inside = document_beside(&w, w.topic, "Minutes").await;
        w.change(
            w.boss,
            Target::Live(w.project),
            json!({ "name": [{ "holders": [who.holder.clone()], "choice": "full_view" }] }),
        )
        .await
        .unwrap();
        let label = format!("{holder:?}");
        // The place itself, its chat and the documents written beside it.
        assert!(
            w.can(who.user, w.project, P::ProjectView).await,
            "{label}: the place"
        );
        assert!(
            w.can(who.user, w.project, P::ChatView).await,
            "{label}: its chat"
        );
        assert!(
            reads(&w, who.user, beside_the_project).await,
            "{label}: a document beside it"
        );
        // And nothing inside it.
        assert!(
            !w.can(who.user, w.topic, P::TopicView).await,
            "{label}: a topic inside"
        );
        assert!(
            !w.can(who.user, w.topic, P::ChatView).await,
            "{label}: its chat"
        );
        assert!(
            !w.can(who.user, w.thread, P::ThreadView).await,
            "{label}: a thread inside"
        );
        assert!(
            !reads(&w, who.user, inside).await,
            "{label}: a document beside a topic inside"
        );
    }
}

#[tokio::test]
async fn naming_somebody_on_one_document_opens_neither_its_place_nor_another_document() {
    for holder in HOLDERS {
        let w = World::new().await;
        let who = named(&w, holder, "pia").await;
        let shared = document_beside(&w, w.topic, "Shared").await;
        let beside = document_beside(&w, w.topic, "Beside").await;
        w.change(
            w.boss,
            page_of(shared),
            json!({ "name": [{ "holders": [who.holder.clone()], "choice": "full_view" }] }),
        )
        .await
        .unwrap();
        let label = format!("{holder:?}");
        assert!(reads(&w, who.user, shared).await, "{label}: the document");
        assert!(
            !reads(&w, who.user, beside).await,
            "{label}: another document beside it"
        );
        assert!(
            !w.can(who.user, w.topic, P::TopicView).await,
            "{label}: the topic it lies beside"
        );
        assert!(
            !w.can(who.user, w.topic, P::DocView).await,
            "{label}: the documents of the topic"
        );
        assert!(
            !w.can(who.user, w.thread, P::ThreadView).await,
            "{label}: a thread there"
        );
    }
}
