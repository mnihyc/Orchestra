//! What the API says of an agent program beyond its descriptor: where its versions come from, what a build of it is checked for, the executors it brings, the files it reads, and its names in each language.
//!
//! A program is named everywhere by [`crate::resources::HarnessKindName`]: its kind and what it is called. The words come from
//! the program's manifest, so a screen never keeps a table of names, and a program Orch does not ship is named the
//! way one it ships is. Words have the English in `name` and the other languages in `names`.
//!
//! The manifest's facts that screens need before a program runs (its version sources, the titles of what a build
//! is checked for, its executors with their tool sets, the files it reads) are in [`ProgramInfo`], beside the
//! descriptor the program's adapter declares. The two never state one thing twice.

use orch_agent_programs::Localized;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Who makes a program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramVendor {
    /// The vendor's name.
    pub name: String,
    /// The licence of the program, as an SPDX expression.
    pub licence: Option<String>,
    /// Where the program is introduced.
    pub home_page: Option<String>,
}

/// The kinds of place a program's builds are published at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProgramChannelKind {
    /// GitHub releases, verified by the digest GitHub keeps or by a checksum file.
    GithubRelease,
    /// An npm package, verified by the registry's integrity value and signature.
    Npm,
    /// A PyPI project, verified by its SHA-256.
    Pypi,
    /// A manifest the vendor publishes.
    SignedManifest,
}

/// One place where newer builds of a program are published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramChannel {
    /// Its name in lists.
    pub id: String,
    /// What kind of place it is, which says how a build from it is verified.
    pub kind: ProgramChannelKind,
    /// Where it is, as a person reads it: the host and the project.
    pub source: String,
}

/// Where the versions of a program come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramSources {
    /// The places newer builds are read from; empty where the vendor publishes none a reader can use, and builds
    /// are added by hand.
    pub channels: Vec<ProgramChannel>,
    /// Whether one named version can be fetched on its own ("Exact").
    pub exact_fetch: bool,
}

/// What one case of a build's check observes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckCaseInfo {
    /// The case's name, as a check's report gives it.
    pub name: String,
    /// What it observes, in a sentence a person reads.
    pub title: Localized,
    /// The feature a failure switches off; absent where the case is about the program as a whole.
    pub feature: Option<String>,
}

/// What a feature that a check can switch off is called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckFeatureInfo {
    /// The feature's identifier, as a check's report and a build's `off` list give it.
    pub id: String,
    /// What it is called, as the thing that stops working: "steering a running turn".
    pub title: Localized,
}

/// Titles for everything the check of a build of one program can report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckCatalogue {
    /// The cases the program's check runs, in order; `runner` (a complete report) is not listed and has a
    /// title of its own.
    pub cases: Vec<CheckCaseInfo>,
    /// The features that may be switched off for a build of it.
    pub features: Vec<CheckFeatureInfo>,
    /// What a report that is incomplete is called: `runner`.
    pub runner: Localized,
}

/// What switches do to the history of a thread when a program cannot take it over natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CarryFallback {
    /// The history is carried as items, and in the handoff notice when it will not fit.
    Items,
    /// Recent history goes in the handoff notice, with the whole conversation kept as a file.
    Handoff,
}

/// What an executor offers, as a machine's policy and an environment's template name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorOffer {
    /// What goes in `exec.offers`: `executor` for an executor of a program's own, `executor:tool_set` for a tool
    /// set of an executor that has several.
    pub value: String,
    /// The name older policies wrote for the same offer, which is still read.
    pub alias: Option<String>,
    /// What it is called.
    pub title: Localized,
    /// One sentence on what it gives an agent.
    pub summary: Localized,
}

/// One set of tools an executor can offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSetInfo {
    /// Its name, which an arrangement's entry and an offer use after the executor's.
    pub name: String,
    /// What it is called.
    pub title: Localized,
    /// One sentence on what it gives an agent.
    pub summary: Localized,
}

