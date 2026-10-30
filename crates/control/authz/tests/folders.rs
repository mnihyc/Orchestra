//! Projects inside projects as the access engine sees them: a role, a default and a lock of a project reaching the folders inside it, the project of a thread being the nearest, what archive closes, and what a project-wide reach passes over.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{Permission as P, Profile};
use orch_access_contract::roles::Role;
use orch_authz::reach::{Agent, AgentReach, Reach, Things};
use orch_authz::{Act, Caller, Error, Policy, nodes};
use orch_db::queries::hierarchy::archive;
use orch_types::Id;
use support::World;

/// The world's project Web, with two folders in it, Checkout and Docs, and in Checkout one more,
/// Payments; a topic in each of the four projects, and a thread in the topic of Payments.
struct Folders {
    web: Id,
    checkout: Id,
    payments: Id,
    docs: Id,
    plan: Id,
    cart: Id,
    refunds: Id,
    notes: Id,
    thread: Id,
}

impl World {
    async fn folders(&self) -> Folders {
        let web = self.project;
        let checkout = self.node("project", web, "Checkout").await;
        let payments = self.node("project", checkout, "Payments").await;
        let docs = self.node("project", web, "Docs").await;
        let plan = self.node("topic", web, "Plan").await;
        let cart = self.node("topic", checkout, "Cart").await;
        let refunds = self.node("topic", payments, "Refunds").await;
        let notes = self.node("topic", docs, "Notes").await;
        let thread = self.node("thread", refunds, "Repro").await;
        Folders {
            web,
            checkout,
            payments,
            docs,
            plan,
            cart,
            refunds,
            notes,
            thread,
        }
    }
}

#[tokio::test]
async fn a_role_on_a_project_reaches_every_project_and_topic_in_it_whatever_the_role_gives() {
    let w = World::new().await;
    let f = w.folders().await;
    let table = [
        (Role::Viewer, f.checkout, P::ProjectView, true),
        (Role::Viewer, f.payments, P::ProjectView, true),
        (Role::Viewer, f.refunds, P::TopicView, true),
        (Role::Viewer, f.thread, P::ThreadView, true),
        (Role::Viewer, f.payments, P::ProjectInteract, false),
        (Role::Viewer, f.thread, P::ThreadInteract, false),
        (Role::Interactor, f.thread, P::ThreadInteract, true),
        (Role::Interactor, f.payments, P::ProjectInteract, false),
        (Role::Developer, f.checkout, P::ProjectInteract, true),
        (Role::Developer, f.payments, P::ProjectInteract, true),
        (Role::Developer, f.refunds, P::TopicManage, true),
        (Role::Developer, f.thread, P::ThreadManage, true),
        (Role::Developer, f.payments, P::ProjectManage, false),
        (Role::Admin, f.checkout, P::ProjectManage, true),
        (Role::Admin, f.payments, P::ProjectManage, true),
        (Role::Admin, f.refunds, P::TopicManage, true),
    ];
    for (index, (role, node, permission, expected)) in table.into_iter().enumerate() {
        let person = w.user(&format!("person{index}")).await;
        w.give(f.web, person, role).await;
        assert_eq!(
            w.can(person, node, permission).await,
            expected,
            "{} on Web, {permission:?} on a node inside it",
            role.as_str()
        );
    }
}

