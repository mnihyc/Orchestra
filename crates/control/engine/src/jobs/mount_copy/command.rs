//! Exact Copy commands keep their original request, destination claim and compensation direction.
use super::{KIND, Recipe, frame, refused};
use crate::{
    EngineError,
    commands::{CommandAuthority, Outgoing},
    jobs::{
        command_identity,
        runner::{Job, Stop},
    },
    mount_copies::{self, CopyDecision, DataRef},
};
use futures::future::BoxFuture;
use orch_authz::Caller;
use orch_db::{
    Conn,
    queries::{
        jobs,
        link::{CommandRow, NewCommand},
    },
};
use orch_link_contract::commands::{
    CommandKind, NodeMountExport, NodeMountImport, NodeVolumeEnsure, NodeVolumeRemove, SerialKey,
};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

type Callers = Vec<(Option<Caller>, Option<Time>)>;
const JOURNAL: &str = "mount_copy_commands";
#[derive(Clone, PartialEq, Serialize, Deserialize)]
enum Version {
    #[serde(rename = "mount_copy_execution_v1")]
    One,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    version: Version,
    job: Id,
    recipe: Recipe,
    canonical: Json,
    kind: CommandKind,
    #[serde(with = "crate::commands::RetainedKey")]
    serial: SerialKey,
    undo: bool,
    deadline: Option<Time>,
}
fn labels(job: Id, copy: &CopyDecision) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("orch.mount_id".into(), copy.mount_id.to_string()),
        ("orch.copy_job".into(), job.to_string()),
    ])
}
fn owned_volumes(copy: &CopyDecision) -> impl Iterator<Item = &DataRef> {
    copy.made.then_some(&copy.to).into_iter().chain(
        copy.kept
            .as_ref()
            .and_then(|kept| kept.copied_into.as_ref()),
    )
}
fn expected(
    job: Id,
    plan: &Json,
    decisions: &mount_copies::MountDecisions,
    command: &Outgoing,
    undo: bool,
) -> Result<bool, EngineError> {
    let Some(copy) = decisions.copies.first() else {
        return Ok(false);
    };
    if command
        .body
        .get("work")
        .is_some_and(|work| !work.is_null() && work["job_id"] != json!(job))
    {
        return Ok(false);
    }
    Ok(match command.kind {
        CommandKind::NodeVolumeEnsure if !undo => {
            let body: NodeVolumeEnsure = serde_json::from_value(command.body.clone())?;
            command.machine == copy.target_machine
                && body.driver == copy.target_driver
                && body.labels == labels(job, copy)
                && command.key == SerialKey::Volume(body.name.clone())
                && owned_volumes(copy)
                    .any(|volume| body.name == volume.source && body.storage == volume.storage)
        }
        CommandKind::NodeMountExport if !undo => {
            let body: NodeMountExport = serde_json::from_value(command.body.clone())?;
            body.job_id == job
                && body.mount_id == copy.mount_id
                && command.key == body.data.key()
                && ((command.machine == copy.source_machine
                    && body.driver == copy.source_driver
                    && body.data == copy.from.link())
                    || (!copy.made
                        && command.machine == copy.target_machine
                        && body.driver == copy.target_driver
                        && body.data == copy.to.link()))
        }
        CommandKind::NodeMountImport => {
            let body: NodeMountImport = serde_json::from_value(command.body.clone())?;
            let destination = if undo {
                Some(&copy.to)
            } else {
                (body.name == copy.to.source && body.storage == copy.to.storage)
                    .then_some(&copy.to)
                    .or_else(|| {
                        copy.kept
                            .as_ref()
                            .and_then(|kept| kept.copied_into.as_ref())
                    })
            };
            let archive = if undo || destination.is_some_and(|target| target != &copy.to) {
                mount_copies::done::KEPT_ARCHIVES
            } else {
                mount_copies::done::ARCHIVES
            };
            let unpacking = plan[mount_copies::done::UNPACKING]
                .as_array()
                .is_some_and(|ids| ids.contains(&json!(copy.mount_id)));
            body.job_id == job
                && body.mount_id == copy.mount_id
                && command.machine == copy.target_machine
                && body.driver == copy.target_driver
                && command.key == SerialKey::Volume(body.name.clone())
                && destination.is_some_and(|target| {
                    body.name == target.source && body.storage == target.storage
                })
                && (!undo || (!copy.made && unpacking))
                && plan[archive][copy.mount_id.to_string()] == serde_json::to_value(&body.archive)?
        }
        CommandKind::NodeVolumeRemove if undo => {
            let body: NodeVolumeRemove = serde_json::from_value(command.body.clone())?;
            command.machine == copy.target_machine
                && body.driver == copy.target_driver
                && body.labels == labels(job, copy)
                && command.key == SerialKey::Volume(body.name.clone())
                && owned_volumes(copy)
                    .any(|volume| body.name == volume.source && body.storage == volume.storage)
        }
        _ => false,
    })
}
impl Packet {
    fn authority(&self) -> CommandAuthority {
        CommandAuthority::Lifecycle(Box::new(
            crate::jobs::lifecycle_commands::Evidence::MountCopyExecution(Box::new(self.clone())),
        ))
    }
    pub fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        if !self.undo {
            self.recipe.origin.lock_identity(conn).await?;
            crate::mount_copies::location::operation::lock_source(conn, &self.recipe.review)
                .await
                .map_err(super::mapped)?;
        }
        jobs::lock(conn, self.job).await?;
        Ok(())
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        command: &NewCommand<'_>,
    ) -> Result<Option<Callers>, EngineError> {
        if command.kind != self.kind.as_str()
            || command.serial_key != self.serial.wire().as_deref()
            || self.deadline != Some(Time::new(command.deadline_at))
        {
            return Ok(None);
        }
        if command_identity::canonical(
            command.id,
            command.machine_id,
            command.serial_key,
            command.kind,
            command.body,
        )? != self.canonical
        {
            return Ok(None);
        }
        let Some(row) = jobs::job(conn, self.job).await? else {
            return Ok(None);
        };
        if !row.is_open() || !self.recipe.matches(&row)? || row.plan["copy_published"] == true {
            return Ok(None);
        }
        let recorded = &row.plan[JOURNAL][command.id.to_string()];
        if recorded["frame"] != self.canonical
            || recorded["undo"] != json!(self.undo)
            || recorded["deadline"] != json!(self.deadline)
            || (!self.undo && row.plan["active_command"]["id"] != json!(command.id))
        {
            return Ok(None);
        }
        let Some(copy) = self.recipe.decisions.copies.first() else {
            return Ok(None);
        };
        if !mount_copies::claims::held(conn, row.id, copy).await? {
            return Ok(None);
        }
        let proposed = Outgoing {
            native_target: None,
            id: command.id,
            machine: command.machine_id,
            key: self.serial.clone(),
            kind: self.kind,
            body: command.body.clone(),
            deadline: Some(Time::new(command.deadline_at)),
            authority: None,
        };
        if !expected(
            row.id,
            &row.plan,
            &self.recipe.decisions,
            &proposed,
            self.undo,
        )? {
            return Ok(None);
        }
        if self.undo {
            if row.plan["committed"] == true || crate::jobs::runner::abort_reason(&row)?.is_none() {
                return Ok(None);
            }
            return Ok(Some(vec![(None, None)]));
        }
        let checkpoint = frame(conn, clock, &row, &self.recipe).await?;
        let caller = checkpoint.parts.principal.caller().clone();
        checkpoint
            .parts
            .first_effect
            .check(clock)
            .map_err(super::mapped)?;
        Ok(Some(vec![(Some(caller), checkpoint.until)]))
    }
    pub fn current_issue<'a>(
        &'a self,
        conn: &'a mut Conn,
        command: &'a NewCommand<'_>,
        clock: &'a dyn Clock,
    ) -> BoxFuture<'a, Result<Option<Callers>, EngineError>> {
        Box::pin(self.current(conn, clock, command))
    }
    pub fn current_stored<'a>(
        &'a self,
        conn: &'a mut Conn,
        command: &'a CommandRow,
        clock: &'a dyn Clock,
    ) -> BoxFuture<'a, Result<Option<Callers>, EngineError>> {
        Box::pin(async move {
            self.current(
                conn,
                clock,
                &NewCommand {
                    native_target: None,
                    id: command.id,
                    machine_id: command.machine_id,
                    kind: &command.kind,
                    body: &command.body,
                    serial_key: command.serial_key(),
                    deadline_at: command.deadline_at,
                },
            )
            .await
        })
    }
}
pub(crate) async fn decorate(job: &Job, command: &mut Outgoing, forward: bool) -> Result<(), Stop> {
    let row = job.row().await?;
    if row.kind != KIND {
        return Ok(());
    }
    let recipe: Recipe =
        serde_json::from_value(row.plan["copy"].clone()).map_err(EngineError::from)?;
    if !recipe.matches(&row)? || !expected(row.id, &row.plan, &recipe.decisions, command, !forward)?
    {
        return Err(refused().into());
    }
    let mut tx = job.engine().db().begin().await?;
    let until = if forward {
        let checkpoint = frame(&mut tx, job.engine().inner.clock.as_ref(), &row, &recipe).await?;
        checkpoint
            .parts
            .first_effect
            .check(job.engine().inner.clock.as_ref())
            .map_err(super::mapped)?;
        checkpoint.until
    } else {
        None
    };
    let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if !recipe.matches(&owned)? {
        return Err(refused().into());
    }
    let saved = owned.plan[JOURNAL].get(command.id.to_string());
    let deadline = match saved {
        Some(record) => {
            serde_json::from_value(record["deadline"].clone()).map_err(EngineError::from)?
        }
        None => command.deadline.unwrap_or_else(|| {
            Time::new(
                job.now() + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            )
        }),
    };
    command.deadline = Some(until.map_or(deadline, |until| deadline.min(until)));
    let packet = Packet {
        version: Version::One,
        job: row.id,
        recipe,
        canonical: command_identity::canonical(
            command.id,
            command.machine,
            command.key.wire().as_deref(),
            command.kind.as_str(),
            &command.body,
        )?,
        kind: command.kind,
        serial: command.key.clone(),
        undo: !forward,
        deadline: command.deadline,
    };
    let mut journal = owned
        .plan
        .get(JOURNAL)
        .cloned()
        .unwrap_or_else(|| json!({}));
    let record = json!({"frame":packet.canonical,"undo":packet.undo,"deadline":packet.deadline});
    let records = journal.as_object_mut().ok_or_else(refused)?;
    if records
        .get(&command.id.to_string())
        .is_some_and(|old| old != &record)
    {
        return Err(refused().into());
    }
    records.insert(command.id.to_string(), record);
    jobs::merge_plan(&mut tx, job.id(), &json!({JOURNAL:journal}), job.now()).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    command.authority = Some(packet.authority());
    Ok(())
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
