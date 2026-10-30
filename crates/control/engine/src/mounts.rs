//! Mounts resolved for an environment from its home node and its own configuration, in read-only, shared or private mode; changes applied or marked stale.
//!
//! One resolution is used by connected attaches, managed unit definitions and change plans.
//! Private views that the target cannot make become read-only, never shared. The underlying
//! data stays on its machine; only the environment's own layer is persistent environment data.

use crate::error::EngineError;
use chrono::{DateTime, Utc};
use orch_api_contract::mounts::{EnvMount, EnvMountReason, EnvMountState, MountMode, MountRef};
use orch_authz::resources;
use orch_db::Conn;
use orch_db::queries::environments::hosting as environments;
use orch_db::queries::resources::mounts as rows;
use orch_driver_contract::mounts::{check_target, check_targets};
use orch_link_contract::commands::{AttachMode, EnvAttachMount};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::HashMap;

mod live;

/// A mount's setting value, supplying node, writer and distance from the environment.
type EnabledMountValue = (Json, Option<Id>, Option<Id>, usize);

/// One resolved mount, with the public result and the machine-only request kept apart.
#[derive(Debug, Clone)]
pub struct ResolvedMount {
    /// The requested attachment and its mode on the chosen machine.
    pub view: EnvMount,
    /// The managed driver's request; absent for unavailable and connected mounts.
    pub spec: Option<Json>,
    /// The connected machine's request; absent where there is no usable location.
    pub attach: Option<EnvAttachMount>,
    /// The persistent private-view layer, when the target can hold it.
    pub layer: Option<Json>,
}

/// A prospective environment and the machine that will hold it.
pub struct Target<'a> {
    /// The environment, used to name its persistent private layers.
    pub environment: Id,
    /// Its settings home.
    pub home: Id,
    /// Its own settings.
    pub own: &'a Json,
    /// Its first owner, the author of its initial own settings.
    pub owner: Id,
    /// The node or connected machine that holds the data.
    pub machine: Option<Id>,
    /// The managed driver, or none for a connected machine.
    pub driver: Option<&'a str>,
    /// The environment's workspace path.
    pub workspace: &'a str,
    /// Its home directory, where known.
    pub home_path: Option<&'a str>,
    /// The storage chosen for workspace and private layers.
    pub storage: Option<String>,
    /// Locations a copy makes or fills on the machine, read in place of what the machine has for those mounts.
    pub planned: &'a [rows::LocationRow],
    /// Mounts a plan detaches there, though the machine may have a location for them.
    pub detached: &'a [Id],
}

/// Reads the mounts an existing environment is given on its current machine.
pub async fn resolve(
    conn: &mut Conn,
    environment: Id,
    at: DateTime<Utc>,
) -> Result<Vec<ResolvedMount>, EngineError> {
    let env = environments::environment(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("the environment is not there"))?;
    let runtime = environments::live_runtime(conn, environment).await?;
    let machine = match &runtime {
        Some(runtime) => orch_db::queries::resources::hosting::host(conn, runtime.host_id)
            .await?
            .map(|host| host.machine_id),
        None => env.machine_id,
    };
    let storage: Option<Json> =
        orch_settings::get(conn, Place::Environment(environment), keys::ENV_STORAGE).await?;
    resolve_for(
        conn,
        &Target {
            environment,
            home: env.home_node_id,
            own: &env.config,
            owner: env.owner_id,
            machine,
            driver: runtime.as_ref().map(|runtime| runtime.driver.as_str()),
            workspace: &env.workspace_root,
            home_path: env.facts.get("home").and_then(Json::as_str),
            storage: storage
                .as_ref()
                .and_then(|s| s.get("workspace"))
                .and_then(Json::as_str)
                .map(str::to_owned),
            planned: &[],
            detached: &[],
        },
        at,
    )
    .await
}

/// The machine whose locations an existing environment's mounts come from: its unit's host, or the machine it is
/// attached on.
pub async fn location_machine(conn: &mut Conn, environment: Id) -> Result<Option<Id>, EngineError> {
    let env = environments::environment(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("the environment is not there"))?;
    Ok(match environments::live_runtime(conn, environment).await? {
        Some(runtime) => orch_db::queries::resources::hosting::host(conn, runtime.host_id)
            .await?
            .map(|host| host.machine_id),
        None => env.machine_id,
    })
}

