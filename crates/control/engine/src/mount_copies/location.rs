//! Immutable native facts of a location-only copy; these facts grant no authority.

use crate::{EngineError, operation_scope::mount_locations::Location};
use orch_db::{
    Conn, DbError,
    queries::{environment_facts, environments, resources::mounts as rows},
};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};

pub mod operation;
pub mod preparation;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    id: Id,
    location: Location,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mount {
    id: Id,
    revision: i64,
    owner: Option<Id>,
    home: Option<Id>,
    org: Option<Id>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Storage {
    name: String,
    default: bool,
    capacity_gb: i64,
    private_views: bool,
}

fn storages(value: Json) -> Result<Vec<Storage>, EngineError> {
    let observed: Vec<orch_driver_contract::volumes::StorageInfo> = serde_json::from_value(value)?;
    let mut declarations = observed
        .into_iter()
        .map(|storage| Storage {
            name: storage.name,
            default: storage.default,
            capacity_gb: storage.capacity_gb,
            private_views: storage.private_views,
        })
        .collect::<Vec<_>>();
    declarations.sort_by(|a, b| {
        (&a.name, a.default, a.capacity_gb, a.private_views).cmp(&(
            &b.name,
            b.default,
            b.capacity_gb,
            b.private_views,
        ))
    });
    Ok(declarations)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Maker {
    machine: Id,
    owner: Option<Id>,
    host: Option<Id>,
    driver: String,
    storages: Vec<Storage>,
    declaration: Json,
    policy: Option<Json>,
}

impl Maker {
    async fn load(conn: &mut Conn, machine: Id) -> Result<Option<Self>, EngineError> {
        let Some(location) = environments::location_machine(conn, machine).await? else {
            return Ok(None);
        };
        let Some((driver, storage_facts)) = rows::data_driver(conn, machine).await? else {
            return Ok(None);
        };
        let direct: Option<Json> = sqlx::query_scalar(
            "SELECT facts->'environment_descriptor' FROM machines WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(machine)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?
        .flatten();
        // Match the same priority as data_driver: a direct data maker precedes node drivers.
        let declaration = if let Some(direct) = direct.filter(|value| {
            value["name"].as_str() == Some(driver.as_str())
                && matches!(
                    value["data_storage"]["made_by_orch"].as_str(),
                    Some("volume" | "directory")
                )
        }) {
            Some(direct)
        } else {
            rows::capabilities(conn, machine)
                .await?
                .and_then(|(drivers, _)| drivers.get(&driver).cloned())
        };
        let Some(declaration) = declaration else {
            return Ok(None);
        };
        let policy = environment_facts::machine(conn, machine, None)
            .await?
            .and_then(|facts| facts.policy);
        Ok(Some(Self {
            machine,
            owner: location.owner_id,
            host: location.host_id,
            driver,
            storages: storages(storage_facts)?,
            declaration,
            policy,
        }))
    }
}

async fn member(conn: &mut Conn, mount: Id, machine: Id) -> Result<Option<Member>, EngineError> {
    let Some(id) = sqlx::query_scalar(
        "SELECT membership_id FROM mount_locations WHERE mount_id=$1 AND machine_id=$2",
    )
    .bind(mount)
    .bind(machine)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?
    else {
        return Ok(None);
    };
    Ok(rows::locations(conn, &[mount])
        .await?
        .iter()
        .find(|row| row.machine_id == machine)
        .map(|row| Member {
            id,
            location: Location::from_row(row),
        }))
}

/// Exact original memberships and native makers, independent of request credentials and permission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    mount: Mount,
    source: Member,
    target: Option<Member>,
    reader: Maker,
    writer: Maker,
}

impl Review {
    /// Read only the native facts; the Copy source must separately prove both machine rights.
    pub async fn capture(
        conn: &mut Conn,
        mount: Id,
        source: Id,
        target: Id,
    ) -> Result<Self, EngineError> {
        Self::select(conn, mount, source, target)
            .await?
            .ok_or_else(|| {
                EngineError::unreadable("the mount copy has no current source or data maker")
            })
    }

    pub async fn select(
        conn: &mut Conn,
        mount: Id,
        source: Id,
        target: Id,
    ) -> Result<Option<Self>, EngineError> {
        if source == target {
            return Ok(None);
        }
        let Some(mount) = rows::mounts(conn, &[mount]).await?.pop() else {
            return Ok(None);
        };
        let Some(source) = member(conn, mount.id, source).await? else {
            return Ok(None);
        };
        let Some(reader) = Maker::load(conn, source.location.machine).await? else {
            return Ok(None);
        };
        let Some(writer) = Maker::load(conn, target).await? else {
            return Ok(None);
        };
        let target = member(conn, mount.id, target).await?;
        Ok(Some(Self {
            mount: Mount {
                id: mount.id,
                revision: mount.revision,
                owner: mount.owner_id,
                home: mount.home_id,
                org: mount.org_id,
            },
            source,
            target,
            reader,
            writer,
        }))
    }

    /// A comparison is factual only; callers still use a current full operation frame before effects.
    pub async fn current(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        Ok(Self::select(
            conn,
            self.mount.id,
            self.reader.machine,
            self.writer.machine,
        )
        .await?
        .as_ref()
            == Some(self))
    }

    pub fn source(&self) -> &Location {
        &self.source.location
    }

    pub fn target(&self) -> Option<&Location> {
        self.target.as_ref().map(|member| &member.location)
    }

    pub fn target_machine(&self) -> Id {
        self.writer.machine
    }

    pub fn source_driver(&self) -> &str {
        &self.reader.driver
    }

    pub fn target_driver(&self) -> &str {
        &self.writer.driver
    }
}
