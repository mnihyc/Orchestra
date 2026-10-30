//! Registered external translators carry a leased model request across formats, convert streamed tools and usage, and leave endpoint credential custody intact.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::model::{MESSAGES, MODEL, ProviderRig, RESPONSES, protocols};
use orch_connector_core::roles::provider::{ProviderParts, plug_in};
use orch_link::commands::Progress;
use orch_plugin::registration::{BuiltIn, Plugins, registration_of};
use orch_plugin::supervisor::Options;
use orch_plugin_contract::registration::PluginHost;
use orch_testkit::doubles::model::Verdict;
use serde_json::json;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_external_translator_carries_json_streamed_tools_errors_and_usage_through_the_real_connector()
 {
    let data = tempfile::tempdir().unwrap();
    let program = data.path().join("translator.py");
    orch_testkit::doubles::plugin::write_translator(&program).unwrap();
    let registration = registration_of(
        "translator",
        "controlled-translator",
        &["python3".to_owned(), program.to_string_lossy().into_owned()],
        None,
        &std::collections::BTreeMap::new(),
        "never",
    )
    .unwrap();
    let plugins = Plugins::start(
        PluginHost::Connector,
        &[registration],
        &BuiltIn::new(),
        &Options::new("test"),
    )
    .await
    .unwrap();
    tokio::time::timeout(common::PATIENCE, plugins.settled())
        .await
        .unwrap();
    let translators: Vec<_> = plugins
        .translators()
        .into_iter()
        .map(|(_, translator)| translator)
        .collect();
    assert_eq!(translators.len(), 1);
    let mut p = ProviderRig::with(move |rig, _| {
        rig.plug = Some(Arc::new(move |builder, roles, data, key, clock| {
            let parts = ProviderParts::new(
                protocols().with_translators(translators.clone()),
                data,
                key,
                clock,
            )
            .unwrap();
            plug_in(builder, roles, parts)
        }));
    })
    .await;
    let started = p
        .start_provider(&[(MESSAGES, p.mock.messages_base_url(), json!({}))])
        .await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    let lease = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let harness = p.harness(p.thread);
    p.mock
        .queue("say Translated answer.\nusage 100 20 cached=10 cache_write=5")
        .unwrap();
    let answer = harness
        .post("/v1/responses", &json!({"model": MODEL, "input":"hello"}))
        .await;
    assert_eq!(answer.status, 200);
    assert_eq!(
        answer.json()["output"][0]["content"][0]["text"],
        "Translated answer."
    );
    p.mock
        .queue("say Streamed answer.\ntool lookup {\"name\":\"world\"}\nusage 200 30")
        .unwrap();
    let streamed = harness.post("/v1/responses", &json!({"model":MODEL, "input":"hello", "stream":true,
        "tools":[{"type":"function", "name":"lookup", "parameters":{"type":"object", "properties":{"name":{"type":"string"}}}}]
    })).await;
    assert_eq!(streamed.status, 200);
    let text = streamed.text();
    assert!(
        text.contains("response.output_text.delta")
            && text.contains("response.function_call_arguments.delta"),
        "{text}"
    );
    let usage = p.wait_provider_event("lease.usage", 2).await;
    assert_eq!(usage[0].body["usage"]["input_tokens"], 100);
    assert_eq!(usage[0].body["usage"]["cache_write_tokens"], 5);
    let close = p.close_lease(lease).await;
    assert_eq!(close["requests"], 2);
    assert_eq!(close["usage"]["total_tokens"], 350);
    let requests = p.model_requests();
    assert!(
        requests
            .iter()
            .all(|request| request.path.ends_with("/messages")
                && request.credential.verdict == Verdict::Accepted
                && request.credential.presented_by.len() == 1)
    );
    p.mock.set_credential(Some("refused-key"));
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let refused = harness
        .post("/v1/responses", &json!({"model":MODEL, "input":"hello"}))
        .await;
    assert_eq!(refused.status, 401);
    assert!(refused.json().get("error").is_some());
    assert!(!refused.text().contains(&p.key));
    p.rig.kill().await;
    plugins.shutdown().await;
}
