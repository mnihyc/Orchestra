//! Permission pages, defaults pages, roles, what one person holds and the people behind a chip, over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::access::{
    AccessChoice, EffectiveAccessGet, EffectiveAccessGetQuery, HistoryList, HistoryListQuery,
    HolderInput, NodeDefaultsGet, NodeDefaultsUpdate, NodeRoleCreate, NodeRoleList, PageChange,
    PageChangeAdd, PageChangeName, PageChangeOwners, PageChangeRow, PageChangeWhole, PageKind,
    PermissionCatalogueGet, PermissionPage, PermissionPageGet, PermissionPageUpdate,
    PermissionPeopleList, PermissionPeopleListQuery, Role, RoleCreateBody, RoleDelete, RoleUpdate,
    RoleUpdateBody, SubjectInput, SubjectKind,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{Node, PageState, TopicCreate, TopicCreateBody};
use orch_db::queries::access::testing as access_rows;
use orch_settings::Target;
use orch_types::Id;
use places::{Who, first_missing, join, make_org, make_project, missing_permissions, person, read};
use serde_json::json;
use support::World;

/// Acme with its owner `boss`, a viewer `dan` and a project Web.
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

async fn page(world: &World, who: &Who, node: Id) -> PermissionPage {
    world
        .app
        .call(PermissionPageGet, &[&"nodes", &node])
        .token(&who.token)
        .send()
        .await
        .ok()
}

async fn change(
    world: &World,
    who: &Who,
    node: Id,
    change: &PageChange,
) -> orch_api_core::testing::Reply<PermissionPage> {
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &node])
        .token(&who.token)
        .body(change)
        .send()
        .await
}

fn naming(user: Id, choice: AccessChoice) -> PageChange {
    PageChange {
        name: Some(vec![PageChangeName {
            holders: vec![HolderInput::User { id: user }],
            choice,
            export: None,
            options: None,
            expires_at: None,
            note: None,
        }]),
        ..PageChange::default()
    }
}

