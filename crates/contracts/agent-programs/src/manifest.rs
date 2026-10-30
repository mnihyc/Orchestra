//! The program manifest: what Orch needs to know about an agent program before anything of it runs.
//!
//! An agent program is an extension made of three things: a protocol adapter in Orch (Rust code, one for each
//! wire protocol), this manifest (data, one for each program) and, where the program has a plugin system, a
//! companion plugin that runs inside it. The manifest says who the program is, which protocol adapter drives it,
//! how it is launched, where its builds come from and how they are verified, what a build is checked for, which
//! executors it brings and which files it reads.
//!
//! **Manifest and descriptor, one source each.** The manifest holds what is true of a program however it is built
//! and before it runs. The descriptor an adapter declares holds what the running adapter claims of one build and
//! what a check of that build proves: its capabilities, modes, parameters and sign-in methods. A fact is written
//! in one of them and never both: the descriptor's `harness.name` is filled from [`ProgramManifest::names`] where
//! an adapter builds its descriptor, and an adapter reads what it reports from its own code and the build it
//! found.
//!
//! A program that Orch does not ship sends its manifest in the `hello` of its adapter plugin; the connector passes
//! it on in the machine's facts, and the API treats it like a built-in one, with its kind shown beside its name
//! so that no program can pass itself off as another.

use std::collections::{BTreeMap, BTreeSet};

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::channels::Channel;
use crate::localized::Localized;

/// The version of the manifest's own shape.
pub const MANIFEST_VERSION: u32 = 1;

/// Who makes a program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Vendor {
    /// The vendor's name.
    pub name: String,
    /// The licence of the program, as an SPDX expression, when it has a public one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    /// Where the program is introduced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_page: Option<String>,
}

/// A file Orch writes into a session's own home before the program starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LaunchFile {
    /// Where it goes, below the session's home.
    pub path: String,
    /// What it holds; `{home}`, `{workspace}` and `{loopback}` stand for the session's own places.
    pub content: String,
}

/// How a program is started.
///
/// A protocol adapter that drives a program by its own protocol builds the command line itself and reads only the
/// program names; the other members are for a generic adapter, which starts what they say.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Launch {
    /// The names the program answers to, in the order they are looked for in a person's installation. The tool
    /// cache keeps a build under the program's kind.
    #[serde(default)]
    pub programs: Vec<String>,
    /// The arguments that start it in the mode Orch drives.
    #[serde(default)]
    pub args: Vec<String>,
    /// The variables it is started with; `{home}`, `{workspace}` and `{loopback}` are replaced.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The files written into the session's home first.
    #[serde(default)]
    pub files: Vec<LaunchFile>,
}

/// Where a program's process gets its home when nothing names one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HomeRule {
    /// A directory of its own, which Orch names and keeps.
    #[default]
    Connector,
    /// None is given: the program has its own default home, and its adapter uses it.
    Program,
}

/// What a program asks of the place it runs in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Placement {
    /// Whether it may run inside environments by default, which lets the arrangements that put a program in an
    /// environment (`key_separated`, `credential_shared`, `handed_over`) be offered on the default template.
    #[serde(default)]
    pub colocated: bool,
    /// Where its processes get their homes.
    #[serde(default)]
    pub home: HomeRule,
    /// Whether it needs a build of the program on the machine: the owner of a person's machine lists where it is.
    #[serde(default)]
    pub needs_binary: bool,
}

/// A template of the provider pool that a program needs, where the system's own cannot run it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeNeed {
    /// Its name.
    pub name: String,
    /// The driver it is for.
    pub driver: String,
    /// The driver's own spec.
    pub spec: Json,
    /// The labels a host must carry.
    #[serde(default)]
    pub host_labels: Vec<String>,
}

/// Where a program's builds come from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Builds {
    /// The places its vendor publishes builds, in the order they are read; none when it publishes none a reader
    /// can use, and builds are added by hand.
    #[serde(default)]
    pub channels: Vec<Channel>,
}

