//! Selection of the current machine program uses its actual runtime build and exact consent tuple.

use std::collections::BTreeMap;

use orch_agent_contract::arrangement::{self, Arrangement, Incompatible, PlaceSide, ProviderSide};
use orch_agent_contract::descriptor::{
    AgentDescriptor, ArrangementEntry, ArrangementType, ExecutorBuild,
};
use orch_agent_programs::{ProgramManifest, Programs, executors::ExecutorWire};
use orch_link_contract::session::MachineFactsHarness;
use orch_settings_contract::version_override::{VersionOverrides, allowed};

pub(super) mod read;

fn executor_program(executor: &str) -> Option<&'static ProgramManifest> {
    let programs = Programs::builtin();
    let executor = programs.executor(executor)?;
    if executor.wire != ExecutorWire::Program {
        return None;
    }
    programs.program(executor.program.as_deref()?)
}

fn declared_program(entry: &ArrangementEntry) -> Option<&'static ProgramManifest> {
    (entry.kind == ArrangementType::Separated).then_some(())?;
    executor_program(entry.executor.as_deref()?)
}

pub(super) fn needed(descriptor: &AgentDescriptor) -> bool {
    descriptor
        .arrangements
        .iter()
        .any(|entry| declared_program(entry).is_some())
}

fn same_program(entry: &ArrangementEntry) -> bool {
    declared_program(entry).is_some() && entry.executor_version == Some(ExecutorBuild::Same)
}

fn version(reported: Option<&str>) -> &str {
    reported
        .filter(|version| !version.is_empty())
        .unwrap_or("unknown")
}

pub(super) fn candidates(
    descriptor: &AgentDescriptor,
    provider: &ProviderSide,
    place: &PlaceSide,
    programs: &BTreeMap<String, MachineFactsHarness>,
    reported_harness_version: Option<&str>,
    overrides: &VersionOverrides,
) -> Vec<(ArrangementType, Result<Arrangement, Incompatible>)> {
    let mut current = place.clone();
    // Inside the connected/no-cache domain, even Any/Range still require a confirmed program.
    current.executors.retain(|executor| {
        executor_program(&executor.kind).is_none_or(|manifest| {
            programs
                .get(&manifest.kind)
                .is_some_and(|program| executor.version == program.version)
        })
    });
    let mut original = arrangement::candidates(descriptor, provider, &current);
    if !descriptor.arrangements.iter().any(same_program) {
        return original;
    }
    let mut checked = descriptor.clone();
    checked.arrangements.retain_mut(|entry| {
        if same_program(entry) {
            let eligible = declared_program(entry)
                .and_then(|manifest| programs.get(&manifest.kind))
                .is_some_and(|program| {
                    allowed(
                        overrides,
                        &program.kind,
                        &program.path,
                        &program.version,
                        version(reported_harness_version),
                    )
                });
            if !eligible {
                return false;
            }
            // This candidate's Same condition has already been checked against the runtime row.
            entry.executor_version = Some(ExecutorBuild::Any);
        }
        true
    });
    let replacement = arrangement::candidates(&checked, provider, &current)
        .into_iter()
        .find(|(kind, _)| *kind == ArrangementType::Separated)
        .map(|(_, candidate)| candidate);
    for (kind, candidate) in &mut original {
        if *kind == ArrangementType::Separated {
            if let Some(replacement) = &replacement {
                *candidate = replacement.clone();
            } else if candidate.is_ok() {
                *candidate = Err(Incompatible::ExecutorMissing {
                    offers: place.offers.clone(),
                    usable: descriptor
                        .separated()
                        .filter_map(|entry| entry.executor.clone())
                        .collect(),
                });
            }
        }
    }
    original
}

