//! Who may share, and how much, over HTTP: a page says beforehand what its reader may give and what a place covers, roles are given by the rule of the rows, and the policy that keeps sharing to owners holds at each scope.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::access::{
    AccessChoice, AgentLevel, Holder, HolderInput, NodeDefaultsGet, NodeRoleCreate, NodeRoleList,
    PageChange, PageChangeAdd, PageChangeName, PageChangeRow, PermissionPage, PermissionPageGet,
    PermissionPageUpdate, PermissionPeopleList, PermissionPeopleListQuery, Role, RoleCreateBody,
    RoleDelete, RoleUpdate, RoleUpdateBody, SubjectInput, SubjectKind,
};
use orch_api_contract::common::ErrorCode;
use orch_authz::documents;
use orch_db::queries::access::PageKey;
use orch_db::queries::access::testing as rows;
use orch_db::queries::documents as document_rows;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_settings::Target;
use orch_types::{Id, new_id};
use places::{Who, give_role, join, make_org, make_project, make_topic, person};
use serde_json::json;
use support::World;

/// Acme with its owner `boss`, a viewer `dan` and the project Web, where `eve` and `fay` are
/// Developers.
struct Firm {
    boss: Who,
    dan: Who,
    eve: Who,
    fay: Who,
    acme: Id,
    web: Id,
}

