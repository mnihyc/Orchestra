//! Existing binding options project confirmed creation choices without creating preview objects.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod rig;
mod support;

use orch_api_contract::binding::{
    NodeBindingOptionsGet, NodeBindingOptionsGetQuery, ThreadBindingOptionsGet,
};
use orch_api_contract::common::ErrorCode;
use orch_types::Json;
use rig::Scene;

#[tokio::test]
async fn creation_options_have_ordered_contextual_bounds_and_rebinding_has_no_new_gift() {
    let s = Scene::new().await;
    let project = s.tree.project.base.id;
    let before: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM nodes),(SELECT count(*) FROM permission_entries)",
    )
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    let options = s
        .rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&project])
        .token(&s.ada.token)
        .query(&NodeBindingOptionsGetQuery::default())
        .send()
        .await
        .ok();
    let raw = serde_json::to_value(options).unwrap();
    let access = raw
        .get("agent_access")
        .expect("creation options include genuine source availability");
    assert_eq!(access["preset"]["where"], "topic");
    let choices = access["choices"].as_array().unwrap();
    assert_eq!(
        choices
            .iter()
            .map(|choice| choice["where"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["nothing", "thread", "topic", "project"]
    );
    assert_eq!(choices[0]["parts"], serde_json::json!({}));
    assert!(
        choices[1]["parts"].get("timeline").is_none(),
        "This thread never grants its intrinsic conversation or siblings"
    );
    let after: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM nodes),(SELECT count(*) FROM permission_entries)",
    )
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(after, before, "availability creates no node or ACL entry");
    let thread = s.thread().await;
    let rebound = s
        .rig
        .world
        .app
        .call(ThreadBindingOptionsGet, &[&thread.base.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let raw: Json = serde_json::to_value(rebound).unwrap();
    assert!(
        raw.get("agent_access").is_none(),
        "rebinding an existing holder is not another creation gift"
    );
}

#[tokio::test]
async fn creation_page_context_uses_field_local_json_transport_and_preserves_the_existing_door() {
    let s = Scene::new().await;
    let project = s.tree.project.base.id;
    for query in [
        "thread_access=%22private%22",
        "thread_access=%7B%22rows%22%3A%5B%7B%22permission%22%3A%22doc.view%22%2C%22inherit%22%3Afalse%7D%5D%7D",
        "thread_access=null",
    ] {
        let options = s
            .rig
            .world
            .app
            .call(NodeBindingOptionsGet, &[&project])
            .token(&s.ada.token)
            .raw_query(query)
            .send()
            .await
            .ok();
        assert!(
            serde_json::to_value(options)
                .unwrap()
                .get("agent_access")
                .is_some(),
            "{query}"
        );
    }
    for query in [
        "thread_access=",
        "thread_access=%7B",
        "thread_access=private",
    ] {
        let problem = s
            .rig
            .world
            .app
            .call(NodeBindingOptionsGet, &[&project])
            .token(&s.ada.token)
            .raw_query(query)
            .send()
            .await
            .problem();
        assert_eq!(problem.code, ErrorCode::RequestInvalid, "{query}");
    }
    let denied = s
        .rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&s.tree.topic.base.id])
        .token(&s.cai.token)
        .query(&NodeBindingOptionsGetQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    assert_eq!(
        denied.extra.get("required").and_then(Json::as_str),
        Some("topic.start_threads")
    );
}