impl Builds {
    /// Whether one named version can be fetched on its own from some channel.
    #[must_use]
    pub fn exact_fetch(&self) -> bool {
        self.channels.iter().any(Channel::exact_fetch)
    }
}

/// How a program prints its interface for a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Printed {
    /// On its standard output, as text: each name is looked for as a whole word.
    Text,
    /// As JSON Schema files written into the directory its arguments name: `Type.member` is a property of the
    /// definition `Type`, and any other name a string the files hold (a method's name).
    Schema,
}

/// One name an adapter needs of its program's interface, and the capability that needs it when not the adapter
/// as a whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Need {
    /// The method, the type's member (`Type.member`) or the flag.
    pub name: String,
    /// The descriptor's capability that is switched off without it; absent when the adapter needs it as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
}

/// What an adapter needs of its program's interface, and how the program is asked.
///
/// A build of a program is checked once, in the background, when it is first kept: a host runs it so that it
/// prints the interface it offers and says which names it lacks. A name the adapter needs as a whole fails the
/// build when it is missing; a name only one capability needs switches that capability off for the build, which is
/// still checked and offered with everything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InterfaceProbe {
    /// The names the program has in a build, in order of preference; the launch names when empty.
    #[serde(default)]
    pub programs: Vec<String>,
    /// The arguments that make it print its interface; `{out}` stands for the directory it writes into.
    pub args: Vec<String>,
    /// Where it prints it.
    pub printed: Printed,
    /// What the adapter uses of it.
    pub needs: Vec<Need>,
}

/// The argument that stands for the directory a program writes its interface into.
pub const OUT: &str = "{out}";

impl InterfaceProbe {
    /// What a check found, by the names a build lacks: whether the adapter can run it at all, and the capabilities
    /// switched off for it, each once and in the order of the list.
    #[must_use]
    pub fn verdict(&self, missing: &[String]) -> (bool, Vec<String>) {
        let mut usable = true;
        let mut off: Vec<String> = Vec::new();
        for need in &self.needs {
            if !missing.iter().any(|name| name == &need.name) {
                continue;
            }
            match &need.capability {
                None => usable = false,
                Some(capability) if !off.contains(capability) => off.push(capability.clone()),
                Some(_) => {}
            }
        }
        (usable, off)
    }
}

/// One native observation the adapter's own cases make of a build against the scripted model, and the descriptor
/// feature its failure switches off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConformanceCase {
    /// The case's name, which the adapter's conformance hooks answer to.
    pub name: String,
    /// The descriptor feature a failure switches off (`approvals.command`, `subagents`, `import_native`); absent
    /// when the case is about the adapter as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    /// What the case observes, in a sentence; absent for a case the shared catalogue ([`crate::catalogue`])
    /// already titles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<Localized>,
}

/// What a build of the program is checked for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Checks {
    /// What the interface probe looks for; none when the program prints no interface to look at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<InterfaceProbe>,
    /// The cases every build passes to be checked. With none, the interface probe is the whole check.
    #[serde(default)]
    pub conformance: Vec<ConformanceCase>,
    /// Observations of surfaces Orch does not use; they never certify or switch anything off.
    #[serde(default)]
    pub probes: Vec<String>,
    /// What a feature that only this program has is called, as the thing that stops working when a check
    /// switches it off; the shared catalogue titles the rest.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub features: BTreeMap<String, Localized>,
    /// What the program calls the tools that the shared conformance scripts call by a role (see [`TOOL_ROLES`]),
    /// each with the names it gives their arguments. A case that calls a tool needs its role named here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_names: BTreeMap<String, ToolName>,
}

/// The roles of the tools that the shared conformance scripts call: what a script has the program do, in words that
/// name no program. A script says `command` and its arguments as `command`; the program's manifest says which tool is
/// its `command` and what it calls that argument.
pub const TOOL_ROLES: [&str; 4] = ["command", "ask_user", "read_file", "spawn_agent"];

