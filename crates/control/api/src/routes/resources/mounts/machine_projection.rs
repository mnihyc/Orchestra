//! Native machine snapshot exposure selection, before its ordinary wire projection.

use super::*;
use std::collections::HashSet;

struct Location {
    mount: rows::MachineMountData,
    kind: api::MountLocationKind,
    remove: bool,
    size: Option<u64>,
    environments: Vec<rows::MachineMountEnvironment>,
}
struct Choice {
    mount: rows::MachineMountData,
    path: bool,
    default_storage: bool,
    storages: Vec<String>,
}
pub(super) struct Mounts {
    pub host: Option<Id>,
    locations: Vec<Location>,
    choices: Vec<Choice>,
    pub addresses: BTreeSet<Id>,
}

impl Mounts {
    pub fn mounts(&self) -> BTreeSet<Id> {
        self.locations
            .iter()
            .map(|row| row.mount.id)
            .chain(self.choices.iter().map(|row| row.mount.id))
            .collect()
    }
    pub fn environments(&self) -> BTreeSet<Id> {
        self.locations
            .iter()
            .flat_map(|row| row.environments.iter().map(|environment| environment.id))
            .collect()
    }
    pub fn preserved_by(&self, fresh: &Self) -> bool {
        let locations: BTreeMap<_, _> = fresh
            .locations
            .iter()
            .map(|row| (row.mount.id, row))
            .collect();
        let choices: BTreeMap<_, _> = fresh
            .choices
            .iter()
            .map(|row| (row.mount.id, row))
            .collect();
        self.host == fresh.host
            && self.addresses.is_subset(&fresh.addresses)
            && self.locations.iter().all(|old| {
                locations.get(&old.mount.id).is_some_and(|new| {
                    let environments: BTreeSet<_> =
                        new.environments.iter().map(|row| row.id).collect();
                    old.mount.id == new.mount.id
                        && old.mount.kind == new.mount.kind
                        && old.mount.source == new.mount.source
                        && old.mount.storage == new.mount.storage
                        && (!old.remove || new.remove)
                        && old
                            .environments
                            .iter()
                            .all(|old| environments.contains(&old.id))
                })
            })
            && self.choices.iter().all(|old| {
                choices.get(&old.mount.id).is_some_and(|new| {
                    let storages: BTreeSet<_> = new.storages.iter().cloned().collect();
                    old.mount.id == new.mount.id
                        && (!old.path || new.path)
                        && (!old.default_storage || new.default_storage)
                        && old
                            .storages
                            .iter()
                            .all(|storage| storages.contains(storage))
                })
            })
    }
    pub fn render(
        &self,
        addresses: &std::collections::HashMap<Id, String>,
    ) -> ApiResult<api::MachineMounts> {
        let reference = |row: &rows::MachineMountData| MountRef {
            id: row.id,
            name: row.name.clone(),
            address: addresses.get(&row.id).cloned().unwrap_or_default(),
        };
        let locations = self
            .locations
            .iter()
            .map(|row| {
                Ok(api::MachineMount {
                    mount: reference(&row.mount),
                    target: row.mount.target.clone(),
                    kind: row.kind,
                    source: row.remove.then(|| row.mount.source.clone()).flatten(),
                    storage: row.mount.storage.clone(),
                    data_kind: row.mount.data_kind.as_deref().map(parse).transpose()?,
                    size_bytes: row.size,
                    environments: row
                        .environments
                        .iter()
                        .map(|row| {
                            Ok(api::MountEnvironment {
                                id: row.id,
                                name: row.name.clone(),
                                status: parse(&row.status)?,
                            })
                        })
                        .collect::<ApiResult<_>>()?,
                    can_remove: row.remove,
                })
            })
            .collect::<ApiResult<_>>()?;
        let available = self
            .choices
            .iter()
            .map(|row| api::MachineMountChoice {
                mount: reference(&row.mount),
                path: row.path,
                default_storage: row.default_storage,
                storages: row.storages.clone(),
            })
            .collect();
        Ok(api::MachineMounts {
            grouping: Vec::new(),
            locations,
            available,
        })
    }
}