/// Resolves mounts for a prospective environment, keeping origin order and checking each setting's writer.
pub async fn resolve_for(
    conn: &mut Conn,
    target: &Target<'_>,
    at: DateTime<Utc>,
) -> Result<Vec<ResolvedMount>, EngineError> {
    // A carried layer has its own retained name, which later live changes and recreates must reuse.
    let retained = if target.environment.is_nil() {
        None
    } else {
        let runtime = environments::live_runtime(conn, target.environment).await?;
        match runtime {
            Some(runtime) if target.driver == Some(runtime.driver.as_str()) => {
                let host =
                    orch_db::queries::resources::hosting::host(conn, runtime.host_id).await?;
                host.filter(|host| Some(host.machine_id) == target.machine)
                    .map(|_| runtime.volumes)
            }
            _ => None,
        }
    };
    let def = registry::get(keys::ENV_MOUNTS)
        .ok_or_else(|| EngineError::unreadable("the mount setting is not registered"))?;
    let snapshot = Snapshot::load(conn, Place::Node(target.home), Wanted::Keys(&[def])).await?;
    let inherited = snapshot.resolve(def);
    let chain: Vec<Id> = snapshot.chain().iter().map(|link| link.id).collect();
    let written = rows::settings(conn, &chain).await?;
    let nodes = orch_db::queries::hierarchy::with_ancestors(conn, &[target.home]).await?;
    // Parts already exclude values below a lock. Keep the nearest source of each map member.
    let mut enabled: HashMap<Id, EnabledMountValue> = HashMap::new();
    for (depth, part) in inherited.parts.iter().enumerate() {
        let writer = part.source.node.and_then(|node| {
            written
                .iter()
                .find(|(id, _, _)| *id == node)
                .and_then(|(_, _, by)| *by)
        });
        add_values(&mut enabled, &part.value, part.source.node, writer, depth);
    }
    if inherited.locked_by.is_none()
        && let Some(own) = target.own.get(keys::ENV_MOUNTS)
    {
        let writer = rows::configuration_writer(conn, target.environment)
            .await?
            .unwrap_or(target.owner);
        add_values(&mut enabled, own, None, Some(writer), inherited.parts.len());
    }
    let mut enabled: Vec<_> = enabled
        .into_iter()
        .filter(|(_, (value, _, _, _))| *value != Json::Bool(false))
        .collect();
    enabled.sort_by(|(a, (_, _, _, da)), (b, (_, _, _, db))| db.cmp(da).then(a.cmp(b)));
    enabled.retain(|(id, _)| !target.detached.contains(id));
    let ids: Vec<Id> = enabled.iter().map(|(id, _)| *id).collect();
    let mounts = rows::mounts(conn, &ids).await?;
    let mut locations = rows::locations(conn, &ids).await?;
    // A copy's location replaces what the machine has for that mount before anything reads it.
    locations.retain(|location| {
        !target.planned.iter().any(|planned| {
            planned.mount_id == location.mount_id && planned.machine_id == location.machine_id
        })
    });
    locations.extend(
        target
            .planned
            .iter()
            .filter(|planned| ids.contains(&planned.mount_id))
            .cloned(),
    );
    let addresses: HashMap<_, _> = resources::addresses(conn, &ids)
        .await?
        .into_iter()
        .collect();
    let capabilities = match target.machine {
        Some(machine) => rows::capabilities(conn, machine).await?,
        None => None,
    };
    let described = match target.machine {
        Some(machine) => {
            orch_db::queries::environment_facts::on_machine(conn, machine, target.driver).await?
        }
        None => None,
    };
    let descriptor = described.as_ref();
    let placement = descriptor
        .and_then(|d| d.get("mounts"))
        .and_then(Json::as_str)
        .unwrap_or("none");
    let private_views = descriptor
        .and_then(|d| d.get("private_views"))
        .and_then(Json::as_bool)
        .unwrap_or(false)
        && capabilities.as_ref().is_some_and(|(_, storages)| {
            storages.as_array().is_some_and(|storages| {
                storages.iter().any(|storage| {
                    let chosen = target.storage.as_deref().map_or_else(
                        || storage.get("default").and_then(Json::as_bool) == Some(true),
                        |name| storage.get("name").and_then(Json::as_str) == Some(name),
                    );
                    chosen && storage.get("private_views").and_then(Json::as_bool) == Some(true)
                })
            })
        });
    let ordered: Vec<_> = enabled
        .iter()
        .filter_map(|(id, _)| mounts.iter().find(|row| row.id == *id))
        .filter(|row| row.enabled)
        .collect();
    let targets: Vec<_> = ordered
        .iter()
        .map(|row| (row.name.as_str(), row.target.as_str()))
        .collect();
    let mut reserved = vec![target.workspace, "/run/orch"];
    if let Some(home) = target.home_path {
        reserved.push(home);
    }
    let conflicts = check_targets(&targets, &reserved);
    let mut result = Vec::new();
    for (id, (value, from, writer, _)) in enabled {
        let Some(row) = mounts.iter().find(|row| row.id == id && row.enabled) else {
            continue;
        };
        let asked = asked_mode(&value, row.read_only)?;
        let mode = if asked == MountMode::Private && !private_views {
            MountMode::ReadOnly
        } else {
            asked
        };
        let location = locations.iter().find(|location| {
            location.mount_id == id && Some(location.machine_id) == target.machine
        });
        let mut reason = if placement == "none" {
            Some(EnvMountReason::Driver)
        } else if location.is_none() {
            Some(EnvMountReason::NoLocation)
        } else if conflicts.iter().any(|(name, _)| *name == row.name) {
            Some(EnvMountReason::TargetConflict)
        } else {
            None
        };
        if !crate::mcp::source_may_use(
            conn,
            writer.unwrap_or(target.owner),
            id,
            from.unwrap_or(target.home),
            Time::new(at),
        )
        .await?
        {
            reason = Some(EnvMountReason::Refused);
        }
        let path = location.and_then(|location| {
            if placement == "placed" {
                Some(row.target.clone())
            } else if location.kind == "path" {
                Some(location.source.clone())
            } else {
                None
            }
        });
        if placement == "in_place"
            && path.as_deref().is_some_and(|path| {
                check_target(path).is_err()
                    || reserved.iter().any(|reserved| overlaps(path, reserved))
            })
        {
            reason = Some(EnvMountReason::TargetConflict);
        }
        let layer = (reason.is_none() && mode == MountMode::Private).then(|| retained.as_ref().and_then(|volumes| volumes.get(format!("over:{id}"))).cloned().unwrap_or_else(|| json!({"name": format!("orch-over-{}-{}", target.environment, id), "storage": target.storage})));
        let spec = location
            .filter(|_| reason.is_none() && target.driver.is_some())
            .map(|location| {
                json!({
                    "name": row.name, "kind": location.kind, "source": location.source,
                    "target": row.target, "mode": asked, "layer": layer,
                })
            });
        let attach = location
            .filter(|_| {
                reason.is_none()
                    && descriptor
                        .and_then(|descriptor| descriptor.get("lifecycle"))
                        .and_then(Json::as_str)
                        == Some("attach")
            })
            .map(|location| EnvAttachMount {
                volume: (location.kind == "volume").then(|| {
                    orch_driver_contract::runtime::VolumeRef {
                        name: location.source.clone(),
                        storage: location.storage.clone(),
                        directory: None,
                    }
                }),
                mount_id: id,
                name: row.name.clone(),
                source: location.source.clone(),
                mode: if mode == MountMode::Shared {
                    AttachMode::Shared
                } else {
                    AttachMode::ReadOnly
                },
            });
        result.push(ResolvedMount {
            view: EnvMount {
                mount: MountRef {
                    id,
                    name: row.name.clone(),
                    address: addresses.get(&id).cloned().unwrap_or_default(),
                },
                state: if reason.is_some() {
                    EnvMountState::Unavailable
                } else {
                    EnvMountState::Pending
                },
                reason,
                path: if reason.is_some() { None } else { path },
                mode,
                asked,
                enforced: placement == "placed",
                from: from
                    .and_then(|id| nodes.iter().find(|node| node.id == id))
                    .map(|node| {
                        Ok::<_, EngineError>(orch_api_contract::nodes::NodeRef {
                            thread_display: None,
                            id: node.id,
                            kind: serde_json::from_value(json!(node.kind))?,
                            name: None,
                            redacted: true,
                        })
                    })
                    .transpose()?,
            },
            spec,
            attach,
            layer,
        });
    }
    Ok(result)
}

