//! What the API says of agent programs, made from their manifests and descriptors: names in each language, version sources, what a build is checked for, executors with their tool sets.
//!
//! Nothing here names a program. A program is a manifest in the registry, and a screen never keeps a table of what
//! programs are called, how their builds are checked or what their executors offer: it reads these.

use orch_agent_contract::descriptor::AgentDescriptor;
use orch_agent_programs::manifest::InstructionScope;
use orch_agent_programs::{Catalogue, ExecutorManifest, Localized, ProgramManifest, Programs};
use orch_api_contract::programs::{
    CarryFallback, CheckCaseInfo, CheckCatalogue, CheckFeatureInfo, ExecutorInfo, ExecutorOffer,
    MachineExecutor, MachineExecutorOrigin, ProgramChannel, ProgramChannelKind, ProgramFilesInfo,
    ProgramInfo, ProgramInstructionFile, ProgramPrivacy, ProgramSources, ProgramVendor,
    ToolSetInfo,
};

pub use orch_engine::programs::{
    executor_ref, harness_name, named_by, shipped_name, tool_set_info,
};

/// An executor, with its tool sets and the offers an exec node can make of it.
#[must_use]
pub fn executor_info(programs: &Programs, executor: &ExecutorManifest) -> ExecutorInfo {
    let offers = if executor.tool_sets.is_empty() {
        vec![ExecutorOffer {
            value: executor.kind.clone(),
            alias: None,
            title: executor.title.clone(),
            summary: executor.title.clone(),
        }]
    } else {
        executor
            .tool_sets
            .iter()
            .map(|set| ExecutorOffer {
                value: format!("{}:{}", executor.kind, set.name),
                // The word offers were written with before they named their executor, when no other executor
                // has a set of that name.
                alias: (programs.canonical_offer(&set.name)
                    == format!("{}:{}", executor.kind, set.name))
                .then(|| set.name.clone()),
                title: set.title.clone(),
                summary: set.summary.clone(),
            })
            .collect()
    };
    ExecutorInfo {
        kind: executor.kind.clone(),
        title: executor.title.clone(),
        program: executor.program.clone(),
        tool_sets: executor.tool_sets.iter().map(tool_set_info).collect(),
        offers,
    }
}

/// An executor a machine's plugin reported, retaining its localized declared tool-set text.
/// An older report without a tool-set title leaves that name unavailable instead of inventing it from an identifier.
#[must_use]
pub fn executor_info_of(
    descriptor: &orch_executor_contract::descriptor::ExecutorDescriptor,
) -> ExecutorInfo {
    let title = descriptor.titles.iter().fold(
        Localized::english(&descriptor.title),
        |words, (language, text)| words.with(language, text),
    );
    let tool_sets: Vec<_> = descriptor
        .tool_sets
        .iter()
        .map(|set| ToolSetInfo {
            name: set.name.clone(),
            title: set
                .titles
                .iter()
                .fold(Localized::english(&set.title), |words, (language, text)| {
                    words.with(language, text)
                }),
            summary: set.summaries.iter().fold(
                Localized::english(&set.summary),
                |words, (language, text)| words.with(language, text),
            ),
        })
        .collect();
    let offers = if tool_sets.is_empty() {
        vec![ExecutorOffer {
            value: descriptor.kind.clone(),
            alias: None,
            title: title.clone(),
            summary: title.clone(),
        }]
    } else {
        tool_sets
            .iter()
            .map(|set| ExecutorOffer {
                value: format!("{}:{}", descriptor.kind, set.name),
                alias: None,
                title: set.title.clone(),
                summary: set.summary.clone(),
            })
            .collect()
    };
    ExecutorInfo {
        kind: descriptor.kind.clone(),
        title,
        program: descriptor.program.clone(),
        tool_sets,
        offers,
    }
}

