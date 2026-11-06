//! The existing Snapshot read door and its actual current credential targets.
//! Stored copied bytes follow the Thread's current Environment; native recordings keep theirs;
//! a threadless whole-workspace export remains private to its maker even after removal.

use crate::EngineError;
use crate::commands::Outgoing;
use crate::operation_scope::{Prepared, targets};
use orch_access_contract::decisions::{Decision, ObjectRef, Refusal};
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::queries::{
    environments::workspace as environments,
    link::CommandRow,
    storage::{self, SnapshotRow},
};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::CommandKind;
use orch_link_contract::source::FsRecordRead;
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};

const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

/// Ordinary refusal keeps the API's existing missing/export and named permission outcomes.
pub enum Access {
    /// The original maker or ordinary Files permission admits this read/write.
    Allowed,
    /// Private export custody belongs to another account; its existence stays hidden.
    MakerOnly,
    /// The existing resource permission producer refused the caller.
    Refused(Box<Refusal>),
}

/// Request-local facts, never serialized as admission or projected as a new response.
#[derive(Clone)]
pub struct Source {
    /// The actual stored object read by the operation.
    pub row: SnapshotRow,
    environment: Id,
}

impl Source {
    /// The same row and placement read by the ordinary Snapshot door.
    pub async fn load(conn: &mut Conn, id: Id) -> Result<Option<Self>, DbError> {
        let Some(row) = storage::snapshot(conn, id).await? else {
            return Ok(None);
        };
        Self::from_row(conn, row).await
    }

    /// Existing callers that already loaded the object preserve their principal/load order.
    pub async fn from_row(conn: &mut Conn, row: SnapshotRow) -> Result<Option<Self>, DbError> {
        let environment =
            if let Some(thread) = row.thread_id.filter(|_| row.storage != "environment") {
                let Some(environment) = environments::thread_place(conn, thread)
                    .await?
                    .and_then(|place| place.environment_id)
                else {
                    return Ok(None);
                };
                environment
            } else {
                row.environment_id
            };
        Ok(Some(Self { row, environment }))
    }

    /// The existing Files governor, as a factual identity rather than an access proof.
    #[must_use]
    pub fn governing_environment(&self) -> Id {
        self.environment
    }

    /// Exact existing account rule. Permission is FilesView for reads and FilesManage for the
    /// existing rename/delete callers; no page access is invented for private export custody.
    pub async fn check(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        permission: Permission,
    ) -> Result<Access, orch_authz::Error> {
        if self.row.thread_id.is_none() {
            return Ok(if self.row.created_by == Some(principal.user()) {
                Access::Allowed
            } else {
                Access::MakerOnly
            });
        }
        Ok(
            match orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: self.environment,
                },
                permission,
                Policy::default(),
            )
            .await?
            {
                Decision::Allowed { .. } => Access::Allowed,
                Decision::Refused(refusal) => Access::Refused(Box::new(refusal)),
            },
        )
    }

    /// Bind current facts through the existing operation catalogue. SnapshotGet's mandatory
    /// EnvironmentRef reads the live origin even when copied bytes now follow another place.
    pub async fn prepare(
        &self,
        conn: &mut Conn,
        operation: &str,
    ) -> Result<Option<Prepared>, EngineError> {
        if !matches!(
            operation,
            "SnapshotGet" | "SnapshotFileList" | "SnapshotFileContentGet" | "SnapshotArchiveGet"
        ) {
            return Err(EngineError::unreadable("not a Snapshot read operation"));
        }
        let Some(governing) = identity(conn, self.environment).await? else {
            return Ok(None);
        };
        let mut prepared = Prepared::for_selection(operation).map_err(EngineError::unreadable)?;
        prepared
            .bind(0, ENVIRONMENT, std::slice::from_ref(&governing))
            .map_err(EngineError::unreadable)?;
        if operation == "SnapshotGet" {
            let origin = if self.row.environment_id == self.environment {
                governing
            } else {
                let Some(origin) = identity(conn, self.row.environment_id).await? else {
                    return Ok(None);
                };
                origin
            };
            prepared
                .bind(1, ENVIRONMENT, &[origin])
                .map_err(EngineError::unreadable)?;
        }
        Ok(Some(prepared))
    }

    /// An immutable association witness, independent of current targets and ordinary permissions.
    #[must_use]
    pub fn witness(&self) -> ReadWitness {
        ReadWitness::from_snapshot(&self.row)
    }
}

async fn identity(
    conn: &mut Conn,
    environment: Id,
) -> Result<Option<orch_access_contract::key_scope::KeyResolvedTarget>, EngineError> {
    match targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await
    {
        Ok(target) => Ok(Some(target)),
        Err(targets::TargetError::Missing) => Ok(None),
        Err(targets::TargetError::Db(error)) => Err(error.into()),
        Err(targets::TargetError::Authz(error)) => Err(error.into()),
    }
}

/// Only the Snapshot association, never cached authority or target ancestry.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadWitness {
    id: Id,
    thread: Option<Id>,
    environment: Id,
    generation: i32,
    maker: Option<Id>,
    kind: String,
    storage: String,
    directory: Option<String>,
    tree: Option<String>,
    manifest: Option<Hash>,
}

impl ReadWitness {
    /// Captures only an actual stored association; this carries no caller or permission.
    #[must_use]
    pub fn from_snapshot(row: &SnapshotRow) -> Self {
        Self {
            id: row.id,
            thread: row.thread_id,
            environment: row.environment_id,
            generation: row.env_generation,
            maker: row.created_by,
            kind: row.kind.clone(),
            storage: row.storage.clone(),
            directory: row.directory.clone(),
            tree: row.tree_ref.clone(),
            manifest: row.manifest_blob,
        }
    }

