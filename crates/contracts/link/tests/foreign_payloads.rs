//! A link reader receives concrete native facts and file metadata, and rejects malformed payloads before dispatch.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_link_contract::{
    commands::{Cmd, Command},
    events::{Event, Report},
    foreign,
};
use serde_json::{Value, json};

const ID: &str = "0199c1a0-0000-7000-8000-000000000001";
const OTHER: &str = "0199c1a0-0000-7000-8000-000000000002";

#[test]
fn every_named_payload_has_a_concrete_schema_and_refuses_unrelated_values() {
    macro_rules! objects {
        ($($shape:ty),* $(,)?) => { $(
            let schema = schemars::schema_for!($shape);
            let value = serde_json::to_value(schema).unwrap();
            assert!(value.get("type").is_some() || value.get("anyOf").is_some()
                || value.get("oneOf").is_some() || value.get("$ref").is_some(),
                "an independent reader needs the named payload's shape");
            assert!(serde_json::from_value::<$shape>(json!(27)).is_err());
            assert!(serde_json::from_value::<$shape>(json!("unrelated text")).is_err());
        )* };
    }
    objects!(
        foreign::AgentDescriptor,
        foreign::AgentProfile,
        foreign::ApprovalDecision,
        foreign::HistoryReport,
        foreign::NativeSession,
        foreign::ModelSuggestion,
        foreign::LoginState,
        foreign::HarnessRequest,
        foreign::ItemPayload,
        foreign::TokenUsage,
        foreign::Rate,
        foreign::ProviderFacts,
        foreign::DirEntry,
        foreign::FileStat,
        foreign::ArchiveListing,
        foreign::FileMatch,
        foreign::GitStatus,
        foreign::ProcessInfo,
        foreign::ResourceSample,
        foreign::JobProgress,
        foreign::McpServerSpec,
        foreign::EndpointOptions,
        foreign::RuntimeSpec,
        foreign::DriverDescriptor,
        foreign::UnitRef,
    );
    let status: foreign::ItemStatus = serde_json::from_value(json!("completed")).unwrap();
    assert_eq!(serde_json::to_value(status).unwrap(), json!("completed"));
    assert!(serde_json::from_value::<foreign::ItemStatus>(json!("invented status")).is_err());
}