/// The executors a machine can offer: those the registry carries, which every connector release carries, and the
/// executor plugins in the `facts` the machine reported (one a release already carries is not offered twice).
#[must_use]
pub fn machine_executors(programs: &Programs, facts: &orch_types::Json) -> Vec<MachineExecutor> {
    let mut all: Vec<MachineExecutor> = executors(programs)
        .into_iter()
        .map(|executor| MachineExecutor {
            executor,
            origin: MachineExecutorOrigin::Builtin,
            state: None,
        })
        .collect();
    let reported = facts
        .get("plugins")
        .and_then(orch_types::Json::as_array)
        .into_iter()
        .flatten();
    for plugin in reported {
        if plugin.get("kind").and_then(orch_types::Json::as_str) != Some("executor") {
            continue;
        }
        let Some(descriptor) = plugin.get("executor").and_then(|executor| {
            serde_json::from_value::<orch_executor_contract::descriptor::ExecutorDescriptor>(
                executor.clone(),
            )
            .ok()
        }) else {
            continue;
        };
        if all
            .iter()
            .any(|known| known.executor.kind == descriptor.kind)
        {
            continue;
        }
        all.push(MachineExecutor {
            executor: executor_info_of(&descriptor),
            origin: MachineExecutorOrigin::Plugin,
            state: plugin
                .get("state")
                .and_then(orch_types::Json::as_str)
                .map(str::to_owned),
        });
    }
    all
}

/// Every executor the registry knows.
#[must_use]
pub fn executors(programs: &Programs) -> Vec<ExecutorInfo> {
    programs
        .executors()
        .map(|executor| executor_info(programs, executor))
        .collect()
}

/// What switches do to a history a program cannot take over natively: items where it takes a neutral history, the
/// handoff notice where it does not. `import_neutral` is what the program declares.
#[must_use]
pub fn carry_of(import_neutral: bool, off: &[String]) -> CarryFallback {
    if import_neutral && !off.iter().any(|feature| feature == "import_neutral") {
        CarryFallback::Items
    } else {
        CarryFallback::Handoff
    }
}

/// What a check of a build of `manifest` reports, with the title of each.
#[must_use]
pub fn check_catalogue(manifest: &ProgramManifest) -> CheckCatalogue {
    let catalogue = Catalogue::builtin();
    let cases = manifest
        .checks
        .conformance
        .iter()
        .map(|case| CheckCaseInfo {
            name: case.name.clone(),
            title: case
                .title
                .clone()
                .or_else(|| catalogue.cases.get(&case.name).cloned())
                .unwrap_or_else(|| Localized::english(&case.name)),
            feature: case.feature.clone(),
        })
        .collect();
    // Every feature the program's check can switch off: those its interface names and its cases name.
    let mut ids: Vec<String> = manifest
        .checks
        .interface
        .iter()
        .flat_map(|probe| probe.needs.iter())
        .filter_map(|need| need.capability.clone())
        .chain(
            manifest
                .checks
                .conformance
                .iter()
                .filter_map(|case| case.feature.clone()),
        )
        .collect();
    ids.sort();
    ids.dedup();
    let features = ids
        .into_iter()
        .map(|id| CheckFeatureInfo {
            title: catalogue
                .feature(&id, manifest.checks.features.get(&id))
                .clone(),
            id,
        })
        .collect();
    CheckCatalogue {
        cases,
        features,
        runner: catalogue.runner.clone(),
    }
}

