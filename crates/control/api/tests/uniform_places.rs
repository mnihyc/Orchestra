//! Places share announcement, inherited environment and tag policies, and role subjects use the same member word as page holders.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod rig;
mod support;

use orch_api_contract::access::{NodeRoleCreate, Role, RoleCreateBody, SubjectInput, SubjectKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{NodeTagCreate, NodeTagCreateBody};
use orch_api_contract::nodes::{Node, NodeArchive, NodeTagSet, NodeTagSetBody, Tag};
use orch_api_contract::settings::{NodeSettingDelete, NodeSettingSet, NodeSettingSetBody};
use orch_api_contract::topics::{
    AnnouncementCreateBody, NodeAnnouncementCreate, NodeAnnouncementList, NodeAnnouncementListQuery,
};
use orch_settings::Target;
use orch_types::{Id, new_id};
use places::{Who, join, make_department, make_org, make_project, make_topic, person, read};
use serde_json::json;
use support::World;

async fn choose(
    world: &World,
    who: &Who,
    node: Id,
    key: &str,
    value: serde_json::Value,
    locked: bool,
) {
    world
        .app
        .call(NodeSettingSet, &[&node, &key])
        .token(&who.token)
        .body(&NodeSettingSetBody {
            value,
            locked: Some(locked),
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}

async fn default(world: &World, who: &Who, topic: Id) -> Option<Id> {
    match read(world, who, topic).await {
        Node::Topic(row) => row.default_environment_id,
        _ => panic!("a topic was made"),
    }
}

fn tags(node: &Node) -> &[Tag] {
    match node {
        Node::Org(row) => &row.base.tags,
        Node::Project(row) => &row.base.tags,
        Node::Topic(row) => &row.base.tags,
        Node::Thread(row) => &row.base.tags,
    }
}

#[tokio::test]
async fn the_default_environment_resolves_at_every_place_and_an_ancestor_lock_decides_it() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Acme", "acme").await;
    let dep = make_department(&world, &owner, org.base.id, "Research", "research").await;
    let project = make_project(&world, &owner, dep.base.id, "Web").await;
    let topic = make_topic(&world, &owner, project.base.id, "Release").await;
    let ids = [new_id(), new_id(), new_id(), new_id(), new_id()];
    world
        .set_at(Target::System, "env.default_id", json!(ids[0]))
        .await;
    assert_eq!(default(&world, &owner, topic.base.id).await, Some(ids[0]));
    for (index, node) in [org.base.id, dep.base.id, project.base.id, topic.base.id]
        .into_iter()
        .enumerate()
    {
        choose(
            &world,
            &owner,
            node,
            "env.default_id",
            json!(ids[index + 1]),
            false,
        )
        .await;
        assert_eq!(
            default(&world, &owner, topic.base.id).await,
            Some(ids[index + 1])
        );
    }
    world
        .app
        .call(NodeSettingDelete, &[&topic.base.id, &"env.default_id"])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert_eq!(default(&world, &owner, topic.base.id).await, Some(ids[3]));
    choose(
        &world,
        &owner,
        dep.base.id,
        "env.default_id",
        json!(ids[2]),
        true,
    )
    .await;
    assert_eq!(default(&world, &owner, topic.base.id).await, Some(ids[2]));
    let locked = world
        .app
        .call(NodeSettingSet, &[&topic.base.id, &"env.default_id"])
        .token(&owner.token)
        .body(&NodeSettingSetBody {
            value: json!(ids[4]),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(locked.code, ErrorCode::AccessLocked);
    world
        .app
        .call(NodeSettingDelete, &[&dep.base.id, &"env.default_id"])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert_eq!(default(&world, &owner, topic.base.id).await, Some(ids[3]));
    let viewer = person(&world, "viewer").await;
    join(&world, org.base.id, org.base.id, &viewer, "viewer", &owner).await;
    let denied = world
        .app
        .call(NodeSettingSet, &[&topic.base.id, &"env.default_id"])
        .token(&viewer.token)
        .body(&NodeSettingSetBody {
            value: json!(ids[4]),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn every_kind_of_place_defines_and_carries_tags_with_the_same_limit_and_archive_guard() {
    let s = rig::Scene::new().await;
    let world = &s.rig.world;
    let thread = s.thread_by(&s.ada, "Work").await;
    let dep = make_department(world, &s.ada, s.tree.org.base.id, "Research", "research").await;
    let root_tag = world
        .app
        .call(NodeTagCreate, &[&s.tree.org.base.id])
        .token(&s.ada.token)
        .body(&NodeTagCreateBody {
            name: "shared label".to_owned(),
            color: None,
        })
        .send()
        .await
        .ok();
    for node in [
        s.tree.org.base.id,
        dep.base.id,
        s.tree.project.base.id,
        s.tree.topic.base.id,
        thread.base.id,
    ] {
        let own = world
            .app
            .call(NodeTagCreate, &[&node])
            .token(&s.ada.token)
            .body(&NodeTagCreateBody {
                name: "local label".to_owned(),
                color: None,
            })
            .send()
            .await
            .ok();
        let assigned = world
            .app
            .call(NodeTagSet, &[&node])
            .token(&s.ada.token)
            .body(&NodeTagSetBody {
                tag_ids: vec![root_tag.id, own.id, own.id],
                expected_revision: None,
            })
            .send()
            .await
            .ok();
        assert_eq!(
            tags(&assigned).len(),
            2,
            "duplicates are one assignment at every kind"
        );
        assert_eq!(tags(&read(world, &s.ada, node).await).len(), 2);
        let stale = world
            .app
            .call(NodeTagSet, &[&node])
            .token(&s.ada.token)
            .body(&NodeTagSetBody {
                tag_ids: Vec::new(),
                expected_revision: Some(0),
            })
            .send()
            .await
            .problem();
        assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
        let refused = world
            .app
            .call(NodeTagSet, &[&node])
            .token(&s.cai.token)
            .body(&NodeTagSetBody {
                tag_ids: Vec::new(),
                expected_revision: None,
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
    }
    choose(
        world,
        &s.ada,
        s.tree.org.base.id,
        "tags.max_per_place",
        json!(0),
        false,
    )
    .await;
    let too_many = world
        .app
        .call(NodeTagSet, &[&dep.base.id])
        .token(&s.ada.token)
        .body(&NodeTagSetBody {
            tag_ids: vec![root_tag.id],
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(too_many.code, ErrorCode::RequestInvalid);
    let review = places::archive_body(world, &s.ada.token, s.tree.org.base.id, true).await;
    world
        .app
        .call(NodeArchive, &[&s.tree.org.base.id])
        .token(&s.ada.token)
        .body(&review)
        .send()
        .await
        .ok();
    let archived = world
        .app
        .call(NodeTagSet, &[&thread.base.id])
        .token(&s.ada.token)
        .body(&NodeTagSetBody {
            tag_ids: Vec::new(),
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(archived.code, ErrorCode::NodeArchived);
    assert_eq!(
        tags(&read(world, &s.ada, thread.base.id).await).len(),
        2,
        "archive preserves the labels"
    );
}

fn notice(title: &str) -> AnnouncementCreateBody {
    AnnouncementCreateBody {
        title: title.to_owned(),
        body: "Details".to_owned(),
        category: None,
        severity: None,
        targets: None,
        delivery: None,
        fallback: None,
        wake_idle: None,
    }
}

#[tokio::test]
async fn org_department_project_and_topic_share_announcements_and_equal_clock_pages_keep_every_delivery()
 {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Acme", "acme").await;
    let dep = make_department(&world, &owner, org.base.id, "Research", "research").await;
    let project = make_project(&world, &owner, dep.base.id, "Web").await;
    let topic = make_topic(&world, &owner, project.base.id, "Release").await;
    let private = world
        .app
        .call(orch_api_contract::nodes::ProjectCreate, &[&dep.base.id])
        .token(&owner.token)
        .body(&orch_api_contract::nodes::ProjectCreateBody {
            name: "Private".to_owned(),
            description: None,
            access: Some(orch_api_contract::nodes::NewNodeAccess::Private),
        })
        .send()
        .await
        .ok();
    let hidden = make_topic(&world, &owner, private.base.id, "Hidden").await;
    let viewer = person(&world, "viewer").await;
    join(&world, org.base.id, org.base.id, &viewer, "viewer", &owner).await;
    for place in [org.base.id, dep.base.id, project.base.id, topic.base.id] {
        let denied = world
            .app
            .call(NodeAnnouncementCreate, &[&place])
            .token(&viewer.token)
            .body(&notice("Denied"))
            .send()
            .await
            .problem();
        assert_eq!(denied.code, ErrorCode::AuthzDenied);
        let sent = world
            .app
            .call(NodeAnnouncementCreate, &[&place])
            .token(&owner.token)
            .body(&notice("Shared update"))
            .send()
            .await
            .ok();
        assert_eq!(sent.announcements.len(), 1);
        assert_eq!(sent.announcements[0].topic_id, topic.base.id);
        assert!(
            !sent
                .announcements
                .iter()
                .any(|row| row.topic_id == hidden.base.id),
            "even an owner skips a private branch below the origin"
        );
    }
    world
        .app
        .call(NodeAnnouncementCreate, &[&hidden.base.id])
        .token(&owner.token)
        .body(&notice("Private words"))
        .send()
        .await
        .ok();
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    loop {
        let page = world
            .app
            .call(NodeAnnouncementList, &[&org.base.id])
            .token(&viewer.token)
            .query(&NodeAnnouncementListQuery {
                limit: Some(1),
                cursor,
                ..Default::default()
            })
            .send()
            .await
            .ok();
        for row in page.items {
            assert!(seen.insert(row.id));
            assert_ne!(row.title, "Private words");
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 4);
    let review = places::archive_body(&world, &owner.token, dep.base.id, true).await;
    world
        .app
        .call(NodeArchive, &[&dep.base.id])
        .token(&owner.token)
        .body(&review)
        .send()
        .await
        .ok();
    assert_eq!(
        world
            .app
            .call(NodeAnnouncementCreate, &[&dep.base.id])
            .token(&owner.token)
            .body(&notice("While archived"))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NodeArchived
    );
}

#[tokio::test]
async fn a_members_subject_round_trips_through_the_api_and_matches_only_the_named_org_domain() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Acme", "acme").await;
    let dep = make_department(&world, &owner, org.base.id, "Research", "research").await;
    let member = person(&world, "member").await;
    let sibling = person(&world, "sibling").await;
    join(&world, org.base.id, dep.base.id, &member, "viewer", &owner).await;
    join(&world, org.base.id, org.base.id, &sibling, "viewer", &owner).await;
    let project = make_project(&world, &owner, org.base.id, "Shared work").await;
    let given = world
        .app
        .call(NodeRoleCreate, &[&project.base.id])
        .token(&owner.token)
        .body(&RoleCreateBody {
            subject: SubjectInput {
                kind: SubjectKind::Members,
                id: dep.base.id,
                role: None,
            },
            role: Role::Developer,
            expires_at: None,
            note: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        serde_json::to_value(&given).unwrap()["subject"]["kind"],
        "members"
    );
    let Node::Project(allowed) = read(&world, &member, project.base.id).await else {
        panic!("a project was made")
    };
    let Node::Project(outside) = read(&world, &sibling, project.base.id).await else {
        panic!("a project was made")
    };
    assert!(
        allowed
            .base
            .access
            .permissions
            .iter()
            .any(|permission| permission == "project.interact")
    );
    assert!(
        !outside
            .base
            .access
            .permissions
            .iter()
            .any(|permission| permission == "project.interact")
    );
    let listed = world
        .app
        .call(orch_api_contract::access::NodeRoleList, &[&project.base.id])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert!(
        listed
            .own
            .iter()
            .any(|row| row.id == given.id && row.subject.kind == SubjectKind::Members)
    );
}
