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
//!
//! A check keeps what it proved. When a release adds a case to a program's manifest, a build whose stored check passed
//! every case it had stays `checked`: the cases it has not run are pending ([`pending_cases`]), not failed, and the
//! background runs them ([`check_completing`]). A run that decides nothing about the build (no host, a runner that
//! failed, a node of an earlier release that lacks a case) never takes away what the build had proved.

use chrono::Duration;

use orch_agent_programs::manifest::Checks;

use orch_agent_programs::manifest::InterfaceProbe;

use orch_agent_programs::manifest::Printed;

use orch_agent_programs::ProgramManifest;

use orch_agent_programs::Programs;

use orch_api_contract::admin::HarnessBuild;

use orch_api_contract::admin::HarnessBuildCheck;

use orch_api_contract::admin::HarnessBuildStatus;

use orch_api_contract::admin::HarnessCheckStage;

use orch_api_contract::admin::HarnessConformanceCase;

use orch_db::DbError;

use orch_db::queries::resources::hosting::HostState;

use orch_db::queries::resources::management;

use orch_engine::build_checks::Claim;

use orch_engine::build_checks::Request;

use orch_engine::build_checks::Work;

use orch_link::commands::CommandFailure;

use orch_link_contract::commands::NodeToolCheck;

use orch_link_contract::commands::NodeToolConformance;

use orch_link_contract::commands::NodeToolConformanceResult;

use orch_link_contract::commands::NodeToolEnsure;

use orch_link_contract::commands::SerialKey;

use orch_link_contract::commands::ToolInterface;

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
/// checked, and every kept build of the version that a registered machine reports installed (the build such a
/// machine runs is checked, however old); one that could not be checked for want of a host is tried again. Other
/// older builds are checked only when a system admin asks: a channel lists many releases, and each check fetches
/// its build.
pub async fn check_waiting(app: &App) {
    check_waiting_kinds(app, None, false).await;
}

/// Checks only the harness kinds requested by effective place policies.
pub(crate) async fn check_requested(app: &App, requested: &std::collections::BTreeSet<String>) {
    check_waiting_kinds(app, Some(requested), false).await;
}

/// Runs, for every kept build that was checked before, the cases of the present manifest it has not run: a release
/// that adds a case does not leave the builds it had checked unproved for want of someone's press. Not demand: it
/// fetches nothing that was not fetched once for its first check, and it is off when the schedule is.
pub async fn check_completing(app: &App) {
    check_waiting_kinds(app, None, true).await;
}

/// The interval of the release schedule in hours; zero keeps what the system does on its own off.
async fn channel_hours(app: &App) -> i64 {
    let key = orch_settings_contract::registry::keys::AGENTS_HARNESS_CHANNEL_HOURS;
    let Ok(mut conn) = app.db.acquire().await else {
        return 0;
    };
    orch_settings::get::<i64>(&mut conn, orch_settings::Place::System, key)
        .await
        .ok()
        .flatten()
        .or_else(|| {
            orch_settings_contract::registry::get(key)
                .and_then(|setting| setting.default_value())
                .and_then(|value| value.as_i64())
        })
        .unwrap_or(0)
}