/// What a program's manifest says, as screens need it before it runs.
#[must_use]
pub fn program_info(
    programs: &Programs,
    manifest: &ProgramManifest,
    descriptor: Option<&AgentDescriptor>,
) -> ProgramInfo {
    let shipped = Programs::builtin().program(&manifest.kind) == Some(manifest);
    ProgramInfo {
        builtin: shipped,
        vendor: manifest.vendor.as_ref().map(|vendor| ProgramVendor {
            name: vendor.name.clone(),
            licence: vendor.licence.clone(),
            home_page: vendor.home_page.clone(),
        }),
        protocol: manifest.protocol.clone(),
        sources: ProgramSources {
            channels: manifest
                .builds
                .channels
                .iter()
                .map(|channel| ProgramChannel {
                    id: channel.id().to_owned(),
                    kind: match channel {
                        orch_agent_programs::Channel::GithubRelease(_) => {
                            ProgramChannelKind::GithubRelease
                        }
                        orch_agent_programs::Channel::Npm(_) => ProgramChannelKind::Npm,
                        orch_agent_programs::Channel::Pypi(_) => ProgramChannelKind::Pypi,
                        orch_agent_programs::Channel::SignedManifest(_) => {
                            ProgramChannelKind::SignedManifest
                        }
                    },
                    source: channel.source(),
                })
                .collect(),
            exact_fetch: manifest.builds.exact_fetch(),
        },
        checks: check_catalogue(manifest),
        carry: carry_of(
            descriptor.is_some_and(|descriptor| descriptor.capabilities.import_neutral),
            &[],
        ),
        executors: manifest
            .executors
            .iter()
            .filter_map(|kind| programs.executor(kind))
            .map(|executor| executor_info(programs, executor))
            .collect(),
        files: ProgramFilesInfo {
            instruction_files: manifest
                .files
                .instruction_files
                .iter()
                .map(|file| ProgramInstructionFile {
                    path: file.path.clone(),
                    in_home: file.scope == InstructionScope::Home,
                })
                .collect(),
            workspace_config: manifest.files.workspace_config.clone(),
        },
        colocated: manifest.placement.colocated,
        privacy: manifest
            .privacy
            .iter()
            .map(|switch| ProgramPrivacy {
                name: switch.name.clone(),
                how: switch.how.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offers_of_an_executor_are_its_sets_namespaced_with_the_old_word_as_their_alias() {
        let all = executors(Programs::builtin());
        let orch = all
            .iter()
            .find(|executor| executor.kind == "orch-exec")
            .unwrap();
        let values: Vec<_> = orch
            .offers
            .iter()
            .map(|offer| offer.value.as_str())
            .collect();
        assert_eq!(
            values,
            [
                "orch-exec:claude-code",
                "orch-exec:plain",
                "orch-exec:custom"
            ]
        );
        assert_eq!(orch.offers[1].alias.as_deref(), Some("plain"));
        let codex = all
            .iter()
            .find(|executor| executor.kind == "codex-exec-server")
            .unwrap();
        assert_eq!(codex.offers.len(), 1);
        assert_eq!(codex.offers[0].value, "codex-exec-server");
        assert_eq!(codex.program.as_deref(), Some("codex"));
    }

    #[test]
    fn an_external_executors_report_carries_localized_tool_set_text_without_identifier_fallback() {
        let report = serde_json::json!({
            "kind": "research-tools", "title": "Research tools", "titles": {"zh-CN": "研究工具"},
            "protocol": "mcp", "sandbox_modes": ["open"],
            "tool_sets": [{"name": "files-v2", "title": "Workspace files", "titles": {"zh-CN": "工作区文件"},
                "summary": "Reads the shared workspace", "summaries": {"zh-CN": "读取共享工作区"}}]
        });
        let facts = serde_json::json!({"plugins": [{"kind": "executor", "state": "running", "executor": report}]});
        let projected = machine_executors(Programs::builtin(), &facts);
        let executor = &projected
            .iter()
            .find(|row| row.executor.kind == "research-tools")
            .unwrap()
            .executor;
        assert_eq!(executor.title.get("zh-CN"), "研究工具");
        assert_eq!(executor.tool_sets[0].title.get("zh-CN"), "工作区文件");
        assert_eq!(executor.offers[0].summary.get("zh-CN"), "读取共享工作区");
        assert_eq!(executor.offers[0].value, "research-tools:files-v2");
        assert_eq!(executor.offers[0].alias, None);
        let mut legacy = report;
        legacy["tool_sets"][0]
            .as_object_mut()
            .unwrap()
            .remove("title");
        legacy["tool_sets"][0]
            .as_object_mut()
            .unwrap()
            .remove("titles");
        let legacy = serde_json::from_value(legacy).unwrap();
        assert_eq!(executor_info_of(&legacy).offers[0].title.en(), "");
    }

    #[test]
    fn the_check_of_a_program_is_titled_case_by_case_and_feature_by_feature() {
        let manifest = Programs::builtin().program("codex").unwrap();
        let info = program_info(Programs::builtin(), manifest, None);
        assert!(info.builtin);
        assert_eq!(info.checks.cases.len(), manifest.checks.conformance.len());
        assert_eq!(
            info.checks.cases[0].title.en(),
            "Starts with isolated settings"
        );
        let steering = info
            .checks
            .features
            .iter()
            .find(|feature| feature.id == "steer")
            .unwrap();
        assert_eq!(steering.title.en(), "steering a running turn");
        assert_eq!(info.sources.channels[0].source, "github.com/openai/codex");
        assert!(info.sources.exact_fetch);
        assert_eq!(info.executors[0].kind, "codex-exec-server");
        assert!(
            program_info(
                Programs::builtin(),
                Programs::builtin().program("fake").unwrap(),
                None
            )
            .sources
            .channels
            .is_empty()
        );
    }
}
