//! Installation-owned inventory joined across drivers and the verified tool cache.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use orch_driver_contract::resources::{
    Resource, ResourceInventory, ResourceKind, ResourceRef, ResourceUnavailable,
};
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_types::{Clock, Hash, Time};

use crate::drivers::{DriverHost, failure};
use crate::ledger::Ledger;
use crate::tools::ToolCache;

struct Located {
    shown: Resource,
    driver: Option<String>,
    system: String,
    native: ResourceRef,
}

fn opaque(driver: &str, system: &str, resource: &ResourceRef) -> ResourceRef {
    // JSON preserves component boundaries when a driver's identifiers contain punctuation.
    let key = serde_json::json!([driver, system, resource]).to_string();
    ResourceRef {
        kind: resource.kind,
        id: Hash::of(key.as_bytes()).to_string(),
    }
}

fn mapped(driver: &str, system: &str, resource: &ResourceRef) -> ResourceRef {
    if resource.kind == ResourceKind::ToolVersion {
        opaque("", "", resource)
    } else {
        opaque(driver, system, resource)
    }
}

fn dependencies(located: &[Located], roots: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    let by_id: BTreeMap<_, _> = located
        .iter()
        .map(|entry| (entry.shown.resource.id.as_str(), &entry.shown))
        .collect();
    let mut pending: Vec<_> = roots.into_iter().collect();
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(entry) = by_id.get(id.as_str()) {
            pending.extend(
                entry
                    .depends_on
                    .iter()
                    .map(|dependency| dependency.id.clone()),
            );
        }
    }
    seen
}

/// Pins a runtime's exact transitive occupancy before stop/removal can make its references vanish.
pub(crate) async fn runtime_occupancy(
    drivers: &DriverHost,
    tools: &ToolCache,
    ledger: &Ledger,
    runtime: orch_types::Id,
    at: Time,
) -> Result<BTreeSet<String>, CommandError> {
    let (located, _) = gather(drivers, tools, ledger).await;
    let runtime = runtime.to_string();
    let roots = located
        .iter()
        .filter(|entry| {
            entry
                .shown
                .labels
                .get(orch_driver_contract::runtime::LABEL_RUNTIME_ID)
                == Some(&runtime)
        })
        .map(|entry| entry.shown.resource.id.clone());
    let used = dependencies(&located, roots);
    ledger
        .resource_occupying(&used, at)
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
    Ok(used)
}

/// Records explicit verified cache use without interpreting the node's opaque identifier elsewhere.
pub(crate) fn tool_used(
    ledger: &Ledger,
    name: &str,
    version: &str,
    at: Time,
) -> Result<(), CommandError> {
    let resource = mapped(
        "",
        "",
        &ResourceRef {
            kind: ResourceKind::ToolVersion,
            id: format!("{name}/{version}"),
        },
    );
    ledger
        .resource_used(resource.id, at)
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))
}

/// Records the exact owned image a successful ensure made available.
pub(crate) async fn image_used(
    drivers: &DriverHost,
    tools: &ToolCache,
    ledger: &Ledger,
    driver: &str,
    digest: &str,
    at: Time,
) -> Result<(), CommandError> {
    let (located, _) = gather(drivers, tools, ledger).await;
    for entry in located.iter().filter(|entry| {
        entry.driver.as_deref() == Some(driver)
            && (entry.native.id == digest
                || entry.shown.aliases.iter().any(|alias| alias == digest))
    }) {
        ledger
            .resource_used(entry.shown.resource.id.clone(), at)
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
    }
    Ok(())
}

