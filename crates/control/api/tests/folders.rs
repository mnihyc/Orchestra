//! Projects inside projects over HTTP: a folder made, listed, counted and moved, and a topic moved to another project or folder with its threads, its documents, its environments' homes and its use; and what each is refused for.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use chrono::{NaiveDate, TimeZone, Utc};
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::directory::{NodeTagCreate, NodeTagCreateBody, NodeTagList};
use orch_api_contract::documents::{
    NodeDocumentCreate, NodeDocumentCreateBody, NodeDocumentList, NodeDocumentListQuery,
};
use orch_api_contract::nodes::{
    MovePreviewBody, NewNodeAccess, Node, NodeActivityList, NodeActivityListQuery, NodeArchive,
    NodeMoveBody, NodeRef, OrgTreeGet, OrgTreeGetQuery, PageState, Project, ProjectChildCreate,
    ProjectCreate, ProjectCreateBody, ProjectMove, ProjectMovePreview, Topic, TopicList,
    TopicListQuery, TopicMove, TopicMovePreview, TopicUpdate, TopicUpdateBody,
};
use orch_api_contract::usage::{UsageAnalytics, UsageGet, UsageGetQuery};
use orch_db::queries::environments::hosting;
use orch_db::queries::hierarchy::testing::{insert_node, put_thread_in_environment};
use orch_db::usage::ledger::{self, Attribution, NewEvent, Tokens, record};
use orch_settings::Target;
use orch_types::{Id, Json, new_id};
use places::{
    Who, first_missing, join, make_org, make_project, make_topic, missing_permissions, person,
    read, told,
};
use serde_json::json;
use support::World;

/// Acme with a project, Web, and in it a topic, Plan; a Developer and a Viewer of the whole org,
/// and the boss who owns it.
struct Acme {
    boss: Who,
    dev: Who,
    viewer: Who,
    acme: Id,
    web: Id,
    plan: Id,
}

async fn acme(world: &World) -> Acme {
    let boss = person(world, "boss").await;
    let org = make_org(world, &boss, "Acme", "acme").await.base.id;
    let dev = person(world, "dev").await;
    join(world, org, org, &dev, "developer", &boss).await;
    let viewer = person(world, "viewer").await;
    join(world, org, org, &viewer, "viewer", &boss).await;
    let web = make_project(world, &boss, org, "Web").await.base.id;
    let plan = make_topic(world, &boss, web, "Plan").await.base.id;
    Acme {
        boss,
        dev,
        viewer,
        acme: org,
        web,
        plan,
    }
}

fn folder_body(name: &str) -> ProjectCreateBody {
    ProjectCreateBody {
        name: name.to_owned(),
        description: None,
        access: None,
    }
}

async fn make_folder(world: &World, by: &Who, project: Id, name: &str) -> Project {
    world
        .app
        .call(ProjectChildCreate, &[&project])
        .token(&by.token)
        .body(&folder_body(name))
        .send()
        .await
        .ok()
}

async fn folder_refused(world: &World, by: &Who, project: Id, name: &str) -> Problem {
    world
        .app
        .call(ProjectChildCreate, &[&project])
        .token(&by.token)
        .body(&folder_body(name))
        .send()
        .await
        .problem()
}

async fn revision(world: &World, who: &Who, id: Id) -> i64 {
    match read(world, who, id).await {
        Node::Org(node) => node.base.revision,
        Node::Project(node) => node.base.revision,
        Node::Topic(node) => node.base.revision,
        Node::Thread(node) => node.base.revision,
    }
}

fn ids(path: &[NodeRef]) -> Vec<Id> {
    path.iter().map(|each| each.id).collect()
}

async fn project_moved(world: &World, by: &Who, node: Id, parent: Id) -> Project {
    let expected_revision = revision(world, by, node).await;
    world
        .app
        .call(ProjectMove, &[&node])
        .token(&by.token)
        .body(&NodeMoveBody {
            parent_id: parent,
            preflight_id: None,
            expected_revision,
        })
        .send()
        .await
        .ok()
}

async fn project_move_refused(world: &World, by: &Who, node: Id, parent: Id) -> Problem {
    let expected_revision = revision(world, by, node).await;
    world
        .app
        .call(ProjectMove, &[&node])
        .token(&by.token)
        .body(&NodeMoveBody {
            parent_id: parent,
            preflight_id: None,
            expected_revision,
        })
        .send()
        .await
        .problem()
}

async fn topic_moved(world: &World, by: &Who, node: Id, parent: Id) -> Topic {
    let expected_revision = revision(world, by, node).await;
    world
        .app
        .call(TopicMove, &[&node])
        .token(&by.token)
        .body(&NodeMoveBody {
            parent_id: parent,
            preflight_id: None,
            expected_revision,
        })
        .send()
        .await
        .ok()
}

async fn topic_move_refused(world: &World, by: &Who, node: Id, parent: Id) -> Problem {
    let expected_revision = revision(world, by, node).await;
    world
        .app
        .call(TopicMove, &[&node])
        .token(&by.token)
        .body(&NodeMoveBody {
            parent_id: parent,
            preflight_id: None,
            expected_revision,
        })
        .send()
        .await
        .problem()
}

async fn plan_of_topic(
    world: &World,
    by: &Who,
    node: Id,
    parent: Id,
) -> orch_api_contract::nodes::NodeMovePlan {
    world
        .app
        .call(TopicMovePreview, &[&node])
        .token(&by.token)
        .body(&MovePreviewBody { parent_id: parent })
        .send()
        .await
        .ok()
}

async fn path_of(world: &World, node: Id) -> Vec<Id> {
    let mut conn = world.conn().await;
    sqlx::query_scalar("SELECT path FROM nodes WHERE id = $1")
        .bind(node)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}

// ---------------------------------------------------------------- a folder is made

