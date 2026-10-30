//! The suite every environment driver passes: create, stop, start, recreate with volumes intact, sources, imports and reconciliation.
//!
//! A driver is driven through its contract's trait (`orch_driver_contract::driver::EnvironmentDriver`), so the
//! same cases serve a built-in driver and a program behind its plugin bridge. What a unit is made from is the
//! driver's own (`spec`, as its descriptor's schema says), so the suite is given a request that the driver can
//! make a unit from, and makes its unit with a name, a runtime id, volumes and the labels of a system of its own:
//! nothing of another system is touched, and `list` can be judged.
//!
//! | Rule | The driver |
//! |------|------------|
//! | `driver.descriptor` | declares a name, a version and a schema for a spec, consistently with itself |
//! | `driver.volumes` | makes the unit's volumes, and making one that is there changes nothing |
//! | `driver.create` | makes the unit the request names, and asked again by the same name answers the same unit |
//! | `driver.start` | runs it: `inspect` says `running` within a minute |
//! | `driver.stop` | stops it, giving it time to end by itself: `inspect` says `stopped` |
//! | `driver.start_again` | runs a stopped unit again |
//! | `driver.list` | lists it among the units of its system, and not among another system's |
//! | `driver.stats` | reports what it uses, or says that it does not (`unsupported`), never another error |
//! | `driver.network` | puts an open network in force, and refuses a restricted one it does not declare it can enforce |
//! | `driver.recreate` | removed and made again from the same request, the unit takes its volumes back and runs |
//! | `driver.remove` | removes it, keeping its volumes: `inspect` says `absent`, `list` names it no more, and a second removal is no error |
//!
//! Whether the files of a volume outlived the unit is not seen from here: the contract gives no window into a
//! unit. A driver's own tests prove it on what it makes (a container's volume, a directory), and this suite
//! proves that the same volumes are asked for and made again, and never removed with a unit. The volumes the
//! suite made are removed at its end.

use std::time::Duration;

use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::network::{EgressMode, NetworkPolicy};
use orch_driver_contract::runtime::{RuntimeSpec, VolumeRef, VolumeRole, unit_labels, unit_name};
use orch_driver_contract::units::{UnitRef, UnitStateKind};
use orch_driver_contract::volumes::VolumeRequest;
use orch_types::new_id;

use crate::report::{Checked, Report, skip};

/// How long a unit may take to come to the state it was sent to: a container whose image is on the host starts
/// in seconds, a microVM boots in well under a minute.
pub const SETTLE_WITHIN: Duration = Duration::from_secs(60);

/// How long one case may take in all: a unit made, started and checked, with room for a slow host.
pub const CASE_WITHIN: Duration = Duration::from_secs(300);

/// How often the suite looks again while it waits for a state.
const POLL_EVERY: Duration = Duration::from_millis(250);

/// How long a unit is given to end by itself when it is stopped: what the node gives one.
const GRACE: Duration = Duration::from_secs(10);

/// The request the suite makes its unit from: `base` with a fresh runtime id, the name a unit of that kind and
/// id has, the labels of `system`, and volumes named after the unit.
#[must_use]
pub fn unit_request(base: &RuntimeSpec, system: &str) -> RuntimeSpec {
    let mut request = base.clone();
    request.runtime_id = new_id();
    request.name = unit_name(request.kind, &request.runtime_id);
    request
        .labels
        .extend(unit_labels(system, request.kind, &request.runtime_id));
    let named = request
        .volumes
        .keys()
        .map(|role| {
            (
                *role,
                VolumeRef {
                    name: format!("{}-{}", request.name, role_word(*role)),
                    storage: None,
                    directory: None,
                },
            )
        })
        .collect();
    request.volumes = named;
    request
}

const fn role_word(role: VolumeRole) -> &'static str {
    match role {
        VolumeRole::Workspace => "workspace",
        VolumeRole::Home => "home",
        VolumeRole::State => "state",
    }
}