async fn gather(
    drivers: &DriverHost,
    tools: &ToolCache,
    ledger: &Ledger,
) -> (Vec<Located>, Vec<ResourceUnavailable>) {
    let mut all = Vec::new();
    let mut unavailable: Vec<_> = drivers
        .failures()
        .into_iter()
        .map(|(driver, reason)| ResourceUnavailable { driver, reason })
        .collect();
    if let Err(error) = tools.check_root() {
        unavailable.push(ResourceUnavailable {
            driver: "tools".into(),
            reason: error.to_string(),
        });
    }
    let mut systems = ledger.systems();
    let system = &drivers.settings().system_name;
    if !systems.contains(system) {
        systems.push(system.clone());
    }
    for (driver, host) in drivers.all() {
        for system in &systems {
            match host.contract().resources(system).await {
                Ok(resources) => {
                    for mut shown in resources {
                        let native = shown.resource.clone();
                        shown.resource = mapped(&driver, system, &native);
                        let owner =
                            (native.kind != ResourceKind::ToolVersion).then_some(driver.clone());
                        all.push(Located {
                            shown,
                            driver: owner,
                            system: system.clone(),
                            native,
                        });
                    }
                }
                Err(error) => unavailable.push(ResourceUnavailable {
                    driver: driver.clone(),
                    reason: error.to_string(),
                }),
            }
        }
    }
    for (name, version) in tools.list() {
        let native = ResourceRef {
            kind: ResourceKind::ToolVersion,
            id: format!("{name}/{version}"),
        };
        all.push(Located {
            shown: Resource {
                resource: mapped("", "", &native),
                name: format!("{name} {version}"),
                size_bytes: tools.size_of(&name, &version),
                labels: BTreeMap::from([("orch.system".into(), system.clone())]),
                depends_on: Vec::new(),
                aliases: Vec::new(),
                last_used_at: None,
                busy: false,
            },
            driver: None,
            system: system.clone(),
            native,
        });
    }
    // A unit can still use an artifact labelled with an earlier system name. Resolve native
    // dependencies after collecting every owned location so both records share the same opaque id.
    let identities: BTreeMap<_, _> = all
        .iter()
        .map(|entry| {
            (
                (entry.driver.clone(), entry.native.clone()),
                entry.shown.resource.clone(),
            )
        })
        .collect();
    for entry in &mut all {
        entry.shown.depends_on = entry
            .shown
            .depends_on
            .iter()
            .map(|reference| {
                if reference.kind == ResourceKind::ToolVersion {
                    mapped("", "", reference)
                } else {
                    identities
                        .get(&(entry.driver.clone(), reference.clone()))
                        .cloned()
                        .unwrap_or_else(|| {
                            mapped(
                                entry.driver.as_deref().unwrap_or_default(),
                                &entry.system,
                                reference,
                            )
                        })
                }
            })
            .collect();
    }
    let mut merged = BTreeMap::<ResourceRef, Located>::new();
    for entry in all {
        if let Some(held) = merged.get_mut(&entry.shown.resource) {
            held.shown.busy |= entry.shown.busy;
            held.shown.size_bytes = held.shown.size_bytes.or(entry.shown.size_bytes);
            held.shown.depends_on.extend(entry.shown.depends_on);
            held.shown.depends_on.sort();
            held.shown.depends_on.dedup();
        } else {
            merged.insert(entry.shown.resource.clone(), entry);
        }
    }
    (merged.into_values().collect(), unavailable)
}

/// Reads a local uninstall preview without advancing use dates or writing the node ledger.
pub async fn snapshot(
    drivers: &DriverHost,
    tools: &ToolCache,
    ledger: &Ledger,
    clock: &dyn Clock,
) -> ResourceInventory {
    let (located, unavailable) = gather(drivers, tools, ledger).await;
    ResourceInventory {
        observed_at: clock.time(),
        resources: located.into_iter().map(|entry| entry.shown).collect(),
        unavailable,
    }
}

/// Measures a directory without following links; failure leaves its size unknown.
pub fn directory_bytes(path: &Path) -> std::io::Result<i64> {
    let mut total = 0_i64;
    let mut pending = vec![path.to_owned()];
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        } else {
            total = total.saturating_add(i64::try_from(metadata.len()).unwrap_or(i64::MAX));
        }
    }
    Ok(total)
}

/// Joins current inventories and records observed cache use in the existing node ledger.
pub async fn list(
    drivers: &DriverHost,
    tools: &ToolCache,
    ledger: &Ledger,
    clock: &dyn Clock,
) -> Result<ResourceInventory, CommandError> {
    let (located, unavailable) = gather(drivers, tools, ledger).await;
    let mut resources: Vec<_> = located.into_iter().map(|entry| entry.shown).collect();
    let all = resources
        .iter()
        .map(|entry| entry.resource.id.clone())
        .collect();
    let used: BTreeSet<_> = resources
        .iter()
        .flat_map(|entry| entry.depends_on.iter().map(|id| id.id.clone()))
        .chain(
            resources
                .iter()
                .filter(|entry| entry.busy)
                .map(|entry| entry.resource.id.clone()),
        )
        .collect();
    let at = clock.time();
    // An unreadable driver might still use a tool. A partial view cannot advance or erase use facts.
    if unavailable.is_empty() {
        let uses = ledger
            .resource_uses(&all, &used, at)
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        for entry in &mut resources {
            entry.last_used_at = uses.get(&entry.resource.id).copied();
        }
    }
    Ok(ResourceInventory {
        observed_at: at,
        resources,
        unavailable,
    })
}

