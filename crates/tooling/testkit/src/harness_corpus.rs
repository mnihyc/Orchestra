//! Offline harness upgrade evidence, shared by every program's real adapter tests.
//!
//! The format is documented in `tests/fixtures/harness/README.md` at the workspace root.
//! Protocol bytes are fixtures, never a second parser. Replay supplies those bytes to the
//! existing launcher seam; observations come from the adapter's actual event stream.

mod replay;
pub use replay::{Bindings, Transcript, collect_through, matches, replay_io};

use orch_agent_contract::adapter::AdapterEvent;
use orch_types::Json;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Every build accounts for every scenario, including an explicit reason where unsupported.
pub const SCENARIOS: &[&str] = &[
    "start",
    "resume",
    "fork",
    "tool_command",
    "tool_file_change",
    "tool_read",
    "tool_search",
    "tool_web_search",
    "tool_web_fetch",
    "tool_mcp",
    "tool_subagent",
    "tool_plan",
    "approval_command",
    "approval_file_change",
    "approval_permissions",
    "approval_plan",
    "approval_mcp",
    "approval_unknown",
    "approval_checkpoint",
    "approval_prompt_user_rule",
    "approval_network",
    "user_input",
    "interrupt",
    "steer",
    "compaction_automatic",
    "compaction_manual",
    "compaction_marker",
    "compaction_idle",
    "usage_limit",
    "rate_limit",
    "overloaded",
    "sign_in_expired",
    "sign_in_revoked",
    "context_overflow",
    "broken_stream",
    "process_exit",
    "background_end",
    "background_wake",
    "subagents",
    "mcp_startup",
    "mcp_failure",
    "mcp_elicitation",
    "hooks",
    "model_reroute",
    "settings_program",
    "settings_server",
    "session_close",
    "session_unload",
    "automatic_divergence",
    "tolerance_unknown_fields",
    "tolerance_unknown_event",
    "tolerance_unknown_method",
    "tolerance_unknown_enum",
    "tolerance_optional_fields",
    "tolerance_malformed",
];

/// The one checked-in corpus root; independent of the test process's working directory.
pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/harness")
}

/// Why evidence could not be read or did not agree with the real adapter.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CorpusError {
    /// An invalid, incomplete or accidentally unlisted corpus.
    #[error("invalid harness corpus: {0}")]
    Invalid(String),
    /// Reading or writing the fixture seam failed.
    #[error("harness replay I/O: {0}")]
    Io(String),
    /// Protocol bytes or neutral observations differ from the declared evidence.
    #[error("harness replay mismatch at {path}: expected {expected}, got {actual}")]
    Mismatch {
        path: String,
        expected: Json,
        actual: Json,
    },
    /// A native barrier did not arrive; no wall-clock recording is replayed.
    #[error("harness replay timed out at {0}")]
    Timeout(String),
}

impl From<std::io::Error> for CorpusError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
impl From<serde_json::Error> for CorpusError {
    fn from(error: serde_json::Error) -> Self {
        Self::Invalid(error.to_string())
    }
}

/// An observation in the real adapter's public wire vocabulary, without invented facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub name: String,
    pub body: Json,
}
impl Observation {
    /// Preserve every actual body member; expected patterns explicitly select stable facts.
    pub fn from_event(event: &AdapterEvent) -> Result<Self, CorpusError> {
        let (name, body) = event.to_wire()?;
        Ok(Self {
            name: name.to_owned(),
            body,
        })
    }
    /// The same observations from a public pure mapper, without a transport envelope.
    pub fn from_session(
        event: &orch_agent_contract::events::SessionEvent,
    ) -> Result<Self, CorpusError> {
        let value = serde_json::to_value(event)?;
        let name = value["kind"]
            .as_str()
            .ok_or_else(|| CorpusError::Invalid("session event has no kind".into()))?;
        Ok(Self {
            name: name.to_owned(),
            body: value["body"].clone(),
        })
    }

    /// Record an actual adapter call's return or canonical error, including startup/close facts.
    pub fn from_result<T: Serialize>(
        method: &str,
        result: &Result<T, orch_agent_contract::adapter::AdapterError>,
    ) -> Result<Self, CorpusError> {
        let body = match result {
            Ok(value) => {
                serde_json::json!({"method":method,"ok":true,"value":serde_json::to_value(value)?})
            }
            Err(error) => {
                serde_json::json!({"method":method,"ok":false,"error":{"code":error.code(),"message":error.detail(),"retryable":error.retryable()}})
            }
        };
        Ok(Self {
            name: "adapter.result".into(),
            body,
        })
    }
}

