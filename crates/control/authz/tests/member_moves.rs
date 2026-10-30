//! A member is moved from one org of their tree to another by somebody who holds `members.manage` at both ends: what they held by the old place is replaced by what the new one gives, and everything that names them in person stays; the plan says it in numbers beforehand.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::{NodeKind, Role};
use orch_authz::Caller;
use orch_authz::ownership::{
    self, MemberMove, MoveCheck, MoveError, MovePlan, Moved, Reached, Unmovable, Waiting,
};
use orch_authz::usable::{self, Author, Policies, TurnAsk, TurnDecision};
use orch_db::audit::Origin;
use orch_db::queries::access::testing::{give_role, insert_member};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_types::Id;
use serde_json::json;
use support::World;

async fn plan(w: &World, by: Id, user: Id, to: Id, role: Option<Role>) -> MovePlan {
    let principal = w.principal(Caller::session(by)).await;
    let mut conn = w.conn().await;
    ownership::plan_move(
        &mut conn,
        &principal,
        &MemberMove { user, to, role },
        &Policies::default(),
    )
    .await
    .unwrap()
}

async fn move_by(
    w: &World,
    by: Id,
    user: Id,
    to: Id,
    role: Option<Role>,
) -> Result<Moved, MoveError> {
    let principal = w.principal(Caller::session(by)).await;
    let mut tx = w.db.pool.begin().await.unwrap();
    let moved = ownership::move_member(
        &mut tx,
        &principal,
        &MemberMove { user, to, role },
        &Policies::default(),
        &Origin::default(),
    )
    .await;
    if moved.is_ok() {
        tx.commit().await.unwrap();
    }
    moved
}

/// A member of the tree of the root org with this domain, and this role there.
async fn member_in(w: &World, name: &str, domain: Id, role: Role) -> Id {
    let user = w.user(name).await;
    {
        let mut conn = w.conn().await;
        insert_member(&mut conn, w.root, user, domain)
            .await
            .unwrap();
    }
    w.give(domain, user, role).await;
    user
}

async fn domain_of(w: &World, user: Id) -> Id {
    w.principal_of(user)
        .await
        .memberships()
        .domain_in(w.root)
        .unwrap()
}