const fn state_word(state: UnitStateKind) -> &'static str {
    match state {
        UnitStateKind::Running => "running",
        UnitStateKind::Stopped => "stopped",
        UnitStateKind::Absent => "absent",
        UnitStateKind::Failed => "failed",
    }
}

const DESCRIPTOR_TITLE: &str =
    "It declares a name, a version and a schema for a spec, consistently with itself";
const VOLUMES_TITLE: &str =
    "It makes the unit's volumes, and making one that is there changes nothing";
const CREATE_TITLE: &str =
    "It makes the unit the request names, and asked again by that name answers the same unit";
const START_TITLE: &str = "It runs the unit: inspect says running within a minute";
const STOP_TITLE: &str = "It stops the unit, giving it time to end by itself";
const START_AGAIN_TITLE: &str = "It runs a stopped unit again";
const LIST_TITLE: &str =
    "It lists the unit among its system's units, and not among another system's";
const STATS_TITLE: &str = "It reports what the unit uses, or says that it does not";
const NETWORK_TITLE: &str = "It puts an open network in force, and refuses a restricted one it does not declare it enforces";
const RECREATE_TITLE: &str =
    "Removed and made again from the same request, the unit takes its volumes back and runs";
const REMOVE_TITLE: &str =
    "It removes the unit and keeps its volumes; a second removal is no error";

/// The rules that need the unit the suite made.
const WITH_UNIT: [(&str, &str); 8] = [
    ("driver.start", START_TITLE),
    ("driver.stop", STOP_TITLE),
    ("driver.start_again", START_AGAIN_TITLE),
    ("driver.list", LIST_TITLE),
    ("driver.stats", STATS_TITLE),
    ("driver.network", NETWORK_TITLE),
    ("driver.recreate", RECREATE_TITLE),
    ("driver.remove", REMOVE_TITLE),
];

/// Runs the driver's cases into `report`, with units made from `base` under the system `system`.
pub async fn run(
    driver: &dyn EnvironmentDriver,
    base: &RuntimeSpec,
    system: &str,
    report: &mut Report,
) {
    let descriptor = driver.describe();
    report.checked(
        "driver.descriptor",
        DESCRIPTOR_TITLE,
        descriptor.check().map_err(|faults| {
            faults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
                .into()
        }),
    );
    if !descriptor.makes_units_for(base.kind) {
        let why = format!(
            "it declares no spec for a unit of the kind {}",
            base.kind.as_str()
        );
        report.checked("driver.volumes", VOLUMES_TITLE, Err(skip(why.clone())));
        report.checked("driver.create", CREATE_TITLE, Err(skip(why.clone())));
        for (rule, title) in WITH_UNIT {
            report.checked(rule, title, Err(skip(why.clone())));
        }
        return;
    }
    let request = unit_request(base, system);
    let volumes: Vec<VolumeRequest> = request
        .volumes
        .values()
        .map(|volume| VolumeRequest {
            name: volume.name.clone(),
            storage: volume.storage.clone(),
            labels: request.labels.clone(),
        })
        .collect();
    report
        .run(
            "driver.volumes",
            VOLUMES_TITLE,
            CASE_WITHIN,
            ensure_volumes(driver, &volumes),
        )
        .await;
    let mut made = None;
    report
        .run(
            "driver.create",
            CREATE_TITLE,
            CASE_WITHIN,
            create(driver, &request, &mut made),
        )
        .await;
    if let Some(unit) = made {
        report
            .run(
                "driver.start",
                START_TITLE,
                CASE_WITHIN,
                start(driver, &unit),
            )
            .await;
        report
            .run("driver.stop", STOP_TITLE, CASE_WITHIN, stop(driver, &unit))
            .await;
        report
            .run(
                "driver.start_again",
                START_AGAIN_TITLE,
                CASE_WITHIN,
                start(driver, &unit),
            )
            .await;
        report
            .run(
                "driver.list",
                LIST_TITLE,
                CASE_WITHIN,
                listed(driver, system, &unit),
            )
            .await;
        report
            .run(
                "driver.stats",
                STATS_TITLE,
                CASE_WITHIN,
                stats(driver, &unit),
            )
            .await;
        let restricted = descriptor.network_egress.contains(&EgressMode::Restricted);
        report
            .run(
                "driver.network",
                NETWORK_TITLE,
                CASE_WITHIN,
                network(driver, &unit, restricted),
            )
            .await;
        report
            .run(
                "driver.recreate",
                RECREATE_TITLE,
                CASE_WITHIN,
                recreate(driver, &request, &volumes, &unit),
            )
            .await;
        report
            .run(
                "driver.remove",
                REMOVE_TITLE,
                CASE_WITHIN,
                remove(driver, system, &unit),
            )
            .await;
    } else {
        for (rule, title) in WITH_UNIT {
            report.checked(rule, title, Err(skip("not asked: no unit was made")));
        }
    }
    for volume in &volumes {
        // The suite's own volumes go; one that is already gone is no fault of the driver's.
        let _ = driver.remove_volume(&volume.name).await;
    }
}

