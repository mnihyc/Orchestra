//! Published SDK type evidence is separate from recorded native execution.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use crate::{sessions, stream};
use orch_types::Json;
use serde_json::json;

fn snapshots() -> [Json; 2] {
    [
        serde_json::from_str(include_str!("../tests/fixtures/protocol/2.1.312/sdk.json")).unwrap(),
        serde_json::from_str(include_str!("../tests/fixtures/protocol/2.1.319/sdk.json")).unwrap(),
    ]
}

fn fields<'a>(snapshot: &'a Json, name: &str) -> &'a serde_json::Map<String, Json> {
    snapshot["declarations"][name]["fields"]
        .as_object()
        .unwrap_or_else(|| panic!("missing published declaration {name}"))
}

fn consumed_fields(snapshot: &Json) -> Result<(), String> {
    for (declaration, consumed) in [
        ("SDKControlRequest", &["type", "request_id", "request"][..]),
        ("SDKControlResponse", &["type", "response"][..]),
        (
            "ControlResponse",
            &["subtype", "request_id", "response"][..],
        ),
        (
            "ControlErrorResponse",
            &["subtype", "request_id", "error"][..],
        ),
        (
            "SDKSystemMessage",
            &[
                "model",
                "mcp_servers",
                "permissionMode",
                "uuid",
                "session_id",
            ][..],
        ),
        (
            "SDKResultSuccess",
            &[
                "subtype",
                "is_error",
                "usage",
                "modelUsage",
                "duration_api_ms",
                "session_id",
            ][..],
        ),
        (
            "SDKHookStartedMessage",
            &["hook_id", "hook_name", "hook_event"][..],
        ),
        (
            "SDKHookResponseMessage",
            &["hook_id", "outcome", "exit_code"][..],
        ),
        (
            "SDKAPIRetryMessage",
            &["attempt", "max_retries", "retry_delay_ms", "error_status"][..],
        ),
        (
            "SDKTaskNotificationMessage",
            &["task_id", "tool_use_id", "status", "summary"][..],
        ),
    ] {
        for field in consumed {
            if !fields(snapshot, declaration).contains_key(*field) {
                return Err(format!(
                    "{declaration}.{field} is consumed but no longer declared"
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn stream_json_discriminators_and_control_envelopes_match_both_published_versions() {
    for snapshot in snapshots() {
        consumed_fields(&snapshot).unwrap();
        for (declaration, expected) in [
            ("SDKAssistantMessage", stream::FrameKind::Assistant),
            ("SDKUserMessage", stream::FrameKind::User),
            ("SDKResultSuccess", stream::FrameKind::Result),
            ("SDKSystemMessage", stream::FrameKind::System),
            ("SDKPartialAssistantMessage", stream::FrameKind::StreamEvent),
            ("SDKControlRequest", stream::FrameKind::ControlRequest),
            ("SDKControlResponse", stream::FrameKind::ControlResponse),
        ] {
            let native_type = fields(&snapshot, declaration)["type"]["literals"][0]
                .as_str()
                .unwrap();
            let frame = stream::Frame::parse(
                &json!({"type":native_type,"future_member":{"value":true}}).to_string(),
            )
            .unwrap();
            assert_eq!(
                frame.kind(),
                expected,
                "{declaration} in {}",
                snapshot["native_version"]
            );
            assert_eq!(frame.get("future_member"), Some(&json!({"value":true})));
        }
        let subtype = fields(&snapshot, "SDKControlGetSettingsRequest")["subtype"]["literals"][0]
            .as_str()
            .unwrap();
        assert_eq!(
            subtype, "get_settings",
            "the native settings reader's spelling must still exist"
        );
        let request = stream::control_request("r1", json!({"subtype":subtype}));
        assert_eq!(
            request["type"],
            fields(&snapshot, "SDKControlRequest")["type"]["literals"][0]
        );
        assert_eq!(request["request_id"], "r1");
        let body = json!({"effective":{"idleCompaction":false},"sources":[],"applied":true});
        let reply = stream::Frame::from_value(stream::control_success("r1", body.clone()));
        assert_eq!(
            reply.control_outcome(),
            Some(("r1".into(), stream::ControlOutcome::Success(body)))
        );
        let reply = stream::Frame::from_value(stream::control_error("r1", "refused"));
        assert_eq!(
            reply.control_outcome(),
            Some(("r1".into(), stream::ControlOutcome::Error("refused".into())))
        );
    }
}

#[test]
fn managed_pin_spellings_match_their_published_native_support_and_unset_cache_values_stay_unset() {
    let candidate = crate::config::SUPPORTED_RANGE.trim_start_matches('=');
    assert!(
        snapshots()
            .iter()
            .any(|snapshot| snapshot["native_version"].as_str() == Some(candidate)),
        "the supported candidate needs published schema evidence"
    );
    for snapshot in snapshots() {
        let declared = fields(&snapshot, "Settings");
        for (key, value) in sessions::managed_settings(false) {
            let shape = declared
                .get(&key)
                .or_else(|| snapshot["runtime_setting_fields"].get(&key))
                .unwrap_or_else(|| {
                    panic!(
                        "managed setting {key} absent in {}",
                        snapshot["native_version"]
                    )
                });
            if let Some(source) = shape.get("source") {
                assert!(
                    snapshot["evidence"]["runtime_files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|file| file == source),
                    "{key} needs an actual package runtime-schema source"
                );
            }
            if value.is_boolean() {
                assert_eq!(shape["kind"], "boolean", "{key}");
            }
            if value.is_number() {
                assert_eq!(shape["kind"], "number", "{key}");
            }
            if let Some(word) = value.as_str() {
                let choices = shape["literals"].as_array().unwrap();
                if !choices.is_empty() {
                    assert!(
                        choices.contains(&json!(word)),
                        "unsupported managed value {key}"
                    );
                }
            }
        }
        for name in sessions::managed_environment(false).keys() {
            let qualification = snapshot["qualification_environment_symbols"]
                .get(name)
                .and_then(Json::as_bool);
            if name == "CLAUDE_CODE_DISABLE_FEATURE_FLAGS" && qualification == Some(false) {
                assert_eq!(
                    crate::descriptor::support_of(snapshot["native_version"].as_str().unwrap()),
                    orch_agent_contract::descriptor::HarnessSupport::Unsupported,
                    "an older dialect without the newly qualified pin is not an accepted native runtime"
                );
                continue;
            }
            assert!(
                snapshot["managed_environment_symbols"][name] == true
                    || qualification == Some(true),
                "native environment switch {name} absent in {}",
                snapshot["native_version"]
            );
        }
        for name in [
            "CLAUDE_CODE_PROMPT_CACHE_TTL",
            "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL",
            "CLAUDE_CODE_SKIP_FAST_MODE_ORG_CHECK",
        ] {
            assert_eq!(
                snapshot["managed_environment_symbols"][name], true,
                "resolved or qualification switch {name} must exist"
            );
        }
        if snapshot["native_version"].as_str() == Some(candidate) {
            assert_eq!(
                snapshot["qualification_environment_symbols"]["CLAUDE_CODE_DISABLE_FEATURE_FLAGS"],
                true,
                "the candidate must declare the switch before native qualification"
            );
        }
        for name in ["promptCacheTtl", "subagentPromptCacheTtl"] {
            let choices = declared[name]["literals"].as_array().unwrap();
            for value in ["5m", "1h"] {
                assert!(
                    choices.contains(&json!(value)),
                    "{name} no longer takes {value}"
                );
            }
            assert!(
                !sessions::managed_settings(false).contains_key(name),
                "unset must not install a cache lifetime"
            );
        }
        if snapshot["native_version"].as_str() == Some(candidate) {
            assert!(
                snapshot["declarations"]["Settings"]["property_names"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("denyInsteadOfPrompting"))
            );
            assert_eq!(
                sessions::managed_settings(false)["permissions"]["denyInsteadOfPrompting"],
                false
            );
        } else {
            assert_eq!(
                crate::descriptor::support_of(snapshot["native_version"].as_str().unwrap()),
                orch_agent_contract::descriptor::HarnessSupport::Unsupported,
                "old published evidence does not become an accepted runtime"
            );
        }
        assert!(
            !sessions::managed_environment(false).contains_key("CLAUDE_CODE_DISABLE_MCP_TASKS"),
            "MCP tasks retain their in-call wait"
        );
        assert_eq!(snapshot["evidence"]["kind"], "published_protocol");
        assert_eq!(
            snapshot["evidence"]["package"]["name"],
            "@anthropic-ai/claude-agent-sdk"
        );
        let sha = snapshot["evidence"]["package"]["sha256"].as_str().unwrap();
        assert_eq!(sha.len(), 64);
        assert!(sha.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}

#[test]
fn removed_or_renamed_consumed_fields_fail_the_drift_guard_while_future_members_do_not() {
    let baseline = snapshots()[1].clone();
    for (declaration, member) in [
        ("SDKSystemMessage", "model"),
        ("ControlResponse", "request_id"),
        ("SDKHookResponseMessage", "outcome"),
    ] {
        let mut renamed = baseline.clone();
        let fields = renamed["declarations"][declaration]["fields"]
            .as_object_mut()
            .unwrap();
        let value = fields.remove(member).unwrap();
        fields.insert(format!("renamed_{member}"), value);
        assert!(consumed_fields(&renamed).is_err(), "{declaration}.{member}");
    }
    let mut extended = baseline;
    extended["declarations"]["SDKSystemMessage"]["fields"]["future_optional"] =
        json!({"optional":true,"kind":"reference","literals":[]});
    assert!(consumed_fields(&extended).is_ok());
}
