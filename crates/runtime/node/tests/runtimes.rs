//! The commands about runtimes, on the in-memory hub with the scripted controller and the in-memory driver: a unit is made, started, stopped, recreated as a successor on the same volumes, and removed, and the system is told what became of it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::{PATIENCE, Rig, SYSTEM};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::runtime::{RuntimeKind, VolumeRole};
use orch_driver_contract::units::UnitStateKind;
use orch_link::commands::Progress;
use orch_link_contract::commands::{
    NodeCapabilitiesApply, NodeRuntimeStart, NodeRuntimeStop, NodeVolumeRemove, SerialKey,
};
use orch_node_core::ledger::Ledger;
use orch_testkit::doubles::driver::{MemoryDriver, Operation};
use orch_types::new_id;
use serde_json::json;

async fn send_original_stop(
    rig: &Rig,
    runtime: orch_types::Id,
    target: orch_link_contract::native_target::LifecycleTarget,
) -> Progress {
    let mut frame = orch_link_contract::commands::Cmd::new(
        new_id(),
        &SerialKey::Runtime(runtime),
        rig.deadline(),
        &NodeRuntimeStop {
            runtime_id: runtime,
            reason: None,
        },
    )
    .unwrap();
    frame.native_target = Some(orch_link_contract::native_target::NativeTarget::Lifecycle(
        target,
    ));
    let mut handle = rig.controller.link().commands().issue(frame);
    tokio::time::timeout(PATIENCE, handle.wait()).await.unwrap()
}

async fn send_original_start(
    rig: &Rig,
    runtime: orch_types::Id,
    target: orch_link_contract::native_target::LifecycleTarget,
) -> Progress {
    let mut frame = orch_link_contract::commands::Cmd::new(
        new_id(),
        &SerialKey::Runtime(runtime),
        rig.deadline(),
        &NodeRuntimeStart {
            runtime_id: runtime,
        },
    )
    .unwrap();
    frame.native_target = Some(orch_link_contract::native_target::NativeTarget::Lifecycle(
        target,
    ));
    let mut handle = rig.controller.link().commands().issue(frame);
    tokio::time::timeout(PATIENCE, handle.wait()).await.unwrap()
}

fn original_create_frame(
    rig: &Rig,
    spec: &orch_driver_contract::runtime::RuntimeSpec,
    target: orch_link_contract::native_target::LifecycleTarget,
) -> orch_link_contract::commands::Cmd {
    let command = new_id();
    let mut frame = orch_link_contract::commands::Cmd::new(
        command,
        &SerialKey::Runtime(spec.runtime_id),
        rig.deadline(),
        &rig.create_body(spec, command),
    )
    .unwrap();
    frame.native_target = Some(orch_link_contract::native_target::NativeTarget::Lifecycle(
        target,
    ));
    frame
}

#[tokio::test]
async fn a_new_native_unit_has_its_admitted_birth_identity_and_a_retry_never_relabels_it() {
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "admitted-native-birth");
        let birth = new_id();
        let target = LifecycleTarget {
            subject: LifecycleSubject::Runtime {
                runtime_id: spec.runtime_id,
            },
            observed: None,
            opening: Some(birth),
            pending: Vec::new(),
            revision: 1,
        };
        let frame = original_create_frame(&rig, &spec, target.clone());
        let mut first = rig.controller.link().commands().issue(frame.clone());
        let Progress::Succeeded(result) =
            tokio::time::timeout(PATIENCE, first.wait()).await.unwrap()
        else {
            panic!(
                "the actual native Create must receive its admitted birth: {:?}",
                first.progress()
            );
        };
        assert_eq!(result["source"]["incarnation"], json!(birth));
        assert_eq!(
            rig.role()
                .ledger()
                .get(spec.runtime_id)
                .unwrap()
                .incarnation
                .id(),
            Some(birth)
        );
        let unit = rig.driver.unit(&spec.name).unwrap().unit;
        let mut retry = rig.controller.link().commands().issue(frame);
        assert_eq!(
            tokio::time::timeout(PATIENCE, retry.wait()).await.unwrap(),
            Progress::Succeeded(result)
        );
        assert_eq!(rig.driver.targets_of(Operation::Create).len(), 1);
        let other = LifecycleTarget {
            opening: Some(new_id()),
            revision: 2,
            ..target
        };
        let frame = original_create_frame(&rig, &spec, other);
        let mut refused = rig.controller.link().commands().issue(frame);
        assert!(matches!(
            tokio::time::timeout(PATIENCE, refused.wait())
                .await
                .unwrap(),
            Progress::Failed(_)
        ));
        assert_eq!(rig.driver.unit(&spec.name).unwrap().unit, unit);
        assert_eq!(rig.driver.targets_of(Operation::Create).len(), 1);
        assert_eq!(
            rig.role()
                .ledger()
                .get(spec.runtime_id)
                .unwrap()
                .incarnation
                .id(),
            Some(birth)
        );
        rig.stop().await;
    }
}

#[tokio::test]
async fn an_actual_created_unit_recovers_its_original_metadata_after_the_ledger_write_fails() {
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget, RuntimeSource};
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let role = rig.role();
    let spec = rig.spec(RuntimeKind::Environment, "recover-created-unit");
    let command = new_id();
    let cancel = tokio_util::sync::CancellationToken::new();
    let body = serde_json::to_value(rig.create_body(&spec, command).0).unwrap();
    let target = LifecycleTarget {
        subject: LifecycleSubject::Runtime {
            runtime_id: spec.runtime_id,
        },
        observed: None,
        opening: Some(new_id()),
        pending: Vec::new(),
        revision: 1,
    };
    let directory = rig.data.path().join("actual-native-receipt");
    let fences = NativeFences::new(directory.clone());
    let ledger_file = rig
        .settings
        .data_dir
        .join("state")
        .join(orch_node_core::ledger::LEDGER_FILE);
    let backup = rig.data.path().join("before-creation.json");
    rig.driver.hold(Operation::Create);
    {
        let creating = role
            .runtimes()
            .create_original(command, body, &cancel, &target, &fences, None);
        tokio::pin!(creating);
        tokio::time::timeout(PATIENCE, async {
            loop {
                assert!(futures::poll!(&mut creating).is_pending());
                if !rig.driver.targets_of(Operation::Create).is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        std::fs::rename(&ledger_file, &backup).unwrap();
        std::fs::create_dir(&ledger_file).unwrap();
        rig.driver.release(Operation::Create);
        assert!(
            creating.await.is_err(),
            "the actual main journal commit must fail"
        );
    }
    assert!(
        rig.driver.unit(&spec.name).is_some(),
        "the actual driver already created the original"
    );
    assert!(role.ledger().get(spec.runtime_id).is_none());
    let stored = fences
        .uninstalled_receipt(&target.subject)
        .unwrap()
        .expect("the real driver receipt precedes the main ledger commit");
    assert!(
        !serde_json::to_string(&stored.value)
            .unwrap()
            .contains(spec.enroll_token.expose()),
        "native recovery stores no enrollment credential"
    );
    std::fs::remove_dir(&ledger_file).unwrap();
    std::fs::rename(&backup, &ledger_file).unwrap();
    let restarted = NativeFences::new(directory);
    let stop = LifecycleTarget {
        opening: None,
        pending: vec![target.opening.unwrap()],
        revision: 2,
        ..target.clone()
    };
    let result = role
        .runtimes()
        .stop_original(&stop, &restarted, None, None)
        .await
        .unwrap()
        .expect("the actual original receipt supports Stop without re-Create");
    assert_eq!(
        result,
        orch_link_contract::commands::NodeRuntimeStopResult::Stopped {
            source: RuntimeSource {
                incarnation: target.opening
            }
        }
    );
    let restored = role
        .ledger()
        .get(spec.runtime_id)
        .expect("the full original record is installed");
    assert_eq!(restored.unit, rig.driver.unit(&spec.name).unwrap().unit);
    assert_eq!(restored.incarnation.id(), target.opening);
    assert!(role.ledger().capabilities(spec.runtime_id).is_some());
    assert!(role.ledger().mounts(spec.runtime_id).is_some());
    assert!(role.ledger().network(spec.runtime_id).is_some());
    assert!(role.ledger().about(spec.runtime_id).is_some());
    assert_eq!(rig.driver.targets_of(Operation::Create).len(), 1);
    assert!(
        restarted
            .uninstalled_receipt(&stop.subject)
            .unwrap()
            .is_none()
    );
    role.ledger().remove(spec.runtime_id).unwrap();
    let later = LifecycleTarget {
        revision: 3,
        ..stop
    };
    assert!(
        role.runtimes()
            .stop_original(&later, &restarted, None, None)
            .await
            .is_err(),
        "an acknowledged old receipt cannot recreate a lost current source later"
    );
    assert!(role.ledger().get(spec.runtime_id).is_none());
    rig.stop().await;
}

#[tokio::test]
async fn a_new_birth_cannot_claim_an_existing_source_by_copying_its_identity() {
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "copied-native-birth");
    let unit = rig.made(&spec).await;
    let source = rig
        .role()
        .ledger()
        .get(spec.runtime_id)
        .unwrap()
        .incarnation
        .id()
        .unwrap();
    let target = LifecycleTarget {
        subject: LifecycleSubject::Runtime {
            runtime_id: spec.runtime_id,
        },
        observed: None,
        opening: Some(source),
        pending: Vec::new(),
        revision: 1,
    };
    let frame = original_create_frame(&rig, &spec, target);
    let mut claimed = rig.controller.link().commands().issue(frame);
    assert!(
        matches!(
            tokio::time::timeout(PATIENCE, claimed.wait())
                .await
                .unwrap(),
            Progress::Failed(_)
        ),
        "a matching UUID alone is not this original's creation receipt"
    );
    assert_eq!(rig.driver.unit(&spec.name).unwrap().unit, unit);
    assert_eq!(rig.driver.targets_of(Operation::Create).len(), 1);
    assert_eq!(
        rig.role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id(),
        Some(source)
    );
    rig.stop().await;
}

#[tokio::test]
async fn an_admitted_stop_prevents_an_older_birth_from_creating_a_unit_after_its_image_wait() {
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget, NativeTarget};
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "withdrawn-native-birth");
        let birth = new_id();
        let target = LifecycleTarget {
            subject: LifecycleSubject::Runtime {
                runtime_id: spec.runtime_id,
            },
            observed: None,
            opening: Some(birth),
            pending: Vec::new(),
            revision: 1,
        };
        rig.driver.hold(Operation::EnsureImage);
        struct Release(std::sync::Arc<MemoryDriver>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.release(Operation::EnsureImage);
            }
        }
        let release = Release(rig.driver.clone());
        let frame = original_create_frame(&rig, &spec, target.clone());
        let mut creating = rig.controller.link().commands().issue(frame);
        rig.until("the birth's image wait or its original refusal", || {
            !rig.driver.targets_of(Operation::EnsureImage).is_empty()
                || creating.progress().is_final()
        })
        .await;
        assert!(
            !creating.progress().is_final(),
            "the original birth must reach its actual preparation wait: {:?}",
            creating.progress()
        );
        assert!(rig.driver.targets_of(Operation::Create).is_empty());
        let mut frame = orch_link_contract::commands::Cmd::new(
            new_id(),
            &SerialKey::Runtime(spec.runtime_id),
            rig.deadline(),
            &NodeRuntimeStop {
                runtime_id: spec.runtime_id,
                reason: None,
            },
        )
        .unwrap();
        frame.native_target = Some(NativeTarget::Lifecycle(LifecycleTarget {
            opening: None,
            pending: vec![birth],
            revision: 2,
            ..target
        }));
        let mut stopping = rig.controller.link().commands().issue(frame);
        rig.until("the later Stop's durable native acceptance", || {
            matches!(stopping.progress(), Progress::Accepted) || stopping.progress().is_final()
        })
        .await;
        assert_eq!(stopping.progress(), Progress::Accepted);
        drop(release);
        assert!(matches!(
            tokio::time::timeout(PATIENCE, creating.wait())
                .await
                .unwrap(),
            Progress::Failed(_)
        ));
        let Progress::Succeeded(result) = tokio::time::timeout(PATIENCE, stopping.wait())
            .await
            .unwrap()
        else {
            panic!(
                "the exact closed birth is confirmed absent: {:?}",
                stopping.progress()
            );
        };
        assert_eq!(result, json!({"outcome":"absent","originals":[birth]}));
        assert!(rig.driver.targets_of(Operation::Create).is_empty());
        assert!(rig.driver.targets_of(Operation::Stop).is_empty());
        assert!(rig.driver.targets_of(Operation::Remove).is_empty());
        assert!(rig.driver.targets_of(Operation::RemoveVolume).is_empty());
        assert!(rig.role().ledger().get(spec.runtime_id).is_none());
        rig.stop().await;
    }
}