#[tokio::test]
async fn a_role_on_one_folder_reaches_the_projects_in_it_and_neither_its_parent_nor_a_project_beside_it()
 {
    let w = World::new().await;
    let f = w.folders().await;
    let person = w.user("dev").await;
    w.give(f.checkout, person, Role::Developer).await;
    for (what, node, permission, expected) in [
        ("the folder itself", f.checkout, P::ProjectInteract, true),
        ("a folder in it", f.payments, P::ProjectInteract, true),
        ("a topic of the folder", f.cart, P::TopicManage, true),
        (
            "a topic of the folder in it",
            f.refunds,
            P::TopicManage,
            true,
        ),
        ("a thread of that", f.thread, P::ThreadManage, true),
        ("its parent project", f.web, P::ProjectView, false),
        ("a folder beside it", f.docs, P::ProjectView, false),
        ("a topic of its parent", f.plan, P::TopicView, false),
        (
            "a topic of the folder beside it",
            f.notes,
            P::TopicView,
            false,
        ),
        ("the root org", w.root, P::OrgView, false),
    ] {
        assert_eq!(
            w.can(person, node, permission).await,
            expected,
            "a Developer of Checkout on {what}"
        );
    }
}

#[tokio::test]
async fn the_page_of_a_folder_follows_the_defaults_of_the_project_it_lies_in_and_a_lock_there_holds_it()
 {
    let w = World::new().await;
    let f = w.folders().await;
    let developer = w.user("dev").await;
    w.give(f.web, developer, Role::Developer).await;
    // As shipped only Admins manage a project, and nobody here is one.
    for project in [f.web, f.checkout, f.payments] {
        assert!(!w.can(developer, project, P::ProjectManage).await);
    }
    // Web lets its Developers manage what is inside it: its default names them.
    w.name_default(
        Some(f.web),
        P::ProjectManage,
        "any",
        Holder::Role {
            role: Role::Developer,
            at: None,
        },
    )
    .await;
    assert!(
        w.can(developer, f.checkout, P::ProjectManage).await,
        "a folder follows it"
    );
    assert!(
        w.can(developer, f.payments, P::ProjectManage).await,
        "and so does the one in it"
    );
    assert!(
        !w.can(developer, f.web, P::ProjectManage).await,
        "Web's own page follows the defaults above it, not its own"
    );

    // Checkout cuts its own row loose: its page is its owners' alone; the folder in it follows
    // the defaults of Checkout, which are not its page.
    w.row(f.checkout, P::ProjectManage, false).await;
    assert!(!w.can(developer, f.checkout, P::ProjectManage).await);
    assert!(w.can(developer, f.payments, P::ProjectManage).await);

    // Web locks the row: the page that was cut loose follows it while it holds.
    w.default_row(f.web, P::ProjectManage, "any", true, true)
        .await;
    assert!(
        w.can(developer, f.checkout, P::ProjectManage).await,
        "the lock decides"
    );
    // Lifted, the page is as it was.
    w.default_row(f.web, P::ProjectManage, "any", true, false)
        .await;
    assert!(!w.can(developer, f.checkout, P::ProjectManage).await);
    // A lock in Checkout's own defaults holds the folder in it, whatever Payments stored.
    w.row(f.payments, P::ProjectManage, false).await;
    assert!(!w.can(developer, f.payments, P::ProjectManage).await);
    w.default_row(f.checkout, P::ProjectManage, "any", true, true)
        .await;
    assert!(w.can(developer, f.payments, P::ProjectManage).await);
}

#[tokio::test]
async fn a_default_changed_on_a_project_is_followed_by_the_pages_of_the_folders_and_topics_inside_it()
 {
    let w = World::new().await;
    let f = w.folders().await;
    let count = |place: Id, permission: P| {
        let w = &w;
        async move {
            let mut conn = w.conn().await;
            orch_authz::defaults::followers(&mut conn, place, &[(permission, Profile::Any)])
                .await
                .unwrap()
        }
    };
    // Web's own topic (Login) and the four in its folders; the three folders.
    assert_eq!(count(f.web, P::TopicView).await, 5);
    assert_eq!(count(f.web, P::ProjectView).await, 3);
    assert_eq!(count(f.checkout, P::TopicView).await, 2);
    assert_eq!(count(f.docs, P::ProjectView).await, 0);
    // A folder that cut the row loose, and the topic of another, leave it.
    w.row(f.payments, P::ProjectView, false).await;
    w.row(f.refunds, P::TopicView, false).await;
    assert_eq!(count(f.web, P::ProjectView).await, 2);
    assert_eq!(count(f.web, P::TopicView).await, 4);
    // Checkout stopped following Web's topic row for what lies in it: nothing in it follows Web.
    w.default_row(f.checkout, P::TopicView, "any", false, false)
        .await;
    assert_eq!(count(f.web, P::TopicView).await, 3, "Login, Plan and Notes");
}