fn check_waiting_kinds<'a>(
    app: &'a App,
    requested: Option<&'a std::collections::BTreeSet<String>>,
    completing_only: bool,
) -> futures::future::BoxFuture<'a, ()> {
    Box::pin(async move {
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
        let mut newest: std::collections::BTreeMap<
            (String, String, String),
            (String, HarnessBuild),
        > = std::collections::BTreeMap::new();
        // The builds that machines run: a check applies to every machine that reports the version.
        let running: std::collections::BTreeSet<(String, String)> = match app.db.acquire().await {
            Ok(mut conn) => {
                orch_db::queries::resources::binding::machine_harness_versions(&mut conn)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            }
            Err(_) => std::collections::BTreeSet::new(),
        };
        let mut run_by_machines: std::collections::BTreeMap<String, HarnessBuild> =
            std::collections::BTreeMap::new();
        // The builds taken up for the cases of the present manifest that they have not run.
        let mut completing: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let hours = channel_hours(app).await;
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
            if hours > 0 && takes_up_for_pending_cases(&programs, &build, app.clock.now(), hours) {
                completing.insert(key.clone());
                run_by_machines.insert(key.clone(), build.clone());
            }
            if completing_only {
                continue;
            }
            if running.contains(&(build.kind.clone(), build.version.clone())) {
                run_by_machines.insert(key.clone(), build.clone());
            }
            let slot = newest.entry(platform(&build));
            match slot {
                std::collections::btree_map::Entry::Vacant(free) => {
                    free.insert((key, build));
                }
                std::collections::btree_map::Entry::Occupied(mut held) => {
                    if crate::hosting::version_order(&build.version, &held.get().1.version).is_gt()
                    {
                        held.insert((key, build));
                    }
                }
            }
        }
        let mut to_check: std::collections::BTreeMap<String, HarnessBuild> = run_by_machines;
        for (key, build) in newest.into_values() {
            to_check.entry(key).or_insert(build);
        }
        for (key, build) in to_check {
            // A check whose server went away is taken again once it is stale; one under way keeps the build.
            if !matches!(
                build.status,
                HarnessBuildStatus::Untested | HarnessBuildStatus::Checking
            ) && !lacks_native_proof(&programs, &build)
                && !completing.contains(&key)
            {
                continue;
            }
            if let Err(error) = check(app, &key, &build).await {
                tracing::warn!(%error, kind = %build.kind, version = %build.version, "a harness build could not be checked");
            }
        }
    })
}

/// Whether a build that is marked checked has no native proof at all: an older interface-only certification, a check
/// that did not end, or one whose record contradicts itself or holds a failure. Such a build is untested until its
/// native cases pass in the background. A check that passed every case it had is proof, whatever cases the
/// manifest has gained since ([`pending_cases`]).
pub fn lacks_native_proof(programs: &Programs, build: &HarnessBuild) -> bool {
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
            || !sound_native_report(checks, &check.cases)
            || check.cases.iter().any(|case| !case.passed)
    })
}

/// The names of the cases of the present manifest that the stored check of a build has not run: not failed, not
/// verified yet. None for a build that was never checked, which is untested and not pending.
#[must_use]
pub fn pending_cases(programs: &Programs, build: &HarnessBuild) -> Vec<String> {
    let (Some((_, checks)), Some(check)) = (checks_of(programs, &build.kind), build.check.as_ref())
    else {
        return Vec::new();
    };
    checks
        .cases()
        .map(|(name, _)| name)
        .filter(|name| !check.cases.iter().any(|case| case.name == *name))
        .map(str::to_owned)
        .collect()
}

/// Whether the background takes up a build for the cases of the present manifest it has not run: its native cases were
/// run once (a check that ended with some passed, so not an interface that failed, a refused download or a runner
/// that failed), it lacks a case, and the last run was not one that a node of an earlier release could not complete
/// within the interval (that would only run the same cases again).
fn takes_up_for_pending_cases(
    programs: &Programs,
    build: &HarnessBuild,
    now: chrono::DateTime<chrono::Utc>,
    hours: i64,
) -> bool {
    let Some(check) = build.check.as_ref() else {
        return false;
    };
    matches!(
        build.status,
        HarnessBuildStatus::Checked | HarnessBuildStatus::Failed
    ) && build.source != orch_api_contract::admin::HarnessBuildSource::Release
        && check.stage == HarnessCheckStage::Complete
        && check
            .cases
            .iter()
            .any(|case| case.passed && case.name != "runner")
        && !pending_cases(programs, build).is_empty()
        && !check.error.as_deref().is_some_and(|note| {
            note.contains(EARLIER_NODE_NOTE)
                && now.signed_duration_since(check.at.get()) < Duration::hours(hours)
        })
}

/// Checks the build kept under `key` now, unless a check holds it already.
///
/// # Errors
/// The database failed.
pub async fn check(app: &App, key: &str, build: &HarnessBuild) -> Result<(), DbError> {
    if let Some(claim) = claim_task(app, key).await? {
        finish(app, key, build, claim).await?;
    }
    Ok(())
}

/// Marks the build kept under `key` as `checking` for this check, unless a check holds it already; the answer says
/// whether this one has it.
///
/// # Errors
/// The database failed.
pub async fn claim(app: &App, key: &str) -> Result<bool, DbError> {
    Ok(claim_task(app, key).await?.is_some())
}