#[tokio::test]
async fn losing_a_native_start_never_confirms_its_proposed_source_stopped_without_its_receipt() {
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "unsettled-native-start");
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(true));
    let role = rig.role();
    let observed = role
        .ledger()
        .get(spec.runtime_id)
        .unwrap()
        .incarnation
        .id()
        .unwrap();
    let opening = new_id();
    let target = LifecycleTarget {
        subject: LifecycleSubject::Runtime {
            runtime_id: spec.runtime_id,
        },
        observed: Some(observed),
        opening: Some(opening),
        pending: Vec::new(),
        revision: 1,
    };
    let directory = rig.data.path().join("unsettled-native-start");
    let fences = NativeFences::new(directory.clone());
    rig.driver.hold(Operation::Start);
    {
        let starting = role.runtimes().start_original(&target, &fences, None);
        tokio::pin!(starting);
        tokio::time::timeout(PATIENCE, async {
            loop {
                assert!(futures::poll!(&mut starting).is_pending());
                if !rig.driver.targets_of(Operation::Start).is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            role.ledger().get(spec.runtime_id).unwrap().incarnation.id(),
            Some(opening)
        );
    }
    rig.driver.release(Operation::Start);
    let restarted = NativeFences::new(directory);
    let stop = LifecycleTarget {
        opening: None,
        pending: vec![opening],
        revision: 2,
        ..target
    };
    assert!(
        role.runtimes()
            .stop_original(&stop, &restarted, None, None)
            .await
            .is_err(),
        "the retained proposed source and an empty/stopped inspection are not a completed first-effect receipt"
    );
    assert!(rig.driver.targets_of(Operation::Stop).is_empty());
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    rig.stop().await;
}

#[tokio::test]
async fn losing_a_started_native_birth_keeps_an_empty_slot_unconfirmed_until_its_effect_is_settled()
{
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let role = rig.role();
    let spec = rig.spec(RuntimeKind::Environment, "unsettled-native-birth");
    let command = new_id();
    let body = serde_json::to_value(rig.create_body(&spec, command).0).unwrap();
    let target = LifecycleTarget {
        subject: LifecycleSubject::Runtime {
            runtime_id: spec.runtime_id,
        },
        observed: None,
        opening: Some(new_id()),
        pending: Vec::new(),
        revision: 1,
    };
    let directory = rig.data.path().join("unsettled-native-birth");
    let fences = NativeFences::new(directory.clone());
    rig.driver.hold(Operation::Create);
    let cancel = tokio_util::sync::CancellationToken::new();
    {
        let creating = role
            .runtimes()
            .create_original(command, body, &cancel, &target, &fences, None);
        tokio::pin!(creating);
        tokio::time::timeout(PATIENCE, async {
            loop {
                assert!(futures::poll!(&mut creating).is_pending());
                if !rig.driver.targets_of(Operation::Create).is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(role.ledger().get(spec.runtime_id).is_none());
        assert!(rig.driver.unit(&spec.name).is_none());
        // Dropping this waiter supplies no daemon receipt, even though this fake has not allocated yet.
    }
    rig.driver.release(Operation::Create);
    let restarted = NativeFences::new(directory);
    let stop = LifecycleTarget {
        opening: None,
        pending: vec![target.opening.unwrap()],
        revision: 2,
        ..target
    };
    assert!(
        role.runtimes()
            .stop_original(&stop, &restarted, None, None)
            .await
            .is_err(),
        "an empty listing is not proof that an unresolved first effect is closed"
    );
    assert!(role.ledger().get(spec.runtime_id).is_none());
    assert!(rig.driver.targets_of(Operation::Stop).is_empty());
    assert!(rig.driver.targets_of(Operation::Remove).is_empty());
    rig.stop().await;
}

#[tokio::test]
async fn missing_native_records_confirm_absence_only_after_successful_driver_observations() {
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::commands::NodeRuntimeStopResult;
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "absent-owned-birth");
    let original = new_id();
    let pending = new_id();
    let target = LifecycleTarget {
        subject: LifecycleSubject::Runtime {
            runtime_id: spec.runtime_id,
        },
        observed: Some(original),
        opening: None,
        pending: vec![pending],
        revision: 1,
    };
    let fences = NativeFences::new(rig.data.path().join("closed-native-birth"));
    let role = rig.role();
    let result = role
        .runtimes()
        .stop_original(&target, &fences, None, None)
        .await
        .unwrap()
        .expect("successful complete observations prove exact originals absent");
    let mut originals = vec![original, pending];
    originals.sort_unstable();
    assert_eq!(result, NodeRuntimeStopResult::Absent { originals });
    assert!(role.ledger().get(spec.runtime_id).is_none());
    let target = LifecycleTarget {
        revision: 2,
        ..target
    };
    rig.driver.fail_next(
        Operation::List,
        DriverError::Unavailable("the native unit list is unavailable".into()),
    );
    assert!(
        role.runtimes()
            .stop_original(&target, &fences, None, None)
            .await
            .is_err(),
        "a failed observation cannot confirm absence"
    );
    assert!(rig.driver.targets_of(Operation::Stop).is_empty());
    assert!(rig.driver.targets_of(Operation::Remove).is_empty());
    rig.stop().await;
}

#[tokio::test]
async fn an_untracked_existing_unit_is_never_adopted_by_a_new_birth_or_confirmed_absent() {
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "unknown-native-birth");
    let actual = rig.driver.create(&spec).await.unwrap();
    assert!(rig.role().ledger().get(spec.runtime_id).is_none());
    let target = LifecycleTarget {
        subject: LifecycleSubject::Runtime {
            runtime_id: spec.runtime_id,
        },
        observed: None,
        opening: Some(new_id()),
        pending: Vec::new(),
        revision: 1,
    };
    let frame = original_create_frame(&rig, &spec, target.clone());
    let mut creating = rig.controller.link().commands().issue(frame);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, creating.wait())
            .await
            .unwrap(),
        Progress::Failed(_)
    ));
    let target = LifecycleTarget {
        observed: Some(new_id()),
        opening: None,
        revision: 2,
        ..target
    };
    assert!(matches!(
        send_original_stop(&rig, spec.runtime_id, target).await,
        Progress::Failed(_)
    ));
    assert_eq!(rig.driver.unit(&spec.name).unwrap().unit, actual);
    assert_eq!(
        rig.driver.targets_of(Operation::Create).len(),
        1,
        "only the fixture's original allocation happened"
    );
    assert!(rig.driver.targets_of(Operation::Stop).is_empty());
    assert!(rig.driver.targets_of(Operation::Remove).is_empty());
    assert!(rig.role().ledger().get(spec.runtime_id).is_none());
    rig.stop().await;
}

#[tokio::test]
async fn the_native_role_starts_only_the_original_unit_with_its_admitted_candidate_identity() {
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "role-original-start");
        rig.made(&spec).await;
        rig.driver.set_enrolled(&spec.name, Some(true));
        let original = rig
            .role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        let opening = new_id();
        let start = LifecycleTarget {
            pending: Vec::new(),
            subject: LifecycleSubject::Runtime {
                runtime_id: spec.runtime_id,
            },
            observed: Some(original),
            opening: Some(opening),
            revision: 1,
        };
        let Progress::Succeeded(first_reply) =
            send_original_start(&rig, spec.runtime_id, start.clone()).await
        else {
            panic!("the admitted original unit must start");
        };
        assert_eq!(first_reply["source"]["incarnation"], json!(opening));
        assert_eq!(first_reply["reused"], false);
        assert_eq!(
            rig.role()
                .ledger()
                .get(spec.runtime_id)
                .unwrap()
                .incarnation
                .id(),
            Some(opening)
        );
        let reused = LifecycleTarget {
            observed: Some(opening),
            opening: Some(new_id()),
            revision: 2,
            ..start.clone()
        };
        let Progress::Succeeded(reuse_reply) =
            send_original_start(&rig, spec.runtime_id, reused).await
        else {
            panic!("the actual running unit must be reused");
        };
        assert_eq!(reuse_reply["source"]["incarnation"], json!(opening));
        assert_eq!(reuse_reply["reused"], true);
        assert_eq!(
            rig.role()
                .ledger()
                .get(spec.runtime_id)
                .unwrap()
                .incarnation
                .id(),
            Some(opening)
        );
        assert!(matches!(
            rig.stop_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let started = rig.driver.targets_of(Operation::Start).len();
        let stale = LifecycleTarget {
            opening: Some(new_id()),
            revision: 3,
            ..start
        };
        assert!(
            matches!(send_original_start(&rig, spec.runtime_id, stale).await,
            Progress::Failed(error) if error.known() == Some(orch_link_contract::commands::ErrorCode::Conflict))
        );
        assert_eq!(rig.driver.targets_of(Operation::Start).len(), started);
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
        rig.stop().await;
    }
}

