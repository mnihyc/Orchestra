//! The five doors of an environment are rows of its own page and of no thread's: each row alone opens what it opens, whoever manages the environment holds the Manage of all five, the system ships them as it says in both placements, a thread's people are let into its environment by the tick and no further, and a row lost while a shell is open closes it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{Area, ObjectKind, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::depends::Open;
use orch_authz::doors::{self, Offer, Wants};
use orch_authz::environments::{self, Shell};
use orch_authz::pages::Target;
use orch_authz::usable::Policies;
use orch_authz::{Caller, Policy};
use orch_types::Id;
use serde_json::json;
use support::{World, user_holder};

/// The rows of an environment's page that are its own two levels and its five doors: what a person
/// holds there apart from saying who else may.
fn the_room(held: Vec<P>) -> Vec<P> {
    held.into_iter()
        .filter(|row| row.area() == Area::Environment || row.area().is_door())
        .collect()
}

/// The ten rows of the five doors, each with the row that it holds below it, if any.
const DOORS: [(P, Option<P>); 10] = [
    (P::FilesView, None),
    (P::FilesManage, Some(P::FilesView)),
    (P::TerminalView, None),
    (P::TerminalManage, Some(P::TerminalView)),
    (P::BrowserView, None),
    (P::BrowserManage, Some(P::BrowserView)),
    (P::VncView, None),
    (P::VncManage, Some(P::VncView)),
    (P::PortsView, None),
    (P::PortsManage, Some(P::PortsView)),
];

/// The rows of the page of an environment that the caller holds when they look into it.
const LOOKING: [P; 6] = [
    P::EnvironmentView,
    P::FilesView,
    P::TerminalView,
    P::BrowserView,
    P::VncView,
    P::PortsView,
];

/// An environment of the topic with every row of its page cut loose, so that no default and no role
/// gives anything: it is held by whoever the page names, and by its owner, who is the org's boss.
/// The thread of the world runs in it.
async fn closed(w: &World) -> Id {
    let environment = w.thing("environment", w.boss, w.topic, "Main").await;
    w.put_in(w.thread, environment).await;
    for row in ObjectKind::Environment.permissions() {
        if row.area() == Area::Environment || row.area().is_door() {
            w.thing_row("environment", environment, row, false).await;
        }
    }
    environment
}

#[tokio::test]
async fn each_of_the_ten_rows_of_the_five_doors_alone_opens_what_it_opens_and_nothing_else() {
    let w = World::new().await;
    let environment = closed(&w).await;
    for (row, below) in DOORS {
        let person = w
            .member(&format!("named_in_{}", row.as_str().replace('.', "_")))
            .await;
        w.grant("environment", environment, row, Holder::User { id: person })
            .await;
        let mut expected: Vec<P> = below.into_iter().collect();
        expected.push(row);
        assert_eq!(
            w.holds_on(person, "environment", environment).await,
            expected,
            "{row}: itself and the row it holds, no other door, not the environment, not a thing about access"
        );
        assert!(
            w.holds(person, w.thread).await.is_empty(),
            "{row} gives not a word in the thread that runs there"
        );
    }
}

#[tokio::test]
async fn whoever_manages_an_environment_holds_the_manage_of_all_five_doors_with_nobody_named_in_them()
 {
    let w = World::new().await;
    let environment = closed(&w).await;
    let manager = w.member("manager").await;
    w.grant(
        "environment",
        environment,
        P::EnvironmentManage,
        Holder::User { id: manager },
    )
    .await;
    let held = w.holds_on(manager, "environment", environment).await;
    let everything: Vec<P> = the_room(ObjectKind::Environment.permissions().collect());
    assert_eq!(
        held, everything,
        "its two levels and all ten rows of its doors"
    );
    assert_eq!(everything.len(), 12);
    // Not one of the doors names them, and each is held through what they manage.
    let principal = w.principal(Caller::session(manager)).await;
    let mut conn = w.conn().await;
    let effective = orch_authz::resources::effective(
        &mut conn,
        &principal,
        support::things::object("environment", environment),
        Policy::default(),
    )
    .await
    .unwrap();
    for (door, _) in DOORS {
        let held = effective
            .held
            .iter()
            .find(|held| held.permission == door)
            .unwrap();
        assert_eq!(
            held.through,
            [orch_access_contract::decisions::Through::Level {
                of: P::EnvironmentManage
            }],
            "{door}"
        );
    }
    drop(conn);
    // It is a word in no thread there, whatever else the page says.
    assert!(w.holds(manager, w.thread).await.is_empty());
}

