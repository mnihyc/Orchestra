//! The card a reference's chip opens: the thing and the part it names as the caller may read them, the part numbered as it is now, and for a caller who may not read the thing nothing at all, not even that it is there.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::{Desk, paragraph};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DocumentOp, InlineRefKind, RefPart, ReferencePreview, ReferencePreviewGet,
    ReferencePreviewGetQuery,
};
use orch_api_contract::nodes::NodeKind;
use orch_types::{Id, new_id};

fn ask(kind: InlineRefKind, id: Id) -> ReferencePreviewGetQuery {
    ReferencePreviewGetQuery {
        kind,
        id,
        block: None,
        turn: None,
        message: None,
    }
}

async fn card(
    desk: &Desk,
    who: &places::Who,
    query: &ReferencePreviewGetQuery,
) -> ReferencePreview {
    desk.world
        .app
        .call(ReferencePreviewGet, &[])
        .token(&who.token)
        .query(query)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_readers_card_shows_the_document_where_it_lies_its_line_and_the_block_it_names_numbered_as_it_is_now()
 {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Rollout plan").await;
    let blocks = desk
        .paragraphs(
            &desk.ada,
            made.base.id,
            &["Build it.", "Ship it behind the flag."],
        )
        .await;
    let second = blocks[1].id;
    let vera = desk.member("vera", "viewer").await;
    let query = ReferencePreviewGetQuery {
        block: Some(second),
        ..ask(InlineRefKind::Document, made.base.id)
    };
    let shown = card(&desk, &vera, &query).await;
    assert_eq!(
        (shown.kind, shown.id, shown.title.as_str(), shown.node_kind),
        (InlineRefKind::Document, made.base.id, "Rollout plan", None)
    );
    assert_eq!(
        shown
            .place
            .as_ref()
            .map(|place| (place.id, place.name.as_deref())),
        Some((desk.topic, Some("Plans"))),
        "where it lies"
    );
    assert_eq!(
        shown.description, "Build it.",
        "a document nobody described is shown by the first line of its first paragraph"
    );
    assert!(shown.updated_at.is_some());
    let part = shown.part.expect("the block is there");
    assert_eq!(
        (part.part, part.number, part.anchor, part.text.as_str()),
        (
            RefPart::Block(second),
            2,
            Some(second),
            "Ship it behind the flag."
        )
    );
    // A paragraph written above it moves nothing: the same block, now the third.
    desk.write(
        &desk.ada,
        made.base.id,
        vec![DocumentOp::Insert {
            block: paragraph("First of all."),
            parent_id: None,
            after_id: None,
        }],
    )
    .await
    .ok();
    let again = card(&desk, &vera, &query).await;
    let part = again.part.expect("the block is still there");
    assert_eq!((part.number, part.anchor), (3, Some(second)));
    // A block that is gone is no part; the document is still shown.
    let gone = ReferencePreviewGetQuery {
        block: Some(new_id()),
        ..ask(InlineRefKind::Document, made.base.id)
    };
    assert!(card(&desk, &vera, &gone).await.part.is_none());
}

#[tokio::test]
async fn a_places_card_names_its_kind_and_the_place_it_lies_in() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let shown = card(&desk, &vera, &ask(InlineRefKind::Node, desk.topic)).await;
    assert_eq!(
        (shown.title.as_str(), shown.node_kind),
        ("Plans", Some(NodeKind::Topic))
    );
    assert_eq!(shown.place.map(|place| place.id), Some(desk.project));
    assert!(shown.part.is_none());
}

#[tokio::test]
async fn a_known_unreadable_reference_has_a_metadata_free_card_and_wrong_parts_are_refused() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Rollout plan").await;
    desk.paragraphs(&desk.ada, made.base.id, &["Ship it."])
        .await;
    let sam = desk.stranger("sam").await;
    for query in [
        ask(InlineRefKind::Document, made.base.id),
        ask(InlineRefKind::Node, desk.topic),
    ] {
        let shown = card(&desk, &sam, &query).await;
        assert_eq!(shown.title, "");
        assert_eq!(shown.description, "");
        assert!(
            shown.place.is_none()
                && shown.updated_at.is_none()
                && shown.updated_by.is_none()
                && shown.part.is_none()
                && shown.file.is_none()
        );
        assert!(shown.unavailable.is_some());
        let wire = serde_json::to_string(&shown).unwrap();
        assert!(
            !wire.contains("Rollout") && !wire.contains("Plans"),
            "{wire}"
        );
    }
    for query in [
        ask(InlineRefKind::Chat, new_id()),
        ask(InlineRefKind::Document, new_id()),
    ] {
        let problem = desk
            .world
            .app
            .call(ReferencePreviewGet, &[])
            .token(&sam.token)
            .query(&query)
            .send()
            .await
            .problem();
        assert_eq!(
            (problem.status, problem.code),
            (404, ErrorCode::NotFound),
            "{query:?}: the same answer whether it exists or not"
        );
        assert!(!problem.detail.contains("Rollout") && !problem.detail.contains("Plans"));
    }
    let wrong = desk
        .world
        .app
        .call(ReferencePreviewGet, &[])
        .token(&desk.ada.token)
        .query(&ReferencePreviewGetQuery {
            turn: Some(1),
            ..ask(InlineRefKind::Document, made.base.id)
        })
        .send()
        .await
        .problem();
    assert_eq!(
        (wrong.status, wrong.code),
        (400, ErrorCode::RequestInvalid),
        "a document has blocks, not turns"
    );
}
