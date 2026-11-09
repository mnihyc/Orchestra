//! The real native batch reads only stored paragraphs under its own current source namespace.
use super::*;
use orch_api_contract::summaries::{SummaryBatchBody, SummaryBatchGet};

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
    let reply = desk
        .world
        .app
        .call(SummaryBatchGet, &[])
        .token(&token)
        .body(&SummaryBatchBody {
            targets: vec![topic],
            offset: 0,
        })
        .send()
        .await
        .ok();
    assert_eq!(reply.items.len(), 1);
    assert_eq!(reply.items[0].text, None);
    assert!(!serde_json::to_string(&reply).unwrap().contains("canary"));
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
    let reply = desk
        .world
        .app
        .call(SummaryBatchGet, &[])
        .token(&reader.token)
        .body(&SummaryBatchBody {
            targets: vec![good, derived, private, missing, good],
            offset: 0,
        })
        .send()
        .await
        .ok();
    assert_eq!(reply.items.len(), 2);
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