#[tokio::test]
async fn the_native_role_never_ignores_an_old_or_foreign_runtime_target() {
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "role-original-stop");
        rig.made(&spec).await;
        rig.driver.set_enrolled(&spec.name, Some(true));
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let original = rig
            .role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        let old = LifecycleTarget {
            pending: Vec::new(),
            subject: LifecycleSubject::Runtime {
                runtime_id: spec.runtime_id,
            },
            observed: Some(original),
            opening: None,
            revision: 1,
        };
        assert!(matches!(
            rig.stop_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let replacement = rig
            .role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        assert_ne!(replacement, original);
        let stopped = rig.driver.targets_of(Operation::Stop).len();
        assert!(
            matches!(send_original_stop(&rig, spec.runtime_id, old.clone()).await,
            Progress::Failed(error) if error.known() == Some(orch_link_contract::commands::ErrorCode::Conflict))
        );
        assert_eq!(rig.driver.targets_of(Operation::Stop).len(), stopped);
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
        let foreign = LifecycleTarget {
            subject: LifecycleSubject::Runtime {
                runtime_id: new_id(),
            },
            observed: Some(replacement),
            revision: 2,
            ..old.clone()
        };
        assert!(
            matches!(send_original_stop(&rig, spec.runtime_id, foreign).await,
            Progress::Failed(error) if error.known() == Some(orch_link_contract::commands::ErrorCode::Conflict))
        );
        assert_eq!(rig.driver.targets_of(Operation::Stop).len(), stopped);
        let current = LifecycleTarget {
            observed: Some(replacement),
            revision: 3,
            ..old
        };
        assert!(matches!(
            send_original_stop(&rig, spec.runtime_id, current).await,
            Progress::Succeeded(_)
        ));
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
        rig.stop().await;
    }
}

#[tokio::test]
async fn a_created_units_reply_keeps_its_actual_native_identity_across_reuse_and_node_restart() {
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "created-native-source");
        let Progress::Succeeded(created) = rig.create(&spec).await else {
            panic!("the actual unit must be created");
        };
        let original = rig
            .role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        assert_ne!(original, spec.runtime_id);
        assert_eq!(created["source"]["incarnation"], json!(original));
        let Progress::Succeeded(reused) = rig.create(&spec).await else {
            panic!("the same stopped unit must be reused");
        };
        assert_eq!(reused["unit"], created["unit"]);
        assert_eq!(reused["source"], created["source"]);
        rig.driver.set_enrolled(&spec.name, Some(true));
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let running = rig
            .role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        assert_ne!(running, original);
        let Progress::Succeeded(reused_running) = rig.create(&spec).await else {
            panic!("the genuinely running unit must be reused");
        };
        assert_eq!(reused_running["unit"], created["unit"]);
        assert_eq!(reused_running["source"]["incarnation"], json!(running));
        rig.stop().await;
        rig.start().await;
        let Progress::Succeeded(after_restart) = rig.create(&spec).await else {
            panic!("node restart keeps the original recorded unit");
        };
        assert_eq!(after_restart["unit"], created["unit"]);
        assert_eq!(after_restart["source"], reused_running["source"]);
        rig.stop().await;
    }
}