/// The roles a person is given by name, as (place, role).
async fn roles_of(w: &World, user: Id) -> Vec<(Id, String)> {
    let mut conn = w.conn().await;
    sqlx::query_as(
        "SELECT node_id, role FROM node_roles
          WHERE subject_kind = 'user' AND subject_id = $1 AND removed_at IS NULL ORDER BY node_id, role",
    )
    .bind(user)
    .fetch_all(&mut *conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_member_moves_up_down_and_sideways_by_an_admin_of_the_root_or_of_both_ends_and_not_of_one()
 {
    let w = World::new().await;
    let d = w.deep().await;
    let mel = member_in(&w, "mel", d.d3, Role::Developer).await;
    // Up to the root org, by an admin of the root (whose first owner is the boss).
    let moved = move_by(&w, w.boss, mel, w.root, None).await.unwrap();
    assert_eq!(
        moved,
        Moved {
            from: d.d3,
            to: w.root,
            role_before: Some(Role::Developer),
            role_after: Some(Role::Developer)
        }
    );
    assert_eq!(domain_of(&w, mel).await, w.root);
    assert_eq!(
        roles_of(&w, mel).await,
        [(w.root, "developer".to_owned())],
        "taken from the old org and given on the new one"
    );
    // Down into a department.
    move_by(&w, w.boss, mel, d.sales, None).await.unwrap();
    assert_eq!(domain_of(&w, mel).await, d.sales);
    // Sideways, by somebody who is an admin of both ends and of nothing above.
    let pair = w.member("pair").await;
    w.give(d.sales, pair, Role::Admin).await;
    w.give(d.ops, pair, Role::Admin).await;
    move_by(&w, pair, mel, d.ops, None).await.unwrap();
    assert_eq!(domain_of(&w, mel).await, d.ops);
    // An admin of one end alone is refused, and told who holds the other.
    let half = w.member("half").await;
    w.give(d.ops, half, Role::Admin).await;
    let Err(MoveError::Refused(refused)) = move_by(&w, half, mel, d.sales, None).await else {
        panic!("an admin of one end only is refused");
    };
    assert!(
        refused.leaving.is_none(),
        "they hold it where the person is"
    );
    let joining = refused.joining.expect("and lack it where the person goes");
    assert!(
        !joining.missing[0].ask.is_empty(),
        "somebody holds it there, and is named"
    );
    assert_eq!(domain_of(&w, mel).await, d.ops, "nothing was done");
    // The plan is shown to them with the numbers, and says what is lacking; to a stranger nothing.
    let shown = plan(&w, half, mel, d.sales, None).await;
    assert!(matches!(shown.check, MoveCheck::Refused(_)));
    assert!(shown.numbers.is_some());
    let stranger = w.member("stranger").await;
    let hidden = plan(&w, stranger, mel, d.sales, None).await;
    assert!(matches!(hidden.check, MoveCheck::Refused(_)));
    assert!(
        hidden.numbers.is_none(),
        "nothing of the person to somebody who holds nothing at either end"
    );
}

#[tokio::test]
async fn what_the_old_place_gave_is_replaced_by_what_the_new_one_gives_and_what_names_them_stays() {
    let w = World::new().await;
    let d = w.deep().await;
    let mel = member_in(&w, "mel", d.d3, Role::Developer).await;
    let retail = w.node("project", d.sales, "Retail").await;
    let sales_topic = w.node("topic", retail, "Calls").await;
    let sales_thread = w.node("thread", sales_topic, "Call").await;
    // Named in person: an Admin of a project in Sales, an entry on a provider, a group, a thread of theirs.
    w.give(d.sales_project, mel, Role::Admin).await;
    let ada = w.member("ada").await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    w.grant(
        "provider",
        provider,
        P::ProviderUse,
        Holder::User { id: mel },
    )
    .await;
    let group = w.group(w.root, "writers", &[mel]).await;
    let mine = w.node_owned_by("thread", d.topic, "Mine", mel).await;
    let epoch = w.epoch().await;
    assert!(
        w.can(mel, d.thread, P::ThreadInteract).await,
        "a Developer of Vision"
    );
    assert!(!w.can(mel, sales_thread, P::ThreadInteract).await);
    assert!(
        w.holds(mel, d.sales_project)
            .await
            .contains(&P::ProjectManage)
    );

    move_by(&w, w.boss, mel, d.sales, None).await.unwrap();

    assert_eq!(w.epoch().await, epoch + 1, "one move of the epoch");
    assert!(
        !w.can(mel, d.thread, P::ThreadInteract).await,
        "what Vision gave a Developer is gone"
    );
    assert!(
        w.can(mel, sales_thread, P::ThreadInteract).await,
        "what Sales gives a Developer is held"
    );
    assert!(
        w.holds(mel, d.sales_project)
            .await
            .contains(&P::ProjectManage),
        "a role named on a project stays"
    );
    assert!(
        matches!(
            w.usable(Caller::session(mel), provider, None).await,
            usable::Usable::Yes(_)
        ),
        "an entry stays"
    );
    assert!(
        w.principal_of(mel).await.groups().contains(&group),
        "and a group"
    );
    assert_eq!(
        w.read(w.boss, orch_authz::pages::Target::Live(mine))
            .await
            .first_owner,
        Some(mel),
        "and what they own"
    );
    let mut conn = w.conn().await;
    let (moved, by): (Option<chrono::DateTime<chrono::Utc>>, Option<Id>) = sqlx::query_as(
        "SELECT moved_at, moved_by FROM org_members WHERE org_id = $1 AND user_id = $2",
    )
    .bind(w.root)
    .bind(mel)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        (moved, by),
        (Some(w.now.get()), Some(w.boss)),
        "a member throughout, moved by the boss now"
    );
    drop(conn);
    let line = w
        .audit()
        .await
        .into_iter()
        .find(|row| row.action == "member.moved")
        .unwrap();
    assert!(line.history);
    assert_eq!(line.object_id, Some(mel));
    assert_eq!(line.details["from"], json!(d.d3));
    assert_eq!(line.details["to"], json!(d.sales));
}

#[tokio::test]
async fn a_role_held_already_on_the_new_org_is_combined_with_the_moved_one_and_admin_is_given_by_an_admin_only()
 {
    let w = World::new().await;
    let d = w.deep().await;
    // The higher of the two holds, whichever it was: one row on the new org.
    let mel = member_in(&w, "mel", d.d3, Role::Developer).await;
    w.give(d.sales, mel, Role::Interactor).await;
    let moved = move_by(&w, w.boss, mel, d.sales, None).await.unwrap();
    assert_eq!(moved.role_after, Some(Role::Developer));
    assert_eq!(roles_of(&w, mel).await, [(d.sales, "developer".to_owned())]);
    let nan = member_in(&w, "nan", d.d3, Role::Viewer).await;
    w.give(d.sales, nan, Role::Developer).await;
    let moved = move_by(&w, w.boss, nan, d.sales, None).await.unwrap();
    assert_eq!(
        moved.role_after,
        Some(Role::Developer),
        "the lower moved role does not take away the higher"
    );
    assert_eq!(roles_of(&w, nan).await, [(d.sales, "developer".to_owned())]);
    // The request may name another role.
    let ola = member_in(&w, "ola", d.d3, Role::Developer).await;
    let moved = move_by(&w, w.boss, ola, w.root, Some(Role::Viewer))
        .await
        .unwrap();
    assert_eq!(
        (moved.role_before, moved.role_after),
        (Some(Role::Developer), Some(Role::Viewer))
    );
    // Somebody who holds `members.manage` at both ends by their pages and is no admin where the person goes.
    let mover = w.member("mover").await;
    for place in [d.d3, d.sales] {
        w.name(place, P::MembersManage, Holder::User { id: mover })
            .await;
    }
    let ann = member_in(&w, "ann", d.d3, Role::Admin).await;
    let Err(MoveError::Refused(refused)) = move_by(&w, mover, ann, d.sales, None).await else {
        panic!("the same role is Admin, which an admin gives");
    };
    assert!(refused.leaving.is_none() && refused.joining.is_none());
    assert!(
        refused
            .admins_there
            .is_some_and(|admins| admins.contains(&w.boss)),
        "who may give it is told"
    );
    assert_eq!(domain_of(&w, ann).await, d.d3);
    // Another role than Admin is theirs to give.
    let moved = move_by(&w, mover, ann, d.sales, Some(Role::Developer))
        .await
        .unwrap();
    assert_eq!(moved.role_after, Some(Role::Developer));
    // An admin of the root gives it.
    let bea = member_in(&w, "bea", d.d3, Role::Admin).await;
    move_by(&w, w.boss, bea, d.sales, None).await.unwrap();
    assert_eq!(roles_of(&w, bea).await, [(d.sales, "admin".to_owned())]);
}

#[tokio::test]
async fn the_plan_says_in_numbers_what_the_person_loses_and_gains_and_changes_nothing() {
    let w = World::new().await;
    let d = w.deep().await;
    let mel = member_in(&w, "mel", d.d3, Role::Developer).await;
    {
        let mut conn = w.conn().await;
        give_role(
            &mut conn, w.project, "members", d.d3, "viewer", w.boss, None,
        )
        .await
        .unwrap();
        give_role(
            &mut conn,
            w.project,
            "members",
            d.sales,
            "interactor",
            w.boss,
            None,
        )
        .await
        .unwrap();
        give_role(
            &mut conn, w.topic, "members", d.sales, "viewer", w.boss, None,
        )
        .await
        .unwrap();
    }
    let mine = w.node_owned_by("thread", d.topic, "Mine", mel).await;
    let (epoch, roles) = (w.epoch().await, roles_of(&w, mel).await);
    // Sideways: Vision and what is in it for Sales and what is in it.
    let sideways = plan(&w, w.boss, mel, d.sales, None).await;
    assert_eq!(sideways.check, MoveCheck::Allowed);
    let numbers = sideways.numbers.unwrap();
    assert_eq!((numbers.from, numbers.to), (d.d3, d.sales));
    assert_eq!(
        (numbers.role_before, numbers.role_after),
        (Some(Role::Developer), Some(Role::Developer))
    );
    let reached = |kind, lost, gained| Reached { kind, lost, gained };
    assert_eq!(
        numbers.places,
        [
            reached(NodeKind::Org, 1, 1),
            reached(NodeKind::Project, 1, 1),
            reached(NodeKind::Thread, 2, 0),
            reached(NodeKind::Topic, 2, 0),
        ]
    );
    assert_eq!(
        (numbers.member_roles_lost, numbers.member_roles_gained),
        (1, 2)
    );
    assert_eq!(
        numbers
            .stays_behind
            .iter()
            .map(|thing| thing.object.id)
            .collect::<Vec<_>>(),
        [mine],
        "what they own in Vision stays theirs and stays in Vision"
    );
    // Up from the deepest department into the one above: it only gains.
    let up = plan(&w, w.boss, mel, d.d1, None).await.numbers.unwrap();
    assert_eq!(
        up.places,
        [reached(NodeKind::Org, 0, 3)],
        "Research itself, ML and Ops"
    );
    assert!(up.stays_behind.is_empty());
    // Nothing was done by planning.
    assert_eq!(domain_of(&w, mel).await, d.d3);
    assert_eq!(roles_of(&w, mel).await, roles);
    assert_eq!(w.epoch().await, epoch);
}

#[tokio::test]
async fn an_input_without_an_author_waits_on_a_thread_whose_provider_only_the_old_place_gave() {
    let w = World::new().await;
    let d = w.deep().await;
    let mel = member_in(&w, "mel", d.d3, Role::Developer).await;
    let ada = w.member("ada").await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    w.model(provider, "big", &["low"]).await;
    // Developers of Vision, and of nothing else, may use it.
    w.put(
        "provider",
        provider,
        P::ProviderUse,
        Holder::Role {
            role: Role::Developer,
            at: Some(d.d3),
        },
        json!({}),
    )
    .await;
    let mine = w.node_owned_by("thread", d.topic, "Mine", mel).await;
    w.bind_thread(mine, provider).await;
    let theirs = w.node_owned_by("thread", d.topic, "Other", mel).await;
    let elsewhere = w.provider(ada, w.root, "Open").await;
    w.model(elsewhere, "big", &["low"]).await;
    w.grant(
        "provider",
        elsewhere,
        P::ProviderUse,
        Holder::User { id: mel },
    )
    .await;
    w.bind_thread(theirs, elsewhere).await;
    let turn = |thread: Id, provider: Id| {
        let w = &w;
        async move {
            let mut conn = w.conn().await;
            usable::check_turn(
                &mut conn,
                Author::FirstOwner { now: w.now },
                &TurnAsk {
                    thread,
                    provider,
                    profile: None,
                    model: "big",
                    effort: None,
                    arrangement: None,
                    models_allowed: None,
                },
                &Policies::default(),
            )
            .await
            .unwrap()
        }
    };
    assert!(turn(mine, provider).await.is_allowed());
    let numbers = plan(&w, w.boss, mel, d.sales, None).await.numbers.unwrap();
    assert_eq!(
        numbers.would_wait,
        [Waiting {
            thread: mine,
            name: "Mine".to_owned(),
            provider
        }],
        "the thread on a provider they have by name stays with them"
    );
    assert!(
        turn(mine, provider).await.is_allowed(),
        "planning did not move anybody"
    );
    move_by(&w, w.boss, mel, d.sales, None).await.unwrap();
    assert!(
        matches!(turn(mine, provider).await, TurnDecision::Refused(_)),
        "and now the input waits"
    );
    assert!(
        turn(theirs, elsewhere).await.is_allowed(),
        "where the entry names them it goes on"
    );
}

#[tokio::test]
async fn a_move_that_cannot_be_asked_for_is_told_so_before_anybody_is_asked_what_they_hold() {
    let w = World::new().await;
    let d = w.deep().await;
    let mel = member_in(&w, "mel", d.d3, Role::Developer).await;
    let globex = orch_types::new_id();
    let outsider = w.user("outsider").await;
    {
        let mut conn = w.conn().await;
        insert_node(&mut conn, globex, "org", None, "Globex", w.boss)
            .await
            .unwrap();
    }
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let policies = Policies::default();
    for (user, to, expected) in [
        (mel, d.d3, Unmovable::AlreadyThere),
        (mel, d.project, Unmovable::NoSuchOrg),
        (mel, d.topic, Unmovable::NoSuchOrg),
        (mel, globex, Unmovable::NotAMember),
        (outsider, d.sales, Unmovable::NotAMember),
    ] {
        let check = ownership::may_move_member(
            &mut conn,
            &principal,
            &MemberMove {
                user,
                to,
                role: None,
            },
            &policies,
        )
        .await
        .unwrap();
        assert_eq!(check, MoveCheck::Impossible(expected), "{to} for {user}");
    }
}

#[tokio::test]
async fn planning_inside_a_transaction_leaves_it_whole_and_the_move_then_goes_on_in_it() {
    let w = World::new().await;
    let d = w.deep().await;
    let mel = member_in(&w, "mel", d.d3, Role::Developer).await;
    let ada = w.member("ada").await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    w.put(
        "provider",
        provider,
        P::ProviderUse,
        Holder::Role {
            role: Role::Developer,
            at: Some(d.d3),
        },
        json!({}),
    )
    .await;
    let mine = w.node_owned_by("thread", d.topic, "Mine", mel).await;
    w.bind_thread(mine, provider).await;
    let ask = MemberMove {
        user: mel,
        to: d.sales,
        role: None,
    };
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut tx = w.db.pool.begin().await.unwrap();
    let planned = ownership::plan_move(&mut tx, &principal, &ask, &Policies::default())
        .await
        .unwrap();
    assert_eq!(planned.numbers.unwrap().would_wait.len(), 1);
    // The plan took nothing with it: the member is where they were, inside the same transaction.
    let (domain,): (Id,) =
        sqlx::query_as("SELECT domain_id FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(w.root)
            .bind(mel)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(domain, d.d3);
    ownership::move_member(
        &mut tx,
        &principal,
        &ask,
        &Policies::default(),
        &Origin::default(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(domain_of(&w, mel).await, d.sales);
}
