//! The registry: the manifests of the programs Orch ships, and the lookups every list of programs reads.
//!
//! The connector's composition, the seeded profiles, the plugin-name collision lists, the default of which programs
//! may run inside environments, the API's build checks and release channels, the node's test runner, `orchctl`
//! and the screens all iterate this registry instead of a list of their own. A program a connector reports in the
//! `hello` of its adapter plugin is added to a copy of it ([`Programs::with_program`]), and is treated like a
//! built-in one, except that it can never take the kind of one that Orch ships.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::executors::ExecutorManifest;
use crate::manifest::{ManifestFault, ProgramManifest};

/// The manifests of the programs Orch ships, one file for each.
const PROGRAM_FILES: [&str; 4] = [
    include_str!("../manifests/claude-code.json"),
    include_str!("../manifests/codex.json"),
    include_str!("../manifests/dsh.json"),
    include_str!("../manifests/fake.json"),
];

/// The manifests of the executors Orch ships, one file for each.
const EXECUTOR_FILES: [&str; 2] = [
    include_str!("../executors/codex-exec-server.json"),
    include_str!("../executors/orch-exec.json"),
];

/// The manifest of a program Orch ships. The adapter of such a program reads its name and its facts here, so that
/// they are written once.
#[must_use]
pub fn shipped(kind: &str) -> &'static ProgramManifest {
    #[expect(
        clippy::expect_used,
        reason = "an adapter asks for its own kind, and a test of this package reads every shipped manifest"
    )]
    fn of(kind: &str) -> &'static ProgramManifest {
        Programs::builtin()
            .program(kind)
            .expect("a shipped adapter has a shipped manifest")
    }
    of(kind)
}

/// The manifest of an executor Orch ships. The executor reads its title, its tool sets and its limits here, so that
/// they are written once.
#[must_use]
pub fn shipped_executor(kind: &str) -> &'static ExecutorManifest {
    #[expect(
        clippy::expect_used,
        reason = "an executor asks for its own kind, and a test of this package reads every shipped manifest"
    )]
    fn of(kind: &str) -> &'static ExecutorManifest {
        Programs::builtin()
            .executor(kind)
            .expect("a shipped executor has a shipped manifest")
    }
    of(kind)
}

/// A kind or a name kept for the life of the process: the connector's adapters name their kind as `&'static str`, and
/// a program a connector reports is one more kind of the same few. One text is kept once.
#[must_use]
pub fn intern(text: &str) -> &'static str {
    static KEPT: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    let mut kept = KEPT
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(found) = kept.get(text) {
        return found;
    }
    let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
    kept.insert(leaked);
    leaked
}

/// Why a registry cannot be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryFault {
    /// A manifest is not written as one.
    #[error("manifest {name}: {reason}")]
    Unreadable {
        /// The file or the kind.
        name: String,
        /// What the reader said.
        reason: String,
    },
    /// A manifest is inconsistent with itself.
    #[error("program {kind}: {faults:?}")]
    Inconsistent {
        /// The program's kind.
        kind: String,
        /// What is wrong.
        faults: Vec<ManifestFault>,
    },
    /// An executor's manifest is inconsistent with itself.
    #[error("executor {kind}: {faults:?}")]
    InconsistentExecutor {
        /// The executor's kind.
        kind: String,
        /// What is wrong.
        faults: Vec<String>,
    },
    /// Two programs or two executors have one kind.
    #[error("the kind {0:?} is taken")]
    Taken(String),
    /// A program names an executor no manifest describes.
    #[error("program {program} brings executor {executor}, which no manifest describes")]
    UnknownExecutor {
        /// The program's kind.
        program: String,
        /// The executor it names.
        executor: String,
    },
}

/// The programs and executors Orch knows, by kind.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Programs {
    programs: BTreeMap<String, ProgramManifest>,
    executors: BTreeMap<String, ExecutorManifest>,
}

