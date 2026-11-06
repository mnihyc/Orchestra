//! A real scoped Shared page keeps the native aggregate lookup batched as entries grow.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod reader_diagnostics;
mod support;
mod thread_history_scope;

use desk::Desk;
use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::common::PageQuery;
use orch_api_contract::nodes::{NodeGet, SharedList};
use orch_api_core::testing::RawReply;
use orch_db::queries::documents;
use orch_types::{Id, Json};
use serde_json::json;

async fn shared(desk: &Desk, token: &str) -> RawReply {
    desk.world
        .app
        .call(SharedList, &[])
        .token(token)
        // Both fixture sizes fit one complete native window with no real lookahead.
        .query(&PageQuery {
            limit: Some(3),
            cursor: None,
        })
        .send_bytes()
        .await
}

fn page(reply: RawReply) -> Json {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let page: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(page["partial"], false);
    assert!(page["next_cursor"].is_null());
    page
}

async fn give(desk: &Desk, reader: Id, document: Id) {
    desk.world.app.call(PermissionPageUpdate, &[&"documents", &document])
        .token(&desk.ada.token)
        .raw_body(&json!({"name":[{"holders":[{"kind":"user","id":reader}],"choice":"full_view","export":false}]}))
        .send().await.ok();
}

fn aggregate_signatures(evidence: &Json) -> Vec<String> {
    assert_eq!(
        evidence["omitted"], 0,
        "calibration must not lose fingerprint evidence"
    );
    let signatures: Vec<_> = evidence["signatures"]
        .as_object()
        .unwrap()
        .keys()
        .filter(|signature| {
            let signature: Json = serde_json::from_str(signature).unwrap();
            let hints = signature["hints"].as_array().unwrap();
            hints.iter().any(|hint| hint.as_str() == Some("documents"))
                && hints.iter().any(|hint| hint.as_str() == Some("holder"))
        })
        .cloned()
        .collect();
    assert!(
        !signatures.is_empty(),
        "the real owner_counts query must calibrate its own observed fingerprint"
    );
    signatures
}

fn aggregate_calls(evidence: &Json, signatures: &[String]) -> u64 {
    assert_eq!(
        evidence["omitted"], 0,
        "request must not truncate query evidence"
    );
    signatures
        .iter()
        .map(|signature| {
            evidence["signatures"][signature.as_str()]
                .as_u64()
                .unwrap_or(0)
        })
        .sum()
}

#[tokio::test]
async fn scoped_shared_document_aggregate_queries_do_not_repeat_per_returned_document() {
    let desk = Desk::new().await;
    let reader = desk.stranger("batch_shared_reader").await;
    let first = desk
        .make(&desk.ada, "First actual batched Shared document")
        .await
        .base
        .id;
    let second = desk
        .make(&desk.ada, "Second actual batched Shared document")
        .await
        .base
        .id;
    // Both are real human-owned documents beside the SAME unviewable place. The second
    // becomes a native entrance only after its actual ordinary View grant below.
    give(&desk, reader.id(), first).await;
    let parent = desk
        .world
        .app
        .call(NodeGet, &[&desk.topic])
        .token(&reader.token)
        .send_bytes()
        .await;
    assert_eq!(
        parent.status.as_u16(),
        403,
        "Shared's native containing-place suppression premise"
    );
    let selected = thread_history_scope::key(
        &desk.world,
        reader.id(),
        vec![
            thread_history_scope::rule("SharedList", "document", first, "self", "read"),
            thread_history_scope::rule("SharedList", "document", second, "self", "read"),
        ],
    )
    .await
    .unwrap();

    let signatures = {
        let mut conn = desk.world.conn().await;
        // Prepare on the SAME held connection before calibration so type/preparation
        // traffic cannot be mistaken for the aggregate query's execution fingerprint.
        documents::owner_counts(&mut conn, &[first, second], desk.world.now())
            .await
            .unwrap();
        let (owners, evidence) = reader_diagnostics::trace(
            "shared_owner_lookup_calibration",
            documents::owner_counts(&mut conn, &[first, second], desk.world.now()),
        )
        .await;
        let owners = owners.unwrap();
        assert!(owners.contains_key(&first) && owners.contains_key(&second));
        aggregate_signatures(&evidence)
    }; // Release calibration's connection before the actual HTTP request.

    let native_one = page(shared(&desk, &reader.token).await);
    assert_eq!(native_one["items"].as_array().unwrap().len(), 1);
    assert_eq!(native_one["items"][0]["document"]["id"], json!(first));
    assert_eq!(page(shared(&desk, &selected.secret).await), native_one); // Warm actual scoped path.
    let (one, one_queries) = reader_diagnostics::trace(
        "shared_scoped_one_document",
        shared(&desk, &selected.secret),
    )
    .await;
    assert_eq!(
        page(one),
        native_one,
        "scoped projection keeps actual ordinary DTOs"
    );

    give(&desk, reader.id(), second).await;
    let native_two = page(shared(&desk, &reader.token).await);
    assert_eq!(native_two["items"].as_array().unwrap().len(), 2);
    let ids: Vec<_> = native_two["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["document"]["id"].clone())
        .collect();
    assert!(ids.contains(&json!(first)) && ids.contains(&json!(second)));
    assert_eq!(page(shared(&desk, &selected.secret).await), native_two); // Same warm boundary.
    let (two, two_queries) = reader_diagnostics::trace(
        "shared_scoped_two_documents",
        shared(&desk, &selected.secret),
    )
    .await;
    assert_eq!(page(two), native_two);

    let one_calls = aggregate_calls(&one_queries, &signatures);
    let two_calls = aggregate_calls(&two_queries, &signatures);
    assert!(
        one_calls > 0,
        "the actual HTTP page must execute its calibrated native aggregate"
    );
    eprintln!(
        "shared_document_batch_queries {}",
        json!({
            "one_documents":1,"two_documents":2,"one_aggregate_calls":one_calls,"two_aggregate_calls":two_calls,
            "one_total_statements":one_queries["statements"],"two_total_statements":two_queries["statements"],
        })
    );
    // Only the calibrated aggregate must be cardinality-independent. Other legitimate
    // audience/scope work is recorded above, without an invented global query threshold.
    assert_eq!(
        two_calls, one_calls,
        "native aggregate lookup repeated per returned document"
    );
}
