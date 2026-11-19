//! The check of each build of an agent program: what its adapter uses, looked for in what the build offers. Once
//! per build, in the background, never when a session starts.
//!
//! A build read from a vendor's channel, or added by a system admin, starts untested. At each look of the channels'
//! schedule the newest build of each harness, system and processor that was never checked is checked, and a system
//! admin may check any build at any time. A host of that system and processor that is online and belongs to nobody's
//! home puts the build in its tool cache, verifying the vendor's checksum and refusing a build that differs, and runs
//! it so that it prints its interface (`node.tool.check`), then runs the adapter's native cases against the
//! scripted model in a networkless unit (`node.tool.conformance`). What it found stays with the build:
//!
//! * `checked`: the interface is usable and every native case passed. A name that only one capability needs switches that
//!   capability off for the build ([`HarnessBuildCheck::off`]), and the build is still offered.
//! * `failed`: a required interface name is missing, a native case failed, the build was refused, or the runner failed.
//! * `untested`, with the reason: no host could run the check now; the next look tries again.

use chrono::Duration;
use orch_agent_programs::manifest::{Checks, InterfaceProbe, Printed};
use orch_agent_programs::{ProgramManifest, Programs};
use orch_api_contract::admin::{
    HarnessBuild, HarnessBuildCheck, HarnessBuildStatus, HarnessCheckStage, HarnessConformanceCase,
};
use orch_db::DbError;
use orch_db::queries::resources::hosting::{self as host_rows, HostState};
use orch_db::queries::resources::management;
use orch_link::commands::CommandFailure;
use orch_link_contract::commands::{
    NodeToolCheck, NodeToolConformance, NodeToolConformanceResult, NodeToolEnsure, SerialKey,
    ToolInterface,
};
use orch_types::Time;

use crate::dispatch::RunError;
use crate::state::App;

/// What a build's identity is among the others: harness, system and processor, as the rows write them.
fn platform(build: &HarnessBuild) -> (String, String, String) {
    let word = |value: serde_json::Value| value.as_str().unwrap_or_default().to_owned();
    (
        build.kind.clone(),
        word(serde_json::json!(build.os)),
        word(serde_json::json!(build.arch)),
    )
}

/// Every program the system knows now: those Orch ships, and those its machines report.
async fn programs_of(app: &App) -> Result<Programs, DbError> {
    let mut conn = app.db.acquire().await?;
    orch_db::programs::registry(&mut conn).await
}

/// What a program's manifest says is checked of a build of it: the interface to look at, and the native cases.
fn checks_of<'a>(programs: &'a Programs, kind: &str) -> Option<(&'a ProgramManifest, &'a Checks)> {
    let manifest = programs.program(kind)?;
    manifest
        .checks
        .interface
        .is_some()
        .then_some((manifest, &manifest.checks))
}

/// Checks, for each harness, system and processor, the newest build kept in the system's rows when it was never
/// checked; one that could not be checked for want of a host is tried again. Older builds are checked only when a
/// system admin asks: a channel lists many releases, and each check fetches its build.
pub async fn check_waiting(app: &App) {
    check_waiting_kinds(app, None).await;
}

/// Checks only the harness kinds requested by effective place policies.
pub(crate) async fn check_requested(app: &App, requested: &std::collections::BTreeSet<String>) {
    check_waiting_kinds(app, Some(requested)).await;
}