async fn ensure_volumes(driver: &dyn EnvironmentDriver, volumes: &[VolumeRequest]) -> Checked {
    for volume in volumes {
        driver
            .ensure_volume(volume)
            .await
            .map_err(|error| format!("the volume {} was not made: {error}", volume.name))?;
        driver.ensure_volume(volume).await.map_err(|error| {
            format!(
                "asked again for the volume {}, which is there, it failed: {error}",
                volume.name
            )
        })?;
    }
    Ok(())
}

async fn create(
    driver: &dyn EnvironmentDriver,
    request: &RuntimeSpec,
    made: &mut Option<UnitRef>,
) -> Checked {
    let unit = driver
        .create(request)
        .await
        .map_err(|error| format!("create failed: {error}"))?;
    *made = Some(unit.clone());
    if unit.name != request.name {
        return Err(format!(
            "the unit is named {:?}, not {:?} as its request says",
            unit.name, request.name
        )
        .into());
    }
    if unit.runtime_id != request.runtime_id {
        return Err("the unit carries another runtime id than its request".into());
    }
    let again = driver
        .create(request)
        .await
        .map_err(|error| format!("asked again for the same name, create failed: {error}"))?;
    if again != unit {
        return Err("asked again for the same name, it answered another unit".into());
    }
    Ok(())
}

