//! Who may make a team org: as Orch ships, a system admin and somebody one invited to make one, each
//! invitation used once; anybody signed in, or system admins only, where the system says so. The
//! invitation is given and taken back from Users with its audit lines, and what the system says of
//! itself tells each caller whether they may make one now.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use admin_support::{assert_no_system_admin, lines_of};
use orch_api_contract::admin::{
    AdminUserList, AdminUserListQuery, AdminUserOrgInvitationGive, AdminUserOrgInvitationTakeBack,
};
use orch_api_contract::auth::{MetaGet, OrgCreation};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{Org, OrgCreate, OrgCreateBody};
use orch_api_core::testing::Reply;
use places::{Who, admin, person};
use serde_json::json;
use support::World;

/// Asks to make the team org `slug`, as `by`.
async fn make(world: &World, by: &Who, slug: &str) -> Reply<Org> {
    world
        .app
        .call(OrgCreate, &[])
        .token(&by.token)
        .body(&OrgCreateBody {
            name: slug.to_owned(),
            slug: slug.to_owned(),
        })
        .send()
        .await
}

/// `by` gives `to` an invitation to make a team org.
async fn give(world: &World, by: &Who, to: &Who) {
    world
        .app
        .call(AdminUserOrgInvitationGive, &[&to.id()])
        .token(&by.token)
        .send()
        .await
        .ok();
}

/// `by` takes back the invitation `from` holds.
async fn take_back(world: &World, by: &Who, from: &Who) {
    world
        .app
        .call(AdminUserOrgInvitationTakeBack, &[&from.id()])
        .token(&by.token)
        .send()
        .await
        .ok();
}

/// What the system says of who may make a team org, to `who` or to nobody signed in.
async fn said_to(world: &World, who: Option<&Who>) -> (OrgCreation, bool) {
    let mut call = world.app.call(MetaGet, &[]);
    if let Some(who) = who {
        call = call.token(&who.token);
    }
    let meta = call.send().await.ok();
    (meta.org_creation, meta.may_create_org)
}

/// Whether a person's row in Users says they hold an invitation.
async fn listed_as_invited(world: &World, root: &Who, username: &str) -> bool {
    world
        .app
        .call(AdminUserList, &[])
        .token(&root.token)
        .query(&AdminUserListQuery::default())
        .send()
        .await
        .ok()
        .items
        .iter()
        .find(|user| user.base.username == username)
        .unwrap()
        .org_invited_at
        .is_some()
}

#[tokio::test]
async fn as_shipped_a_system_admin_makes_a_team_org_and_somebody_invited_makes_one_once() {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let ada = person(&world, "ada").await;

    assert_eq!(
        said_to(&world, None).await,
        (OrgCreation::Invited, false),
        "nobody signed in makes nothing"
    );
    assert_eq!(
        said_to(&world, Some(&ada)).await,
        (OrgCreation::Invited, false)
    );
    assert_eq!(
        said_to(&world, Some(&root)).await,
        (OrgCreation::Invited, true)
    );

    let refused = make(&world, &ada, "adas").await.problem();
    assert_eq!(refused.code, ErrorCode::OrgCreationDisabled);
    assert_eq!(refused.status, 403);
    assert_eq!(refused.extra["org_creation"], json!("invited"));
    make(&world, &root, "roots").await.ok();

    give(&world, &root, &ada).await;
    let given = lines_of(&world, "org.invitation_given").await;
    assert_eq!(given.len(), 1);
    assert_eq!(given[0].actor_id, Some(root.id()));
    assert_eq!(given[0].object_id, Some(ada.id()), "it is about the person");
    assert!(listed_as_invited(&world, &root, "ada").await);
    assert_eq!(
        said_to(&world, Some(&ada)).await,
        (OrgCreation::Invited, true)
    );

    let made = make(&world, &ada, "adas").await.ok();
    assert_eq!(made.slug, "adas");
    let created = lines_of(&world, "node.created").await;
    let line = created
        .iter()
        .find(|line| line.details["kind"] == "org" && line.details["name"] == "adas")
        .unwrap();
    assert_eq!(
        line.details["invitation"]["given_by"],
        json!(root.id()),
        "the org a used invitation made names it"
    );
    let roots = created
        .iter()
        .find(|line| line.details["kind"] == "org" && line.details["name"] == "roots")
        .unwrap();
    assert!(
        roots.details.get("invitation").is_none(),
        "an admin uses none"
    );

    assert!(
        !listed_as_invited(&world, &root, "ada").await,
        "it was used"
    );
    assert_eq!(
        said_to(&world, Some(&ada)).await,
        (OrgCreation::Invited, false)
    );
    let again = make(&world, &ada, "again").await.problem();
    assert_eq!(
        again.code,
        ErrorCode::OrgCreationDisabled,
        "an invitation is used once"
    );
}