#[tokio::test]
async fn the_catalogue_is_the_data_every_page_is_drawn_from() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let catalogue = world
        .app
        .call(PermissionCatalogueGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    let names: Vec<&str> = catalogue
        .permissions
        .iter()
        .map(|def| def.name.as_str())
        .collect();
    for expected in [
        "thread.interact",
        "project.manage",
        "access.manage",
        "project.archive",
    ] {
        assert!(names.contains(&expected), "{expected} is in the catalogue");
    }
    assert_eq!(catalogue.choices.len(), 3);
    assert_eq!(
        catalogue
            .roles
            .iter()
            .map(|role| role.key)
            .collect::<Vec<_>>(),
        vec![Role::Viewer, Role::Interactor, Role::Developer, Role::Admin]
    );
    let manage = catalogue
        .choices
        .iter()
        .find(|choice| choice.key == AccessChoice::FullManage)
        .unwrap();
    assert!(manage.permissions["project"].contains(&"project.manage".to_owned()));
    // Nobody who is not signed in reads it.
    let refused = world
        .app
        .call(PermissionCatalogueGet, &[])
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn a_page_shows_its_owners_its_rows_and_what_each_follows() {
    let world = World::new().await;
    let s = setup(&world).await;
    let shown = page(&world, &s.dan, s.web).await;
    assert_eq!(shown.object.kind, "project");
    assert_eq!(shown.object.id, s.web);
    assert_eq!(shown.object.name, "Web");
    assert_eq!(shown.page, PageKind::Live);
    assert_eq!(shown.state, PageState::Follows);
    assert_eq!(shown.owner.as_ref().unwrap().username, "boss");
    assert_eq!(shown.owners.len(), 1);
    assert!(!shown.owners_writable, "a viewer changes nothing here");
    assert!(!shown.rows.iter().any(|row| row.writable));
    let view = shown
        .rows
        .iter()
        .find(|row| row.permission == "project.view")
        .expect("the page has its View row");
    assert!(view.inherit);
    let inherited = view.inherited.as_ref().expect("it follows the defaults");
    assert!(
        inherited.holders.iter().any(|holder| matches!(
            holder,
            orch_api_contract::access::Holder::Role {
                role: Role::Viewer,
                ..
            }
        )),
        "the Viewers of the place hold it as shipped"
    );
    assert!(
        shown.admins.count >= 1,
        "boss, the owner of the org above, is an admin"
    );
    assert!(shown.depends.is_empty());
    // The owner may change it.
    let mine = page(&world, &s.boss, s.web).await;
    assert!(mine.owners_writable);
    assert!(mine.rows.iter().all(|row| row.writable));
    // A stranger is refused the page, with what is missing.
    let eve = person(&world, "eve").await;
    let refused = world
        .app
        .call(PermissionPageGet, &[&"nodes", &s.web])
        .token(&eve.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "project.view");
}

#[tokio::test]
async fn the_page_of_a_resource_is_the_resources_to_read_and_one_that_is_not_there_is_not_found() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let refused = world
        .app
        .call(PermissionPageGet, &[&"providers", &orch_types::new_id()])
        .token(&ada.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn naming_somebody_at_a_choice_gives_them_that_and_changes_the_page_as_a_whole() {
    let world = World::new().await;
    let s = setup(&world).await;
    // Dan cannot make a topic in Web, nor change its page.
    let topic = |who: &Who| {
        world
            .app
            .call(TopicCreate, &[&s.web])
            .token(&who.token)
            .body(&TopicCreateBody::default())
            .send()
    };
    assert_eq!(topic(&s.dan).await.problem().code, ErrorCode::AuthzDenied);
    let before = page(&world, &s.boss, s.web).await;
    let refused = change(
        &world,
        &s.dan,
        s.web,
        &naming(s.dan.id(), AccessChoice::FullManage),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    // He reads the project and holds no more, so that naming himself at Manage gives more than he
    // has: he is told everything he lacks, and who can give it.
    assert!(
        missing_permissions(&refused).contains(&"project.manage".to_owned()),
        "{refused:?}"
    );
    assert_eq!(first_missing(&refused)["ask"][0]["username"], "boss");

    // Boss names him at "Can interact".
    let after = change(
        &world,
        &s.boss,
        s.web,
        &naming(s.dan.id(), AccessChoice::FullInteract),
    )
    .await
    .ok();
    assert_eq!(after.state, PageState::Custom);
    assert_eq!(after.revision, before.revision + 1);
    let interact = after
        .rows
        .iter()
        .find(|row| row.permission == "project.interact")
        .unwrap();
    assert_eq!(interact.entries.len(), 1);
    assert!(matches!(
        &interact.entries[0].holder,
        orch_api_contract::access::Holder::User { user } if user.username == "dan"
    ));
    assert_eq!(
        interact.entries[0].created_by.as_ref().unwrap().username,
        "boss"
    );
    assert!(!interact.entries[0].outside);
    // Now he can make a topic.
    assert_eq!(topic(&s.dan).await.ok_status(), 200);
    // The audit trail and the history of the project show the change.
    let audit = world.audit().await;
    assert!(audit.iter().any(|action| action == "permission.added"));
    let history = world
        .app
        .call(HistoryList, &[&"nodes", &s.web])
        .token(&s.dan.token)
        .query(&HistoryListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        history
            .items
            .iter()
            .any(|line| line.action == "permission.added"),
        "{:?}",
        history
            .items
            .iter()
            .map(|line| &line.action)
            .collect::<Vec<_>>()
    );
}

trait OkStatus {
    fn ok_status(self) -> u16;
}

impl<T: std::fmt::Debug> OkStatus for orch_api_core::testing::Reply<T> {
    fn ok_status(self) -> u16 {
        let status = self.status.as_u16();
        let _ = self.ok();
        status
    }
}

#[tokio::test]
async fn a_page_changed_meanwhile_is_refused_with_the_page_as_it_is() {
    let world = World::new().await;
    let s = setup(&world).await;
    let seen = page(&world, &s.boss, s.web).await;
    change(
        &world,
        &s.boss,
        s.web,
        &naming(s.dan.id(), AccessChoice::FullView),
    )
    .await
    .ok();
    let mut stale = naming(s.dan.id(), AccessChoice::FullInteract);
    stale.expected_revision = Some(seen.revision);
    let refused = change(&world, &s.boss, s.web, &stale).await.problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(refused.extra["current"]["object"]["id"], json!(s.web));
    assert_eq!(
        refused.extra["current"]["revision"],
        json!(seen.revision + 1)
    );
}

#[tokio::test]
async fn a_change_that_does_not_fit_the_page_is_refused_naming_the_member() {
    let world = World::new().await;
    let s = setup(&world).await;
    let nonsense = PageChange {
        rows: Some(vec![PageChangeRow {
            permission: "provider.use".to_owned(),
            profile: None,
            inherit: None,
            locked: None,
            add: Some(vec![PageChangeAdd {
                holder: HolderInput::User { id: s.dan.id() },
                options: None,
                expires_at: None,
                note: None,
            }]),
            update: None,
            remove: None,
        }]),
        ..PageChange::default()
    };
    let refused = change(&world, &s.boss, s.web, &nonsense).await.problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(!refused.extra["errors"].as_array().unwrap().is_empty());
    // Nothing was changed.
    assert_eq!(page(&world, &s.boss, s.web).await.state, PageState::Follows);
}

#[tokio::test]
async fn an_admin_who_names_themself_beyond_what_administering_gives_is_marked_as_forcing_it() {
    let world = World::new().await;
    let s = setup(&world).await;
    let root = places::admin(&world, "root").await;
    let private = change(
        &world,
        &s.boss,
        s.web,
        &PageChange {
            whole: Some(PageChangeWhole::Private),
            ..PageChange::default()
        },
    )
    .await
    .ok();
    assert_eq!(private.state, PageState::Restricted);
    // The owner names somebody: an ordinary entry.
    change(
        &world,
        &s.boss,
        s.web,
        &naming(s.dan.id(), AccessChoice::FullView),
    )
    .await
    .ok();
    // Root, a system admin with As admin on, is on no row of the page. Administering the project
    // gives him its View and Manage, so naming himself at Full view forces nothing.
    let viewing = change(
        &world,
        &root,
        s.web,
        &naming(root.id(), AccessChoice::FullView),
    )
    .await
    .ok();
    assert_eq!(viewing.state, PageState::Restricted);
    // Naming somebody from outside is no part of administering: naming himself in that row is
    // forcing what the page does not give him.
    change(
        &world,
        &root,
        s.web,
        &PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "access.external".to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![PageChangeAdd {
                    holder: HolderInput::User { id: root.id() },
                    options: None,
                    expires_at: None,
                    note: None,
                }]),
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        },
    )
    .await
    .ok();
    let marked: Vec<(Option<String>, bool)> = sqlx::query_as(
        "SELECT details->>'permission', override FROM audit_log WHERE action = 'permission.added' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    let forced: Vec<&str> = marked
        .iter()
        .filter(|(_, forced)| *forced)
        .filter_map(|(permission, _)| permission.as_deref())
        .collect();
    assert_eq!(
        forced,
        ["access.external"],
        "only the row that administering does not give is marked: {marked:?}"
    );
    // And he reads the project.
    assert!(matches!(read(&world, &root, s.web).await, Node::Project(_)));
}

#[tokio::test]
async fn somebody_from_outside_is_named_only_by_those_who_may() {
    let world = World::new().await;
    let s = setup(&world).await;
    let eve = person(&world, "eve").await;
    // As shipped the owners of a thing may.
    let named = change(
        &world,
        &s.boss,
        s.web,
        &naming(eve.id(), AccessChoice::FullView),
    )
    .await
    .ok();
    let row = named
        .rows
        .iter()
        .find(|row| row.permission == "project.view")
        .unwrap();
    assert!(
        row.entries.iter().any(|entry| entry.outside),
        "the entry says she is outside"
    );
    let zed = person(&world, "zed").await;
    // Where the row that lets people name an outsider names nobody and the
    // policy is off, an owner may not.
    change(
        &world,
        &s.boss,
        s.web,
        &PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "access.external".to_owned(),
                profile: None,
                inherit: Some(false),
                locked: None,
                add: None,
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        },
    )
    .await
    .ok();
    world
        .set_at(Target::Node(s.acme), "sharing.allow_external", json!(false))
        .await;
    let refused = change(
        &world,
        &s.boss,
        s.web,
        &naming(zed.id(), AccessChoice::FullView),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::SharingExternalForbidden);
    assert!(refused.extra["ask"].is_array());
    assert!(refused.extra.contains_key("ask_more"));
}

#[tokio::test]
async fn the_owners_row_keeps_a_person_and_names_the_first_owner() {
    let world = World::new().await;
    let s = setup(&world).await;
    let owners = |add: Vec<Id>, remove: Vec<Id>, first: Option<Id>| PageChange {
        owners: Some(PageChangeOwners {
            add: Some(add.into_iter().map(|id| HolderInput::User { id }).collect()),
            remove: Some(remove),
            first,
        }),
        ..PageChange::default()
    };
    let two = change(
        &world,
        &s.boss,
        s.web,
        &owners(vec![s.dan.id()], vec![], None),
    )
    .await
    .ok();
    assert_eq!(two.owners.len(), 2);
    // Dan, now an owner, is made the first.
    let moved = change(
        &world,
        &s.boss,
        s.web,
        &owners(vec![], vec![], Some(s.dan.id())),
    )
    .await
    .ok();
    assert_eq!(moved.owner.as_ref().unwrap().username, "dan");
    assert_eq!(
        moved.owners.len(),
        2,
        "the person who was first stays an owner"
    );
    let Node::Project(project) = read(&world, &s.dan, s.web).await else {
        panic!("a project stays one");
    };
    assert_eq!(project.base.owner.username, "dan");
    assert_eq!(project.base.owners, 2);
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "node.transferred")
    );
    // Taking both out leaves nobody: the last stays.
    let ids: Vec<Id> = moved.owners.iter().map(|entry| entry.id).collect();
    let refused = change(&world, &s.dan, s.web, &owners(vec![], ids, None))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::OwnerLast);
}