async fn archived_below(w: &World, node: Id) -> Option<Id> {
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    match nodes::check(
        &mut conn,
        &principal,
        node,
        P::TopicInteract,
        Act::Use,
        Policy::default(),
    )
    .await
    {
        Err(Error::Archived { node }) => Some(node),
        other => {
            assert!(
                other.unwrap().is_allowed(),
                "a boss holds it where nothing is archived"
            );
            None
        }
    }
}

#[tokio::test]
async fn an_archived_project_closes_what_is_inside_it_and_nothing_beside_or_above() {
    let w = World::new().await;
    let f = w.folders().await;
    {
        let mut conn = w.conn().await;
        assert!(
            archive(&mut conn, f.checkout, w.boss, w.now.get())
                .await
                .unwrap()
        );
    }
    assert_eq!(
        archived_below(&w, f.cart).await,
        Some(f.checkout),
        "its own topic"
    );
    assert_eq!(
        archived_below(&w, f.refunds).await,
        Some(f.checkout),
        "a topic of a folder in it"
    );
    assert_eq!(
        archived_below(&w, f.plan).await,
        None,
        "a topic of the project above"
    );
    assert_eq!(
        archived_below(&w, f.notes).await,
        None,
        "a topic of a folder beside it"
    );
    // It is still seen.
    let person = w.user("viewer").await;
    w.give(f.web, person, Role::Viewer).await;
    assert!(w.can(person, f.payments, P::ProjectView).await);
    assert!(w.can(person, f.refunds, P::TopicView).await);
    // A folder inside archives nearer: the nearest archived node is named.
    {
        let mut conn = w.conn().await;
        assert!(
            archive(&mut conn, f.payments, w.boss, w.now.get())
                .await
                .unwrap()
        );
    }
    assert_eq!(archived_below(&w, f.refunds).await, Some(f.payments));
    assert_eq!(archived_below(&w, f.cart).await, Some(f.checkout));
}

#[tokio::test]
async fn what_is_made_private_below_a_project_is_told_from_what_follows_it() {
    let w = World::new().await;
    let f = w.folders().await;
    let asked = [
        f.checkout, f.payments, f.docs, f.plan, f.cart, f.refunds, f.notes,
    ];
    let below = |place: Id, ids: Vec<Id>| {
        let w = &w;
        async move {
            let principal = w.principal(Caller::session(w.boss)).await;
            let mut conn = w.conn().await;
            let mut found: Vec<Id> =
                nodes::restricted_below(&mut conn, &principal, &ids, place, Policy::default())
                    .await
                    .unwrap()
                    .into_iter()
                    .collect();
            found.sort();
            found
        }
    };
    let sorted = |mut ids: Vec<Id>| {
        ids.sort();
        ids
    };
    assert!(
        below(f.web, asked.to_vec()).await.is_empty(),
        "everything follows Web"
    );

    // Docs cuts its own page loose: it does not follow Web; the topic in it follows Docs's defaults.
    w.row(f.docs, P::ProjectView, false).await;
    assert_eq!(below(f.web, asked.to_vec()).await, vec![f.docs]);
    // A topic cut loose is private too.
    w.row(f.plan, P::TopicView, false).await;
    assert_eq!(
        below(f.web, asked.to_vec()).await,
        sorted(vec![f.docs, f.plan])
    );
    // Made private as a whole, a folder's defaults are cut loose with its page, and what lies in
    // it does not follow what is above it any more.
    w.row(f.checkout, P::ProjectView, false).await;
    w.default_row(f.checkout, P::ProjectView, "any", false, false)
        .await;
    w.default_row(f.checkout, P::TopicView, "any", false, false)
        .await;
    assert_eq!(
        below(f.web, asked.to_vec()).await,
        sorted(vec![
            f.docs, f.plan, f.checkout, f.payments, f.cart, f.refunds
        ])
    );
    // Seen from Checkout itself, which is private, what lies in it follows it: nothing is passed over.
    assert!(
        below(f.checkout, vec![f.payments, f.cart, f.refunds])
            .await
            .is_empty(),
        "the sender is the place the others are asked against"
    );
}

