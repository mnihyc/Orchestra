//! People from outside an org's tree: listed for its member managers with every way they reach it, and taken out by a plan read whole, which removes what names them alone and holds them out where a group still lets them in.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::directory::{
    OrgOutsiderList, OrgOutsiderListQuery, OrgOutsiderRemove, OrgOutsiderRemoveBody,
    OrgOutsiderRemovePreview, OutsidePerson, OutsideRemovalPlan,
};
use orch_api_contract::nodes::{Node, NodeGet};
use orch_api_core::testing::Reply;
use orch_db::queries::access::PageKey;
use orch_db::queries::access::testing as access_rows;
use orch_types::Id;
use places::{Who, admin, join, make_org, make_project, person};
use support::World;

/// An org with a project, a member, and two people from outside: Olga named on the project by
/// name and, with Pat, through a group of another org that holds a role there.
struct Acme {
    world: World,
    root: Who,
    ada: Who,
    olga: Who,
    pat: Who,
    acme: Id,
    web: Id,
}

async fn acme() -> Acme {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let acme = make_org(&world, &root, "Acme", "acme").await.base.id;
    let globex = make_org(&world, &root, "Globex", "globex").await.base.id;
    let web = make_project(&world, &root, acme, "Web").await.base.id;
    let ada = person(&world, "ada").await;
    let olga = person(&world, "olga").await;
    let pat = person(&world, "pat").await;
    join(&world, acme, acme, &ada, "developer", &root).await;
    join(&world, globex, globex, &olga, "developer", &root).await;
    join(&world, globex, globex, &pat, "developer", &root).await;
    let mut conn = world.conn().await;
    access_rows::add_entry(
        &mut conn,
        &PageKey::new("node", web, "live"),
        "any",
        "project.view",
        ("user", Some(olga.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    let contractors = access_rows::insert_group(&mut conn, globex, "Contractors", root.id())
        .await
        .unwrap();
    access_rows::add_to_group(&mut conn, contractors, olga.id())
        .await
        .unwrap();
    access_rows::add_to_group(&mut conn, contractors, pat.id())
        .await
        .unwrap();
    access_rows::give_role(
        &mut conn,
        web,
        "group",
        contractors,
        "viewer",
        root.id(),
        None,
    )
    .await
    .unwrap();
    Acme {
        world,
        root,
        ada,
        olga,
        pat,
        acme,
        web,
    }
}

async fn outsiders(at: &Acme, who: &Who) -> Reply<Page<OutsidePerson>> {
    at.world
        .app
        .call(OrgOutsiderList, &[&at.acme])
        .query(&OrgOutsiderListQuery::default())
        .token(&who.token)
        .send()
        .await
}

async fn preview(at: &Acme, of: &Who) -> OutsideRemovalPlan {
    at.world
        .app
        .call(OrgOutsiderRemovePreview, &[&at.acme, &of.id()])
        .token(&at.root.token)
        .send()
        .await
        .ok()
}

async fn confirm(
    at: &Acme,
    of: &Who,
    plan: &OutsideRemovalPlan,
) -> Reply<orch_api_contract::directory::OutsideRemovalResult> {
    at.world
        .app
        .call(OrgOutsiderRemove, &[&at.acme, &of.id()])
        .token(&at.root.token)
        .body(&OrgOutsiderRemoveBody {
            plan_id: plan.id,
            reviewed_fingerprint: plan.reviewed_fingerprint.clone().unwrap_or_default(),
            reason: "the contract ended".to_owned(),
        })
        .send()
        .await
}

async fn read(at: &Acme, who: &Who, id: Id) -> Reply<Node> {
    at.world
        .app
        .call(NodeGet, &[&id])
        .token(&who.token)
        .send()
        .await
}

#[tokio::test]
async fn whoever_manages_the_members_sees_each_outsider_with_every_way_they_reach_the_org() {
    let at = acme().await;
    let listed = outsiders(&at, &at.root).await.ok();
    let names: Vec<&str> = listed
        .items
        .iter()
        .map(|p| p.user.username.as_str())
        .collect();
    assert_eq!(names, ["olga", "pat"], "members are not from outside");
    let olga = &listed.items[0];
    assert_eq!(olga.sources.len(), 2);
    let direct = olga.sources.iter().find(|s| s.direct).unwrap();
    assert_eq!(
        (direct.object.id, direct.row.as_str()),
        (at.web, "project.view")
    );
    assert!(direct.can_change);
    let through = olga.sources.iter().find(|s| !s.direct).unwrap();
    assert_eq!(through.holder["kind"], "group");
    assert_eq!(through.holder["name"], "Contractors");
    assert!(!olga.suspended);

    // A member who does not manage members reads nobody from outside.
    let refused = outsiders(&at, &at.ada).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn taking_an_outsider_out_removes_what_names_them_and_holds_them_out_where_a_group_still_lets_them_in()
 {
    let at = acme().await;
    read(&at, &at.olga, at.web).await.ok();
    let plan = preview(&at, &at.olga).await;
    assert_eq!(plan.removes.len(), 2);
    assert!(plan.suspension_needed, "the group still lets her in");
    assert!(plan.next_cursor.is_none() && plan.reviewed_fingerprint.is_some());
    assert!(plan.missing.is_empty());

    let done = confirm(&at, &at.olga, &plan).await.ok();
    assert_eq!((done.removed_entries, done.removed_roles), (1, 0));
    assert!(done.suspension.is_some());
    assert_eq!(
        read(&at, &at.olga, at.web).await.problem().code,
        ErrorCode::OrgSuspended
    );
    // Nobody else in the group loses anything, and the group itself is untouched.
    read(&at, &at.pat, at.web).await.ok();
    let listed = outsiders(&at, &at.root).await.ok();
    let olga = listed
        .items
        .iter()
        .find(|p| p.user.id == at.olga.id())
        .unwrap();
    assert!(olga.suspended);
    assert_eq!(olga.sources.len(), 1, "only the group's role is left");
}

#[tokio::test]
async fn a_plan_whose_ways_changed_or_was_not_read_whole_changes_nothing() {
    let at = acme().await;
    let plan = preview(&at, &at.olga).await;
    let mut forged = plan.clone();
    forged.reviewed_fingerprint = Some("0".repeat(64));
    let refused = confirm(&at, &at.olga, &forged).await.problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);

    // A way added after the review makes the plan stale, whole.
    let mut conn = at.world.conn().await;
    access_rows::add_entry(
        &mut conn,
        &PageKey::new("node", at.acme, "live"),
        "any",
        "org.view",
        ("user", Some(at.olga.id()), None),
        None,
        at.world.now(),
    )
    .await
    .unwrap();
    let stale = confirm(&at, &at.olga, &plan).await.problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    read(&at, &at.olga, at.web).await.ok();
    let listed = outsiders(&at, &at.root).await.ok();
    let olga = listed
        .items
        .iter()
        .find(|p| p.user.id == at.olga.id())
        .unwrap();
    assert!(!olga.suspended);
    assert_eq!(olga.sources.len(), 3);
}