async fn firm(world: &World) -> Firm {
    let boss = person(world, "boss").await;
    let (dan, eve, fay) = (
        person(world, "dan").await,
        person(world, "eve").await,
        person(world, "fay").await,
    );
    let acme = make_org(world, &boss, "Acme", "acme").await.base.id;
    for who in [&dan, &eve, &fay] {
        join(world, acme, acme, who, "viewer", &boss).await;
    }
    let web = make_project(world, &boss, acme, "Web").await.base.id;
    for who in [&eve, &fay] {
        give_role(world, web, who, "developer", &boss).await;
    }
    Firm {
        boss,
        dan,
        eve,
        fay,
        acme,
        web,
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

async fn give(
    world: &World,
    by: &Who,
    place: Id,
    user: Id,
    role: Role,
) -> orch_api_core::testing::Reply<orch_api_contract::access::RoleHeld> {
    world
        .app
        .call(NodeRoleCreate, &[&place])
        .token(&by.token)
        .body(&RoleCreateBody {
            subject: SubjectInput {
                kind: SubjectKind::User,
                role: None,
                id: user,
            },
            role,
            expires_at: None,
            note: None,
        })
        .send()
        .await
}

async fn the_policy(world: &World, place: Id, value: bool) {
    world
        .set_at(Target::Node(place), "sharing.allow_holders", json!(value))
        .await;
}

#[tokio::test]
async fn a_page_tells_what_its_reader_may_give_and_how_many_people_a_role_stands_for() {
    let world = World::new().await;
    let f = firm(&world).await;
    // The owner gives what they like; a Viewer gives what a Viewer has and no role above Viewer.
    let boss = page(&world, &f.boss, f.web).await;
    assert_eq!(
        boss.gives.choices,
        [
            AccessChoice::FullView,
            AccessChoice::FullInteract,
            AccessChoice::FullManage
        ]
    );
    assert_eq!(
        boss.gives.roles,
        [Role::Viewer, Role::Interactor, Role::Developer, Role::Admin]
    );
    assert!(boss.gives.holders_share && boss.rows.iter().any(|row| row.can_give));
    let dan = page(&world, &f.dan, f.web).await;
    assert_eq!(dan.gives.choices, [AccessChoice::FullView]);
    assert_eq!(dan.gives.roles, [Role::Viewer]);
    assert_eq!(dan.gives.agents, [AgentLevel::View]);
    let view = dan
        .rows
        .iter()
        .find(|row| row.permission == "project.view")
        .unwrap();
    let manage = dan
        .rows
        .iter()
        .find(|row| row.permission == "project.manage")
        .unwrap();
    assert!(
        view.can_give && !view.writable,
        "giving is not managing the row"
    );
    assert!(!manage.can_give);
    // A role named on a page is told with how many people it is now, and the people request lists the same.
    change(
        &world,
        &f.boss,
        f.web,
        &PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "project.interact".to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![PageChangeAdd {
                    holder: HolderInput::Role {
                        role: Role::Developer,
                        at: None,
                    },
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
    let shown = page(&world, &f.dan, f.web).await;
    let chip = shown
        .rows
        .iter()
        .find(|row| row.permission == "project.interact")
        .unwrap()
        .entries
        .iter()
        .find_map(|entry| match &entry.holder {
            Holder::Role {
                role: Role::Developer,
                count,
                ..
            } => Some(*count),
            _ => None,
        })
        .expect("the chip of the Developers");
    let people = world
        .app
        .call(PermissionPeopleList, &[&"nodes", &f.web])
        .token(&f.dan.token)
        .query(&PermissionPeopleListQuery {
            holder: format!("role:{}:developer", f.web),
            limit: Some(200),
            cursor: None,
        })
        .send()
        .await
        .ok();
    let mut names: Vec<&str> = people
        .items
        .iter()
        .map(|user| user.username.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["boss", "eve", "fay"],
        "the owner counts as an Admin"
    );
    assert_eq!(
        chip,
        i64::try_from(names.len()).unwrap(),
        "the chip counts the people that the request lists"
    );
}

#[tokio::test]
async fn a_place_tells_how_many_places_threads_and_documents_inside_follow_it_and_how_many_do_not()
{
    let world = World::new().await;
    let f = firm(&world).await;
    let login = make_topic(&world, &f.boss, f.web, "Login").await.base.id;
    let secret = make_topic(&world, &f.boss, f.web, "Secret").await.base.id;
    let (fix, hidden) = (new_id(), new_id());
    {
        let mut conn = world.conn().await;
        let owned = f.boss.id();
        insert_node(&mut conn, fix, "thread", Some(login), "Fix", owned)
            .await
            .unwrap();
        insert_node(&mut conn, hidden, "thread", Some(login), "Hidden", owned)
            .await
            .unwrap();
        for (node, permission) in [(secret, "topic.view"), (hidden, "thread.view")] {
            rows::set_row(
                &mut conn,
                &PageKey::new("node", node, "live"),
                "any",
                permission,
                false,
                false,
            )
            .await
            .unwrap();
        }
        for (title, private) in [("Handbook", false), ("Secrets", true)] {
            let document = new_id();
            document_rows::insert(
                &mut conn,
                &document_rows::NewDocument {
                    id: document,
                    node_id: f.web,
                    org_id: f.acme,
                    owner_id: owned,
                    created_by_thread: None,
                    created_by_agent: None,
                    title,
                    now: world.now(),
                },
            )
            .await
            .unwrap();
            documents::start(
                &mut conn,
                document,
                owned,
                orch_types::Time::new(world.now()),
            )
            .await
            .unwrap();
            if private {
                rows::set_row(
                    &mut conn,
                    &PageKey::new("document", document, "live"),
                    "any",
                    "doc.view",
                    false,
                    false,
                )
                .await
                .unwrap();
            }
        }
    }
    // The owner sees what follows and what was made private; a Viewer sees only what follows.
    let boss = page(&world, &f.boss, f.web)
        .await
        .covers
        .expect("a place says what it covers");
    assert_eq!((boss.places, boss.threads, boss.documents), (1, 1, 1));
    assert_eq!(
        (boss.apart.places, boss.apart.threads, boss.apart.documents),
        (1, 1, 1),
        "the private topic, the thread cut loose and the document made private"
    );
    let dan = page(&world, &f.dan, f.web).await.covers.unwrap();
    assert_eq!((dan.places, dan.threads, dan.documents), (1, 1, 1));
    assert_eq!(
        (dan.apart.places, dan.apart.threads, dan.apart.documents),
        (0, 0, 0),
        "what he may not see is counted in neither"
    );
    // Nothing else says it: not a thread, not the defaults of the place.
    assert!(page(&world, &f.boss, fix).await.covers.is_none());
    let defaults = world
        .app
        .call(NodeDefaultsGet, &[&f.web])
        .token(&f.boss.token)
        .send()
        .await
        .ok();
    assert!(defaults.covers.is_none());
}

#[tokio::test]
async fn a_role_is_given_by_the_rule_of_the_rows_up_to_the_giver_s_own_and_taken_back_by_whoever_gave_it()
 {
    let world = World::new().await;
    let f = firm(&world).await;
    let (gus, hal) = (person(&world, "gus").await, person(&world, "hal").await);
    for each in [&gus, &hal] {
        join(&world, f.acme, f.acme, each, "viewer", &f.boss).await;
    }
    // What the roles request tells beforehand.
    for (who, expected) in [
        (&f.dan, vec![Role::Viewer]),
        (
            &f.fay,
            vec![Role::Viewer, Role::Interactor, Role::Developer],
        ),
        (
            &f.boss,
            vec![Role::Viewer, Role::Interactor, Role::Developer, Role::Admin],
        ),
    ] {
        let listed = world
            .app
            .call(NodeRoleList, &[&f.web])
            .token(&who.token)
            .send()
            .await
            .ok();
        assert_eq!(listed.can_give, expected, "{}", who.person.username);
    }
    // A Developer gives a Developer's role, and the role says who gave it; Admin is for an admin.
    let given = give(&world, &f.fay, f.web, gus.id(), Role::Developer)
        .await
        .ok();
    assert_eq!(given.created_by.username, "fay");
    let refused = give(&world, &f.fay, f.web, hal.id(), Role::Admin)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "access.own");
    // A Viewer gives no more than a Viewer's.
    assert_eq!(
        give(&world, &f.dan, f.web, hal.id(), Role::Interactor)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    // Whoever gave a role takes it back, and not one that somebody else gave; a role is not raised past the giver's own.
    let boss_gave = give(&world, &f.boss, f.web, hal.id(), Role::Viewer)
        .await
        .ok();
    let taking = |who: &Who, role: Id| {
        world
            .app
            .call(RoleDelete, &[&role])
            .token(&who.token)
            .send()
    };
    assert_eq!(
        taking(&f.fay, boss_gave.id).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        taking(&f.eve, given.id).await.problem().code,
        ErrorCode::AuthzDenied,
        "nor one that a colleague of the same level gave"
    );
    taking(&f.fay, given.id).await.ok();
    let raised = world
        .app
        .call(RoleUpdate, &[&boss_gave.id])
        .token(&f.boss.token)
        .body(&RoleUpdateBody {
            role: Some(Role::Interactor),
            expires_at: None,
            note: None,
            expected_revision: boss_gave.revision,
        })
        .send()
        .await
        .ok();
    assert_eq!(raised.role, Role::Interactor);
    // Where the place keeps sharing to its owners and to whoever manages its page, Fay gives none, and the owner still does.
    the_policy(&world, f.acme, false).await;
    let kept = give(&world, &f.fay, f.web, gus.id(), Role::Viewer)
        .await
        .problem();
    assert_eq!(kept.code, ErrorCode::AuthzDenied);
    assert_eq!(kept.extra["required"], "access.manage");
    let ask = kept.extra["missing"][0]["ask"].as_array().unwrap();
    assert!(
        ask.iter().any(|user| user["username"] == "boss"),
        "and told who can: {ask:?}"
    );
    let listed = world
        .app
        .call(NodeRoleList, &[&f.web])
        .token(&f.fay.token)
        .send()
        .await
        .ok();
    assert!(
        listed.can_give.is_empty(),
        "the roles request says so beforehand"
    );
    give(&world, &f.boss, f.web, gus.id(), Role::Viewer)
        .await
        .ok();
}

#[tokio::test]
async fn the_policy_that_keeps_sharing_to_owners_is_read_at_each_scope_and_a_lock_above_decides() {
    let world = World::new().await;
    let f = firm(&world).await;
    let login = make_topic(&world, &f.boss, f.web, "Login").await.base.id;
    let guests: Vec<Who> = {
        let mut made = Vec::new();
        for name in ["guy1", "guy2", "guy3", "guy4"] {
            let guest = person(&world, name).await;
            join(&world, f.acme, f.acme, &guest, "viewer", &f.boss).await;
            made.push(guest);
        }
        made
    };
    let share = |at: usize| {
        let body = naming(guests[at].id(), AccessChoice::FullView);
        let (world, fay) = (&world, &f.fay);
        async move { change(world, fay, login, &body).await }
    };
    // As shipped the holders of a thing share it.
    share(0).await.ok();
    // Off for the org: a Developer of the project is refused, and told that the page is for those who manage it.
    the_policy(&world, f.acme, false).await;
    let refused = share(1).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "access.manage");
    let page_now = page(&world, &f.fay, login).await;
    assert!(!page_now.gives.holders_share && page_now.rows.iter().all(|row| !row.can_give));
    // The owner shares all the same.
    change(
        &world,
        &f.boss,
        login,
        &naming(guests[1].id(), AccessChoice::FullView),
    )
    .await
    .ok();
    // On again for the project, which is nearer than the org.
    the_policy(&world, f.web, true).await;
    share(2).await.ok();
    // Off for the topic, nearer still.
    the_policy(&world, login, false).await;
    assert_eq!(share(3).await.problem().code, ErrorCode::AuthzDenied);
    // A lock at the org decides for everything inside.
    the_policy(&world, login, true).await;
    {
        let mut conn = world.conn().await;
        orch_settings::write::write(
            &mut conn,
            &orch_settings::write::Write {
                actor: orch_db::audit::Actor::System,
                target: Target::Node(f.acme),
                key: "sharing.allow_holders",
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
    assert_eq!(share(3).await.problem().code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn somebody_who_holds_nothing_on_a_page_learns_nothing_by_asking_to_change_it() {
    let world = World::new().await;
    let f = firm(&world).await;
    let stranger = person(&world, "stranger").await;
    let refused = change(&world, &stranger, f.web, &PageChange::default())
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    // A reader who owns nothing and manages nothing is not let to do what only they do.
    let cut = change(
        &world,
        &f.fay,
        f.web,
        &PageChange {
            whole: Some(orch_api_contract::access::PageChangeWhole::Private),
            ..PageChange::default()
        },
    )
    .await
    .problem();
    assert_eq!(cut.code, ErrorCode::AuthzDenied);
    assert_eq!(cut.extra["required"], "access.manage");
}
