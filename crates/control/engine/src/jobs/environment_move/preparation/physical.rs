//! Native physical association checks for the already selected lifecycle source.
use super::*;
use orch_link_contract::commands::*;
use orch_types::{Id, Json, Time};

impl Origin {
    pub(super) async fn physical(
        &self,
        conn: &mut Conn,
        machine: Id,
        kind: CommandKind,
        body: &Json,
        now: Time,
    ) -> Result<bool, EngineError> {
        match kind {
            CommandKind::FsManifest => {
                let ask: FsManifest = serde_json::from_value(body.clone())?;
                Ok(self
                    .source(ask.environment_id)
                    .is_some_and(|s| s.machine_matches(machine)))
            }
            CommandKind::FsDataMeasure => {
                let ask: orch_link_contract::source::FsDataMeasure =
                    serde_json::from_value(body.clone())?;
                Ok(self
                    .source(ask.environment_id)
                    .is_some_and(|s| s.machine_matches(machine)))
            }
            CommandKind::FsStat => {
                let ask: FsStat = serde_json::from_value(body.clone())?;
                if ask.mount.is_some()
                    || (!ask.path.is_empty()
                        && orch_link_contract::commands::validate_selection_pattern(&ask.path)
                            .is_err())
                {
                    return Ok(false);
                }
                if self
                    .source(ask.environment_id)
                    .is_some_and(|s| s.machine_matches(machine))
                {
                    return Ok(true);
                }
                for (object, _) in self.destinations() {
                    if object.kind == orch_access_contract::permissions::ObjectKind::Environment
                        && object.id == ask.environment_id
                    {
                        let source = origin::Environment::load(conn, object.id).await?;
                        if source.machine_matches(machine) {
                            return Ok(true);
                        }
                    }
                }
                Ok(false)
            }
            CommandKind::NodeKeepEstimate => {
                let ask: NodeKeepEstimate = serde_json::from_value(body.clone())?;
                Ok(self.environments().any(|id| {
                    self.source(id)
                        .is_some_and(|s| s.runtime_matches(machine, ask.runtime_id))
                }))
            }
            CommandKind::NodeRuntimeVolumesMeasure => {
                let ask: NodeRuntimeVolumesMeasure = serde_json::from_value(body.clone())?;
                let source = self.environments().find_map(|id| {
                    self.source(id)
                        .filter(|s| s.runtime_matches(machine, ask.runtime_id))
                });
                let Some(source) = source else {
                    return Ok(false);
                };
                if ask.volumes.is_empty() {
                    return Ok(false);
                }
                let mut seen = std::collections::BTreeSet::new();
                for volume in &ask.volumes {
                    if !seen.insert(volume.selector.to_string())
                        || source
                            .volumes()
                            .get(crate::volume_carry::role(volume.selector))
                            != Some(&serde_json::to_value(&volume.volume)?)
                    {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            CommandKind::NodeVolumeCarryCheck => {
                let ask: NodeVolumeCarryCheck = serde_json::from_value(body.clone())?;
                if !self.actual_machine(conn, machine).await? {
                    return Ok(false);
                }
                let driver = orch_db::queries::resources::mounts::data_driver(conn, machine)
                    .await?
                    .is_some_and(|(d, _)| d == ask.driver);
                if driver {
                    return Ok(true);
                }
                for (object, _) in self.destinations() {
                    if object.kind == orch_access_contract::permissions::ObjectKind::Template
                        && orch_db::queries::resources::hosting::template(conn, object.id)
                            .await?
                            .is_some_and(|t| t.driver == ask.driver)
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CommandKind::NodeVolumeInspect => {
                if !matches!(self.kind, Kind::RecoverPreflight | Kind::RecoverCreate) {
                    return Ok(false);
                }
                let ask: NodeVolumeInspect = serde_json::from_value(body.clone())?;
                if self.source(ask.environment_id).is_none()
                    || !self.actual_machine(conn, machine).await?
                {
                    return Ok(false);
                }
                let row:Option<(Option<Id>,Json)>=sqlx::query_as("SELECT template_id,volumes FROM runtimes WHERE environment_id=$1 AND lost_at IS NOT NULL ORDER BY generation DESC,created_at DESC LIMIT 1")
                    .bind(ask.environment_id).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
                let volumes = row.map(|(_, v)| v).unwrap_or_else(|| serde_json::json!({}));
                let recorded = volumes
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(role, _)| role.as_str() != "state")
                    .any(|(_, v)| {
                        v.get("directory").is_none()
                            && v["name"].as_str() == Some(ask.name.as_str())
                            && v["storage"].as_str() == ask.storage.as_deref()
                    });
                let named = ask.name == format!("orch-ws-{}", ask.environment_id)
                    || ask.name == format!("orch-home-{}", ask.environment_id);
                if !recorded && !named {
                    return Ok(false);
                }
                for (object, _) in self.destinations() {
                    if object.kind == orch_access_contract::permissions::ObjectKind::Template
                        && orch_db::queries::resources::hosting::template(conn, object.id)
                            .await?
                            .is_some_and(|t| t.driver == ask.driver)
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CommandKind::NodeMountMeasure | CommandKind::NodeMountFingerprint => {
                let (mount, driver, data) = if kind == CommandKind::NodeMountMeasure {
                    let a: NodeMountMeasure = serde_json::from_value(body.clone())?;
                    (a.mount_id, a.driver, a.data)
                } else {
                    let a: NodeMountFingerprint = serde_json::from_value(body.clone())?;
                    (a.mount_id, a.driver, a.data)
                };
                if !self.actual_machine(conn, machine).await? {
                    return Ok(false);
                }
                let mut attached = false;
                for env in self.environments() {
                    if crate::mounts::views(conn, env, now.get())
                        .await?
                        .iter()
                        .any(|v| v.mount.id == mount)
                    {
                        attached = true;
                        break;
                    }
                }
                if !attached {
                    return Ok(false);
                }
                let location = orch_db::queries::resources::mounts::locations(conn, &[mount])
                    .await?
                    .into_iter()
                    .find(|l| l.machine_id == machine);
                let Some(location) = location else {
                    return Ok(false);
                };
                let expected = crate::mount_copies::DataRef {
                    kind: location.kind,
                    source: location.source,
                    storage: location.storage,
                }
                .link();
                Ok(expected == data
                    && orch_db::queries::resources::mounts::data_driver(conn, machine)
                        .await?
                        .is_some_and(|(d, _)| d == driver))
            }
            _ => Ok(false),
        }
    }
    async fn actual_machine(&self, conn: &mut Conn, machine: Id) -> Result<bool, EngineError> {
        if self
            .environments()
            .any(|id| self.source(id).is_some_and(|s| s.machine_matches(machine)))
        {
            return Ok(true);
        }
        use orch_access_contract::permissions::ObjectKind;
        for (object, _) in self.destinations() {
            match object.kind {
                ObjectKind::Machine if object.id == machine => return Ok(true),
                ObjectKind::Host => {
                    if orch_db::queries::resources::hosting::host(conn, object.id)
                        .await?
                        .is_some_and(|h| h.machine_id == machine)
                    {
                        return Ok(true);
                    }
                }
                ObjectKind::Environment
                    if origin::Environment::load(conn, object.id)
                        .await?
                        .machine_matches(machine) =>
                {
                    return Ok(true);
                }
                _ => {}
            }
        }
        Ok(false)
    }
}

/// Source locks precede the exact physical location/keeper read; the current frame is repeated after this wait.
pub(super) async fn lock(
    conn: &mut Conn,
    machine: Id,
    kind: CommandKind,
    body: &Json,
) -> Result<(), EngineError> {
    if matches!(
        kind,
        CommandKind::NodeMountMeasure | CommandKind::NodeMountFingerprint
    ) {
        let mount: Id = serde_json::from_value(body.get("mount_id").cloned().ok_or_else(closed)?)?;
        sqlx::query("SELECT id FROM mounts WHERE id=$1 FOR SHARE")
            .bind(mount)
            .execute(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        sqlx::query(
            "SELECT mount_id FROM mount_locations WHERE mount_id=$1 AND machine_id=$2 FOR SHARE",
        )
        .bind(mount)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    }
    Ok(())
}
