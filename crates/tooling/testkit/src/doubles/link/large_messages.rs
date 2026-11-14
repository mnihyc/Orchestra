//! Typed examples whose requests or replies exceed a whole message, without live providers or workspaces.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "synthetic contract examples fail by panicking"
)]

use orch_link_contract::commands::{Cmd, CmdRes, Command, CommandKind, CommandResult};
use orch_link_contract::frames::Frame;
use orch_types::{Json, new_id};
use serde_json::json;

/// The previously uncovered kinds, in catalogue order.
pub const KINDS: [CommandKind; 16] = [
    CommandKind::AgentDiscoverSessions,
    CommandKind::AgentHistoryManage,
    CommandKind::AgentHistoryHomeCleanup,
    CommandKind::OrchToolResult,
    CommandKind::ProviderComplete,
    CommandKind::ProviderGenerate,
    CommandKind::ProviderService,
    CommandKind::ProviderEmbed,
    CommandKind::FsList,
    CommandKind::FsArchiveList,
    CommandKind::GitStatus,
    CommandKind::GitCommit,
    CommandKind::GitPush,
    CommandKind::FsRestorePlan,
    CommandKind::ProcList,
    CommandKind::NodeResourcesList,
];

/// Repeated distinct rows, with enough real serialized content to pass `minimum`.
fn rows(minimum: usize, mut row: impl FnMut(usize) -> Json) -> Json {
    let first = row(0);
    let count = minimum / serde_json::to_vec(&first).unwrap().len() + 2;
    Json::Array((0..count).map(&mut row).collect())
}

fn status(minimum: usize) -> Json {
    json!({"available":true, "reason":null, "repository_dir":".", "status_hash":null,
        "remotes":[], "upstream":null, "branch":"main", "head":null, "ahead":0, "behind":0,
        "changes": rows(minimum, |n| json!({"path":format!("{}/file-{n}", "dir/".repeat(240)), "index":" ", "worktree":"M"}))})
}

/// A complete typed frame with at least `minimum` bytes of content. Request examples are decoded
/// through the catalogue; reply examples through their exact result type, so missing fields cannot hide.
#[must_use]
pub fn example(kind: CommandKind, minimum: usize) -> Frame {
    use CommandKind as K;
    let id = new_id();
    let at = "2026-11-14T12:00:00.000Z";
    let hash = "0".repeat(64);
    let request = match kind {
        K::AgentHistoryManage => Some(
            json!({"provider_id":new_id(), "agent_id":new_id(), "remove":false,
            "sources":rows(minimum, |n| json!({"source":{"thread_id":new_id(), "segment_id":new_id(), "native_session_ref":format!("session-{n}-{}", "x".repeat(256))}, "started_by_orch":true}))}),
        ),
        K::OrchToolResult => Some(
            json!({"thread_id":new_id(), "call_ref":"fixture-call", "result":{"text":"x".repeat(minimum)}}),
        ),
        K::ProviderComplete => Some(
            json!({"provider_id":new_id(), "purpose":"summary", "protocol":"openai", "model_name":"fixture-model", "system":"Summarize", "input":"x".repeat(minimum), "max_output_tokens":10}),
        ),
        K::ProviderGenerate => Some(
            json!({"provider_id":new_id(), "protocol":"openai", "model_name":"fixture-model", "params":null, "model_parameters":{}, "input":{"system":"x".repeat(minimum), "messages":[], "tools":[], "max_output_tokens":10}}),
        ),
        K::ProviderService => Some(
            json!({"provider_id":new_id(), "offering":"fixture", "capability":"image_generation", "input":{"images":["x".repeat(minimum)]}, "definition":hash}),
        ),
        K::ProviderEmbed => Some(
            json!({"provider_id":new_id(), "protocol":"openai", "model_name":"fixture-model", "inputs":rows(minimum, |n| json!(format!("input-{n}-{}", "x".repeat(256))))}),
        ),
        K::GitCommit => Some(
            json!({"environment_id":new_id(), "job_id":new_id(), "path":".", "paths":rows(minimum, |n| json!(format!("{}/file-{n}", "dir/".repeat(240)))), "message":"Save the selected files", "expected_head":null, "expected_status":hash, "author":{"name":"Fixture", "email":"fixture@example.invalid"}}),
        ),
        _ => None,
    };
    if let Some(body) = request {
        let typed = Command::decode(kind.as_str(), body).expect("a valid command example");
        return Frame::Cmd(Cmd {
            id,
            kind: kind.as_str().into(),
            key: None,
            deadline: at.parse().unwrap(),
            body: typed.body().unwrap(),
        });
    }
    let body = match kind {
        K::AgentDiscoverSessions => {
            json!({"sessions":rows(minimum, |n| json!({"native_session_ref":format!("session-{n}"), "title":"x".repeat(256), "cwd":"project", "updated_at":at, "adopted_thread_id":null}))})
        }
        K::AgentHistoryHomeCleanup => {
            json!({"items":rows(minimum, |n| json!({"directory":format!("cache-{n}-{}", "x".repeat(256)), "files":1, "bytes":1024, "error":null}))})
        }
        K::FsList => {
            json!({"entries":rows(minimum, |n| json!({"name":format!("file-{n}"), "path":format!("{}/file-{n}", "dir/".repeat(1000)), "type":"file", "size":1, "mtime":at, "mode":420, "link_target":null, "cached":false})), "truncated":false})
        }
        K::FsArchiveList => {
            json!({"format":"zip", "compression":null, "entries":rows(minimum, |n| json!({"path":format!("{}/file-{n}", "dir/".repeat(1000)), "kind":"file", "size":1, "modified_at":at})), "truncated":false, "encrypted":false})
        }
        K::GitStatus => status(minimum),
        K::GitPush => json!({"status":status(minimum)}),
        K::FsRestorePlan => {
            json!({"plan_blob":hash, "paths":rows(minimum, |n| json!({"path":format!("{}/file-{n}", "dir/".repeat(24)), "action":"add", "expected":"none", "later_change":null, "later_writers":[]})), "writers_active":false, "undo_available":true})
        }
        K::ProcList => {
            json!({"as_of":at, "processes":rows(minimum, |n| json!({"pid":n+1, "ppid":1, "name":"worker", "command":"x".repeat(256), "user":"fixture", "state":"running", "cpu_percent":0.0, "memory_bytes":1024, "started_at":at, "thread_id":null, "item_id":null}))})
        }
        K::NodeResourcesList => {
            json!({"observed_at":at, "resources":rows(minimum, |n| json!({"resource":{"kind":"volume", "id":format!("volume-{n}")}, "name":"x".repeat(256), "size_bytes":1024, "labels":{}, "depends_on":[], "last_used_at":at, "busy":false})), "unavailable":[]})
        }
        _ => panic!("{kind} has no large example"),
    };
    let typed = CommandResult::decode(kind, body).expect("a valid reply example");
    Frame::CmdRes(CmdRes {
        id,
        outcome: Ok(typed.body().unwrap()),
    })
}