/// Resolves the visible attachment list, adding the machine's observed result when it has one.
pub async fn views(
    conn: &mut Conn,
    environment: Id,
    at: DateTime<Utc>,
) -> Result<Vec<EnvMount>, EngineError> {
    let environment_row = environments::environment(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("the environment is not there"))?;
    let facts = environment_row.facts.get("mounts").and_then(Json::as_array);
    let mut result = Vec::new();
    for mount in resolve(conn, environment, at).await? {
        let mut view = mount.view;
        if let Some(fact) = facts.and_then(|facts| {
            facts.iter().find(|fact| {
                fact.get("mount_id") == Some(&json!(view.mount.id))
                    || fact.get("name").and_then(Json::as_str) == Some(view.mount.name.as_str())
            })
        }) && view.reason.is_none()
        {
            if let Some(state) = fact.get("state") {
                view.state = serde_json::from_value(state.clone())?;
            }
            if let Some(reason) = fact.get("reason") {
                view.reason = serde_json::from_value(reason.clone())?;
            }
            view.path = fact.get("path").and_then(Json::as_str).map(str::to_owned);
            if let Some(mode) = fact.get("mode").filter(|value| !value.is_null()) {
                view.mode = serde_json::from_value(mode.clone())?;
            }
            view.enforced = fact
                .get("enforced")
                .and_then(Json::as_bool)
                .unwrap_or(false);
        }
        result.push(view);
    }
    Ok(result)
}

