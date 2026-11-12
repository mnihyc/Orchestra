//! Actual mount, machine, predecessor and native data-maker for one location effect.
use super::{
    continuing::{self, Error, Source},
    sources::Facts,
    targets,
};
use crate::{EngineError, commands::Outgoing};
use futures::future::BoxFuture;
use orch_access_contract::{key_scope::KeyTargetKind, permissions::ObjectKind};
use orch_api_contract::{
    mount_location_sources::{self as declarations, Source as Role},
    mounts::MountLocationSet,
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{
    Caller, CapturedOperation, Principal,
    resources::{Locating, LocationAsk, LocationKind},
    usable::Policies,
};
use orch_db::{
    Conn, DbError,
    queries::{
        environments,
        resources::mounts::{self as rows, LocationRow},
    },
};
use orch_link_contract::commands::{CommandKind, NodeVolumeEnsure, SerialKey};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

/// A native location key retains its actual predecessor, never a replacement lookup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub mount: Id,
    pub machine: Id,
    pub host: Option<Id>,
    pub kind: String,
    pub source: String,
    pub storage: Option<String>,
    pub data_kind: Option<String>,
    pub by: Id,
    pub at: Time,
}
impl Location {
    pub fn from_row(row: &LocationRow) -> Self {
        Self {
            mount: row.mount_id,
            machine: row.machine_id,
            host: row.host_id,
            kind: row.kind.clone(),
            source: row.source.clone(),
            storage: row.storage.clone(),
            data_kind: row.data_kind.clone(),
            by: row.created_by,
            at: Time::new(row.created_at),
        }
    }
    pub fn matches(&self, row: &LocationRow) -> bool {
        let fresh = Self::from_row(row);
        self.mount == fresh.mount
            && self.machine == fresh.machine
            && self.host == fresh.host
            && self.kind == fresh.kind
            && self.source == fresh.source
            && self.storage == fresh.storage
            && self.by == fresh.by
            && self.at.get().timestamp_micros() == fresh.at.get().timestamp_micros()
    }
    pub fn row(&self) -> LocationRow {
        LocationRow {
            mount_id: self.mount,
            machine_id: self.machine,
            host_id: self.host,
            host_name: None,
            kind: self.kind.clone(),
            source: self.source.clone(),
            storage: self.storage.clone(),
            data_kind: self.data_kind.clone(),
            created_by: self.by,
            created_at: self.at.get(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Physical {
    owner: Option<Id>,
    home: Option<Id>,
    org: Option<Id>,
    machine_owner: Option<Id>,
    host: Option<Id>,
    driver: Option<(String, Json)>,
    policy: Option<Json>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetSource {
    origin: CapturedOperation,
    location: Location,
    previous: Option<Location>,
    physical: Physical,
}
impl std::fmt::Debug for SetSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SetSource([private])")
    }
}
async fn physical(conn: &mut Conn, location: &Location) -> Result<Physical, EngineError> {
    let mount = rows::mounts(conn, &[location.mount])
        .await?
        .pop()
        .ok_or_else(|| EngineError::unreadable("the mount disappeared"))?;
    let machine = environments::location_machine(conn, location.machine)
        .await?
        .ok_or_else(|| EngineError::unreadable("the location machine disappeared"))?;
    let policy = orch_db::queries::environment_facts::machine(conn, location.machine, None)
        .await?
        .and_then(|facts| facts.policy);
    Ok(Physical {
        owner: mount.owner_id,
        home: mount.home_id,
        org: mount.org_id,
        machine_owner: machine.owner_id,
        host: machine.host_id,
        driver: if location.kind == "volume" {
            rows::data_driver(conn, location.machine).await?
        } else {
            None
        },
        policy,
    })
}
pub struct Set;
impl Source for Set {
    type Role = Role;
    type Input = SetSource;
    type Value = Location;
    const ROUTE: Route = MountLocationSet::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::SET;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a SetSource,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Location, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            if physical(conn, &input.location).await? != input.physical {
                return Err(Error::Refused);
            }
            let selected = rows::locations(conn, &[input.location.mount])
                .await?
                .into_iter()
                .find(|row| row.machine_id == input.location.machine);
            if match (&input.previous, selected.as_ref()) {
                (None, None) => false,
                (Some(old), Some(fresh)) => !old.matches(fresh),
                _ => true,
            } {
                return Err(Error::Refused);
            }
            let mount = targets::resource_identity(
                conn,
                orch_access_contract::decisions::ObjectRef {
                    kind: ObjectKind::Mount,
                    id: input.location.mount,
                },
            )
            .await
            .map_err(EngineError::unreadable)?;
            let machine = targets::machine_subject(conn, input.location.machine)
                .await
                .map_err(EngineError::unreadable)?;
            Ok((
                input.location.clone(),
                vec![
                    Facts::one(Role::Mount, KeyTargetKind::Object(ObjectKind::Mount), mount),
                    Facts::one(
                        Role::Machine,
                        KeyTargetKind::Object(ObjectKind::Machine),
                        machine,
                    ),
                ],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a SetSource,
        _value: &'a Location,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let ask = LocationAsk {
                mount: input.location.mount,
                machine: input.location.machine,
                kind: if input.location.kind == "volume" {
                    LocationKind::Volume
                } else {
                    LocationKind::Path
                },
                storage: input.location.storage.clone(),
            };
            if !matches!(
                orch_authz::resources::may_add_location(
                    conn,
                    principal,
                    &ask,
                    &Policies::default()
                )
                .await?,
                Locating::Allowed
            ) {
                return Err(Error::Refused);
            }
            if input.location.kind == "path" {
                let policy: orch_link_contract::policy::LocalPolicy =
                    serde_json::from_value(input.physical.policy.clone().ok_or(Error::Refused)?)
                        .map_err(EngineError::from)?;
                policy
                    .check_root(std::path::Path::new(&input.location.source))
                    .map_err(|_| Error::Refused)?;
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a SetSource,
        _value: &'a Location,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { input.lock_source(conn).await.map_err(Error::from) })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a SetSource,
        old: &'a Location,
        fresh: &'a Location,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old == fresh) })
    }
}
impl SetSource {
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        location: &LocationRow,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some("MountLocationSet")
            || location.created_by != caller.user()
        {
            return Err(EngineError::unreadable(
                "the mount location changed its operation",
            ));
        }
        let location = Location::from_row(location);
        let previous = rows::locations(conn, &[location.mount])
            .await?
            .iter()
            .find(|row| row.machine_id == location.machine)
            .map(Location::from_row);
        let physical = physical(conn, &location).await?;
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            location,
            previous,
            physical,
        })
    }
    pub fn can_record(&self, current: Option<&LocationRow>) -> bool {
        if current.is_some_and(|row| self.location.matches(row)) {
            return false;
        }
        match (&self.previous, current) {
            (None, None) => true,
            (Some(old), Some(row)) => old.matches(row),
            _ => false,
        }
    }
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    pub fn location(&self) -> &Location {
        &self.location
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        sqlx::query("SELECT id FROM mounts WHERE id=$1 FOR SHARE")
            .bind(self.location.mount)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
            .bind(self.location.machine)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        sqlx::query("SELECT id FROM hosts WHERE machine_id=$1 FOR SHARE")
            .bind(self.location.machine)
            .fetch_all(conn)
            .await
            .map_err(DbError::from)?;
        Ok(())
    }
    pub fn command(
        &self,
        command: &Outgoing,
    ) -> Result<super::http_command_source::CommandEvidence, EngineError> {
        let body: NodeVolumeEnsure = serde_json::from_value(command.body.clone())?;
        if command.kind != CommandKind::NodeVolumeEnsure
            || command.machine != self.location.machine
            || command.key != SerialKey::Volume(self.location.source.clone())
            || self.location.kind != "volume"
            || body.name != self.location.source
            || body.storage != self.location.storage
            || body.labels
                != std::collections::BTreeMap::from([(
                    "orch.mount_id".to_owned(),
                    self.location.mount.to_string(),
                )])
            || self
                .physical
                .driver
                .as_ref()
                .is_none_or(|(driver, _)| *driver != body.driver)
        {
            return Err(EngineError::unreadable(
                "the native volume command changed its location source",
            ));
        }
        Ok(super::http_command_source::CommandEvidence::MountEnsure(
            self.clone(),
        ))
    }
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match continuing::prepare::<Set>(conn, clock, &self.origin, self.clone()).await {
            Ok(frame) => Ok(Some(vec![(Some(frame.principal().caller().clone()), None)])),
            Err(Error::Refused | Error::Binding(_)) => Ok(None),
            Err(Error::Native(error)) => Err(error),
        }
    }
}

