//! The schemas of the realtime contract, as the generator of the browser's types takes them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_contract::routes::{Shape, all};
use schemars::{SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value};

/// The settings the OpenAPI document is generated with: a schema says what is written, so a member that
/// may be `null` is always there and one that may be absent is left out.
fn generator() -> SchemaGenerator {
    let mut settings = SchemaSettings::draft2020_12().for_serialize();
    settings.definitions_path = "#/components/schemas/".into();
    settings.meta_schema = None;
    SchemaGenerator::new(settings)
}

fn realtime() -> Map<String, Value> {
    let mut generator = generator();
    orch_realtime_contract::schemas(&mut generator);
    generator.take_definitions(true)
}

fn api() -> Map<String, Value> {
    let mut generator = generator();
    for route in all() {
        for shape in [route.query, route.body, route.reply] {
            if let Shape::Json(write) = shape {
                write(&mut generator);
            }
        }
    }
    generator.take_definitions(true)
}

/// The variants of a union of frames, by their `op`.
fn variants<'a>(definitions: &'a Map<String, Value>, union: &str) -> Vec<(&'a str, &'a Value)> {
    definitions[union]["oneOf"]
        .as_array()
        .unwrap_or_else(|| panic!("{union} is not a union"))
        .iter()
        .map(|variant| {
            (
                variant["properties"]["op"]["const"]
                    .as_str()
                    .expect("a variant names its op"),
                variant,
            )
        })
        .collect()
}

fn names(list: &Value) -> Vec<&str> {
    list.as_array()
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

#[test]
fn every_frame_of_both_directions_is_a_variant_that_carries_its_op() {
    let definitions = realtime();
    let ops = |union: &str| {
        variants(&definitions, union)
            .into_iter()
            .map(|(op, _)| op)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ops("ClientFrame"),
        [
            "auth", "reauth", "sub", "unsub", "visible", "typing", "ping"
        ]
    );
    assert_eq!(
        ops("ServerFrame"),
        [
            "ready",
            "subscribed",
            "sync",
            "delta",
            "presence",
            "event",
            "resync",
            "closed",
            "error",
            "pong"
        ]
    );
    assert_eq!(ops("TerminalClientFrame"), ["resize", "ping"]);
    assert_eq!(
        ops("TerminalServerFrame"),
        ["ready", "exit", "closed", "pong"]
    );
    // The browser tells frames apart by `op`, so no variant may be a bare reference that hides it.
    for union in [
        "ClientFrame",
        "ServerFrame",
        "TerminalClientFrame",
        "TerminalServerFrame",
    ] {
        for (op, variant) in variants(&definitions, union) {
            assert!(variant.get("$ref").is_none(), "{union} {op} is a reference");
            assert!(
                names(&variant["required"]).contains(&"op"),
                "{union} {op} does not require op"
            );
        }
    }
}

#[test]
fn the_members_a_frame_has_to_have_are_required_and_those_that_may_be_left_out_are_not() {
    let definitions = realtime();
    // (union, op, required, may be left out)
    let table: [(&str, &str, &[&str], &[&str]); 12] = [
        ("ClientFrame", "auth", &["token"], &[]),
        (
            "ClientFrame",
            "sub",
            &["id", "feed"],
            &["since", "sub_streams"],
        ),
        ("ClientFrame", "unsub", &["id"], &[]),
        ("ClientFrame", "visible", &["feed", "visible"], &[]),
        ("ClientFrame", "typing", &["feed"], &["at"]),
        ("ClientFrame", "ping", &["n"], &[]),
        (
            "ServerFrame",
            "ready",
            &[
                "connection_id",
                "user_id",
                "server_time",
                "access_expires_at",
                "heartbeat_secs",
                "build",
            ],
            &[],
        ),
        (
            "ServerFrame",
            "subscribed",
            &["id", "feed", "seq", "mode"],
            &[],
        ),
        (
            "ServerFrame",
            "sync",
            &["id", "feed", "prev", "seq", "data"],
            &[],
        ),
        (
            "ServerFrame",
            "delta",
            &["id", "feed", "item_id", "field", "offset", "len", "data"],
            &[],
        ),
        ("ServerFrame", "error", &["code", "detail"], &["id"]),
        (
            "TerminalServerFrame",
            "ready",
            &["cols", "rows", "replay_bytes"],
            &[],
        ),
    ];
    for (union, op, required, optional) in table {
        let (_, variant) = variants(&definitions, union)
            .into_iter()
            .find(|(name, _)| *name == op)
            .unwrap();
        let has = names(&variant["required"]);
        for member in required {
            assert!(
                has.contains(member),
                "{union} {op}: {member} is not required"
            );
        }
        for member in optional {
            assert!(
                variant["properties"].get(member).is_some(),
                "{union} {op}: no {member}"
            );
            assert!(!has.contains(member), "{union} {op}: {member} is required");
        }
    }
}

#[test]
fn a_frame_a_client_sends_names_no_member_beyond_the_protocols() {
    let definitions = realtime();
    for (op, variant) in variants(&definitions, "ClientFrame") {
        assert_eq!(variant["additionalProperties"], false, "{op}");
    }
}

#[test]
fn the_data_of_every_feed_and_every_event_is_in_the_schema_by_name() {
    let definitions = realtime();
    for name in [
        "ThreadData",
        "TopicData",
        "EnvData",
        "OrgData",
        // A tree's readable row is flattened into the listing that carries it; the browser still names it.
        "TreeNode",
        "UserData",
        "ChatData",
        "DocData",
        "UserEvent",
        "TypingNotice",
        "ResourceEvent",
        "ResourceSampleOrOffline",
        "ProcsDataOrOffline",
        "TerminalsDataOrOffline",
        "EnvironmentPortsOrOffline",
        "GitDataOrOffline",
        "JobData",
        "ForkData",
        "DirDataOrOffline",
        "Offline",
        "RawEvent",
        "PresenceUser",
        "DeltaField",
        "SubscribeMode",
        "FeedClosedReason",
        "SocketErrorCode",
        "TerminalClosedReason",
    ] {
        assert!(
            definitions.contains_key(name),
            "{name} is not in the schema"
        );
    }
}

#[test]
fn no_type_of_the_socket_has_the_name_of_a_different_type_of_the_api() {
    let realtime = realtime();
    let api = api();
    let mut shared = 0;
    for (name, schema) in &realtime {
        if let Some(other) = api.get(name) {
            shared += 1;
            assert_eq!(
                schema, other,
                "{name} means one thing on the socket and another in the API"
            );
        }
    }
    // The rows the socket carries are the API's own, so there is something to compare.
    assert!(shared > 20, "only {shared} names are shared");
}

/// For looking at what the generator of the browser's types makes of these schemas:
/// `ORCH_REALTIME_SCHEMA_DUMP=/some/file cargo test -p orch-realtime-contract --test schemas`.
#[test]
fn the_schemas_are_written_where_they_are_asked_for() {
    let Ok(path) = std::env::var("ORCH_REALTIME_SCHEMA_DUMP") else {
        return;
    };
    let text = serde_json::to_string_pretty(&realtime()).unwrap();
    std::fs::write(path, text).unwrap();
}