/// An ordered expected event. Object subsets must be requested explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedEvent {
    pub name: String,
    pub body: Json,
    #[serde(default)]
    pub subset: bool,
}

/// A managed-launch pin and the scenario proving divergence is refused if it arrives anyway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedPin {
    pub name: String,
    pub value: Json,
    pub divergence_scenario: String,
}

/// The exact published package supplying a derived fixture's protocol schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaPackage {
    pub name: String,
    pub version: String,
    pub sha256: String,
}

/// Where these bytes came from; written protocol fixtures never claim a real program recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Evidence {
    Recorded {
        references: Vec<String>,
    },
    PublishedProtocol {
        references: Vec<String>,
        package: SchemaPackage,
    },
    Synthetic {
        reason: String,
    },
}

/// Support is an assertion, not a silent missing file or an ignored test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Scenario {
    Supported {
        events: Vec<ExpectedEvent>,
        /// Non-event operations such as Close may deliberately produce no neutral event.
        #[serde(default)]
        quiet: bool,
        /// Explicitly irrelevant event kinds, never a wildcard or implicit omission.
        #[serde(default)]
        ignore_events: Vec<String>,
    },
    Unsupported {
        reason: String,
    },
}
impl Scenario {
    /// Check exact event order/count after the caller has awaited its native completion barrier.
    pub fn assert_observed(
        &self,
        observed: &[Observation],
        bindings: &mut Bindings,
    ) -> Result<(), CorpusError> {
        let Self::Supported {
            events,
            ignore_events,
            ..
        } = self
        else {
            return Err(CorpusError::Invalid(
                "cannot replay an unsupported scenario".into(),
            ));
        };
        let observed = observed
            .iter()
            .filter(|event| !ignore_events.contains(&event.name))
            .collect::<Vec<_>>();
        if observed.len() != events.len() {
            return Err(CorpusError::Mismatch {
                path: "event count".into(),
                expected: Json::from(events.len()),
                actual: Json::from(observed.len()),
            });
        }
        for (at, (expected, actual)) in events.iter().zip(observed).enumerate() {
            if expected.name != actual.name {
                return Err(CorpusError::Mismatch {
                    path: format!("events[{at}].name"),
                    expected: Json::from(expected.name.clone()),
                    actual: Json::from(actual.name.clone()),
                });
            }
            matches(
                &expected.body,
                &actual.body,
                expected.subset,
                bindings,
                &format!("events[{at}].body"),
            )?;
        }
        Ok(())
    }
}

/// `expected.json`, one per program build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub program: String,
    pub version: String,
    pub evidence: Evidence,
    pub scenarios: BTreeMap<String, Scenario>,
    #[serde(default)]
    pub managed_pins: Vec<ManagedPin>,
}
impl Manifest {
    /// Reject missing support decisions, empty excuses and untested managed pins.
    pub fn validate(&self) -> Result<(), CorpusError> {
        let invalid = |text: String| CorpusError::Invalid(text);
        if self.format != 1 || !component(&self.program) || !component(&self.version) {
            return Err(invalid(
                "format must be 1 with a plain program and version".into(),
            ));
        }
        match &self.evidence {
            Evidence::Recorded { references } | Evidence::PublishedProtocol { references, .. }
                if references.is_empty()
                    || references.iter().any(|item| item.trim().is_empty()) =>
            {
                return Err(invalid("evidence needs a source reference".into()));
            }
            Evidence::Synthetic { reason } if reason.trim().is_empty() => {
                return Err(invalid("synthetic evidence needs its reason".into()));
            }
            _ => {}
        }
        if let Evidence::PublishedProtocol { package, .. } = &self.evidence
            && (package.name.trim().is_empty()
                || package.version.trim().is_empty()
                || package.sha256.len() != 64
                || !package.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(invalid(
                "published protocol evidence needs its exact package/version/SHA-256".into(),
            ));
        }
        for &name in SCENARIOS {
            if !self.scenarios.contains_key(name) {
                return Err(invalid(format!("missing scenario {name}")));
            }
        }
        for (name, scenario) in &self.scenarios {
            if !component(name) {
                return Err(invalid(format!("invalid scenario filename {name}")));
            }
            match scenario {
                Scenario::Unsupported { .. } if name.starts_with("tolerance_") => {
                    return Err(invalid(format!("{name} is required for every protocol")));
                }
                Scenario::Unsupported { reason } if reason.trim().is_empty() => {
                    return Err(invalid(format!("{name} needs an unsupported reason")));
                }
                Scenario::Supported {
                    events,
                    quiet,
                    ignore_events,
                } => {
                    if events.is_empty() && (!quiet || name.starts_with("tolerance_")) {
                        return Err(invalid(format!(
                            "{name} needs observations or explicit quiet"
                        )));
                    }
                    if events
                        .iter()
                        .any(|event| event.name.is_empty() || ignore_events.contains(&event.name))
                        || ignore_events
                            .iter()
                            .any(|name| name.is_empty() || name.contains('*'))
                    {
                        return Err(invalid(format!("{name} has ambiguous event expectations")));
                    }
                }
                _ => {}
            }
        }
        for pin in &self.managed_pins {
            if pin.name.trim().is_empty()
                || !matches!(self.scenarios.get(&pin.divergence_scenario), Some(Scenario::Supported { events, .. }) if events.iter().any(divergence_event))
            {
                return Err(invalid(format!(
                    "managed pin {} needs a supported harness_diverged scenario",
                    pin.name
                )));
            }
        }
        Ok(())
    }
}