#[tokio::test]
async fn a_folder_is_made_by_whoever_opens_topics_in_a_project_and_its_maker_owns_it() {
    let world = World::new().await;
    let a = acme(&world).await;
    let folder = make_folder(&world, &a.dev, a.web, "Checkout").await;
    assert_eq!(folder.base.parent_id, Some(a.web));
    assert_eq!(folder.base.org_id, a.acme);
    assert_eq!(ids(&folder.base.path), vec![a.acme, a.web, folder.base.id]);
    assert_eq!(
        folder.base.owner.username, "dev",
        "its maker is its first owner"
    );
    assert_eq!(folder.base.owners, 1);
    assert!(folder.base.access.owns);
    assert_eq!(
        folder.base.page,
        PageState::Follows,
        "its page follows its parent's defaults"
    );
    assert_eq!((folder.topic_count, folder.project_count), (0, 0));
    // The project it lies in counts it; its topics are still counted apart.
    let Node::Project(web) = read(&world, &a.boss, a.web).await else {
        panic!("a project stays one");
    };
    assert_eq!((web.topic_count, web.project_count), (1, 1));
    // A viewer opens no topic there and so makes no folder, and is told what they lack.
    let refused = folder_refused(&world, &a.viewer, a.web, "Nope").await;
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["project.interact"]);
    assert_eq!(told(&refused)["required"], "project.interact");
    // A project in an org stays with the org's admins: the same Developer makes none there.
    let in_the_org = world
        .app
        .call(ProjectCreate, &[&a.acme])
        .token(&a.dev.token)
        .body(&folder_body("Mobile"))
        .send()
        .await
        .problem();
    assert_eq!(in_the_org.code, ErrorCode::AuthzDenied);
    assert_eq!(told(&in_the_org)["required"], "org.interact");
    // A folder is made in a project, and in nothing else.
    for not_a_project in [a.plan, a.acme] {
        let refused = folder_refused(&world, &a.boss, not_a_project, "Odd").await;
        assert_eq!(refused.code, ErrorCode::NotFound, "{not_a_project}");
    }
    // Folders nest: one is made in a folder in the same form.
    let inner = make_folder(&world, &a.dev, folder.base.id, "Payments").await;
    assert_eq!(
        ids(&inner.base.path),
        vec![a.acme, a.web, folder.base.id, inner.base.id]
    );
    let Node::Project(outer) = read(&world, &a.boss, folder.base.id).await else {
        panic!("a project stays one");
    };
    assert_eq!(outer.project_count, 1);
}