async fn check_waiting_kinds(app: &App, requested: Option<&std::collections::BTreeSet<String>>) {
    let stored = match app.db.acquire().await {
        Ok(mut conn) => management::stored_harness_builds(&mut conn).await,
        Err(error) => Err(error),
    };
    let stored = match stored {
        Ok(stored) => stored,
        Err(error) => {
            tracing::warn!(%error, "the kept harness builds could not be read for their checks");
            return;
        }
    };
    let programs = match programs_of(app).await {
        Ok(programs) => programs,
        Err(error) => {
            tracing::warn!(%error, "the programs could not be read for the checks of their builds");
            return;
        }
    };
    let mut newest: std::collections::BTreeMap<(String, String, String), (String, HarnessBuild)> =
        std::collections::BTreeMap::new();
    let trust_mirror = match app.db.acquire().await {
        Ok(mut conn) => crate::harness_channels::trusts_mirror(&mut conn)
            .await
            .unwrap_or(false),
        Err(_) => false,
    };
    for (key, value) in stored {
        if !crate::harness_channels::checksum_trusted(&value, trust_mirror) {
            continue;
        }
        let Ok(build) = serde_json::from_value::<HarnessBuild>(value) else {
            continue;
        };
        if checks_of(&programs, &build.kind).is_none()
            || requested.is_some_and(|kinds| !kinds.contains(&build.kind))
        {
            continue;
        }
        let slot = newest.entry(platform(&build));
        match slot {
            std::collections::btree_map::Entry::Vacant(free) => {
                free.insert((key, build));
            }
            std::collections::btree_map::Entry::Occupied(mut held) => {
                if crate::hosting::version_order(&build.version, &held.get().1.version).is_gt() {
                    held.insert((key, build));
                }
            }
        }
    }
    for (key, build) in newest.into_values() {
        // A check whose server went away is taken again once it is stale; one under way keeps the build.
        if !matches!(
            build.status,
            HarnessBuildStatus::Untested | HarnessBuildStatus::Checking
        ) && !lacks_native_proof(&programs, &build)
        {
            continue;
        }
        if let Err(error) = check(app, &key, &build).await {
            tracing::warn!(%error, kind = %build.kind, version = %build.version, "a harness build could not be checked");
        }
    }
}

/// Older interface-only certifications do not qualify as checked for the current native catalogue.
pub(crate) fn lacks_native_proof(programs: &Programs, build: &HarnessBuild) -> bool {
    let Some((_, checks)) = checks_of(programs, &build.kind) else {
        return false;
    };
    if build.status != HarnessBuildStatus::Checked
        || build.source == orch_api_contract::admin::HarnessBuildSource::Release
    {
        return false;
    }
    build.check.as_ref().is_none_or(|check| {
        check.stage != HarnessCheckStage::Complete
            || !complete_native_report(checks, &check.cases)
            || check.cases.iter().any(|case| !case.passed)
    })
}

/// Checks the build kept under `key` now, unless a check holds it already.
///
/// # Errors
/// The database failed.
pub async fn check(app: &App, key: &str, build: &HarnessBuild) -> Result<(), DbError> {
    if claim(app, key).await? {
        finish(app, key, build).await?;
    }
    Ok(())
}

/// Marks the build kept under `key` as `checking` for this check, unless a check holds it already; the answer says
/// whether this one has it.
///
/// # Errors
/// The database failed.
pub async fn claim(app: &App, key: &str) -> Result<bool, DbError> {
    let mut conn = app.db.acquire().await?;
    claim_in(&mut conn, key, app.clock.now()).await
}

