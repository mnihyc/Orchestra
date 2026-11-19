//! A place's icon in a colour of its own: written as `#rrggbb` by whoever manages the place, kept in small letters,
//! shown in its view and in every tree, taken away by null, and replaced by one of the twelve (and the reverse); a
//! request that gives both, or a colour that is no `#rrggbb`, is refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{
    Node, NodeUpdate, NodeUpdateBody, OrgTreeGet, OrgTreeGetQuery, OrgTreeGetReply,
};
use orch_api_contract::search::{NameSearchList, NameSearchListQuery};
use orch_types::{Id, Revision};
use places::{Who, join, make_org, make_project, person};
use support::World;

async fn tree(world: &World, who: &Who, org: Id) -> OrgTreeGetReply {
    world
        .app
        .call(OrgTreeGet, &[&org])
        .token(&who.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok()
}

fn body(
    color: Option<Option<u32>>,
    custom_color: Option<Option<String>>,
    expected_revision: Revision,
) -> NodeUpdateBody {
    NodeUpdateBody {
        name: None,
        description: None,
        color,
        custom_color,
        readme_document_id: None,
        expected_revision,
    }
}

#[tokio::test]
async fn a_colour_of_its_own_is_shown_everywhere_and_one_of_the_twelve_replaces_it() {
    let world = World::new().await;
    let maker = person(&world, "ada").await;
    let reader = person(&world, "bea").await;
    let org = make_org(&world, &maker, "Office", "office").await.base.id;
    let project = make_project(&world, &maker, org, "Work").await;
    join(&world, org, org, &reader, "viewer", &maker).await;
    let change = |who: &Who, body: NodeUpdateBody| {
        world
            .app
            .call(NodeUpdate, &[&project.base.id])
            .token(&who.token)
            .body(&body)
            .send()
    };

    // A reader may not give it one, and nothing that is no #rrggbb is taken.
    let denied = change(
        &reader,
        body(None, Some(Some("#3a7bd5".into())), project.base.revision),
    )
    .await
    .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    for wrong in ["3a7bd5", "#3a7bd", "#3a7bd5ff", "#ggggggg", "blue"] {
        let refused = change(
            &maker,
            body(None, Some(Some(wrong.into())), project.base.revision),
        )
        .await
        .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{wrong}");
        assert_eq!(refused.extra["errors"][0]["path"], "custom_color");
    }
    let both = change(
        &maker,
        body(
            Some(Some(3)),
            Some(Some("#3a7bd5".into())),
            project.base.revision,
        ),
    )
    .await
    .problem();
    assert_eq!(both.code, ErrorCode::RequestInvalid);

    // Given in capitals, it is kept in small letters, in the view and in every reader's tree.
    let Node::Project(own) = change(
        &maker,
        body(None, Some(Some("#3A7BD5".into())), project.base.revision),
    )
    .await
    .ok() else {
        panic!("a project")
    };
    assert_eq!(own.base.custom_color.as_deref(), Some("#3a7bd5"));
    assert_eq!(own.base.color, None);
    let hits = world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&NameSearchListQuery {
            starred: None,
            participating: None,
            q: Some("Work".to_owned()),
            kinds: Some("project".to_owned()),
            org_id: Some(org),
            limit: None,
        })
        .send()
        .await
        .ok();
    let hit = hits.iter().find(|hit| hit.id == project.base.id).unwrap();
    assert_eq!(hit.custom_color.as_deref(), Some("#3a7bd5"));
    assert_eq!(hit.color, None);
    let reader_tree = tree(&world, &reader, org).await;
    let row = reader_tree
        .readable()
        .into_iter()
        .find(|row| row.id == project.base.id)
        .unwrap();
    assert_eq!(row.custom_color.as_deref(), Some("#3a7bd5"));

    // One of the twelve takes its place, and a colour of its own the place of the twelve's.
    let Node::Project(palette) = change(&maker, body(Some(Some(5)), None, own.base.revision))
        .await
        .ok()
    else {
        panic!("a project")
    };
    assert_eq!(palette.base.color, Some(5));
    assert_eq!(palette.base.custom_color, None);
    let Node::Project(again) = change(
        &maker,
        body(None, Some(Some("#112233".into())), palette.base.revision),
    )
    .await
    .ok() else {
        panic!("a project")
    };
    assert_eq!(again.base.color, None);
    assert_eq!(again.base.custom_color.as_deref(), Some("#112233"));

    // Null takes it away: the colour of its kind again.
    let Node::Project(plain) = change(&maker, body(None, Some(None), again.base.revision))
        .await
        .ok()
    else {
        panic!("a project")
    };
    assert_eq!(plain.base.color, None);
    assert_eq!(plain.base.custom_color, None);
}
