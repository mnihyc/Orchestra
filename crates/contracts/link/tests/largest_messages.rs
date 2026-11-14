//! The largest message of every kind of command, the body the system sends and the answer it receives.
//!
//! Each kind is decided in [`largest`], a `match` without a wildcard, so that a kind added to the catalogue
//! without a decision does not build. A decision is one of: nothing in the message can grow; what grows is names
//! or the description of one setup; what grows is capped by a constant, and the cap is less than half of a message
//! of the default; the program that makes the message cuts it to what the session carries; or the message can grow
//! past the default, and the kind says in what case. A case that is open is refused where it is sent, with its
//! size and the setting (`large_messages` of the link proves it), and is listed here so that none is found by
//! meeting it.
//!
//! Sizes are the written bytes of the bound, taking a name, a path or a line as it is written: the escapes that
//! JSON puts before control characters are not counted, because a name or a line made of them is not a use of the
//! product.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_contract::common::MAX_JSON_BYTES;
use orch_api_contract::native::PAGE_BYTES;
use orch_api_contract::threads::INPUT_TEXT_MAX_CHARS;
use orch_api_contract::workspace::{
    FILE_MATCH_TEXT_CHARS, FILE_SEARCH_LIMIT_MAX, GIT_DIFF_MAX_BYTES,
};
use orch_driver_contract::sources::DOCKERFILE_MAX_BYTES;
use orch_link_contract::commands::{Command, CommandKind, CommandResult};
use orch_link_contract::version::Limits;
use orch_provider_contract::limits::REPORT_MAX_BYTES;
use serde_json::Value;