async fn claim_task(app: &App, key: &str) -> Result<Option<Claim>, DbError> {
    let mut tx = app.db.begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let claim =
        claim_in_before(&mut tx, key, app.clock.now(), None, || Ok::<_, DbError>(())).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(claim)
}

fn native_error(error: impl std::fmt::Display) -> DbError {
    DbError::Other(sqlx::Error::Protocol(error.to_string()))
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
    Ok(
        claim_in_before(conn, key, now, None, || Ok::<_, DbError>(()))
            .await?
            .is_some(),
    )
}

/// The same native claimant with its prepared original operation and actual first SQL cut.
pub async fn claim_in_before<E: From<DbError>>(
    conn: &mut orch_db::Conn,
    key: &str,
    now: chrono::DateTime<chrono::Utc>,
    request: Option<Request>,
    before: impl FnOnce() -> Result<(), E>,
) -> Result<Option<Claim>, E> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        return Err(native_error("a native build claim requires its transaction").into());
    }
    if sqlx::query("SELECT key FROM settings WHERE node_id IS NULL AND key=$1 FOR UPDATE")
        .bind(key)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?
        .is_none()
    {
        return Ok(None);
    }
    let claim = Claim::prepare(conn, key, request)
        .await
        .map_err(native_error)?;
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
    before()?;
    let claimed = management::claim_harness_build_check(
        conn,
        key,
        now,
        now - Duration::seconds(i64::from(stage) + i64::from(interface) + i64::from(ensure)),
    )
    .await?;
    if claimed {
        claim.store(conn).await.map_err(native_error)?;
        Ok(Some(claim))
    } else {
        Ok(None)
    }
}

/// Runs the check of a build this check has claimed, and keeps what it found with the build.
///
/// # Errors
/// The database failed.
pub async fn finish(
    app: &App,
    key: &str,
    build: &HarnessBuild,
    claim: Claim,
) -> Result<(), DbError> {
    if !claim.owns(key, build) {
        return Err(native_error(
            "the supplied build differs from its native claim",
        ));
    }
    let mut work = Work::new(claim);
    let outcome = finish_work(app, key, build, &mut work).await;
    if outcome.is_err() {
        let mut tx = app.db.begin().await?;
        work.claim()
            .release(&mut tx, Time::new(app.clock.now()))
            .await
            .map_err(native_error)?;
        tx.commit().await.map_err(DbError::from)?;
    }
    outcome
}

async fn current(app: &App, work: &Work, conn: &mut orch_db::Conn) -> Result<(), DbError> {
    if let Some(checked) = work
        .checkpoint(conn, app.clock.as_ref())
        .await
        .map_err(native_error)?
    {
        checked
            .before
            .check(app.clock.as_ref())
            .map_err(native_error)?;
    }
    Ok(())
}

