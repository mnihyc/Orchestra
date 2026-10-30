//! The holder a person chooses is the current person, group, role or membership of a visible place.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::access::{
    AccessChoice, AccessThrough, EffectiveAccessGet, EffectiveAccessGetQuery, Holder, HolderInput,
    HolderSearch, HolderSearchQuery, NodeRoleCreate, PageChange, PageChangeName, PageChangeWhole,
    PermissionPageUpdate, Role, RoleCreateBody, SubjectInput, SubjectKind,
};
use orch_api_contract::access::{
    NodeRoleList, PageChangeOwners, PageChangeRow, PermissionPageGet, PermissionPeopleList,
    PermissionPeopleListQuery,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{
    Group, GroupHolderAdd, GroupHolderList, GroupHolderRemove, OrgGroupCreate, OrgGroupCreateBody,
};
use orch_api_contract::nodes::{Node, NodeGet, NodeUpdate, NodeUpdateBody};
use orch_types::Id;
use places::{Who, join, make_department, make_org, make_project, person, read};
use support::World;

struct Tree {
    owner: Who,
    bob: Who,
    other: Who,
    root: Id,
    dep2: Id,
    dep3: Id,
}

#[tokio::test]
async fn roles_name_all_owners_and_a_viewer_chip_marks_actual_owners_including_nested_groups() {
    let world = World::new().await;
    let t = tree(&world).await;
    // Their roles are in separate departments. Opening the root names both in its explicit
    // Viewers chip without inventing a role at the root for either of them.
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &t.root])
        .token(&t.owner.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![
                    HolderInput::User { id: t.bob.id() },
                    HolderInput::User { id: t.other.id() },
                ],
                choice: AccessChoice::FullView,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let inner = group(&world, &t.owner, t.dep2, "Reviewers").await;
    let outer = group(&world, &t.owner, t.dep3, "Owners").await;
    add(
        &world,
        &t.owner,
        inner.id,
        HolderInput::User { id: t.bob.id() },
    )
    .await;
    add(
        &world,
        &t.owner,
        outer.id,
        HolderInput::Group { id: inner.id },
    )
    .await;
    let project = make_project(&world, &t.owner, t.dep3, "Review")
        .await
        .base
        .id;
    let page = world
        .app
        .call(PermissionPageGet, &[&"nodes", &project])
        .token(&t.owner.token)
        .send()
        .await
        .ok();
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &project])
        .token(&t.owner.token)
        .body(&PageChange {
            expected_revision: Some(page.revision),
            owners: Some(PageChangeOwners {
                add: Some(vec![HolderInput::Group { id: outer.id }]),
                remove: None,
                first: None,
            }),
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::Role {
                    role: Role::Viewer,
                    at: Some(t.root),
                }],
                choice: AccessChoice::FullView,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let roles = world
        .app
        .call(NodeRoleList, &[&project])
        .token(&t.other.token)
        .send()
        .await
        .ok();
    assert_eq!(roles.owners.len(), 2);
    assert!(matches!(&roles.owners[0], Holder::User { user } if user.id == t.owner.id()));
    assert!(
        roles
            .owners
            .iter()
            .any(|holder| matches!(holder, Holder::Group { group } if group.id == outer.id))
    );
    assert!(
        roles.own.iter().all(|held| held.subject.id != t.bob.id()),
        "ownership is not a fabricated role assignment"
    );
    let mut cursor = None;
    let mut people = Vec::new();
    let mut owners = Vec::new();
    loop {
        let page = world
            .app
            .call(PermissionPeopleList, &[&"nodes", &project])
            .token(&t.other.token)
            .query(&PermissionPeopleListQuery {
                holder: format!("role:{}:viewer", t.root),
                limit: Some(1),
                cursor,
            })
            .send()
            .await
            .ok();
        assert!(
            page.owners
                .iter()
                .all(|owner| page.items.iter().any(|user| user.id == *owner)),
            "the mark never reveals somebody outside this page"
        );
        people.extend(page.items.into_iter().map(|user| user.id));
        owners.extend(page.owners);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(
        people.contains(&t.owner.id())
            && people.contains(&t.bob.id())
            && people.contains(&t.other.id()),
        "{people:?}: owner {}, bob {}, other {}",
        t.owner.id(),
        t.bob.id(),
        t.other.id()
    );
    owners.sort_unstable();
    let mut expected = vec![t.owner.id(), t.bob.id()];
    expected.sort_unstable();
    assert_eq!(owners, expected);
    world
        .app
        .call(GroupHolderRemove, &[&outer.id])
        .token(&t.owner.token)
        .body(&HolderInput::Group { id: inner.id })
        .send()
        .await
        .ok();
    let after = world
        .app
        .call(PermissionPeopleList, &[&"nodes", &project])
        .token(&t.other.token)
        .query(&PermissionPeopleListQuery {
            holder: format!("role:{}:viewer", t.root),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert!(after.items.iter().any(|user| user.id == t.bob.id()));
    assert!(
        !after.owners.contains(&t.bob.id()),
        "a group change removes the ownership mark with its authority"
    );
}

async fn tree(world: &World) -> Tree {
    let owner = person(world, "owner").await;
    let bob = person(world, "bob").await;
    let other = person(world, "other").await;
    let root = make_org(world, &owner, "Acme", "acme").await.base.id;
    let dep2 = make_department(world, &owner, root, "dep2", "dep2")
        .await
        .base
        .id;
    let dep3 = make_department(world, &owner, root, "dep3", "dep3")
        .await
        .base
        .id;
    join(world, root, dep2, &bob, "developer", &owner).await;
    join(world, root, dep3, &other, "viewer", &owner).await;
    Tree {
        owner,
        bob,
        other,
        root,
        dep2,
        dep3,
    }
}

async fn search(
    world: &World,
    who: &Who,
    root: Id,
    place: Id,
    q: &str,
) -> Vec<orch_api_contract::access::HolderCandidate> {
    world
        .app
        .call(HolderSearch, &[])
        .token(&who.token)
        .query(&HolderSearchQuery {
            q: q.to_owned(),
            org_id: Some(root),
            place_id: Some(place),
            include_work: Some(false),
            list_places: None,
        })
        .send()
        .await
        .ok()
        .items
}

async fn group(world: &World, owner: &Who, at: Id, name: &str) -> Group {
    world
        .app
        .call(OrgGroupCreate, &[&at])
        .token(&owner.token)
        .body(&OrgGroupCreateBody {
            name: name.to_owned(),
            description: None,
        })
        .send()
        .await
        .ok()
}

async fn add(world: &World, by: &Who, group: Id, holder: HolderInput) {
    let reply = world
        .app
        .call(GroupHolderAdd, &[&group])
        .token(&by.token)
        .body(&holder)
        .send()
        .await;
    assert!(reply.status.is_success(), "{:?}", reply.status);
}

async fn open_to(world: &World, by: &Who, node: Id, holder: HolderInput) {
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &node])
        .token(&by.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            name: Some(vec![PageChangeName {
                holders: vec![holder],
                choice: AccessChoice::FullView,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn every_locator_form_names_the_holder_and_shows_its_whole_place() {
    let world = World::new().await;
    let t = tree(&world).await;
    let reviewers = group(&world, &t.owner, t.dep2, "Reviewers").await;
    for locator in ["acme.dep2 developer", "dep2 developer", "acme.dep2 开发者"] {
        let results = search(&world, &t.owner, t.root, t.dep3, locator).await;
        assert_eq!(results.len(), 1, "{locator}");
        assert_eq!(
            results[0].input,
            HolderInput::Role {
                role: Role::Developer,
                at: Some(t.dep2)
            }
        );
        assert_eq!(results[0].place, "Acme > dep2");
    }
    for locator in ["acme.dep2", "every dep2", "everyone dep2", "all dep2"] {
        let results = search(&world, &t.owner, t.root, t.dep3, locator).await;
        assert_eq!(results.len(), 1, "{locator}");
        assert_eq!(
            results[0].input,
            HolderInput::Role {
                role: Role::Viewer,
                at: Some(t.dep2)
            }
        );
    }
    let members = search(&world, &t.owner, t.root, t.dep3, "members of dep2").await;
    assert_eq!(members[0].input, HolderInput::Members { id: t.dep2 });
    let named_group = search(&world, &t.owner, t.root, t.dep3, "dep2 reviewers").await;
    assert_eq!(named_group.len(), 1);
    assert_eq!(
        named_group[0].input,
        HolderInput::Group { id: reviewers.id }
    );
    assert_eq!(named_group[0].place, "Acme > dep2");
    for locator in ["acme.dep2.bob", "@bob"] {
        let results = search(&world, &t.owner, t.root, t.dep3, locator).await;
        assert_eq!(results.len(), 1, "{locator}");
        assert_eq!(results[0].input, HolderInput::User { id: t.bob.id() });
        assert_eq!(results[0].place, "acme.dep2.bob");
    }
    assert!(
        search(&world, &t.owner, t.root, t.dep3, "dep2.nobody developer")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn partial_paths_list_both_matches_with_the_pages_tree_first_and_private_places_are_not_found()
 {
    let world = World::new().await;
    let t = tree(&world).await;
    let sub = make_department(&world, &t.owner, t.dep2, "sub3", "sub3")
        .await
        .base
        .id;
    let globex = make_org(&world, &t.owner, "Globex", "globex").await.base.id;
    let dep = make_department(&world, &t.owner, globex, "dep2", "dep2")
        .await
        .base
        .id;
    let other_sub = make_department(&world, &t.owner, dep, "sub3", "sub3")
        .await
        .base
        .id;
    let results = search(&world, &t.owner, t.root, t.dep3, "dep2.sub3 developer").await;
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].input,
        HolderInput::Role {
            role: Role::Developer,
            at: Some(sub)
        }
    );
    assert_eq!(
        results[1].input,
        HolderInput::Role {
            role: Role::Developer,
            at: Some(other_sub)
        }
    );
    assert_eq!(results[0].place, "Acme > dep2 > sub3");
    assert_eq!(results[1].place, "Globex > dep2 > sub3");
    assert!(
        search(&world, &t.other, t.root, t.dep3, "acme.dep2 developer")
            .await
            .is_empty()
    );
    assert!(
        search(&world, &t.other, t.root, t.dep3, "globex.dep2 developer")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn giving_dep2_developers_developer_at_dep3_reaches_a_dep2_developer_and_nobody_else() {
    let world = World::new().await;
    let t = tree(&world).await;
    let chosen = search(&world, &t.owner, t.root, t.dep3, "dep2 developer")
        .await
        .remove(0);
    let HolderInput::Role {
        role,
        at: Some(place),
    } = chosen.input
    else {
        panic!("no role");
    };
    let given = world
        .app
        .call(NodeRoleCreate, &[&t.dep3])
        .token(&t.owner.token)
        .body(&RoleCreateBody {
            subject: SubjectInput {
                kind: SubjectKind::Role,
                id: place,
                role: Some(role),
            },
            role: Role::Developer,
            expires_at: None,
            note: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(given.subject.at.as_ref().unwrap().id, t.dep2);
    let Node::Org(bob) = read(&world, &t.bob, t.dep3).await else {
        panic!("not an org");
    };
    let Node::Org(other) = read(&world, &t.other, t.dep3).await else {
        panic!("not an org");
    };
    assert_eq!(bob.base.access.role, Some(Role::Developer));
    assert_eq!(other.base.access.role, Some(Role::Viewer));
}

#[tokio::test]
async fn a_nested_group_reaches_its_member_and_reaches_shows_the_chain_even_after_a_rename() {
    let world = World::new().await;
    let t = tree(&world).await;
    let inner = group(&world, &t.owner, t.dep2, "Developers").await;
    let outer = group(&world, &t.owner, t.dep3, "Reviewers").await;
    add(
        &world,
        &t.owner,
        inner.id,
        HolderInput::Role {
            role: Role::Developer,
            at: Some(t.dep2),
        },
    )
    .await;
    add(
        &world,
        &t.owner,
        outer.id,
        HolderInput::Group { id: inner.id },
    )
    .await;
    let work = make_project(&world, &t.owner, t.dep3, "Review")
        .await
        .base
        .id;
    open_to(&world, &t.owner, work, HolderInput::Group { id: outer.id }).await;
    assert!(matches!(read(&world, &t.bob, work).await, Node::Project(_)));
    let refused = world
        .app
        .call(NodeGet, &[&work])
        .token(&t.other.token)
        .send()
        .await;
    assert!(!refused.status.is_success());
    let effective = world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &work])
        .token(&t.owner.token)
        .query(&EffectiveAccessGetQuery {
            user_id: t.bob.id(),
        })
        .send()
        .await
        .ok();
    let chain = effective
        .permissions
        .iter()
        .filter(|permission| permission.permission == "project.view")
        .flat_map(|permission| &permission.through)
        .find_map(|through| match through {
            AccessThrough::Entry { chain, .. } => Some(chain),
            _ => None,
        })
        .unwrap();
    assert!(matches!(&chain[0], Holder::Group { group } if group.id == outer.id));
    assert!(matches!(&chain[1], Holder::Group { group } if group.id == inner.id));
    assert!(chain.iter().any(|holder| matches!(holder, Holder::Role { role: Role::Developer, at: Some(at), .. } if at.id == t.dep2)));
    assert!(matches!(chain.last().unwrap(), Holder::User { user } if user.id == t.bob.id()));
    let Node::Org(current) = read(&world, &t.owner, t.dep2).await else {
        panic!("not an org");
    };
    world
        .app
        .call(NodeUpdate, &[&t.dep2])
        .token(&t.owner.token)
        .body(&NodeUpdateBody {
            name: Some("Department Two".to_owned()),
            description: None,
            color: None,
            custom_color: None,
            readme_document_id: None,
            expected_revision: current.base.revision,
        })
        .send()
        .await
        .ok();
    assert!(
        matches!(read(&world, &t.bob, work).await, Node::Project(_)),
        "the grant stores a place identifier"
    );
    let results = search(&world, &t.owner, t.root, t.dep3, "acme.dep2 developer").await;
    assert_eq!(results[0].place, "Acme > Department Two");
}

#[tokio::test]
async fn group_cycles_and_a_sixth_level_are_refused_without_changing_any_members() {
    let world = World::new().await;
    let t = tree(&world).await;
    let mut groups = Vec::new();
    for number in 0..6 {
        groups.push(group(&world, &t.owner, t.dep2, &format!("Group {number}")).await);
    }
    for pair in groups[..5].windows(2) {
        add(
            &world,
            &t.owner,
            pair[0].id,
            HolderInput::Group { id: pair[1].id },
        )
        .await;
    }
    for (outer, inner) in [(groups[4].id, groups[0].id), (groups[4].id, groups[5].id)] {
        let fault = world
            .app
            .call(GroupHolderAdd, &[&outer])
            .token(&t.owner.token)
            .body(&HolderInput::Group { id: inner })
            .send()
            .await
            .problem();
        assert_eq!(fault.code, ErrorCode::RequestInvalid);
    }
    let members = world
        .app
        .call(GroupHolderList, &[&groups[4].id])
        .token(&t.owner.token)
        .send()
        .await
        .ok();
    assert!(members.is_empty());
}

#[tokio::test]
async fn members_by_domain_follow_a_move_and_everyone_at_a_place_also_includes_roles_from_above() {
    let world = World::new().await;
    let t = tree(&world).await;
    let members = group(&world, &t.owner, t.dep3, "Department members").await;
    add(
        &world,
        &t.owner,
        members.id,
        HolderInput::Members { id: t.dep2 },
    )
    .await;
    let work = make_project(&world, &t.owner, t.dep3, "Members work")
        .await
        .base
        .id;
    open_to(
        &world,
        &t.owner,
        work,
        HolderInput::Group { id: members.id },
    )
    .await;
    assert!(matches!(read(&world, &t.bob, work).await, Node::Project(_)));
    let before = orch_authz::epoch::current(&mut *world.conn().await)
        .await
        .unwrap();
    let mut tx = world.app.app.db.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    orch_db::queries::access::move_member(
        &mut tx,
        t.root,
        t.bob.id(),
        t.dep2,
        t.dep3,
        "developer",
        t.owner.id(),
        world.now(),
    )
    .await
    .unwrap();
    orch_authz::epoch::bump(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    assert!(
        orch_authz::epoch::current(&mut *world.conn().await)
            .await
            .unwrap()
            > before
    );
    assert!(
        !world
            .app
            .call(NodeGet, &[&work])
            .token(&t.bob.token)
            .send()
            .await
            .status
            .is_success()
    );
    let everyone = group(&world, &t.owner, t.dep3, "Everyone in department two").await;
    add(
        &world,
        &t.owner,
        everyone.id,
        HolderInput::Role {
            role: Role::Viewer,
            at: Some(t.dep2),
        },
    )
    .await;
    let principal = orch_authz::Principal::load(
        &mut *world.conn().await,
        orch_authz::Caller::session(t.owner.id()),
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    assert!(
        principal.groups().contains(&everyone.id),
        "an Admin from above can see dep2 but is no member by domain of dep2"
    );
    assert!(!principal.groups().contains(&members.id));
    world
        .app
        .call(GroupHolderRemove, &[&everyone.id])
        .token(&t.owner.token)
        .body(&HolderInput::Role {
            role: Role::Viewer,
            at: Some(t.dep2),
        })
        .send()
        .await
        .ok();
    let principal = orch_authz::Principal::load(
        &mut *world.conn().await,
        orch_authz::Caller::session(t.owner.id()),
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    assert!(!principal.groups().contains(&everyone.id));
}

#[tokio::test]
async fn everyone_in_a_place_includes_a_page_only_guest_without_opening_the_places_children() {
    let world = World::new().await;
    let t = tree(&world).await;
    let guest = person(&world, "guest").await;
    let child = make_project(&world, &t.owner, t.dep2, "Private child")
        .await
        .base
        .id;
    open_to(
        &world,
        &t.owner,
        t.dep2,
        HolderInput::User { id: guest.id() },
    )
    .await;
    let Node::Org(place) = read(&world, &guest, t.dep2).await else {
        panic!("not an org");
    };
    assert_eq!(
        place.base.access.role, None,
        "one page's View never invents an inherited role"
    );
    assert!(
        !world
            .app
            .call(NodeGet, &[&child])
            .token(&guest.token)
            .send()
            .await
            .status
            .is_success()
    );

    let everyone = group(
        &world,
        &t.owner,
        t.dep3,
        "Everyone who can see department two",
    )
    .await;
    add(
        &world,
        &t.owner,
        everyone.id,
        HolderInput::Role {
            role: Role::Viewer,
            at: Some(t.dep2),
        },
    )
    .await;
    let reached = make_project(&world, &t.owner, t.dep3, "Guests can review")
        .await
        .base
        .id;
    open_to(
        &world,
        &t.owner,
        reached,
        HolderInput::Group { id: everyone.id },
    )
    .await;
    assert!(matches!(
        read(&world, &guest, reached).await,
        Node::Project(_)
    ));
    let direct = make_project(&world, &t.owner, t.dep3, "Direct everyone")
        .await
        .base
        .id;
    open_to(
        &world,
        &t.owner,
        direct,
        HolderInput::Role {
            role: Role::Viewer,
            at: Some(t.dep2),
        },
    )
    .await;
    assert!(matches!(
        read(&world, &guest, direct).await,
        Node::Project(_)
    ));

    let page = world
        .app
        .call(PermissionPageGet, &[&"nodes", &t.dep2])
        .token(&t.owner.token)
        .send()
        .await
        .ok();
    let entry = page
        .rows
        .iter()
        .find(|row| row.permission == "org.view")
        .unwrap()
        .entries
        .iter()
        .find(|entry| matches!(&entry.holder, Holder::User { user } if user.id == guest.id()))
        .unwrap()
        .id;
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &t.dep2])
        .token(&t.owner.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "org.view".to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: None,
                update: None,
                remove: Some(vec![entry]),
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    for node in [t.dep2, reached, direct] {
        assert!(
            !world
                .app
                .call(NodeGet, &[&node])
                .token(&guest.token)
                .send()
                .await
                .status
                .is_success(),
            "view removal ends this dynamic holder"
        );
    }
}