/// Resolves an opaque id afresh and refuses busy objects and dependencies before exact removal.
pub async fn remove(
    drivers: &DriverHost,
    tools: &ToolCache,
    ledger: &Ledger,
    clock: &dyn Clock,
    resource: &ResourceRef,
    expected_last_used_at: Option<Time>,
) -> Result<(), CommandError> {
    remove_with_runtime(
        drivers,
        tools,
        ledger,
        clock,
        resource,
        expected_last_used_at,
    )
    .await
    .map(|_| ())
}

/// Returns a locally recorded runtime only when its exact native unit was removed.
pub(crate) async fn remove_with_runtime(
    drivers: &DriverHost,
    tools: &ToolCache,
    ledger: &Ledger,
    clock: &dyn Clock,
    resource: &ResourceRef,
    expected_last_used_at: Option<Time>,
) -> Result<Option<orch_types::Id>, CommandError> {
    let (located, unavailable) = gather(drivers, tools, ledger).await;
    let selected = located
        .iter()
        .find(|entry| &entry.shown.resource == resource)
        .ok_or_else(|| {
            CommandError::new(
                ErrorCode::NotFound,
                "the object is no longer in this node's inventory",
            )
        })?;
    if selected.shown.busy
        || located
            .iter()
            .any(|entry| entry.shown.depends_on.contains(resource))
    {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "the object is in use",
        ));
    }
    if expected_last_used_at
        .is_some_and(|expected| ledger.resource_last_used(&resource.id) != Some(expected))
    {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "the object was used after the cleanup plan was made",
        ));
    }
    let occupied = if !selected.shown.depends_on.is_empty() {
        let occupied = dependencies(&located, [resource.id.clone()]);
        ledger
            .resource_occupying(&occupied, clock.time())
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        occupied
    } else {
        BTreeSet::new()
    };
    let runtime = (resource.kind == ResourceKind::Unit)
        .then(|| {
            selected
                .shown
                .labels
                .get(orch_driver_contract::runtime::LABEL_RUNTIME_ID)
                .and_then(|id| id.parse::<orch_types::Id>().ok())
                .and_then(|id| ledger.get(id))
                .filter(|record| {
                    Some(record.driver.as_str()) == selected.driver.as_deref()
                        && record.unit.reference == selected.native.id
                })
                .map(|record| record.runtime_id)
        })
        .flatten();
    if let Some(driver) = &selected.driver {
        let host = drivers.get(driver).ok_or_else(|| {
            CommandError::new(ErrorCode::OfflinePeer, "the object's driver is unavailable")
        })?;
        host.contract()
            .remove_resource(&selected.system, &selected.native)
            .await
            .map_err(|error| failure(&error))?;
        ledger
            .resource_occupancy_ended(&occupied, clock.time())
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        Ok(runtime)
    } else {
        if !unavailable.is_empty() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "an unreadable inventory may still use this tool",
            ));
        }
        let (name, version) = selected.native.id.split_once('/').ok_or_else(|| {
            CommandError::new(ErrorCode::Internal, "an inventoried tool has no version")
        })?;
        tools
            .remove_cached(name, version)
            .await
            .map_err(CommandError::from)?;
        Ok(None)
    }
}

/// Stops a selected owned unit through its driver's lifecycle seam, never by a process pattern.
pub async fn stop_for_purge(
    drivers: &DriverHost,
    tools: &ToolCache,
    ledger: &Ledger,
    resource: &ResourceRef,
) -> Result<(), CommandError> {
    if resource.kind != ResourceKind::Unit {
        return Err(failure(
            &orch_driver_contract::driver::DriverError::InvalidSpec(
                "only a unit can be stopped".into(),
            ),
        ));
    }
    let (located, _) = gather(drivers, tools, ledger).await;
    let selected = located
        .iter()
        .find(|entry| &entry.shown.resource == resource)
        .ok_or_else(|| CommandError::new(ErrorCode::NotFound, "the owned unit is absent"))?;
    if located
        .iter()
        .any(|entry| entry.shown.depends_on.contains(resource))
    {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "another object still needs this unit",
        ));
    }
    let driver = selected
        .driver
        .as_ref()
        .and_then(|driver| drivers.get(driver))
        .ok_or_else(|| {
            CommandError::new(ErrorCode::OfflinePeer, "the unit's driver is unavailable")
        })?;
    driver
        .contract()
        .stop_resource(
            &selected.system,
            &selected.native,
            drivers.settings().stop_grace,
        )
        .await
        .map_err(|error| failure(&error))
}

/// Removes only objects in a confirmed local plan, in dependency order, with one result per object.
pub async fn purge(
    drivers: &DriverHost,
    tools: &ToolCache,
    ledger: &Ledger,
    clock: &dyn Clock,
    selected: &[Resource],
) -> Vec<(ResourceRef, Result<(), CommandError>)> {
    purge_controlled(
        drivers,
        tools,
        ledger,
        clock,
        selected,
        &tokio_util::sync::CancellationToken::new(),
        |_, _| {},
    )
    .await
}

