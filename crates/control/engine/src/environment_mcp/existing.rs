//! The stored server and actual offering used by either existing-definition import.

use super::{ImportSelection, may_import, select_import};
use crate::{
    EngineError,
    operation_scope::{
        continuing::{Error as CheckError, Source},
        sources::Facts,
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    mcp_sources::{self as declared, Source as Role},
    operation_sources::Declaration,
    resources::{McpOfferingUse, McpServerReimport},
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::resources::mcp::{self as rows, McpRow},
};
use orch_link_contract::commands::{McpImport, SerialKey};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

/// The actual manager request, not a source selected from a serialized response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Intent {
    Reimport {
        server: Id,
    },
    Offering {
        server: Id,
        machine: Id,
        expected_revision: i64,
        expected_fingerprint: Option<Hash>,
    },
}
impl Intent {
    pub fn server(&self) -> Id {
        match self {
            Self::Reimport { server } | Self::Offering { server, .. } => *server,
        }
    }
    pub fn operation(&self) -> &'static str {
        match self {
            Self::Reimport { .. } => declared::REIMPORT.operation,
            Self::Offering { .. } => declared::OFFERING_USE.operation,
        }
    }
}
/// Native selector refusals retain the same transport-independent meaning.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the imported server is unavailable")]
    Missing,
    #[error("this server was not imported from a machine")]
    NotImported,
    #[error("No machine offers this imported server")]
    NoOffering,
    #[error("Review the different definition and choose Use this machine's")]
    Review,
    #[error("The server changed; review its offerings again")]
    StaleRevision,
    #[error("The reported definition changed; review it again")]
    StaleFingerprint,
    #[error("no environment of this machine permits this caller to reimport")]
    NoEnvironment,
    #[error(transparent)]
    Import(#[from] super::Error),
    #[error(transparent)]
    Native(#[from] EngineError),
}
impl From<DbError> for Error {
    fn from(error: DbError) -> Self {
        Self::Native(error.into())
    }
}
fn native(error: super::Error) -> Error {
    Error::Import(error)
}

/// Same real required governors for native writers and already completed response readers.
/// No copy operand is supplied only when the caller is genuinely preparing no new copy.
pub async fn facts(
    conn: &mut Conn,
    row: &McpRow,
    copy: Option<(Id, Id)>,
) -> Result<Vec<Facts<Role>>, EngineError> {
    let subject = match row.environment_id {
        Some(id) => ObjectRef {
            kind: ObjectKind::Environment,
            id,
        },
        None => ObjectRef {
            kind: ObjectKind::McpServer,
            id: row.id,
        },
    };
    let managed = targets::page_object(conn, subject)
        .await
        .map_err(EngineError::unreadable)?;
    let parent = match row.environment_id {
        Some(id) => vec![(
            KeyTargetKind::Object(ObjectKind::Environment),
            vec![
                targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id,
                    },
                )
                .await
                .map_err(EngineError::unreadable)?,
            ],
        )],
        None => Vec::new(),
    };
    let (environment, machine) = match copy {
        Some((environment, machine)) => (
            vec![(
                KeyTargetKind::Object(ObjectKind::Environment),
                vec![
                    targets::page_object(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Environment,
                            id: environment,
                        },
                    )
                    .await
                    .map_err(EngineError::unreadable)?,
                ],
            )],
            vec![(
                KeyTargetKind::Object(ObjectKind::Machine),
                vec![
                    targets::machine_subject(conn, machine)
                        .await
                        .map_err(EngineError::unreadable)?,
                ],
            )],
        ),
        None => (Vec::new(), Vec::new()),
    };
    Ok(vec![
        Facts::one(
            Role::Subject,
            KeyTargetKind::Object(ObjectKind::McpServer),
            managed,
        ),
        Facts::collection(Role::Parent, parent),
        Facts::collection(Role::ImportEnvironment, environment),
        Facts::collection(Role::ImportMachine, machine),
    ])
}
pub async fn require_server(
    conn: &mut Conn,
    principal: &Principal,
    row: &McpRow,
) -> Result<(), CheckError> {
    let (subject, permission) = match row.environment_id {
        Some(id) => (
            ObjectRef {
                kind: ObjectKind::Environment,
                id,
            },
            Permission::EnvironmentManage,
        ),
        None => (
            ObjectRef {
                kind: ObjectKind::McpServer,
                id: row.id,
            },
            Permission::McpManage,
        ),
    };
    if !orch_authz::resources::check(conn, principal, subject, permission, Policy::default())
        .await?
        .is_allowed()
    {
        return Err(CheckError::Refused);
    }
    Ok(())
}
fn fingerprint(row: &McpRow) -> Result<Hash, EngineError> {
    crate::jobs::command_identity::hash(
        &serde_json::json!({"id":row.id,"imported":row.imported,"name":row.name,"description":row.description,"org":row.org_id,"home":row.home_id,"owner":row.owner_id,"machine":row.machine_id,"environment":row.environment_id,"spec":row.spec,"enabled":row.enabled,"revision":row.revision}),
    )
}
/// Actual native operands. Only identifiers, original input and stored fingerprints are durable.
pub struct Selection {
    pub row: McpRow,
    pub source: ImportSelection,
    pub origin: Json,
    pub expected: Option<Hash>,
}
impl Selection {
    pub fn command(&self) -> McpImport {
        let mut command = self.source.command(self.row.id);
        command.expected_fingerprint = self.expected;
        command
    }
}
/// Select the same native offering once; key bounds participate before choosing its environment.
pub async fn select<K: Kind>(
    conn: &mut Conn,
    caller: &Caller,
    now: Time,
    intent: &Intent,
) -> Result<Selection, Error> {
    if intent.operation() != K::DECLARATION.operation {
        return Err(Error::Missing);
    }
    let mut row = rows::servers(conn, &[intent.server()])
        .await?
        .pop()
        .ok_or(Error::Missing)?;
    if !row.imported {
        return Err(Error::NotImported);
    }
    let offerings = rows::offerings(conn, &[row.id]).await?;
    let offering = match intent {
        Intent::Reimport { .. } => offerings
            .iter()
            .find(|offering| Some(offering.machine_id) == row.machine_id)
            .or(offerings.first()),
        Intent::Offering {
            machine,
            expected_revision,
            ..
        } => {
            if row.revision != *expected_revision {
                return Err(Error::StaleRevision);
            }
            offerings.iter().find(|offering| {
                offering.machine_id == *machine
                    && (offering.state == "found" || offering.proposal.is_some())
            })
        }
    }
    .ok_or(Error::NoOffering)?;
    if matches!(intent, Intent::Reimport { .. })
        && offering
            .proposal
            .as_ref()
            .is_some_and(|proposal| proposal["reason"] == "different_definition")
    {
        return Err(Error::Review);
    }
    let origin = match intent {
        Intent::Offering { .. } => offering
            .proposal
            .as_ref()
            .map(|proposal| proposal["source"].clone())
            .unwrap_or_else(|| offering.source.clone()),
        Intent::Reimport { .. } => {
            if offering.source.is_object() {
                offering.source.clone()
            } else {
                row.spec["imported"].clone()
            }
        }
    };
    let expected = match intent {
        Intent::Offering {
            expected_fingerprint,
            ..
        } => {
            let reported = origin
                .get("definition_fingerprint")
                .cloned()
                .and_then(|value| serde_json::from_value::<Option<Hash>>(value).ok())
                .flatten();
            if reported != *expected_fingerprint {
                return Err(Error::StaleFingerprint);
            }
            reported
        }
        Intent::Reimport { .. } => None,
    };
    let machine = offering.machine_id;
    let environments = match intent {
        Intent::Offering { .. } => vec![
            offering
                .proposal
                .as_ref()
                .and_then(|proposal| proposal["environment"].as_str())
                .and_then(|id| id.parse::<Id>().ok())
                .or(offering.environment_id)
                .ok_or(Error::NoEnvironment)?,
        ],
        Intent::Reimport { .. } => rows::on_machine(conn, machine).await?,
    };
    let scope = if origin["scope"] == "directory" {
        serde_json::json!("project")
    } else {
        origin["scope"].clone()
    };
    for environment in environments {
        let prepared = crate::operation_scope::sources::bind_required(
            K::ROUTE,
            K::DECLARATION,
            &facts(conn, &row, Some((environment, machine))).await?,
        )
        .map_err(EngineError::unreadable)?;
        let Ok(admitted) = prepared.preparation_caller(caller) else {
            continue;
        };
        let principal = Principal::load(conn, admitted, now)
            .await
            .map_err(EngineError::from)?;
        if !may_import(conn, &principal, environment)
            .await
            .map_err(native)?
        {
            continue;
        }
        let selected = select_import(
            conn,
            environment,
            origin["source"].as_str().unwrap_or_default(),
            scope.clone(),
            origin["directory"].as_str().map(str::to_owned),
            origin["name"].as_str().unwrap_or_default(),
        )
        .await
        .map_err(native)?;
        if selected.machine() != machine {
            continue;
        }
        row.machine_id = Some(machine);
        return Ok(Selection {
            row,
            source: selected,
            origin,
            expected,
        });
    }
    Err(Error::NoEnvironment)
}