/// Actual native retained source: active association or the exact displaced-data row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Removal {
    Active,
    Kept(Id),
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveSource {
    origin: CapturedOperation,
    location: Location,
    physical: Physical,
    removal: Removal,
    confirmed: Option<String>,
}
impl std::fmt::Debug for RemoveSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoveSource([private])")
    }
}
pub struct Delete;
pub struct Purge;
async fn removal_location(
    conn: &mut Conn,
    mount: Id,
    machine: Id,
    removal: &Removal,
) -> Result<Option<LocationRow>, EngineError> {
    match removal {
        Removal::Active => Ok(rows::locations(conn, &[mount])
            .await?
            .into_iter()
            .find(|row| row.machine_id == machine)),
        Removal::Kept(id) => Ok(rows::kept(conn, &[mount])
            .await?
            .into_iter()
            .find(|row| row.id == *id && row.machine_id == machine)
            .map(|row| LocationRow {
                mount_id: mount,
                machine_id: row.machine_id,
                host_id: None,
                host_name: None,
                kind: row.kind,
                source: row.source,
                storage: row.storage,
                data_kind: row.data_kind,
                created_by: row.kept_by,
                created_at: row.kept_at,
            })),
    }
}
/// Existing active-volume safety predicate, shared by request and later native send.
pub async fn running_uses(conn: &mut Conn, mount: Id, machine: Id) -> Result<Vec<Id>, EngineError> {
    Ok(sqlx::query_scalar("SELECT DISTINCT e.id FROM environments e LEFT JOIN runtimes r ON r.environment_id=e.id AND r.state<>'removed' LEFT JOIN hosts h ON h.id=r.host_id WHERE (r.id IS NOT NULL OR (e.deleted_at IS NULL AND e.status IN ('running','starting','offline','failed'))) AND (e.machine_id=$2 OR h.machine_id=$2) AND (EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(e.facts->'mounts','[]')) f WHERE f->>'mount_id'=$1::text AND f->>'state'='attached') OR e.config->'env.mounts' ? $1::text)").bind(mount).bind(machine).fetch_all(conn).await.map_err(DbError::from)?)
}
async fn load_remove(
    conn: &mut Conn,
    input: &RemoveSource,
) -> Result<(Location, Vec<Facts<Role>>), Error> {
    if physical(conn, &input.location).await? != input.physical
        || !input.matches_current(conn).await?
    {
        return Err(Error::Refused);
    }
    let mount = rows::mounts(conn, &[input.location.mount])
        .await?
        .pop()
        .ok_or(Error::Refused)?;
    if input.location.kind == "volume" && input.confirmed.as_deref() != Some(mount.name.as_str()) {
        return Err(Error::Refused);
    }
    let mount = targets::resource_identity(
        conn,
        orch_access_contract::decisions::ObjectRef {
            kind: ObjectKind::Mount,
            id: input.location.mount,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    let machine = targets::machine_subject(conn, input.location.machine)
        .await
        .map_err(EngineError::unreadable)?;
    Ok((
        input.location.clone(),
        vec![
            Facts::one(Role::Mount, KeyTargetKind::Object(ObjectKind::Mount), mount),
            Facts::one(
                Role::Machine,
                KeyTargetKind::Object(ObjectKind::Machine),
                machine,
            ),
        ],
    ))
}
async fn require_remove(
    conn: &mut Conn,
    principal: &Principal,
    input: &RemoveSource,
) -> Result<(), Error> {
    let ask = LocationAsk {
        mount: input.location.mount,
        machine: input.location.machine,
        kind: if input.location.kind == "volume" {
            LocationKind::Volume
        } else {
            LocationKind::Path
        },
        storage: input.location.storage.clone(),
    };
    if !matches!(
        orch_authz::resources::may_add_location(conn, principal, &ask, &Policies::default())
            .await?,
        Locating::Allowed
    ) {
        return Err(Error::Refused);
    }
    if input.location.kind == "volume"
        && input.removal == Removal::Active
        && !running_uses(conn, input.location.mount, input.location.machine)
            .await?
            .is_empty()
    {
        return Err(Error::Refused);
    }
    Ok(())
}
macro_rules! removal_source {
    ($name:ident,$route:path,$row:ident) => {
        impl Source for $name {
            type Role = Role;
            type Input = RemoveSource;
            type Value = Location;
            const ROUTE: Route = <$route>::ROUTE;
            const DECLARATION: &'static Declaration<Role> = &declarations::$row;
            fn prepare<'a>(
                conn: &'a mut Conn,
                _caller: &'a Caller,
                input: &'a RemoveSource,
                _now: Time,
            ) -> BoxFuture<'a, Result<(Location, Vec<Facts<Role>>), Error>> {
                Box::pin(async move {
                    if input.operation() != Self::ROUTE.name {
                        return Err(Error::Refused);
                    }
                    load_remove(conn, input).await
                })
            }
            fn require<'a>(
                conn: &'a mut Conn,
                principal: &'a Principal,
                input: &'a RemoveSource,
                _value: &'a Location,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(require_remove(conn, principal, input))
            }
            fn locks<'a>(
                conn: &'a mut Conn,
                _principal: &'a Principal,
                input: &'a RemoveSource,
                _value: &'a Location,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(async move { input.lock_source(conn).await.map_err(Error::from) })
            }
            fn check_opened<'a>(
                _conn: &'a mut Conn,
                _principal: &'a Principal,
                _input: &'a RemoveSource,
                old: &'a Location,
                fresh: &'a Location,
            ) -> BoxFuture<'a, Result<bool, Error>> {
                Box::pin(async move { Ok(old == fresh) })
            }
        }
    };
}
removal_source!(
    Delete,
    orch_api_contract::mounts::MountLocationDelete,
    DELETE
);
removal_source!(Purge, orch_api_contract::mounts::MountKeptPurge, PURGE);
impl RemoveSource {
    fn operation(&self) -> &'static str {
        match self.removal {
            Removal::Active => "MountLocationDelete",
            Removal::Kept(_) => "MountKeptPurge",
        }
    }
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        location: &LocationRow,
        removal: Removal,
        confirmed: Option<String>,
    ) -> Result<Self, EngineError> {
        let source = Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            location: Location::from_row(location),
            physical: physical(conn, &Location::from_row(location)).await?,
            removal,
            confirmed,
        };
        if caller.admitted_operation() != Some(source.operation())
            || !source.matches_current(conn).await?
        {
            return Err(EngineError::unreadable(
                "the removed location changed its original source",
            ));
        }
        Ok(source)
    }
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    pub fn location(&self) -> &Location {
        &self.location
    }
    pub fn removal(&self) -> &Removal {
        &self.removal
    }
    pub async fn matches_current(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        Ok(removal_location(
            conn,
            self.location.mount,
            self.location.machine,
            &self.removal,
        )
        .await?
        .as_ref()
        .is_some_and(|row| self.location.matches(row)))
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        sqlx::query("SELECT id FROM mounts WHERE id=$1 FOR SHARE")
            .bind(self.location.mount)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
            .bind(self.location.machine)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        sqlx::query("SELECT id FROM hosts WHERE machine_id=$1 FOR SHARE")
            .bind(self.location.machine)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
        if let Removal::Kept(id) = self.removal {
            sqlx::query("SELECT id FROM mount_kept_data WHERE id=$1 FOR SHARE")
                .bind(id)
                .fetch_optional(conn)
                .await
                .map_err(DbError::from)?;
        }
        Ok(())
    }
    pub fn command(
        &self,
        command: &Outgoing,
    ) -> Result<super::http_command_source::CommandEvidence, EngineError> {
        let body: orch_link_contract::commands::NodeVolumeRemove =
            serde_json::from_value(command.body.clone())?;
        if command.kind != CommandKind::NodeVolumeRemove
            || command.machine != self.location.machine
            || command.key != SerialKey::Volume(self.location.source.clone())
            || self.location.kind != "volume"
            || body.name != self.location.source
            || body.storage != self.location.storage
            || body.labels
                != std::collections::BTreeMap::from([(
                    "orch.mount_id".to_owned(),
                    self.location.mount.to_string(),
                )])
            || self
                .physical
                .driver
                .as_ref()
                .is_none_or(|(driver, _)| *driver != body.driver)
        {
            return Err(EngineError::unreadable(
                "the native removal changed its original volume source",
            ));
        }
        Ok(super::http_command_source::CommandEvidence::MountRemove(
            self.clone(),
        ))
    }
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let frame = match self.removal {
            Removal::Active => {
                continuing::prepare::<Delete>(conn, clock, &self.origin, self.clone())
                    .await
                    .map(|frame| frame.principal().caller().clone())
            }
            Removal::Kept(_) => {
                continuing::prepare::<Purge>(conn, clock, &self.origin, self.clone())
                    .await
                    .map(|frame| frame.principal().caller().clone())
            }
        };
        match frame {
            Ok(caller) => Ok(Some(vec![(Some(caller), None)])),
            Err(Error::Refused | Error::Binding(_)) => Ok(None),
            Err(Error::Native(error)) => Err(error),
        }
    }
}