/// As [`claim`], in the transaction of a request that checked its caller's authority.
///
/// # Errors
/// The database failed.
pub async fn claim_in(
    conn: &mut orch_db::Conn,
    key: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, DbError> {
    let stage = timeout_setting(
        conn,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_CONFORMANCE_TIMEOUT_SECS,
    )
    .await?;
    let interface = timeout_setting(
        conn,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_INTERFACE_TIMEOUT_SECS,
    )
    .await?;
    let ensure = orch_link_contract::commands::CommandKind::NodeToolEnsure.deadline_secs();
    management::claim_harness_build_check(
        conn,
        key,
        now,
        now - Duration::seconds(i64::from(stage) + i64::from(interface) + i64::from(ensure)),
    )
    .await
}

/// Runs the check of a build this check has claimed, and keeps what it found with the build.
///
/// # Errors
/// The database failed.
pub async fn finish(app: &App, key: &str, build: &HarnessBuild) -> Result<(), DbError> {
    if !checksum_is_trusted(app, key, build).await? {
        let mut conn = app.db.acquire().await?;
        let check = serde_json::json!({
            "at": Time::new(app.clock.now()), "machine": null, "missing": [], "off": [],
            "error": "the build no longer has trusted checksum authority; import its vendor checksum or explicitly trust its publisher"
        });
        management::set_harness_build_status(
            &mut conn,
            key,
            "untested",
            Some(&check),
            app.clock.now(),
        )
        .await?;
        return Ok(());
    }
    let programs = programs_of(app).await?;
    let (status, mut check) = run(app, &programs, key, build).await;
    if let Some(previous) = &build.check
        && let Some((_, checks)) = checks_of(&programs, &build.kind)
    {
        preserve_unproved_limits(checks, previous, &mut check);
    }
    if status != HarnessBuildStatus::Untested {
        check.stage = HarnessCheckStage::Complete;
    }
    let mut conn = app.db.acquire().await?;
    let check = serde_json::to_value(&check)
        .map_err(|error| DbError::Other(sqlx::Error::Decode(error.into())))?;
    management::set_harness_build_status(
        &mut conn,
        key,
        status_word(status),
        Some(&check),
        app.clock.now(),
    )
    .await?;
    Ok(())
}

/// A recorded download-integrity refusal; ordinary compatibility failures have no such prefix.
pub(crate) const BUILD_REFUSAL_PREFIX: &str = "refused:";

async fn checksum_is_trusted(app: &App, key: &str, build: &HarnessBuild) -> Result<bool, DbError> {
    let mut conn = app.db.acquire().await?;
    let trust_mirror = crate::harness_channels::trusts_mirror(&mut conn).await?;
    Ok(orch_db::queries::settings::row(&mut conn, None, key)
        .await?
        .is_some_and(|stored| {
            crate::harness_channels::checksum_trusted(&stored.value, trust_mirror)
                && stored
                    .value
                    .get("sha256")
                    .and_then(serde_json::Value::as_str)
                    == Some(build.sha256.as_str())
        }))
}

fn status_word(status: HarnessBuildStatus) -> &'static str {
    match status {
        HarnessBuildStatus::Untested => "untested",
        HarnessBuildStatus::Checking => "checking",
        HarnessBuildStatus::Checked => "checked",
        HarnessBuildStatus::Failed => "failed",
    }
}