/// Original source tuple of one real existing-resource copy.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    intent: Intent,
    command_id: Id,
    server_fingerprint: Hash,
    base_spec: Hash,
    source_fingerprint: Hash,
    machine: Id,
    scope: Json,
    command: McpImport,
}
impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExistingMcpImportInput([native])")
    }
}
impl Input {
    pub fn capture(intent: Intent, id: Id, selected: &Selection) -> Result<Self, EngineError> {
        Ok(Self {
            intent,
            command_id: id,
            server_fingerprint: fingerprint(&selected.row)?,
            base_spec: crate::jobs::command_identity::hash(&selected.row.spec)?,
            source_fingerprint: selected.source.fingerprint()?,
            machine: selected.source.machine(),
            scope: selected.source.api_scope().clone(),
            command: selected.command(),
        })
    }
    pub fn intent(&self) -> &Intent {
        &self.intent
    }
    pub fn id(&self) -> Id {
        self.command_id
    }
    pub fn server(&self) -> Id {
        self.command.server_id
    }
    pub fn machine(&self) -> Id {
        self.machine
    }
    pub fn environment(&self) -> Id {
        self.command.environment_id
    }
    pub fn command(&self) -> &McpImport {
        &self.command
    }
    pub fn serial(&self) -> SerialKey {
        SerialKey::Environment(self.environment())
    }
}
/// The two rows differ only in their server-derived native intent and canonical operation.
pub trait Kind: Send + Sync + 'static {
    const ROUTE: Route;
    const DECLARATION: &'static Declaration<Role>;
}
pub struct Reimport;
pub struct Offering;
impl Kind for Reimport {
    const ROUTE: Route = McpServerReimport::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declared::REIMPORT;
}
impl Kind for Offering {
    const ROUTE: Route = McpOfferingUse::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declared::OFFERING_USE;
}
pub struct Stored<K>(PhantomData<K>);
impl<K: Kind> Source for Stored<K> {
    type Role = Role;
    type Input = Input;
    type Value = Selection;
    const ROUTE: Route = K::ROUTE;
    const DECLARATION: &'static Declaration<Role> = K::DECLARATION;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        now: Time,
    ) -> BoxFuture<'a, Result<(Selection, Vec<Facts<Role>>), CheckError>> {
        Box::pin(async move {
            let selected = select::<K>(conn, caller, now, &input.intent)
                .await
                .map_err(|error| match error {
                    Error::Native(error) => CheckError::Native(error),
                    _ => CheckError::Refused,
                })?;
            if fingerprint(&selected.row)? != input.server_fingerprint
                || selected.source.fingerprint()? != input.source_fingerprint
                || selected.command() != input.command
            {
                return Err(CheckError::Refused);
            }
            let facts = facts(
                conn,
                &selected.row,
                Some((input.environment(), input.machine)),
            )
            .await?;
            Ok((selected, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        value: &'a Selection,
    ) -> BoxFuture<'a, Result<(), CheckError>> {
        Box::pin(async move {
            require_server(conn, principal, &value.row).await?;
            if !may_import(conn, principal, input.environment())
                .await
                .map_err(|error| CheckError::Native(EngineError::unreadable(error)))?
            {
                return Err(CheckError::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Input,
        value: &'a Selection,
    ) -> BoxFuture<'a, Result<(), CheckError>> {
        Box::pin(async move {
            rows::lock_definition_for_command(conn, input.server(), Some(input.id())).await?;
            let mut environments = vec![input.environment()];
            environments.extend(value.row.environment_id);
            environments.sort_unstable();
            environments.dedup();
            sqlx::query("SELECT id FROM environments WHERE id=ANY($1) ORDER BY id FOR SHARE")
                .bind(environments)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM mcp_servers WHERE id=$1 FOR SHARE")
                .bind(input.server())
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                .bind(input.machine)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT machine_id FROM mcp_server_offerings WHERE server_id=$1 AND machine_id=$2 FOR SHARE").bind(input.server()).bind(input.machine).fetch_optional(conn).await.map_err(DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        old: &'a Selection,
        fresh: &'a Selection,
    ) -> BoxFuture<'a, Result<bool, CheckError>> {
        Box::pin(async move {
            Ok(fingerprint(&old.row)? == fingerprint(&fresh.row)?
                && old.source.preserves(&fresh.source)
                && old.command() == fresh.command())
        })
    }
}

/// Controller-owned outcome of this same native copy, never a permission or new request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub server_id: Id,
    pub revision: Option<i64>,
    pub selected: bool,
    pub cleanup_queued: bool,
}

/// A real accepted existing-resource input, never the creator's unfiled-copy proof.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    origin: CapturedOperation,
    session: Option<Id>,
    until: Option<Time>,
    input: Input,
    canonical: Json,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExistingMcpImportCommand([private])")
    }
}
impl Command {
    fn matches(&self, row: &orch_db::queries::link::CommandRow) -> Result<bool, EngineError> {
        Ok(row.id == self.input.id()
            && row.kind == "mcp.import"
            && crate::jobs::command_identity::canonical(
                row.id,
                row.machine_id,
                row.serial_key(),
                &row.kind,
                &row.body,
            )? == self.canonical)
    }
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        session: Option<Id>,
        until: Option<Time>,
        input: Input,
        command: &crate::commands::Outgoing,
        now: Time,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let (route, declaration, facts) = match input.intent {
            Intent::Reimport { .. } => {
                let (_, facts) = Stored::<Reimport>::prepare(conn, caller, &input, now)
                    .await
                    .map_err(EngineError::unreadable)?;
                (Reimport::ROUTE, Reimport::DECLARATION, facts)
            }
            Intent::Offering { .. } => {
                let (_, facts) = Stored::<Offering>::prepare(conn, caller, &input, now)
                    .await
                    .map_err(EngineError::unreadable)?;
                (Offering::ROUTE, Offering::DECLARATION, facts)
            }
        };
        let required = crate::operation_scope::sources::bind_required(route, declaration, &facts)
            .map_err(EngineError::unreadable)?
            .required_facets()
            .map_err(EngineError::unreadable)?;
        if origin.operation() != input.intent.operation()
            || session.is_some() == caller.grant().is_some()
            || session.is_some() && until.is_none()
            || command.id != input.id()
            || command.machine != input.machine()
            || command.key != input.serial()
            || command.kind != orch_link_contract::commands::CommandKind::McpImport
            || command.body != serde_json::to_value(input.command())?
            || !required
                .iter()
                .all(|fact| caller.admitted_facets().unwrap_or(&[]).contains(fact))
        {
            return Err(EngineError::unreadable(
                "the native import does not match its original stored source",
            ));
        }
        Ok(Self {
            origin,
            session,
            until,
            input,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
    pub fn input(&self) -> &Input {
        &self.input
    }
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    /// Only an actual canonical privately owned successful native copy has a completed effect.
    pub(super) async fn completed(
        &self,
        conn: &mut Conn,
    ) -> Result<Option<orch_link_contract::commands::McpImportResult>, EngineError> {
        let Some(stored) =
            orch_db::queries::link::command_with_authority(conn, self.input.id()).await?
        else {
            return Ok(None);
        };
        if stored.command.state != "succeeded"
            || stored.authority
                != Some(serde_json::to_value(
                    crate::commands::CommandAuthority::mcp_existing_import(self.clone()),
                )?)
            || !orch_db::queries::link::command_authority_required(conn, self.input.id()).await?
            || crate::jobs::command_identity::canonical(
                stored.command.id,
                stored.command.machine_id,
                stored.command.serial_key(),
                &stored.command.kind,
                &stored.command.body,
            )? != self.canonical
        {
            return Ok(None);
        }
        let Some(result) = stored.command.result else {
            return Ok(None);
        };
        let Ok(result) =
            serde_json::from_value::<orch_link_contract::commands::McpImportResult>(result)
        else {
            return Ok(None);
        };
        if self.input.command.expected_fingerprint.is_some()
            && result.definition_fingerprint != self.input.command.expected_fingerprint
        {
            return Ok(None);
        }
        Ok(Some(result))
    }
    /// Acknowledging this accepted effect records no new target, dispatch or public Read.
    /// Source admission owns every forward command; this only files its exact native outcome.
    pub async fn reconcile(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Completion, EngineError> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(EngineError::unreadable(
                "native import completion requires its acknowledgment transaction",
            ));
        }
        self.lock_source(conn).await?;
        let _ = orch_db::queries::link::lock_command_with_authority(conn, self.input.id())
            .await?
            .ok_or_else(|| EngineError::unreadable("the native copy journal is absent"))?;
        let reply = self.completed(conn).await?.ok_or_else(|| {
            EngineError::unreadable("the native import has no exact successful copy")
        })?;
        if let Some(old) =
            orch_db::queries::link::mcp_import_completion(conn, self.input.id()).await?
        {
            let old: Completion = serde_json::from_value(old)?;
            if old.server_id != self.input.server() {
                return Err(EngineError::unreadable(
                    "the native completion belongs to another server",
                ));
            }
            return Ok(old);
        }
        let now = clock.now();
        let subject = rows::subject(conn, self.input.server()).await?;
        if let Some(environment) = subject.and_then(|subject| subject.environment_id) {
            sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
                .bind(environment)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
        }
        let row = rows::lock(conn, self.input.server()).await?;
        let outcome = if let Some(mut row) = row {
            let selected = crate::jobs::command_identity::hash(&row.spec)? == self.input.base_spec;
            let spec = super::ImportRecord {
                source: &self.input.command.source,
                scope: &self.input.scope,
                directory: self.input.command.directory.as_deref(),
                name: &self.input.command.name,
            }
            .spec(reply, self.origin.user(), Time::new(now));
            if selected {
                row.spec = spec;
                row.machine_id = Some(self.input.machine);
                rows::select_definition(
                    conn,
                    &row,
                    self.input.machine,
                    self.input.environment(),
                    now,
                )
                .await?;
            } else {
                let mut source = spec["imported"].clone();
                source["transport"] = spec["transport"].clone();
                rows::offer(
                    conn,
                    row.id,
                    self.input.machine,
                    Some(self.input.environment()),
                    &source,
                    now,
                )
                .await?;
                if source["definition_fingerprint"]
                    != row.spec["imported"]["definition_fingerprint"]
                {
                    sqlx::query("UPDATE mcp_server_offerings SET state='found',reason='different_definition' WHERE server_id=$1 AND machine_id=$2")
                        .bind(row.id).bind(self.input.machine).execute(&mut *conn).await.map_err(DbError::from)?;
                }
            }
            let revision = rows::servers(conn, &[row.id])
                .await?
                .pop()
                .ok_or_else(|| EngineError::unreadable("the reconciled server is absent"))?
                .revision;
            Completion {
                server_id: row.id,
                revision: Some(revision),
                selected,
                cleanup_queued: false,
            }
        } else {
            let id =
                crate::ids::derived("mcp.existing-import.inverse", &[self.input.id().as_bytes()]);
            let mut cleanup = crate::commands::Outgoing::new(
                id,
                self.input.machine,
                SerialKey::Machine,
                &orch_link_contract::commands::McpForget {
                    server_id: self.input.server(),
                },
            )?;
            let proof = super::ForgetCommand::capture_abandoned_existing(
                conn,
                clock,
                self.clone(),
                &cleanup,
            )
            .await?;
            cleanup.deadline = proof.deadline();
            let private =
                serde_json::to_value(crate::commands::CommandAuthority::mcp_forget(proof))?;
            orch_db::queries::link::insert_command_with_authority(
                conn,
                &orch_db::queries::link::NewCommand {
                    id,
                    machine_id: cleanup.machine,
                    kind: cleanup.kind.as_str(),
                    body: &cleanup.body,
                    serial_key: cleanup.key.wire().as_deref(),
                    deadline_at: cleanup
                        .deadline
                        .ok_or_else(|| {
                            EngineError::unreadable(
                                "the exact copied-file inverse has no native deadline",
                            )
                        })?
                        .get(),
                },
                now,
                Some(&private),
            )
            .await?;
            Completion {
                server_id: self.input.server(),
                revision: None,
                selected: false,
                cleanup_queued: true,
            }
        };
        let mut audit = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(self.origin.user()),
            orch_db::audit::action!(orch_access_contract::audit::AuditAction::McpImported).as_str(),
            "mcp_server",
            Some(outcome.server_id),
            "Completed a native MCP definition copy",
        );
        audit.details = serde_json::json!({"command":self.input.id(),"machine":self.input.machine,"environment":self.input.environment(),"selected":outcome.selected,"revision":outcome.revision,"cleanup_queued":outcome.cleanup_queued});
        orch_db::audit::append(conn, &audit, &orch_db::audit::Origin::default(), now).await?;
        if !orch_db::queries::link::finish_mcp_import(
            conn,
            self.input.id(),
            &serde_json::to_value(&outcome)?,
        )
        .await?
        {
            return Err(EngineError::unreadable(
                "the native import completion was already filed",
            ));
        }
        Ok(outcome)
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        rows::lock_definition_for_command(conn, self.input.server(), Some(self.input.id())).await?;
        Ok(())
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.id != self.input.id()
            || crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key,
                command.kind,
                command.body,
            )? != self.canonical
        {
            return Ok(None);
        }
        let Some(session) = crate::commands::authority::session_state(
            conn,
            &self.origin,
            self.session,
            clock.time(),
        )
        .await?
        else {
            return Ok(None);
        };
        let Some(caller) = self.origin.restore_current(conn, clock).await? else {
            return Ok(None);
        };
        if self.origin.operation() != self.input.intent.operation()
            || self.session.is_some() == caller.grant().is_some()
            || self.session.is_some() && self.until.is_none()
        {
            return Ok(None);
        }
        let caller = caller
            .clone()
            .with_admin_mode(caller.as_admin() && session.is_some_and(|(admin, _)| admin));
        let current = match self.input.intent {
            Intent::Reimport { .. } => {
                current::<Reimport>(conn, &self.origin, &caller, &self.input, clock).await
            }
            Intent::Offering { .. } => {
                current::<Offering>(conn, &self.origin, &caller, &self.input, clock).await
            }
        }?;
        let Some(caller) = current else {
            return Ok(None);
        };
        let until = match (self.until, session.map(|(_, until)| until)) {
            (Some(old), Some(now)) => Some(old.min(now)),
            (old, now) => old.or(now),
        };
        Ok((caller.key_deadline_allows(clock)
            && until.is_none_or(|until| clock.now() < until.get()))
        .then_some(vec![(Some(caller), until)]))
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue_callers(
            conn,
            &orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            clock,
        )
        .await
    }
}

/// Read only the actual command's private kind; malformed required origin never becomes old work.
pub async fn recorded_command(conn: &mut Conn, id: Id) -> Result<Option<Command>, EngineError> {
    let Some(row) =
        orch_db::queries::link::command_with_authority_kind(conn, id, "mcp.import").await?
    else {
        return Ok(None);
    };
    if row.command.kind != "mcp.import" {
        return Ok(None);
    }
    let Some(value) = row.authority else {
        if orch_db::queries::link::command_authority_required(conn, id).await? {
            return Err(EngineError::unreadable(
                "the required native import origin is unavailable",
            ));
        }
        return Ok(None);
    };
    let authority: crate::commands::CommandAuthority = serde_json::from_value(value)?;
    match authority {
        crate::commands::CommandAuthority::McpImport(source) => match *source {
            super::ImportCommand::Existing(source) => {
                if !source.matches(&row.command)? {
                    return Err(EngineError::unreadable(
                        "the native import journal belongs to another copy",
                    ));
                }
                Ok(Some(source))
            }
            super::ImportCommand::Creation(_) => Ok(None),
        },
        _ => Err(EngineError::unreadable(
            "the native copy has another private command kind",
        )),
    }
}
async fn current<K: Kind>(
    conn: &mut Conn,
    origin: &CapturedOperation,
    caller: &Caller,
    input: &Input,
    clock: &dyn Clock,
) -> Result<Option<Caller>, EngineError> {
    let frame = match crate::operation_scope::continuing::prepare_with_caller::<Stored<K>>(
        conn,
        clock,
        origin,
        caller,
        input.clone(),
    )
    .await
    {
        Ok(frame) => frame,
        Err(CheckError::Refused | CheckError::Binding(_)) => return Ok(None),
        Err(CheckError::Native(error)) => return Err(error),
    };
    let caller = frame.principal().caller().clone();
    if frame.into_parts().first_effect.check(clock).is_err() {
        return Ok(None);
    }
    Ok(Some(caller))
}