    /// Relabeling does not rebind immutable content; deletion or replacement does.
    #[must_use]
    pub fn matches(&self, row: &SnapshotRow) -> bool {
        *self == Self::from_snapshot(row)
    }
}

impl std::fmt::Debug for ReadWitness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReadWitness([private])")
    }
}

/// Private original operation and physical source for one new recorded-store command.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotReadAuthority {
    origin: CapturedOperation,
    snapshot: ReadWitness,
    command: Id,
    #[serde(deserialize_with = "read_body")]
    body: Json,
    machine: Id,
    custodian_generation: i32,
    custodian_workspace_root: String,
}

fn read_body<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Json, D::Error> {
    let body = Json::deserialize(deserializer)?;
    serde_json::from_value::<FsRecordRead>(body.clone()).map_err(serde::de::Error::custom)?;
    Ok(body)
}

impl std::fmt::Debug for SnapshotReadAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SnapshotReadAuthority([private])")
    }
}

fn request_matches(operation: &str, snapshot: &ReadWitness, body: &FsRecordRead) -> bool {
    let selection = match operation {
        "SnapshotFileList" => body.path.is_none() && !body.export,
        "SnapshotFileContentGet" => {
            body.path.as_ref().is_some_and(|path| {
                !path.is_empty()
                    && !path.starts_with('/')
                    && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
            }) && !body.export
        }
        "SnapshotArchiveGet" => body.path.is_none() && body.export,
        _ => false,
    };
    selection
        && snapshot.storage == "environment"
        && snapshot.thread.is_some()
        && body.environment_id == snapshot.environment
        && body.snapshot_id == snapshot.id
}

impl SnapshotReadAuthority {
    /// Inputs come from the admitted producer and same actual current custodian/command. The
    /// Snapshot's historical generation is not the current custodian generation.
    pub fn capture(
        caller: &Caller,
        snapshot: &SnapshotRow,
        custodian_machine: Id,
        custodian_generation: i32,
        custodian_workspace_root: &str,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let body: FsRecordRead = serde_json::from_value(command.body.clone())?;
        let snapshot = ReadWitness::from_snapshot(snapshot);
        if !request_matches(origin.operation(), &snapshot, &body)
            || command.id.is_nil()
            || command.kind != CommandKind::FsRecordRead
            || command.machine != custodian_machine
            || command.key.wire().is_some()
        {
            return Err(EngineError::unreadable(
                "recorded read origin does not match its actual command",
            ));
        }
        Ok(Self {
            origin,
            snapshot,
            command: command.id,
            body: command.body.clone(),
            machine: custodian_machine,
            custodian_generation,
            custodian_workspace_root: custodian_workspace_root.to_owned(),
        })
    }

    /// Called only after this command's actual ordinary/witness check on the SAME
    /// existing admission transaction. Keep current key and stored source expiry facts
    /// until the final synchronous native queue boundary, not just a discarded bool.
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let Some(caller) = self.origin.restore_current(conn, clock).await? else {
            return Ok(None);
        };
        let Some(source) = Source::load(conn, self.snapshot.id).await? else {
            return Ok(None);
        };
        if !self.snapshot.matches(&source.row)
            || source.row.state != storage::snapshot_state::COMPLETE
            || source.row.expires_at.is_some_and(|end| end <= clock.now())
        {
            return Ok(None);
        }
        Ok(Some(vec![(
            Some(caller),
            source.row.expires_at.map(Time::new),
        )]))
    }

    /// Called by the existing durable unacknowledged first-send/retry checkpoint. Accepted
    /// result/custody reconciliation does not invoke a new forward-authority check.
    pub async fn authorized(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        now: Time,
    ) -> Result<bool, EngineError> {
        let Ok(body) = serde_json::from_value::<FsRecordRead>(self.body.clone()) else {
            return Ok(false);
        };
        if self.command.is_nil()
            || !request_matches(self.origin.operation(), &self.snapshot, &body)
            || command.id != self.command
            || command.machine_id != self.machine
            || command.kind != "fs.record.read"
            || command.serial_key().is_some()
            || command.body != self.body
        {
            return Ok(false);
        }
        let Some(caller) = self.origin.restore(conn, now).await? else {
            return Ok(false);
        };
        let Some(source) = Source::load(conn, self.snapshot.id).await? else {
            return Ok(false);
        };
        if !self.snapshot.matches(&source.row)
            || source.row.state != storage::snapshot_state::COMPLETE
            || source.row.expires_at.is_some_and(|end| end <= now.get())
        {
            return Ok(false);
        }
        let Some(environment) = environments::environment(conn, source.row.environment_id).await?
        else {
            return Ok(false);
        };
        if environment.machine_id != Some(self.machine)
            || environment.generation != self.custodian_generation
            || environment.workspace_root != self.custodian_workspace_root
        {
            return Ok(false);
        }
        let Some(plan) = source.prepare(conn, self.origin.operation()).await? else {
            return Ok(false);
        };
        let Ok(caller) = plan.admit(&caller, true) else {
            return Ok(false);
        };
        let principal = Principal::load(conn, caller, now).await?;
        Ok(matches!(
            source
                .check(conn, &principal, Permission::FilesView)
                .await?,
            Access::Allowed
        ))
    }
}
