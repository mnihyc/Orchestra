//! A place's model and provider defaults written before pools keep their meaning as targets.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_testkit::database::TestDb;
use serde_json::{Value, json};
use std::path::Path;

/// The data part of the migration that made model and provider defaults targets, run again over rows written in the
/// earlier shape (the schema part is already in the fresh database).
fn conversion() -> String {
    let file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../migrations/0021_pool_bindings.sql");
    let text = std::fs::read_to_string(file).unwrap();
    let start = text
        .find("-- A place's model and provider defaults")
        .expect("the migration converts stored defaults");
    text[start..].to_owned()
}

#[tokio::test]
async fn a_stored_model_name_and_provider_id_become_the_same_targets_and_typed_ones_stay() {
    let db = TestDb::fresh().await;
    let provider = orch_types::new_id();
    let typed = json!({"type":"pool","kind":"model","id":orch_types::new_id()});
    for (key, value) in [
        ("agent.model", json!("gpt-6-luna")),
        ("agent.default_provider_id", json!(provider.to_string())),
        ("agent.effort", json!("low")),
    ] {
        sqlx::query("INSERT INTO settings(node_id,key,value) VALUES(NULL,$1,$2)")
            .bind(key)
            .bind(value)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    sqlx::raw_sql(sqlx::AssertSqlSafe(conversion()))
        .execute(&db.pool)
        .await
        .unwrap();
    let read = |key: &'static str| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_as::<_, (Value, i64)>(
                "SELECT value,revision FROM settings WHERE node_id IS NULL AND key=$1",
            )
            .bind(key)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        read("agent.model").await,
        (json!({"type":"offering","name":"gpt-6-luna"}), 2)
    );
    assert_eq!(
        read("agent.default_provider_id").await,
        (
            json!({"type":"single","resource":{"kind":"provider","id":provider.to_string()}}),
            2
        )
    );
    assert_eq!(
        read("agent.effort").await,
        (json!("low"), 1),
        "other keys keep their values"
    );
    // A value already in the new shape, and a second run, change nothing.
    sqlx::query("UPDATE settings SET value=$1 WHERE node_id IS NULL AND key='agent.model'")
        .bind(&typed)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(conversion()))
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(read("agent.model").await, (typed, 2));
    assert_eq!(read("agent.default_provider_id").await.1, 2);
}
