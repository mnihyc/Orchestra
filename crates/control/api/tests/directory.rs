//! Groups, the members of an org and their suspension, over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate, Role,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{
    GroupDelete, GroupMemberAdd, GroupMemberList, GroupMemberRemove, GroupUpdate, GroupUpdateBody,
    OrgGroupCreate, OrgGroupCreateBody, OrgGroupList, OrgGroupListQuery, OrgMemberList,
    OrgMemberListQuery, OrgMemberResume, OrgMemberSuspend, OrgMemberSuspendBody, OrgSuspensionList,
};
use orch_api_contract::nodes::{Node, NodeGet};
use orch_types::Id;
use places::{Who, give_role, join, make_department, make_org, make_project, person, read};
use support::World;

struct Setup {
    boss: Who,
    dan: Who,
    acme: Id,
    web: Id,
}

async fn setup(world: &World) -> Setup {
    let boss = person(world, "boss").await;
    let dan = person(world, "dan").await;
    let acme = make_org(world, &boss, "Acme", "acme").await;
    join(world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let web = make_project(world, &boss, acme.base.id, "Web").await;
    Setup {
        boss,
        dan,
        acme: acme.base.id,
        web: web.base.id,
    }
}

async fn make_group(
    world: &World,
    who: &Who,
    org: Id,
    name: &str,
) -> orch_api_contract::directory::Group {
    world
        .app
        .call(OrgGroupCreate, &[&org])
        .token(&who.token)
        .body(&OrgGroupCreateBody {
            name: name.to_owned(),
            description: Some("the people".to_owned()),
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_group_is_made_listed_described_anew_and_deleted_by_whoever_manages_the_members() {
    let world = World::new().await;
    let s = setup(&world).await;
    let crew = make_group(&world, &s.boss, s.acme, "Crew").await;
    assert_eq!(
        (crew.name.as_str(), crew.member_count, crew.org_id),
        ("Crew", 0, s.acme)
    );
    assert_eq!(crew.description, "the people");
    let again = world
        .app
        .call(OrgGroupCreate, &[&s.acme])
        .token(&s.boss.token)
        .body(&OrgGroupCreateBody {
            name: "crew".to_owned(),
            description: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        again.code,
        ErrorCode::ConflictExists,
        "a name is unique in its org, whatever the case"
    );
    let refused = world
        .app
        .call(OrgGroupCreate, &[&s.acme])
        .token(&s.dan.token)
        .body(&OrgGroupCreateBody {
            name: "Mine".to_owned(),
            description: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "members.manage");
    let listed = world
        .app
        .call(OrgGroupList, &[&s.acme])
        .token(&s.dan.token)
        .query(&OrgGroupListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(listed.len(), 1);
    let renamed = world
        .app
        .call(GroupUpdate, &[&crew.id])
        .token(&s.boss.token)
        .body(&GroupUpdateBody {
            name: Some("Crew A".to_owned()),
            description: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(renamed.name, "Crew A");
    assert_eq!(renamed.description, "the people");
    let refused = world
        .app
        .call(GroupUpdate, &[&crew.id])
        .token(&s.dan.token)
        .body(&GroupUpdateBody::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "members.manage");
    // The groups of the orgs inside one are listed with `under`.
    let research = make_department(&world, &s.boss, s.acme, "Research", "research").await;
    make_group(&world, &s.boss, research.base.id, "Scientists").await;
    let direct = world
        .app
        .call(OrgGroupList, &[&s.acme])
        .token(&s.boss.token)
        .query(&OrgGroupListQuery { under: None })
        .send()
        .await
        .ok();
    let under = world
        .app
        .call(OrgGroupList, &[&s.acme])
        .token(&s.boss.token)
        .query(&OrgGroupListQuery { under: Some(true) })
        .send()
        .await
        .ok();
    assert_eq!((direct.len(), under.len()), (1, 2));
    let gone = world
        .app
        .call(GroupDelete, &[&crew.id])
        .token(&s.boss.token)
        .send()
        .await;
    assert_eq!(gone.status, 204);
    let audit = world.audit().await;
    for action in ["group.created", "group.updated", "group.deleted"] {
        assert!(audit.iter().any(|each| each == action), "{action}");
    }
}

#[tokio::test]
async fn a_member_of_the_root_org_is_put_into_a_group_and_taken_out_and_access_follows_at_once() {
    let world = World::new().await;
    let s = setup(&world).await;
    let eve = person(&world, "eve").await;
    let zed = person(&world, "zed").await;
    join(&world, s.acme, s.acme, &eve, "viewer", &s.boss).await;
    let crew = make_group(&world, &s.boss, s.acme, "Crew").await;
    // Only members of the root org are put in a group.
    let refused = world
        .app
        .call(GroupMemberAdd, &[&crew.id, &zed.id()])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    for who in [&eve, &s.dan] {
        let added = world
            .app
            .call(GroupMemberAdd, &[&crew.id, &who.id()])
            .token(&s.boss.token)
            .send()
            .await;
        assert_eq!(added.status, 204);
    }
    let again = world
        .app
        .call(GroupMemberAdd, &[&crew.id, &eve.id()])
        .token(&s.boss.token)
        .send()
        .await;
    assert_eq!(again.status, 204, "putting somebody in twice is no harm");
    let people = world
        .app
        .call(GroupMemberList, &[&crew.id])
        .token(&s.dan.token)
        .send()
        .await
        .ok();
    assert_eq!(people.len(), 2);
    let listed = world
        .app
        .call(OrgGroupList, &[&s.acme])
        .token(&s.dan.token)
        .query(&OrgGroupListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(listed[0].member_count, 2);
    // The group is named on the page of Web, and its members read it.
    let private = world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &s.web])
        .token(&s.boss.token)
        .body(&PageChange {
            whole: Some(orch_api_contract::access::PageChangeWhole::Private),
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::Group { id: crew.id }],
                choice: AccessChoice::FullView,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await;
    assert!(private.status.is_success(), "{:?}", private.status);
    assert!(matches!(read(&world, &eve, s.web).await, Node::Project(_)));
    // Out of the group, out of the project, at the next request.
    let removed = world
        .app
        .call(GroupMemberRemove, &[&crew.id, &eve.id()])
        .token(&s.boss.token)
        .send()
        .await;
    assert_eq!(removed.status, 204);
    let refused = world
        .app
        .call(NodeGet, &[&s.web])
        .token(&eve.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let refused = world
        .app
        .call(GroupMemberRemove, &[&crew.id, &s.dan.id()])
        .token(&s.dan.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "members.manage");
}

#[tokio::test]
async fn deleting_a_group_takes_the_roles_it_holds_and_the_entries_that_name_it() {
    let world = World::new().await;
    let s = setup(&world).await;
    let crew = make_group(&world, &s.boss, s.acme, "Crew").await;
    world
        .app
        .call(GroupMemberAdd, &[&crew.id, &s.dan.id()])
        .token(&s.boss.token)
        .send()
        .await;
    {
        let mut conn = world.conn().await;
        orch_db::queries::access::testing::give_role(
            &mut conn,
            s.web,
            "group",
            crew.id,
            "developer",
            s.boss.id(),
            None,
        )
        .await
        .unwrap();
    }
    let before = world
        .app
        .call(orch_api_contract::access::NodeRoleList, &[&s.web])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(before.own.len(), 1);
    world
        .app
        .call(GroupDelete, &[&crew.id])
        .token(&s.boss.token)
        .send()
        .await;
    let after = world
        .app
        .call(orch_api_contract::access::NodeRoleList, &[&s.web])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert!(after.own.is_empty());
    let missing = world
        .app
        .call(GroupMemberList, &[&crew.id])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn the_members_of_an_org_are_listed_with_their_domain_address_and_role() {
    let world = World::new().await;
    let s = setup(&world).await;
    let research = make_department(&world, &s.boss, s.acme, "Research", "research").await;
    let eve = person(&world, "eve").await;
    join(&world, s.acme, research.base.id, &eve, "developer", &s.boss).await;
    give_role(&world, s.web, &eve, "admin", &s.boss).await;
    let list = |org: Id, direct: Option<bool>, q: Option<&str>| {
        world
            .app
            .call(OrgMemberList, &[&org])
            .token(&s.dan.token)
            .query(&OrgMemberListQuery {
                direct,
                q: q.map(str::to_owned),
                limit: None,
                cursor: None,
            })
            .send()
    };
    let all = list(s.acme, None, None).await.ok();
    let names: Vec<&str> = all
        .items
        .iter()
        .map(|member| member.user.username.as_str())
        .collect();
    assert_eq!(names, vec!["boss", "dan", "eve"]);
    let boss = &all.items[0];
    assert!(boss.is_owner);
    assert_eq!(boss.address, "acme.boss");
    assert_eq!(boss.domain.id, s.acme);
    let eve_member = &all.items[2];
    assert_eq!(eve_member.address, "acme.research.eve");
    assert_eq!(eve_member.domain.id, research.base.id);
    assert_eq!(eve_member.role, Role::Developer);
    assert_eq!(
        eve_member.also, 1,
        "an Admin role on a project, beside the one at her domain"
    );
    assert!(!eve_member.is_owner);
    assert!(
        all.items.iter().all(|member| !member.suspended),
        "a viewer is not told"
    );
    // The members of a department are its own and those of the ones inside.
    let of_research = list(research.base.id, None, None).await.ok();
    assert_eq!(of_research.items.len(), 1);
    // Direct: those whose domain is the org itself.
    let direct = list(s.acme, Some(true), None).await.ok();
    assert_eq!(direct.items.len(), 2);
    let found = list(s.acme, None, Some("EV")).await.ok();
    assert_eq!(found.items.len(), 1);
    // Paged.
    let first = world
        .app
        .call(OrgMemberList, &[&s.acme])
        .token(&s.dan.token)
        .query(&OrgMemberListQuery {
            direct: None,
            q: None,
            limit: Some(2),
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(first.items.len(), 2);
    let rest = world
        .app
        .call(OrgMemberList, &[&s.acme])
        .token(&s.dan.token)
        .query(&OrgMemberListQuery {
            direct: None,
            q: None,
            limit: Some(2),
            cursor: first.next_cursor,
        })
        .send()
        .await
        .ok();
    assert_eq!(rest.items.len(), 1);
    assert_eq!(rest.items[0].user.username, "eve");
    let stranger = person(&world, "stranger").await;
    let refused = world
        .app
        .call(OrgMemberList, &[&s.acme])
        .token(&stranger.token)
        .query(&OrgMemberListQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn a_suspended_member_is_shut_out_of_the_org_until_it_is_lifted() {
    let world = World::new().await;
    let s = setup(&world).await;
    let suspend = |who: &Who, user: Id, reason: Option<&str>| {
        world
            .app
            .call(OrgMemberSuspend, &[&s.acme, &user])
            .token(&who.token)
            .body(&OrgMemberSuspendBody {
                reason: reason.map(str::to_owned),
            })
            .send()
    };
    // A viewer suspends nobody.
    let refused = suspend(&s.dan, s.boss.id(), None).await.problem();
    assert_eq!(refused.extra["required"], "members.manage");
    let held = suspend(&s.boss, s.dan.id(), Some("left the building"))
        .await
        .ok();
    assert_eq!(held.user.username, "dan");
    assert_eq!(held.reason, "left the building");
    assert_eq!(held.by.username, "boss");
    assert!(!held.outside);
    // He is refused everything of the org, and told which org it is.
    let refused = world
        .app
        .call(NodeGet, &[&s.web])
        .token(&s.dan.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::OrgSuspended);
    assert_eq!(refused.extra["org"]["id"], serde_json::json!(s.acme));
    // Listed, and visible to the managers in the list of members.
    let listed = world
        .app
        .call(OrgSuspensionList, &[&s.acme])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(listed.len(), 1);
    let members = world
        .app
        .call(OrgMemberList, &[&s.acme])
        .token(&s.boss.token)
        .query(&OrgMemberListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        members
            .items
            .iter()
            .find(|member| member.user.username == "dan")
            .unwrap()
            .suspended
    );
    let again = suspend(&s.boss, s.dan.id(), None).await.problem();
    assert_eq!(again.code, ErrorCode::ConflictExists);
    // Lifted: everything is as it was.
    let resumed = world
        .app
        .call(OrgMemberResume, &[&s.acme, &s.dan.id()])
        .token(&s.boss.token)
        .send()
        .await;
    assert_eq!(resumed.status, 204);
    assert!(matches!(
        read(&world, &s.dan, s.web).await,
        Node::Project(_)
    ));
    let nothing = world
        .app
        .call(OrgMemberResume, &[&s.acme, &s.dan.id()])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(nothing.code, ErrorCode::NotFound);
    let audit = world.audit().await;
    assert!(audit.iter().any(|action| action == "member.suspended"));
    assert!(audit.iter().any(|action| action == "member.resumed"));
}

#[tokio::test]
async fn nobody_suspends_themself_an_owner_or_a_system_admin_and_somebody_let_in_is_marked_outside()
{
    let world = World::new().await;
    let s = setup(&world).await;
    let root = places::admin(&world, "root").await;
    let admin = person(&world, "adm").await;
    join(&world, s.acme, s.acme, &admin, "admin", &s.boss).await;
    let suspend = |who: &Who, user: Id| {
        world
            .app
            .call(OrgMemberSuspend, &[&s.acme, &user])
            .token(&who.token)
            .body(&OrgMemberSuspendBody::default())
            .send()
    };
    for (who, target, why) in [
        (&admin, admin.id(), "oneself"),
        (&admin, s.boss.id(), "an owner of that org"),
        (&admin, root.id(), "a system admin"),
    ] {
        let refused = suspend(who, target).await.problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied, "{why}");
        assert_eq!(refused.extra["override"], false, "{why}");
    }
    // Somebody of another tree who was let into a project here.
    let outsider = person(&world, "visitor").await;
    give_role(&world, s.web, &outsider, "viewer", &s.boss).await;
    let held = suspend(&admin, outsider.id()).await.ok();
    assert!(held.outside);
    // Somebody who has nothing here is not found.
    let nobody = person(&world, "nobody").await;
    assert_eq!(
        suspend(&admin, nobody.id()).await.problem().code,
        ErrorCode::NotFound
    );
}