#[tokio::test]
async fn a_lock_on_the_manage_of_a_door_does_not_take_it_from_whoever_manages_the_environment() {
    let w = World::new().await;
    let manager = w.member("manager").await;
    let reader = w.member("reader").await;
    let environment = w.thing("environment", w.boss, w.topic, "Main").await;
    w.grant(
        "environment",
        environment,
        P::EnvironmentManage,
        Holder::User { id: manager },
    )
    .await;
    w.grant(
        "environment",
        environment,
        P::TerminalManage,
        Holder::User { id: reader },
    )
    .await;
    // The org keeps shells to nobody but the Admins, and locks it.
    w.change_defaults(
        w.boss,
        w.root,
        json!({ "rows": [{ "permission": "terminal.manage", "profile": "managed", "inherit": false, "locked": true,
                           "add": [{ "holder": support::role_holder(Role::Admin) }] }] }),
    )
    .await
    .unwrap();
    assert!(
        !w.can_on(reader, "environment", environment, P::TerminalManage)
            .await,
        "what the page named counts for nothing under the lock"
    );
    assert!(
        w.can_on(manager, "environment", environment, P::TerminalManage)
            .await,
        "managing the environment holds the Manage of its doors whatever a row says, because an agent put there can do all of it"
    );
}

#[tokio::test]
async fn the_doors_ship_to_the_roles_of_the_place_in_both_placements() {
    for placement in ["managed", "connected"] {
        let w = World::new().await;
        let mut conn = w.conn().await;
        let environment = orch_db::queries::resources::access_fixtures::insert_environment(
            &mut conn,
            w.boss,
            Some(w.topic),
            "Main",
            placement,
        )
        .await
        .unwrap();
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("environment", environment, "live"),
            "any",
            "access.own",
            ("user", Some(w.boss), None),
            None,
            w.now.get(),
        )
        .await
        .unwrap();
        drop(conn);
        let everything: Vec<P> = the_room(ObjectKind::Environment.permissions().collect());
        let looking: Vec<P> = LOOKING.to_vec();
        // As shipped: whoever sees an environment looks into it, in both placements. Whoever manages
        // it holds the Manage of the doors, and where it is made from a template those are the
        // Developers; on a person's machine nobody until its page says so.
        for (role, expected) in [
            (Role::Viewer, looking.clone()),
            (Role::Interactor, looking.clone()),
            (
                Role::Developer,
                if placement == "managed" {
                    everything.clone()
                } else {
                    looking.clone()
                },
            ),
            (
                Role::Admin,
                if placement == "managed" {
                    everything.clone()
                } else {
                    looking.clone()
                },
            ),
        ] {
            let person = w.member(&format!("{placement}_{}", role.as_str())).await;
            w.give(w.root, person, role).await;
            assert_eq!(
                the_room(w.holds_on(person, "environment", environment).await),
                expected,
                "{placement}: a {role}"
            );
        }
        // Browsers and displays are driven by whoever manages the environment and by nobody else, in
        // both placements: neither row ships to any role.
        for manage in [P::BrowserManage, P::VncManage, P::PortsManage] {
            let nobody = w
                .member(&format!(
                    "{placement}_{}",
                    manage.as_str().replace('.', "_")
                ))
                .await;
            w.give(w.root, nobody, Role::Interactor).await;
            assert!(!w.can_on(nobody, "environment", environment, manage).await);
        }
    }
}

