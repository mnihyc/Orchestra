//! Moving a project or a department inside its tree, with the plan that comes first, and the tags that topics carry.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{
    NodeTagCreate, NodeTagCreateBody, NodeTagList, TagDelete, TagUpdate, TagUpdateBody,
};
use orch_api_contract::nodes::{
    MovePreviewBody, Node, NodeActivityList, NodeActivityListQuery, NodeMoveBody, NodeMovePlan,
    OrgMove, OrgMovePreview, ProjectMove, ProjectMovePreview, TopicUpdate, TopicUpdateBody,
};
use orch_settings::Target;
use orch_types::Id;
use places::{
    Who, first_missing, give_role, join, make_department, make_org, make_project, make_topic,
    missing_permissions, person, read,
};
use serde_json::json;
use support::World;

/// Acme with two departments and a project with a topic in the root.
struct Acme {
    boss: Who,
    acme: Id,
    research: Id,
    sales: Id,
    web: Id,
    login: Id,
}

async fn acme(world: &World) -> Acme {
    let boss = person(world, "boss").await;
    let org = make_org(world, &boss, "Acme", "acme").await;
    let research = make_department(world, &boss, org.base.id, "Research", "research").await;
    let sales = make_department(world, &boss, org.base.id, "Sales", "sales").await;
    let web = make_project(world, &boss, org.base.id, "Web").await;
    let login = make_topic(world, &boss, web.base.id, "Login").await;
    Acme {
        boss,
        acme: org.base.id,
        research: research.base.id,
        sales: sales.base.id,
        web: web.base.id,
        login: login.base.id,
    }
}