pub(super) fn mismatch(
    descriptor: &AgentDescriptor,
    machine: &str,
    programs: &BTreeMap<String, MachineFactsHarness>,
    reported_harness_version: Option<&str>,
    overrides: &VersionOverrides,
    candidate: &Result<Arrangement, Incompatible>,
) -> Option<String> {
    if !matches!(candidate, Err(Incompatible::ExecutorMissing { .. })) {
        return None;
    }
    let agent = version(reported_harness_version);
    descriptor.separated().filter(|entry| same_program(entry)).find_map(|entry| {
        let manifest = declared_program(entry)?;
        let name = manifest.names.en();
        // A program the machine's owner has not confirmed supplies no tools, whatever its version.
        let Some(program) = programs.get(&manifest.kind) else {
            return Some(format!("{name} on {machine} is not confirmed. The machine's owner can confirm it on the machine's page."));
        };
        (!allowed(overrides,&program.kind,&program.path,&program.version,agent)).then(|| {
            format!("{name} on {machine} is {}; this agent runs {agent}. An environment manager can allow it on the environment's page.",program.version)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::arrangement::{CredentialLocation, Serves};
    use orch_agent_contract::credentials::CredentialForm;
    use orch_agent_contract::descriptor::SandboxMode;
    use orch_agent_contract::facts::ExecutorFact;
    use orch_settings_contract::version_override::VersionOverride;
    use orch_types::{Id, new_id};
    use serde_json::json;

    const EXECUTOR: &str = "codex-exec-server";

    fn candidates(
        descriptor: &AgentDescriptor,
        provider: &ProviderSide,
        place: &PlaceSide,
        program: Option<&MachineFactsHarness>,
        reported: Option<&str>,
        overrides: &VersionOverrides,
    ) -> Vec<(ArrangementType, Result<Arrangement, Incompatible>)> {
        let programs = program
            .into_iter()
            .map(|program| (program.kind.clone(), program.clone()))
            .collect();
        super::candidates(descriptor, provider, place, &programs, reported, overrides)
    }

    fn mismatch(
        descriptor: &AgentDescriptor,
        machine: &str,
        program: &MachineFactsHarness,
        reported: Option<&str>,
        overrides: &VersionOverrides,
        candidate: &Result<Arrangement, Incompatible>,
    ) -> Option<String> {
        let programs = BTreeMap::from([(program.kind.clone(), program.clone())]);
        super::mismatch(
            descriptor, machine, &programs, reported, overrides, candidate,
        )
    }

    fn descriptor(build: &str) -> AgentDescriptor {
        let mut value = crate::test_common::descriptor();
        value["kind"] = json!("codex");
        value["harness"]["version"] = json!("9.9.9");
        value["arrangements"] = json!([
            {"type":"separated","executor":EXECUTOR,"executor_version":build},
            {"type":"key_separated"},{"type":"together"}
        ]);
        serde_json::from_value(value).unwrap()
    }

    fn provider() -> ProviderSide {
        ProviderSide {
            serves: vec![Serves::Forwarded {
                protocol: "openai-responses".into(),
            }],
            issues: vec![CredentialForm::api_key()],
            credential: CredentialLocation::AtHome,
        }
    }

    fn place(version: &str) -> PlaceSide {
        PlaceSide {
            environment_id: new_id(),
            offers: vec![EXECUTOR.into()],
            executors: vec![ExecutorFact {
                kind: EXECUTOR.into(),
                version: version.into(),
                available: true,
                reason: None,
                sandbox_modes: vec![SandboxMode::Open],
                tool_sets: None,
            }],
            can_run_harness: false,
            same_machine: false,
            harness_version: None,
        }
    }

    fn program(version: &str) -> MachineFactsHarness {
        MachineFactsHarness {
            kind: "codex".into(),
            path: "/controlled/codex".into(),
            version: version.into(),
            daemon: false,
        }
    }

    fn consent(program: &MachineFactsHarness) -> VersionOverrides {
        VersionOverrides::from([(
            "codex".into(),
            VersionOverride {
                path: program.path.clone(),
                version: program.version.clone(),
                by: Id::nil(),
                at: "2026-10-31T00:00:00.000Z".parse().unwrap(),
            },
        )])
    }

    fn separated(candidates: &[(ArrangementType, Result<Arrangement, Incompatible>)]) -> bool {
        candidates
            .iter()
            .find(|(kind, _)| *kind == ArrangementType::Separated)
            .unwrap()
            .1
            .is_ok()
    }

    #[test]
    fn only_known_runtime_equality_or_current_consent_makes_this_same_candidate_eligible() {
        let descriptor = descriptor("same");
        let before = descriptor.clone();
        let provider = provider();
        for (program_version, reported, automatic) in [
            ("1.0.0", Some("1.0.0"), true),
            ("1.0.0", Some("2.0.0"), false),
            ("1.0.0", None, false),
            ("unknown", Some("unknown"), false),
            ("unknown", None, false),
        ] {
            let program = program(program_version);
            let place = place(program_version);
            assert_eq!(
                separated(&candidates(
                    &descriptor,
                    &provider,
                    &place,
                    Some(&program),
                    reported,
                    &VersionOverrides::new()
                )),
                automatic
            );
            assert!(separated(&candidates(
                &descriptor,
                &provider,
                &place,
                Some(&program),
                reported,
                &consent(&program)
            )));
        }
        assert_eq!(
            descriptor, before,
            "selection never edits the published adapter declaration"
        );
        let denied_program = program("1.0.0");
        let denied = candidates(
            &descriptor,
            &provider,
            &place("1.0.0"),
            Some(&denied_program),
            None,
            &VersionOverrides::new(),
        );
        let (_, candidate) = denied
            .iter()
            .find(|(kind, _)| *kind == ArrangementType::Separated)
            .unwrap();
        assert_eq!(
            mismatch(
                &descriptor,
                "Controlled machine",
                &denied_program,
                None,
                &VersionOverrides::new(),
                candidate
            )
            .as_deref(),
            Some(
                "Codex on Controlled machine is 1.0.0; this agent runs unknown. An environment manager can allow it on the environment's page."
            )
        );
        assert!(
            mismatch(
                &descriptor,
                "Controlled machine",
                &denied_program,
                None,
                &VersionOverrides::new(),
                &Err(Incompatible::NoCommonProtocol)
            )
            .is_none()
        );
        for fallback in ["1.0.0", "unknown"] {
            let mut descriptor = descriptor.clone();
            descriptor.harness.version = fallback.into();
            let program = program(fallback);
            let place = place(fallback);
            assert!(separated(&arrangement::candidates(
                &descriptor,
                &provider,
                &place
            )));
            assert!(
                !separated(&candidates(
                    &descriptor,
                    &provider,
                    &place,
                    Some(&program),
                    None,
                    &VersionOverrides::new()
                )),
                "descriptor equality cannot stand in for a missing actual runtime build"
            );
        }
    }

    #[test]
    fn stale_path_or_version_consent_and_stale_executor_facts_do_not_admit_a_new_program() {
        let descriptor = descriptor("same");
        let provider = provider();
        let program = program("1.0.0");
        for (path, approved_version) in
            [("/previous/codex", "1.0.0"), ("/controlled/codex", "0.9.0")]
        {
            let mut overrides = consent(&program);
            let entry = overrides.get_mut("codex").unwrap();
            entry.path = path.into();
            entry.version = approved_version.into();
            assert!(!separated(&candidates(
                &descriptor,
                &provider,
                &place("1.0.0"),
                Some(&program),
                None,
                &overrides
            )));
        }
        assert!(!separated(&candidates(
            &descriptor,
            &provider,
            &place("0.9.0"),
            Some(&program),
            None,
            &consent(&program)
        )));
        for (reported, overrides) in [
            (Some("1.0.0"), VersionOverrides::new()),
            (None, consent(&program)),
        ] {
            let stale = candidates(
                &descriptor,
                &provider,
                &place("0.9.0"),
                Some(&program),
                reported,
                &overrides,
            );
            let (_, candidate) = stale
                .iter()
                .find(|(kind, _)| *kind == ArrangementType::Separated)
                .unwrap();
            assert!(candidate.is_err());
            assert!(
                mismatch(
                    &descriptor,
                    "Controlled machine",
                    &program,
                    reported,
                    &overrides,
                    candidate
                )
                .is_none(),
                "stale executor facts do not ask for consent that is already satisfied"
            );
        }
    }

    #[test]
    fn a_machine_that_confirmed_no_program_is_told_so_instead_of_what_its_environment_offers() {
        let descriptor = descriptor("same");
        let provider = provider();
        let place = place("1.0.0");
        let none = BTreeMap::new();
        let refused = super::candidates(
            &descriptor,
            &provider,
            &place,
            &none,
            Some("1.0.0"),
            &VersionOverrides::new(),
        );
        let (_, candidate) = refused
            .iter()
            .find(|(kind, _)| *kind == ArrangementType::Separated)
            .unwrap();
        assert_eq!(
            super::mismatch(
                &descriptor,
                "Controlled machine",
                &none,
                Some("1.0.0"),
                &VersionOverrides::new(),
                candidate
            )
            .as_deref(),
            Some(
                "Codex on Controlled machine is not confirmed. The machine's owner can confirm it on the machine's page."
            )
        );
    }

    #[test]
    fn absent_confirmation_is_unavailable_for_codex_same_any_and_ranges() {
        let provider = provider();
        let place = place("1.0.0");
        for build in ["same", "*", ">=1.0.0, <2.0.0"] {
            let descriptor = descriptor(build);
            assert!(!separated(&candidates(
                &descriptor,
                &provider,
                &place,
                None,
                Some("1.0.0"),
                &VersionOverrides::new()
            )));
        }
    }

    #[test]
    fn explicit_any_ranges_and_other_arrangements_keep_their_existing_candidates() {
        let provider = provider();
        let place = place("1.0.0");
        let program = program("1.0.0");
        for build in ["*", ">=1.0.0, <2.0.0", ">=2.0.0"] {
            let descriptor = descriptor(build);
            assert_eq!(
                candidates(
                    &descriptor,
                    &provider,
                    &place,
                    Some(&program),
                    None,
                    &consent(&program)
                ),
                arrangement::candidates(&descriptor, &provider, &place)
            );
        }
        let descriptor = descriptor("same");
        let before = arrangement::candidates(&descriptor, &provider, &place);
        let after = candidates(
            &descriptor,
            &provider,
            &place,
            Some(&program),
            None,
            &VersionOverrides::new(),
        );
        assert_eq!(
            after
                .iter()
                .filter(|(kind, _)| *kind != ArrangementType::Separated)
                .collect::<Vec<_>>(),
            before
                .iter()
                .filter(|(kind, _)| *kind != ArrangementType::Separated)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_eligible_original_any_or_range_candidate_does_not_report_a_same_mismatch() {
        let provider = provider();
        let place = place("1.0.0");
        let program = program("1.0.0");
        for build in ["*", ">=1.0.0, <2.0.0"] {
            let alternative = descriptor(build).arrangements.remove(0);
            let mut descriptor = descriptor("same");
            descriptor.arrangements.insert(1, alternative);
            let found = candidates(
                &descriptor,
                &provider,
                &place,
                Some(&program),
                None,
                &VersionOverrides::new(),
            );
            let (_, candidate) = found
                .iter()
                .find(|(kind, _)| *kind == ArrangementType::Separated)
                .unwrap();
            assert!(candidate.is_ok());
            assert!(
                mismatch(
                    &descriptor,
                    "Controlled machine",
                    &program,
                    None,
                    &VersionOverrides::new(),
                    candidate
                )
                .is_none()
            );
        }
    }
}