#[tokio::test]
async fn the_project_of_a_thread_is_the_nearest_and_the_project_policy_reaches_the_folders_inside_it_and_not_those_above()
 {
    let w = World::new().await;
    let f = w.folders().await;
    // A topic in a folder inside Payments, with a thread; a thread beside the agent's, in its topic;
    // a thread in Cart, which lies in Checkout above Payments; one in Plan, in Web above that; one in
    // Notes, in a folder beside Checkout.
    let cards = w.node("project", f.payments, "Cards").await;
    let swipes = w.node("topic", cards, "Swipes").await;
    let inside = w.node("thread", swipes, "Terminal").await;
    let beside = w.node("thread", f.refunds, "Another").await;
    let above = w.node("thread", f.cart, "Basket").await;
    let higher = w.node("thread", f.plan, "Draft").await;
    let across = w.node("thread", f.notes, "Wiki").await;

    let reach_of = |thread: Id, policy: AgentReach, things: Vec<Id>| {
        let w = &w;
        async move {
            let mut conn = w.conn().await;
            let agent = Agent::of(&mut conn, thread).await.unwrap().unwrap();
            Reach::load(
                &mut conn,
                agent,
                policy,
                w.now,
                &Things {
                    nodes: things,
                    ..Things::default()
                },
            )
            .await
            .unwrap()
        }
    };
    let agent = {
        let mut conn = w.conn().await;
        Agent::of(&mut conn, f.thread).await.unwrap().unwrap()
    };
    assert_eq!(
        agent.project,
        Some(f.payments),
        "the project of a thread is the one its topic lies in"
    );
    assert_eq!(agent.topic, f.refunds);

    let all = vec![inside, beside, above, higher, across];
    let by_project = reach_of(f.thread, AgentReach::Project, all.clone()).await;
    for (what, thread, expected) in [
        ("another thread of its topic", beside, true),
        ("a thread of a folder inside its project", inside, true),
        ("a thread of the project above", above, false),
        ("a thread of the project above that", higher, false),
        ("a thread of a project beside the one above", across, false),
    ] {
        assert_eq!(
            by_project.conversation(thread, false).read.is_some(),
            expected,
            "with the policy project, {what}"
        );
    }
    let by_topic = reach_of(f.thread, AgentReach::Topic, all.clone()).await;
    assert!(by_topic.conversation(beside, false).read.is_some());
    assert!(
        by_topic.conversation(inside, false).read.is_none(),
        "the policy topic reaches its own topic alone"
    );
    let by_none = reach_of(f.thread, AgentReach::None, all).await;
    assert!(by_none.conversation(inside, false).read.is_none());

    // From the thread in Cart, whose project is Checkout, the whole of Checkout is reached down to
    // Payments and Cards; Plan above and Notes beside are not.
    let from_cart = reach_of(
        above,
        AgentReach::Project,
        vec![f.thread, inside, higher, across],
    )
    .await;
    assert!(from_cart.conversation(f.thread, false).read.is_some());
    assert!(from_cart.conversation(inside, false).read.is_some());
    assert!(from_cart.conversation(higher, false).read.is_none());
    assert!(from_cart.conversation(across, false).read.is_none());
}