#[tokio::test]
async fn somebody_named_only_on_a_thread_is_refused_its_environment_and_the_tick_lets_them_in_and_no_further()
 {
    let w = World::new().await;
    let environment = closed(&w).await;
    let guest = w.member("guest").await;
    w.change_as_admin(
        w.boss,
        Target::Live(w.thread),
        json!({ "name": [{ "holders": [user_holder(guest)], "choice": "full_manage" }] }),
    )
    .await
    .unwrap();
    // Everything on the thread, and nothing in the room it runs in.
    assert!(w.can(guest, w.thread, P::ThreadManage).await);
    assert!(
        w.holds_on(guest, "environment", environment)
            .await
            .is_empty(),
        "naming somebody on a thread opens none of its files, no shell, no browser"
    );
    // The page of the thread shows the environment beside the names.
    let policies = Policies::default();
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let beside = doors::of_thread(&mut conn, &principal, w.thread, environment, &policies)
        .await
        .unwrap();
    let wants: Vec<_> = beside
        .iter()
        .map(|dependency| (dependency.for_permission, dependency.wants))
        .collect();
    assert_eq!(
        wants,
        [
            (P::ThreadView, Wants::Looking),
            (P::ThreadManage, Wants::Door(P::FilesManage)),
            (P::ThreadManage, Wants::Door(P::TerminalManage)),
        ]
    );
    assert!(
        beside
            .iter()
            .all(|dependency| dependency.open == Open::Nobody && dependency.can_give),
        "nobody is let in yet, and the giver owns the environment"
    );
    // Ticking "Let them into Main" for Can manage is the three changes the page offers.
    let holders = [Holder::User { id: guest }];
    for dependency in &beside {
        let offer = doors::offer(&mut conn, &principal, dependency, &holders, &policies)
            .await
            .unwrap();
        let Offer::Give { object, change } = offer else {
            panic!("the owner gives it themself");
        };
        assert_eq!(object.id, environment);
        w.change_thing(
            w.boss,
            "environment",
            environment,
            serde_json::to_value(&*change).unwrap(),
        )
        .await
        .unwrap();
    }
    drop(conn);
    assert_eq!(
        w.holds_on(guest, "environment", environment).await,
        [
            P::EnvironmentView,
            P::FilesView,
            P::FilesManage,
            P::TerminalView,
            P::TerminalManage,
            P::BrowserView,
            P::VncView,
            P::PortsView,
        ],
        "to look at everything, and to change the files and open a shell: never the environment, never the Manage of browsers or displays"
    );
    // The page now says so.
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let after = doors::of_thread(&mut conn, &principal, w.thread, environment, &policies)
        .await
        .unwrap();
    assert!(
        after.iter().all(|dependency| dependency.open == Open::Part),
        "the guest has it now, and the thread's maker, who owns the thread, does not"
    );
}

#[tokio::test]
async fn the_tick_gives_what_each_of_the_three_choices_goes_with_and_without_it_nothing_is_given() {
    for (choice, manages) in [
        ("full_view", false),
        ("full_interact", false),
        ("full_manage", true),
    ] {
        let w = World::new().await;
        let environment = closed(&w).await;
        let guest = w.member("guest").await;
        w.change_as_admin(
            w.boss,
            Target::Live(w.thread),
            json!({ "name": [{ "holders": [user_holder(guest)], "choice": choice }] }),
        )
        .await
        .unwrap();
        // Without the tick no choice opens anything of the room the thread runs in.
        assert!(
            w.holds_on(guest, "environment", environment)
                .await
                .is_empty(),
            "{choice}"
        );
        // With it: the changes that the page offers for what the choice gives on the thread, and for
        // nothing it does not give.
        let policies = Policies::default();
        let principal = w.principal(Caller::session(w.boss)).await;
        let mut conn = w.conn().await;
        let beside = doors::of_thread(&mut conn, &principal, w.thread, environment, &policies)
            .await
            .unwrap();
        let holders = [Holder::User { id: guest }];
        let mut ticked = 0;
        for dependency in &beside {
            if !w.can(guest, w.thread, dependency.for_permission).await {
                continue;
            }
            ticked += 1;
            let offer = doors::offer(&mut conn, &principal, dependency, &holders, &policies)
                .await
                .unwrap();
            let Offer::Give { change, .. } = offer else {
                panic!("{choice}: the owner gives it themself");
            };
            w.change_thing(
                w.boss,
                "environment",
                environment,
                serde_json::to_value(&*change).unwrap(),
            )
            .await
            .unwrap();
        }
        drop(conn);
        let expected: Vec<P> = if manages {
            vec![
                P::EnvironmentView,
                P::FilesView,
                P::FilesManage,
                P::TerminalView,
                P::TerminalManage,
                P::BrowserView,
                P::VncView,
                P::PortsView,
            ]
        } else {
            LOOKING.to_vec()
        };
        assert_eq!(
            w.holds_on(guest, "environment", environment).await,
            expected,
            "{choice}"
        );
        assert_eq!(
            ticked,
            if manages { 3 } else { 1 },
            "{choice}: looking in for each choice, and the Manage of files and of shells with Can manage alone"
        );
    }
}

