//! Every command has an executable carriage decision for both its request and its answer.
//!
//! Fixed shapes are checked against the actual schemas. Growing shapes use the transport's
//! byte admission, proved with serialized boundary frames instead of assumptions about the
//! length of names, paths, identifier lists or setup data. Kinds that need more than one
//! message are listed explicitly and bound to the production paging policy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_link_contract::commands::{Cmd, CmdRes, Command, CommandKind, CommandResult};
use orch_link_contract::frames::{self, Frame, FrameError};
use orch_link_contract::version::Limits;
use orch_types::new_id;
use serde_json::{Value, json};

/// The mechanism whose executable contract the guard checks.
enum Largest {
    /// Only identifiers, numbers, flags, enumerations and moments; schemas must prove it.
    ByType,
    /// The exact serialized bytes are admitted by the transport; no name or row count is a byte bound.
    Checked,
    /// The logical value spans message allowances through the existing parts transport.
    Paged(&'static str),
}

fn largest(kind: CommandKind) -> Largest {
    use CommandKind as K;
    match kind {
        K::BrowserClose
        | K::DisplayClose
        | K::PtyClose
        | K::AgentStop
        | K::SessionCloseFenced
        | K::SessionClose
        | K::SessionResync
        | K::TurnInterrupt
        | K::ProviderLoginCancel
        | K::ProviderLogout
        | K::ProviderStop
        | K::EnvDetach
        | K::EnvDataRemove
        | K::McpForget
        | K::FsUnwatch
        | K::FsRecordVerify
        | K::PortsLook
        | K::ProcSignal
        | K::ProcSuspend
        | K::ProcResume
        | K::ProcSuspendOwned
        | K::ProcResumeOwned
        | K::WatchSet
        | K::NodeRuntimeStart
        | K::NodeRuntimeStop
        | K::NodeKeepEstimate
        | K::PortClose => Largest::ByType,
        K::MachineProbe
        | K::MachineUpdateNow
        | K::MachinePolicySet
        | K::MachinePrograms
        | K::MachineRolesSet
        | K::UnitAfterUp
        | K::PtyOpen
        | K::ScreenLook
        | K::BrowserOpen
        | K::BrowserAdopt
        | K::BrowserProfile
        | K::DisplayOpen
        | K::DisplayAdopt
        | K::AgentPtyOpen
        | K::AgentReadSession
        | K::SessionPrefixInspect
        | K::SessionContinuationClose
        | K::SessionBackgroundEnd
        | K::SessionContext
        | K::SessionExport
        | K::ProviderLoginInput
        | K::ProviderCredentialIssue
        | K::ProviderCredentialAccept
        | K::AgentCredentialReturn
        | K::AgentCredentialSet
        | K::ExecStart
        | K::McpStop
        | K::FsStat
        | K::FsFetch
        | K::FsWrite
        | K::FsMkdir
        | K::FsRename
        | K::FsRemove
        | K::FsCopy
        | K::FsArchiveRead
        | K::FsManifest
        | K::FsSnapshot
        | K::FsMeasure
        | K::FsMaterialize
        | K::GitPrepare
        | K::GitPrepareCleanup
        | K::GitWorktree
        | K::GitWorktreeCleanup
        | K::GitWorktreeRepair
        | K::GitWorktreeCopyPlan
        | K::GitWorktreeSnapshot
        | K::GitWorktreeStandalone
        | K::FsRecord
        | K::FsRecordRead
        | K::FsDataMeasure
        | K::FsDataExport
        | K::FsDataImport
        | K::FsRestore
        | K::WorkspaceObserve
        | K::NodeResourcesInspect
        | K::NodeResourcesRemove
        | K::NodeKeepFiles
        | K::NodeKeptRemove
        | K::NodeKeptExport
        | K::NodeKeptImport
        | K::NodeVolumeEnsure
        | K::NodeVolumeInspect
        | K::NodeVolumeRemove
        | K::NodeRuntimeExport
        | K::NodeRuntimeImport
        | K::NodeRuntimeVolumeImport
        | K::NodeVolumeCarryCheck
        | K::NodeMountMeasure
        | K::NodeMountFingerprint
        | K::NodeMountExport
        | K::NodeMountImport
        | K::NodeToolEnsure
        | K::NodeToolCheck
        | K::NodeToolConformance
        | K::NodeSkillFetch
        | K::HostPorts
        | K::PortPublish => Largest::Checked,
        K::PtyStatus | K::FsRecordPrune | K::NodeKeptSweep => Largest::Checked,
        K::AgentStart
        | K::SessionOpen
        | K::SessionPrefixOpen
        | K::SessionContinuationResume
        | K::SessionOpenFenced
        | K::SessionMcpReload
        | K::SessionNativeActions
        | K::ApprovalResolve
        | K::ProviderLoginStart
        | K::ProviderStart
        | K::EnvAttach
        | K::McpStart
        | K::McpRestart
        | K::McpImport
        | K::NodeRuntimeCreate
        | K::NodeRuntimeRemove
        | K::NodeNetworkApply
        | K::NodeMountsApply
        | K::NodeCapabilitiesApply
        | K::NodeResourcesApply
        | K::NodeRuntimeVolumesMeasure
        | K::NodeRuntimeVolumesExport
        | K::NodeProbe => Largest::Checked,
        K::GitDiff => Largest::Checked,
        K::FsSearch => Largest::Checked,
        K::TurnStart => Largest::Checked,
        K::TurnSteer => Largest::Checked,
        K::TurnStartFenced => Largest::Checked,
        K::ProviderRefresh => Largest::Checked,
        K::ProviderResetConsume => Largest::Checked,
        K::ProviderDiscoverModels => Largest::Checked,
        K::LeaseOpen => Largest::Checked,
        K::SessionBackground => Largest::Checked,
        K::NodeImageEnsure => Largest::Checked,
        K::NodeTemplateImport => Largest::Checked,
        K::LeaseClose => Largest::Checked,
        K::AgentDiscoverSessions => Largest::Paged(
            "one row of about 300 bytes for each conversation a harness holds on the machine: a harness that holds some fifty thousand passes the default",
        ),
        K::AgentHistoryManage => Largest::Paged(
            "one source in the body and one fact in the answer for each native session a person chose to measure or remove: tens of thousands of sessions pass the default",
        ),
        K::AgentHistoryHomeCleanup => Largest::Paged(
            "one row for each leftover directory of a closed home: tens of thousands of directories pass the default",
        ),
        K::OrchToolResult => Largest::Paged(
            "the answer of one of the system's own tools: the tools that can make a long one cut it with `tool_answers::fits`, at 40,000 bytes at most, and one that does not is as long as it makes it",
        ),
        K::ProviderComplete => Largest::Paged(
            "the text a model is asked to read grows with its context window: a window of some four million tokens passes the default (a million tokens is about four megabytes)",
        ),
        K::ProviderGenerate => Largest::Paged(
            "the whole dialogue of one step grows with the model's context window: a window of some four million tokens passes the default (a million tokens is about four megabytes)",
        ),
        K::ProviderService => Largest::Paged(
            "a request carries its images inline: an image of some twelve megabytes, written as base64, passes the default",
        ),
        K::ProviderEmbed => Largest::Paged(
            "the answer holds one vector, about sixteen kilobytes for three thousand numbers, for each text of a batch: a batch of some one thousand texts passes the default",
        ),
        K::FsList => Largest::Paged(
            "at most FS_LIST_MAX_ENTRIES (5,000) entries, each with a name, a path and a link target: entries of more than about 3,300 bytes each pass the default",
        ),
        K::FsArchiveList => Largest::Paged(
            "at most ARCHIVE_LIST_LIMIT_MAX (5,000) entries, each a path: paths of more than about 3,300 bytes pass the default",
        ),
        K::FsRestorePlan => Largest::Paged(
            "one row of about 200 bytes for every path a restore changes (a snapshot holds up to SNAPSHOT_MAX_ENTRIES, 250,000): a restore that changes some eighty thousand paths passes the default; the whole plan is kept beside the session as `plan_blob`",
        ),
        K::GitStatus => Largest::Paged(
            "at most STATUS_MAX_CHANGES (20,000) changes, each with a path: paths averaging more than about 800 bytes pass the default",
        ),
        K::GitCommit => Largest::Paged(
            "one path for each change a person selected, of the at most 20,000 a status lists: paths averaging more than about 800 bytes pass the default",
        ),
        K::GitPush => Largest::Paged(
            "the answer holds the status of the repository, which lists at most 20,000 changes: paths averaging more than about 800 bytes pass the default",
        ),
        K::ProcList => Largest::Paged(
            "one row of about 400 bytes for each process of the environment: some forty thousand processes pass the default",
        ),
        K::NodeResourcesList => Largest::Paged(
            "one row for each object the installation owns on a host, with what depends on it: a host with some tens of thousands of objects passes the default",
        ),
    }
}

/// The same byte encoder is used by the session queue and command-result fitting. The payload
/// fills the exact remaining wire allowance, including the envelope and JSON escaping.
fn assert_byte_boundary(
    mut frame: Frame,
    limit: usize,
    payload: impl Fn(&mut Frame) -> &mut Value,
) {
    let overhead = frames::encode(&frame, usize::MAX).unwrap().len();
    *payload(&mut frame) = json!("x".repeat(limit - overhead));
    assert_eq!(frames::encode(&frame, limit).unwrap().len(), limit);
    let Value::String(text) = payload(&mut frame) else {
        panic!("the boundary payload is text")
    };
    text.push('x');
    assert!(
        matches!(frames::encode(&frame, limit), Err(FrameError::TooLarge { bytes, max })
        if bytes == limit + 1 && max == limit)
    );
}

#[test]
fn every_growing_kind_is_checked_at_the_exact_serialized_byte_boundary() {
    let limit = Limits::default().message_bytes();
    for kind in CommandKind::ALL {
        if !matches!(largest(*kind), Largest::Checked) {
            continue;
        }
        // Cmd accepts neutral JSON until the catalogue decodes it; transport admission must
        // account for every byte even if a body is later refused for a different reason.
        let request = Frame::Cmd(Cmd {
            id: new_id(),
            kind: kind.as_str().into(),
            key: None,
            deadline: "2026-11-14T12:00:00.000Z".parse().unwrap(),
            body: json!(""),
        });
        assert_byte_boundary(request, limit, |frame| {
            let Frame::Cmd(cmd) = frame else {
                panic!("a command")
            };
            &mut cmd.body
        });
        let reply = Frame::CmdRes(CmdRes::ok(new_id(), &"").unwrap());
        assert_byte_boundary(reply, limit, |frame| {
            let Frame::CmdRes(result) = frame else {
                panic!("a command reply")
            };
            result.outcome.as_mut().unwrap()
        });
    }
}

#[test]
fn every_kind_has_an_executable_carriage_decision_and_the_paged_kinds_are_named() {
    let paged: Vec<_> = CommandKind::ALL
        .iter()
        .filter_map(|kind| {
            let decision = largest(*kind);
            assert_eq!(
                kind.is_paged(),
                matches!(decision, Largest::Paged(_)),
                "{kind}"
            );
            match decision {
                Largest::ByType | Largest::Checked => None,
                Largest::Paged(case) => {
                    assert!(!case.is_empty(), "{kind}");
                    Some(kind.as_str())
                }
            }
        })
        .collect();
    assert_eq!(
        paged,
        [
            "agent.discover_sessions",
            "agent.history.manage",
            "agent.history.home_cleanup",
            "orch.tool_result",
            "provider.complete",
            "provider.generate",
            "provider.service",
            "provider.embed",
            "fs.list",
            "fs.archive.list",
            "git.status",
            "git.commit",
            "git.push",
            "fs.restore.plan",
            "proc.list",
            "node.resources.list",
        ]
    );
}

/// What in a schema can grow without a bound: a text with no format, pattern or choice, a list, a map, or a value
/// of any shape; the place of the first it finds.
fn growth(schema: &Value, root: &Value, followed: &mut Vec<String>, at: &str) -> Option<String> {
    let object = match schema {
        Value::Bool(true) => return Some(format!("{at}: a value of any shape")),
        Value::Object(object) => object,
        _ => return None,
    };
    if let Some(Value::String(reference)) = object.get("$ref") {
        if followed.contains(reference) {
            return Some(format!("{at}: refers to itself"));
        }
        let Some(target) = reference
            .strip_prefix('#')
            .and_then(|pointer| root.pointer(pointer))
        else {
            return Some(format!("{at}: {reference} is not in the schema"));
        };
        followed.push(reference.clone());
        let found = growth(target, root, followed, at);
        followed.pop();
        return found;
    }
    let types: Vec<&str> = match object.get("type") {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let chosen = ["format", "pattern", "enum", "const", "maxLength"]
        .iter()
        .any(|keyword| object.contains_key(*keyword));
    if types.contains(&"string") && !chosen {
        return Some(format!("{at}: a text"));
    }
    if types.contains(&"array") {
        return Some(format!("{at}: a list"));
    }
    if let Some(extra) = object.get("additionalProperties")
        && *extra != Value::Bool(false)
    {
        return Some(format!("{at}: a map"));
    }
    if let Some(Value::Object(properties)) = object.get("properties") {
        for (name, property) in properties {
            if let Some(found) = growth(property, root, followed, &format!("{at}.{name}")) {
                return Some(found);
            }
        }
    }
    for keyword in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(alternatives)) = object.get(keyword) {
            for alternative in alternatives {
                if let Some(found) = growth(alternative, root, followed, at) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// The schema of the body (`Command`) or of the answer (`CommandResult`) of one kind, and the schema around it.
fn schema_of(kind: CommandKind, answer: bool) -> (Value, Value) {
    let root = if answer {
        serde_json::to_value(schemars::schema_for!(CommandResult)).unwrap()
    } else {
        serde_json::to_value(schemars::schema_for!(Command)).unwrap()
    };
    let variant = root["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|variant| {
            variant
                .pointer("/properties/k/const")
                .and_then(Value::as_str)
                == Some(kind.as_str())
                || variant
                    .pointer("/properties/k/enum/0")
                    .and_then(Value::as_str)
                    == Some(kind.as_str())
        })
        .unwrap_or_else(|| panic!("{kind} has no variant in the schema"));
    let content = variant
        .pointer("/properties/b")
        .unwrap_or_else(|| panic!("{kind} has no content in the schema"))
        .clone();
    (content, root)
}

#[test]
fn a_kind_decided_by_its_type_has_nothing_in_its_body_or_its_answer_that_can_grow() {
    for kind in CommandKind::ALL {
        if !matches!(largest(*kind), Largest::ByType) {
            continue;
        }
        for answer in [false, true] {
            let (content, root) = schema_of(*kind, answer);
            let side = if answer { "answer" } else { "body" };
            if let Some(found) = growth(&content, &root, &mut Vec::new(), side) {
                panic!("{kind} is decided by its type, and in its {found}");
            }
        }
    }
}

/// A bounded side has a direct source location as well as an executable schema proof.
fn fixed_paged_side(kind: CommandKind, answer: bool) -> Option<String> {
    use CommandKind as K;
    let name = match (kind, answer) {
        (K::AgentDiscoverSessions, false) => "AgentDiscoverSessions",
        (K::AgentHistoryHomeCleanup, false) => "AgentHistoryHomeCleanup",
        (K::ProcList, false) => "ProcList",
        (K::NodeResourcesList, false) => "NodeResourcesList",
        (K::OrchToolResult, true) => "Empty",
        _ => return None,
    };
    let source = include_str!("../src/commands.rs");
    let declaration = format!("pub struct {name}");
    let offset = source
        .find(&declaration)
        .expect("the named bound is its actual source type");
    Some(format!(
        "crates/contracts/link/src/commands.rs:{}",
        source[..offset].lines().count() + 1
    ))
}

#[test]
fn every_paged_request_and_reply_is_large_or_has_a_source_bound_checked_against_its_schema() {
    for kind in CommandKind::ALL
        .iter()
        .copied()
        .filter(|kind| kind.is_paged())
    {
        for answer in [false, true] {
            let (content, root) = schema_of(kind, answer);
            let growing = growth(&content, &root, &mut Vec::new(), "body");
            let side = if answer { "answer" } else { "request" };
            if let Some(source) = fixed_paged_side(kind, answer) {
                assert!(
                    growing.is_none(),
                    "{kind} {side} is bounded at {source}, but {growing:?}"
                );
                eprintln!("{kind} {side}: bounded by its schema at {source}");
            } else {
                assert!(
                    growing.is_some(),
                    "{kind} {side} needs a source bound if its growing field was removed"
                );
                eprintln!("{kind} {side}: parts beyond one message, typed case for this direction");
            }
        }
    }
}
