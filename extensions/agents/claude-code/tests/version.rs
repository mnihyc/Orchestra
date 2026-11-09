//! Which versions of the harness the adapter starts, against programs that only say their version.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use futures::StreamExt;
use orch_adapter_claude_code::ClaudeCodeAdapter;
use orch_agent_contract::adapter::{
    AdapterError, AdapterEvent, AgentAdapter, AgentStart, EventSink,
};
use orch_agent_contract::descriptor::HarnessSupport;
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::facts::HarnessState;
use orch_agent_contract::loopback::Loopback;
use orch_plugin_contract::Secret;
use orch_types::{SystemClock, new_id};
use serde_json::json;

/// The programs the tests start, written once and before any of them is run: a program that is being
/// written while another thread starts a process cannot be started (the file is busy), so no test writes one.
struct Programs {
    _directory: tempfile::TempDir,
    old: PathBuf,
    just_in_range: PathBuf,
    current: PathBuf,
    newer: PathBuf,
    mute: PathBuf,
    absent: PathBuf,
}

fn programs() -> &'static Programs {
    static PROGRAMS: OnceLock<Programs> = OnceLock::new();
    PROGRAMS.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        let write = |name: &str, body: &str| {
            let path = directory.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            path
        };
        Programs {
            old: write("old", "echo '2.1.284 (Claude Code)'"),
            just_in_range: write("just", "echo '2.1.319 (Claude Code)'"),
            current: write("current", "echo '2.1.319 (Claude Code)'"),
            newer: write("newer", "echo '2.2.1 (Claude Code)'"),
            mute: write("mute", "echo 'nothing to say about that'"),
            absent: directory.path().join("not-there"),
            _directory: directory,
        }
    })
}

async fn start(
    program: &std::path::Path,
) -> (
    ClaudeCodeAdapter,
    Result<orch_agent_contract::adapter::AgentStarted, AdapterError>,
    orch_agent_contract::adapter::EventStream,
) {
    let (sink, events) = EventSink::channel();
    let mut adapter = ClaudeCodeAdapter::new(Arc::new(SystemClock));
    let started = adapter
        .start(
            AgentStart {
                provider_id: new_id(),
                provider_kind: None,
                agent_id: new_id(),
                harness_kind: "claude-code".into(),
                profile: AgentProfile {
                    harness_version: None,
                    attach: None,
                    config: json!({}),
                    env: std::collections::BTreeMap::new(),
                },
                home: None,
                provider_config: json!({}),
                loopback: Loopback::new(41_000, Secret::new("t")),
                binary: Some(program.display().to_string()),
            },
            sink,
        )
        .await;
    (adapter, started, events)
}

#[tokio::test]
async fn a_version_older_than_the_range_is_refused_and_the_message_names_the_range() {
    let (_, started, _) = start(&programs().old).await;
    let Err(AdapterError::HarnessUnavailable(why)) = started else {
        panic!("it was refused");
    };
    assert!(
        why.contains("2.1.284") && why.contains(orch_adapter_claude_code::config::SUPPORTED_RANGE),
        "{why}"
    );
}

#[tokio::test]
async fn provider_environment_in_a_profile_is_refused_before_any_home_or_program_is_opened() {
    let directory = tempfile::tempdir().unwrap();
    let untouched_home = directory.path().join("unopened");
    for key in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_USE_GATEWAY",
        "anthropic_base_url",
    ] {
        let (sink, _events) = EventSink::channel();
        let mut adapter = ClaudeCodeAdapter::new(Arc::new(SystemClock));
        let result = adapter
            .start(
                AgentStart {
                    provider_id: new_id(),
                    provider_kind: None,
                    agent_id: new_id(),
                    harness_kind: "claude-code".into(),
                    profile: AgentProfile {
                        env: std::collections::BTreeMap::from([(
                            key.into(),
                            "private-profile-value".into(),
                        )]),
                        ..AgentProfile::default()
                    },
                    home: Some(untouched_home.display().to_string()),
                    provider_config: json!({}),
                    loopback: Loopback::new(41_000, Secret::new("disposable-token")),
                    binary: Some(
                        directory
                            .path()
                            .join("absent-program")
                            .display()
                            .to_string(),
                    ),
                },
                sink,
            )
            .await;
        let Err(AdapterError::Other {
            code,
            retryable,
            message,
        }) = result
        else {
            panic!("the profile must be refused before starting a program");
        };
        assert_eq!(code, "invalid_input");
        assert!(!retryable);
        assert!(!message.contains("private-profile-value"));
        assert!(!untouched_home.exists());
    }
}

#[tokio::test]
async fn the_oldest_version_of_the_range_and_the_current_one_are_supported() {
    for program in [&programs().just_in_range, &programs().current] {
        let (_, started, _) = start(program).await;
        let started = started.unwrap();
        assert_eq!(started.descriptor.harness.status, HarnessSupport::Supported);
        assert_eq!(started.descriptor.harness.version, started.harness_version);
    }
}

#[tokio::test]
async fn a_newer_version_than_the_range_is_started_and_said_to_be_untested() {
    let (_, started, _) = start(&programs().newer).await;
    let started = started.unwrap();
    assert_eq!(started.harness_version, "2.2.1");
    assert_eq!(started.descriptor.harness.status, HarnessSupport::Untested);
}

#[tokio::test]
async fn a_program_that_does_not_say_its_version_or_is_not_there_is_not_started() {
    for program in [&programs().mute, &programs().absent] {
        let (_, started, _) = start(program).await;
        assert!(matches!(started, Err(AdapterError::HarnessUnavailable(_))));
    }
}

#[tokio::test]
async fn the_state_of_the_harness_is_reported_online_at_the_start_and_offline_at_the_stop() {
    let (mut adapter, started, mut events) = start(&programs().current).await;
    started.unwrap();
    let again = adapter.probe().await.unwrap();
    assert_eq!(
        again.harness.version, "2.1.319",
        "probing asks the program again"
    );
    adapter.stop().await.unwrap();
    let mut states = Vec::new();
    while let Some(AdapterEvent::Status(status)) = events.next().await {
        states.push(status.status);
        if status.status == HarnessState::Offline {
            break;
        }
    }
    assert_eq!(states, [HarnessState::Online, HarnessState::Offline]);
}

#[tokio::test]
async fn the_started_adapter_exposes_the_same_selected_home_output_cache_to_its_host() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("selected-home");
    let (sink, _events) = EventSink::channel();
    let mut adapter = ClaudeCodeAdapter::new(Arc::new(SystemClock));
    assert!(adapter.output_directory().is_none());
    adapter
        .start(
            AgentStart {
                provider_id: new_id(),
                provider_kind: None,
                agent_id: new_id(),
                harness_kind: "claude-code".into(),
                profile: AgentProfile::default(),
                home: Some(home.display().to_string()),
                provider_config: json!({}),
                loopback: Loopback::new(41_000, Secret::new("disposable-token")),
                binary: Some(programs().current.display().to_string()),
            },
            sink,
        )
        .await
        .unwrap();
    assert_eq!(adapter.output_directory(), Some(home.join("outputs")));
    assert!(
        !home.join("outputs").exists(),
        "declaring the output cache does not fabricate image bytes"
    );
    adapter.stop().await.unwrap();
}