/// What a check of `build` comes to.
async fn run(
    app: &App,
    programs: &Programs,
    build_key: &str,
    build: &HarnessBuild,
) -> (HarnessBuildStatus, HarnessBuildCheck) {
    let mut found = HarnessBuildCheck {
        at: Time::new(app.clock.now()),
        machine: None,
        missing: Vec::new(),
        off: Vec::new(),
        error: None,
        stage: HarnessCheckStage::Interface,
        cases: Vec::new(),
    };
    let Some((manifest, checks)) = checks_of(programs, &build.kind) else {
        found.error = Some(format!(
            "no manifest of a program of kind {} says how a build of it is checked",
            build.kind
        ));
        return (HarnessBuildStatus::Untested, found);
    };
    let Some(interface) = checks.interface.as_ref() else {
        return (HarnessBuildStatus::Untested, found);
    };
    let host = match host_for(app, build).await {
        Ok(Some(host)) => host,
        Ok(None) => {
            let (_, os, arch) = platform(build);
            found.error = Some(format!(
                "no host of {os} {arch} outside a personal home is online to check it on; it is checked when one is"
            ));
            return (HarnessBuildStatus::Untested, found);
        }
        Err(error) => {
            found.error = Some(format!("the hosts could not be read: {error}"));
            return (HarnessBuildStatus::Untested, found);
        }
    };
    found.machine = Some(host.name.clone());
    let timeouts = async {
        let mut conn = app.db.acquire().await?;
        use orch_settings_contract::registry::keys;
        Ok::<_, DbError>((
            timeout_setting(&mut conn, keys::AGENTS_HARNESS_INTERFACE_TIMEOUT_SECS).await?,
            timeout_setting(&mut conn, keys::AGENTS_HARNESS_CASE_TIMEOUT_SECS).await?,
            timeout_setting(&mut conn, keys::AGENTS_HARNESS_CONFORMANCE_TIMEOUT_SECS).await?,
        ))
    }
    .await;
    let (print_secs, case_secs, stage_secs) = match timeouts {
        Ok(timeouts) => timeouts,
        Err(error) => {
            found.error = Some(format!("the check settings could not be read: {error}"));
            return (HarnessBuildStatus::Untested, found);
        }
    };
    let mirror = match app.db.acquire().await {
        Ok(mut conn) => orch_settings::get::<String>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::AGENTS_HARNESS_MIRROR,
        )
        .await
        .ok()
        .flatten(),
        Err(_) => None,
    };
    let url = crate::harness_channels::download_url(programs, build, mirror.as_deref());
    let dispatcher = app.link.dispatcher();
    let key = SerialKey::Tool(build.kind.clone());
    let ensured = dispatcher
        .run(
            host.machine_id,
            &key,
            &NodeToolEnsure {
                job_id: None,
                name: build.kind.clone(),
                version: build.version.clone(),
                url,
                sha256: build.sha256.clone(),
            },
        )
        .await;
    if let Err(error) = ensured {
        let refused = matches!(&error, RunError::Command(CommandFailure::Failed(failure))
            if failure.details.as_ref().and_then(|details| details.get("checksum")).is_some());
        found.error = Some(if refused {
            format!(
                "{BUILD_REFUSAL_PREFIX} the download does not match its trusted checksum ({error})"
            )
        } else {
            format!(
                "the build could not be put in the tool cache of {}: {error}",
                host.name
            )
        });
        return (
            if refused {
                HarnessBuildStatus::Failed
            } else {
                HarnessBuildStatus::Untested
            },
            found,
        );
    }
    // A download may take minutes. Revocation or a replaced checksum must still stop execution afterwards.
    match checksum_is_trusted(app, build_key, build).await {
        Ok(true) => {}
        Ok(false) => {
            found.error = Some("the build no longer has trusted checksum authority".to_owned());
            return (HarnessBuildStatus::Untested, found);
        }
        Err(error) => {
            found.error = Some(format!(
                "the build's checksum authority could not be read: {error}"
            ));
            return (HarnessBuildStatus::Untested, found);
        }
    }
    let checked = run_within(
        app,
        host.machine_id,
        &key,
        &probe(manifest, interface, build, print_secs),
        print_secs,
    )
    .await;
    match checked {
        Ok(answer) => {
            let (usable, off) = interface.verdict(&answer.missing);
            found.missing = answer.missing;
            found.off = off;
            if !usable {
                return (HarnessBuildStatus::Failed, found);
            }
            if checks.conformance.is_empty() {
                // Its manifest lists no native case: the interface is the whole check.
                found.stage = HarnessCheckStage::Complete;
                return (HarnessBuildStatus::Checked, found);
            }
            found.stage = HarnessCheckStage::Conformance;
            let saved = async {
                let mut conn = app.db.acquire().await?;
                let mut checking = found.clone();
                if let Some(previous) = &build.check {
                    for feature in &previous.off {
                        if !checking.off.contains(feature) {
                            checking.off.push(feature.clone());
                        }
                    }
                }
                let value = serde_json::to_value(&checking)
                    .map_err(|error| DbError::Other(sqlx::Error::Decode(error.into())))?;
                management::set_harness_build_status(
                    &mut conn,
                    build_key,
                    "checking",
                    Some(&value),
                    app.clock.now(),
                )
                .await
            }
            .await;
            if let Err(error) = saved {
                found.error = Some(format!("the check's stage could not be kept: {error}"));
                return (HarnessBuildStatus::Untested, found);
            }
            let request = NodeToolConformance {
                name: build.kind.clone(),
                version: build.version.clone(),
                programs: manifest
                    .probe_programs()
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                case_timeout_secs: case_secs,
                timeout_secs: stage_secs,
            };
            let began = std::time::Instant::now();
            let checked = run_within(app, host.machine_id, &key, &request, stage_secs).await;
            match checked {
                Ok(answer) => {
                    let status = conformance_verdict(checks, answer, &mut found);
                    (status, found)
                }
                Err(RunError::Command(CommandFailure::Failed(failure)))
                    if matches!(
                        failure.code.as_str(),
                        "unsupported" | "unavailable" | "no_capacity" | "not_found"
                    ) || (failure.retryable && failure.code != "timeout") =>
                {
                    found.error = Some(failure.detail);
                    (HarnessBuildStatus::Untested, found)
                }
                Err(RunError::Dispatch(error)) => {
                    found.error = Some(format!(
                        "the native check could not be dispatched to {}: {error}",
                        host.name
                    ));
                    (HarnessBuildStatus::Untested, found)
                }
                Err(RunError::Command(CommandFailure::Expired | CommandFailure::Cancelled)) => {
                    found.error = Some("the native check was not completed by its host".to_owned());
                    (HarnessBuildStatus::Untested, found)
                }
                Err(error) => {
                    let detail =
                        format!("the native check could not run on {}: {error}", host.name);
                    found.error = Some(detail.clone());
                    found.cases.push(HarnessConformanceCase {
                        name: "runner".to_owned(),
                        passed: false,
                        feature: None,
                        unsupported: None,
                        error: Some(detail),
                        elapsed_ms: i64::try_from(began.elapsed().as_millis()).unwrap_or(i64::MAX),
                    });
                    (HarnessBuildStatus::Failed, found)
                }
            }
        }
        Err(RunError::Command(CommandFailure::Failed(failure))) => {
            // The build ran and could not say what it offers: what the adapter needs cannot be shown to be there.
            found.error = Some(failure.detail);
            (HarnessBuildStatus::Failed, found)
        }
        Err(error) => {
            found.error = Some(format!("the check could not run on {}: {error}", host.name));
            (HarnessBuildStatus::Untested, found)
        }
    }
}