fn add_values(
    enabled: &mut HashMap<Id, EnabledMountValue>,
    value: &Json,
    from: Option<Id>,
    writer: Option<Id>,
    depth: usize,
) {
    if let Some(values) = value.as_object() {
        for (id, value) in values {
            if let Ok(id) = id.parse() {
                enabled.insert(id, (value.clone(), from, writer, depth));
            }
        }
    }
}

fn overlaps(a: &str, b: &str) -> bool {
    a == b
        || a.strip_prefix(b).is_some_and(|tail| tail.starts_with('/'))
        || b.strip_prefix(a).is_some_and(|tail| tail.starts_with('/'))
}

/// Reads a scope's mount mode, with the mount's own read-only rule taking precedence.
pub fn asked_mode(value: &Json, read_only: bool) -> Result<MountMode, EngineError> {
    match (value.as_str(), value.as_bool()) {
        (Some("private"), _) => Ok(MountMode::Private),
        (Some("read_only"), _) | (_, Some(true))
            if read_only || value.as_str() == Some("read_only") =>
        {
            Ok(MountMode::ReadOnly)
        }
        (_, Some(true)) => Ok(MountMode::Shared),
        _ => Err(EngineError::unreadable(
            "a mount setting has no attachment mode",
        )),
    }
}

/// What a plan says of one mount beyond where it is attached: its fate, its default, what may be chosen, and the
/// copy when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fated {
    /// The mount.
    pub mount_id: Id,
    /// What becomes of it.
    pub fate: orch_api_contract::mounts::MountFate,
    /// What its manager has plans preselect.
    pub default_fate: orch_api_contract::mounts::MountFate,
    /// What may be chosen for it in this plan.
    pub fates: Vec<orch_api_contract::mounts::MountFate>,
    /// Why its data cannot be copied to the target.
    pub copy_unavailable: Option<orch_api_contract::forks::CopyUnavailable>,
    /// The copy, when its fate is copy.
    pub copy: Option<orch_api_contract::forks::PlanMountCopy>,
    /// What the data of its source location is.
    pub data_kind: Option<orch_api_contract::mounts::MountDataKind>,
    /// Whether the target is an attached read-only synced location with a readable source.
    pub fingerprints_available: bool,
    /// The explicitly requested comparison of those locations.
    pub fingerprints: Option<orch_api_contract::forks::PlanMountFingerprints>,
    /// The retained background comparison and its live job.
    pub fingerprint_measurement: Option<orch_api_contract::forks::Measurement>,
}

