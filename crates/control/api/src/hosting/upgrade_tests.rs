//! Every program uses the native build selector; no vendor-specific version policy.
#![allow(clippy::unwrap_used, reason = "fixture assertions")]
use super::*;
use orch_api_contract::admin::{HarnessArch, HarnessBuildSource};
use orch_api_contract::resources::MachineOs;

fn listed(kind: &str, version: &str, default: bool, status: HarnessBuildStatus) -> HarnessBuild {
    HarnessBuild {
        kind: kind.into(),
        version: version.into(),
        os: MachineOs::Linux,
        arch: HarnessArch::X86_64,
        url: format!("https://builds.example.test/{kind}-{version}"),
        sha256: "a".repeat(64),
        is_default: default,
        source: if default {
            HarnessBuildSource::Release
        } else {
            HarnessBuildSource::Channel
        },
        status,
        check: None,
        carry: None,
    }
}

fn programs() -> [(&'static str, [&'static str; 4]); 4] {
    [
        ("codex", ["0.164.0", "0.165.0", "0.166.0", "0.167.0"]),
        ("claude-code", ["2.1.311", "2.1.312", "2.1.315", "2.1.319"]),
        ("dsh", ["0.2.1-rc.1", "0.2.1-rc.2", "0.2.1-rc.3", "0.2.1"]),
        ("fourth-program", ["1.0.0", "1.1.0", "1.2.0", "1.3.0"]),
    ]
}

#[test]
fn every_program_resolves_shipped_checked_newest_and_exact_without_retargeting_a_recorded_pin() {
    for (kind, [older, shipped, checked, newest]) in programs() {
        let mut builds = vec![
            listed(kind, older, false, HarnessBuildStatus::Checked),
            listed(kind, shipped, true, HarnessBuildStatus::Checked),
            listed(kind, checked, false, HarnessBuildStatus::Checked),
            listed(kind, newest, false, HarnessBuildStatus::Untested),
        ];
        let request = |version: Option<&str>, newest| HarnessRequest {
            kind: kind.into(),
            version: version.map(str::to_owned),
            newest,
        };
        let pick = |builds: &[HarnessBuild], request| {
            requested_of(builds, "linux", "x86_64", &[request]).unwrap()[0]
                .version
                .clone()
        };
        assert_eq!(pick(&builds, request(None, None)), shipped);
        assert_eq!(pick(&builds, request(None, Some(Newest::Checked))), checked);
        assert_eq!(pick(&builds, request(None, Some(Newest::Any))), newest);
        assert_eq!(
            pick(&builds, request(Some(older), None)),
            older,
            "an explicit downgrade is an ordinary exact request"
        );
        builds[1].is_default = false;
        builds[2].is_default = true;
        assert_eq!(
            pick(&builds, request(Some(shipped), None)),
            shipped,
            "a catalogue change cannot silently retarget the unit's recorded exact build"
        );
        assert!(
            requested_of(&builds, "linux", "aarch64", &[request(Some(shipped), None)]).is_err()
        );
        assert!(
            requested_of(
                &builds,
                "linux",
                "x86_64",
                &[request(Some("unlisted"), None)]
            )
            .is_err()
        );
    }
}

#[test]
fn checksum_refusal_remains_a_refusal_for_every_program_and_policy() {
    for (kind, [_, shipped, _, _]) in programs() {
        let mut build = listed(kind, shipped, true, HarnessBuildStatus::Failed);
        build.check=Some(serde_json::from_value(serde_json::json!({"at":"2026-10-08T14:00:00Z","machine":null,"missing":[],"off":[],"error":"refused: checksum mismatch","stage":"complete","cases":[]})).unwrap());
        for (version, newest) in [
            (None, None),
            (None, Some(Newest::Checked)),
            (None, Some(Newest::Any)),
            (Some(shipped.to_owned()), None),
        ] {
            let request = HarnessRequest {
                kind: kind.into(),
                version,
                newest,
            };
            let error = requested_of(std::slice::from_ref(&build), "linux", "x86_64", &[request])
                .unwrap_err();
            assert!(error.to_string().contains("checksum"));
        }
    }
}

#[test]
fn mixed_machine_versions_are_independent_while_one_unit_cannot_satisfy_conflicting_pins() {
    for (kind, [older, shipped, checked, _]) in programs() {
        let builds = vec![
            listed(kind, older, false, HarnessBuildStatus::Checked),
            listed(kind, shipped, true, HarnessBuildStatus::Checked),
            listed(kind, checked, false, HarnessBuildStatus::Checked),
        ];
        let request = |version: &str| HarnessRequest {
            kind: kind.into(),
            version: Some(version.into()),
            newest: None,
        };
        let first = requested_of(&builds, "linux", "x86_64", &[request(shipped)]).unwrap();
        let other = requested_of(&builds, "linux", "x86_64", &[request(checked)]).unwrap();
        assert_ne!(first[0].version, other[0].version);
        let rollback = requested_of(&builds, "linux", "x86_64", &[request(older)]).unwrap();
        assert_eq!(rollback[0].version, older);
        assert!(
            requested_of(
                &builds,
                "linux",
                "x86_64",
                &[request(shipped), request(checked)]
            )
            .is_err()
        );
    }
}

#[test]
fn a_python_index_pre_release_pin_keeps_the_canonical_existing_build_identity() {
    let builds = [listed(
        "dsh",
        "0.2.1-rc.2",
        true,
        HarnessBuildStatus::Checked,
    )];
    let request = HarnessRequest {
        kind: "dsh".into(),
        version: Some("0.2.1rc2".into()),
        newest: None,
    };
    assert_eq!(
        requested_of(&builds, "linux", "x86_64", &[request]).unwrap()[0].version,
        "0.2.1-rc.2"
    );
}