#[tokio::test]
async fn a_lock_on_the_defaults_of_an_org_decides_the_row_below_it() {
    let world = World::new().await;
    let s = setup(&world).await;
    let defaults = world
        .app
        .call(NodeDefaultsGet, &[&s.acme])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(defaults.page, PageKind::Defaults);
    assert!(defaults.followers.is_some());
    assert!(
        defaults
            .rows
            .iter()
            .any(|row| row.permission == "project.view")
    );
    // Lock the View row of projects at the org.
    let locked = world
        .app
        .call(NodeDefaultsUpdate, &[&s.acme])
        .token(&s.boss.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "project.view".to_owned(),
                profile: None,
                inherit: None,
                locked: Some(true),
                add: None,
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    assert!(
        locked
            .rows
            .iter()
            .find(|row| row.permission == "project.view")
            .unwrap()
            .locked
            .is_some()
    );
    // The project cannot cut its own View row loose.
    let refused = change(
        &world,
        &s.boss,
        s.web,
        &PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "project.view".to_owned(),
                profile: None,
                inherit: Some(false),
                locked: None,
                add: None,
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        },
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AccessLocked);
    assert_eq!(refused.extra["permission"], "project.view");
    assert_eq!(refused.extra["by"]["id"], json!(s.acme));
    // A viewer reads the defaults, and may not change them.
    let refused = world
        .app
        .call(NodeDefaultsUpdate, &[&s.acme])
        .token(&s.dan.token)
        .body(&PageChange::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "access.manage");
}