#[test]
fn starting_a_profile_preserves_its_vendor_configuration_but_refuses_unknown_request_members() {
    let literal = json!({ "id":ID,"k":"agent.start","key":null,
    "deadline":"2026-10-14T16:00:00.000Z","b":{
        "provider_id":ID,"agent_id":OTHER,"harness_kind":"example",
        "profile":{"harness_version":null,"config":{"vendor_setting":"kept"},"env":{"LANG":"C.UTF-8"}},
        "home":null,"provider_config":{}
    }});
    let frame: Cmd = serde_json::from_value(literal.clone()).unwrap();
    let Command::AgentStart(start) = frame.command().unwrap() else {
        panic!("wrong command");
    };
    let profile: orch_agent_contract::facts::AgentProfile = start.profile;
    assert_eq!(profile.config["vendor_setting"], "kept");
    assert_eq!(profile.env["LANG"], "C.UTF-8");
    for path in ["/b", "/b/profile"] {
        let mut invalid = literal.clone();
        invalid
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown_member".into(), json!(true));
        let frame: Cmd = serde_json::from_value(invalid).unwrap();
        assert!(frame.command().is_err());
    }
    assert!(
        serde_json::from_value::<foreign::McpServerSpec>(json!({
            "transport":"stdio","command":["example-server"],"unexpected_launch_option":true
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<foreign::EndpointOptions>(json!({"invented_header_option":true}))
            .is_err()
    );
}

#[test]
fn a_started_item_uses_its_adjacent_kind_and_a_partial_update_keeps_only_the_changed_members() {
    let body = json!({"turn_id":ID,"native_ref":"message-1","stream":"main","kind":"agent_message",
        "payload":{"text":"Hello 你好","text_len":8,"phase":"final"}});
    let event = Event::decode("item.started", body.clone()).unwrap();
    assert_eq!(event.body().unwrap(), body);
    let Event::ItemStarted(start) = event else {
        panic!("wrong event");
    };
    assert!(matches!(
        start.payload,
        foreign::ItemPayload::AgentMessage(_)
    ));
    let mut invalid = body;
    invalid["kind"] = json!("tool_call");
    assert!(Event::decode("item.started", invalid).is_err());
    let changes = json!({"native_ref":"message-1","payload":{"text_len":12}});
    assert_eq!(
        Event::decode("item.updated", changes.clone())
            .unwrap()
            .body()
            .unwrap(),
        changes
    );
}

#[test]
fn file_observations_and_usage_keep_the_same_fields_across_the_actual_link_envelope() {
    let stat = json!({"name":"会議.txt","path":"work/会議.txt","type":"file","size":7,
        "mtime":"2026-10-14T15:00:00.000Z","mode":420,"link_target":null,"cached":true,
        "hash":orch_types::Hash::of(b"content"),"media_type":"text/plain","is_text":true});
    let value: foreign::FileStat = serde_json::from_value(stat.clone()).unwrap();
    let api: orch_api_contract::workspace::FileStat = value;
    assert_eq!(api.entry.size, 7);
    assert_eq!(serde_json::to_value(api).unwrap(), stat);
    let changed = json!({"watch_id":ID,"changed":[{"name":"会議.txt","path":"work/会議.txt","type":"file","size":7,
        "mtime":"2026-10-14T15:00:00.000Z","mode":420,"link_target":null,"cached":true}],"removed":[]});
    assert_eq!(
        Report::decode("fs.changed", changed.clone())
            .unwrap()
            .body()
            .unwrap(),
        changed
    );
    let usage = json!({"lease_id":ID,"model":"example","usage":{"input_tokens":30,"cached_input_tokens":10,
        "cache_write_tokens":5,"output_tokens":4,"reasoning_tokens":1,"total_tokens":34},"request_ref":"native-1"});
    let event = Event::decode("lease.usage", usage.clone()).unwrap();
    assert_eq!(event.body().unwrap(), usage);
    let Event::LeaseUsage(told) = event else {
        panic!("wrong event");
    };
    let same: orch_agent_contract::events::TokenUsage = told.usage;
    assert_eq!(same.cache_write_tokens, 5);
    assert!(same.is_consistent());
    // Older retained reports can omit counts; the reader's previous zero behavior survives.
    let old: foreign::TokenUsage =
        serde_json::from_value(json!({"input_tokens":2,"total_tokens":2})).unwrap();
    assert_eq!(old.cached_input_tokens, 0);
}

#[test]
fn a_runtime_recipe_keeps_its_command_bound_sealed_credential_and_refuses_plaintext() {
    let recipe = json!({"runtime_id":ID,"kind":"environment","name":"orch-env-example","driver":"host",
        "spec":{},"resources":{},"roles":["environment"],"gateway_url":"wss://gateway.example",
        "enroll_token":{"$sealed":"sealed-for-this-command"},"ca_cert":"certificate","tools":[],
        "volumes":{},"mounts":[],"env":{},"labels":{}});
    let typed: foreign::RuntimeSpec = serde_json::from_value(recipe.clone()).unwrap();
    let preserved = serde_json::to_value(typed).unwrap();
    assert_eq!(preserved["enroll_token"], recipe["enroll_token"]);
    let mut invalid = recipe.clone();
    invalid["enroll_token"] = Value::String("an unsealed token".into());
    assert!(serde_json::from_value::<foreign::RuntimeSpec>(invalid).is_err());
    let mut invalid = recipe;
    invalid["unknown_launch_member"] = json!(true);
    assert!(serde_json::from_value::<foreign::RuntimeSpec>(invalid).is_err());
}