/// An executor, with its tool sets and what can be offered of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorInfo {
    /// Its kind, such as `orch-exec`.
    pub kind: String,
    /// What it is called.
    pub title: Localized,
    /// The program whose build it runs, for an executor that is a program's own.
    pub program: Option<String>,
    /// Its tool sets, when it has several.
    pub tool_sets: Vec<ToolSetInfo>,
    /// The offers an environment's exec node can make of it, in the order they are tried.
    pub offers: Vec<ExecutorOffer>,
}

/// Where an executor a machine can offer comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MachineExecutorOrigin {
    /// It comes with the machine's connector release, which carries every executor of the registry.
    Builtin,
    /// It is a plugin the machine's owner registered, and the machine reported what it is.
    Plugin,
}

/// An executor a machine can offer, with its tool sets and the offers an exec node there can make of it. A machine
/// offers exactly these, so the screens of its policy list nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineExecutor {
    /// The executor, its tool sets and the offers.
    pub executor: ExecutorInfo,
    /// Where it comes from.
    pub origin: MachineExecutorOrigin,
    /// For a plugin, whether its supervisor runs it (`running`, `restarting`, `failed`) as the machine last said.
    pub state: Option<String>,
}

/// The executor of an arrangement, a segment or a readiness report, named, with the tool set it works with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorRef {
    /// Its kind, such as `orch-exec`.
    pub kind: String,
    /// What it is called.
    pub title: Localized,
    /// The tool set it works with, where it has several.
    pub tool_set: Option<ToolSetInfo>,
}

/// A file a program reads its standing instructions from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramInstructionFile {
    /// Its name or path.
    pub path: String,
    /// Whether it is looked for in the program's home, which is a person's own where the program runs on their
    /// machine, or in the workspace.
    pub in_home: bool,
}

/// The files a program reads that Orch did not write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramFilesInfo {
    /// Where its instructions come from, in the order it reads them.
    pub instruction_files: Vec<ProgramInstructionFile>,
    /// Where in a workspace it keeps configuration of its own, which the people who manage an environment are
    /// asked about once.
    pub workspace_config: Vec<String>,
}

/// A thing the program is kept from doing unasked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramPrivacy {
    /// What is kept off: `telemetry`, `updates`, `uploads`, `error_reports`.
    pub name: String,
    /// How, in a sentence.
    pub how: Localized,
}

/// What a program's manifest says, as screens need it before the program runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramInfo {
    /// Whether Orch ships the program, and not a connector on some machine that reported it. Whoever reads the list
    /// sees the kind beside the name, so a reported program cannot pass itself off as one Orch ships.
    pub builtin: bool,
    /// Who makes it.
    pub vendor: Option<ProgramVendor>,
    /// The protocol adapter that drives it.
    pub protocol: String,
    /// Where its versions come from.
    pub sources: ProgramSources,
    /// What a build of it is checked for, with the titles of each.
    pub checks: CheckCatalogue,
    /// What switches do to a history it cannot take over natively.
    pub carry: CarryFallback,
    /// The executors it brings.
    pub executors: Vec<ExecutorInfo>,
    /// The files it reads.
    pub files: ProgramFilesInfo,
    /// Whether it may run inside environments by default.
    pub colocated: bool,
    /// What it is kept from doing unasked.
    pub privacy: Vec<ProgramPrivacy>,
}

/// What a model's tokenizer comes to, measured, for the estimates where nobody counted: a paste, the speed of a
/// turn, what fills a context by part. Every measure is kept times a hundred, so that it stays a whole number
/// (482 is 4.82 bytes per token).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenEstimate {
    /// Bytes of ASCII prose per token, times a hundred.
    pub ascii_prose_x100: i64,
    /// Bytes of ASCII that is code, JSON or a command's output, per token, times a hundred.
    pub ascii_structured_x100: i64,
    /// Characters beyond ASCII per token, times a hundred.
    pub other_x100: i64,
    /// What an image the size of a screenshot takes, in tokens.
    pub image_tokens: i64,
    /// Whether these were measured for the model's family; false is a measure between the families measured.
    pub measured: bool,
}