/// Waits until `inspect` says `wanted`, at most [`SETTLE_WITHIN`].
async fn settles(driver: &dyn EnvironmentDriver, unit: &UnitRef, wanted: UnitStateKind) -> Checked {
    let deadline = tokio::time::Instant::now() + SETTLE_WITHIN;
    loop {
        let state = driver
            .inspect(unit)
            .await
            .map_err(|error| format!("inspect failed: {error}"))?;
        if state.state == wanted {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            let detail = state
                .detail
                .map(|detail| format!(" ({detail})"))
                .unwrap_or_default();
            return Err(format!(
                "it was {}{detail} {SETTLE_WITHIN:?} later, not {}",
                state_word(state.state),
                state_word(wanted)
            )
            .into());
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

async fn start(driver: &dyn EnvironmentDriver, unit: &UnitRef) -> Checked {
    driver
        .start(unit)
        .await
        .map_err(|error| format!("start failed: {error}"))?;
    settles(driver, unit, UnitStateKind::Running).await
}

async fn stop(driver: &dyn EnvironmentDriver, unit: &UnitRef) -> Checked {
    driver
        .stop(unit, GRACE)
        .await
        .map_err(|error| format!("stop failed: {error}"))?;
    settles(driver, unit, UnitStateKind::Stopped).await
}

async fn listed(driver: &dyn EnvironmentDriver, system: &str, unit: &UnitRef) -> Checked {
    let units = driver
        .list(system)
        .await
        .map_err(|error| format!("list failed: {error}"))?;
    if !units.iter().any(|listed| listed.name == unit.name) {
        return Err("its system's list does not name the unit".into());
    }
    let elsewhere = format!("{system}-elsewhere");
    let others = driver
        .list(&elsewhere)
        .await
        .map_err(|error| format!("list of another system failed: {error}"))?;
    if others.iter().any(|listed| listed.name == unit.name) {
        return Err("another system's list names the unit".into());
    }
    Ok(())
}

async fn stats(driver: &dyn EnvironmentDriver, unit: &UnitRef) -> Checked {
    match driver.stats(unit).await {
        Ok(_) => Ok(()),
        Err(DriverError::Unsupported(_)) => {
            Err(skip("it says that it does not report resource use"))
        }
        Err(error) => Err(format!("stats failed with {}: {error}", error.code()).into()),
    }
}

async fn network(driver: &dyn EnvironmentDriver, unit: &UnitRef, restricted: bool) -> Checked {
    let open = NetworkPolicy::default();
    let in_force = driver
        .apply_network(unit, &open)
        .await
        .map_err(|error| format!("an open network was refused: {error}"))?;
    if in_force.mode != EgressMode::Open {
        return Err("asked for an open network, it put another in force".into());
    }
    if restricted {
        // Enforcing a restricted network needs the host's proxy and real traffic; the driver's own tests prove it.
        return Ok(());
    }
    let asked = NetworkPolicy {
        mode: EgressMode::Restricted,
        ..NetworkPolicy::default()
    };
    match driver.apply_network(unit, &asked).await {
        Err(DriverError::Unsupported(_)) => Ok(()),
        Err(error) => Err(format!(
            "it refused a restricted network with {} where unsupported was due: {error}",
            error.code()
        )
        .into()),
        Ok(_) => Err(
            "it accepted a restricted network that its descriptor does not say it can enforce"
                .into(),
        ),
    }
}

async fn recreate(
    driver: &dyn EnvironmentDriver,
    request: &RuntimeSpec,
    volumes: &[VolumeRequest],
    unit: &UnitRef,
) -> Checked {
    driver
        .stop(unit, GRACE)
        .await
        .map_err(|error| format!("stop failed: {error}"))?;
    driver
        .remove(unit)
        .await
        .map_err(|error| format!("remove failed: {error}"))?;
    settles(driver, unit, UnitStateKind::Absent).await?;
    for volume in volumes {
        driver.ensure_volume(volume).await.map_err(|error| {
            format!(
                "after the unit was removed, its volume {} could not be taken back: {error}",
                volume.name
            )
        })?;
    }
    let again = driver
        .create(request)
        .await
        .map_err(|error| format!("made again, create failed: {error}"))?;
    if again.name != unit.name {
        return Err("made again from the same request, the unit has another name".into());
    }
    start(driver, &again).await
}

async fn remove(driver: &dyn EnvironmentDriver, system: &str, unit: &UnitRef) -> Checked {
    driver
        .stop(unit, GRACE)
        .await
        .map_err(|error| format!("stop failed: {error}"))?;
    driver
        .remove(unit)
        .await
        .map_err(|error| format!("remove failed: {error}"))?;
    settles(driver, unit, UnitStateKind::Absent).await?;
    match driver.remove(unit).await {
        Ok(()) | Err(DriverError::NotFound(_)) => {}
        Err(error) => {
            return Err(format!("a second removal failed with {}: {error}", error.code()).into());
        }
    }
    let units = driver
        .list(system)
        .await
        .map_err(|error| format!("list failed: {error}"))?;
    if units.iter().any(|listed| listed.name == unit.name) {
        return Err("its system's list still names the removed unit".into());
    }
    Ok(())
}