async fn finish_work(
    app: &App,
    key: &str,
    build: &HarnessBuild,
    work: &mut Work,
) -> Result<(), DbError> {
    if !checksum_is_trusted(app, key, build).await? {
        let mut conn = app.db.acquire().await?;
        let check = serde_json::json!({
            "at": Time::new(app.clock.now()), "machine": null, "missing": [], "off": [],
            "error": "the build no longer has trusted checksum authority; import its vendor checksum or explicitly trust its publisher"
        });
        work.claim()
            .finish_negative(
                &mut conn,
                HarnessBuildStatus::Untested,
                &check,
                Time::new(app.clock.now()),
            )
            .await
            .map_err(native_error)?;
        return Ok(());
    }
    let programs = programs_of(app).await?;
    let (status, mut check) = run(app, &programs, key, build, work).await;
    if let Some(previous) = &build.check
        && decided_nothing(status, &check)
        && previous.stage == HarnessCheckStage::Complete
        && matches!(
            build.status,
            HarnessBuildStatus::Checked | HarnessBuildStatus::Failed
        )
    {
        // A run that could not reach a host, or whose runner failed, says nothing about the build: what the last
        // whole check proved stays, with the word that this attempt did not run.
        let mut kept = previous.clone();
        kept.error = Some(format!(
            "A later check did not run: {}",
            check
                .error
                .as_deref()
                .or_else(|| check.cases.iter().find_map(|case| case.error.as_deref()))
                .unwrap_or("the check could not run")
        ));
        let kept = serde_json::to_value(&kept)
            .map_err(|error| DbError::Other(sqlx::Error::Decode(error.into())))?;
        let mut conn = app.db.begin().await?;
        if build.status == HarnessBuildStatus::Failed {
            work.claim()
                .finish_negative(
                    &mut conn,
                    HarnessBuildStatus::Failed,
                    &kept,
                    Time::new(app.clock.now()),
                )
                .await
                .map_err(native_error)?;
        } else {
            current(app, work, &mut conn).await?;
            management::set_harness_build_status(
                &mut conn,
                key,
                "checked",
                Some(&kept),
                app.clock.now(),
            )
            .await?;
        }
        conn.commit().await.map_err(DbError::from)?;
        return Ok(());
    }
    if let Some(previous) = &build.check
        && let Some((_, checks)) = checks_of(&programs, &build.kind)
    {
        preserve_unproved_limits(checks, previous, &mut check);
    }
    if status != HarnessBuildStatus::Untested {
        check.stage = HarnessCheckStage::Complete;
    }
    let check = serde_json::to_value(&check)
        .map_err(|error| DbError::Other(sqlx::Error::Decode(error.into())))?;
    let mut conn = app.db.begin().await?;
    if matches!(
        status,
        HarnessBuildStatus::Untested | HarnessBuildStatus::Failed
    ) {
        work.claim()
            .finish_negative(&mut conn, status, &check, Time::new(app.clock.now()))
            .await
            .map_err(native_error)?;
        conn.commit().await.map_err(DbError::from)?;
        return Ok(());
    }
    current(app, work, &mut conn).await?;
    management::set_harness_build_status(
        &mut conn,
        key,
        status_word(status),
        Some(&check),
        app.clock.now(),
    )
    .await?;
    conn.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Whether a run found nothing about the build: it could not reach a host, or no case of the build failed and only
/// the runner did. A missing interface name, a refused download and a failed case are findings.
fn decided_nothing(status: HarnessBuildStatus, check: &HarnessBuildCheck) -> bool {
    match status {
        HarnessBuildStatus::Untested => true,
        HarnessBuildStatus::Failed => {
            check.missing.is_empty()
                && check
                    .error
                    .as_deref()
                    .is_none_or(|error| !error.starts_with(BUILD_REFUSAL_PREFIX))
                && check
                    .cases
                    .iter()
                    .any(|case| case.name == "runner" && !case.passed)
                && check
                    .cases
                    .iter()
                    .all(|case| case.name == "runner" || case.passed)
        }
        HarnessBuildStatus::Checking | HarnessBuildStatus::Checked => false,
    }
}

/// A recorded download-integrity refusal; ordinary compatibility failures have no such prefix.
pub const BUILD_REFUSAL_PREFIX: &str = "refused:";

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
    work: &mut Work,
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
    *work = work.on_host(&host);
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
    let key = SerialKey::Tool(build.kind.clone());
    let ensured = run_within(
        app,
        host.machine_id,
        &key,
        &NodeToolEnsure {
            job_id: None,
            name: build.kind.clone(),
            version: build.version.clone(),
            url,
            sha256: build.sha256.clone(),
        },
        orch_link_contract::commands::CommandKind::NodeToolEnsure.deadline_secs(),
        work,
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
        work,
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
                let mut conn = app.db.begin().await?;
                let mut checking = found.clone();
                if let Some(previous) = &build.check {
                    for feature in &previous.off {
                        if !checking.off.contains(feature) {
                            checking.off.push(feature.clone());
                        }
                    }
                    // What the last whole check proved stays readable while this one runs, as its failures do.
                    if previous.stage == HarnessCheckStage::Complete {
                        checking.cases.clone_from(&previous.cases);
                    }
                }
                let value = serde_json::to_value(&checking)
                    .map_err(|error| DbError::Other(sqlx::Error::Decode(error.into())))?;
                current(app, work, &mut conn).await?;
                let saved = management::set_harness_build_status(
                    &mut conn,
                    build_key,
                    "checking",
                    Some(&value),
                    app.clock.now(),
                )
                .await?;
                conn.commit().await.map_err(DbError::from)?;
                Ok::<_, DbError>(saved)
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
            let checked = run_within(app, host.machine_id, &key, &request, stage_secs, work).await;
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
    work: &Work,
) -> Result<B::Result, RunError> {
    let dispatcher = app.link.dispatcher();
    let id = orch_types::new_id();
    let outgoing = orch_engine::commands::Outgoing::new(id, machine, key.clone(), request)
        .map_err(|_| RunError::Dispatch(crate::dispatch::DispatchError::Unavailable))?;
    let authority = orch_engine::build_checks::CheckCommand::capture(work, &outgoing)
        .map_err(|_| RunError::Dispatch(crate::dispatch::DispatchError::Unavailable))?;
    let issued = dispatcher
        .issue_stored(crate::dispatch::StoredCommand {
            id: Some(id),
            machine,
            key: key.wire(),
            kind: B::KIND.as_str().to_owned(),
            deadline: Some(Time::new(
                app.clock.now() + Duration::seconds(i64::from(seconds)),
            )),
            body: outgoing.body,
            authority: Some(orch_engine::commands::CommandAuthority::harness_check(
                authority,
            )),
        })
        .await?;
    dispatcher.wait_for::<B>(issued.id).await
}

/// The words a node of an earlier release leaves on a check that lacks cases it does not know.
const EARLIER_NODE_NOTE: &str = "not run by this node's version";

/// What a node reported is a report when it is not empty (for a program with cases), tells no case twice and no case
/// against itself. Which cases of the present manifest it covers is [`pending_cases`]'s concern: a report is not
/// refused for lacking a case that a node of an earlier release does not have.
fn sound_native_report(checks: &Checks, cases: &[HarnessConformanceCase]) -> bool {
    (cases.len() >= checks.conformance.len().min(1))
        && cases.iter().all(|case| {
            case.elapsed_ms >= 0
                && case.passed == case.error.is_none()
                && case.unsupported.as_ref().is_none_or(|reason| {
                    case.name == "history.native_import" && case.passed && !reason.trim().is_empty()
                })
                && cases.iter().filter(|other| other.name == case.name).count() == 1
        })
}

/// What a certifying report is: sound, and every case it holds passed. A case of the manifest it lacks is pending
/// (the node's release does not have it, or has not run it) and does not fail the build; the server owns each case's
/// feature mapping.
fn conformance_verdict(
    checks: &Checks,
    answer: NodeToolConformanceResult,
    found: &mut HarnessBuildCheck,
) -> HarnessBuildStatus {
    let expected: Vec<(&str, Option<&str>)> = checks.cases().collect();
    let mut cases = answer.cases;
    let sound = sound_native_report(checks, &cases);
    if !sound {
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
    let passed = sound && cases.iter().all(|case| case.passed);
    let pending: Vec<String> = expected
        .iter()
        .filter(|(name, _)| !cases.iter().any(|case| case.name == *name))
        .map(|(name, _)| case_words(checks, name))
        .collect();
    if sound && !pending.is_empty() {
        found.error = Some(format!(
            "Checks pending, {EARLIER_NODE_NOTE} (update the node to run them): {}",
            pending.join(", ")
        ));
    }
    found.cases = cases;
    if passed {
        HarnessBuildStatus::Checked
    } else {
        HarnessBuildStatus::Failed
    }
}

/// A case in the reader's English: the manifest's own title, else the catalogue's, else its name.
fn case_words(checks: &Checks, name: &str) -> String {
    checks
        .conformance
        .iter()
        .find(|case| case.name == name)
        .and_then(|case| case.title.as_ref())
        .or_else(|| {
            orch_agent_programs::catalogue::Catalogue::builtin()
                .cases
                .get(name)
        })
        .map_or_else(|| name.to_owned(), |title| title.en().to_owned())
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
    orch_engine::build_checks::host_for(&mut conn, &os, &arch).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::admin::{HarnessArch, HarnessBuildSource};
    use orch_api_contract::resources::MachineOs;
    use orch_link_contract::commands::NodeToolConformanceResult;
    use orch_types::Time;

    fn programs() -> Programs {
        Programs::builtin().clone()
    }

    fn case(name: &str, passed: bool) -> HarnessConformanceCase {
        HarnessConformanceCase {
            name: name.into(),
            passed,
            feature: None,
            unsupported: None,
            error: if passed { None } else { Some("no".into()) },
            elapsed_ms: 5,
        }
    }

    fn check_with(names: &[&str]) -> HarnessBuildCheck {
        HarnessBuildCheck {
            at: Time::new(chrono::Utc::now() - Duration::days(3)),
            machine: Some("h".into()),
            missing: vec![],
            off: vec![],
            error: None,
            stage: HarnessCheckStage::Complete,
            cases: names.iter().map(|n| case(n, true)).collect(),
        }
    }

    fn build(status: HarnessBuildStatus, check: Option<HarnessBuildCheck>) -> HarnessBuild {
        HarnessBuild {
            kind: "codex".into(),
            version: "0.170.0".into(),
            os: MachineOs::Linux,
            arch: HarnessArch::X86_64,
            url: "https://x".into(),
            sha256: "c".repeat(64),
            is_default: false,
            source: HarnessBuildSource::Channel,
            status,
            check,
            carry: None,
        }
    }

    fn all_but(without: &[&str]) -> Vec<String> {
        let p = programs();
        p.program("codex")
            .unwrap()
            .checks
            .cases()
            .map(|(n, _)| n.to_owned())
            .filter(|n| !without.contains(&n.as_str()))
            .collect()
    }

    #[test]
    fn grown_manifest_is_pending_not_untested() {
        let p = programs();
        let names = all_but(&["background_commands"]);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let b = build(HarnessBuildStatus::Checked, Some(check_with(&names)));
        assert!(!lacks_native_proof(&p, &b));
        assert_eq!(pending_cases(&p, &b), ["background_commands"]);
        assert!(takes_up_for_pending_cases(&p, &b, chrono::Utc::now(), 24));
        // whole: nothing pending, not taken up
        let whole = all_but(&[]);
        let whole: Vec<&str> = whole.iter().map(String::as_str).collect();
        let w = build(HarnessBuildStatus::Checked, Some(check_with(&whole)));
        assert!(pending_cases(&p, &w).is_empty());
        assert!(!takes_up_for_pending_cases(&p, &w, chrono::Utc::now(), 24));
    }

    #[test]
    fn interface_only_and_empty_are_still_untested() {
        let p = programs();
        let mut interface = check_with(&[]);
        interface.stage = HarnessCheckStage::Interface;
        assert!(lacks_native_proof(
            &p,
            &build(HarnessBuildStatus::Checked, Some(interface))
        ));
        assert!(lacks_native_proof(
            &p,
            &build(HarnessBuildStatus::Checked, Some(check_with(&[])))
        ));
        assert!(lacks_native_proof(
            &p,
            &build(HarnessBuildStatus::Checked, None)
        ));
        let mut release = build(HarnessBuildStatus::Checked, None);
        release.source = HarnessBuildSource::Release;
        assert!(!lacks_native_proof(&p, &release));
        assert!(!takes_up_for_pending_cases(
            &p,
            &release,
            chrono::Utc::now(),
            24
        ));
        // a failure inside a checked build is no proof
        let b = build(
            HarnessBuildStatus::Checked,
            Some({
                let mut c = check_with(&["startup"]);
                c.cases.push(case("user_input", false));
                c
            }),
        );
        assert!(lacks_native_proof(&p, &b));
    }

    #[test]
    fn an_earlier_node_leaves_pending_cases_and_no_failure() {
        let p = programs();
        let checks = &p.program("codex").unwrap().checks;
        let names = all_but(&["background_commands"]);
        let answer = NodeToolConformanceResult {
            cases: names.iter().map(|n| case(n, true)).collect(),
        };
        let mut found = check_with(&[]);
        found.cases.clear();
        let status = conformance_verdict(checks, answer, &mut found);
        assert_eq!(status, HarnessBuildStatus::Checked);
        assert!(found.cases.iter().all(|c| c.passed && c.name != "runner"));
        let note = found.error.clone().unwrap();
        assert!(note.contains("not run by this node's version"), "{note}");
        assert!(
            note.contains("Lists a command left running and ends it by name"),
            "{note}"
        );
        // taken up again within the interval? no: the same node would run the same cases
        let mut b = build(HarnessBuildStatus::Checked, Some(found.clone()));
        b.check.as_mut().unwrap().at = Time::new(chrono::Utc::now());
        assert!(!takes_up_for_pending_cases(&p, &b, chrono::Utc::now(), 24));
        // a day later it is
        assert!(takes_up_for_pending_cases(
            &p,
            &b,
            chrono::Utc::now() + Duration::hours(25),
            24
        ));
    }

    #[test]
    fn empty_repeated_and_self_contradicting_reports_still_fail() {
        let p = programs();
        let checks = &p.program("codex").unwrap().checks;
        let mut found = check_with(&[]);
        found.cases.clear();
        assert_eq!(
            conformance_verdict(
                checks,
                NodeToolConformanceResult { cases: vec![] },
                &mut found
            ),
            HarnessBuildStatus::Failed
        );
        assert_eq!(found.cases.len(), 1);
        assert!(found.cases[0].error.as_ref().unwrap().contains("omitted"));
        let mut again = check_with(&[]);
        again.cases.clear();
        let twice = NodeToolConformanceResult {
            cases: vec![case("startup", true), case("startup", true)],
        };
        assert_eq!(
            conformance_verdict(checks, twice, &mut again),
            HarnessBuildStatus::Failed
        );
        // a failed case fails, with its feature off
        let mut f = check_with(&[]);
        f.cases.clear();
        let one = NodeToolConformanceResult {
            cases: vec![case("user_input", false), case("startup", true)],
        };
        assert_eq!(
            conformance_verdict(checks, one, &mut f),
            HarnessBuildStatus::Failed
        );
        assert_eq!(f.off, ["approvals.user_input"]);
        // a program with no cases accepts an empty report
        let none = Checks::default();
        let mut n = check_with(&[]);
        n.cases.clear();
        assert_eq!(
            conformance_verdict(&none, NodeToolConformanceResult { cases: vec![] }, &mut n),
            HarnessBuildStatus::Checked
        );
    }

    #[test]
    fn a_run_that_found_nothing_is_not_a_finding() {
        let mut c = check_with(&[]);
        c.cases.clear();
        assert!(decided_nothing(HarnessBuildStatus::Untested, &c));
        let mut runner = c.clone();
        runner.cases.push(case("runner", false));
        runner.error = Some("the native check could not run on h: boom".into());
        assert!(decided_nothing(HarnessBuildStatus::Failed, &runner));
        let mut real = runner.clone();
        real.cases.push(case("user_input", false));
        assert!(!decided_nothing(HarnessBuildStatus::Failed, &real));
        let mut missing = c.clone();
        missing.missing = vec!["turn/start".into()];
        assert!(!decided_nothing(HarnessBuildStatus::Failed, &missing));
        let mut refused = c.clone();
        refused.error = Some("refused: the download does not match".into());
        refused.cases.push(case("runner", false));
        assert!(!decided_nothing(HarnessBuildStatus::Failed, &refused));
        assert!(!decided_nothing(HarnessBuildStatus::Checked, &c));
    }

    #[test]
    fn only_a_build_whose_cases_ran_is_taken_up_for_the_ones_it_lacks() {
        let p = programs();
        let now = chrono::Utc::now();
        // an interface that failed: no cases, never taken up (it would only fail again)
        let mut interface_failed = check_with(&[]);
        interface_failed.missing = vec!["turn/start".into()];
        assert!(!takes_up_for_pending_cases(
            &p,
            &build(HarnessBuildStatus::Failed, Some(interface_failed)),
            now,
            24
        ));
        // a runner that failed
        let mut runner = check_with(&[]);
        runner.cases.push(case("runner", false));
        assert!(!takes_up_for_pending_cases(
            &p,
            &build(HarnessBuildStatus::Failed, Some(runner)),
            now,
            24
        ));
        // some passed, one failed, others never run: taken up
        let mut some = check_with(&["startup"]);
        some.cases.push(case("user_input", false));
        assert!(takes_up_for_pending_cases(
            &p,
            &build(HarnessBuildStatus::Failed, Some(some)),
            now,
            24
        ));
        // not decided: untested and checking builds are the newest-first rule's, not this one's
        assert!(!takes_up_for_pending_cases(
            &p,
            &build(HarnessBuildStatus::Untested, Some(check_with(&["startup"]))),
            now,
            24
        ));
        assert!(!takes_up_for_pending_cases(
            &p,
            &build(HarnessBuildStatus::Checking, Some(check_with(&["startup"]))),
            now,
            24
        ));
    }
}