/// The original native location/choice rules, additionally narrowed before selecting sources.
pub(super) fn mounts(
    snapshot: rows::MachineMountSnapshot,
    holdings: &BTreeMap<Id, resources::Effective>,
    manages: bool,
    permitted_mounts: &HashSet<Id>,
    permitted_environments: &HashSet<Id>,
) -> ApiResult<Mounts> {
    let storages = snapshot.storages.as_array().cloned().unwrap_or_default();
    let default = storages
        .iter()
        .find(|storage| storage.get("default").and_then(serde_json::Value::as_bool) == Some(true))
        .and_then(|storage| storage.get("name"))
        .and_then(serde_json::Value::as_str);
    let allows_storage = |storage: Option<&str>| {
        manages
            && snapshot.makes_data
            && storage.is_none_or(|name| {
                storages.iter().any(|storage| {
                    storage.get("name").and_then(serde_json::Value::as_str) == Some(name)
                })
            })
    };
    let visible: BTreeSet<_> = snapshot
        .mounts
        .0
        .iter()
        .filter(|row| {
            permitted_mounts.contains(&row.id)
                && (has(holdings.get(&row.id), Permission::MountView)
                    || has(holdings.get(&row.id), Permission::MountManage))
        })
        .map(|row| row.id)
        .collect();
    let mut attached: BTreeMap<Id, Vec<rows::MachineMountEnvironment>> = BTreeMap::new();
    for environment in &snapshot.environments.0 {
        if !permitted_environments.contains(&environment.id)
            || !has(holdings.get(&environment.id), Permission::EnvironmentView)
        {
            continue;
        }
        let ids: BTreeSet<Id> = environment
            .mounts
            .as_array()
            .into_iter()
            .flatten()
            .filter(|fact| {
                fact.get("state").and_then(serde_json::Value::as_str) == Some("attached")
            })
            .filter_map(|fact| {
                fact.get("mount_id")
                    .and_then(serde_json::Value::as_str)?
                    .parse()
                    .ok()
            })
            .collect();
        for id in ids {
            attached.entry(id).or_default().push(environment.clone());
        }
    }
    let reports: Vec<orch_link_contract::events::NodeMountSize> =
        serde_json::from_value(snapshot.sizes).unwrap_or_default();
    let sizes: BTreeMap<_, _> = reports
        .into_iter()
        .map(|report| {
            let kind = match report.kind {
                orch_link_contract::events::NodeMountKind::Path => "path".to_owned(),
                orch_link_contract::events::NodeMountKind::Volume => "volume".to_owned(),
            };
            (
                (report.mount_id, kind, report.source, report.storage),
                report.bytes,
            )
        })
        .collect();
    let mut locations = Vec::new();
    let mut choices = Vec::new();
    for mount in snapshot.mounts.0 {
        if !permitted_mounts.contains(&mount.id) {
            continue;
        }
        let holding = holdings.get(&mount.id);
        let path = manages && has(holding, Permission::MountUse);
        if let Some(kind) = &mount.kind {
            if !has(holding, Permission::MountView) {
                continue;
            }
            let kind: api::MountLocationKind = parse(kind)?;
            let size = snapshot.host_id.and_then(|_| {
                sizes
                    .get(&(
                        mount.id,
                        match kind {
                            api::MountLocationKind::Path => "path".into(),
                            api::MountLocationKind::Volume => "volume".into(),
                        },
                        mount.source.clone().unwrap_or_default(),
                        mount.storage.clone(),
                    ))
                    .copied()
            });
            let environments = attached.remove(&mount.id).unwrap_or_default();
            locations.push(Location {
                mount,
                kind,
                remove: path,
                size,
                environments,
            });
        } else if path {
            let names: Vec<_> = storages
                .iter()
                .filter_map(|storage| storage.get("name").and_then(serde_json::Value::as_str))
                .filter(|name| allows_storage(Some(name)))
                .map(str::to_owned)
                .collect();
            let default_storage = allows_storage(default);
            if path || default_storage || !names.is_empty() {
                choices.push(Choice {
                    mount,
                    path,
                    default_storage,
                    storages: names,
                });
            }
        }
    }
    let selected: BTreeSet<_> = locations
        .iter()
        .map(|row| row.mount.id)
        .chain(choices.iter().map(|row| row.mount.id))
        .collect();
    Ok(Mounts {
        host: snapshot.host_id,
        locations,
        choices,
        addresses: visible.intersection(&selected).copied().collect(),
    })
}

