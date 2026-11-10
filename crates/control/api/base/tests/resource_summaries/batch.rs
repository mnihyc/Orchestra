//! The real native batch reads only stored paragraphs under its own current source namespace.
use super::*;
use orch_api_contract::summaries::{SummaryBatchBody, SummaryBatchGet};

#[tokio::test]
async fn node_openings_carry_the_same_checked_chat_views_without_paid_work() {
    use orch_api_contract::chats::{NodeChatOpeningGet, NodeChatOpeningGetQuery};
    let desk = Desk::new().await;
    let opening = desk
        .world
        .app
        .call(NodeChatOpeningGet, &[&desk.topic])
        .token(&desk.ada.token)
        .query(&NodeChatOpeningGetQuery::default())
        .send()
        .await
        .ok();
    let chat = opening.opening.as_ref().unwrap().sync.chat.base.id;
    let range = opening.summary_range.unwrap();
    assert_eq!(opening.summaries.len(), 2);
    for view in &opening.summaries {
        let query = if view.target == (SummaryTarget::Chat { id: chat }) {
            SummaryReadQuery {
                after: Some(range.after),
                through: Some(range.through),
            }
        } else {
            SummaryReadQuery::default()
        };
        let kind = if matches!(view.target, SummaryTarget::Chat { .. }) {
            "chat"
        } else {
            "chat_title"
        };
        let single = desk
            .world
            .app
            .call(SummaryGet, &[&kind, &chat])
            .token(&desk.ada.token)
            .query(&query)
            .send()
            .await
            .ok();
        assert_eq!(*view, single);
    }
    let work: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM summary_calls)+(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *desk.world.conn().await).await.unwrap();
    assert_eq!(work, 0);
}

#[tokio::test]
async fn full_batches_keep_the_singleton_state_and_refuse_unrelated_chat_ranges() {
    let desk = Desk::new().await;
    let target = SummaryTarget::Document {
        id: desk.make(&desk.ada, "A full summary source").await.base.id,
    };
    cache(
        &desk,
        target,
        None,
        "The actual retained paragraph.",
        json!([target]),
    )
    .await;
    let single = desk
        .world
        .app
        .call(SummaryGet, &[&"document", &target.id()])
        .token(&desk.ada.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    let batch = desk
        .world
        .app
        .call(SummaryBatchGet, &[])
        .token(&desk.ada.token)
        .body(&SummaryBatchBody {
            targets: vec![target],
            full: true,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(batch.unavailable.is_empty());
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].full.as_ref(), Some(&single));
    assert_eq!(batch.items[0].text, single.text);
    let invalid = desk
        .world
        .app
        .call(SummaryBatchGet, &[])
        .token(&desk.ada.token)
        .body(&SummaryBatchBody {
            targets: vec![target],
            full: true,
            ranges: [(
                target.id(),
                orch_api_contract::summaries::SummaryChatRange {
                    after: 0,
                    through: 7,
                },
            )]
            .into(),
            ..Default::default()
        })
        .send()
        .await;
    assert_eq!(invalid.problem().code, ErrorCode::RequestInvalid);
    let work: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM summary_calls)+(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *desk.world.conn().await).await.unwrap();
    assert_eq!(work, 0, "full screen reads never admit paid work");
}