/// What decides how large the largest message of a kind is.
enum Largest {
    /// Identifiers, numbers, flags, enumerations and moments only: nothing in it can grow.
    ByType,
    /// Text that is a name, a path, an address, a reference, a version or a line of words, and short lists of
    /// them: kilobytes.
    Names,
    /// A list of identifiers, forty bytes each: some four hundred thousand of them pass the default.
    Ids,
    /// The description of one thing that is set up or changed (a session, a unit, an environment, a tool server,
    /// the parameters of a model): records made from the settings a person chose and from the catalogue, tens to
    /// hundreds of kilobytes.
    Setup,
    /// A constant caps what grows: `bytes` is the product the cap allows, which must be less than half of a
    /// message, the rest being names and setup.
    Capped { bytes: usize, by: &'static str },
    /// The program that makes the message cuts it to what the session carries.
    Trimmed(&'static str),
    /// It can pass the default, in the case the text says.
    Open(&'static str),
}

fn bytes_of(figure: i64) -> usize {
    usize::try_from(figure).unwrap()
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
        | K::PortPublish => Largest::Names,
        K::PtyStatus | K::FsRecordPrune | K::NodeKeptSweep => Largest::Ids,
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
        | K::NodeProbe => Largest::Setup,
        K::GitDiff => Largest::Capped {
            bytes: bytes_of(GIT_DIFF_MAX_BYTES),
            by: "the text of a diff is cut at GIT_DIFF_MAX_BYTES",
        },
        K::FsSearch => Largest::Capped {
            bytes: FILE_SEARCH_LIMIT_MAX as usize
                * (4_096 + FILE_MATCH_TEXT_CHARS as usize * 4 + 256),
            by: "at most FILE_SEARCH_LIMIT_MAX matches, each a path of at most 4,096 bytes and a line cut to FILE_MATCH_TEXT_CHARS characters",
        },
        K::TurnStart => Largest::Capped {
            bytes: INPUT_TEXT_MAX_CHARS as usize * 4,
            by: "the text of an input is at most INPUT_TEXT_MAX_CHARS characters, of at most four bytes each; the rest of the body is a setup",
        },
        K::TurnSteer => Largest::Capped {
            bytes: INPUT_TEXT_MAX_CHARS as usize * 4,
            by: "the text of an input is at most INPUT_TEXT_MAX_CHARS characters, of at most four bytes each",
        },
        K::TurnStartFenced => Largest::Capped {
            bytes: INPUT_TEXT_MAX_CHARS as usize * 4,
            by: "the text of an input is at most INPUT_TEXT_MAX_CHARS characters, of at most four bytes each; the rest of the body is a setup",
        },
        K::ProviderRefresh => Largest::Capped {
            bytes: 2 * REPORT_MAX_BYTES,
            by: "what a source reports is refused above REPORT_MAX_BYTES, and it is written once as read and once as kept",
        },
        K::ProviderResetConsume => Largest::Capped {
            bytes: 2 * REPORT_MAX_BYTES,
            by: "what a source reports is refused above REPORT_MAX_BYTES, and it is written once as read and once as kept",
        },
        K::ProviderDiscoverModels => Largest::Capped {
            bytes: 2 * REPORT_MAX_BYTES,
            by: "a models list is refused above REPORT_MAX_BYTES, and it is written once as read and once as kept",
        },
        K::LeaseOpen => Largest::Capped {
            bytes: 2 * REPORT_MAX_BYTES,
            by: "it carries the parameters of the models a list of at most REPORT_MAX_BYTES holds, with what the catalogue adds once more",
        },
        K::SessionBackground => Largest::Capped {
            bytes: PAGE_BYTES,
            by: "a page of the roster is cut at PAGE_BYTES",
        },
        K::NodeImageEnsure => Largest::Capped {
            bytes: DOCKERFILE_MAX_BYTES,
            by: "the Dockerfile of an image is at most DOCKERFILE_MAX_BYTES; the rest of the specification is names",
        },
        K::NodeTemplateImport => Largest::Capped {
            bytes: 2 * MAX_JSON_BYTES,
            by: "what a person gives is one request of at most MAX_JSON_BYTES, and the proposed specification is made from it",
        },
        K::LeaseClose => Largest::Trimmed(
            "the connector keeps the usage groups that fit a message of the session it answers on and the exact totals (`fitting_close_groups`)",
        ),
        K::AgentDiscoverSessions => Largest::Open(
            "one row of about 300 bytes for each conversation a harness holds on the machine: a harness that holds some fifty thousand passes the default",
        ),
        K::AgentHistoryManage => Largest::Open(
            "one source in the body and one fact in the answer for each native session a person chose to measure or remove: tens of thousands of sessions pass the default",
        ),
        K::AgentHistoryHomeCleanup => Largest::Open(
            "one row for each leftover directory of a closed home: tens of thousands of directories pass the default",
        ),
        K::OrchToolResult => Largest::Open(
            "the answer of one of the system's own tools: the tools that can make a long one cut it with `tool_answers::fits`, at 40,000 bytes at most, and one that does not is as long as it makes it",
        ),
        K::ProviderComplete => Largest::Open(
            "the text a model is asked to read grows with its context window: a window of some four million tokens passes the default (a million tokens is about four megabytes)",
        ),
        K::ProviderGenerate => Largest::Open(
            "the whole dialogue of one step grows with the model's context window: a window of some four million tokens passes the default (a million tokens is about four megabytes)",
        ),
        K::ProviderService => Largest::Open(
            "a request carries its images inline: an image of some twelve megabytes, written as base64, passes the default",
        ),
        K::ProviderEmbed => Largest::Open(
            "the answer holds one vector, about sixteen kilobytes for three thousand numbers, for each text of a batch: a batch of some one thousand texts passes the default",
        ),
        K::FsList => Largest::Open(
            "at most FS_LIST_MAX_ENTRIES (5,000) entries, each with a name, a path and a link target: entries of more than about 3,300 bytes each pass the default",
        ),
        K::FsArchiveList => Largest::Open(
            "at most ARCHIVE_LIST_LIMIT_MAX (5,000) entries, each a path: paths of more than about 3,300 bytes pass the default",
        ),
        K::FsRestorePlan => Largest::Open(
            "one row of about 200 bytes for every path a restore changes (a snapshot holds up to SNAPSHOT_MAX_ENTRIES, 250,000): a restore that changes some eighty thousand paths passes the default; the whole plan is kept beside the session as `plan_blob`",
        ),
        K::GitStatus => Largest::Open(
            "at most STATUS_MAX_CHANGES (20,000) changes, each with a path: paths averaging more than about 800 bytes pass the default",
        ),
        K::GitCommit => Largest::Open(
            "one path for each change a person selected, of the at most 20,000 a status lists: paths averaging more than about 800 bytes pass the default",
        ),
        K::GitPush => Largest::Open(
            "the answer holds the status of the repository, which lists at most 20,000 changes: paths averaging more than about 800 bytes pass the default",
        ),
        K::ProcList => Largest::Open(
            "one row of about 400 bytes for each process of the environment: some forty thousand processes pass the default",
        ),
        K::NodeResourcesList => Largest::Open(
            "one row for each object the installation owns on a host, with what depends on it: a host with some tens of thousands of objects passes the default",
        ),
    }
}

#[test]
fn every_kind_of_command_fits_a_message_of_the_default_or_says_in_what_case_it_does_not() {
    let limit = Limits::default().message_bytes();
    let mut open = Vec::new();
    for kind in CommandKind::ALL {
        match largest(*kind) {
            Largest::ByType | Largest::Names | Largest::Ids | Largest::Setup => {}
            Largest::Capped { bytes, by } => {
                assert!(!by.is_empty(), "{kind}");
                assert!(
                    bytes * 2 <= limit,
                    "{kind}: {by} allows {bytes} bytes, and half of a message of the default is {}",
                    limit / 2
                );
            }
            Largest::Trimmed(how) => assert!(!how.is_empty(), "{kind}"),
            Largest::Open(case) => {
                assert!(!case.is_empty(), "{kind}");
                open.push(kind.as_str());
            }
        }
    }
    // Whoever opens another kind to the default says so here, in the same change as the kind's reason.
    assert_eq!(
        open,
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
