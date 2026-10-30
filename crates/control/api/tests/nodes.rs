//! The tree over HTTP: making an org, a department, a project and a topic, reading them, renaming, archiving and subscribing, with what each is refused for.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::common::SubscriptionLevel;
use orch_api_contract::nodes::{
    ArchivedFilter, AutoAction, AutoField, FieldSource, NewNodeAccess, Node, NodeArchive, NodeAuto,
    NodeAutoBody, NodeChildList, NodeChildListQuery, NodeGet, NodeKind, NodeMoveBody,
    NodeSubscriptionDelete, NodeSubscriptionSet, NodeSubscriptionSetBody, NodeUnarchive,
    NodeUpdate, NodeUpdateBody, OrgBySlugGet, OrgChildCreate, OrgChildCreateBody, OrgCreate,
    OrgCreateBody, OrgList, OrgMove, OrgType, PageState, ProjectCreate, ProjectCreateBody,
    TopicCreate, TopicCreateBody, TopicCreateReply,
};
use places::{
    admin, children, first_missing, join, make_department, make_org, make_project, make_topic,
    missing_permissions, person, read,
};
use support::{Statements, World};

#[tokio::test]
async fn a_person_makes_a_team_org_and_is_its_first_owner_and_member() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Acme", "acme").await;
    assert_eq!(org.slug, "acme");
    assert_eq!(org.address, "acme");
    assert_eq!(org.depth, 0);
    assert_eq!(org.org_kind, OrgType::Team);
    assert!(!org.closed && !org.flat);
    assert_eq!(org.base.parent_id, None);
    assert_eq!(org.base.org_id, org.base.id);
    assert_eq!(org.base.owner.username, "ada");
    assert_eq!(org.base.owners, 1);
    assert_eq!(org.member_count, 1);
    assert_eq!(
        org.project_count, 1,
        "a team's org starts with a General project"
    );
    assert!(org.base.access.owns);
    assert!(org.base.access.administers || org.base.access.role.is_some());
    assert_eq!(org.base.page, PageState::Follows);
    assert!(!org.base.chat_id.is_nil());
    assert_eq!(org.base.path.len(), 1);
    assert_eq!(org.base.path[0].name.as_deref(), Some("Acme"));
    let audit = world.audit().await;
    assert_eq!(
        audit
            .iter()
            .filter(|action| *action == "node.created")
            .count(),
        2,
        "the org and its General project each leave a row: {audit:?}"
    );
}

#[tokio::test]
async fn the_general_project_is_in_the_new_org() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Acme", "acme").await;
    let inside = children(&world, &ada, org.base.id).await;
    assert_eq!(inside.len(), 1);
    let Node::Project(project) = read(&world, &ada, inside[0]).await else {
        panic!("the child of an org is a project here");
    };
    assert_eq!(project.base.name, "General");
    assert_eq!(project.base.parent_id, Some(org.base.id));
}

#[tokio::test]
async fn a_short_name_belongs_to_one_root_org_and_never_to_a_username() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bob = person(&world, "bob").await;
    make_org(&world, &ada, "Acme", "acme").await;
    places::invite_to_make_org(&world, &bob).await;
    for slug in ["acme", "bob"] {
        let taken = world
            .app
            .call(OrgCreate, &[])
            .token(&bob.token)
            .body(&OrgCreateBody {
                name: "Again".to_owned(),
                slug: slug.to_owned(),
            })
            .send()
            .await
            .problem();
        assert_eq!(taken.code, ErrorCode::ConflictExists, "{slug}");
    }
}