struct Use {
    environment: rows::EnvironmentDataUse,
    role: api::EnvironmentDataRole,
    path: String,
}
struct Root {
    path: String,
    uses: Vec<Use>,
}
pub(super) struct Directories {
    pub host: Option<Id>,
    machine: Id,
    roots: Vec<Root>,
    direct: Option<orch_driver_contract::descriptor::EnvironmentDataStorage>,
    drivers: BTreeMap<String, orch_driver_contract::descriptor::EnvironmentDataStorage>,
}
impl Directories {
    pub fn environments(&self) -> BTreeSet<Id> {
        self.roots
            .iter()
            .flat_map(|root| root.uses.iter().map(|row| row.environment.id))
            .collect()
    }
    fn uses(&self) -> HashSet<(String, Id, u8, String)> {
        self.roots
            .iter()
            .flat_map(|root| {
                root.uses.iter().map(|row| {
                    (
                        root.path.clone(),
                        row.environment.id,
                        match row.role {
                            api::EnvironmentDataRole::Workspace => 0,
                            api::EnvironmentDataRole::Home => 1,
                        },
                        row.path.clone(),
                    )
                })
            })
            .collect()
    }
    pub fn preserved_by(&self, fresh: &Self) -> bool {
        self.machine == fresh.machine
            && self.host == fresh.host
            && self.uses().is_subset(&fresh.uses())
    }
    pub fn render(&self) -> ApiResult<api::MachineEnvironmentData> {
        let roots = self
            .roots
            .iter()
            .map(|root| {
                Ok(api::EnvironmentDirectoryRoot {
                    path: root.path.clone(),
                    uses: root
                        .uses
                        .iter()
                        .map(|row| {
                            Ok(api::EnvironmentDirectoryUse {
                                environment: api::MountEnvironment {
                                    id: row.environment.id,
                                    name: row.environment.name.clone(),
                                    status: parse(&row.environment.status)?,
                                },
                                role: row.role,
                                path: row.path.clone(),
                            })
                        })
                        .collect::<ApiResult<_>>()?,
                })
            })
            .collect::<ApiResult<_>>()?;
        Ok(api::MachineEnvironmentData {
            machine_id: self.machine,
            roots,
            direct: self.direct.clone(),
            drivers: self.drivers.clone(),
        })
    }
}

/// The original native directory matching and retained role facts, after selected Environment scope.
pub(super) fn directories(
    machine: Id,
    snapshot: rows::EnvironmentDataSnapshot,
    holdings: &BTreeMap<Id, resources::Effective>,
    permitted: &HashSet<Id>,
) -> ApiResult<Directories> {
    let paths: Vec<String> = serde_json::from_value(snapshot.roots).map_err(ApiError::internal)?;
    let mut roots: Vec<_> = paths
        .into_iter()
        .map(|path| Root {
            path,
            uses: Vec::new(),
        })
        .collect();
    for environment in snapshot.environments.0 {
        if !permitted.contains(&environment.id)
            || !has(holdings.get(&environment.id), Permission::EnvironmentView)
        {
            continue;
        }
        for (role, path) in [
            (api::EnvironmentDataRole::Workspace, &environment.workspace),
            (api::EnvironmentDataRole::Home, &environment.home),
        ] {
            let Some(path) = path else {
                continue;
            };
            for root in &mut roots {
                if std::path::Path::new(path).starts_with(&root.path) {
                    root.uses.push(Use {
                        environment: environment.clone(),
                        role,
                        path: path.clone(),
                    });
                }
            }
        }
    }
    let direct = serde_json::from_value(snapshot.direct).map_err(ApiError::internal)?;
    let drivers = snapshot
        .drivers
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, facts)| {
            let value = facts
                .get("data_storage")
                .cloned()
                .map_or_else(|| Ok(Default::default()), serde_json::from_value)
                .map_err(ApiError::internal)?;
            Ok((name.clone(), value))
        })
        .collect::<ApiResult<_>>()?;
    Ok(Directories {
        host: snapshot.host_id,
        machine,
        roots,
        direct,
        drivers,
    })
}