async fn move_review(world: &World, a: &Acme) -> NodeMovePlan {
    world
        .app
        .call(ProjectMovePreview, &[&a.web])
        .token(&a.boss.token)
        .body(&MovePreviewBody {
            parent_id: a.research,
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_confirmed_move_refuses_a_changed_subtree_or_new_destination_consequences() {
    let world = World::new().await;
    let a = acme(&world).await;
    let review = move_review(&world, &a).await;
    make_topic(&world, &a.boss, a.web, "A newer child").await;
    let stale = world
        .app
        .call(ProjectMove, &[&a.web])
        .token(&a.boss.token)
        .body(&NodeMoveBody {
            parent_id: a.research,
            expected_revision: review.revision,
            preflight_id: Some(review.preflight_id),
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert!(
        matches!(read(&world, &a.boss, a.web).await, Node::Project(project) if project.base.parent_id == Some(a.acme))
    );
    let review: NodeMovePlan = serde_json::from_value(stale.extra["current"].clone()).unwrap();
    world
        .set_at(Target::Node(a.research), "chat.enabled", json!(false))
        .await;
    let stale = world
        .app
        .call(ProjectMove, &[&a.web])
        .token(&a.boss.token)
        .body(&NodeMoveBody {
            parent_id: a.research,
            expected_revision: review.revision,
            preflight_id: Some(review.preflight_id),
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let fresh: NodeMovePlan = serde_json::from_value(stale.extra["current"].clone()).unwrap();
    assert!(
        fresh
            .settings
            .iter()
            .any(|setting| setting.key == "chat.enabled" && setting.to == Some(json!(false)))
    );
    let moved = world
        .app
        .call(ProjectMove, &[&a.web])
        .token(&a.boss.token)
        .body(&NodeMoveBody {
            parent_id: a.research,
            expected_revision: fresh.revision,
            preflight_id: Some(fresh.preflight_id),
        })
        .send()
        .await
        .ok();
    assert_eq!(moved.base.parent_id, Some(a.research));
}

#[tokio::test]
async fn a_move_review_reports_an_own_value_overridden_by_a_new_inherited_lock_without_keeping_the_trial()
 {
    let world = World::new().await;
    let a = acme(&world).await;
    world
        .set_at(Target::Node(a.web), "chat.enabled", json!(true))
        .await;
    {
        let mut conn = world.conn().await;
        orch_settings::write::write(
            &mut conn,
            &orch_settings::write::Write {
                actor: orch_db::audit::Actor::System,
                target: Target::Node(a.research),
                key: "chat.enabled",
                value: json!(false),
                locked: Some(true),
                expected_revision: None,
                may_lock: true,
                origin: &orch_db::audit::Origin::default(),
            },
            world.now(),
        )
        .await
        .unwrap();
    }
    let plan = move_review(&world, &a).await;
    let setting = plan
        .settings
        .iter()
        .find(|setting| setting.key == "chat.enabled")
        .unwrap();
    assert_eq!(setting.from, Some(json!(true)));
    assert_eq!(setting.to, Some(json!(false)));
    assert!(
        matches!(&setting.locked_by, Some(orch_api_contract::nodes::NodeOrSystem::Node(node)) if node.id == a.research)
    );
    let mut conn = world.conn().await;
    assert_eq!(
        orch_settings::get::<bool>(&mut conn, orch_settings::Place::Node(a.web), "chat.enabled")
            .await
            .unwrap(),
        Some(true)
    );
}

async fn revision(world: &World, who: &Who, id: Id) -> i64 {
    match read(world, who, id).await {
        Node::Org(node) => node.base.revision,
        Node::Project(node) => node.base.revision,
        Node::Topic(node) => node.base.revision,
        Node::Thread(node) => node.base.revision,
    }
}

#[tokio::test]
async fn a_project_is_moved_under_another_org_of_its_tree_and_all_inside_it_goes_with_it() {
    let world = World::new().await;
    let a = acme(&world).await;
    let before = revision(&world, &a.boss, a.web).await;
    let moved = world
        .app
        .call(ProjectMove, &[&a.web])
        .token(&a.boss.token)
        .body(&NodeMoveBody {
            parent_id: a.research,
            preflight_id: None,
            expected_revision: before,
        })
        .send()
        .await
        .ok();
    assert_eq!(moved.base.parent_id, Some(a.research));
    assert_eq!(moved.base.revision, before + 1);
    assert_eq!(
        moved
            .base
            .path
            .iter()
            .map(|each| each.id)
            .collect::<Vec<_>>(),
        vec![a.acme, a.research, a.web]
    );
    assert_eq!(moved.topic_count, 1, "what was in it is still in it");
    let Node::Topic(login) = read(&world, &a.boss, a.login).await else {
        panic!("a topic stays one");
    };
    assert_eq!(
        login
            .base
            .path
            .iter()
            .map(|each| each.id)
            .collect::<Vec<_>>(),
        vec![a.acme, a.research, a.web, a.login],
        "its way is rewritten with the project's"
    );
    assert_eq!(login.base.org_id, a.acme);
    assert_eq!(login.base.owner.username, "boss", "its owners stay");
    // The org it left counts one project less, the one it joined one more.
    let Node::Org(research) = read(&world, &a.boss, a.research).await else {
        panic!("an org stays one");
    };
    assert_eq!(research.project_count, 1);
    let Node::Org(acme) = read(&world, &a.boss, a.acme).await else {
        panic!("an org stays one");
    };
    assert_eq!(acme.project_count, 1, "General stays");
    // One row of the audit trail for each place whose Activity shows it.
    let moves = world
        .audit()
        .await
        .into_iter()
        .filter(|action| action == "node.moved")
        .count();
    assert_eq!(moves, 2);
    let there = world
        .app
        .call(NodeActivityList, &[&a.research])
        .token(&a.boss.token)
        .query(&NodeActivityListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        there
            .items
            .iter()
            .any(|line| line.action.as_deref() == Some("node.moved")),
        "the org it joined shows it"
    );
    let here = world
        .app
        .call(NodeActivityList, &[&a.acme])
        .token(&a.boss.token)
        .query(&NodeActivityListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        here.items
            .iter()
            .any(|line| line.action.as_deref() == Some("node.moved")),
        "the org it left shows it too"
    );
}

#[tokio::test]
async fn a_move_with_a_stale_revision_is_refused_with_the_node_as_it_is() {
    let world = World::new().await;
    let a = acme(&world).await;
    let stale = world
        .app
        .call(ProjectMove, &[&a.web])
        .token(&a.boss.token)
        .body(&NodeMoveBody {
            parent_id: a.research,
            preflight_id: None,
            expected_revision: 99,
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["name"], "Web");
}

#[tokio::test]
async fn nothing_is_moved_out_of_its_tree_into_itself_or_to_become_a_root_org() {
    let world = World::new().await;
    let a = acme(&world).await;
    let other = make_org(&world, &a.boss, "Other", "other").await;
    let refuse = |node: Id, parent: Id, is_org: bool| {
        let world = &world;
        let boss = &a.boss;
        async move {
            let revision = revision(world, boss, node).await;
            let body = NodeMoveBody {
                parent_id: parent,
                preflight_id: None,
                expected_revision: revision,
            };
            if is_org {
                world
                    .app
                    .call(OrgMove, &[&node])
                    .token(&boss.token)
                    .body(&body)
                    .send()
                    .await
                    .problem()
            } else {
                world
                    .app
                    .call(ProjectMove, &[&node])
                    .token(&boss.token)
                    .body(&body)
                    .send()
                    .await
                    .problem()
            }
        }
    };
    // Into another tree.
    let problem = refuse(a.web, other.base.id, false).await;
    assert_eq!(problem.code, ErrorCode::NodeWrongParent);
    assert_eq!(problem.extra["parent"]["id"], json!(other.base.id));
    // Under a project, which is no org.
    let problem = refuse(a.sales, a.web, true).await;
    assert_eq!(problem.code, ErrorCode::NodeWrongParent);
    // A department into a department below itself.
    let below = make_department(&world, &a.boss, a.research, "ML", "ml").await;
    let problem = refuse(a.research, below.base.id, true).await;
    assert_eq!(problem.code, ErrorCode::NodeWrongParent);
    // Into itself.
    let problem = refuse(a.research, a.research, true).await;
    assert_eq!(problem.code, ErrorCode::NodeWrongParent);
    // A root org does not become a department.
    let problem = refuse(a.acme, other.base.id, true).await;
    assert_eq!(problem.code, ErrorCode::NodeWrongParent);
}

#[tokio::test]
async fn a_department_is_moved_with_what_is_in_it_and_never_deeper_than_the_system_allows() {
    let mut config = orch_config::ServerConfig::default();
    config.limits.org_depth = 3;
    let world = World::with(config).await;
    let a = acme(&world).await;
    let ml = make_department(&world, &a.boss, a.research, "ML", "ml").await;
    // Sales goes under Research: acme.research.sales, three orgs deep, the most.
    let moved = world
        .app
        .call(OrgMove, &[&a.sales])
        .token(&a.boss.token)
        .body(&NodeMoveBody {
            parent_id: a.research,
            preflight_id: None,
            expected_revision: revision(&world, &a.boss, a.sales).await,
        })
        .send()
        .await
        .ok();
    assert_eq!(moved.address, "acme.research.sales");
    assert_eq!(moved.depth, 2);
    assert_eq!(ml.depth, 2);
    // Research, with ML and Sales inside, cannot go under a new department
    // beneath Acme: acme.extra.research.ml would be four deep.
    let extra = make_department(&world, &a.boss, a.acme, "Extra", "extra").await;
    let refused = world
        .app
        .call(OrgMove, &[&a.research])
        .token(&a.boss.token)
        .body(&NodeMoveBody {
            parent_id: extra.base.id,
            preflight_id: None,
            expected_revision: revision(&world, &a.boss, a.research).await,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::OrgTooDeep);
    assert_eq!(refused.extra["limit"], 3);
}

#[tokio::test]
async fn a_move_needs_manage_of_what_is_moved_and_the_right_to_make_one_where_it_goes() {
    let world = World::new().await;
    let a = acme(&world).await;
    let dan = person(&world, "dan").await;
    join(&world, a.acme, a.acme, &dan, "viewer", &a.boss).await;
    // Dan manages the project through an Admin role on it, and may make
    // nothing in Research.
    give_role(&world, a.web, &dan, "admin", &a.boss).await;
    let refused = world
        .app
        .call(ProjectMove, &[&a.web])
        .token(&dan.token)
        .body(&NodeMoveBody {
            parent_id: a.research,
            preflight_id: None,
            expected_revision: revision(&world, &a.boss, a.web).await,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "org.interact");
    assert_eq!(refused.extra["object"]["id"], json!(a.research));
    // Nor may a viewer of everything else move the project at all.
    let eve = person(&world, "eve").await;
    join(&world, a.acme, a.acme, &eve, "viewer", &a.boss).await;
    let refused = world
        .app
        .call(ProjectMove, &[&a.web])
        .token(&eve.token)
        .body(&NodeMoveBody {
            parent_id: a.research,
            preflight_id: None,
            expected_revision: revision(&world, &a.boss, a.web).await,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "project.manage");
}

#[tokio::test]
async fn the_plan_of_a_move_is_refused_to_whoever_could_not_make_the_move() {
    let world = World::new().await;
    let a = acme(&world).await;
    let dan = person(&world, "dan").await;
    join(&world, a.acme, a.acme, &dan, "viewer", &a.boss).await;
    let refused = world
        .app
        .call(ProjectMovePreview, &[&a.web])
        .token(&dan.token)
        .body(&MovePreviewBody {
            parent_id: a.research,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "project.manage");
    assert_eq!(
        missing_permissions(&refused),
        vec!["project.manage".to_owned()]
    );
    assert_eq!(first_missing(&refused)["ask"][0]["username"], "boss");
    // A manager of the project who may not make projects where it would go
    // is refused for that end.
    let eve = person(&world, "eve").await;
    join(&world, a.acme, a.acme, &eve, "viewer", &a.boss).await;
    give_role(&world, a.web, &eve, "admin", &a.boss).await;
    let refused = world
        .app
        .call(ProjectMovePreview, &[&a.web])
        .token(&eve.token)
        .body(&MovePreviewBody { parent_id: a.sales })
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "org.interact");
    assert_eq!(refused.extra["object"]["id"], json!(a.sales));
}

#[tokio::test]
async fn the_plan_of_a_move_says_what_would_change_before_anything_does() {
    let world = World::new().await;
    let a = acme(&world).await;
    // Chat is switched off in Research and on above it; a person is an Admin
    // there and of nothing else. He is a member of Research, so that the boss,
    // a member of the same tree, may be told his name (a preview counts and does
    // not name people the caller may not know: authz previews.rs).
    world
        .set_at(Target::Node(a.research), "chat.enabled", json!(false))
        .await;
    let dan = person(&world, "dan").await;
    join(&world, a.acme, a.research, &dan, "admin", &a.boss).await;
    let plan = world
        .app
        .call(ProjectMovePreview, &[&a.web])
        .token(&a.boss.token)
        .body(&MovePreviewBody {
            parent_id: a.research,
        })
        .send()
        .await
        .ok();
    assert_eq!(plan.node.id, a.web);
    assert_eq!(plan.from.id, a.acme);
    assert_eq!(plan.to.id, a.research);
    assert!(plan.blockers.is_empty());
    let chat = plan
        .settings
        .iter()
        .find(|setting| setting.key == "chat.enabled")
        .expect("chat resolves differently there");
    assert_eq!(chat.from, Some(json!(true)));
    assert_eq!(chat.to, Some(json!(false)));
    assert_eq!(
        plan.roles.gaining, 1,
        "Dan, who holds no role above it now, would inherit his role in Research"
    );
    assert_eq!(plan.roles.losing, 0, "Acme is still above it");
    assert!(
        plan.access
            .gaining
            .iter()
            .any(|person| person.user.id == dan.id()
                && person
                    .permissions
                    .iter()
                    .any(|permission| permission == "project.view")),
        "the access engine reports Dan's actual gain, not an empty placeholder: {:?}",
        plan.access
    );
    assert!(
        matches!(read(&world, &a.boss, a.web).await, Node::Project(project) if project.base.parent_id == Some(a.acme)),
        "reviewing keeps the original parent"
    );
    assert_eq!(plan.addresses.members, 0);
    assert!(
        plan.access.pages >= 2,
        "the project and its topic follow the defaults"
    );
    // Nothing moved.
    let Node::Project(web) = read(&world, &a.boss, a.web).await else {
        panic!("a project stays one");
    };
    assert_eq!(web.base.parent_id, Some(a.acme));
}

#[tokio::test]
async fn a_plan_of_an_org_counts_its_members_and_names_what_stops_it() {
    let world = World::new().await;
    let a = acme(&world).await;
    let dan = person(&world, "dan").await;
    join(&world, a.acme, a.sales, &dan, "viewer", &a.boss).await;
    let plan = world
        .app
        .call(OrgMovePreview, &[&a.sales])
        .token(&a.boss.token)
        .body(&MovePreviewBody {
            parent_id: a.research,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        plan.addresses.members, 1,
        "Dan's address begins with the org's path"
    );
    let blocked = world
        .app
        .call(OrgMovePreview, &[&a.sales])
        .token(&a.boss.token)
        .body(&MovePreviewBody { parent_id: a.web })
        .send()
        .await
        .ok();
    assert_eq!(blocked.blockers.len(), 1);
    assert_eq!(blocked.blockers[0].code, "node.wrong_parent");
    let eve = person(&world, "eve").await;
    let refused = world
        .app
        .call(OrgMovePreview, &[&a.sales])
        .token(&eve.token)
        .body(&MovePreviewBody {
            parent_id: a.research,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::AuthzDenied,
        "a plan is for those who may carry it out"
    );
}

#[tokio::test]
async fn tags_are_defined_at_any_place_and_listed_with_those_above() {
    let world = World::new().await;
    let a = acme(&world).await;
    let dan = person(&world, "dan").await;
    join(&world, a.acme, a.acme, &dan, "viewer", &a.boss).await;
    let define = |who: &Who, node: Id, name: &str, color: u32| {
        world
            .app
            .call(NodeTagCreate, &[&node])
            .token(&who.token)
            .body(&NodeTagCreateBody {
                name: name.to_owned(),
                color: Some(color),
            })
            .send()
    };
    let urgent = define(&a.boss, a.acme, "urgent", 3).await.ok();
    assert_eq!(urgent.node.id, a.acme);
    assert_eq!(urgent.color, 3);
    let mobile = define(&a.boss, a.web, "mobile", 5).await.ok();
    assert_eq!(mobile.node.id, a.web);
    let listed = world
        .app
        .call(NodeTagList, &[&a.web])
        .token(&dan.token)
        .send()
        .await
        .ok();
    assert_eq!(
        listed
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        vec!["urgent", "mobile"],
        "the org's first, then the project's"
    );
    let of_org = world
        .app
        .call(NodeTagList, &[&a.acme])
        .token(&dan.token)
        .send()
        .await
        .ok();
    assert_eq!(of_org.len(), 1);
    // A name is unique in its node without regard to case.
    let again = define(&a.boss, a.acme, "URGENT", 1).await.problem();
    assert_eq!(again.code, ErrorCode::ConflictExists);
    assert_eq!(
        define(&a.boss, a.acme, "x", 12).await.problem().code,
        ErrorCode::RequestInvalid,
        "a colour is an index of the palette"
    );
    assert_eq!(
        define(&a.boss, a.acme, "  ", 1).await.problem().code,
        ErrorCode::RequestInvalid
    );
    // A viewer defines none; whoever manages a topic defines its own, as at any place.
    let refused = define(&dan, a.web, "mine", 1).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "project.manage");
    assert_eq!(define(&a.boss, a.login, "t", 1).await.ok().node.id, a.login);
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "tag.created")
    );
}

#[tokio::test]
async fn automatic_tag_colors_are_unused_then_cycle_after_the_latest_manual_tag() {
    let world = World::new().await;
    let a = acme(&world).await;
    for color in 0..orch_api_contract::users::COLOR_COUNT {
        let tag = world
            .app
            .call(NodeTagCreate, &[&a.web])
            .token(&a.boss.token)
            .body(&NodeTagCreateBody {
                name: format!("Automatic {color}"),
                color: None,
            })
            .send()
            .await
            .ok();
        assert_eq!(
            tag.color, color,
            "own tags get distinct unused colors first"
        );
        world.advance(chrono::Duration::milliseconds(1));
    }
    world
        .app
        .call(NodeTagCreate, &[&a.web])
        .token(&a.boss.token)
        .body(&NodeTagCreateBody {
            name: "Manual".into(),
            color: Some(5),
        })
        .send()
        .await
        .ok();
    world.advance(chrono::Duration::milliseconds(1));
    let next = world
        .app
        .call(NodeTagCreate, &[&a.web])
        .token(&a.boss.token)
        .body(&NodeTagCreateBody {
            name: "After manual".into(),
            color: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        next.color, 6,
        "a full palette follows the last tag, rather than the number of rows"
    );
    let project = make_project(&world, &a.boss, a.acme, "Concurrent labels")
        .await
        .base
        .id;
    let left = world
        .app
        .call(NodeTagCreate, &[&project])
        .token(&a.boss.token)
        .body(&NodeTagCreateBody {
            name: "Left".into(),
            color: None,
        });
    let right = world
        .app
        .call(NodeTagCreate, &[&project])
        .token(&a.boss.token)
        .body(&NodeTagCreateBody {
            name: "Right".into(),
            color: None,
        });
    let (left, right) = tokio::join!(left.send(), right.send());
    let mut colors = [left.ok().color, right.ok().color];
    colors.sort_unstable();
    assert_eq!(colors, [0, 1]);
}

#[tokio::test]
async fn a_tag_is_renamed_and_deleting_it_takes_it_off_every_topic() {
    let world = World::new().await;
    let a = acme(&world).await;
    let tag = world
        .app
        .call(NodeTagCreate, &[&a.web])
        .token(&a.boss.token)
        .body(&NodeTagCreateBody {
            name: "mobile".to_owned(),
            color: Some(2),
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(TopicUpdate, &[&a.login])
        .token(&a.boss.token)
        .body(&TopicUpdateBody {
            status: None,
            tag_ids: Some(vec![tag.id]),
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let renamed = world
        .app
        .call(TagUpdate, &[&tag.id])
        .token(&a.boss.token)
        .body(&TagUpdateBody {
            name: Some("phone".to_owned()),
            color: Some(7),
        })
        .send()
        .await
        .ok();
    assert_eq!((renamed.name.as_str(), renamed.color), ("phone", 7));
    let Node::Topic(topic) = read(&world, &a.boss, a.login).await else {
        panic!("a topic stays one");
    };
    assert_eq!(topic.base.tags[0].name, "phone");
    let seq = topic.seq;
    let gone = world
        .app
        .call(TagDelete, &[&tag.id])
        .token(&a.boss.token)
        .send()
        .await;
    assert_eq!(gone.status, 204);
    let Node::Topic(topic) = read(&world, &a.boss, a.login).await else {
        panic!("a topic stays one");
    };
    assert!(topic.base.tags.is_empty(), "it is off every topic");
    assert!(topic.seq > seq, "the topic that carried it was stamped");
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "tag.deleted")
    );
    let missing = world
        .app
        .call(TagDelete, &[&tag.id])
        .token(&a.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}