fn divergence_event(event: &ExpectedEvent) -> bool {
    match event.name.as_str() {
        "item.completed" => event.body["payload"]["code"] == "harness_diverged",
        "turn.completed" => event.body["error"]["code"] == "harness_diverged",
        _ => false,
    }
}
/// Unknown frames must produce counted completed markers even when raw logging is disabled.
pub fn assert_unrecognized(
    observed: &[Observation],
    program: &str,
    event: &str,
    count: u64,
) -> Result<(), CorpusError> {
    let actual = observed
        .iter()
        .filter(|row| row.name == "item.completed")
        .filter_map(|row| {
            let payload = &row.body["payload"];
            let data = &payload["data"];
            if payload["marker"] == "harness_changed"
                && data["change"] == "unrecognized_event"
                && data["program"] == program
                && data["subject"] == event
            {
                data["value"]["count"].as_u64()
            } else {
                None
            }
        })
        .fold(0u64, u64::saturating_add);
    if actual == count {
        Ok(())
    } else {
        Err(CorpusError::Mismatch {
            path: "unrecognized event count".into(),
            expected: Json::from(count),
            actual: Json::from(actual),
        })
    }
}

fn component(text: &str) -> bool {
    !text.is_empty()
        && text != "."
        && text != ".."
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte))
}

/// One deterministic step; directions are from the adapter's viewpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "direction", rename_all = "snake_case", deny_unknown_fields)]
pub enum Frame {
    ToHarness {
        frame: Json,
        #[serde(default)]
        subset: bool,
    },
    FromHarness {
        frame: Json,
    },
    /// Deliberately malformed protocol text, or a one-shot version command's raw output.
    Raw {
        line: String,
        /// False deliberately leaves a partial line before a broken stream/process exit.
        #[serde(default = "line_end")]
        newline: bool,
    },
    Stderr {
        line: String,
    },
    /// Logical wall-clock movement; recorded timing is never retained.
    Tick {
        milliseconds: u32,
    },
    /// The launcher's process-control wrapper returns this code when replay ends.
    Exit {
        code: Option<i32>,
    },
}

fn line_end() -> bool {
    true
}

/// A checked manifest and its declared supported scenarios, read without running any program.
pub struct Corpus {
    pub manifest: Manifest,
    pub frames: BTreeMap<String, Vec<Frame>>,
}
impl Corpus {
    pub fn load(directory: &Path) -> Result<Self, CorpusError> {
        let manifest: Manifest =
            serde_json::from_slice(&std::fs::read(directory.join("expected.json"))?)?;
        manifest.validate()?;
        let mut frames = BTreeMap::new();
        for (name, scenario) in &manifest.scenarios {
            let path = directory.join(format!("{name}.jsonl"));
            match scenario {
                Scenario::Unsupported { .. } if path.exists() => {
                    return Err(CorpusError::Invalid(format!(
                        "unsupported {name} has an unreplayed fixture"
                    )));
                }
                Scenario::Unsupported { .. } => continue,
                Scenario::Supported { .. } => {}
            }
            let text = std::fs::read_to_string(path)?;
            let rows = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(serde_json::from_str)
                .collect::<Result<Vec<Frame>, _>>()?;
            if rows.is_empty() {
                return Err(CorpusError::Invalid(format!("{name} has no replay steps")));
            }
            frames.insert(name.clone(), rows);
        }
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "jsonl")
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_none_or(|name| !frames.contains_key(name))
            {
                return Err(CorpusError::Invalid(format!(
                    "unlisted scenario {}",
                    path.display()
                )));
            }
        }
        Ok(Self { manifest, frames })
    }
}