/// The role of the tool that a shared conformance case calls, for the cases that call one.
#[must_use]
pub fn tool_role_of(case: &str) -> Option<&'static str> {
    match case {
        "command_approval" => Some("command"),
        "user_input" => Some("ask_user"),
        "nested_subagent" => Some("spawn_agent"),
        "parallel_calls" | "image_result" => Some("read_file"),
        _ => None,
    }
}

/// A tool that a shared conformance script calls by its role, as the program names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolName {
    /// What the program calls the tool.
    pub name: String,
    /// The program's names of the arguments of the script, where they differ from the script's own: the script's
    /// name, then the program's. `command` has the argument `command`, `read_file` has `path`, `spawn_agent` has
    /// `message`; `ask_user` has `questions`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arguments: BTreeMap<String, String>,
}

impl ToolName {
    /// The call a script makes: the tool's name, and the arguments the script gave with the names the program gives
    /// them (only the members of an object are renamed; the values are the script's).
    #[must_use]
    pub fn call(&self, arguments: &Json) -> (String, Json) {
        let renamed = match arguments {
            Json::Object(members) => Json::Object(
                members
                    .iter()
                    .map(|(key, value)| {
                        (
                            self.arguments.get(key).unwrap_or(key).clone(),
                            value.clone(),
                        )
                    })
                    .collect(),
            ),
            other => other.clone(),
        };
        (self.name.clone(), renamed)
    }
}

impl Checks {
    /// The names of the required cases and the feature each switches off, in order.
    pub fn cases(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.conformance
            .iter()
            .map(|case| (case.name.as_str(), case.feature.as_deref()))
    }
}

/// Whose directory an instruction file is looked for in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstructionScope {
    /// The program's own home, which is the person's where the program runs on their machine.
    Home,
    /// The workspace and the directories on the way to where a session works.
    Workspace,
}

/// A file a program reads its standing instructions from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstructionFile {
    /// Its name or path, relative to the directory it is looked for in.
    pub path: String,
    /// Where it is looked for.
    pub scope: InstructionScope,
}

/// The files a program reads that Orch did not write.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramFiles {
    /// The adapter can remove documented disposable caches from a home Orch made.
    #[serde(default)]
    pub managed_home_cleanup: bool,
    /// Where its instructions come from, in the order it reads them.
    #[serde(default)]
    pub instruction_files: Vec<InstructionFile>,
    /// The places in a workspace where it keeps configuration of its own, which the people who manage an
    /// environment are asked about once before a session may use it.
    #[serde(default)]
    pub workspace_config: Vec<String>,
}

/// A switch Orch sets so that the program does nothing Orch did not start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrivacySwitch {
    /// What it keeps off: `telemetry`, `updates`, `uploads`, `error_reports`.
    pub name: String,
    /// One sentence on how it is kept off, for the page that lists the program.
    pub how: Localized,
}

/// The manifest of an agent program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramManifest {
    /// The version of this shape; a reader that does not know it ignores the manifest.
    pub manifest_version: u32,
    /// The program's kind: lower case words joined by dashes, such as `codex`. Profiles, builds and policies name
    /// the program by it.
    pub kind: String,
    /// What the program is called, in each language.
    pub names: Localized,
    /// Who makes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<Vendor>,
    /// The protocol adapter that drives it: the name of one wire protocol Orch has an adapter for.
    pub protocol: String,
    /// How it is started.
    #[serde(default)]
    pub launch: Launch,
    /// What it asks of the place it runs in.
    #[serde(default)]
    pub placement: Placement,
    /// A template of the provider pool it needs, where the system's cannot run it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeNeed>,
    /// Where its builds come from.
    #[serde(default)]
    pub builds: Builds,
    /// What a build is checked for.
    #[serde(default)]
    pub checks: Checks,
    /// The executors it brings: their kinds, which an environment's exec node can offer.
    #[serde(default)]
    pub executors: Vec<String>,
    /// The files it reads.
    #[serde(default)]
    pub files: ProgramFiles,
    /// What Orch keeps it from doing unasked.
    #[serde(default)]
    pub privacy: Vec<PrivacySwitch>,
}