#[tokio::test]
async fn a_removed_runtime_reports_its_original_native_identity_after_its_local_record_is_gone() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "removed-original-source");
    rig.made(&spec).await;
    let original = rig
        .role()
        .ledger()
        .get(spec.runtime_id)
        .unwrap()
        .incarnation
        .id()
        .unwrap();
    assert!(matches!(
        rig.remove_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    rig.wait_told("the original removed native source", |events| {
        events.iter().any(|event| {
            event.kind == "node.runtime"
                && event.body["runtime_id"] == json!(spec.runtime_id)
                && event.body["state"] == "absent"
                && event.body["source"]["incarnation"] == json!(original)
        })
    })
    .await;
    assert!(rig.role().ledger().get(spec.runtime_id).is_none());
    rig.stop().await;
}

#[tokio::test]
async fn an_original_native_start_keeps_its_proposed_identity_and_cannot_override_a_newer_stop_or_retarget_reuse()
 {
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::native_target::{LifecycleAction, LifecycleSubject, LifecycleTarget};
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "original-native-start");
        rig.made(&spec).await;
        rig.driver.set_enrolled(&spec.name, Some(true));
        let role = rig.role();
        let original = role
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        let fences = NativeFences::new(rig.data.path().join("original-native-start"));
        let subject = LifecycleSubject::Runtime {
            runtime_id: spec.runtime_id,
        };
        let stop = LifecycleTarget {
            pending: Vec::new(),
            subject: subject.clone(),
            observed: Some(original),
            opening: None,
            revision: 2,
        };
        assert!(
            fences
                .observe_lifecycle(&stop, LifecycleAction::Stop)
                .unwrap()
        );
        let opening = new_id();
        let old = LifecycleTarget {
            opening: Some(opening),
            revision: 1,
            ..stop.clone()
        };
        assert!(
            role.runtimes()
                .start_original(&old, &fences, None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(rig.driver.targets_of(Operation::Start).is_empty());
        let current = LifecycleTarget { revision: 3, ..old };
        rig.driver.hold(Operation::Start);
        struct Release(std::sync::Arc<MemoryDriver>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.release(Operation::Start);
            }
        }
        let release = Release(rig.driver.clone());
        let starting = role.runtimes().start_original(&current, &fences, None);
        tokio::pin!(starting);
        tokio::time::timeout(PATIENCE, async {
            loop {
                let before = rig.driver.targets_of(Operation::Start).len();
                assert!(futures::poll!(&mut starting).is_pending());
                if rig.driver.targets_of(Operation::Start).len() > before {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            role.ledger().get(spec.runtime_id).unwrap().incarnation.id(),
            Some(opening)
        );
        drop(release);
        assert!(starting.await.unwrap().is_some());
        let reuse = LifecycleTarget {
            pending: Vec::new(),
            subject: subject.clone(),
            observed: Some(opening),
            opening: Some(new_id()),
            revision: 4,
        };
        assert!(
            role.runtimes()
                .start_original(&reuse, &fences, None)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            role.ledger().get(spec.runtime_id).unwrap().incarnation.id(),
            Some(opening)
        );
        let stale = LifecycleTarget {
            observed: Some(original),
            opening: Some(new_id()),
            revision: 5,
            ..reuse
        };
        assert!(
            role.runtimes()
                .start_original(&stale, &fences, None)
                .await
                .unwrap()
                .is_none()
        );
        let absent = LifecycleTarget {
            pending: Vec::new(),
            subject,
            observed: None,
            opening: Some(new_id()),
            revision: 6,
        };
        assert!(
            role.runtimes()
                .start_original(&absent, &fences, None)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            role.ledger().get(spec.runtime_id).unwrap().incarnation.id(),
            Some(opening)
        );
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
        rig.stop().await;
    }
}

#[tokio::test]
async fn removing_an_original_runtime_after_its_queue_never_removes_a_later_start_or_its_data() {
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "queued-original-remove");
        rig.made(&spec).await;
        rig.driver.set_enrolled(&spec.name, Some(true));
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let role = rig.role();
        let original = role
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        assert!(matches!(
            rig.stop_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        rig.driver.hold(Operation::Inspect);
        struct Release(std::sync::Arc<MemoryDriver>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.release(Operation::Inspect);
            }
        }
        let release = Release(rig.driver.clone());
        let starting = role.runtimes().start(spec.runtime_id, None);
        tokio::pin!(starting);
        tokio::time::timeout(PATIENCE, async {
            loop {
                let before = rig.driver.targets_of(Operation::Inspect).len();
                assert!(futures::poll!(&mut starting).is_pending());
                if rig.driver.targets_of(Operation::Inspect).len() > before {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            role.ledger().get(spec.runtime_id).unwrap().incarnation.id(),
            Some(original)
        );
        let target = LifecycleTarget {
            pending: Vec::new(),
            subject: LifecycleSubject::Runtime {
                runtime_id: spec.runtime_id,
            },
            observed: Some(original),
            opening: None,
            revision: 1,
        };
        let fences = NativeFences::new(rig.data.path().join("native-original-remove"));
        let removing = role.runtimes().remove_original(&target, &fences);
        tokio::pin!(removing);
        assert!(futures::poll!(&mut removing).is_pending());
        drop(release);
        starting.await.unwrap();
        let current = role.ledger().get(spec.runtime_id).unwrap().incarnation;
        assert_ne!(current.id(), Some(original));
        assert!(!removing.await.unwrap());
        assert!(rig.driver.targets_of(Operation::Remove).is_empty());
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
        assert_eq!(
            role.ledger().get(spec.runtime_id).unwrap().incarnation,
            current
        );
        for volume in spec.volumes.values() {
            assert!(rig.driver.volume(&volume.name).is_some());
        }
        let current_target = LifecycleTarget {
            observed: current.id(),
            revision: 2,
            ..target.clone()
        };
        assert!(
            role.runtimes()
                .remove_original(&current_target, &fences)
                .await
                .unwrap()
        );
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Absent);
        assert!(role.ledger().get(spec.runtime_id).is_none());
        for volume in spec.volumes.values() {
            assert!(rig.driver.volume(&volume.name).is_some());
        }
        rig.stop().await;
    }
}

#[tokio::test]
async fn an_absent_runtime_removal_never_adopts_a_later_unit_and_lost_original_identity_keeps_the_unit_unconfirmed()
 {
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "absent-original-remove");
    let target = LifecycleTarget {
        pending: Vec::new(),
        subject: LifecycleSubject::Runtime {
            runtime_id: spec.runtime_id,
        },
        observed: None,
        opening: None,
        revision: 1,
    };
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(true));
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    let role = rig.role();
    let fences = NativeFences::new(rig.data.path().join("native-absent-remove"));
    assert!(
        !role
            .runtimes()
            .remove_original(&target, &fences)
            .await
            .unwrap()
    );
    let original = role
        .ledger()
        .get(spec.runtime_id)
        .unwrap()
        .incarnation
        .id()
        .unwrap();
    role.ledger().remove(spec.runtime_id).unwrap();
    let unknown = LifecycleTarget {
        observed: Some(original),
        revision: 2,
        ..target
    };
    assert!(
        role.runtimes()
            .remove_original(&unknown, &fences)
            .await
            .is_err()
    );
    assert!(rig.driver.targets_of(Operation::Remove).is_empty());
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    rig.stop().await;
}

#[tokio::test]
async fn a_stop_for_an_original_runtime_cannot_stop_its_later_start_after_waiting_for_the_native_lock()
 {
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "queued-original-stop");
        rig.made(&spec).await;
        rig.driver.set_enrolled(&spec.name, Some(true));
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let role = rig.role();
        let original = role
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        assert!(matches!(
            rig.stop_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let stopped = rig.driver.targets_of(Operation::Stop).len();
        rig.driver.hold(Operation::Inspect);
        struct Release(std::sync::Arc<MemoryDriver>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.release(Operation::Inspect);
            }
        }
        let release = Release(rig.driver.clone());
        let starting = role.runtimes().start(spec.runtime_id, None);
        tokio::pin!(starting);
        tokio::time::timeout(PATIENCE, async {
            loop {
                let before = rig.driver.targets_of(Operation::Inspect).len();
                assert!(futures::poll!(&mut starting).is_pending());
                if rig.driver.targets_of(Operation::Inspect).len() > before {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the newer start must hold its actual runtime lock inside the driver inspection");
        assert_eq!(
            role.ledger().get(spec.runtime_id).unwrap().incarnation.id(),
            Some(original)
        );
        let target = LifecycleTarget {
            pending: Vec::new(),
            subject: LifecycleSubject::Runtime {
                runtime_id: spec.runtime_id,
            },
            observed: Some(original),
            opening: None,
            revision: 1,
        };
        let fences = NativeFences::new(rig.data.path().join("native-original-stop"));
        let ending = role.runtimes().stop_original(&target, &fences, None, None);
        tokio::pin!(ending);
        assert!(futures::poll!(&mut ending).is_pending());
        drop(release);
        starting.await.unwrap();
        let current = role.ledger().get(spec.runtime_id).unwrap().incarnation;
        assert_ne!(current.id(), Some(original));
        assert!(ending.await.unwrap().is_none());
        assert_eq!(rig.driver.targets_of(Operation::Stop).len(), stopped);
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
        assert_eq!(
            role.ledger().get(spec.runtime_id).unwrap().incarnation,
            current
        );
        rig.stop().await;
    }
}

#[tokio::test]
async fn original_authority_expiring_during_native_inspection_keeps_the_original_unit_untouched() {
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget, NativeTarget};
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        for stopping in [false, true] {
            let mut rig = Rig::new();
            rig.have_connector();
            rig.start().await;
            let spec = rig.spec(kind, "authority-during-native-inspect");
            rig.made(&spec).await;
            rig.driver.set_enrolled(&spec.name, Some(true));
            if stopping {
                assert!(matches!(
                    rig.start_runtime(spec.runtime_id).await,
                    Progress::Succeeded(_)
                ));
            }
            let original = rig
                .role()
                .ledger()
                .get(spec.runtime_id)
                .unwrap()
                .incarnation
                .id()
                .unwrap();
            let action = if stopping {
                Operation::Stop
            } else {
                Operation::Start
            };
            let before = rig.driver.targets_of(action).len();
            let before_inspect = rig.driver.targets_of(Operation::Inspect).len();
            rig.driver.hold(Operation::Inspect);
            struct Release(std::sync::Arc<MemoryDriver>);
            impl Drop for Release {
                fn drop(&mut self) {
                    self.0.release(Operation::Inspect);
                }
            }
            let release = Release(rig.driver.clone());
            let id = new_id();
            let key = SerialKey::Runtime(spec.runtime_id);
            let mut command = if stopping {
                orch_link_contract::commands::Cmd::new(
                    id,
                    &key,
                    rig.deadline(),
                    &NodeRuntimeStop {
                        runtime_id: spec.runtime_id,
                        reason: None,
                    },
                )
            } else {
                orch_link_contract::commands::Cmd::new(
                    id,
                    &key,
                    rig.deadline(),
                    &NodeRuntimeStart {
                        runtime_id: spec.runtime_id,
                    },
                )
            }
            .unwrap();
            command.native_target = Some(NativeTarget::Lifecycle(LifecycleTarget {
                pending: Vec::new(),
                subject: LifecycleSubject::Runtime {
                    runtime_id: spec.runtime_id,
                },
                observed: Some(original),
                opening: (!stopping).then(new_id),
                revision: 1,
            }));
            command.authority_until = Some(orch_types::Time::new(
                rig.clock.now() + chrono::Duration::seconds(10),
            ));
            let mut result = rig.controller.link().commands().issue(command);
            rig.until(
                "the original native command to wait inside driver inspection",
                || rig.driver.targets_of(Operation::Inspect).len() > before_inspect,
            )
            .await;
            rig.manual.advance(chrono::Duration::seconds(11));
            drop(release);
            let ended = tokio::time::timeout(PATIENCE, result.wait()).await.unwrap();
            assert!(
                matches!(ended, Progress::Failed(ref error) if error.known() == Some(orch_link_contract::commands::ErrorCode::Timeout)),
                "{ended:?}"
            );
            assert_eq!(rig.driver.targets_of(action).len(), before);
            assert_eq!(
                rig.driver.state_of(&spec.name),
                if stopping {
                    UnitStateKind::Running
                } else {
                    UnitStateKind::Stopped
                }
            );
            assert_eq!(
                rig.role()
                    .ledger()
                    .get(spec.runtime_id)
                    .unwrap()
                    .incarnation
                    .id(),
                Some(original)
            );
            rig.stop().await;
        }
    }
}

#[tokio::test]
async fn a_newer_stop_during_start_preparation_keeps_the_old_candidate_from_starting() {
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::native_target::{LifecycleAction, LifecycleSubject, LifecycleTarget};
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "superseded-preparing-start");
        rig.made(&spec).await;
        rig.driver.set_enrolled(&spec.name, Some(true));
        let role = rig.role();
        let original = role
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        let target = LifecycleTarget {
            pending: Vec::new(),
            subject: LifecycleSubject::Runtime {
                runtime_id: spec.runtime_id,
            },
            observed: Some(original),
            opening: Some(new_id()),
            revision: 1,
        };
        let fences = NativeFences::new(rig.data.path().join("preparing-original-start"));
        rig.driver.hold(Operation::Inspect);
        struct Release(std::sync::Arc<MemoryDriver>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.release(Operation::Inspect);
            }
        }
        let release = Release(rig.driver.clone());
        let starting = role.runtimes().start_original(&target, &fences, None);
        tokio::pin!(starting);
        tokio::time::timeout(PATIENCE, async {
            loop {
                let before = rig.driver.targets_of(Operation::Inspect).len();
                assert!(futures::poll!(&mut starting).is_pending());
                if rig.driver.targets_of(Operation::Inspect).len() > before {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            fences
                .observe_lifecycle(
                    &LifecycleTarget {
                        opening: None,
                        revision: 2,
                        ..target.clone()
                    },
                    LifecycleAction::Stop
                )
                .unwrap()
        );
        drop(release);
        assert!(
            matches!(starting.await, Err(error) if error.known() == Some(orch_link_contract::commands::ErrorCode::Conflict))
        );
        assert!(rig.driver.targets_of(Operation::Start).is_empty());
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
        assert_eq!(
            role.ledger().get(spec.runtime_id).unwrap().incarnation.id(),
            Some(original)
        );
        rig.stop().await;
    }
}

#[tokio::test]
async fn a_newer_control_during_native_inspection_prevents_an_old_stop_of_the_same_start() {
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::native_target::{LifecycleAction, LifecycleSubject, LifecycleTarget};
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "superseded-inspecting-stop");
        rig.made(&spec).await;
        rig.driver.set_enrolled(&spec.name, Some(true));
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let role = rig.role();
        let original = role
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        let target = LifecycleTarget {
            pending: Vec::new(),
            subject: LifecycleSubject::Runtime {
                runtime_id: spec.runtime_id,
            },
            observed: Some(original),
            opening: None,
            revision: 1,
        };
        let fences = NativeFences::new(rig.data.path().join("inspecting-original-stop"));
        let stopped = rig.driver.targets_of(Operation::Stop).len();
        rig.driver.hold(Operation::Inspect);
        struct Release(std::sync::Arc<MemoryDriver>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.release(Operation::Inspect);
            }
        }
        let release = Release(rig.driver.clone());
        let stopping = role.runtimes().stop_original(&target, &fences, None, None);
        tokio::pin!(stopping);
        tokio::time::timeout(PATIENCE, async {
            loop {
                let before = rig.driver.targets_of(Operation::Inspect).len();
                assert!(futures::poll!(&mut stopping).is_pending());
                if rig.driver.targets_of(Operation::Inspect).len() > before {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            fences
                .observe_lifecycle(
                    &LifecycleTarget {
                        revision: 2,
                        ..target.clone()
                    },
                    LifecycleAction::Stop
                )
                .unwrap()
        );
        drop(release);
        assert!(
            matches!(stopping.await, Err(error) if error.known() == Some(orch_link_contract::commands::ErrorCode::Conflict))
        );
        assert_eq!(rig.driver.targets_of(Operation::Stop).len(), stopped);
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
        assert_eq!(
            role.ledger().get(spec.runtime_id).unwrap().incarnation.id(),
            Some(original)
        );
        rig.stop().await;
    }
}

#[tokio::test]
async fn a_runtime_stop_admitted_as_absent_never_selects_a_later_unit_and_a_stale_order_cannot_stop_an_unchanged_start()
 {
    use orch_link::native_fences::NativeFences;
    use orch_link_contract::native_target::{LifecycleAction, LifecycleSubject, LifecycleTarget};
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "absent-original-stop");
    let subject = LifecycleSubject::Runtime {
        runtime_id: spec.runtime_id,
    };
    let absent = LifecycleTarget {
        pending: Vec::new(),
        subject: subject.clone(),
        observed: None,
        opening: None,
        revision: 1,
    };
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(true));
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    let role = rig.role();
    let fences = NativeFences::new(rig.data.path().join("native-absent-stop"));
    assert!(
        role.runtimes()
            .stop_original(&absent, &fences, None, None)
            .await
            .unwrap()
            .is_none()
    );
    let original = role
        .ledger()
        .get(spec.runtime_id)
        .unwrap()
        .incarnation
        .id()
        .unwrap();
    let newer = LifecycleTarget {
        pending: Vec::new(),
        subject,
        observed: Some(original),
        opening: Some(new_id()),
        revision: 3,
    };
    assert!(
        fences
            .observe_lifecycle(&newer, LifecycleAction::Start)
            .unwrap()
    );
    let stale = LifecycleTarget {
        opening: None,
        revision: 2,
        ..newer
    };
    assert!(
        role.runtimes()
            .stop_original(&stale, &fences, None, None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(rig.driver.targets_of(Operation::Stop).is_empty());
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    rig.stop().await;
}

#[tokio::test]
async fn a_new_native_start_gets_its_own_persistent_identity_while_a_running_unit_keeps_its_original_one()
 {
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "native-identity");
        rig.made(&spec).await;
        rig.driver.set_enrolled(&spec.name, Some(true));
        let made = rig.role().ledger().get(spec.runtime_id).unwrap();
        assert!(made.incarnation.id().is_some());
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let first = rig.role().ledger().get(spec.runtime_id).unwrap();
        assert_ne!(first.incarnation, made.incarnation);
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        assert_eq!(
            rig.role()
                .ledger()
                .get(spec.runtime_id)
                .unwrap()
                .incarnation,
            first.incarnation
        );
        assert!(matches!(
            rig.stop_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        assert_eq!(
            rig.role()
                .ledger()
                .get(spec.runtime_id)
                .unwrap()
                .incarnation,
            first.incarnation
        );
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let next = rig.role().ledger().get(spec.runtime_id).unwrap();
        assert_ne!(next.incarnation, first.incarnation);
        assert_eq!(
            next.started_at, first.started_at,
            "the unchanged test clock is not the physical identity"
        );
        rig.stop().await;
        rig.start().await;
        assert_eq!(
            rig.role()
                .ledger()
                .get(spec.runtime_id)
                .unwrap()
                .incarnation,
            next.incarnation
        );
        rig.stop().await;
    }
}

#[tokio::test]
async fn the_original_native_start_identity_is_saved_before_a_held_driver_start_can_finish() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "held-native-identity");
    rig.made(&spec).await;
    let before = rig
        .role()
        .ledger()
        .get(spec.runtime_id)
        .unwrap()
        .incarnation;
    rig.driver.hold(Operation::Start);
    struct Release(std::sync::Arc<MemoryDriver>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release(Operation::Start);
        }
    }
    let release = Release(rig.driver.clone());
    let command = new_id();
    rig.controller.issue_with_id(
        command,
        &SerialKey::Runtime(spec.runtime_id),
        rig.deadline(),
        &NodeRuntimeStart {
            runtime_id: spec.runtime_id,
        },
    );
    rig.wait_call(Operation::Start, &spec.name).await;
    let accepted = rig
        .role()
        .ledger()
        .get(spec.runtime_id)
        .unwrap()
        .incarnation;
    assert_ne!(accepted, before);
    drop(release);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(command))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    assert_eq!(
        rig.role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation,
        accepted
    );
    rig.stop().await;
}

