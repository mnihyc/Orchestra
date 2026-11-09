//! Current native Host cascade facts and ordinary prerequisites.

use super::*;
use crate::key_operations::{native, targets};
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::host_delete_sources::Source;
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn objects(
    conn: &mut Conn,
    source: Source,
    objects: impl IntoIterator<Item = (ObjectKind, Id)>,
) -> ApiResult<native::Facts<Source>> {
    let mut groups = BTreeMap::new();
    for (kind, id) in objects.into_iter().collect::<BTreeSet<_>>() {
        let target = match kind {
            ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread => {
                targets::node_identity(conn, id).await?
            }
            ObjectKind::Machine => targets::machine_subject(conn, id).await?,
            _ => targets::resource_identity(conn, ObjectRef { kind, id }).await?,
        };
        groups
            .entry(KeyTargetKind::Object(kind))
            .or_insert_with(Vec::new)
            .push(target);
    }
    Ok(native::Facts::collection(
        source,
        groups.into_iter().collect(),
    ))
}
pub(super) async fn host_fact(conn: &mut Conn, host: Id) -> ApiResult<native::Facts<Source>> {
    Ok(native::Facts::one(
        Source::Host,
        KeyTargetKind::Object(ObjectKind::Host),
        targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host,
            },
        )
        .await?,
    ))
}
pub(super) async fn footprint(conn: &mut Conn, stored: &Stored) -> ApiResult<footprint::Footprint> {
    let mut value = footprint::of(stored);
    for machine in &value.copy_targets {
        match hosts::host_of_machine(conn, *machine).await? {
            Some(host) => {
                value.used.insert((ObjectKind::Host, host));
            }
            None => {
                value.used.insert((ObjectKind::Machine, *machine));
            }
        }
    }
    Ok(value)
}
pub(super) async fn required(
    conn: &mut Conn,
    stored: &Stored,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let footprint = footprint(conn, stored).await?;
    Ok(vec![
        host_fact(conn, stored.host).await?,
        objects(conn, Source::Managed, footprint.managed).await?,
        objects(conn, Source::Used, footprint.used).await?,
    ])
}
pub(super) fn permission(kind: ObjectKind, used: bool) -> ApiResult<Permission> {
    let value = match (used, kind) {
        (false, ObjectKind::Environment) => Permission::EnvironmentManage,
        (false, ObjectKind::Provider) => Permission::ProviderManage,
        (true, ObjectKind::Host) => Permission::HostUse,
        (true, ObjectKind::Template) => Permission::TemplateUse,
        (true, ObjectKind::Machine) => Permission::MachineUse,
        (true, ObjectKind::Mount) => Permission::MountUse,
        _ => {
            return Err(ApiError::internal(
                "the native cascade supplied an unsupported authority role",
            ));
        }
    };
    Ok(value)
}
pub(super) async fn require(
    conn: &mut Conn,
    principal: &Principal,
    stored: &Stored,
) -> ApiResult<()> {
    operation_access::review(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Host,
            id: stored.host,
        },
        Permission::HostManage,
    )
    .await
    .map_err(refused)?;
    let footprint = footprint(conn, stored).await?;
    for (used, objects) in [(false, footprint.managed), (true, footprint.used)] {
        for (kind, id) in objects {
            operation_access::review(
                conn,
                principal,
                ObjectRef { kind, id },
                permission(kind, used)?,
            )
            .await
            .map_err(refused)?;
        }
    }
    if let Some(data) = &stored.data {
        for mount in data.custody.mounts.iter().filter(|mount| {
            matches!(
                mount.fate,
                api::HostDataFate::Move | api::HostDataFate::Delete
            )
        }) {
            crate::routes::resources::mounts::locating(conn, principal, &mount.location).await?;
        }
    }
    Ok(())
}

/// Names are optional disclosure. The mixed native result's source set is explicit and selected.
pub(super) async fn content(conn: &mut Conn, stored: &Stored) -> ApiResult<native::Facts<Source>> {
    let footprint = footprint(conn, stored).await?;
    let mut selected = footprint.managed;
    selected.extend(footprint.used);
    selected.insert((ObjectKind::Host, stored.host));
    selected.extend(
        stored
            .read_environments
            .iter()
            .map(|id| (ObjectKind::Environment, *id)),
    );
    if let Some(data) = &stored.data {
        for mount in &data.custody.mounts {
            selected.insert((ObjectKind::Mount, mount.location.mount_id));
        }
    }
    let mut names = BTreeSet::new();
    for (kind, id) in &selected {
        let target = match kind {
            ObjectKind::Machine => targets::machine_subject(conn, *id).await?,
            _ => {
                targets::resource_identity(
                    conn,
                    ObjectRef {
                        kind: *kind,
                        id: *id,
                    },
                )
                .await?
            }
        };
        names.extend(
            target
                .node_ancestors
                .into_iter()
                .filter_map(|parent| parent.id.map(|id| (parent.kind, id))),
        );
    }
    for (kind, id) in names {
        if let KeyTargetKind::Object(kind) = kind {
            selected.insert((kind, id));
        }
    }
    objects(conn, Source::Content, selected).await
}