#[tokio::test]
async fn a_role_is_given_listed_changed_and_taken_away() {
    let world = World::new().await;
    let s = setup(&world).await;
    let eve = person(&world, "eve").await;
    join(&world, s.acme, s.acme, &eve, "viewer", &s.boss).await;
    let give = |who: &Who, node: Id, user: Id, role: Role| {
        world
            .app
            .call(NodeRoleCreate, &[&node])
            .token(&who.token)
            .body(&RoleCreateBody {
                subject: SubjectInput {
                    kind: SubjectKind::User,
                    role: None,
                    id: user,
                },
                role,
                expires_at: None,
                note: Some("for the release".to_owned()),
            })
            .send()
    };
    let held = give(&s.boss, s.web, eve.id(), Role::Developer).await.ok();
    assert_eq!(held.role, Role::Developer);
    assert_eq!(held.node.id, s.web);
    assert_eq!(held.subject.user.as_ref().unwrap().username, "eve");
    assert_eq!(held.note, "for the release");
    assert!(!held.domain && !held.outside);
    assert_eq!(held.created_by.username, "boss");
    // Again: she holds one there.
    assert_eq!(
        give(&s.boss, s.web, eve.id(), Role::Admin)
            .await
            .problem()
            .code,
        ErrorCode::ConflictExists
    );
    // Listed in the place, and for the topics below it from above.
    let listed = world
        .app
        .call(NodeRoleList, &[&s.web])
        .token(&s.dan.token)
        .send()
        .await
        .ok();
    assert_eq!(listed.own.len(), 1);
    assert!(listed.above.iter().any(|role| {
        role.subject
            .user
            .as_ref()
            .is_some_and(|user| user.username == "dan")
    }));
    assert!(
        listed.above.iter().any(|role| role.domain),
        "Dan's role at his domain is marked"
    );
    // Changed: needs the revision seen.
    let stale = world
        .app
        .call(RoleUpdate, &[&held.id])
        .token(&s.boss.token)
        .body(&RoleUpdateBody {
            role: Some(Role::Interactor),
            expires_at: None,
            note: None,
            expected_revision: 99,
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["role"], "developer");
    let changed = world
        .app
        .call(RoleUpdate, &[&held.id])
        .token(&s.boss.token)
        .body(&RoleUpdateBody {
            role: Some(Role::Interactor),
            expires_at: Some(None),
            note: Some("less".to_owned()),
            expected_revision: held.revision,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        (changed.role, changed.note.as_str()),
        (Role::Interactor, "less")
    );
    assert_eq!(changed.revision, held.revision + 1);
    // A viewer changes none.
    let refused = world
        .app
        .call(RoleDelete, &[&held.id])
        .token(&s.dan.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "access.manage");
    // Taken away.
    let gone = world
        .app
        .call(RoleDelete, &[&held.id])
        .token(&s.boss.token)
        .send()
        .await;
    assert_eq!(gone.status, 204);
    let listed = world
        .app
        .call(NodeRoleList, &[&s.web])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert!(listed.own.is_empty());
    let audit = world.audit().await;
    for expected in ["role.given", "role.changed", "role.removed"] {
        assert!(audit.iter().any(|action| action == expected), "{expected}");
    }
}

#[tokio::test]
async fn admin_is_given_and_taken_only_by_an_owner_or_an_admin_of_the_place() {
    let world = World::new().await;
    let s = setup(&world).await;
    let eve = person(&world, "eve").await;
    join(&world, s.acme, s.acme, &eve, "viewer", &s.boss).await;
    // Dan is named in the row that decides who changes the page of Web, and
    // is no owner and no admin of it.
    change(
        &world,
        &s.boss,
        s.web,
        &PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "access.manage".to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![PageChangeAdd {
                    holder: HolderInput::User { id: s.dan.id() },
                    options: None,
                    expires_at: None,
                    note: None,
                }]),
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        },
    )
    .await
    .ok();
    let refused = world
        .app
        .call(NodeRoleCreate, &[&s.web])
        .token(&s.dan.token)
        .body(&RoleCreateBody {
            subject: SubjectInput {
                kind: SubjectKind::User,
                role: None,
                id: eve.id(),
            },
            role: Role::Admin,
            expires_at: None,
            note: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "access.own");
    // Nor a role above their own: Dan is a Viewer, who gives a Viewer's.
    let above = world
        .app
        .call(NodeRoleCreate, &[&s.web])
        .token(&s.dan.token)
        .body(&RoleCreateBody {
            subject: SubjectInput {
                kind: SubjectKind::User,
                role: None,
                id: eve.id(),
            },
            role: Role::Developer,
            expires_at: None,
            note: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(above.code, ErrorCode::AuthzDenied);
    // The same person gives the role they hold.
    let given = world
        .app
        .call(NodeRoleCreate, &[&s.web])
        .token(&s.dan.token)
        .body(&RoleCreateBody {
            subject: SubjectInput {
                kind: SubjectKind::User,
                role: None,
                id: eve.id(),
            },
            role: Role::Viewer,
            expires_at: None,
            note: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(given.role, Role::Viewer);
    assert_eq!(given.created_by.username, "dan", "a role says who gave it");
}

#[tokio::test]
async fn a_role_is_never_held_on_a_thread_and_the_role_at_a_domain_is_not_taken_away_by_itself() {
    let world = World::new().await;
    let s = setup(&world).await;
    let topic = places::make_topic(&world, &s.boss, s.web, "Login").await;
    let thread = orch_types::new_id();
    {
        let mut conn = world.conn().await;
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            thread,
            "thread",
            Some(topic.base.id),
            "Fix",
            s.boss.id(),
        )
        .await
        .unwrap();
    }
    let on_thread = world
        .app
        .call(NodeRoleCreate, &[&thread])
        .token(&s.boss.token)
        .body(&RoleCreateBody {
            subject: SubjectInput {
                kind: SubjectKind::User,
                role: None,
                id: s.dan.id(),
            },
            role: Role::Viewer,
            expires_at: None,
            note: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(on_thread.code, ErrorCode::RequestInvalid);
    // Dan's role at his domain.
    let listed = world
        .app
        .call(NodeRoleList, &[&s.acme])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    let domain = listed
        .own
        .iter()
        .find(|role| role.domain)
        .expect("his role at his domain");
    let refused = world
        .app
        .call(RoleDelete, &[&domain.id])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::MemberDomainRole);
}

#[tokio::test]
async fn a_role_given_to_somebody_outside_the_tree_is_named_by_whoever_may_name_an_outsider() {
    let world = World::new().await;
    let s = setup(&world).await;
    let zed = person(&world, "zed").await;
    // Where the row that lets people name an outsider names nobody and the
    // policy is off, an owner may not.
    change(
        &world,
        &s.boss,
        s.web,
        &PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "access.external".to_owned(),
                profile: None,
                inherit: Some(false),
                locked: None,
                add: None,
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        },
    )
    .await
    .ok();
    world
        .set_at(Target::Node(s.acme), "sharing.allow_external", json!(false))
        .await;
    let refused = world
        .app
        .call(NodeRoleCreate, &[&s.web])
        .token(&s.boss.token)
        .body(&RoleCreateBody {
            subject: SubjectInput {
                kind: SubjectKind::User,
                role: None,
                id: zed.id(),
            },
            role: Role::Viewer,
            expires_at: None,
            note: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::SharingExternalForbidden);
    world
        .set_at(Target::Node(s.acme), "sharing.allow_external", json!(true))
        .await;
    let given = world
        .app
        .call(NodeRoleCreate, &[&s.web])
        .token(&s.boss.token)
        .body(&RoleCreateBody {
            subject: SubjectInput {
                kind: SubjectKind::User,
                role: None,
                id: zed.id(),
            },
            role: Role::Viewer,
            expires_at: None,
            note: None,
        })
        .send()
        .await
        .ok();
    assert!(given.outside);
    // She reads the project, and makes nobody a member.
    assert!(matches!(read(&world, &zed, s.web).await, Node::Project(_)));
}

#[tokio::test]
async fn what_one_person_holds_is_told_with_what_gives_it_to_them() {
    let world = World::new().await;
    let s = setup(&world).await;
    change(
        &world,
        &s.boss,
        s.web,
        &naming(s.dan.id(), AccessChoice::FullInteract),
    )
    .await
    .ok();
    let held = world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &s.web])
        .token(&s.boss.token)
        .query(&EffectiveAccessGetQuery {
            user_id: s.dan.id(),
        })
        .send()
        .await
        .ok();
    assert_eq!(held.user.username, "dan");
    assert_eq!(held.role, Some(Role::Viewer));
    assert!(!held.administers);
    let interact = held
        .permissions
        .iter()
        .find(|each| each.permission == "project.interact")
        .expect("he holds it now");
    assert!(interact.through.iter().any(|through| matches!(
        through,
        orch_api_contract::access::AccessThrough::Entry { .. }
    )));
    // Himself, he may ask; of another, he may not.
    let myself = world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &s.web])
        .token(&s.dan.token)
        .query(&EffectiveAccessGetQuery {
            user_id: s.dan.id(),
        })
        .send()
        .await
        .ok();
    assert_eq!(myself.user.username, "dan");
    let refused = world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &s.web])
        .token(&s.dan.token)
        .query(&EffectiveAccessGetQuery {
            user_id: s.boss.id(),
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "access.manage");
}