#[tokio::test]
async fn a_root_orgs_short_name_is_at_least_three_characters_of_the_allowed_kind() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    for slug in ["ab", "Acme", "-acme", "a b", ""] {
        let refused = world
            .app
            .call(OrgCreate, &[])
            .token(&ada.token)
            .body(&OrgCreateBody {
                name: "Acme".to_owned(),
                slug: slug.to_owned(),
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{slug:?}");
        assert!(
            refused.extra["errors"][0]["path"] == "slug",
            "the fault names the member: {slug:?}"
        );
    }
}

#[tokio::test]
async fn nobody_makes_an_org_without_signing_in() {
    let world = World::new().await;
    let refused = world
        .app
        .call(OrgCreate, &[])
        .body(&OrgCreateBody {
            name: "Acme".to_owned(),
            slug: "acme".to_owned(),
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn a_department_is_an_org_inside_an_org_with_its_address_and_depth() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let acme = make_org(&world, &ada, "Acme", "acme").await;
    let research = make_department(&world, &ada, acme.base.id, "Research", "research").await;
    assert_eq!(research.base.parent_id, Some(acme.base.id));
    assert_eq!(research.base.org_id, acme.base.id);
    assert_eq!(research.address, "acme.research");
    assert_eq!(research.depth, 1);
    let ml = make_department(&world, &ada, research.base.id, "ML", "ml").await;
    assert_eq!(ml.address, "acme.research.ml");
    assert_eq!(ml.depth, 2);
    assert_eq!(
        ml.base.path.iter().map(|each| each.id).collect::<Vec<_>>(),
        vec![acme.base.id, research.base.id, ml.base.id]
    );
    let again = world
        .app
        .call(OrgChildCreate, &[&acme.base.id])
        .token(&ada.token)
        .body(&OrgChildCreateBody {
            name: "Research twice".to_owned(),
            slug: "research".to_owned(),
            description: None,
            general: None,
            access: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        again.code,
        ErrorCode::ConflictExists,
        "a short name is unique among the orgs of one parent"
    );
    let found = world
        .app
        .call(OrgBySlugGet, &[&"acme.research.ml"])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(found.base.id, ml.base.id, "an address finds the org");
}

#[tokio::test]
async fn a_department_may_be_given_a_general_project_when_asked() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let acme = make_org(&world, &ada, "Acme", "acme").await;
    let sales = world
        .app
        .call(OrgChildCreate, &[&acme.base.id])
        .token(&ada.token)
        .body(&OrgChildCreateBody {
            name: "Sales".to_owned(),
            slug: "sales".to_owned(),
            description: Some("Who sells".to_owned()),
            general: Some(true),
            access: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(sales.project_count, 1);
    assert_eq!(sales.base.description, "Who sells");
    assert_eq!(sales.base.description_source, FieldSource::User);
}

#[tokio::test]
async fn orgs_may_lie_in_one_another_only_as_deep_as_the_system_allows() {
    let mut config = orch_config::ServerConfig::default();
    config.limits.org_depth = 3;
    let world = World::with(config).await;
    let ada = person(&world, "ada").await;
    let acme = make_org(&world, &ada, "Acme", "acme").await;
    let two = make_department(&world, &ada, acme.base.id, "Two", "two").await;
    let three = make_department(&world, &ada, two.base.id, "Three", "three").await;
    assert_eq!(three.depth, 2);
    let refused = world
        .app
        .call(OrgChildCreate, &[&three.base.id])
        .token(&ada.token)
        .body(&OrgChildCreateBody {
            name: "Four".to_owned(),
            slug: "four".to_owned(),
            description: None,
            general: None,
            access: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::OrgTooDeep);
    assert_eq!(refused.extra["limit"], 3);
}

#[tokio::test]
async fn a_personal_org_holds_no_org_inside_it_as_shipped() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let own = ada.person.personal_org;
    let refused = world
        .app
        .call(OrgChildCreate, &[&own])
        .token(&ada.token)
        .body(&OrgChildCreateBody {
            name: "Team".to_owned(),
            slug: "team".to_owned(),
            description: None,
            general: None,
            access: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::OrgFlat);
    assert_eq!(refused.extra["org"]["id"], serde_json::json!(own));
    // A project is made there all the same: flat forbids orgs, not work.
    let project = make_project(&world, &ada, own, "Notes").await;
    assert_eq!(project.base.org_id, own);
}

#[tokio::test]
async fn a_project_and_a_topic_are_made_by_whoever_the_engine_lets() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let dan = person(&world, "dan").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    join(&world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    // A viewer reads the org and may not make a project in it.
    let refused = world
        .app
        .call(ProjectCreate, &[&acme.base.id])
        .token(&dan.token)
        .body(&ProjectCreateBody {
            name: "Web".to_owned(),
            description: None,
            access: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "org.interact");
    assert_eq!(
        missing_permissions(&refused),
        vec!["org.interact".to_owned()]
    );
    let missing = first_missing(&refused);
    assert_eq!(missing["object"]["id"], serde_json::json!(acme.base.id));
    assert_eq!(missing["object"]["kind"], "org");
    assert_eq!(
        missing["ask"][0]["username"], "boss",
        "the people who can give it are named, not their ids"
    );
    assert_eq!(refused.extra["requestable"], true);
    // The owner makes one, and a topic in it.
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    assert_eq!(web.base.parent_id, Some(acme.base.id));
    assert_eq!(web.topic_count, 0);
    let topic = make_topic(&world, &boss, web.base.id, "Login").await;
    assert_eq!(topic.base.title_source, FieldSource::User);
    assert_eq!(topic.thread_count, 0);
    assert_eq!(topic.seq, 1, "making a topic stamps its feed once");
    // A viewer cannot make a topic either, and is told what is missing.
    let refused = world
        .app
        .call(TopicCreate, &[&web.base.id])
        .token(&dan.token)
        .body(&TopicCreateBody::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "project.interact");
}

#[tokio::test]
async fn a_topic_made_without_a_name_is_called_new_topic_and_kept_by_automation() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let own = ada.person.personal_org;
    let project = make_project(&world, &ada, own, "Notes").await;
    let reply = world
        .app
        .call(TopicCreate, &[&project.base.id])
        .token(&ada.token)
        .body(&TopicCreateBody::default())
        .send()
        .await
        .ok();
    let TopicCreateReply::Topic(topic) = reply else {
        panic!("a topic alone was asked for");
    };
    assert_eq!(topic.base.name, "New topic");
    assert_eq!(topic.base.title_source, FieldSource::Auto);
}

#[tokio::test]
async fn a_first_thread_that_cannot_be_bound_makes_no_topic_either() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let project = make_project(&world, &ada, ada.person.personal_org, "Notes").await;
    // A topic is made with its first thread in one act, or not at all: a thread named with nothing
    // it could run on is refused, and the topic with it.
    let before = children(&world, &ada, project.base.id).await;
    let refused = world
        .app
        .call(TopicCreate, &[&project.base.id])
        .token(&ada.token)
        .raw_body(&serde_json::json!({ "first_thread": {} }))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::BindingIncompatible);
    assert_eq!(children(&world, &ada, project.base.id).await, before);
}

#[tokio::test]
async fn a_private_project_is_its_makers_alone_until_somebody_is_named() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let dan = person(&world, "dan").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    join(&world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let open = make_project(&world, &boss, acme.base.id, "Open").await;
    let secret = world
        .app
        .call(ProjectCreate, &[&acme.base.id])
        .token(&boss.token)
        .body(&ProjectCreateBody {
            name: "Secret".to_owned(),
            description: None,
            access: Some(NewNodeAccess::Private),
        })
        .send()
        .await
        .ok();
    assert_eq!(secret.base.page, PageState::Restricted);
    // Dan, a viewer of the org, sees the open project and not the secret one.
    let seen = children(&world, &dan, acme.base.id).await;
    assert!(seen.contains(&open.base.id));
    assert!(!seen.contains(&secret.base.id));
    let refused = world
        .app
        .call(NodeGet, &[&secret.base.id])
        .token(&dan.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "project.view");
    // The maker sees both.
    let seen = children(&world, &boss, acme.base.id).await;
    assert!(seen.contains(&secret.base.id));
}

#[tokio::test]
async fn a_stranger_is_refused_a_node_with_what_is_missing_and_who_can_give_it() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let eve = person(&world, "eve").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let refused = world
        .app
        .call(NodeGet, &[&acme.base.id])
        .token(&eve.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.status, 403);
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "org.view");
    assert_eq!(
        refused.extra["object"],
        serde_json::json!({ "kind": "org", "id": acme.base.id })
    );
    assert_eq!(missing_permissions(&refused), vec!["org.view".to_owned()]);
    assert_eq!(first_missing(&refused)["ask"][0]["username"], "boss");
    assert_eq!(refused.extra["override"], false);
    assert_eq!(refused.extra["requestable"], true);
    assert!(refused.extra["missing"][0]["closed"].is_null());
}

#[tokio::test]
async fn a_node_that_is_not_there_is_not_found_and_not_refused() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let refused = world
        .app
        .call(NodeGet, &[&orch_types::new_id()])
        .token(&ada.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    let refused = world
        .app
        .call(NodeGet, &[&"not-an-id"])
        .token(&ada.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn an_admin_of_the_place_reads_a_private_page_by_administering_it_and_only_as_admin() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let root = admin(&world, "root").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let private = world
        .app
        .call(ProjectCreate, &[&acme.base.id])
        .token(&boss.token)
        .body(&ProjectCreateBody {
            name: "Secret".to_owned(),
            description: None,
            access: Some(NewNodeAccess::Private),
        })
        .send()
        .await
        .ok();
    // With As admin on, a system admin administers everything: the private page names only its
    // owners, and administering it gives its View and Manage all the same, marked as such.
    let read = world
        .app
        .call(NodeGet, &[&private.base.id])
        .token(&root.token)
        .send()
        .await
        .ok();
    let Node::Project(project) = read else {
        panic!("the project is read");
    };
    let marker = project.base.access.admin_access.as_ref().unwrap();
    assert!(marker.private);
    // With the switch off he is nobody there, and nothing is offered to force.
    places::admin_mode(&world, &root, false).await;
    let refused = world
        .app
        .call(NodeGet, &[&private.base.id])
        .token(&root.token)
        .send()
        .await
        .problem();
    assert!(
        matches!(refused.code, ErrorCode::AuthzDenied | ErrorCode::NotFound),
        "{refused:?}"
    );
    assert_ne!(
        refused.extra.get("override"),
        Some(&serde_json::json!(true))
    );
}

#[tokio::test]
async fn the_orgs_a_person_belongs_to_are_listed_their_own_first() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let zed = person(&world, "zed").await;
    let acme = make_org(&world, &ada, "Acme", "acme").await;
    let beta = make_org(&world, &zed, "Beta", "beta").await;
    join(&world, beta.base.id, beta.base.id, &ada, "viewer", &zed).await;
    let listed = world
        .app
        .call(OrgList, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    let slugs: Vec<&str> = listed.iter().map(|org| org.slug.as_str()).collect();
    assert_eq!(slugs, vec!["ada", "acme", "beta"]);
    assert_eq!(listed[0].org_kind, OrgType::Personal);
    assert!(listed[1].base.id == acme.base.id);
}

#[tokio::test]
async fn renaming_is_by_a_manager_hands_the_title_to_a_person_and_needs_the_revision_seen() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let dan = person(&world, "dan").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    join(&world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    let topic = make_topic(&world, &boss, web.base.id, "Login").await;

    let refused = world
        .app
        .call(NodeUpdate, &[&web.base.id])
        .token(&dan.token)
        .body(&NodeUpdateBody {
            readme_document_id: None,
            color: None,
            custom_color: None,
            name: Some("Website".to_owned()),
            description: None,
            expected_revision: web.base.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "project.manage");

    let renamed = world
        .app
        .call(NodeUpdate, &[&web.base.id])
        .token(&boss.token)
        .body(&NodeUpdateBody {
            readme_document_id: None,
            color: None,
            custom_color: None,
            name: Some("Website".to_owned()),
            description: Some("Public site".to_owned()),
            expected_revision: web.base.revision,
        })
        .send()
        .await
        .ok();
    let Node::Project(renamed) = renamed else {
        panic!("a project stays one");
    };
    assert_eq!(renamed.base.name, "Website");
    assert_eq!(renamed.base.description, "Public site");
    assert_eq!(renamed.base.title_source, FieldSource::User);
    assert_eq!(renamed.base.description_source, FieldSource::User);
    assert_eq!(renamed.base.revision, web.base.revision + 1);

    // The revision that was seen is stale now, and the answer shows the node.
    let stale = world
        .app
        .call(NodeUpdate, &[&web.base.id])
        .token(&boss.token)
        .body(&NodeUpdateBody {
            readme_document_id: None,
            color: None,
            custom_color: None,
            name: Some("Another".to_owned()),
            description: None,
            expected_revision: web.base.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["name"], "Website");

    let empty = world
        .app
        .call(NodeUpdate, &[&topic.base.id])
        .token(&boss.token)
        .body(&NodeUpdateBody {
            readme_document_id: None,
            color: None,
            custom_color: None,
            name: Some("   ".to_owned()),
            description: None,
            expected_revision: topic.base.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(empty.code, ErrorCode::RequestInvalid);
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "node.updated"),
        "the rename left its row"
    );
}

#[tokio::test]
async fn handing_a_field_back_to_automation_needs_the_matching_setting_to_be_on() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let project = make_project(&world, &ada, ada.person.personal_org, "Notes").await;
    let topic = make_topic(&world, &ada, project.base.id, "Plans").await;
    assert_eq!(topic.base.title_source, FieldSource::User);
    // `auto.title` is on as shipped.
    let back = world
        .app
        .call(NodeAuto, &[&topic.base.id])
        .token(&ada.token)
        .body(&NodeAutoBody {
            field: AutoField::Title,
            action: AutoAction::Resume,
        })
        .send()
        .await
        .ok();
    let Node::Topic(back) = back else {
        panic!("a topic stays one");
    };
    assert_eq!(back.base.title_source, FieldSource::Auto);
    // `auto.description` is off as shipped.
    let refused = world
        .app
        .call(NodeAuto, &[&topic.base.id])
        .token(&ada.token)
        .body(&NodeAutoBody {
            field: AutoField::Description,
            action: AutoAction::Resume,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AutoDisabled);
    assert_eq!(refused.extra["setting"], "auto.description");
}

#[tokio::test]
async fn archiving_closes_a_project_to_changes_and_unarchiving_opens_it_again() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let dan = person(&world, "dan").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    join(&world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    let topic = make_topic(&world, &boss, web.base.id, "Login").await;

    let refused = world
        .app
        .call(NodeArchive, &[&web.base.id])
        .token(&dan.token)
        .body(&places::archive_body(&world, &dan.token, web.base.id, true).await)
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "project.archive");

    let archived = world
        .app
        .call(NodeArchive, &[&web.base.id])
        .token(&boss.token)
        .body(&places::archive_body(&world, &boss.token, web.base.id, true).await)
        .send()
        .await
        .ok();
    let Node::Project(archived) = archived.node else {
        panic!("a project stays one");
    };
    assert!(archived.base.archived);
    assert!(archived.base.archived_at.is_some());
    assert_eq!(archived.base.archived_by.as_ref().unwrap().username, "boss");

    // Everything inside is archived with it.
    let Node::Topic(inside) = read(&world, &boss, topic.base.id).await else {
        panic!("a topic stays one");
    };
    assert!(inside.base.archived);
    assert!(
        inside.base.archived_at.is_none(),
        "it is archived through its project"
    );

    // A change under it is refused as one under an archived node.
    let refused = world
        .app
        .call(NodeUpdate, &[&topic.base.id])
        .token(&boss.token)
        .body(&NodeUpdateBody {
            readme_document_id: None,
            color: None,
            custom_color: None,
            name: Some("Sign in".to_owned()),
            description: None,
            expected_revision: inside.base.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeArchived);
    assert_eq!(
        refused.extra["archived_node"]["id"],
        serde_json::json!(web.base.id)
    );
    let refused = world
        .app
        .call(TopicCreate, &[&web.base.id])
        .token(&boss.token)
        .body(&TopicCreateBody::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeArchived);

    // The archive filter of a list.
    let listed = |filter: ArchivedFilter| {
        let world = &world;
        let boss = &boss;
        let id = acme.base.id;
        async move {
            world
                .app
                .call(NodeChildList, &[&id])
                .token(&boss.token)
                .query(&NodeChildListQuery {
                    archived: Some(filter),
                    ..NodeChildListQuery::default()
                })
                .send()
                .await
                .ok()
                .items
                .len()
        }
    };
    assert_eq!(listed(ArchivedFilter::Exclude).await, 1, "only General");
    assert_eq!(listed(ArchivedFilter::Include).await, 2);
    assert_eq!(listed(ArchivedFilter::Only).await, 1);

    let back = world
        .app
        .call(NodeUnarchive, &[&web.base.id])
        .token(&boss.token)
        .body(&places::archive_body(&world, &boss.token, web.base.id, false).await)
        .send()
        .await
        .ok();
    let Node::Project(back) = back.node else {
        panic!("a project stays one");
    };
    assert!(!back.base.archived);
    let audit = world.audit().await;
    assert!(audit.iter().any(|action| action == "node.archived"));
    assert!(audit.iter().any(|action| action == "node.unarchived"));
}

#[tokio::test]
async fn orgs_and_departments_archive_their_contents_read_only_and_the_same_holder_brings_them_back()
 {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let acme = make_org(&world, &ada, "Acme", "acme").await;
    let department =
        make_department(&world, &ada, acme.base.id, "Engineering", "engineering").await;
    let project = make_project(&world, &ada, department.base.id, "Web").await;
    let viewer = person(&world, "viewer").await;
    join(&world, acme.base.id, acme.base.id, &viewer, "viewer", &ada).await;
    for id in [department.base.id, acme.base.id] {
        let denied = world
            .app
            .call(NodeArchive, &[&id])
            .token(&viewer.token)
            .body(&places::archive_body(&world, &viewer.token, id, true).await)
            .send()
            .await
            .problem();
        assert_eq!(denied.code, ErrorCode::AuthzDenied);
        assert_eq!(denied.extra["required"], "org.archive");
        let archived = world
            .app
            .call(NodeArchive, &[&id])
            .token(&ada.token)
            .body(&places::archive_body(&world, &ada.token, id, true).await)
            .send()
            .await
            .ok();
        assert!(
            matches!(archived.node, Node::Org(ref org) if org.base.archived && org.base.archived_at.is_some())
        );
        assert!(
            matches!(read(&world, &viewer, project.base.id).await, Node::Project(ref row) if row.base.archived)
        );
        let refused = world
            .app
            .call(ProjectCreate, &[&department.base.id])
            .token(&ada.token)
            .body(&ProjectCreateBody {
                name: "While archived".to_owned(),
                description: None,
                access: None,
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::NodeArchived);
        let restored = world
            .app
            .call(NodeUnarchive, &[&id])
            .token(&ada.token)
            .body(&places::archive_body(&world, &ada.token, id, false).await)
            .send()
            .await
            .ok();
        assert!(matches!(restored.node, Node::Org(ref org) if !org.base.archived));
        assert!(
            matches!(read(&world, &viewer, project.base.id).await, Node::Project(ref row) if !row.base.archived)
        );
    }
    make_project(&world, &ada, department.base.id, "After restore").await;
}

#[tokio::test]
async fn the_system_is_not_an_archivable_org_even_for_its_admin() {
    let world = World::new().await;
    let boss = admin(&world, "boss").await;
    let refused = world
        .app
        .call(
            orch_api_contract::nodes::NodeArchivePreview,
            &[&orch_types::Id::nil()],
        )
        .token(&boss.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_subscription_is_the_callers_own_and_the_nearest_one_counts() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let dan = person(&world, "dan").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    join(&world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    let access = world
        .app
        .call(NodeSubscriptionSet, &[&acme.base.id])
        .token(&dan.token)
        .body(&NodeSubscriptionSetBody {
            level: SubscriptionLevel::Important,
        })
        .send()
        .await
        .ok();
    assert_eq!(access.subscription, Some(SubscriptionLevel::Important));
    assert_eq!(access.effective_subscription, SubscriptionLevel::Important);
    let Node::Project(project) = read(&world, &dan, web.base.id).await else {
        panic!("a project stays one");
    };
    assert_eq!(project.base.access.subscription, None);
    assert_eq!(
        project.base.access.effective_subscription,
        SubscriptionLevel::Important,
        "the org's row reaches the project below it"
    );
    let Node::Project(for_boss) = read(&world, &boss, web.base.id).await else {
        panic!("a project stays one");
    };
    assert_eq!(
        for_boss.base.access.effective_subscription,
        SubscriptionLevel::Off
    );
    let access = world
        .app
        .call(NodeSubscriptionDelete, &[&acme.base.id])
        .token(&dan.token)
        .send()
        .await
        .ok();
    assert_eq!(access.subscription, None);
    assert_eq!(access.effective_subscription, SubscriptionLevel::Off);
    // A stranger subscribes to nothing they cannot view.
    let eve = person(&world, "eve").await;
    let refused = world
        .app
        .call(NodeSubscriptionSet, &[&acme.base.id])
        .token(&eve.token)
        .body(&NodeSubscriptionSetBody {
            level: SubscriptionLevel::All,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn the_children_of_a_node_are_listed_in_the_order_made_and_only_as_far_as_viewable() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let mut made = Vec::new();
    for name in ["One", "Two", "Three", "Four"] {
        made.push(
            make_project(&world, &boss, acme.base.id, name)
                .await
                .base
                .id,
        );
    }
    let first = world
        .app
        .call(NodeChildList, &[&acme.base.id])
        .token(&boss.token)
        .query(&NodeChildListQuery {
            limit: Some(3),
            ..NodeChildListQuery::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(first.items.len(), 3);
    let cursor = first.next_cursor.clone().expect("more follow");
    let rest = world
        .app
        .call(NodeChildList, &[&acme.base.id])
        .token(&boss.token)
        .query(&NodeChildListQuery {
            limit: Some(3),
            cursor: Some(cursor),
            ..NodeChildListQuery::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(rest.items.len(), 2, "General was first; the four follow");
    assert!(rest.next_cursor.is_none());
    let all: Vec<_> = first
        .items
        .iter()
        .chain(&rest.items)
        .filter_map(orch_api_contract::common::Listed::as_visible)
        .map(orch_api_core::routes::nodes::node_id)
        .collect();
    assert_eq!(&all[1..], made.as_slice());
    let bad = world
        .app
        .call(NodeChildList, &[&acme.base.id])
        .token(&boss.token)
        .raw_query("cursor=nonsense")
        .send()
        .await
        .problem();
    assert_eq!(bad.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_list_of_children_costs_the_same_statements_for_one_node_and_for_many() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    async fn measure(world: &World, boss: &places::Who, id: orch_types::Id) -> (usize, usize) {
        let call = world
            .app
            .call(NodeChildList, &[&id])
            .token(&boss.token)
            .query(&NodeChildListQuery::default());
        let before = Statements::now();
        let listed = call.send().await.ok();
        (Statements::now() - before, listed.items.len())
    }
    // The first read of a session fills what later reads of it keep; what is compared is a list's own.
    measure(&world, &boss, acme.base.id).await;
    let (few, few_count) = measure(&world, &boss, acme.base.id).await;
    for index in 0..12 {
        make_project(&world, &boss, acme.base.id, &format!("P{index}")).await;
    }
    let (many, many_count) = measure(&world, &boss, acme.base.id).await;
    assert_eq!((few_count, many_count), (1, 13));
    assert_eq!(few, many, "{few} statements for 1 child, {many} for 13");
}

#[tokio::test]
async fn an_api_token_kept_to_a_subtree_sees_nothing_outside_it() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    let docs = make_project(&world, &boss, acme.base.id, "Docs").await;
    let token = orch_api_core::middleware::authentication::new_token(
        orch_api_core::middleware::authentication::API_TOKEN_PREFIX,
    );
    sqlx::query(
        "INSERT INTO api_tokens (id, user_id, name, token_hash, scopes, node_id)
         VALUES ($1, $2, 'kept', $3, ARRAY['*'], $4)",
    )
    .bind(orch_types::new_id())
    .bind(boss.id())
    .bind(orch_api_core::middleware::authentication::token_hash(
        &token,
    ))
    .bind(web.base.id)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let inside = world
        .app
        .call(NodeGet, &[&web.base.id])
        .token(&token)
        .send()
        .await
        .ok();
    assert_eq!(orch_api_core::routes::nodes::node_id(&inside), web.base.id);
    let outside = world
        .app
        .call(NodeGet, &[&docs.base.id])
        .token(&token)
        .send()
        .await
        .problem();
    assert_eq!(outside.code, ErrorCode::AuthzDenied);
    assert_eq!(NodeKind::Project, NodeKind::Project);
}

#[tokio::test]
async fn a_person_suspended_in_an_org_is_refused_everything_there_and_told_which_org() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let dan = person(&world, "dan").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    join(&world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    {
        let mut conn = world.conn().await;
        orch_db::queries::access::testing::suspend_in(&mut conn, acme.base.id, dan.id(), boss.id())
            .await
            .unwrap();
    }
    let refused = world
        .app
        .call(NodeGet, &[&acme.base.id])
        .token(&dan.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::OrgSuspended);
    assert_eq!(refused.extra["org"]["id"], serde_json::json!(acme.base.id));
    assert_eq!(refused.extra["org"]["name"], "Acme");
}

#[tokio::test]
async fn one_short_name_serves_orgs_of_different_parents_and_is_refused_only_beside_itself() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let acme = make_org(&world, &ada, "Acme", "acme").await;
    let beta = make_org(&world, &ada, "Beta", "beta").await;
    // The same name in two orgs, and in two parents of one org, is two addresses.
    let acme_dep2 = make_department(&world, &ada, acme.base.id, "dep2", "dep2").await;
    let beta_dep2 = make_department(&world, &ada, beta.base.id, "dep2", "dep2").await;
    let inner = make_department(&world, &ada, acme_dep2.base.id, "dep2", "dep2").await;
    assert_eq!(acme_dep2.address, "acme.dep2");
    assert_eq!(beta_dep2.address, "beta.dep2");
    assert_eq!(inner.address, "acme.dep2.dep2");
    // Beside itself one address would name two orgs: refused, naming the org that has it and offering a free one.
    let again = world
        .app
        .call(OrgChildCreate, &[&acme.base.id])
        .token(&ada.token)
        .body(&OrgChildCreateBody {
            name: "Department two".to_owned(),
            slug: "dep2".to_owned(),
            description: None,
            general: None,
            access: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(again.code, ErrorCode::ConflictExists);
    assert_eq!(
        again.extra["existing_id"],
        serde_json::json!(acme_dep2.base.id)
    );
    assert_eq!(again.extra["existing"]["name"], "dep2");
    assert_eq!(again.extra["suggestion"], "dep2-2");
    assert_eq!(again.extra["errors"][0]["path"], "slug");
    // An address is found whatever case it is written in.
    let found = world
        .app
        .call(OrgBySlugGet, &[&"Beta.DEP2"])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(found.base.id, beta_dep2.base.id);
}

#[tokio::test]
async fn an_org_is_not_moved_beside_an_org_of_its_short_name() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let acme = make_org(&world, &ada, "Acme", "acme").await;
    make_department(&world, &ada, acme.base.id, "dep2", "dep2").await;
    let ops = make_department(&world, &ada, acme.base.id, "Ops", "ops").await;
    let inner = make_department(&world, &ada, ops.base.id, "dep2 of Ops", "dep2").await;
    let refused = world
        .app
        .call(OrgMove, &[&inner.base.id])
        .token(&ada.token)
        .body(&NodeMoveBody {
            parent_id: acme.base.id,
            preflight_id: None,
            expected_revision: inner.base.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::ConflictExists,
        "acme.dep2 would name two orgs"
    );
}