/// What is wrong with a manifest.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestFault {
    /// The shape's version is not one this reader knows.
    #[error("manifest version {0} is not one this reader knows")]
    Version(u32),
    /// The kind is not lower case words joined by dashes.
    #[error("the kind {0:?} is lower case words joined by dashes")]
    Kind(String),
    /// The program has no English name.
    #[error("the program has no English name")]
    Name,
    /// No protocol adapter is named.
    #[error("the program names no protocol adapter")]
    Protocol,
    /// Two entries share a name where each must be one.
    #[error("{0}")]
    Duplicate(String),
    /// A channel is written wrongly.
    #[error("{0}")]
    Channel(String),
    /// A value in the manifest is empty or malformed.
    #[error("{0}")]
    Value(String),
}

impl ProgramManifest {
    /// Whether the kind is lower case words joined by dashes.
    #[must_use]
    pub fn is_kind(text: &str) -> bool {
        !text.is_empty()
            && text.split('-').all(|word| {
                word.chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_lowercase())
                    && word
                        .chars()
                        .all(|each| each.is_ascii_lowercase() || each.is_ascii_digit())
            })
    }

    /// The names the program's executable answers to in a person's installation, in order: the launch names, and
    /// the kind when none are given.
    #[must_use]
    pub fn program_names(&self) -> Vec<&str> {
        if self.launch.programs.is_empty() {
            vec![self.kind.as_str()]
        } else {
            self.launch.programs.iter().map(String::as_str).collect()
        }
    }

    /// The names a build of the program has when its interface is checked: the probe's own, else the launch names.
    #[must_use]
    pub fn probe_programs(&self) -> Vec<&str> {
        match &self.checks.interface {
            Some(probe) if !probe.programs.is_empty() => {
                probe.programs.iter().map(String::as_str).collect()
            }
            _ => self.program_names(),
        }
    }

    /// What the program is called in English: the name the descriptor's `harness.name` is filled from.
    #[must_use]
    pub fn name(&self) -> &str {
        self.names.en()
    }

    /// Whether a conformance case proves `feature`: its failure switches the feature off and its pass is the
    /// proof. Auto (`approval_modes.auto`) exists for a program only through one.
    #[must_use]
    pub fn proves(&self, feature: &str) -> bool {
        self.checks
            .conformance
            .iter()
            .any(|case| case.feature.as_deref() == Some(feature))
    }

    /// The program's name in each language but English, in the shape a descriptor's `text` has it: language, then
    /// place, then words.
    #[must_use]
    pub fn name_text(&self) -> BTreeMap<String, BTreeMap<String, String>> {
        self.names
            .others()
            .into_iter()
            .map(|(language, words)| {
                (
                    language,
                    BTreeMap::from([("harness.name".to_owned(), words)]),
                )
            })
            .collect()
    }

    /// Checks that the manifest is consistent with itself; every fault is returned.
    pub fn check(&self) -> Result<(), Vec<ManifestFault>> {
        let mut faults = Vec::new();
        if self.manifest_version != MANIFEST_VERSION {
            faults.push(ManifestFault::Version(self.manifest_version));
        }
        if !Self::is_kind(&self.kind) {
            faults.push(ManifestFault::Kind(self.kind.clone()));
        }
        if !self.names.is_complete() {
            faults.push(ManifestFault::Name);
        }
        if self.protocol.trim().is_empty() {
            faults.push(ManifestFault::Protocol);
        }
        let mut ids = BTreeSet::new();
        for channel in &self.builds.channels {
            if !ids.insert(channel.id()) {
                faults.push(ManifestFault::Duplicate(format!(
                    "channel {} is declared twice",
                    channel.id()
                )));
            }
            faults.extend(channel.faults().into_iter().map(ManifestFault::Channel));
        }
        let mut cases = BTreeSet::new();
        for case in &self.checks.conformance {
            if case.name.trim().is_empty() || !cases.insert(case.name.as_str()) {
                faults.push(ManifestFault::Duplicate(format!(
                    "conformance case {:?} is unnamed or declared twice",
                    case.name
                )));
            }
        }
        for (role, tool) in &self.checks.tool_names {
            if !TOOL_ROLES.contains(&role.as_str()) {
                faults.push(ManifestFault::Value(format!(
                    "{role:?} is no role of a tool a conformance script calls"
                )));
            }
            if tool.name.trim().is_empty() {
                faults.push(ManifestFault::Value(format!(
                    "the tool of the role {role} is unnamed"
                )));
            }
        }
        for case in &self.checks.conformance {
            if let Some(role) = tool_role_of(&case.name)
                && !self.checks.tool_names.contains_key(role)
            {
                faults.push(ManifestFault::Value(format!(
                    "the case {} calls a tool by its role {role}, which the manifest does not name",
                    case.name
                )));
            }
        }
        if let Some(probe) = &self.checks.interface {
            let mut names = BTreeSet::new();
            for need in &probe.needs {
                if need.name.trim().is_empty() || !names.insert(need.name.as_str()) {
                    faults.push(ManifestFault::Duplicate(format!(
                        "interface name {:?} is unnamed or listed twice",
                        need.name
                    )));
                }
            }
            if probe.args.is_empty() {
                faults.push(ManifestFault::Value(
                    "the interface probe names no arguments".to_owned(),
                ));
            }
            if probe.printed == Printed::Schema && !probe.args.iter().any(|arg| arg == OUT) {
                faults.push(ManifestFault::Value(format!(
                    "an interface printed as schema files needs the {OUT} argument"
                )));
            }
        }
        if self.placement.needs_binary && self.program_names().iter().any(|name| name.is_empty()) {
            faults.push(ManifestFault::Value(
                "a program that needs a build names it".to_owned(),
            ));
        }
        for executor in &self.executors {
            if executor.trim().is_empty() {
                faults.push(ManifestFault::Value("an executor is unnamed".to_owned()));
            }
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal() -> ProgramManifest {
        serde_json::from_value(json!({
            "manifest_version": 1, "kind": "tiny", "names": { "en": "Tiny" }, "protocol": "acp"
        }))
        .unwrap()
    }

    #[test]
    fn a_manifest_that_names_only_what_it_must_is_a_manifest() {
        let manifest = minimal();
        assert_eq!(manifest.check(), Ok(()));
        assert_eq!(manifest.program_names(), ["tiny"]);
        assert!(!manifest.placement.colocated);
        assert!(!manifest.builds.exact_fetch());
        assert!(manifest.checks.interface.is_none());
    }

    #[test]
    fn a_program_must_opt_in_to_managed_home_cache_maintenance() {
        let mut manifest = minimal();
        assert!(!manifest.files.managed_home_cleanup);
        manifest.files.managed_home_cleanup = true;
        let restored: ProgramManifest =
            serde_json::from_value(serde_json::to_value(&manifest).unwrap()).unwrap();
        assert!(restored.files.managed_home_cleanup);
        assert_eq!(restored.check(), Ok(()));
    }

    #[test]
    fn a_manifest_that_cannot_hold_together_is_refused_for_each_reason() {
        let mut broken = minimal();
        broken.manifest_version = 9;
        broken.kind = "Tiny Program".to_owned();
        broken.names = Localized::default();
        broken.protocol = " ".to_owned();
        broken.checks.conformance = vec![
            ConformanceCase {
                name: "a".to_owned(),
                feature: None,
                title: None,
            },
            ConformanceCase {
                name: "a".to_owned(),
                feature: None,
                title: None,
            },
        ];
        broken.checks.interface = Some(InterfaceProbe {
            programs: Vec::new(),
            args: vec!["--print".to_owned()],
            printed: Printed::Schema,
            needs: Vec::new(),
        });
        let faults = broken.check().unwrap_err();
        assert!(faults.contains(&ManifestFault::Version(9)));
        assert!(faults.contains(&ManifestFault::Kind("Tiny Program".to_owned())));
        assert!(faults.contains(&ManifestFault::Name));
        assert!(faults.contains(&ManifestFault::Protocol));
        assert!(
            faults
                .iter()
                .any(|fault| matches!(fault, ManifestFault::Duplicate(_)))
        );
        assert!(
            faults
                .iter()
                .any(|fault| matches!(fault, ManifestFault::Value(text) if text.contains("{out}")))
        );
    }

    #[test]
    fn a_verdict_fails_a_build_for_a_name_it_needs_and_switches_off_what_one_capability_needs() {
        let probe = InterfaceProbe {
            programs: Vec::new(),
            args: vec!["--help".to_owned()],
            printed: Printed::Text,
            needs: vec![
                Need {
                    name: "--model".to_owned(),
                    capability: None,
                },
                Need {
                    name: "--fork".to_owned(),
                    capability: Some("fork_native".to_owned()),
                },
                Need {
                    name: "--fork-at".to_owned(),
                    capability: Some("fork_native".to_owned()),
                },
            ],
        };
        assert_eq!(probe.verdict(&[]), (true, Vec::new()));
        assert_eq!(
            probe.verdict(&["--fork".to_owned(), "--fork-at".to_owned()]),
            (true, vec!["fork_native".to_owned()])
        );
        assert_eq!(probe.verdict(&["--model".to_owned()]), (false, Vec::new()));
    }

    #[test]
    fn kinds_are_words_joined_by_dashes() {
        assert!(ProgramManifest::is_kind("claude-code"));
        assert!(ProgramManifest::is_kind("fake-two"));
        assert!(!ProgramManifest::is_kind("Codex"));
        assert!(!ProgramManifest::is_kind("claude_code"));
        assert!(!ProgramManifest::is_kind("-x"));
        assert!(!ProgramManifest::is_kind(""));
    }

    #[test]
    fn a_case_that_calls_a_tool_by_its_role_needs_the_manifest_to_name_that_tool() {
        let mut manifest = minimal();
        manifest.checks.conformance = vec![ConformanceCase {
            name: "command_approval".to_owned(),
            feature: None,
            title: None,
        }];
        let Err(faults) = manifest.check() else {
            panic!("a case with no tool for its role");
        };
        assert!(faults.iter().any(|fault| matches!(
            fault,
            ManifestFault::Value(text) if text.contains("role command")
        )));
        manifest.checks.tool_names.insert(
            "command".to_owned(),
            serde_json::from_value(json!({ "name": "shell", "arguments": { "command": "cmd" } }))
                .unwrap(),
        );
        assert_eq!(manifest.check(), Ok(()));
        manifest.checks.tool_names.insert(
            "teleport".to_owned(),
            ToolName {
                name: String::new(),
                arguments: BTreeMap::new(),
            },
        );
        let Err(faults) = manifest.check() else {
            panic!("a role no script has");
        };
        assert_eq!(
            faults.len(),
            2,
            "the role is unknown and its tool is unnamed: {faults:?}"
        );
    }

    #[test]
    fn a_script_calls_a_tool_with_the_names_its_program_gives_the_arguments() {
        let tool: ToolName = serde_json::from_value(
            json!({ "name": "exec_command", "arguments": { "command": "cmd" } }),
        )
        .unwrap();
        let (name, arguments) = tool.call(&json!({ "command": "touch x", "timeout": 3 }));
        assert_eq!(name, "exec_command");
        assert_eq!(arguments, json!({ "cmd": "touch x", "timeout": 3 }));
        let plain: ToolName = serde_json::from_value(json!({ "name": "Ask" })).unwrap();
        assert_eq!(
            plain.call(&json!({ "questions": [] })).1,
            json!({ "questions": [] })
        );
        assert_eq!(tool_role_of("user_input"), Some("ask_user"));
        assert_eq!(tool_role_of("startup"), None);
    }
}
