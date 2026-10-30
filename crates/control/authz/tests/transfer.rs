//! Handing a thing over on a real database: to whom it may go, what the person who was first keeps, the line it leaves, and that a thing is never left without an owner.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::roles::Role;
use orch_authz::Caller;
use orch_authz::pages::{ChangeError, Target};
use orch_authz::things::Thing;
use orch_authz::transfer::{Handed, TransferError, hand_over};
use orch_authz::usable::ResourcePolicy;
use orch_db::audit::Origin;
use orch_db::queries::identity::testing::suspend;
use orch_types::Id;
use serde_json::json;
use support::World;

async fn hand(w: &World, by: Id, to: Id, keep_previous: bool) -> Result<Handed, TransferError> {
    hand_as(w, Caller::session(by), to, keep_previous).await
}

async fn hand_as(
    w: &World,
    caller: Caller,
    to: Id,
    keep_previous: bool,
) -> Result<Handed, TransferError> {
    let principal = w.principal(caller).await;
    let mut tx = w.db.pool.begin().await.unwrap();
    let handed = hand_over(
        &mut tx,
        &principal,
        Thing::Node(w.topic),
        to,
        keep_previous,
        &ResourcePolicy {
            owners_name_outsiders: true,
            shared_subscriptions: None,
        },
        &Origin::default(),
    )
    .await;
    if handed.is_ok() {
        tx.commit().await.unwrap();
    }
    handed
}

async fn owners(w: &World) -> (Option<Id>, Vec<Id>) {
    let page = w.read(w.boss, Target::Live(w.topic)).await;
    let users = page
        .owners
        .iter()
        .filter_map(|entry| match entry.holder {
            orch_access_contract::pages::Holder::User { id } => Some(id),
            _ => None,
        })
        .collect();
    (page.first_owner, users)
}

#[tokio::test]
async fn the_first_place_goes_to_the_other_person_and_the_one_who_was_first_stays_unless_taken_out()
{
    let w = World::new().await;
    let rae = w.member("rae").await;
    let before = w.epoch().await;
    let handed = hand(&w, w.boss, rae, true).await.unwrap();
    assert!(handed.changed && handed.previous_stays);
    let (first, row) = owners(&w).await;
    assert_eq!(first, Some(rae));
    assert_eq!(row.first(), Some(&rae), "first in the row from now on");
    assert!(
        row.contains(&w.boss),
        "the person who was first stays an owner"
    );
    assert!(w.epoch().await > before);
    let line = w
        .audit()
        .await
        .into_iter()
        .find(|row| row.action == "node.transferred")
        .expect("the passing on is audited");
    assert!(
        line.history,
        "and is a line of the history of what was handed over"
    );
    assert_eq!(line.details["to"], json!(rae));
    // Handing it back, and taking the other out in the same step.
    let back = hand(&w, rae, w.boss, false).await.unwrap();
    assert!(back.changed && !back.previous_stays);
    let (first, row) = owners(&w).await;
    assert_eq!(first, Some(w.boss));
    assert_eq!(row, [w.boss], "a thing is never left without an owner");
}

#[tokio::test]
async fn it_goes_only_to_an_active_member_and_only_an_owner_or_an_admin_hands_it_over() {
    let w = World::new().await;
    let rae = w.member("rae").await;
    let stranger = w.user("sam").await;
    let suspended = w.member("sue").await;
    {
        let mut conn = w.conn().await;
        suspend(&mut conn, suspended).await.unwrap();
    }
    for (to, why) in [
        (stranger, "of no org"),
        (suspended, "suspended"),
        (Id::nil(), "nobody"),
    ] {
        let refused = hand(&w, w.boss, to, true).await;
        assert!(
            matches!(
                refused,
                Err(TransferError::NotAnActiveMember | TransferError::NoSuchUser)
            ),
            "{why}: {refused:?}"
        );
    }
    // The caller is judged before the person it goes to: even a stranger is a question for owners.
    w.give(w.root, rae, Role::Viewer).await;
    let by_viewer = hand(&w, rae, stranger, true).await;
    assert!(
        matches!(&by_viewer, Err(TransferError::Change(error)) if matches!(**error, ChangeError::Denied(_))),
        "{by_viewer:?}"
    );
    let by_admin = {
        let dee = w.member("dee").await;
        w.give(w.project, dee, Role::Admin).await;
        let ordinary = hand_as(&w, Caller::session(dee).with_admin_mode(false), rae, false).await;
        assert!(
            matches!(&ordinary, Err(TransferError::Change(error)) if matches!(**error, ChangeError::Denied(_))),
            "Off gives no administrative ownership authority"
        );
        hand_as(&w, Caller::session(dee).with_admin_mode(true), rae, false).await
    };
    assert!(
        by_admin.unwrap().changed,
        "an admin of the place hands it over"
    );
    // To the person who is first already: nothing changes, and nothing is audited.
    let lines = w.audit().await.len();
    let same = hand(&w, rae, rae, true).await.unwrap();
    assert!(!same.changed);
    assert_eq!(w.audit().await.len(), lines);
}