#[tokio::test]
async fn an_unavailable_driver_reports_its_schema_without_a_schedulable_descriptor() {
    use orch_node_core::drivers::{DriverHost, Factory, Registrations};
    use std::sync::Arc;

    let mut rig = Rig::new();
    rig.settings.drivers = vec!["cluster".into()];
    let schema = json!({"type":"object", "required":["namespace"]});
    let factory: Factory = Arc::new(|_| {
        Box::pin(async { Err(DriverError::Unavailable("the cluster is offline".into())) })
    });
    let host = DriverHost::load(
        &rig.settings,
        Registrations::new().register_with_schema("cluster", factory, schema.clone()),
        Arc::clone(&rig.desk),
    )
    .await;
    rig.start_with(host).await;
    rig.wait_told("the registered schema", |events| {
        events.iter().any(|event| {
            event.kind == "node.facts"
                && event.body["driver_configuration_schemas"]["cluster"] == schema
        })
    })
    .await;
    let report = rig
        .applied()
        .into_iter()
        .find(|event| event.kind == "node.facts")
        .unwrap()
        .body;
    assert_eq!(report["drivers"], json!({}));
    assert_eq!(report["driver_configuration_schemas"]["cluster"], schema);
    let mut old = report;
    old.as_object_mut()
        .unwrap()
        .remove("driver_configuration_schemas");
    let decoded: orch_link_contract::events::NodeFacts =
        serde_json::from_value(old.clone()).unwrap();
    assert!(decoded.driver_configuration_schemas.is_empty());
    assert_eq!(serde_json::to_value(decoded).unwrap(), old);
    rig.stop().await;
}

#[tokio::test]
async fn a_failed_restriction_keeps_an_open_unit_stopped_across_restart_until_an_explicit_open_plan()
 {
    use orch_api_contract::environment::EgressMode;
    use orch_link_contract::commands::NodeNetworkApply;
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "kept-stopped");
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(true));
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    let restriction = NodeNetworkApply {
        runtime_id: spec.runtime_id,
        required_mode: EgressMode::Restricted,
        allowed_hosts: vec!["pypi.org".to_owned()],
        always: Vec::new(),
        always_revision: String::new(),
        revision: "restrict-1".to_owned(),
    };
    let reply = rig
        .succeed(&SerialKey::Runtime(spec.runtime_id), &restriction)
        .await;
    assert_eq!(reply["required_mode"], "restricted");
    assert_eq!(reply["state"], "unavailable");
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    rig.stop().await;
    rig.start().await;
    let error = match rig.start_runtime(spec.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("the guarded old unit restarted: {other:?}"),
    };
    assert_eq!(error.code, "network.restricted_unavailable");
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    assert_eq!(
        rig.driver.volumes().len(),
        3,
        "the network guard removes no files"
    );
    let open = NodeNetworkApply {
        required_mode: EgressMode::Open,
        revision: "explicit-open".to_owned(),
        ..restriction
    };
    rig.succeed(&SerialKey::Runtime(spec.runtime_id), &open)
        .await;
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_is_made_started_stopped_and_removed_and_the_system_is_told_each_state() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "one");

    let unit = rig.made(&spec).await;
    assert_eq!(unit.name, spec.name);
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);

    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);

    assert!(matches!(
        rig.stop_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    assert_eq!(
        rig.driver.unit(&spec.name).unwrap().last_grace,
        Some(rig.settings.stop_grace),
        "a unit is given the node's grace to stop by itself"
    );

    assert!(matches!(
        rig.remove_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Absent);
    assert_eq!(
        rig.driver.volumes().len(),
        3,
        "a unit goes and its volumes stay"
    );

    rig.wait_state(spec.runtime_id, "absent").await;
    let told: Vec<String> = rig
        .states_of(spec.runtime_id)
        .into_iter()
        .map(|(state, _)| state)
        .collect();
    assert_eq!(told, ["stopped", "running", "stopped", "absent"]);
    rig.stop().await;
}

#[tokio::test]
async fn the_image_is_made_available_and_the_spec_checked_before_the_unit_is_made() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "ordered");
    rig.made(&spec).await;
    let order: Vec<Operation> = rig
        .driver
        .calls()
        .into_iter()
        .map(|call| call.operation)
        .filter(|operation| {
            matches!(
                operation,
                Operation::Validate | Operation::EnsureImage | Operation::Create
            )
        })
        .collect();
    assert_eq!(
        order,
        [
            Operation::Validate,
            Operation::EnsureImage,
            Operation::Create
        ]
    );
    rig.stop().await;
}

#[tokio::test]
async fn asking_again_for_a_unit_that_is_made_answers_the_unit_that_is_there() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "twice");
    let first = rig.made(&spec).await;
    let second = rig.made(&spec).await;
    assert_eq!(first, second);
    assert_eq!(rig.driver.units().len(), 1, "created by name, once");
    rig.stop().await;
}

#[tokio::test]
async fn a_provider_runtime_is_made_the_same_way_with_its_state_volume_alone() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Provider, "prov");
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    assert_eq!(rig.driver.volumes().len(), 1);
    rig.stop().await;
}

#[tokio::test]
async fn a_request_that_the_connector_inside_the_unit_could_not_use_is_refused_before_the_driver_is_asked()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;

    // A token of another authority than the certificate given.
    let strange = Rig::new();
    let mut spec = rig.spec(RuntimeKind::Environment, "strange");
    spec.enroll_token = strange.token().into();
    let error = match rig.create(&spec).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "invalid");
    assert!(
        error.detail.contains("another authority"),
        "{}",
        error.detail
    );

    // A plaintext token is refused by the link's typed body before dispatch.
    let spec = rig.spec(RuntimeKind::Environment, "plain");
    let mut body = serde_json::to_value(&spec).unwrap();
    body["enroll_token"] = json!(spec.enroll_token.expose());
    let error = serde_json::from_value::<orch_link_contract::commands::NodeRuntimeCreate>(body)
        .expect_err("the wire contract must refuse a plaintext enrollment token");
    assert!(
        error.to_string().contains("$sealed"),
        "the token must use the link's secret representation"
    );

    // A driver this host does not have.
    let mut elsewhere = rig.spec(RuntimeKind::Environment, "elsewhere");
    elsewhere.driver = "host".into();
    let error = match rig.create(&elsewhere).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "unsupported");

    assert!(
        rig.driver.targets_of(Operation::Create).is_empty(),
        "the driver was never asked to make any of them"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_host_without_the_connector_in_its_cache_makes_no_unit() {
    let mut rig = Rig::new();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "nothing-to-run");
    let error = match rig.create(&spec).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "not_found");
    assert!(
        error.detail.contains("node.tool.ensure"),
        "{}",
        error.detail
    );
    rig.stop().await;
}