impl Fated {
    /// A mount that stays as it is: the result is in an environment used as it stands.
    #[must_use]
    pub fn unchanged(mount_id: Id, default_fate: orch_api_contract::mounts::MountFate) -> Self {
        use orch_api_contract::mounts::MountFate;
        Self {
            mount_id,
            fate: MountFate::AsIs,
            default_fate,
            fates: vec![MountFate::AsIs],
            copy_unavailable: Some(orch_api_contract::forks::CopyUnavailable::Unchanged),
            copy: None,
            data_kind: None,
            fingerprints_available: false,
            fingerprints: None,
            fingerprint_measurement: None,
        }
    }
}

/// Compares the mounts of two environments before a fork, switch or move is confirmed. `to` is resolved with the
/// plan's copies and detachments; a mount the plan detaches is said to be detached because it was asked.
#[must_use]
pub fn plan(
    from: &[EnvMount],
    to: &[EnvMount],
    fated: &[Fated],
) -> Vec<orch_api_contract::forks::PlanMount> {
    use orch_api_contract::forks::{MountAction, MountDetachedBecause, PlanMount};
    use orch_api_contract::mounts::MountFate;
    let mut ids: Vec<Id> = from.iter().chain(to).map(|mount| mount.mount.id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| {
            let source = from.iter().find(|mount| mount.mount.id == id);
            let target = to.iter().find(|mount| mount.mount.id == id);
            let reference = target.or(source)?;
            let fated = fated
                .iter()
                .find(|fated| fated.mount_id == id)
                .cloned()
                .unwrap_or_else(|| Fated::unchanged(id, MountFate::AsIs));
            let asked_off = fated.fate == MountFate::Detach;
            let attached = target.filter(|mount| mount.reason.is_none() && !asked_off);
            Some(PlanMount {
                mount: reference.mount.clone(),
                action: match (source, attached) {
                    (_, None) => MountAction::Detach,
                    (None, Some(_)) => MountAction::Attach,
                    _ => MountAction::Keep,
                },
                path: attached.and_then(|mount| mount.path.clone()),
                mode: attached.map(|mount| mount.mode),
                detached_because: if attached.is_some() {
                    None
                } else if asked_off {
                    Some(MountDetachedBecause::Asked)
                } else {
                    Some(match target.and_then(|mount| mount.reason) {
                        Some(EnvMountReason::Driver) => MountDetachedBecause::Driver,
                        Some(
                            EnvMountReason::Refused
                            | EnvMountReason::Missing
                            | EnvMountReason::TargetConflict,
                        ) => MountDetachedBecause::Refused,
                        _ => MountDetachedBecause::NoLocation,
                    })
                },
                fate: fated.fate,
                default_fate: fated.default_fate,
                fates: fated.fates,
                copy_unavailable: fated.copy_unavailable,
                copy: fated.copy,
                data_kind: fated.data_kind,
                fingerprints_available: fated.fingerprints_available,
                fingerprints: fated.fingerprints,
                fingerprint_measurement: fated.fingerprint_measurement,
            })
        })
        .collect()
}