#[tokio::test]
async fn where_the_giver_does_not_hold_the_environment_the_same_step_asks_whoever_can_give_it() {
    let w = World::new().await;
    let environment = closed(&w).await;
    let developer = w.member("developer").await;
    let guest = w.member("guest").await;
    // A Developer of the place manages threads, and shares them; the environment's page was cut loose
    // from the Developers, so they hold nothing there.
    w.give(w.project, developer, Role::Developer).await;
    assert!(w.can(developer, w.thread, P::ThreadManage).await);
    let policies = Policies::default();
    let principal = w.principal(Caller::session(developer)).await;
    let mut conn = w.conn().await;
    let beside = doors::of_thread(&mut conn, &principal, w.thread, environment, &policies)
        .await
        .unwrap();
    assert!(
        beside.iter().all(|dependency| !dependency.can_give),
        "they cannot give what they do not hold"
    );
    let holders = [Holder::User { id: guest }];
    let Offer::Ask {
        item,
        holders: asked,
    } = doors::offer(&mut conn, &principal, &beside[0], &holders, &policies)
        .await
        .unwrap()
    else {
        panic!("asked of whoever can give it");
    };
    assert_eq!(item.object.id, environment);
    assert_eq!(item.object.kind, ObjectKind::Environment);
    assert_eq!(
        item.permissions,
        LOOKING.to_vec(),
        "looking in is the environment and the View row of each door, and nothing more"
    );
    assert!(item.ask.contains(&w.boss), "its owner can give it");
    assert_eq!(asked, holders);
    // Nothing was given to the guest by naming them on the thread.
    assert!(
        w.holds_on(guest, "environment", environment)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn somebody_who_loses_what_they_opened_a_shell_by_has_it_closed_wherever_the_shell_was_opened_beside()
 {
    let w = World::new().await;
    let environment = closed(&w).await;
    let opener = w.member("opener").await;
    let entry = w
        .grant(
            "environment",
            environment,
            P::EnvironmentManage,
            Holder::User { id: opener },
        )
        .await;
    let policy = Policy::default();
    // Opened beside a thread, and from the environment's own page.
    for thread in [Some(w.thread), None] {
        let shell = Shell {
            environment,
            thread,
            opened_by: opener,
        };
        let principal = w.principal(Caller::session(opener)).await;
        let mut conn = w.conn().await;
        assert!(
            environments::stays_open(&mut conn, &principal, &shell, policy)
                .await
                .unwrap(),
            "they manage the environment: the Manage of its terminal is theirs"
        );
    }
    let mut conn = w.conn().await;
    sqlx::query("UPDATE permission_entries SET removed_at = now() WHERE id = $1")
        .bind(entry)
        .execute(&mut *conn)
        .await
        .unwrap();
    for thread in [Some(w.thread), None] {
        let shell = Shell {
            environment,
            thread,
            opened_by: opener,
        };
        let principal = w.principal(Caller::session(opener)).await;
        assert!(
            !environments::stays_open(&mut conn, &principal, &shell, policy)
                .await
                .unwrap(),
            "the row that opened it is gone, whichever thread it was opened beside"
        );
    }
}