#[tokio::test]
async fn an_enrollment_token_is_single_use_and_a_second_unit_is_not_given_the_first_ones() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let first = rig.spec(RuntimeKind::Environment, "first");
    rig.made(&first).await;
    let mut second = rig.spec(RuntimeKind::Environment, "second");
    second.enroll_token = first.enroll_token.clone();
    let error = match rig.create(&second).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert!(error.detail.contains("single-use"), "{}", error.detail);
    assert_eq!(rig.driver.units().len(), 1);
    // The same runtime asked for again, with the same token, is not another unit.
    rig.made(&first).await;
    rig.stop().await;
}

#[tokio::test]
async fn a_successor_is_made_on_the_same_volumes_while_the_old_unit_is_kept_and_two_units_never_run_on_one_volume()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let old = rig.spec(RuntimeKind::Environment, "old");
    let mut new = rig.spec(RuntimeKind::Environment, "new");
    new.volumes = old.volumes.clone();
    rig.made(&old).await;
    rig.start_runtime(old.runtime_id).await;

    // The successor is made while the old unit runs; it may not start until the old one has stopped.
    rig.made(&new).await;
    let error = match rig.start_runtime(new.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert!(error.detail.contains(&old.name), "{}", error.detail);
    assert_eq!(rig.driver.state_of(&new.name), UnitStateKind::Stopped);

    rig.stop_runtime(old.runtime_id).await;
    assert!(matches!(
        rig.start_runtime(new.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&new.name), UnitStateKind::Running);
    assert_eq!(
        rig.driver.state_of(&old.name),
        UnitStateKind::Stopped,
        "the old unit is kept until it is told to go"
    );

    // And back: the old unit is started again as it was only when the successor is out of the way.
    let error = match rig.start_runtime(old.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "conflict");
    rig.remove_runtime(new.runtime_id).await;
    assert!(matches!(
        rig.start_runtime(old.runtime_id).await,
        Progress::Succeeded(_)
    ));

    // Removing the old unit once the successor has taken over leaves the volumes where they are.
    rig.stop_runtime(old.runtime_id).await;
    rig.remove_runtime(old.runtime_id).await;
    assert_eq!(rig.driver.volumes().len(), 3);
    rig.stop().await;
}

#[tokio::test]
async fn starts_on_independent_volumes_can_enter_the_driver_together() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let first = rig.spec(RuntimeKind::Environment, "independent-a");
    let second = rig.spec(RuntimeKind::Environment, "independent-b");
    rig.made(&first).await;
    rig.made(&second).await;
    rig.driver.clear_calls();
    rig.driver.hold(Operation::Start);
    let deadline = rig.deadline();
    let one = rig.controller.issue(
        &SerialKey::Runtime(first.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: first.runtime_id,
        },
    );
    rig.wait_call(Operation::Start, &first.name).await;
    let two = rig.controller.issue(
        &SerialKey::Runtime(second.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: second.runtime_id,
        },
    );
    rig.wait_call(Operation::Start, &second.name).await;
    assert_eq!(
        rig.driver.targets_of(Operation::Start),
        [first.name.clone(), second.name.clone()]
    );
    rig.driver.release(Operation::Start);
    for command in [one, two] {
        assert!(matches!(
            tokio::time::timeout(PATIENCE, rig.controller.wait_command(command))
                .await
                .unwrap(),
            Progress::Succeeded(_)
        ));
    }
    rig.stop().await;
}

#[tokio::test]
async fn concurrent_starts_on_the_same_volumes_do_not_both_enter_the_driver() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let first = rig.spec(RuntimeKind::Environment, "first-shared");
    let mut second = rig.spec(RuntimeKind::Environment, "second-shared");
    second.volumes = first.volumes.clone();
    let independent = rig.spec(RuntimeKind::Environment, "independent");
    rig.made(&first).await;
    rig.made(&second).await;
    rig.made(&independent).await;
    rig.start_runtime(independent.runtime_id).await;
    rig.driver.clear_calls();
    rig.driver.hold(Operation::Start);
    let deadline = rig.deadline();
    let one = rig.controller.issue(
        &SerialKey::Runtime(first.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: first.runtime_id,
        },
    );
    rig.wait_call(Operation::Start, &first.name).await;
    let two = rig.controller.issue(
        &SerialKey::Runtime(second.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: second.runtime_id,
        },
    );
    let other = rig.controller.issue(
        &SerialKey::Runtime(independent.runtime_id),
        deadline,
        &NodeRuntimeStop {
            runtime_id: independent.runtime_id,
            reason: None,
        },
    );
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(other))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        rig.driver.targets_of(Operation::Start).as_slice(),
        std::slice::from_ref(&first.name)
    );
    rig.driver.release(Operation::Start);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(one))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    let error = match tokio::time::timeout(PATIENCE, rig.controller.wait_command(two))
        .await
        .unwrap()
    {
        Progress::Failed(error) => error,
        other => panic!("the second unit started on the same volumes: {other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert_eq!(rig.driver.state_of(&first.name), UnitStateKind::Running);
    assert_eq!(rig.driver.state_of(&second.name), UnitStateKind::Stopped);
    rig.stop().await;
}

#[tokio::test]
async fn a_successor_waits_when_the_predecessors_driver_is_unavailable() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let old = rig.spec(RuntimeKind::Environment, "old-driver");
    let mut new = rig.spec(RuntimeKind::Environment, "new-driver");
    new.volumes = old.volumes.clone();
    rig.made(&old).await;
    rig.made(&new).await;
    rig.role()
        .ledger()
        .update(old.runtime_id, |record| {
            record.driver = "unavailable-driver".into()
        })
        .unwrap();
    let error = match rig.start_runtime(new.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("the successor started without checking its predecessor: {other:?}"),
    };
    assert_eq!(error.code, "unavailable");
    assert!(error.retryable);
    assert_eq!(rig.driver.state_of(&new.name), UnitStateKind::Stopped);
    let name = old.volumes[&VolumeRole::Workspace].name.clone();
    let removal = NodeVolumeRemove {
        driver: "memory".into(),
        name: name.clone(),
        storage: None,
        labels: Default::default(),
    };
    let error = rig
        .refused(&SerialKey::Volume(name.clone()), &removal)
        .await;
    assert_eq!(error.code, "unavailable");
    assert!(error.retryable);
    assert!(rig.driver.volume(&name).is_some());
    let invalid = NodeVolumeRemove {
        driver: String::new(),
        name: name.clone(),
        storage: None,
        labels: Default::default(),
    };
    let error = rig.refused(&SerialKey::Volume(name), &invalid).await;
    assert_eq!(
        error.code, "unsupported",
        "an empty driver cannot collide with the name fence"
    );
    rig.stop().await;
}