#[tokio::test]
async fn a_folder_made_private_is_its_makers_alone_until_somebody_is_named() {
    let world = World::new().await;
    let a = acme(&world).await;
    let folder = world
        .app
        .call(ProjectChildCreate, &[&a.web])
        .token(&a.dev.token)
        .body(&ProjectCreateBody {
            access: Some(NewNodeAccess::Private),
            ..folder_body("Secret")
        })
        .send()
        .await
        .ok();
    assert_eq!(folder.base.page, PageState::Restricted);
    let seen = world
        .app
        .call(orch_api_contract::nodes::NodeGet, &[&folder.base.id])
        .token(&a.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(
        seen.code,
        ErrorCode::AuthzDenied,
        "nobody who only holds a role above it reads it"
    );
    let own = read(&world, &a.dev, folder.base.id).await;
    assert!(matches!(own, Node::Project(_)));
}

#[tokio::test]
async fn a_folder_is_listed_in_its_parent_and_in_the_tree_and_the_topics_of_a_project_are_its_own()
{
    let world = World::new().await;
    let a = acme(&world).await;
    let checkout = make_folder(&world, &a.dev, a.web, "Checkout").await.base.id;
    let payments = make_folder(&world, &a.dev, checkout, "Payments")
        .await
        .base
        .id;
    let cart = make_topic(&world, &a.dev, checkout, "Cart").await.base.id;
    let refunds = make_topic(&world, &a.dev, payments, "Refunds")
        .await
        .base
        .id;
    // What lies directly in Web: its topic and its folder.
    let inside = places::children(&world, &a.boss, a.web).await;
    assert_eq!(inside.len(), 2);
    assert!(inside.contains(&a.plan) && inside.contains(&checkout));
    // Its list of topics is its own topics alone.
    let topics = world
        .app
        .call(TopicList, &[&a.web])
        .token(&a.boss.token)
        .query(&TopicListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        topics
            .items
            .iter()
            .filter_map(orch_api_contract::common::Listed::as_visible)
            .map(|topic| topic.base.id)
            .collect::<Vec<_>>(),
        vec![a.plan]
    );
    // The tree of the root org carries every node, each with the node it lies in.
    let tree = world
        .app
        .call(OrgTreeGet, &[&a.acme])
        .token(&a.boss.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    let parent_of = |id: Id| {
        tree.readable()
            .into_iter()
            .find(|node| node.id == id)
            .unwrap_or_else(|| panic!("{id} is in the tree"))
            .parent_id
    };
    assert_eq!(parent_of(checkout), Some(a.web));
    assert_eq!(parent_of(payments), Some(checkout));
    assert_eq!(parent_of(cart), Some(checkout));
    assert_eq!(parent_of(refunds), Some(payments));
    assert_eq!(parent_of(a.plan), Some(a.web));
    // A viewer sees them all, a person with nothing in the org none.
    let stranger = person(&world, "stranger").await;
    let none = world
        .app
        .call(OrgTreeGet, &[&a.acme])
        .token(&stranger.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    assert!(none.nodes.is_empty());
}

#[tokio::test]
async fn projects_may_lie_in_one_another_only_as_deep_as_the_system_allows() {
    let mut config = orch_config::ServerConfig::default();
    config.limits.project_depth = 3;
    let world = World::with(config).await;
    let a = acme(&world).await;
    // Web is the first project of its path, so two folders more fit.
    let two = make_folder(&world, &a.boss, a.web, "Two").await;
    let three = make_folder(&world, &a.boss, two.base.id, "Three").await;
    assert_eq!(
        ids(&three.base.path),
        vec![a.acme, a.web, two.base.id, three.base.id]
    );
    let refused = folder_refused(&world, &a.boss, three.base.id, "Four").await;
    assert_eq!(refused.code, ErrorCode::ProjectTooDeep);
    assert_eq!(refused.extra["limit"], 3);
    // The depth of orgs is another count: a project in an org three orgs down is the first project.
    let nothing: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE name = 'Four'")
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(nothing, 0, "nothing was made");
}

// ---------------------------------------------------------------- a project is moved

#[tokio::test]
async fn a_project_is_moved_into_another_project_into_an_org_and_back_with_all_that_is_in_it() {
    let world = World::new().await;
    let a = acme(&world).await;
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    let guide = make_topic(&world, &a.boss, docs, "Guide").await.base.id;
    let before = revision(&world, &a.boss, docs).await;
    // Docs goes into Web, and becomes a folder of it.
    let moved = project_moved(&world, &a.boss, docs, a.web).await;
    assert_eq!(moved.base.parent_id, Some(a.web));
    assert_eq!(moved.base.revision, before + 1);
    assert_eq!(ids(&moved.base.path), vec![a.acme, a.web, docs]);
    assert_eq!(moved.topic_count, 1, "what was in it is in it still");
    assert_eq!(
        path_of(&world, guide).await,
        vec![a.acme, a.web, docs, guide]
    );
    let Node::Project(web) = read(&world, &a.boss, a.web).await else {
        panic!("a project stays one");
    };
    assert_eq!((web.project_count, web.topic_count), (1, 1));
    let Node::Org(acme) = read(&world, &a.boss, a.acme).await else {
        panic!("an org stays one");
    };
    assert_eq!(acme.project_count, 2, "General and Web: Docs left the org");
    // Its owners and its page stay.
    assert_eq!(moved.base.owner.username, "boss");
    // And back into the org.
    let back = project_moved(&world, &a.boss, docs, a.acme).await;
    assert_eq!(back.base.parent_id, Some(a.acme));
    assert_eq!(ids(&back.base.path), vec![a.acme, docs]);
    assert_eq!(path_of(&world, guide).await, vec![a.acme, docs, guide]);
    // Both places show it in their Activity, each time.
    let activity = |place: Id| {
        let world = &world;
        let boss = &a.boss;
        async move {
            world
                .app
                .call(NodeActivityList, &[&place])
                .token(&boss.token)
                .query(&NodeActivityListQuery::default())
                .send()
                .await
                .ok()
                .items
                .iter()
                .filter(|line| line.action.as_deref() == Some("node.moved"))
                .count()
        }
    };
    assert!(activity(a.web).await >= 1, "the project it joined shows it");
    assert!(activity(a.acme).await >= 1);
}

#[tokio::test]
async fn a_project_is_refused_into_itself_below_itself_into_another_tree_and_into_what_is_no_place_for_it()
 {
    let world = World::new().await;
    let a = acme(&world).await;
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    let inner = make_folder(&world, &a.boss, docs, "Inner").await.base.id;
    let deeper = make_folder(&world, &a.boss, inner, "Deeper").await.base.id;
    let topic = make_topic(&world, &a.boss, docs, "Guide").await.base.id;
    // Another root org of the same person.
    let other = make_org(&world, &a.boss, "Other", "other").await.base.id;
    let elsewhere = make_project(&world, &a.boss, other, "Elsewhere")
        .await
        .base
        .id;

    for (what, parent) in [
        ("into itself", docs),
        ("into a folder in it", inner),
        ("below itself, two levels", deeper),
        ("into a project of another tree", elsewhere),
        ("into the root org of another tree", other),
        ("into a topic", topic),
    ] {
        let refused = project_move_refused(&world, &a.boss, docs, parent).await;
        assert_eq!(refused.code, ErrorCode::NodeWrongParent, "{what}");
        // The plan says so before anything is done.
        let plan = world
            .app
            .call(ProjectMovePreview, &[&docs])
            .token(&a.boss.token)
            .body(&MovePreviewBody { parent_id: parent })
            .send()
            .await
            .ok();
        assert_eq!(
            plan.blockers.first().map(|blocker| blocker.code.as_str()),
            Some("node.wrong_parent"),
            "{what}"
        );
    }
    // Nothing moved.
    assert_eq!(
        path_of(&world, deeper).await,
        vec![a.acme, docs, inner, deeper]
    );
    // A folder goes out of the project it lies in to another place of the tree.
    let moved = project_moved(&world, &a.boss, deeper, a.web).await;
    assert_eq!(ids(&moved.base.path), vec![a.acme, a.web, deeper]);
}

#[tokio::test]
async fn a_project_is_refused_past_the_depth_the_system_allows_counting_what_is_in_it() {
    let mut config = orch_config::ServerConfig::default();
    config.limits.project_depth = 3;
    let world = World::with(config).await;
    let a = acme(&world).await;
    // Web > Two: two projects deep. Z > W: two deep as well.
    let two = make_folder(&world, &a.boss, a.web, "Two").await.base.id;
    let z = make_project(&world, &a.boss, a.acme, "Z").await.base.id;
    let w = make_folder(&world, &a.boss, z, "W").await.base.id;
    let refused = project_move_refused(&world, &a.boss, z, two).await;
    assert_eq!(
        refused.code,
        ErrorCode::ProjectTooDeep,
        "two and two make four"
    );
    assert_eq!(refused.extra["limit"], 3);
    let plan = world
        .app
        .call(ProjectMovePreview, &[&z])
        .token(&a.boss.token)
        .body(&MovePreviewBody { parent_id: two })
        .send()
        .await
        .ok();
    assert_eq!(
        plan.blockers.first().map(|blocker| blocker.code.as_str()),
        Some("project.too_deep")
    );
    // A project with nothing in it fits: three deep is allowed.
    let leaf = make_project(&world, &a.boss, a.acme, "Leaf").await.base.id;
    let moved = project_moved(&world, &a.boss, leaf, two).await;
    assert_eq!(ids(&moved.base.path), vec![a.acme, a.web, two, leaf]);
    // The same project goes into Web, where it makes three.
    let fits = project_moved(&world, &a.boss, z, a.web).await;
    assert_eq!(ids(&fits.base.path), vec![a.acme, a.web, z]);
    assert_eq!(path_of(&world, w).await, vec![a.acme, a.web, z, w]);
}

#[tokio::test]
async fn moving_a_project_into_a_project_takes_the_right_to_make_projects_there() {
    let world = World::new().await;
    let a = acme(&world).await;
    // The Developer makes a folder in Web, so owns it; and sees a project of the org but may open
    // no topic there.
    let mine = make_folder(&world, &a.dev, a.web, "Mine").await.base.id;
    let dan = person(&world, "dan").await;
    join(&world, a.acme, a.acme, &dan, "viewer", &a.boss).await;
    let closed = make_project(&world, &a.boss, a.acme, "Closed")
        .await
        .base
        .id;
    // As shipped a Developer of the org opens topics in every project of it; this one cut that
    // loose, so only its owner does.
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::set_row(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", closed, "live"),
        "any",
        "project.interact",
        false,
        false,
    )
    .await
    .unwrap();
    drop(conn);
    let refused = project_move_refused(&world, &a.dev, mine, closed).await;
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        first_missing(&refused)["permissions"],
        json!(["project.interact"])
    );
    assert_eq!(told(&refused)["object"]["id"], json!(closed));
    // The plan is refused the same way, and moves nothing.
    let plan = world
        .app
        .call(ProjectMovePreview, &[&mine])
        .token(&a.dev.token)
        .body(&MovePreviewBody { parent_id: closed })
        .send()
        .await
        .problem();
    assert_eq!(plan.code, ErrorCode::AuthzDenied);
    // Into the project where the Developer opens topics it goes.
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    let moved = project_moved(&world, &a.dev, mine, docs).await;
    assert_eq!(moved.base.parent_id, Some(docs));
    // Somebody who manages nothing of it cannot move it.
    let unmanaged = project_move_refused(&world, &dan, a.web, docs).await;
    assert_eq!(unmanaged.code, ErrorCode::AuthzDenied);
    assert_eq!(told(&unmanaged)["required"], "project.manage");
}

// ---------------------------------------------------------------- a topic is moved

async fn thread_in(world: &World, topic: Id, by: &Who, name: &str) -> Id {
    let thread = new_id();
    let mut conn = world.conn().await;
    insert_node(&mut conn, thread, "thread", Some(topic), name, by.id())
        .await
        .unwrap();
    thread
}

#[tokio::test]
async fn a_topic_is_moved_to_another_project_and_to_a_folder_and_its_threads_documents_and_settings_follow()
 {
    let world = World::new().await;
    let a = acme(&world).await;
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    let guides = make_folder(&world, &a.boss, docs, "Guides").await.base.id;
    let thread = thread_in(&world, a.plan, &a.boss, "Draft").await;
    // A document beside the topic, an environment whose home it is, and a setting that differs.
    let document = world
        .app
        .call(NodeDocumentCreate, &[&a.plan])
        .token(&a.boss.token)
        .body(&NodeDocumentCreateBody {
            title: "Outline".to_owned(),
            blocks: None,
            markdown: Some("Steps".to_owned()),
            access: None,
        })
        .send()
        .await
        .ok();
    let environment = {
        let mut conn = world.conn().await;
        put_thread_in_environment(&mut conn, thread, "managed", a.boss.id(), a.plan)
            .await
            .unwrap()
    };
    world
        .set_at(Target::Node(a.web), "chat.enabled", json!(true))
        .await;
    world
        .set_at(Target::Node(guides), "chat.enabled", json!(false))
        .await;
    let chat = |topic: Id| {
        let world = &world;
        async move {
            let mut conn = world.conn().await;
            orch_settings::get::<bool>(&mut conn, orch_settings::Place::Node(topic), "chat.enabled")
                .await
                .unwrap()
        }
    };
    assert_eq!(chat(a.plan).await, Some(true));

    // Its page follows the defaults of the place it lies in: somebody who views the project it
    // leaves reads it now; somebody who views the folder it goes to does not yet.
    let leaving = person(&world, "leaving").await;
    let arriving = person(&world, "arriving").await;
    places::give_role(&world, a.web, &leaving, "viewer", &a.boss).await;
    places::give_role(&world, guides, &arriving, "viewer", &a.boss).await;
    let reads = |who: &Who| {
        let world = &world;
        let plan = a.plan;
        let token = who.token.clone();
        async move {
            world
                .app
                .call(orch_api_contract::nodes::NodeGet, &[&plan])
                .token(&token)
                .send()
                .await
                .body
                .is_ok()
        }
    };
    assert!(reads(&leaving).await && !reads(&arriving).await);

    // The plan says what changes: the setting, and nothing moved yet.
    let plan = plan_of_topic(&world, &a.boss, a.plan, guides).await;
    assert_eq!(plan.node.id, a.plan);
    assert_eq!(plan.from.id, a.web);
    assert_eq!(plan.to.id, guides);
    assert!(plan.blockers.is_empty());
    let setting = plan
        .settings
        .iter()
        .find(|each| each.key == "chat.enabled")
        .expect("chat resolves differently there");
    assert_eq!(
        (&setting.from, &setting.to),
        (&Some(json!(true)), &Some(json!(false)))
    );
    assert_eq!(path_of(&world, a.plan).await, vec![a.acme, a.web, a.plan]);

    // Moved into the folder of another project.
    let before = revision(&world, &a.boss, a.plan).await;
    let moved = topic_moved(&world, &a.boss, a.plan, guides).await;
    assert_eq!(moved.base.parent_id, Some(guides));
    assert_eq!(moved.base.revision, before + 1);
    assert_eq!(ids(&moved.base.path), vec![a.acme, docs, guides, a.plan]);
    assert_eq!(moved.base.owner.username, "boss", "its owners stay");
    assert_eq!(
        path_of(&world, thread).await,
        vec![a.acme, docs, guides, a.plan, thread],
        "its threads go with it"
    );
    assert_eq!(
        chat(a.plan).await,
        Some(false),
        "its settings resolve along the new path"
    );
    assert!(
        !reads(&leaving).await && reads(&arriving).await,
        "the roles held above it are those of the new place"
    );

    // The document is beside the topic still, so it is found under the folder and not under Web.
    let found = |place: Id| {
        let world = &world;
        let boss = &a.boss;
        async move {
            world
                .app
                .call(NodeDocumentList, &[&place])
                .token(&boss.token)
                .query(&NodeDocumentListQuery {
                    under: Some(true),
                    ..NodeDocumentListQuery::default()
                })
                .send()
                .await
                .ok()
                .items
                .iter()
                .filter_map(orch_api_contract::common::Listed::as_visible)
                .map(|each| each.id)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(found(guides).await, vec![document.base.id]);
    assert_eq!(
        found(docs).await,
        vec![document.base.id],
        "and under the project the folder lies in"
    );
    assert!(found(a.web).await.is_empty());
    // The environment's home is the topic: it is under the folder, and no longer under Web.
    let mut conn = world.conn().await;
    assert_eq!(
        hosting::environments_under(&mut conn, guides)
            .await
            .unwrap(),
        vec![environment]
    );
    assert!(
        hosting::environments_under(&mut conn, a.web)
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    // Web counts one topic less, the folder one more.
    let Node::Project(web) = read(&world, &a.boss, a.web).await else {
        panic!("a project stays one");
    };
    let Node::Project(folder) = read(&world, &a.boss, guides).await else {
        panic!("a project stays one");
    };
    assert_eq!((web.topic_count, folder.topic_count), (0, 1));
    // Both places learn of it in their Activity.
    for place in [a.web, guides] {
        let lines = world
            .app
            .call(NodeActivityList, &[&place])
            .token(&a.boss.token)
            .query(&NodeActivityListQuery::default())
            .send()
            .await
            .ok();
        assert!(
            lines
                .items
                .iter()
                .any(|line| line.action.as_deref() == Some("node.moved")),
            "{place}"
        );
    }
    // And back to the project it came from.
    let back = topic_moved(&world, &a.boss, a.plan, a.web).await;
    assert_eq!(ids(&back.base.path), vec![a.acme, a.web, a.plan]);
    assert_eq!(chat(a.plan).await, Some(true));
}

#[tokio::test]
async fn a_topic_is_moved_by_whoever_manages_it_to_a_place_where_they_may_open_topics() {
    let world = World::new().await;
    let a = acme(&world).await;
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    // The Developer opened a topic, so manages it; and opens topics in Docs too as shipped.
    let mine = make_topic(&world, &a.dev, a.web, "Mine").await.base.id;
    // A project where topics are opened by its owners alone.
    let closed = make_project(&world, &a.boss, a.acme, "Closed")
        .await
        .base
        .id;
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::set_row(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", closed, "live"),
        "any",
        "project.interact",
        false,
        false,
    )
    .await
    .unwrap();
    drop(conn);
    let refused = topic_move_refused(&world, &a.dev, mine, closed).await;
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        first_missing(&refused)["permissions"],
        json!(["project.create_topics"])
    );
    // Somebody who does not manage the topic is refused whatever they hold where it goes.
    let nope = topic_move_refused(&world, &a.viewer, mine, docs).await;
    assert_eq!(nope.code, ErrorCode::AuthzDenied);
    assert_eq!(told(&nope)["required"], "topic.manage");
    // The plan is asked the same.
    let plan = world
        .app
        .call(TopicMovePreview, &[&mine])
        .token(&a.viewer.token)
        .body(&MovePreviewBody { parent_id: docs })
        .send()
        .await
        .problem();
    assert_eq!(plan.code, ErrorCode::AuthzDenied);
    let moved = topic_moved(&world, &a.dev, mine, docs).await;
    assert_eq!(moved.base.parent_id, Some(docs));
    // A stale revision is refused with the topic as it stands.
    let stale = world
        .app
        .call(TopicMove, &[&mine])
        .token(&a.dev.token)
        .body(&NodeMoveBody {
            parent_id: a.web,
            preflight_id: None,
            expected_revision: 1,
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    // To the project it lies in: nothing changes.
    let revision_now = revision(&world, &a.dev, mine).await;
    let same = topic_moved(&world, &a.dev, mine, docs).await;
    assert_eq!(same.base.revision, revision_now);
}

#[tokio::test]
async fn a_topic_is_refused_another_tree_what_is_no_project_and_while_a_thread_of_it_is_held() {
    let world = World::new().await;
    let a = acme(&world).await;
    let other = make_org(&world, &a.boss, "Other", "other").await.base.id;
    let elsewhere = make_project(&world, &a.boss, other, "Elsewhere")
        .await
        .base
        .id;
    let thread = thread_in(&world, a.plan, &a.boss, "Draft").await;
    for (what, parent) in [
        ("a project of another tree", elsewhere),
        ("an org", a.acme),
        ("a topic", a.plan),
        ("a thread", thread),
    ] {
        let refused = topic_move_refused(&world, &a.boss, a.plan, parent).await;
        assert_eq!(refused.code, ErrorCode::NodeWrongParent, "{what}");
    }
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    // A switch that is under way holds the thread: the topic stays where it is.
    let held = |state: &'static str| {
        let world = &world;
        async move {
            let mut conn = world.conn().await;
            sqlx::query(
                "INSERT INTO jobs (id, kind, node_id, state, plan) VALUES ($1, 'thread_fork', $2, $3, $4)",
            )
            .bind(new_id())
            .bind(thread)
            .bind(state)
            .bind(json!({ "result": "replace" }))
            .execute(&mut *conn)
            .await
            .unwrap();
        }
    };
    held("running").await;
    let refused = topic_move_refused(&world, &a.boss, a.plan, docs).await;
    assert_eq!(refused.code, ErrorCode::ThreadNotIdle);
    assert_eq!(refused.extra["thread"], json!(thread));
    let thread_state: String = sqlx::query_scalar("SELECT state FROM threads WHERE node_id = $1")
        .bind(thread)
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(
        refused.extra["state"],
        json!(thread_state),
        "state describes the thread, not the holding job"
    );
    assert_eq!(refused.extra["job"], json!("thread_fork"));
    let plan = plan_of_topic(&world, &a.boss, a.plan, docs).await;
    assert_eq!(
        plan.blockers.first().map(|blocker| blocker.code.as_str()),
        Some("thread.not_idle")
    );
    assert_eq!(path_of(&world, a.plan).await, vec![a.acme, a.web, a.plan]);
    // Once it ended the topic moves.
    sqlx::query("UPDATE jobs SET state = 'succeeded'")
        .execute(&mut *world.conn().await)
        .await
        .unwrap();
    let moved = topic_moved(&world, &a.boss, a.plan, docs).await;
    assert_eq!(moved.base.parent_id, Some(docs));
}

// ---------------------------------------------------------------- what a topic used

fn attribution(acme: Id, project: Id, topic: Id, thread: Id, by: Id) -> Attribution {
    Attribution {
        org_id: acme,
        project_id: project,
        topic_id: topic,
        thread_id: thread,
        segment_id: new_id(),
        turn_id: new_id(),
        provider_id: new_id(),
        entry_id: None,
        user_id: Some(by),
        agent_id: None,
        harness_kind: Some("codex".to_owned()),
        environment_id: None,
        environment_kind: Some("managed".to_owned()),
        arrangement: None,
        turn_model: "gpt-6-sol".to_owned(),
        effort: None,
        params: Json::Null,
    }
}

async fn spend(world: &World, place: &Attribution, day: u32, tokens: i64) {
    let mut conn = world.conn().await;
    record(
        &mut conn,
        &NewEvent {
            id: new_id(),
            at: Utc.with_ymd_and_hms(2026, 10, day, 9, 0, 0).unwrap(),
            origin: ledger::origin::FORWARDER,
            scope: ledger::scope::REQUEST,
            counted: true,
            attribution: place,
            model: "gpt-6-sol",
            tier: None,
            tokens: Tokens {
                input: tokens,
                cached_input: 0,
                cache_write: 0,
                output: 0,
                reasoning: 0,
                total: tokens,
            },
            context_tokens: None,
            request_ref: None,
            rate: None,
            rate_source: None,
            credits: None,
        },
    )
    .await
    .unwrap();
}

async fn used_in(world: &World, by: &Who, node: Id) -> i64 {
    let asked = UsageGetQuery {
        health: None,
        turn_id: None,
        org_id: None,
        measure: None,
        effort: None,
        tier: None,
        program: None,
        outcome: None,
        scope: format!("node:{node}"),
        from: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap().to_string(),
        to: NaiveDate::from_ymd_opt(2026, 10, 31).unwrap().to_string(),
        bucket: None,
        series_by: None,
        breakdowns: None,
        limit: None,
        user_id: None,
        model: None,
        provider_id: None,
        harness_kind: None,
        environment_kind: None,
        project_id: None,
        topic_id: None,
        thread_id: None,
    };
    let figures: UsageAnalytics = world
        .app
        .call(UsageGet, &[])
        .token(&by.token)
        .query(&asked)
        .send()
        .await
        .ok();
    figures.totals.tokens.total_tokens
}

#[tokio::test]
async fn the_use_of_a_topic_goes_with_it_and_the_figures_of_a_project_take_in_the_folders_inside_it()
 {
    let world = World::new().await;
    let a = acme(&world).await;
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    let guides = make_folder(&world, &a.boss, docs, "Guides").await.base.id;
    let guide = make_topic(&world, &a.boss, guides, "Guide").await.base.id;
    let draft = thread_in(&world, a.plan, &a.boss, "Draft").await;
    let read_thread = thread_in(&world, guide, &a.boss, "Read").await;
    // Each topic used something, put down to the project it lies in directly.
    spend(
        &world,
        &attribution(a.acme, a.web, a.plan, draft, a.boss.id()),
        3,
        1_000,
    )
    .await;
    spend(
        &world,
        &attribution(a.acme, a.web, a.plan, draft, a.boss.id()),
        4,
        2_000,
    )
    .await;
    spend(
        &world,
        &attribution(a.acme, guides, guide, read_thread, a.boss.id()),
        4,
        300,
    )
    .await;

    // A project's figures take in the folders in it, a folder's its own and the ones inside it.
    assert_eq!(used_in(&world, &a.boss, a.web).await, 3_000);
    assert_eq!(used_in(&world, &a.boss, docs).await, 300);
    assert_eq!(used_in(&world, &a.boss, guides).await, 300);
    assert_eq!(used_in(&world, &a.boss, a.acme).await, 3_300);

    // The topic goes to the folder: its history goes with it, and nothing it used changes.
    let moved = topic_moved(&world, &a.boss, a.plan, guides).await;
    assert_eq!(moved.base.parent_id, Some(guides));
    assert_eq!(
        used_in(&world, &a.boss, a.web).await,
        0,
        "the project it left"
    );
    assert_eq!(
        used_in(&world, &a.boss, docs).await,
        3_300,
        "the project the folder lies in"
    );
    assert_eq!(used_in(&world, &a.boss, guides).await, 3_300);
    assert_eq!(
        used_in(&world, &a.boss, a.acme).await,
        3_300,
        "no figure of the org changed"
    );
    // The topic's own, and the rows of the ledger and of both rollups name the folder now.
    let mut conn = world.conn().await;
    for table in ["usage_events", "usage_daily", "usage_monthly"] {
        let names: Vec<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT DISTINCT project_id FROM {table} WHERE topic_id = $1"
        )))
        .bind(a.plan)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(names, vec![guides], "{table}");
    }
    let total: i64 = sqlx::query_scalar(
        "SELECT sum(total_tokens)::bigint FROM usage_events WHERE topic_id = $1",
    )
    .bind(a.plan)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(total, 3_000);
    // The audit row of the move says how many rows followed.
    let details: Json = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE action = 'node.moved' AND object_id = $1",
    )
    .bind(a.plan)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(details["usage_rows"]["ledger"], 2);
}

// ---------------------------------------------------------------- the tags a moved topic carries

#[tokio::test]
async fn a_tag_a_topic_carries_stays_when_it_is_moved_and_only_a_tag_that_is_added_is_held_to_the_rule()
 {
    let world = World::new().await;
    let a = acme(&world).await;
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    let tag_on = |node: Id, name: &'static str| {
        let world = &world;
        let boss = &a.boss;
        async move {
            world
                .app
                .call(NodeTagCreate, &[&node])
                .token(&boss.token)
                .body(&NodeTagCreateBody {
                    name: name.to_owned(),
                    color: Some(1),
                })
                .send()
                .await
                .ok()
                .id
        }
    };
    let bug = tag_on(a.web, "bug").await;
    let guide = tag_on(docs, "guide").await;
    let tagged = |tags: Vec<Id>| TopicUpdateBody {
        status: None,
        tag_ids: Some(tags),
        expected_revision: None,
    };
    let set = world
        .app
        .call(TopicUpdate, &[&a.plan])
        .token(&a.boss.token)
        .body(&tagged(vec![bug]))
        .send()
        .await
        .ok();
    assert_eq!(set.base.tags.len(), 1);
    // The tag is defined by the project the topic leaves; it is not defined above it any more.
    let moved = topic_moved(&world, &a.boss, a.plan, docs).await;
    assert_eq!(
        moved.base.tags.iter().map(|tag| tag.id).collect::<Vec<_>>(),
        vec![bug]
    );
    // It may stay while another is added, which the new place defines.
    let both = world
        .app
        .call(TopicUpdate, &[&a.plan])
        .token(&a.boss.token)
        .body(&tagged(vec![bug, guide]))
        .send()
        .await
        .ok();
    assert_eq!(both.base.tags.len(), 2);
    // A tag of the place it left is not added by anybody now.
    let other = make_topic(&world, &a.boss, docs, "Other").await.base.id;
    let refused = world
        .app
        .call(TopicUpdate, &[&other])
        .token(&a.boss.token)
        .body(&tagged(vec![bug]))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    // The tags a topic may be given are those of its way now.
    let listed = world
        .app
        .call(NodeTagList, &[&a.plan])
        .token(&a.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(
        listed.iter().map(|tag| tag.id).collect::<Vec<_>>(),
        vec![guide]
    );
}

// ---------------------------------------------------------------- what a project takes in

#[tokio::test]
async fn the_activity_the_documents_and_the_environments_of_a_project_take_in_the_folders_inside_it()
 {
    let world = World::new().await;
    let a = acme(&world).await;
    let checkout = make_folder(&world, &a.boss, a.web, "Checkout")
        .await
        .base
        .id;
    let payments = make_folder(&world, &a.boss, checkout, "Payments")
        .await
        .base
        .id;
    let cart = make_topic(&world, &a.boss, checkout, "Cart").await.base.id;
    let refunds = make_topic(&world, &a.boss, payments, "Refunds")
        .await
        .base
        .id;
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    let guide = make_topic(&world, &a.boss, docs, "Guide").await.base.id;

    // Its Activity is the lines about it and about what lies directly in it, and in each folder in it.
    let lines_about = |place: Id| {
        let world = &world;
        let boss = &a.boss;
        async move {
            let mut nodes: Vec<Id> = world
                .app
                .call(NodeActivityList, &[&place])
                .token(&boss.token)
                .query(&NodeActivityListQuery::default())
                .send()
                .await
                .ok()
                .items
                .iter()
                .map(|line| line.node.id)
                .collect();
            nodes.sort();
            nodes.dedup();
            nodes
        }
    };
    let web = lines_about(a.web).await;
    for (what, node) in [
        ("a topic of the project", a.plan),
        ("a folder in it", checkout),
        ("a topic of the folder", cart),
        ("a folder in the folder", payments),
        ("a topic of that", refunds),
    ] {
        assert!(web.contains(&node), "the Activity of Web shows {what}");
    }
    assert!(
        !web.contains(&guide),
        "and not what lies in another project"
    );
    let at_checkout = lines_about(checkout).await;
    assert!(at_checkout.contains(&refunds) && at_checkout.contains(&cart));
    assert!(
        !at_checkout.contains(&a.plan),
        "a folder does not show what lies beside it"
    );

    // The documents beside it, with those of the folders, and those of the project alone.
    let document = |node: Id| {
        let world = &world;
        let boss = &a.boss;
        async move {
            world
                .app
                .call(NodeDocumentCreate, &[&node])
                .token(&boss.token)
                .body(&NodeDocumentCreateBody {
                    title: "Notes".to_owned(),
                    blocks: None,
                    markdown: Some("x".to_owned()),
                    access: None,
                })
                .send()
                .await
                .ok()
                .base
                .id
        }
    };
    let (own, inner, elsewhere) = (
        document(a.plan).await,
        document(refunds).await,
        document(guide).await,
    );
    let listed = |under: bool| {
        let world = &world;
        let boss = &a.boss;
        async move {
            let mut found: Vec<Id> = world
                .app
                .call(NodeDocumentList, &[&a.web])
                .token(&boss.token)
                .query(&NodeDocumentListQuery {
                    under: Some(under),
                    ..NodeDocumentListQuery::default()
                })
                .send()
                .await
                .ok()
                .items
                .iter()
                .filter_map(orch_api_contract::common::Listed::as_visible)
                .map(|each| each.id)
                .collect();
            found.sort();
            found
        }
    };
    let mut both = vec![own, inner];
    both.sort();
    assert_eq!(
        listed(true).await,
        both,
        "the folders' documents are under Web"
    );
    assert!(!listed(true).await.contains(&elsewhere));

    // The environments whose home is a topic of a folder, and those its threads run in, are under it.
    let thread = thread_in(&world, refunds, &a.boss, "Fix").await;
    let environment = {
        let mut conn = world.conn().await;
        put_thread_in_environment(&mut conn, thread, "managed", a.boss.id(), refunds)
            .await
            .unwrap()
    };
    let mut conn = world.conn().await;
    for place in [a.web, checkout, payments] {
        assert_eq!(
            hosting::environments_under(&mut conn, place).await.unwrap(),
            vec![environment]
        );
    }
    assert!(
        hosting::environments_under(&mut conn, docs)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_plan_of_a_move_into_a_project_says_who_gains_and_loses_what_and_which_settings_change()
{
    let world = World::new().await;
    let a = acme(&world).await;
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    let mobile = make_project(&world, &a.boss, a.acme, "Mobile")
        .await
        .base
        .id;
    // Chat is switched off in Docs; a person is an Admin there and of nothing else.
    world
        .set_at(Target::Node(docs), "chat.enabled", json!(false))
        .await;
    let dan = person(&world, "dan").await;
    places::give_role(&world, docs, &dan, "admin", &a.boss).await;
    let plan = |node: Id, parent: Id| {
        let world = &world;
        let boss = &a.boss;
        async move {
            world
                .app
                .call(ProjectMovePreview, &[&node])
                .token(&boss.token)
                .body(&MovePreviewBody { parent_id: parent })
                .send()
                .await
                .ok()
        }
    };
    let into = plan(mobile, docs).await;
    assert!(into.blockers.is_empty());
    assert_eq!((into.from.id, into.to.id), (a.acme, docs));
    assert_eq!(
        into.roles.gaining, 1,
        "Dan would hold his role in Docs inside Mobile"
    );
    assert_eq!(into.roles.losing, 0);
    let chat = into
        .settings
        .iter()
        .find(|setting| setting.key == "chat.enabled")
        .expect("chat resolves differently there");
    assert_eq!(
        (&chat.from, &chat.to),
        (&Some(json!(true)), &Some(json!(false)))
    );
    assert!(into.access.pages >= 1);
    // Nothing moved by the plan; the move does what it said, and the plan back says the reverse.
    assert_eq!(path_of(&world, mobile).await, vec![a.acme, mobile]);
    project_moved(&world, &a.boss, mobile, docs).await;
    let out = plan(mobile, a.acme).await;
    assert_eq!((out.roles.gaining, out.roles.losing), (0, 1));
    let chat = out
        .settings
        .iter()
        .find(|setting| setting.key == "chat.enabled")
        .expect("and back again");
    assert_eq!(
        (&chat.from, &chat.to),
        (&Some(json!(false)), &Some(json!(true)))
    );
}

#[tokio::test]
async fn nothing_is_moved_out_of_what_is_archived_or_into_it() {
    let world = World::new().await;
    let a = acme(&world).await;
    let docs = make_project(&world, &a.boss, a.acme, "Docs").await.base.id;
    let other = make_project(&world, &a.boss, a.acme, "Other").await.base.id;
    let archive = |node: Id| {
        let world = &world;
        let boss = &a.boss;
        async move {
            world
                .app
                .call(NodeArchive, &[&node])
                .token(&boss.token)
                .body(&places::archive_body(world, &boss.token, node, true).await)
                .send()
                .await
                .ok();
        }
    };
    // A place that is archived takes nothing into it.
    archive(docs).await;
    let into = topic_move_refused(&world, &a.boss, a.plan, docs).await;
    assert_eq!(into.code, ErrorCode::NodeArchived);
    let folder_into = project_move_refused(&world, &a.boss, other, docs).await;
    assert_eq!(folder_into.code, ErrorCode::NodeArchived);
    // And what is archived is not moved, nor what lies in an archived place.
    archive(a.plan).await;
    let out = topic_move_refused(&world, &a.boss, a.plan, other).await;
    assert_eq!(out.code, ErrorCode::NodeArchived);
    let folder = make_folder(&world, &a.boss, other, "Inner").await.base.id;
    archive(other).await;
    let below = project_move_refused(&world, &a.boss, folder, a.web).await;
    assert_eq!(
        below.code,
        ErrorCode::NodeArchived,
        "it lies in an archived project"
    );
    assert_eq!(path_of(&world, a.plan).await, vec![a.acme, a.web, a.plan]);
}