/// What a host is asked to run for `interface` of the program `manifest` describes.
fn probe(
    manifest: &ProgramManifest,
    interface: &InterfaceProbe,
    build: &HarnessBuild,
    timeout_secs: u32,
) -> NodeToolCheck {
    NodeToolCheck {
        name: build.kind.clone(),
        version: build.version.clone(),
        programs: manifest
            .probe_programs()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        args: interface.args.clone(),
        printed: match interface.printed {
            Printed::Text => ToolInterface::Text,
            Printed::Schema => ToolInterface::Schema,
        },
        names: interface
            .needs
            .iter()
            .map(|need| need.name.clone())
            .collect(),
        timeout_secs,
    }
}

async fn timeout_setting(conn: &mut orch_db::Conn, key: &str) -> Result<u32, DbError> {
    let value = orch_settings::get::<i64>(conn, orch_settings::Place::System, key)
        .await
        .map_err(|error| DbError::Other(sqlx::Error::Decode(error.into())))?
        .or_else(|| {
            orch_settings_contract::registry::get(key)
                .and_then(|setting| setting.default_value())
                .and_then(|value| value.as_i64())
        })
        .ok_or_else(|| DbError::Other(sqlx::Error::Protocol(format!("no default for {key}"))))?;
    u32::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            DbError::Other(sqlx::Error::Protocol(format!(
                "{key} must fit positive seconds"
            )))
        })
}

async fn run_within<B: orch_link_contract::commands::CommandBody>(
    app: &App,
    machine: orch_types::Id,
    key: &SerialKey,
    request: &B,
    seconds: u32,
) -> Result<B::Result, RunError> {
    let dispatcher = app.link.dispatcher();
    let issued = dispatcher
        .issue_as(
            orch_types::new_id(),
            machine,
            key,
            Time::new(app.clock.now() + Duration::seconds(i64::from(seconds))),
            request,
        )
        .await?;
    dispatcher.wait_for::<B>(issued.id).await
}

/// Only a complete native report certifies a build; the server owns each case's feature mapping.
fn conformance_verdict(
    checks: &Checks,
    answer: NodeToolConformanceResult,
    found: &mut HarnessBuildCheck,
) -> HarnessBuildStatus {
    let expected: Vec<(&str, Option<&str>)> = checks.cases().collect();
    let mut cases = answer.cases;
    let complete = complete_native_report(checks, &cases);
    if !complete {
        cases.push(HarnessConformanceCase {
            name: "runner".to_owned(),
            passed: false,
            feature: None,
            unsupported: None,
            error: Some(
                "the native report omitted, repeated or contradicted required cases".to_owned(),
            ),
            elapsed_ms: 0,
        });
    }
    for case in &mut cases {
        case.feature = expected
            .iter()
            .find(|(name, _)| *name == case.name)
            .and_then(|(_, feature)| *feature)
            .map(str::to_owned);
        if (!case.passed || case.unsupported.is_some())
            && let Some(feature) = &case.feature
            && !found.off.contains(feature)
        {
            found.off.push(feature.clone());
        }
    }
    let passed = complete && cases.iter().all(|case| case.passed);
    found.cases = cases;
    if passed {
        HarnessBuildStatus::Checked
    } else {
        HarnessBuildStatus::Failed
    }
}