#[tokio::test]
async fn removal_waits_for_an_in_flight_start_and_cannot_remove_the_running_volume() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "starting-volume");
    rig.made(&spec).await;
    let name = spec.volumes[&VolumeRole::Workspace].name.clone();
    rig.driver.clear_calls();
    rig.driver.hold(Operation::Start);
    let deadline = rig.deadline();
    let start = rig.controller.issue(
        &SerialKey::Runtime(spec.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: spec.runtime_id,
        },
    );
    rig.wait_call(Operation::Start, &spec.name).await;
    let removal = rig.controller.issue(
        &SerialKey::Volume(name.clone()),
        deadline,
        &NodeVolumeRemove {
            driver: spec.driver.clone(),
            name: name.clone(),
            storage: None,
            labels: Default::default(),
        },
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(rig.driver.targets_of(Operation::RemoveVolume).is_empty());
    rig.driver.release(Operation::Start);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(start))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    let error = match tokio::time::timeout(PATIENCE, rig.controller.wait_command(removal))
        .await
        .unwrap()
    {
        Progress::Failed(error) => error,
        other => panic!("the running volume was removed: {other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert!(rig.driver.volume(&name).is_some());
    rig.stop().await;
}

#[tokio::test]
async fn a_capability_restart_fences_a_successor_on_the_same_volumes() {
    let driver = MemoryDriver::new("memory").replacing();
    let mut descriptor = driver.describe();
    descriptor
        .capabilities
        .insert("debugger".into(), json!({"type":"object"}));
    descriptor.live_changes.capabilities.push("debugger".into());
    let mut rig = Rig::with_driver(driver.with_descriptor(descriptor));
    rig.have_connector();
    rig.start().await;
    let mut old = rig.spec(RuntimeKind::Environment, "restarting");
    old.capabilities = serde_json::from_value(
        json!({"allowed":["debugger"],"base":[],"granted":[],"patches":{"debugger":{}}}),
    )
    .unwrap();
    let mut new = rig.spec(RuntimeKind::Environment, "waiting-successor");
    new.volumes = old.volumes.clone();
    rig.made(&old).await;
    rig.made(&new).await;
    rig.start_runtime(old.runtime_id).await;
    rig.driver.clear_calls();
    rig.driver.hold(Operation::Start);
    let deadline = rig.deadline();
    let change = rig.controller.issue(&SerialKey::Runtime(old.runtime_id), deadline,
        &NodeCapabilitiesApply {runtime_id:old.runtime_id, capabilities:json!({"allowed":["debugger"],"base":[],"granted":["debugger"],"patches":{"debugger":{}}})});
    rig.wait_call(Operation::Start, &old.name).await;
    let successor = rig.controller.issue(
        &SerialKey::Runtime(new.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: new.runtime_id,
        },
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.driver.targets_of(Operation::Start), [old.name.clone()]);
    rig.driver.release(Operation::Start);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(change))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    let error = match tokio::time::timeout(PATIENCE, rig.controller.wait_command(successor))
        .await
        .unwrap()
    {
        Progress::Failed(error) => error,
        other => panic!("the successor started during a capability restart: {other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert_eq!(rig.driver.state_of(&old.name), UnitStateKind::Running);
    assert_eq!(rig.driver.state_of(&new.name), UnitStateKind::Stopped);
    rig.stop().await;
}

#[tokio::test]
async fn capability_reconciliation_does_not_replay_over_a_running_successor() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let old = rig.spec(RuntimeKind::Environment, "replay-old");
    let mut new = rig.spec(RuntimeKind::Environment, "replay-successor");
    new.volumes = old.volumes.clone();
    rig.made(&old).await;
    let successor = rig.made(&new).await;
    assert!(matches!(
        rig.start_runtime(old.runtime_id).await,
        Progress::Succeeded(_)
    ));
    // Model a native start outside node commands. Reconciliation must not compound
    // that existing conflict by replaying a second start on the old unit.
    rig.driver.start(&successor).await.unwrap();
    let ledger = rig.role();
    let mut capabilities = ledger.ledger().capabilities(old.runtime_id).unwrap();
    capabilities.applied_started_at = Some(rig.clock.time());
    ledger
        .ledger()
        .set_capabilities(old.runtime_id, capabilities)
        .unwrap();
    rig.driver.clear_calls();
    ledger.runtimes().reconcile_live_capabilities().await;
    assert!(rig.driver.targets_of(Operation::Start).is_empty());
    assert_eq!(rig.driver.state_of(&new.name), UnitStateKind::Running);
    rig.stop().await;
}

#[tokio::test]
async fn commands_for_one_runtime_run_one_at_a_time_and_for_two_runtimes_side_by_side() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let (a, b) = (
        rig.spec(RuntimeKind::Environment, "a"),
        rig.spec(RuntimeKind::Environment, "b"),
    );
    rig.made(&a).await;
    rig.made(&b).await;
    rig.start_runtime(b.runtime_id).await;

    rig.driver.hold(Operation::Start);
    let deadline = rig.deadline();
    let first = rig.controller.issue(
        &SerialKey::Runtime(a.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: a.runtime_id,
        },
    );
    // The start of `a` is inside the driver, held there.
    rig.wait_call(Operation::Start, &a.name).await;
    let second = rig.controller.issue(
        &SerialKey::Runtime(a.runtime_id),
        deadline,
        &NodeRuntimeStop {
            runtime_id: a.runtime_id,
            reason: None,
        },
    );
    let other = rig.controller.issue(
        &SerialKey::Runtime(b.runtime_id),
        deadline,
        &NodeRuntimeStop {
            runtime_id: b.runtime_id,
            reason: None,
        },
    );
    // The other runtime's stop is done while `a` is held; the stop of `a` waits behind its start.
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(other))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        rig.driver.targets_of(Operation::Stop),
        std::slice::from_ref(&b.name)
    );
    assert!(!rig.controller.progress(first).unwrap().is_final());

    rig.driver.release(Operation::Start);
    for id in [first, second] {
        assert!(matches!(
            tokio::time::timeout(PATIENCE, rig.controller.wait_command(id))
                .await
                .unwrap(),
            Progress::Succeeded(_)
        ));
    }
    assert_eq!(
        rig.driver.targets_of(Operation::Stop),
        [b.name.clone(), a.name.clone()]
    );
    assert_eq!(
        rig.driver.max_in_flight(&a.name),
        1,
        "never two calls for one unit at once"
    );
    assert_eq!(rig.driver.state_of(&a.name), UnitStateKind::Stopped);
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_the_host_has_no_room_for_fails_as_no_capacity_and_says_what_the_units_there_declare()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let (resident, tenant) = (
        rig.spec(RuntimeKind::Environment, "resident"),
        rig.spec(RuntimeKind::Environment, "tenant"),
    );
    rig.made(&resident).await;
    rig.start_runtime(resident.runtime_id).await;

    rig.driver.fail_next(
        Operation::Create,
        DriverError::NoCapacity("cannot allocate memory".into()),
    );
    let error = match rig.create(&tenant).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "no_capacity");
    assert!(
        error.detail.contains("cannot allocate memory"),
        "{}",
        error.detail
    );
    assert!(
        error.detail.contains("declare 512 MiB of memory"),
        "{}",
        error.detail
    );
    assert!(
        rig.driver.unit(&tenant.name).is_none(),
        "nothing was left half made"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_runtime_the_node_never_heard_of_cannot_be_started_or_stopped_and_is_already_removed() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let nobody = new_id();
    for progress in [
        rig.start_runtime(nobody).await,
        rig.stop_runtime(nobody).await,
    ] {
        let Progress::Failed(error) = progress else {
            panic!("{progress:?}");
        };
        assert_eq!(error.code, "not_found");
    }
    assert!(matches!(
        rig.remove_runtime(nobody).await,
        Progress::Succeeded(_)
    ));
    rig.stop().await;
}