/// Lists which private changes can travel, independently of the underlying mount's data.
#[must_use]
pub fn private_plan(
    from: &[EnvMount],
    to: &[EnvMount],
    carry: bool,
    detached: &[Id],
) -> Vec<orch_api_contract::forks::PrivateViewPlan> {
    use orch_api_contract::forks::{PrivateViewLeftBecause as Why, PrivateViewPlan};
    from.iter()
        .filter(|mount| mount.mode == MountMode::Private && mount.reason.is_none())
        .map(|source| {
            let target = to.iter().find(|mount| mount.mount.id == source.mount.id);
            let left_because = match target {
                None if detached.contains(&source.mount.id) => Some(Why::Asked),
                None => Some(Why::NoLocation),
                Some(mount) if mount.reason.is_some() => Some(Why::NoLocation),
                Some(mount)
                    if mount.asked == MountMode::Private && mount.mode != MountMode::Private =>
                {
                    Some(Why::NoPrivateViews)
                }
                Some(mount) if mount.mode != MountMode::Private => Some(Why::OtherMode),
                _ if !carry => Some(Why::Asked),
                _ => None,
            };
            PrivateViewPlan {
                measurement: None,
                measured: false,
                mount: source.mount.clone(),
                file_count: 0,
                total_bytes: 0,
                carried: left_because.is_none(),
                left_because,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_can_tighten_a_mount_and_a_private_view_never_writes_through() {
        assert_eq!(asked_mode(&json!(true), true).unwrap(), MountMode::ReadOnly);
        assert_eq!(asked_mode(&json!(true), false).unwrap(), MountMode::Shared);
        assert_eq!(
            asked_mode(&json!("read_only"), false).unwrap(),
            MountMode::ReadOnly
        );
        assert_eq!(
            asked_mode(&json!("private"), true).unwrap(),
            MountMode::Private
        );
        assert!(asked_mode(&json!("shared"), true).is_err());
    }

    fn mount(id: Id, mode: MountMode, reason: Option<EnvMountReason>) -> EnvMount {
        EnvMount {
            mount: MountRef {
                id,
                name: "data".into(),
                address: "team.data".into(),
            },
            state: EnvMountState::Attached,
            reason,
            path: Some("/mnt/data".into()),
            mode,
            asked: mode,
            enforced: true,
            from: None,
        }
    }

    #[test]
    fn a_plan_says_the_target_mode_and_why_a_mount_or_its_private_changes_stay_behind() {
        use orch_api_contract::forks::{MountAction, MountDetachedBecause, PrivateViewLeftBecause};
        let id = orch_types::new_id();
        let source = vec![mount(id, MountMode::Private, None)];
        let read_only = vec![mount(id, MountMode::ReadOnly, None)];
        let plan = plan(&source, &read_only, &[]);
        assert_eq!(plan[0].action, MountAction::Keep);
        assert_eq!(plan[0].mode, Some(MountMode::ReadOnly));
        assert_eq!(
            private_plan(&source, &read_only, true, &[])[0].left_because,
            Some(PrivateViewLeftBecause::OtherMode)
        );
        let detached = super::plan(
            &source,
            &[mount(
                id,
                MountMode::Private,
                Some(EnvMountReason::NoLocation),
            )],
            &[],
        );
        assert_eq!(detached[0].action, MountAction::Detach);
        assert_eq!(detached[0].mode, None);
        assert_eq!(
            detached[0].detached_because,
            Some(MountDetachedBecause::NoLocation)
        );
        assert!(private_plan(&source, &source, true, &[])[0].carried);
        assert_eq!(
            private_plan(&source, &source, false, &[])[0].left_because,
            Some(PrivateViewLeftBecause::Asked)
        );
    }

    #[test]
    fn a_mount_detached_on_request_says_so_and_its_private_changes_stay_behind() {
        use orch_api_contract::forks::{MountAction, MountDetachedBecause, PrivateViewLeftBecause};
        use orch_api_contract::mounts::MountFate;
        let id = orch_types::new_id();
        let source = vec![mount(id, MountMode::Private, None)];
        let mut fated = Fated::unchanged(id, MountFate::AsIs);
        fated.fate = MountFate::Detach;
        fated.fates = vec![MountFate::Detach, MountFate::AsIs];
        let plan = plan(&source, &[], std::slice::from_ref(&fated));
        assert_eq!(plan[0].action, MountAction::Detach);
        assert_eq!(plan[0].detached_because, Some(MountDetachedBecause::Asked));
        assert_eq!(plan[0].fate, MountFate::Detach);
        assert_eq!(plan[0].default_fate, MountFate::AsIs);
        assert_eq!(
            private_plan(&source, &[], true, &[id])[0].left_because,
            Some(PrivateViewLeftBecause::Asked)
        );
        let kept = super::plan(&source, &source, &[Fated::unchanged(id, MountFate::Copy)]);
        assert_eq!(kept[0].action, MountAction::Keep);
        assert_eq!(
            kept[0].fates,
            vec![MountFate::AsIs],
            "a target used as it stands keeps it as it is"
        );
        assert_eq!(kept[0].default_fate, MountFate::Copy);
    }
}
