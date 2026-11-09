//! Descriptions remain deliberate writes even when a place has a summarizer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use common::World;
use orch_db::queries::documents as rows;
use orch_documents::versions::Writer;
use orch_testkit::doubles::summarizer::Scripted;
use serde_json::json;
use shared::Places;
use std::sync::Arc;

#[test]
fn description_automation_settings_are_not_registered() {
    for key in [
        "auto.description",
        "auto.topic_rollup",
        "docs.description_chars",
    ] {
        assert!(
            orch_settings_contract::registry::get(key).is_none(),
            "{key}"
        );
    }
    for key in [
        "auto.title",
        "auto.tags",
        "auto.summarizer",
        "auto.triggers",
    ] {
        assert!(
            orch_settings_contract::registry::get(key).is_some(),
            "{key}"
        );
    }
}

#[tokio::test]
async fn document_content_does_not_schedule_a_description_and_a_written_description_survives() {
    let world = World::new().await;
    world.engine().stop();
    let producer = Arc::new(Scripted::default());
    world.engine().attach_summarizer(producer.clone());
    world
        .set(world.topic, "auto.summarizer", json!({"kind":"scripted"}))
        .await;
    let document = world
        .document_beside(world.topic, "Written plan", &["The first paragraph."])
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let before = rows::get(&mut conn, document).await.unwrap().unwrap();
    assert_eq!(before.description, "");
    let obsolete_columns: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns WHERE table_schema=current_schema() AND table_name IN ('nodes','documents') AND column_name='description_source'",
    ).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        obsolete_columns, 0,
        "descriptions have no automatic source state"
    );
    let obsolete_trigger: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_trigger WHERE tgrelid='document_blocks'::regclass AND NOT tgisinternal AND tgname='document_summary_on_write'",
    ).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        obsolete_trigger, 0,
        "writing blocks creates no automatic description job"
    );
    rows::describe(
        &mut conn,
        document,
        "A deliberate description",
        &Writer::User(world.ada),
        world.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE document_blocks SET content=$2,text='A changed paragraph.' WHERE document_id=$1",
    )
    .bind(document)
    .bind(json!([{"type":"text","text":"A changed paragraph."}]))
    .execute(&mut *conn)
    .await
    .unwrap();
    let after = rows::get(&mut conn, document).await.unwrap().unwrap();
    assert_eq!(after.description, "A deliberate description");
    assert_eq!(after.updated_by, Some(world.ada));
    assert!(producer.requests().is_empty());
}