#[tokio::test]
async fn a_refused_making_keeps_the_invitation_and_two_at_once_make_one_org() {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let ada = person(&world, "ada").await;
    make(&world, &root, "taken").await.ok();
    give(&world, &root, &ada).await;

    let clash = make(&world, &ada, "taken").await.problem();
    assert_eq!(clash.code, ErrorCode::ConflictExists);
    assert!(
        listed_as_invited(&world, &root, "ada").await,
        "a making that failed used nothing"
    );

    let (first, second) = tokio::join!(make(&world, &ada, "one"), make(&world, &ada, "two"));
    let made = [first.status.is_success(), second.status.is_success()];
    assert_eq!(
        made.iter().filter(|made| **made).count(),
        1,
        "one invitation makes one org: {made:?}"
    );
    let refused = if made[0] {
        second.problem()
    } else {
        first.problem()
    };
    assert_eq!(refused.code, ErrorCode::OrgCreationDisabled);
}

#[tokio::test]
async fn a_second_invitation_changes_nothing_and_one_taken_back_lets_nobody_make_an_org() {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let ada = person(&world, "ada").await;

    give(&world, &root, &ada).await;
    give(&world, &root, &ada).await;
    assert_eq!(
        lines_of(&world, "org.invitation_given").await.len(),
        1,
        "the second changed nothing"
    );

    take_back(&world, &root, &ada).await;
    take_back(&world, &root, &ada).await;
    let taken = lines_of(&world, "org.invitation_taken_back").await;
    assert_eq!(taken.len(), 1, "a second has nothing to take");
    assert_eq!(taken[0].actor_id, Some(root.id()));
    assert_eq!(taken[0].object_id, Some(ada.id()));
    assert_eq!(taken[0].details["given_by"], json!(root.id()));

    assert!(!listed_as_invited(&world, &root, "ada").await);
    let refused = make(&world, &ada, "adas").await.problem();
    assert_eq!(refused.code, ErrorCode::OrgCreationDisabled);
}

#[tokio::test]
async fn anybody_signed_in_or_system_admins_only_as_the_system_says() {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let ada = person(&world, "ada").await;

    world.set("orgs.created_by", json!("anyone")).await;
    assert_eq!(
        said_to(&world, Some(&ada)).await,
        (OrgCreation::Anyone, true)
    );
    assert_eq!(said_to(&world, None).await, (OrgCreation::Anyone, false));
    make(&world, &ada, "first").await.ok();
    make(&world, &ada, "second").await.ok();

    world.set("orgs.created_by", json!("admins")).await;
    give(&world, &root, &ada).await;
    assert_eq!(
        said_to(&world, Some(&ada)).await,
        (OrgCreation::Admins, false),
        "an invitation counts only while invited people may make one"
    );
    let refused = make(&world, &ada, "third").await.problem();
    assert_eq!(refused.code, ErrorCode::OrgCreationDisabled);
    assert_eq!(refused.extra["org_creation"], json!("admins"));
    make(&world, &root, "rooted").await.ok();

    // The invitation was kept unused, for when the system lets invited people make one again.
    world.set("orgs.created_by", json!("invited")).await;
    make(&world, &ada, "third").await.ok();
}

#[tokio::test]
async fn only_a_system_admin_gives_or_takes_back_an_invitation() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bob = person(&world, "bob").await;

    for (what, refused) in [
        (
            "give",
            world
                .app
                .call(AdminUserOrgInvitationGive, &[&bob.id()])
                .token(&ada.token)
                .send()
                .await
                .problem(),
        ),
        (
            "take back",
            world
                .app
                .call(AdminUserOrgInvitationTakeBack, &[&bob.id()])
                .token(&ada.token)
                .send()
                .await
                .problem(),
        ),
    ] {
        assert_no_system_admin(&refused, what);
    }
    assert!(lines_of(&world, "org.invitation_given").await.is_empty());
    let refused = make(&world, &bob, "bobs").await.problem();
    assert_eq!(refused.code, ErrorCode::OrgCreationDisabled);
}
