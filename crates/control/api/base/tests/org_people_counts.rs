//! An org's cards count active domain and inherited-role people once without changing its domain-member facts.

mod places;
mod support;

use orch_api_contract::nodes::{Node, NodeChildList, NodeChildListQuery};
use orch_db::queries::access::testing as access_rows;
use places::{give_role, join, make_department, make_org, person, read};
use support::World;

#[tokio::test]
async fn a_department_with_no_domain_members_still_counts_inherited_people_and_deduplicates_roles()
-> Result<(), Box<dyn std::error::Error>> {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let inherited = person(&world, "inherited").await;
    let sibling_member = person(&world, "sibling_member").await;
    let root = make_org(&world, &owner, "Acme", "acme").await;
    let department = make_department(&world, &owner, root.base.id, "Research", "research").await;
    let sibling = make_department(&world, &owner, root.base.id, "Operations", "operations").await;
    join(
        &world,
        root.base.id,
        root.base.id,
        &inherited,
        "developer",
        &owner,
    )
    .await;
    join(
        &world,
        root.base.id,
        sibling.base.id,
        &sibling_member,
        "viewer",
        &owner,
    )
    .await;
    // The same person also has a direct role; they still count only once.
    give_role(&world, department.base.id, &inherited, "interactor", &owner).await;
    let Node::Org(shown) = read(&world, &owner, department.base.id).await else {
        return Err("an org is returned".into());
    };
    assert_eq!(
        shown.member_count, 0,
        "domain membership keeps its separate meaning"
    );
    assert_eq!(
        shown.people_count,
        Some(2),
        "the owner and inherited developer work here, not the sibling's viewer"
    );
    let rows = world
        .app
        .call(NodeChildList, &[&root.base.id])
        .token(&owner.token)
        .query(&NodeChildListQuery::default())
        .send()
        .await
        .ok();
    let listed = rows
        .items
        .iter()
        .filter_map(orch_api_contract::common::Listed::as_visible)
        .find_map(|node| match node {
            Node::Org(org) if org.base.id == department.base.id => Some(org),
            _ => None,
        })
        .ok_or("the collection includes its department")?;
    assert_eq!(
        listed.people_count, shown.people_count,
        "the collection and detail share the same counted projection"
    );

    let local = person(&world, "local").await;
    join(
        &world,
        root.base.id,
        department.base.id,
        &local,
        "viewer",
        &owner,
    )
    .await;
    let Node::Org(joined) = read(&world, &owner, department.base.id).await else {
        return Err("an org is returned".into());
    };
    assert_eq!(joined.member_count, 1);
    assert_eq!(
        joined.people_count,
        Some(3),
        "domain members and inherited roles are one union"
    );
    {
        let mut conn = world.conn().await;
        access_rows::suspend_in(&mut conn, department.base.id, inherited.id(), owner.id()).await?;
    }
    let Node::Org(suspended) = read(&world, &owner, department.base.id).await else {
        return Err("an org is returned".into());
    };
    assert_eq!(suspended.member_count, 1);
    assert_eq!(
        suspended.people_count,
        Some(2),
        "the native suspension cut removes the inherited person"
    );
    Ok(())
}