fn complete_native_report(checks: &Checks, cases: &[HarnessConformanceCase]) -> bool {
    let expected: Vec<(&str, Option<&str>)> = checks.cases().collect();
    cases.len() == expected.len()
        && cases.iter().all(|case| {
            case.elapsed_ms >= 0
                && case.passed == case.error.is_none()
                && case.unsupported.as_ref().is_none_or(|reason| {
                    case.name == "history.native_import" && case.passed && !reason.trim().is_empty()
                })
        })
        && expected
            .iter()
            .all(|(name, _)| cases.iter().filter(|case| case.name == *name).count() == 1)
}

/// Rechecking never temporarily re-enables a known broken feature before its own evidence passes.
fn preserve_unproved_limits(
    checks: &Checks,
    previous: &HarnessBuildCheck,
    found: &mut HarnessBuildCheck,
) {
    let expected: Vec<(&str, Option<&str>)> = checks.cases().collect();
    for feature in &previous.off {
        let probes: Vec<_> = expected
            .iter()
            .filter(|(_, affected)| *affected == Some(feature.as_str()))
            .collect();
        let unproved = if probes.is_empty() {
            found.stage == HarnessCheckStage::Interface
        } else {
            !probes.iter().all(|(name, _)| {
                found.cases.iter().any(|case| {
                    case.name == *name
                        && case.passed
                        && case.error.is_none()
                        && case.unsupported.is_none()
                        && case.elapsed_ms >= 0
                })
            })
        };
        if unproved && !found.off.contains(feature) {
            found.off.push(feature.clone());
        }
    }
}

/// A host to check `build` on: online, active, switched on, of the build's system and processor, and in nobody's
/// personal home (a person's machine runs what its owner chose); the system's own before an org's, and one that
/// takes new units first.
async fn host_for(app: &App, build: &HarnessBuild) -> Result<Option<HostState>, DbError> {
    let (_, os, arch) = platform(build);
    let mut conn = app.db.acquire().await?;
    let hosts: Vec<HostState> = host_rows::hosts(&mut conn, None)
        .await?
        .into_iter()
        .filter(|host| {
            host.online
                && host.machine_status == "active"
                && !host.disabled
                && host.platform_os == os
                && host.platform_arch == arch
        })
        .collect();
    let orgs: Vec<_> = hosts.iter().filter_map(|host| host.org_id).collect();
    let personal = management::personal_orgs(&mut conn, &orgs).await?;
    let mut hosts: Vec<HostState> = hosts
        .into_iter()
        .filter(|host| host.org_id.is_none_or(|org| !personal.contains(&org)))
        .collect();
    hosts.sort_by_key(|host| (host.org_id.is_some(), !host.schedulable));
    Ok(hosts.into_iter().next())
}

/// The builds a check is asked for by a system admin: every kept build of `kind` at `version`, with its key.
///
/// # Errors
/// The database failed.
pub async fn kept(
    conn: &mut orch_db::Conn,
    kind: &str,
    version: &str,
) -> Result<Vec<(String, HarnessBuild)>, DbError> {
    let trust_mirror = crate::harness_channels::trusts_mirror(conn).await?;
    Ok(
        orch_db::programs::admin::stored_version(conn, kind, version)
            .await?
            .into_iter()
            .filter(|(_, value)| crate::harness_channels::checksum_trusted(value, trust_mirror))
            .filter_map(|(key, value)| {
                Some((key, serde_json::from_value::<HarnessBuild>(value).ok()?))
            })
            .filter(|(_, build)| build.kind == kind && build.version == version)
            .collect(),
    )
}