#[tokio::test]
async fn a_batch_key_cannot_borrow_a_missing_governors_read_from_its_primary_target() {
    let desk = Desk::new().await;
    let doc = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Scoped contributor").await.base.id,
    };
    let topic = SummaryTarget::Topic { id: desk.topic };
    cache(
        &desk,
        topic,
        None,
        "Private derived canary.",
        json!([topic, doc]),
    )
    .await;
    let scope:orch_access_contract::key_scope::KeyScope=serde_json::from_value(json!({"rules":[{
        "target":{"kind":"topic","id":desk.topic,"reach":"self"},"resources":["topic"],"allow":[{"operation":"SummaryBatchGet","actions":["read"]}]
    }]})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: desk.ada.id(),
            name: "One batch source",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    for full in [false, true] {
        let reply = desk
            .world
            .app
            .call(SummaryBatchGet, &[])
            .token(&token)
            .body(&SummaryBatchBody {
                targets: vec![topic],
                full,
                offset: 0,
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(reply.items.len(), 1);
        assert_eq!(reply.items[0].text, None);
        assert!(!serde_json::to_string(&reply).unwrap().contains("canary"));
    }
}

#[tokio::test]
async fn mixed_batch_keeps_private_viewers_and_every_governor_without_paid_work() {
    let desk = Desk::new().await;
    let reader = desk.member("batch-reader", "viewer").await;
    let good = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Readable").await.base.id,
    };
    let derived = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Public container").await.base.id,
    };
    let private = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Private source canary").await.base.id,
    };
    access::testing::set_row(
        &mut *desk.world.conn().await,
        &access::PageKey::new("document", private.id(), "live"),
        "any",
        "doc.view",
        false,
        false,
    )
    .await
    .unwrap();
    cache(&desk, good, None, "Public paragraph.", json!([good])).await;
    cache(
        &desk,
        good,
        Some(desk.ada.id()),
        "Another viewer private paragraph canary.",
        json!([good]),
    )
    .await;
    cache(
        &desk,
        derived,
        None,
        "Hidden derived paragraph canary.",
        json!([derived, private]),
    )
    .await;
    let missing = SummaryTarget::Document { id: new_id() };
    for full in [false, true] {
        let reply = desk
            .world
            .app
            .call(SummaryBatchGet, &[])
            .token(&reader.token)
            .body(&SummaryBatchBody {
                targets: vec![good, derived, private, missing, good],
                full,
                offset: 0,
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(reply.items.len(), 2);
        assert!(reply.items.iter().all(|item| item.full.is_some() == full));
        assert_eq!(
            reply
                .items
                .iter()
                .find(|item| item.target == good)
                .and_then(|item| item.text.as_deref()),
            Some("Public paragraph.")
        );
        assert_eq!(
            reply
                .items
                .iter()
                .find(|item| item.target == derived)
                .and_then(|item| item.text.as_deref()),
            None
        );
        assert_eq!(reply.unavailable, vec![private, missing]);
        let encoded = serde_json::to_string(&reply).unwrap();
        assert!(!encoded.contains("canary"));
        let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM resource_summary_requests),(SELECT count(*) FROM summary_calls),(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *desk.world.conn().await).await.unwrap();
        assert_eq!(counts, (0, 0, 0));
    }
}

#[tokio::test]
async fn batch_windows_continue_and_bind_only_actual_current_read_sources() {
    let desk = Desk::new().await;
    let one = SummaryTarget::Document {
        id: desk.make(&desk.ada, "First").await.base.id,
    };
    let two = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Second").await.base.id,
    };
    let three = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Third").await.base.id,
    };
    let value = json!(2);
    orch_db::queries::settings::put(
        &mut *desk.world.conn().await,
        None,
        "summary.read_batch_size",
        &orch_db::queries::settings::Stored {
            value: &value,
            locked: false,
            by: Some(desk.ada.id()),
            at: desk.world.now(),
        },
        None,
    )
    .await
    .unwrap();
    let first = desk
        .world
        .app
        .call(SummaryBatchGet, &[])
        .token(&desk.ada.token)
        .body(&SummaryBatchBody {
            targets: vec![one, two, three],
            offset: 0,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.target)
            .collect::<Vec<_>>(),
        vec![one, two]
    );
    assert_eq!(first.next_offset, Some(2));
    let second = desk
        .world
        .app
        .call(SummaryBatchGet, &[])
        .token(&desk.ada.token)
        .body(&SummaryBatchBody {
            targets: vec![one, two, three],
            offset: 2,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        second
            .items
            .iter()
            .map(|item| item.target)
            .collect::<Vec<_>>(),
        vec![three]
    );
    assert_eq!(second.next_offset, None);
}
