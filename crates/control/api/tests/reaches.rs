//! A role held above, a name on a place's own page and a name on one document, told by the effective-access and people requests: what each holds, through which entry, and that neither a name on a place nor a name on a document reaches what lies inside the place or beside another document.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    AccessChoice, AccessThrough, EffectiveAccess, EffectiveAccessGet, EffectiveAccessGetQuery,
    Holder, HolderInput, PageChange, PageChangeName, PermissionPageUpdate, PermissionPeopleList,
    PermissionPeopleListQuery,
};
use orch_db::queries::access::testing::insert_member;
use orch_types::Id;
use places::{Who, person};

/// A member of Acme's tree who holds no role anywhere, so that only a name on a page opens anything.
async fn member_without_a_role(desk: &Desk, name: &str) -> Who {
    let who = person(&desk.world, name).await;
    let mut conn = desk.world.conn().await;
    insert_member(&mut conn, desk.org, who.id(), desk.org)
        .await
        .unwrap();
    who
}

async fn name_on(desk: &Desk, plural: &str, id: Id, user: Id, choice: AccessChoice) {
    desk.world
        .app
        .call(PermissionPageUpdate, &[&plural, &id])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: user }],
                choice,
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

async fn holds(desk: &Desk, plural: &str, id: Id, user: &Who) -> EffectiveAccess {
    desk.world
        .app
        .call(EffectiveAccessGet, &[&plural, &id])
        .token(&desk.ada.token)
        .query(&EffectiveAccessGetQuery { user_id: user.id() })
        .send()
        .await
        .ok()
}

fn words(held: &EffectiveAccess) -> Vec<&str> {
    held.permissions
        .iter()
        .map(|each| each.permission.as_str())
        .collect()
}

/// Whether `permission` is held through an entry on the page itself that names `user`.
fn through_the_page_itself(held: &EffectiveAccess, permission: &str, user: Id) -> bool {
    held.permissions
        .iter()
        .find(|each| each.permission == permission)
        .is_some_and(|each| {
            each.through.iter().any(|way| {
                matches!(
                    way,
                    AccessThrough::Entry { entry, from: None, .. }
                        if matches!(&entry.holder, Holder::User { user: named } if named.id == user)
                )
            })
        })
}

#[tokio::test]
async fn a_name_on_a_places_own_page_holds_the_place_through_that_page_and_nothing_inside_it() {
    let desk = Desk::new().await;
    let pia = member_without_a_role(&desk, "pia").await;
    let beside_the_project = desk
        .make_at(&desk.ada, desk.project, desk::body("Handbook"))
        .await
        .ok()
        .base
        .id;
    let inside = desk.make(&desk.ada, "Minutes").await.base.id;
    name_on(
        &desk,
        "nodes",
        desk.project,
        pia.id(),
        AccessChoice::FullView,
    )
    .await;
    let on_the_place = holds(&desk, "nodes", desk.project, &pia).await;
    assert!(words(&on_the_place).contains(&"project.view"));
    assert!(
        through_the_page_itself(&on_the_place, "project.view", pia.id()),
        "{on_the_place:?}"
    );
    // The documents written beside the place follow its own page.
    let handbook = holds(&desk, "documents", beside_the_project, &pia).await;
    assert!(words(&handbook).contains(&"doc.view"), "{handbook:?}");
    // And nothing that lies inside the place: not the topic, nor a document beside the topic.
    let topic = holds(&desk, "nodes", desk.topic, &pia).await;
    assert!(words(&topic).is_empty(), "{topic:?}");
    let minutes = holds(&desk, "documents", inside, &pia).await;
    assert!(words(&minutes).is_empty(), "{minutes:?}");
}

#[tokio::test]
async fn a_name_on_one_document_holds_that_document_through_its_page_and_neither_its_node_nor_another_document()
 {
    let desk = Desk::new().await;
    let pia = member_without_a_role(&desk, "pia").await;
    let shared = desk.make(&desk.ada, "Shared").await.base.id;
    let beside = desk.make(&desk.ada, "Beside").await.base.id;
    name_on(&desk, "documents", shared, pia.id(), AccessChoice::FullView).await;
    let on_it = holds(&desk, "documents", shared, &pia).await;
    assert!(words(&on_it).contains(&"doc.view"));
    assert!(
        through_the_page_itself(&on_it, "doc.view", pia.id()),
        "{on_it:?}"
    );
    let another = holds(&desk, "documents", beside, &pia).await;
    assert!(words(&another).is_empty(), "{another:?}");
    let topic = holds(&desk, "nodes", desk.topic, &pia).await;
    assert!(words(&topic).is_empty(), "{topic:?}");
}

async fn people_behind(desk: &Desk, plural: &str, id: Id, holder: String) -> Vec<String> {
    let mut names: Vec<String> = desk
        .world
        .app
        .call(PermissionPeopleList, &[&plural, &id])
        .token(&desk.ada.token)
        .query(&PermissionPeopleListQuery {
            holder,
            limit: Some(200),
            cursor: None,
        })
        .send()
        .await
        .ok()
        .items
        .into_iter()
        .map(|user| user.username)
        .collect();
    names.sort_unstable();
    names
}

#[tokio::test]
async fn the_people_behind_a_role_held_above_are_listed_on_the_page_of_what_lies_inside() {
    let desk = Desk::new().await;
    desk.member("rae", "viewer").await;
    let dev = desk.member("dev", "viewer").await;
    places::give_role(&desk.world, desk.project, &dev, "developer", &desk.ada).await;
    let minutes = desk.make(&desk.ada, "Minutes").await.base.id;
    let has = |names: &[String], name: &str| names.iter().any(|each| each == name);
    // The role as it is held at the topic, whether it was given there or in a place above it: the
    // page of the topic and the page of a document beside it show it, and list who holds it.
    for (plural, id) in [("nodes", desk.topic), ("documents", minutes)] {
        let viewers = people_behind(&desk, plural, id, format!("role:{}:viewer", desk.topic)).await;
        assert!(
            has(&viewers, "rae") && has(&viewers, "dev"),
            "{plural}: {viewers:?}"
        );
        let developers =
            people_behind(&desk, plural, id, format!("role:{}:developer", desk.topic)).await;
        assert!(
            has(&developers, "dev") && !has(&developers, "rae"),
            "{plural}: {developers:?}"
        );
    }
}