#[tokio::test]
async fn stopping_a_known_unit_that_has_already_gone_succeeds_and_keeps_its_volume_records() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "missing-workspace");
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    let unit = rig.driver.unit(&spec.name).unwrap().unit;
    EnvironmentDriver::remove(rig.driver.as_ref(), &unit)
        .await
        .unwrap();
    assert!(matches!(
        rig.stop_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    rig.wait_state(spec.runtime_id, "absent").await;
    assert!(rig.role().ledger().get(spec.runtime_id).is_some());
    assert!(rig.driver.targets_of(Operation::RemoveVolume).is_empty());
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_the_node_has_no_record_of_is_found_by_the_label_of_its_system_and_removed_without_its_volumes()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    // A unit from before this node had a ledger, or from a ledger that was lost.
    let stray = rig.spec(RuntimeKind::Environment, "stray");
    for volume in stray.volumes.values() {
        orch_driver_contract::driver::EnvironmentDriver::ensure_volume(
            rig.driver.as_ref(),
            &orch_driver_contract::volumes::VolumeRequest {
                name: volume.name.clone(),
                storage: None,
                labels: std::collections::BTreeMap::new(),
            },
        )
        .await
        .unwrap();
    }
    rig.driver.plant(stray.clone());
    assert!(matches!(
        rig.remove_runtime(stray.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(rig.driver.unit(&stray.name).is_none());
    assert_eq!(
        rig.driver.volumes().len(),
        3,
        "its volumes were not touched"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_node_that_is_started_again_still_knows_the_units_it_made() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "durable");
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    rig.stop().await;

    rig.start().await;
    assert!(matches!(
        rig.stop_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    let recorded = Ledger::open(&rig.settings.data_dir).unwrap();
    let record = recorded.get(spec.runtime_id).unwrap();
    assert_eq!(record.unit.name, spec.name);
    assert!(record.token.is_some() && record.started_at.is_some());
    assert_eq!(
        recorded.systems(),
        [SYSTEM],
        "the system the units belong to is remembered"
    );
    rig.stop().await;
}

// ---------------------------------------------------------------------------------------------- a connector that never enrolls

#[tokio::test]
async fn a_unit_that_ends_before_its_connector_enrolled_is_reported_failed_and_removed_and_its_volumes_stay()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "dies");
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(false));
    rig.start_runtime(spec.runtime_id).await;
    rig.driver.crash(
        &spec.name,
        1,
        "enrolling this runtime: the gateway cannot be reached",
    );

    rig.wait_state(spec.runtime_id, "failed").await;
    let (state, detail) = rig.states_of(spec.runtime_id).pop().unwrap();
    assert_eq!(state, "failed");
    let detail = detail.unwrap();
    assert!(detail.contains("before its connector enrolled"), "{detail}");
    assert!(detail.contains("the gateway cannot be reached"), "{detail}");
    assert!(detail.contains("exit status 1"), "{detail}");
    assert!(
        rig.driver.unit(&spec.name).is_none(),
        "the unit is cleaned up"
    );
    assert_eq!(rig.driver.volumes().len(), 3, "and its volumes are not");
    assert!(
        Ledger::open(&rig.settings.data_dir)
            .unwrap()
            .get(spec.runtime_id)
            .is_none()
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_whose_connector_does_not_enroll_in_time_is_stopped_removed_and_reported_failed() {
    let mut rig = Rig::new();
    rig.settings.enroll_timeout = Duration::from_secs(1);
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "silent");
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(false));
    rig.start_runtime(spec.runtime_id).await;

    rig.wait_state(spec.runtime_id, "failed").await;
    let detail = rig.states_of(spec.runtime_id).pop().unwrap().1.unwrap();
    assert!(
        detail.contains("did not enroll within 1 seconds"),
        "{detail}"
    );
    assert!(rig.driver.unit(&spec.name).is_none());
    let calls = rig.driver.targets_of(Operation::Stop);
    assert_eq!(
        calls,
        std::slice::from_ref(&spec.name),
        "it was asked to stop before it was removed"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_whose_connector_enrolled_is_not_watched_again_and_one_that_cannot_be_looked_into_is_not_judged()
 {
    let mut rig = Rig::new();
    rig.settings.enroll_timeout = Duration::from_secs(1);
    rig.have_connector();
    rig.start().await;
    let (good, unknown) = (
        rig.spec(RuntimeKind::Environment, "good"),
        rig.spec(RuntimeKind::Environment, "unknown"),
    );
    for spec in [&good, &unknown] {
        rig.made(spec).await;
    }
    rig.driver.set_enrolled(&good.name, Some(true));
    // A driver that cannot tell: nothing is known, and nothing is done.
    rig.driver.set_enrolled(&unknown.name, None);
    rig.start_runtime(good.runtime_id).await;
    rig.start_runtime(unknown.runtime_id).await;
    tokio::time::sleep(Duration::from_secs(4)).await;

    assert_eq!(rig.driver.state_of(&good.name), UnitStateKind::Running);
    assert_eq!(rig.driver.state_of(&unknown.name), UnitStateKind::Running);
    let recorded = Ledger::open(&rig.settings.data_dir).unwrap();
    assert!(recorded.get(good.runtime_id).unwrap().enrolled);
    assert!(!recorded.get(unknown.runtime_id).unwrap().enrolled);
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_whose_driver_is_not_loaded_just_now_is_not_forgotten_when_it_is_asked_to_go() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "orphaned");
    rig.made(&spec).await;
    rig.stop().await;

    // The node comes back with the daemon down: it has no driver, and a unit it made is on record.
    let nothing = orch_node_core::drivers::DriverHost::of(
        &rig.settings,
        std::sync::Arc::clone(&rig.desk),
        Vec::new(),
    );
    rig.start_with(nothing).await;
    let error = match rig.remove_runtime(spec.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "unavailable");
    assert!(
        error.retryable,
        "asking again when the driver is back may work"
    );
    assert!(error.detail.contains("memory"), "{}", error.detail);
    assert!(
        Ledger::open(&rig.settings.data_dir)
            .unwrap()
            .get(spec.runtime_id)
            .is_some()
    );
    assert!(
        rig.driver.unit(&spec.name).is_some(),
        "and nothing was done to the unit"
    );
    rig.stop().await;

    rig.start().await;
    assert!(matches!(
        rig.remove_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(rig.driver.unit(&spec.name).is_none());
    rig.stop().await;
}

#[tokio::test]
async fn two_requests_with_one_token_cannot_both_get_past_the_check_while_the_first_is_still_being_made()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let (first, mut second) = (
        rig.spec(RuntimeKind::Environment, "token-a"),
        rig.spec(RuntimeKind::Environment, "token-b"),
    );
    second.enroll_token = first.enroll_token.clone();

    // The first is held at its image, long before the unit is made and recorded.
    rig.driver.hold(Operation::EnsureImage);
    let id = new_id();
    rig.controller.issue_with_id(
        id,
        &SerialKey::Runtime(first.runtime_id),
        rig.deadline(),
        &rig.create_body(&first, id),
    );
    rig.until("the image of the first", || {
        !rig.driver.targets_of(Operation::EnsureImage).is_empty()
    })
    .await;
    let refused = match rig.create(&second).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(refused.code, "conflict");
    assert!(refused.detail.contains("single-use"), "{}", refused.detail);

    rig.driver.release(Operation::EnsureImage);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(id))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.units().len(), 1);
    rig.stop().await;
}

#[tokio::test]
async fn what_one_environment_wrote_in_a_private_view_is_a_volume_of_its_unit_that_no_second_unit_runs_on()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let layer = json!({ "name": "orch-over-shared", "storage": null });
    let source = tempfile::tempdir().unwrap();
    let mount = |name: &str| {
        json!({
            "name": name, "kind": "path", "source": source.path(), "target": "/data/src",
            "mode": "private", "layer": layer
        })
    };
    let (mut old, mut new) = (
        rig.spec(RuntimeKind::Environment, "view-old"),
        rig.spec(RuntimeKind::Environment, "view-new"),
    );
    old.mounts = vec![serde_json::from_value(mount("src")).unwrap()];
    new.mounts = vec![serde_json::from_value(mount("src")).unwrap()];
    // The two have volumes of their own and share the layer only.
    rig.made(&old).await;
    rig.made(&new).await;
    let record = Ledger::open(&rig.settings.data_dir)
        .unwrap()
        .get(old.runtime_id)
        .unwrap();
    assert!(
        record.volumes.contains(&"orch-over-shared".to_owned()),
        "{:?}",
        record.volumes
    );

    rig.start_runtime(old.runtime_id).await;
    let error = match rig.start_runtime(new.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "conflict");
    rig.stop().await;
}

#[tokio::test]
async fn a_new_unit_uses_its_nodes_verified_address_or_that_nodes_explicit_unit_override() {
    for override_address in ["", "wss://units.test/link"] {
        let mut rig = Rig::new();
        rig.settings.managed_endpoint = override_address.into();
        rig.have_connector();
        rig.start().await;
        for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
            let mut spec = rig.spec(kind, "node-address");
            spec.gateway_url = "wss://unreachable-server-wide-address.test:9443".into();
            rig.made(&spec).await;
            let actual = rig.driver.unit(&spec.name).unwrap().spec.gateway_url;
            assert_eq!(
                actual,
                if override_address.is_empty() {
                    orch_testkit::doubles::link::HUB_URL
                } else {
                    override_address
                }
            );
        }
        rig.stop().await;
    }
}

#[tokio::test]
async fn an_admitted_program_keeps_its_real_parent_and_helper_without_starting_a_unit_from_metadata()
 {
    use orch_link_contract::commands::NodeProgramAdmit;
    use orch_link_contract::program_work::Admission;
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let parent = rig.spec(RuntimeKind::Provider, "program-parent");
    rig.made(&parent).await;
    let work = Admission {
        work: new_id(),
        helper: new_id(),
        peer: new_id(),
        parent: parent.runtime_id,
        endpoints: vec![],
        deadline: rig.deadline(),
    };
    let body = NodeProgramAdmit {
        runtime_id: work.work,
        work: work.clone(),
    };
    rig.succeed(&SerialKey::Runtime(work.work), &body).await;
    rig.succeed(&SerialKey::Runtime(work.work), &body).await;
    let kept = rig.role().ledger().program(work.work).unwrap();
    assert_eq!(kept.admission, work);
    assert_eq!(kept.template, parent.spec);
    assert_eq!(kept.tools, parent.tools);
    let mut invented = serde_json::to_value(&body).unwrap();
    invented["work"]["template"] = json!({"image":"another-base"});
    assert!(
        serde_json::from_value::<NodeProgramAdmit>(invented).is_err(),
        "program admission never accepts a replacement for the parent's real original source"
    );
    assert!(kept.unit.is_none());
    assert_eq!(
        rig.driver.list(SYSTEM).await.unwrap().len(),
        1,
        "admission metadata is not an execution effect"
    );
    let mut changed = body.clone();
    changed.work.helper = new_id();
    rig.refused(&SerialKey::Runtime(work.work), &changed).await;
    assert_eq!(
        rig.role().ledger().program(work.work).unwrap().admission,
        work
    );
    let mut unknown = body;
    unknown.runtime_id = new_id();
    unknown.work.work = unknown.runtime_id;
    unknown.work.parent = new_id();
    rig.refused(&SerialKey::Runtime(unknown.runtime_id), &unknown)
        .await;
    assert!(rig.role().ledger().program(unknown.runtime_id).is_none());
    rig.stop().await;
}

#[tokio::test]
async fn an_admitted_program_channel_refuses_other_helpers_and_addresses_only_its_own_unit() {
    use orch_link::program_work::Client;
    use orch_link_contract::commands::NodeProgramAdmit;
    use orch_link_contract::program_work::{Admission, Reply, Request};
    use orch_link_contract::session::Role;
    use orch_link_contract::tunnels::{Service, TicketSource};
    use orch_pki::tickets::{TicketRequest, TicketSigner};
    use orch_testkit::doubles::{
        controller::ScriptedController,
        machine::{MachineSpec, ScriptedMachine},
    };
    let mut rig = Rig::new();
    rig.have_connector();
    let signer = TicketSigner::generate();
    rig.hub.set_ticket_verifier(signer.verifier());
    let peer = new_id();
    let _peer_controller =
        ScriptedController::attach(&rig.hub, rig.clock.clone(), peer, vec![Role::Provider]);
    let machine = ScriptedMachine::start(
        &rig.hub,
        &rig.clock,
        &rig.pki.machine(peer),
        &MachineSpec {
            roles: vec![Role::Provider],
            ..MachineSpec::default()
        },
    );
    rig.start().await;
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let parent = rig.spec(RuntimeKind::Provider, "isolated-channel-parent");
    rig.made(&parent).await;
    let admission = Admission {
        work: new_id(),
        helper: new_id(),
        peer,
        parent: parent.runtime_id,
        endpoints: vec![],
        deadline: rig.deadline(),
    };
    rig.succeed(
        &SerialKey::Runtime(admission.work),
        &NodeProgramAdmit {
            runtime_id: admission.work,
            work: admission.clone(),
        },
    )
    .await;
    let ticket = |helper| {
        signer
            .issue(&TicketRequest {
                src: TicketSource::Machine(peer),
                dst: rig.machine_id,
                service: Service::Program {
                    work_id: admission.work,
                },
                thread_id: Some(helper),
                tool_set: None,
                expires_at: admission.deadline,
            })
            .unwrap()
    };
    let request = Request::ReadFile {
        path: "../../provider-state/auth.json".into(),
        max_bytes: 1024,
    };
    let wrong = Client::new(machine.link.clone(), ticket(new_id()));
    assert!(
        wrong.request(&request, &[]).await.is_err(),
        "a real signed ticket for another helper is still refused by the admitted node"
    );
    let own = Client::new(machine.link.clone(), ticket(admission.helper));
    let answer = own.request(&request, &[]).await.unwrap();
    assert!(
        matches!(answer.reply, Reply::Failed { .. }),
        "the admitted channel cannot substitute its provider's unit for its absent work unit"
    );
    assert!(answer.bytes.is_empty());
    assert!(
        rig.role()
            .ledger()
            .program(admission.work)
            .unwrap()
            .unit
            .is_none()
    );
    assert_eq!(rig.driver.list(SYSTEM).await.unwrap().len(), 1);
    rig.stop().await;
}

#[tokio::test]
async fn the_native_role_stops_its_admitted_pending_start_and_reports_that_actual_source_without_touching_a_later_one()
 {
    use orch_link_contract::{
        commands::NodeRuntimeStopResult,
        native_target::{LifecycleSubject, LifecycleTarget, RuntimeSource},
    };
    for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
        let mut rig = Rig::new();
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(kind, "pending-original-stop-result");
        rig.made(&spec).await;
        rig.driver.set_enrolled(&spec.name, Some(true));
        let observed = rig
            .role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation
            .id()
            .unwrap();
        let opened = new_id();
        let subject = LifecycleSubject::Runtime {
            runtime_id: spec.runtime_id,
        };
        assert!(matches!(
            send_original_start(
                &rig,
                spec.runtime_id,
                LifecycleTarget {
                    subject: subject.clone(),
                    observed: Some(observed),
                    opening: Some(opened),
                    pending: vec![],
                    revision: 1,
                }
            )
            .await,
            Progress::Succeeded(_)
        ));
        let target = LifecycleTarget {
            subject,
            observed: Some(observed),
            opening: None,
            pending: vec![opened],
            revision: 2,
        };
        let Progress::Succeeded(result) =
            send_original_stop(&rig, spec.runtime_id, target.clone()).await
        else {
            panic!("the admitted pending opening is still the original Stop's unit");
        };
        let result: NodeRuntimeStopResult = serde_json::from_value(result).unwrap();
        assert_eq!(
            result,
            NodeRuntimeStopResult::Stopped {
                source: RuntimeSource {
                    incarnation: Some(opened)
                }
            }
        );
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
        let stopped = rig.driver.targets_of(Operation::Stop).len();
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
        let later = rig
            .role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .incarnation;
        assert_ne!(later.id(), Some(opened));
        assert!(
            matches!(send_original_stop(&rig,spec.runtime_id,LifecycleTarget {revision:3,..target}).await,
            Progress::Failed(error) if error.known()==Some(orch_link_contract::commands::ErrorCode::Conflict))
        );
        assert_eq!(rig.driver.targets_of(Operation::Stop).len(), stopped);
        assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
        assert_eq!(
            rig.role()
                .ledger()
                .get(spec.runtime_id)
                .unwrap()
                .incarnation,
            later
        );
        rig.stop().await;
    }
}