/// Reports each confirmed outcome immediately; cancellation stops between irreversible native operations.
pub async fn purge_controlled(
    drivers: &DriverHost,
    tools: &ToolCache,
    ledger: &Ledger,
    clock: &dyn Clock,
    selected: &[Resource],
    cancel: &tokio_util::sync::CancellationToken,
    mut report: impl FnMut(&ResourceRef, &Result<(), CommandError>) + Send,
) -> Vec<(ResourceRef, Result<(), CommandError>)> {
    let mut results = Vec::new();
    let mut pending: BTreeSet<_> = selected.iter().map(|item| item.resource.clone()).collect();
    let mut stopped = BTreeSet::new();
    for item in selected
        .iter()
        .filter(|item| item.resource.kind == ResourceKind::Unit && item.busy)
    {
        if cancel.is_cancelled() {
            break;
        }
        match stop_for_purge(drivers, tools, ledger, &item.resource).await {
            Ok(()) => {
                stopped.insert(item.resource.clone());
            }
            Err(error) => {
                let result = Err(error);
                report(&item.resource, &result);
                results.push((item.resource.clone(), result));
                pending.remove(&item.resource);
            }
        }
    }
    while !pending.is_empty() {
        if cancel.is_cancelled() {
            for resource in pending {
                let result = Err(CommandError {
                    code: "cancelled".into(),
                    detail: if stopped.contains(&resource) {
                        "the unit was stopped; its removal was cancelled"
                    } else {
                        "cleanup was cancelled before removing this object"
                    }
                    .into(),
                    retryable: false,
                    details: None,
                });
                report(&resource, &result);
                results.push((resource, result));
            }
            break;
        }
        let (located, unavailable) = gather(drivers, tools, ledger).await;
        let next = pending
            .iter()
            .find(|resource| {
                !located
                    .iter()
                    .any(|entry| entry.shown.depends_on.contains(resource))
            })
            .cloned();
        let Some(resource) = next else {
            for resource in pending {
                let result = Err(CommandError::new(
                    ErrorCode::Conflict,
                    "another object still depends on this object",
                ));
                report(&resource, &result);
                results.push((resource, result));
            }
            break;
        };
        pending.remove(&resource);
        let result = if located.iter().any(|entry| entry.shown.resource == resource) {
            remove(drivers, tools, ledger, clock, &resource, None).await
        } else if unavailable.is_empty() {
            Ok(())
        } else {
            Err(CommandError::new(
                ErrorCode::OfflinePeer,
                "the object's inventory is unavailable",
            ))
        };
        report(&resource, &result);
        results.push((resource, result));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occupancy_end_survives_restart_and_unused_observations_do_not_renew_it() {
        let data = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(data.path()).unwrap();
        let start = Time::new(
            chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        let end = Time::new(start.get() + chrono::Duration::days(60));
        let later = Time::new(end.get() + chrono::Duration::days(1));
        let all = BTreeSet::from(["image".to_owned(), "tool".to_owned()]);
        ledger.resource_uses(&all, &all, start).unwrap();
        drop(ledger);
        let ledger = Ledger::open(data.path()).unwrap();
        // Recovery first sees the unit gone: the cache must age from this observation of its end.
        assert_eq!(
            ledger.resource_uses(&all, &BTreeSet::new(), end).unwrap()["image"],
            end
        );
        assert_eq!(
            ledger.resource_uses(&all, &BTreeSet::new(), later).unwrap()["image"],
            end
        );
        ledger.resource_occupying(&all, later).unwrap();
        let ended = Time::new(later.get() + chrono::Duration::hours(1));
        ledger.resource_occupancy_ended(&all, ended).unwrap();
        drop(ledger);
        let ledger = Ledger::open(data.path()).unwrap();
        assert_eq!(ledger.resource_last_used("tool"), Some(ended));
        // An older diagnostic reply cannot move the exact fence backwards.
        ledger.resource_used("tool".into(), start).unwrap();
        assert_eq!(ledger.resource_last_used("tool"), Some(ended));
    }

    #[test]
    fn a_directory_measurement_counts_a_link_itself_and_never_reads_its_target() {
        let owned = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(owned.path().join("file"), b"four").unwrap();
        std::fs::write(outside.path().join("secret"), vec![0; 4096]).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), owned.path().join("link")).unwrap();
            let link_bytes = std::fs::symlink_metadata(owned.path().join("link"))
                .unwrap()
                .len();
            assert_eq!(
                directory_bytes(owned.path()).unwrap(),
                4 + i64::try_from(link_bytes).unwrap()
            );
        }
    }
}
