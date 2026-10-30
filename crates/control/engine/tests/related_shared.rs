//! Related search admits only the agent's current shares and preserves the shared path grammar.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_link_contract::commands::OrchTool;
use serde_json::json;
use shared::*;

#[tokio::test]
async fn related_search_resolves_a_document_path_and_checks_every_candidate_against_current_shares()
{
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Writing").await;
    let target = world
        .document_beside(topic, "Referenced plan", &["The release plan"])
        .await;
    let source = world
        .document_beside(topic, "Source plan", &["A source"])
        .await;
    let unshared = world
        .document_beside(topic, "Unshared notes", &["Not given to this agent"])
        .await;
    sqlx::query("UPDATE document_blocks SET content=$2 WHERE document_id=$1")
        .bind(source)
        .bind(json!([
            {"type":"ref","kind":"document","id":target},
            {"type":"ref","kind":"document","id":unshared}
        ]))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(
            topic,
            "related.weights",
            json!({"meaning":"0","place":"0","references":"1"}),
        )
        .await;
    share(
        &world, "document", source, "live", "doc.view", "any", reader,
    )
    .await;
    let absent = ok(call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"related_to":format!("id:{source}")}),
    )
    .await);
    assert!(
        absent["hits"].as_array().unwrap().is_empty(),
        "a person's document access is not an agent share: {absent}"
    );
    assert_eq!(
        absent["searched"]["entries"], 0,
        "unshared candidate counts are not disclosed"
    );
    let grant = share(
        &world, "document", target, "live", "doc.view", "any", reader,
    )
    .await;
    let found = ok(call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"related_to":format!("id:{source}")}),
    )
    .await);
    let hits = found["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["id"], json!(target));
    assert_eq!(hits[0]["related_by"], json!(["references"]));
    assert_eq!(found["meaning"]["state"], "off");
    assert!(
        hits[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("Referenced plan.md")
    );
    let path = found["related_to"].as_str().unwrap();
    let by_path = ok(call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"related_to":path}),
    )
    .await);
    assert_eq!(by_path["hits"], found["hits"]);
    let read = call(
        &world,
        reader,
        OrchTool::ReadShared,
        json!({"path":hits[0]["path"]}),
    )
    .await;
    assert!(
        read.error.is_none(),
        "a related path can be followed with read_shared"
    );
    sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE id=$1")
        .bind(grant)
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    let revoked = ok(call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"related_to":path}),
    )
    .await);
    assert!(
        revoked["hits"].as_array().unwrap().is_empty(),
        "revocation applies on the next call"
    );
    let refused_source = call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"related_to":format!("id:{unshared}")}),
    )
    .await;
    assert_eq!(refused(&refused_source).0, "not_found");
}

#[tokio::test]
async fn related_search_names_documents_and_refuses_a_query_that_would_be_ignored() {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let thread = call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"related_to":format!("id:{reader}")}),
    )
    .await;
    assert_eq!(refused(&thread).0, "invalid");
    let both = call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"related_to":"docs/Plan.md","query":"release"}),
    )
    .await;
    assert_eq!(refused(&both).0, "invalid");
}

#[tokio::test]
async fn related_agent_pages_keep_their_current_readable_window() {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Paged reader").await;
    let topic = world.topic("Paged writing").await;
    let source = world
        .document_beside(topic, "Paged source", &["Source"])
        .await;
    let mut targets = Vec::new();
    for number in 0..4 {
        let id = world
            .document_beside(topic, &format!("Paged target {number}"), &["Target"])
            .await;
        share(&world, "document", id, "live", "doc.view", "any", reader).await;
        targets.push(id);
    }
    share(
        &world, "document", source, "live", "doc.view", "any", reader,
    )
    .await;
    sqlx::query("UPDATE document_blocks SET content=$2 WHERE document_id=$1")
        .bind(source)
        .bind(json!(
            targets
                .iter()
                .map(|id| json!({"type":"ref","kind":"document","id":id}))
                .collect::<Vec<_>>()
        ))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(
            topic,
            "related.weights",
            json!({"meaning":"0","place":"0","references":"1"}),
        )
        .await;
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..4 {
        let page = ok(call(
            &world,
            reader,
            OrchTool::SearchShared,
            json!({"related_to":format!("id:{source}"),"max_results":1,"cursor":cursor}),
        )
        .await);
        let hits = page["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert!(seen.insert(hits[0]["id"].as_str().unwrap().to_owned()));
        assert!(
            hits[0]["cursor"].is_string(),
            "a byte cut can continue after the last shown item"
        );
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 4);
    assert!(cursor.is_none());
}
