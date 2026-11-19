//! Metadata inspection of a large paginated native thread does not retain its whole turn list.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]
mod common;
use common::fake::FakeLauncher;
use orch_adapter_codex::CodexAdapter;
use orch_agent_contract::adapter::{AgentAdapter, AgentStart, EventSink};
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::loopback::Loopback;
use orch_plugin_contract::Secret;
use orch_types::{Json, new_id};
use serde_json::json;
use std::sync::Arc;
const TURNS: usize = 200_000;
const PAGE: usize = 100;

fn high_water_bytes() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|line| line.split_whitespace().next())
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 1024
}

#[tokio::test]
async fn inspecting_two_hundred_thousand_turns_holds_only_metadata_pages() {
    let fake = Arc::new(FakeLauncher::new(|method, params| match method {
        "thread/read" => Ok(
            json!({"thread":{"id":"large-native","model":"mock-large","reasoningEffort":"high"}}),
        ),
        "thread/turns/list" => {
            assert_eq!(params["threadId"], "large-native");
            assert_eq!(params["sortDirection"], "asc");
            assert_eq!(params["itemsView"], "notLoaded");
            assert_eq!(params["limit"], PAGE);
            let first = params["cursor"]
                .as_str()
                .map_or(0, |cursor| cursor.parse::<usize>().unwrap());
            let end = (first + PAGE).min(TURNS);
            let data: Vec<Json> = (first..end)
                .map(|at| {
                    json!({
                        "id":format!("native-turn-{at:08}"),
                        "status":if at % 13 == 0 {"inProgress"} else {"completed"},
                        "startedAt":1_700_000_000+at,"completedAt":1_700_000_001+at,
                        "error":null,"items":[],"itemsView":"notLoaded"
                    })
                })
                .collect();
            Ok(json!({"data":data,"nextCursor":(end<TURNS).then(||end.to_string())}))
        }
        _ => Ok(json!({})),
    }));
    let mut adapter = CodexAdapter::with_launcher(
        Arc::clone(&fake) as Arc<dyn orch_adapter_codex::process::Launcher>
    );
    let home = tempfile::tempdir().unwrap();
    let (sink, _events) = EventSink::channel();
    adapter
        .start(
            AgentStart {
                provider_id: new_id(),
                agent_id: new_id(),
                harness_kind: "codex".into(),
                profile: AgentProfile::default(),
                home: Some(home.path().to_string_lossy().into_owned()),
                provider_config: json!({}),
                loopback: Loopback::new(41999, Secret::new("test-token")),
                binary: None,
            },
            sink,
        )
        .await
        .unwrap();
    let before = high_water_bytes();
    let record = adapter.read_session("large-native", false).await.unwrap();
    let growth = high_water_bytes().saturating_sub(before);
    assert_eq!(
        record.turns,
        i64::try_from(TURNS - TURNS.div_ceil(13)).unwrap()
    );
    assert_eq!(record.entries, 0);
    assert!(record.file.is_none());
    // The fixture needs one 100-row metadata page and 2000 retained request observations.
    // 64 MiB leaves room for RPC/runtime buffers but cannot hold 200000 decoded JSON objects.
    assert!(
        growth < 64 * 1024 * 1024,
        "inspection retained history: high-water growth={growth}"
    );
    assert_eq!(fake.state.calls("thread/turns/list").len(), TURNS / PAGE);
    assert!(fake.state.calls("thread/items/list").is_empty());
    assert!(fake.state.calls("thread/resume").is_empty());
    assert!(fake.state.calls("thread/fork").is_empty());
    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn repeated_turn_cursors_refuse_without_an_unbounded_cursor_set_or_a_partial_count() {
    for cycle in [1, 2, 3] {
        let fake = Arc::new(FakeLauncher::new(move |method, params| match method {
            "thread/read" => Ok(json!({"thread":{"id":"repeated-native"}})),
            "thread/turns/list" => {
                assert_eq!(params["itemsView"], "notLoaded");
                let previous = params["cursor"]
                    .as_str()
                    .map_or(0, |cursor| cursor.parse::<usize>().unwrap());
                Ok(json!({
                    "data":[{"id":"one-turn","status":"completed"}],
                    "nextCursor":((previous+1)%cycle).to_string()
                }))
            }
            _ => Ok(json!({})),
        }));
        let mut adapter = CodexAdapter::with_launcher(
            Arc::clone(&fake) as Arc<dyn orch_adapter_codex::process::Launcher>
        );
        let home = tempfile::tempdir().unwrap();
        let (sink, _events) = EventSink::channel();
        adapter
            .start(
                AgentStart {
                    provider_id: new_id(),
                    agent_id: new_id(),
                    harness_kind: "codex".into(),
                    profile: AgentProfile::default(),
                    home: Some(home.path().to_string_lossy().into_owned()),
                    provider_config: json!({}),
                    loopback: Loopback::new(41999, Secret::new("test-token")),
                    binary: None,
                },
                sink,
            )
            .await
            .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            adapter.read_session("repeated-native", false),
        )
        .await
        .expect("a repeating cursor must refuse before the RPC deadline");
        let error = result.expect_err("a cursor cycle cannot certify a completed-turn count");
        assert!(error.to_string().contains("cursor repeats"), "{error}");
        assert!(
            fake.state.calls("thread/turns/list").len() <= 3 * cycle,
            "a cycle of {cycle} cursors must be caught without walking forever"
        );
        assert!(fake.state.calls("thread/items/list").is_empty());
        adapter.stop().await.unwrap();
    }
}