#[tokio::test]
async fn the_people_behind_a_chip_are_listed_a_page_at_a_time_and_only_for_a_chip_the_page_shows() {
    let world = World::new().await;
    let s = setup(&world).await;
    let (group, extra): (Id, Vec<Who>) = {
        let mut people = Vec::new();
        for name in ["gina", "hank", "iris"] {
            let who = person(&world, name).await;
            join(&world, s.acme, s.acme, &who, "viewer", &s.boss).await;
            people.push(who);
        }
        let mut conn = world.conn().await;
        let group = access_rows::insert_group(&mut conn, s.acme, "Crew", s.boss.id())
            .await
            .unwrap();
        for who in &people {
            access_rows::add_to_group(&mut conn, group, who.id())
                .await
                .unwrap();
        }
        (group, people)
    };
    change(
        &world,
        &s.boss,
        s.web,
        &PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::Group { id: group }],
                choice: AccessChoice::FullView,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        },
    )
    .await
    .ok();
    let chip = format!("group:{group}");
    let first = world
        .app
        .call(PermissionPeopleList, &[&"nodes", &s.web])
        .token(&s.dan.token)
        .query(&PermissionPeopleListQuery {
            holder: chip.clone(),
            limit: Some(2),
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(first.items.len(), 2);
    let rest = world
        .app
        .call(PermissionPeopleList, &[&"nodes", &s.web])
        .token(&s.dan.token)
        .query(&PermissionPeopleListQuery {
            holder: chip,
            limit: Some(2),
            cursor: first.next_cursor.clone(),
        })
        .send()
        .await
        .ok();
    assert_eq!(rest.items.len(), 1);
    assert!(rest.next_cursor.is_none());
    let mut names: Vec<String> = first
        .items
        .iter()
        .chain(&rest.items)
        .map(|user| user.username.clone())
        .collect();
    names.sort();
    let mut expected: Vec<String> = extra
        .iter()
        .map(|who| who.person.username.clone())
        .collect();
    expected.sort();
    assert_eq!(names, expected);
    // The admins of the place.
    let admins = world
        .app
        .call(PermissionPeopleList, &[&"nodes", &s.web])
        .token(&s.dan.token)
        .query(&PermissionPeopleListQuery {
            holder: "admins".to_owned(),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert!(admins.items.iter().any(|user| user.username == "boss"));
    // A chip the page does not show is not answered.
    for chip in [
        format!("group:{}", orch_types::new_id()),
        "everyone".to_owned(),
        "role:nonsense".to_owned(),
    ] {
        let refused = world
            .app
            .call(PermissionPeopleList, &[&"nodes", &s.web])
            .token(&s.dan.token)
            .query(&PermissionPeopleListQuery {
                holder: chip.clone(),
                limit: None,
                cursor: None,
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::NotFound, "{chip}");
    }
}