impl Programs {
    /// The registry of what Orch ships.
    #[must_use]
    pub fn builtin() -> &'static Self {
        #[expect(
            clippy::expect_used,
            reason = "the embedded manifests are data of this package, and its tests read every one of them"
        )]
        fn load() -> Programs {
            Programs::from_files(&PROGRAM_FILES, &EXECUTOR_FILES)
                .expect("the built-in manifests are valid")
        }
        static BUILTIN: OnceLock<Programs> = OnceLock::new();
        BUILTIN.get_or_init(load)
    }

    /// A registry made of manifests written as JSON.
    pub fn from_files(programs: &[&str], executors: &[&str]) -> Result<Self, RegistryFault> {
        let mut registry = Self::default();
        for text in executors {
            let manifest: ExecutorManifest =
                serde_json::from_str(text).map_err(|error| RegistryFault::Unreadable {
                    name: "an executor".to_owned(),
                    reason: error.to_string(),
                })?;
            registry.add_executor(manifest)?;
        }
        for text in programs {
            let manifest: ProgramManifest =
                serde_json::from_str(text).map_err(|error| RegistryFault::Unreadable {
                    name: "a program".to_owned(),
                    reason: error.to_string(),
                })?;
            registry.add_program(manifest)?;
        }
        Ok(registry)
    }

    fn add_executor(&mut self, manifest: ExecutorManifest) -> Result<(), RegistryFault> {
        let faults = manifest.faults();
        if !faults.is_empty() {
            return Err(RegistryFault::InconsistentExecutor {
                kind: manifest.kind,
                faults,
            });
        }
        if self.executors.contains_key(&manifest.kind) {
            return Err(RegistryFault::Taken(manifest.kind));
        }
        self.executors.insert(manifest.kind.clone(), manifest);
        Ok(())
    }

    fn add_program(&mut self, manifest: ProgramManifest) -> Result<(), RegistryFault> {
        if let Err(faults) = manifest.check() {
            return Err(RegistryFault::Inconsistent {
                kind: manifest.kind,
                faults,
            });
        }
        if self.programs.contains_key(&manifest.kind) {
            return Err(RegistryFault::Taken(manifest.kind));
        }
        for executor in &manifest.executors {
            if !self.executors.contains_key(executor) {
                return Err(RegistryFault::UnknownExecutor {
                    program: manifest.kind,
                    executor: executor.clone(),
                });
            }
        }
        self.programs.insert(manifest.kind.clone(), manifest);
        Ok(())
    }

    /// The registry with one more program: one a connector reported, or one a test adds. A kind that is taken is
    /// refused, so a reported program cannot replace one Orch ships or pass itself off as it.
    pub fn with_program(&self, manifest: ProgramManifest) -> Result<Self, RegistryFault> {
        let mut registry = self.clone();
        registry.add_program(manifest)?;
        Ok(registry)
    }

    /// The registry with one more executor.
    pub fn with_executor(&self, manifest: ExecutorManifest) -> Result<Self, RegistryFault> {
        let mut registry = self.clone();
        registry.add_executor(manifest)?;
        Ok(registry)
    }

    /// The program of `kind`.
    #[must_use]
    pub fn program(&self, kind: &str) -> Option<&ProgramManifest> {
        self.programs.get(kind)
    }

    /// Every program, by kind.
    pub fn programs(&self) -> impl Iterator<Item = &ProgramManifest> {
        self.programs.values()
    }

    /// The kinds of every program.
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.programs.keys().map(String::as_str)
    }

    /// The executor of `kind`.
    #[must_use]
    pub fn executor(&self, kind: &str) -> Option<&ExecutorManifest> {
        self.executors.get(kind)
    }

    /// Every executor, by kind.
    pub fn executors(&self) -> impl Iterator<Item = &ExecutorManifest> {
        self.executors.values()
    }

    /// The kinds of the programs that may run inside environments by default, in the order of their kinds.
    #[must_use]
    pub fn colocated_by_default(&self) -> Vec<&str> {
        self.programs
            .values()
            .filter(|manifest| manifest.placement.colocated)
            .map(|manifest| manifest.kind.as_str())
            .collect()
    }

    /// The kinds of programs a template's spec lets run inside its environments: the list its `colocated_harnesses`
    /// gives, none where it has none. The template the system is made with lists the programs whose manifests say
    /// they may run there by default ([`Programs::colocated_by_default`]); any other template says for itself.
    #[must_use]
    pub fn colocated_for(spec: &orch_types::Json) -> Vec<String> {
        spec.get("colocated_harnesses")
            .and_then(orch_types::Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(orch_types::Json::as_str)
            .map(str::to_owned)
            .collect()
    }

    /// Whether any program can offer Auto: some manifest lists the conformance case that proves it. Where none does,
    /// no setting offers the mode.
    #[must_use]
    pub fn auto_possible(&self) -> bool {
        self.programs
            .values()
            .any(|manifest| manifest.proves("approval_modes.auto"))
    }

    /// The program whose build the executor `kind` runs, for an executor that is a program's own.
    #[must_use]
    pub fn program_of_executor(&self, kind: &str) -> Option<&str> {
        self.executors
            .get(kind)
            .and_then(|executor| executor.program.as_deref())
    }

    /// The offers a template makes by default: the executors the programs `kinds` bring that are a program's own, and
    /// the tool sets of Orch's own executor, each named as an environment's exec node offers it (`executor` for a
    /// program's own, `executor:tool_set` for a set of an executor that has several). The tool set made of an
    /// environment's own servers (`custom`) is offered only where somebody asks for it.
    #[must_use]
    pub fn default_offers_for<'a>(&self, kinds: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut offers: Vec<String> = Vec::new();
        for kind in kinds {
            let Some(manifest) = self.programs.get(kind) else {
                continue;
            };
            for executor in &manifest.executors {
                if let Some(brought) = self.executors.get(executor)
                    && brought.tool_sets.is_empty()
                    && !offers.contains(executor)
                {
                    offers.push(executor.clone());
                }
            }
        }
        offers.sort();
        for executor in self
            .executors
            .values()
            .filter(|executor| !executor.tool_sets.is_empty())
        {
            for set in executor.tool_sets.iter().filter(|set| set.name != "custom") {
                offers.push(format!("{}:{}", executor.kind, set.name));
            }
        }
        offers
    }

    /// The offers a template makes by default when it lets the programs that may run in environments by default do
    /// so.
    #[must_use]
    pub fn default_offers(&self) -> Vec<String> {
        self.default_offers_for(self.colocated_by_default())
    }

    /// An offer in its namespaced form. An exec node's offers name an executor (`codex-exec-server`) or a tool set of
    /// an executor (`orch-exec:plain`); a tool set written alone, as offers were before they named their
    /// executor (`plain`), is read as that set of the executor that has it. Anything else, an executor nobody has
    /// described among them, stays as it is.
    #[must_use]
    pub fn canonical_offer(&self, offer: &str) -> String {
        if offer.contains(':') || self.executors.contains_key(offer) {
            return offer.to_owned();
        }
        self.executors
            .values()
            .find(|executor| executor.has_tool_set(offer))
            .map_or_else(
                || offer.to_owned(),
                |executor| format!("{}:{offer}", executor.kind),
            )
    }

    /// The kind of executor an offer is for.
    #[must_use]
    pub fn offered_executor(&self, offer: &str) -> String {
        let canonical = self.canonical_offer(offer);
        canonical
            .split_once(':')
            .map_or(canonical.as_str(), |(executor, _)| executor)
            .to_owned()
    }

    /// Whether `offers` include `offer`: the same offer however either is written, and an executor that has tool sets
    /// is included by an offer of any of its sets.
    #[must_use]
    pub fn offers_include(&self, offers: &[String], offer: &str) -> bool {
        let wanted = self.canonical_offer(offer);
        offers.iter().any(|each| {
            let each = self.canonical_offer(each);
            each == wanted
                || (!wanted.contains(':')
                    && each
                        .split_once(':')
                        .is_some_and(|(executor, _)| executor == wanted))
        })
    }

    /// Every offer an exec node can make: each executor of a program's own, and each tool set of an executor that
    /// has several.
    #[must_use]
    pub fn every_offer(&self) -> Vec<String> {
        let mut offers = Vec::new();
        for executor in self.executors.values() {
            if executor.tool_sets.is_empty() {
                offers.push(executor.kind.clone());
            } else {
                offers.extend(
                    executor
                        .tool_sets
                        .iter()
                        .map(|set| format!("{}:{}", executor.kind, set.name)),
                );
            }
        }
        offers
    }

    /// The kinds that no program may take: every program and executor Orch knows. An external plugin of one of
    /// these names would be refused where plugins are registered.
    #[must_use]
    pub fn reserved_kinds(&self) -> Vec<&str> {
        self.kinds()
            .chain(self.executors.keys().map(String::as_str))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_programs_orch_ships_are_four_and_each_is_consistent() {
        let kinds: Vec<_> = Programs::builtin().kinds().collect();
        assert_eq!(kinds, ["claude-code", "codex", "dsh", "fake"]);
        for manifest in Programs::builtin().programs() {
            assert_eq!(manifest.check(), Ok(()), "{}", manifest.kind);
            assert!(
                manifest.names.covers_every_language(),
                "{} is named in every language",
                manifest.kind
            );
        }
        for executor in Programs::builtin().executors() {
            assert!(executor.faults().is_empty(), "{}", executor.kind);
            assert!(executor.title.covers_every_language());
        }
    }

    #[test]
    fn the_real_programs_run_inside_environments_by_default_and_the_fake_does_not() {
        assert_eq!(
            Programs::builtin().colocated_by_default(),
            ["claude-code", "codex", "dsh"]
        );
    }

    #[test]
    fn the_default_offers_are_what_a_fresh_template_has_always_offered() {
        assert_eq!(
            Programs::builtin().default_offers(),
            [
                "codex-exec-server",
                "orch-exec:claude-code",
                "orch-exec:plain"
            ]
        );
    }

    #[test]
    fn an_offer_is_the_same_offer_whether_it_names_its_executor_or_not() {
        let programs = Programs::builtin();
        assert_eq!(programs.canonical_offer("plain"), "orch-exec:plain");
        assert_eq!(
            programs.canonical_offer("orch-exec:plain"),
            "orch-exec:plain"
        );
        assert_eq!(
            programs.canonical_offer("codex-exec-server"),
            "codex-exec-server"
        );
        assert_eq!(programs.canonical_offer("somebody-elses"), "somebody-elses");
        assert_eq!(programs.offered_executor("claude-code"), "orch-exec");
        assert_eq!(
            programs.offered_executor("codex-exec-server"),
            "codex-exec-server"
        );
        let allowed = vec![
            "codex-exec-server".to_owned(),
            "claude-code".to_owned(),
            "plain".to_owned(),
        ];
        assert!(programs.offers_include(&allowed, "orch-exec:plain"));
        assert!(!programs.offers_include(&allowed, "custom"));
        assert!(
            programs.offers_include(&allowed, "orch-exec"),
            "an executor is offered when any of its sets is"
        );
        assert!(!programs.offers_include(&["plain".to_owned()], "codex-exec-server"));
        assert_eq!(
            programs.every_offer(),
            [
                "codex-exec-server",
                "orch-exec:claude-code",
                "orch-exec:plain",
                "orch-exec:custom"
            ]
        );
    }

    #[test]
    fn a_reported_program_cannot_take_a_kind_orch_ships() {
        let mut manifest = Programs::builtin().program("codex").cloned().unwrap();
        assert_eq!(
            Programs::builtin().with_program(manifest.clone()),
            Err(RegistryFault::Taken("codex".to_owned()))
        );
        manifest.kind = "codex-two".to_owned();
        let more = Programs::builtin().with_program(manifest).unwrap();
        assert!(more.program("codex-two").is_some());
        assert!(Programs::builtin().program("codex-two").is_none());
    }

    #[test]
    fn a_program_cannot_bring_an_executor_nobody_describes() {
        let mut manifest = Programs::builtin().program("fake").cloned().unwrap();
        manifest.kind = "fake-two".to_owned();
        manifest.executors = vec!["missing".to_owned()];
        assert!(matches!(
            Programs::builtin().with_program(manifest),
            Err(RegistryFault::UnknownExecutor { .. })
        ));
    }

    #[test]
    fn codex_keeps_the_interface_and_cases_its_adapter_was_written_against() {
        let codex = Programs::builtin().program("codex").unwrap();
        let probe = codex.checks.interface.as_ref().unwrap();
        assert_eq!(probe.needs.len(), 54);
        assert_eq!(codex.checks.conformance.len(), 8);
        assert_eq!(codex.probe_programs(), ["codex"]);
        let claude = Programs::builtin().program("claude-code").unwrap();
        assert_eq!(claude.checks.interface.as_ref().unwrap().needs.len(), 25);
        assert_eq!(claude.checks.conformance.len(), 6);
        assert_eq!(claude.probe_programs(), ["claude-code", "claude"]);
        assert_eq!(claude.program_names(), ["claude", "claude-code"]);
    }
}
