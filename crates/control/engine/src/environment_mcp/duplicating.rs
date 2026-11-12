//! An exact stored MCP snapshot is another closed source of the existing native import channel.
use crate::{
    EngineError,
    commands::{CommandAuthority, Outgoing},
    operation_scope::{
        continuing::{Error, Source},
        http_origin::HttpOrigin,
        sources::{self, Facts},
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    audit::AuditAction,
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission, ResourceKind},
};
use orch_api_contract::{
    duplicate_sources::{MCP, MCP_MOVE, Source as Role},
    duplicates::{McpServerDuplicate, McpServerMoveOut},
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{
    Caller, CapturedOperation, Policy, Principal,
    ownership::{self, Making, NewAccess},
};
use orch_db::{
    Conn, DbError,
    queries::{
        hierarchy,
        link::{self, CommandRow, NewCommand},
        resources::{
            self, grouping,
            mcp::{self as rows, McpRow},
        },
    },
};
use orch_link_contract::{
    commands::{
        MachineKey, McpImport, McpImportResult, McpSnapshotTransfer, McpStoredCopy, SerialKey,
    },
    policy::LocalPolicy,
};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;

fn refused() -> EngineError {
    EngineError::unreadable("The original stored MCP duplicate source changed.")
}
pub(super) fn fingerprint(row: &McpRow) -> Result<Hash, EngineError> {
    crate::jobs::command_identity::hash(
        &json!({"id":row.id,"home":row.home_id,"org":row.org_id,"environment":row.environment_id,"machine":row.machine_id,"owner":row.owner_id,"imported":row.imported,"revision":row.revision,"name":row.name,"description":row.description,"spec":row.spec}),
    )
}
fn target_error(error: targets::TargetError) -> Error {
    match error {
        targets::TargetError::Missing => Error::Refused,
        targets::TargetError::Db(error) => error.into(),
        targets::TargetError::Authz(error) => error.into(),
    }
}
fn subject(row: &McpRow) -> ObjectRef {
    ObjectRef {
        kind: ObjectKind::McpServer,
        id: row.id,
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CopiedDefinition {
    fingerprint: Option<Hash>,
    transport: String,
    digest: String,
    tools: Option<i64>,
}
impl CopiedDefinition {
    fn of(result: &McpImportResult) -> Self {
        Self {
            fingerprint: result.definition_fingerprint,
            transport: result.transport.clone(),
            digest: result.digest.clone(),
            tools: result.tools,
        }
    }
}
/// Actual source snapshot, new native identity and creation place; none of these facts grants access.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    source: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ending: Option<Box<super::deleting::Deletion>>,
    source_fingerprint: Hash,
    machine_owner: Option<Id>,
    id: Id,
    machine: Id,
    home: Option<Id>,
    org: Option<Id>,
    name: String,
    description: String,
    tags: Vec<Id>,
    command: McpImport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_machine: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recipient: Option<MachineKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recipient_owner: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    issued: Option<Box<Command>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    copied: Option<CopiedDefinition>,
}
impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredMcpDuplicateInput([native])")
    }
}
impl Input {
    /// The native HTTP preparer supplies its real source, actual chosen home and current owner choice.
    pub async fn capture(
        conn: &mut Conn,
        row: &McpRow,
        home: Option<Id>,
        org: Option<Id>,
        name: String,
        include_secrets: bool,
        target_machine: Option<Id>,
    ) -> Result<Self, EngineError> {
        if !row.imported
            || row.environment_id.is_some()
            || name.len() < 2
            || name.len() > 32
            || !name.starts_with(|c: char| c.is_ascii_lowercase())
            || !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
        {
            return Err(refused());
        }
        let machine = row.machine_id.ok_or_else(refused)?;
        let native = resources::machine(conn, machine)
            .await?
            .ok_or_else(refused)?;
        let target = target_machine.unwrap_or(machine);
        let recipient = if target != machine {
            let target = resources::machine(conn, target)
                .await?
                .filter(|target| target.is_active())
                .ok_or_else(refused)?;
            let key = target.link().public_key;
            if key.is_empty() {
                return Err(refused());
            }
            Some((
                MachineKey {
                    machine_id: target.id,
                    public_key: key,
                },
                target.owner_id,
            ))
        } else {
            None
        };
        let digest = row.spec["imported"]["digest"]
            .as_str()
            .ok_or_else(refused)?
            .to_owned();
        let original = &row.spec["imported"];
        Ok(Self {
            source: row.id,
            ending: None,
            source_fingerprint: fingerprint(row)?,
            machine_owner: native.owner_id,
            source_machine: recipient.as_ref().map(|_| machine),
            recipient_owner: recipient.as_ref().and_then(|(_, owner)| *owner),
            recipient: recipient.as_ref().map(|(key, _)| key.clone()),
            issued: None,
            copied: None,
            id: orch_types::new_id(),
            machine,
            home,
            org,
            name,
            description: row.description.clone(),
            tags: if row.org_id == org {
                grouping::tag_ids(conn, ResourceKind::McpServer, row.id).await?
            } else {
                Vec::new()
            },
            command: McpImport {
                expected_fingerprint: None,
                environment_id: None,
                transfer: recipient.map(|(recipient, _)| McpSnapshotTransfer::Issue { recipient }),
                stored: Some(McpStoredCopy {
                    server_id: row.id,
                    expected_digest: digest,
                    include_secrets,
                }),
                server_id: orch_types::new_id(),
                source: original["source"].as_str().unwrap_or("stored").into(),
                scope: original["scope"].as_str().unwrap_or("user").into(),
                name: original["name"].as_str().unwrap_or(&row.name).into(),
                directory: original["directory"].as_str().map(str::to_owned),
            },
        })
    }
    /// Retain the same actual native source deletion in this original copy act.
    pub fn with_ending(mut self, ending: super::deleting::Deletion) -> Result<Self, EngineError> {
        if ending.server() != self.source || !ending.moving() || self.ending.is_some() {
            return Err(refused());
        }
        self.ending = Some(Box::new(ending));
        Ok(self)
    }
    pub fn moving(&self) -> bool {
        self.ending.is_some()
    }
    pub fn server(&self) -> Id {
        self.command.server_id
    }
    pub fn id(&self) -> Id {
        self.id
    }
    pub fn machine(&self) -> Id {
        self.machine
    }
    pub fn is_issue(&self) -> bool {
        matches!(
            self.command.transfer,
            Some(McpSnapshotTransfer::Issue { .. })
        )
    }
    fn source_machine(&self) -> Id {
        self.source_machine.unwrap_or(self.machine)
    }
    fn target_machine(&self) -> Id {
        self.recipient
            .as_ref()
            .map_or(self.machine, |recipient| recipient.machine_id)
    }
    /// The existing native machine Use policy evaluated at the actual destination work.
    pub async fn destination_allowed(
        &self,
        conn: &mut Conn,
        principal: &Principal,
    ) -> Result<bool, Error> {
        machine_use(conn, principal, self.target_machine(), self.home).await
    }
    pub fn serial(&self) -> SerialKey {
        SerialKey::Machine
    }
    pub fn command(&self) -> &McpImport {
        &self.command
    }
    pub fn preserves_request(
        &self,
        source: Id,
        home: Option<Id>,
        name: &str,
        target: Option<Id>,
    ) -> bool {
        self.source == source
            && self.home == home
            && self.name == name
            && self.target_machine() == target.unwrap_or(self.source_machine())
    }
}
async fn machine_use(
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
    home: Option<Id>,
) -> Result<bool, Error> {
    let resource = resources::hosting::host_of_machine(conn, machine)
        .await?
        .unwrap_or(machine);
    let policies = crate::access::resource_policies(conn, resource).await?;
    Ok(matches!(
        orch_authz::usable::usable(conn, principal, resource, home, &policies).await?,
        orch_authz::usable::Usable::Yes(_)
    ))
}
/// The real machine Use/work supplier precedes private recipient facts or public validation causes.
pub async fn machine_allowed(
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
    home: Option<Id>,
) -> Result<bool, Error> {
    let used = match machine_use(conn, principal, machine, home).await {
        Ok(value) => value,
        Err(Error::Native(EngineError::Authz(orch_authz::Error::NotFound(_)))) => false,
        Err(error) => return Err(error),
    };
    if !used {
        return Ok(false);
    }
    let Some(machine) = resources::machine(conn, machine).await?.filter(|machine| {
        machine.is_active() && machine.roles.iter().any(|role| role == "environment")
    }) else {
        return Ok(false);
    };
    let policy: LocalPolicy = machine
        .facts
        .get("policy")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(EngineError::from)?
        .unwrap_or_default();
    Ok(policy.mcp_import)
}
/// Shared HTTP and native current-source producer for this one stored snapshot.
pub struct Stored;
impl Source for Stored {
    type Role = Role;
    type Input = Input;
    type Value = McpRow;
    const ROUTE: Route = McpServerDuplicate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &MCP;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        _: Time,
    ) -> BoxFuture<'a, Result<(McpRow, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = rows::servers(conn, &[input.source])
                .await?
                .pop()
                .ok_or(Error::Refused)?;
            let copy = input.command.stored.as_ref().ok_or(Error::Refused)?;
            let origin = &row.spec["imported"];
            if copy.server_id != row.id
                || Some(copy.expected_digest.as_str()) != origin["digest"].as_str()
                || input.command.server_id == row.id
                || input.command.environment_id.is_some()
                || input.command.expected_fingerprint.is_some()
                || input.command.source != origin["source"].as_str().unwrap_or("stored")
                || input.command.scope != origin["scope"].as_str().unwrap_or("user")
                || input.command.name != origin["name"].as_str().unwrap_or(&row.name)
                || input.command.directory.as_deref() != origin["directory"].as_str()
            {
                return Err(Error::Refused);
            }
            match (&input.command.transfer, &input.recipient, &input.issued) {
                (None, None, None)
                    if input.machine == input.source_machine() && input.copied.is_none() => {}
                (Some(McpSnapshotTransfer::Issue { recipient }), Some(expected), None)
                    if recipient == expected
                        && input.machine == input.source_machine()
                        && input.source_machine() != expected.machine_id
                        && input.copied.is_none() => {}
                (Some(McpSnapshotTransfer::Receive { recipient, .. }), Some(expected), Some(_))
                    if *recipient == expected.machine_id
                        && input.machine == expected.machine_id
                        && input.source_machine() != expected.machine_id
                        && input.copied.is_some() => {}
                _ => return Err(Error::Refused),
            }
            let machine = resources::machine(conn, input.source_machine())
                .await?
                .filter(|machine| {
                    machine.is_active() && machine.roles.iter().any(|role| role == "environment")
                })
                .ok_or(Error::Refused)?;
            let policy: LocalPolicy = machine
                .facts
                .get("policy")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(EngineError::from)?
                .unwrap_or_default();
            if !row.imported
                || row.environment_id.is_some()
                || row.machine_id != Some(input.source_machine())
                || fingerprint(&row)? != input.source_fingerprint
                || machine.owner_id != input.machine_owner
                || !policy.mcp_import
                || rows::recorded(conn, input.server()).await?
            {
                return Err(Error::Refused);
            }
            if let Some(recipient) = &input.recipient {
                let target = resources::machine(conn, recipient.machine_id)
                    .await?
                    .filter(|target| {
                        target.is_active() && target.roles.iter().any(|role| role == "environment")
                    })
                    .ok_or(Error::Refused)?;
                let policy: LocalPolicy = target
                    .facts
                    .get("policy")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(EngineError::from)?
                    .unwrap_or_default();
                if target.link().public_key != recipient.public_key
                    || target.owner_id != input.recipient_owner
                    || !policy.mcp_import
                {
                    return Err(Error::Refused);
                }
            }
            if let Some(issued) = &input.issued {
                if issued.origin.user() != caller.user()
                    || !issued.duplicate.is_issue()
                    || issued.duplicate.issued.is_some()
                {
                    return Err(Error::Refused);
                }
                let fresh = issued.received(conn).await?;
                if fresh != *input {
                    return Err(Error::Refused);
                }
            } else if input.recipient.is_some() && !input.is_issue() {
                return Err(Error::Refused);
            }
            let home = match input.home {
                Some(id) => {
                    let place = hierarchy::node(conn, id)
                        .await?
                        .filter(|place| place.kind == "org" && Some(place.org_id) == input.org)
                        .ok_or(Error::Refused)?;
                    targets::node(conn, place.id).await.map_err(target_error)?
                }
                None if input.org.is_none() => targets::system(),
                None => return Err(Error::Refused),
            };
            let source = targets::page_object(conn, subject(&row))
                .await
                .map_err(target_error)?;
            Ok((
                row,
                vec![
                    Facts::one(
                        Role::Subject,
                        KeyTargetKind::Object(ObjectKind::McpServer),
                        source,
                    ),
                    Facts::one(
                        Role::Destination,
                        KeyTargetKind::Object(ObjectKind::McpServer),
                        home,
                    ),
                    Facts::collection(Role::DestinationEnvironments, vec![]),
                    Facts::collection(
                        Role::Machine,
                        vec![(
                            KeyTargetKind::Object(ObjectKind::Machine),
                            vec![
                                targets::machine_subject(conn, input.target_machine())
                                    .await
                                    .map_err(target_error)?,
                            ],
                        )],
                    ),
                ],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        row: &'a McpRow,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            crate::copy_out::require_continuing(
                conn,
                principal,
                crate::copy_out::Subject::Object(ObjectRef {
                    kind: ObjectKind::McpServer,
                    id: row.id,
                }),
                input.home,
            )
            .await?;
            if !orch_authz::resources::check(
                conn,
                principal,
                subject(row),
                Permission::McpView,
                Policy::default(),
            )
            .await?
            .is_allowed()
                || !machine_use(conn, principal, input.source_machine(), row.home_id).await?
                || !machine_use(conn, principal, input.target_machine(), input.home).await?
                || !matches!(
                    ownership::may_make_here(conn, principal, ResourceKind::McpServer, input.home)
                        .await?,
                    Making::Allowed(_)
                )
            {
                return Err(Error::Refused);
            }
            if input
                .command
                .stored
                .as_ref()
                .is_some_and(|copy| copy.include_secrets)
                && !orch_authz::resources::effective(
                    conn,
                    principal,
                    subject(row),
                    Policy::default(),
                )
                .await?
                .owner
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        _: &'a McpRow,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            rows::lock_definition_for_command(conn, input.server(), Some(input.id)).await?;
            sqlx::query("SELECT id FROM mcp_servers WHERE id=$1 AND deleted_at IS NULL FOR SHARE")
                .bind(input.source)
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            if let Some(home) = input.home {
                sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR SHARE")
                    .bind(home)
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            let mut machines = vec![input.source_machine(), input.target_machine()];
            machines.sort_unstable();
            machines.dedup();
            sqlx::query("SELECT id FROM machines WHERE id=ANY($1) ORDER BY id FOR SHARE")
                .bind(machines)
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        old: &'a McpRow,
        new: &'a McpRow,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(fingerprint(old)? == fingerprint(new)?) })
    }
}
/// The original native stored-copy source under the actual Move operation and deletion roles.
pub struct Moving;
impl Source for Moving {
    type Role = Role;
    type Input = Input;
    type Value = McpRow;
    const ROUTE: Route = McpServerMoveOut::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &MCP_MOVE;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        now: Time,
    ) -> BoxFuture<'a, Result<(McpRow, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let ending = input.ending.as_ref().ok_or(Error::Refused)?;
            if ending.server() != input.source
                || !ending.moving()
                || ending.user() != caller.user()
                || !ending.matches_source(conn).await?
            {
                return Err(Error::Refused);
            }
            let (row, mut facts) = Stored::prepare(conn, caller, input, now).await?;
            let mut targets = Vec::new();
            for machine in ending.copy_machines() {
                targets.push(
                    targets::machine_subject(conn, machine)
                        .await
                        .map_err(target_error)?,
                );
            }
            facts.push(Facts::collection(Role::SourceEnvironments, vec![]));
            facts.push(Facts::collection(
                Role::ForgottenCopies,
                if targets.is_empty() {
                    vec![]
                } else {
                    vec![(KeyTargetKind::Object(ObjectKind::Machine), targets)]
                },
            ));
            Ok((row, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        row: &'a McpRow,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            Stored::require(conn, principal, input, row).await?;
            if !orch_authz::resources::effective(conn, principal, subject(row), Policy::default())
                .await?
                .owner
                || !orch_authz::resources::check(
                    conn,
                    principal,
                    subject(row),
                    Permission::McpManage,
                    Policy::default(),
                )
                .await?
                .is_allowed()
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        row: &'a McpRow,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            rows::lock_definition(conn, input.source).await?;
            Stored::locks(conn, principal, input, row).await
        })
    }
    fn check_opened<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        old: &'a McpRow,
        new: &'a McpRow,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Stored::check_opened(conn, principal, input, old, new)
    }
}
/// The actual admitted duplicate and canonical native copy in the existing private import channel.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    duplicate: Input,
    origin: CapturedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    http: Option<HttpOrigin>,
    as_admin: bool,
    canonical: Json,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredMcpDuplicateCommand([private])")
    }
}
impl Command {
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        input: Input,
        command: &Outgoing,
        raw: &HttpOrigin,
        now: Time,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let raw = match input.issued.as_ref() {
            Some(issued) => issued
                .http
                .as_ref()
                .ok_or_else(refused)?
                .intersect(raw)
                .map_err(EngineError::unreadable)?,
            None => raw.clone(),
        }
        .with_ceiling(
            caller,
            caller.grant().and_then(orch_authz::TokenGrant::expires_at),
        )
        .map_err(EngineError::unreadable)?;
        let (route, declaration) = if input.moving() {
            (Moving::ROUTE, Moving::DECLARATION)
        } else {
            (Stored::ROUTE, Stored::DECLARATION)
        };
        if raw.origin().user() != caller.user() || raw.origin().operation() != declaration.operation
        {
            return Err(refused());
        }
        let (_, facts) = if input.moving() {
            Moving::prepare(conn, caller, &input, now).await
        } else {
            Stored::prepare(conn, caller, &input, now).await
        }
        .map_err(EngineError::unreadable)?;
        let expected = sources::bind_required(route, declaration, &facts)
            .map_err(EngineError::unreadable)?
            .required_facets()
            .map_err(EngineError::unreadable)?;
        if origin.operation() != declaration.operation
            || !expected
                .iter()
                .all(|fact| caller.admitted_facets().unwrap_or(&[]).contains(fact))
            || command.id != input.id
            || command.machine != input.machine
            || command.key != input.serial()
            || command.kind.as_str() != "mcp.import"
            || command.body != serde_json::to_value(&input.command)?
        {
            return Err(refused());
        }
        Ok(Self {
            duplicate: input,
            origin,
            http: Some(raw),
            as_admin: caller.as_admin(),
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
    /// Build the receive operand only from this exact genuine native read result.
    pub async fn received(&self, conn: &mut Conn) -> Result<Input, EngineError> {
        if !self.duplicate.is_issue() || self.duplicate.issued.is_some() {
            return Err(refused());
        }
        let recipient = self.duplicate.recipient.as_ref().ok_or_else(refused)?;
        let result = self.completed(conn).await?.ok_or_else(refused)?;
        let copied = CopiedDefinition::of(&result);
        let sealed = result.snapshot.ok_or_else(refused)?;
        let mut input = self.duplicate.clone();
        input.id = crate::ids::derived("mcp.duplicate.receive", &[self.duplicate.id.as_bytes()]);
        input.machine = recipient.machine_id;
        input.command.transfer = Some(McpSnapshotTransfer::Receive {
            recipient: recipient.machine_id,
            snapshot: sealed,
        });
        input.issued = Some(Box::new(self.clone()));
        input.copied = Some(copied);
        Ok(input)
    }
    pub fn input(&self) -> &Input {
        &self.duplicate
    }
    pub(crate) fn matches(&self, row: &CommandRow) -> Result<bool, EngineError> {
        Ok(row.id == self.duplicate.id
            && row.kind == "mcp.import"
            && crate::jobs::command_identity::canonical(
                row.id,
                row.machine_id,
                row.serial_key(),
                &row.kind,
                &row.body,
            )? == self.canonical)
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        rows::lock_definition_for_command(conn, self.duplicate.server(), Some(self.duplicate.id))
            .await?;
        Ok(())
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.id != self.duplicate.id
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
        let Some(raw) = &self.http else {
            return Ok(None);
        };
        if self.duplicate.moving() {
            self.current_source::<Moving>(conn, clock, raw).await
        } else {
            self.current_source::<Stored>(conn, clock, raw).await
        }
    }
    async fn current_source<F: Source<Input = Input, Value = McpRow>>(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        raw: &HttpOrigin,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let frame = match raw.prepare::<F>(conn, clock, self.duplicate.clone()).await {
            Ok(frame) => frame,
            Err(Error::Native(error)) => return Err(error),
            Err(_) => return Ok(None),
        };
        let until = frame.http_origin().and_then(HttpOrigin::until);
        let caller = frame.principal().caller().clone();
        if frame.into_parts().first_effect.check(clock).is_err() {
            return Ok(None);
        }
        Ok(Some(vec![(Some(caller), until)]))
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        row: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue_callers(
            conn,
            &NewCommand {
                id: row.id,
                machine_id: row.machine_id,
                kind: &row.kind,
                body: &row.body,
                serial_key: row.serial_key(),
                deadline_at: row.deadline_at,
            },
            clock,
        )
        .await
    }
    /// Data-only actual accepted success, including its sealed HTTP origin; no old actor is restored.
    pub(super) async fn completed(
        &self,
        conn: &mut Conn,
    ) -> Result<Option<McpImportResult>, EngineError> {
        let Some(row) = link::command_with_authority(conn, self.duplicate.id).await? else {
            return Ok(None);
        };
        let Some(value) = row.authority else {
            return Ok(None);
        };
        let actual: CommandAuthority = serde_json::from_value(value)?;
        if !actual.matches_native_receipt(
            &CommandAuthority::mcp_duplicate_import(self.clone()),
            &row.command,
        )? || !link::command_authority_required(conn, self.duplicate.id).await?
        {
            return Ok(None);
        }
        let Some(result) = row
            .command
            .result
            .and_then(|result| serde_json::from_value::<McpImportResult>(result).ok())
        else {
            return Ok(None);
        };
        if self.duplicate.is_issue() {
            if result.snapshot.is_none() {
                return Ok(None);
            }
        } else {
            if result.snapshot.is_some() {
                return Ok(None);
            }
            if self
                .duplicate
                .copied
                .as_ref()
                .is_some_and(|expected| *expected != CopiedDefinition::of(&result))
            {
                return Ok(None);
            }
        }
        Ok(Some(result))
    }
    /// File this exact completed creation once, even if its old HTTP waiter ended.
    pub async fn reconcile(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<super::existing::Completion, EngineError> {
        if self.duplicate.is_issue() {
            return Err(EngineError::unreadable(
                "A sealed snapshot read is not a completed creation.",
            ));
        }
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(refused());
        }
        self.lock_source(conn).await?;
        let _row = link::lock_command_with_authority(conn, self.duplicate.id)
            .await?
            .ok_or_else(refused)?;
        let reply = self.completed(conn).await?.ok_or_else(refused)?;
        if let Some(old) = link::mcp_import_completion(conn, self.duplicate.id).await? {
            let old: super::existing::Completion = serde_json::from_value(old)?;
            if old.server_id != self.duplicate.server() {
                return Err(refused());
            }
            return Ok(old);
        }
        let input = &self.duplicate;
        if rows::recorded(conn, input.server()).await? {
            return Err(refused());
        }
        let home_live = match input.home {
            None => true,
            Some(id) => sqlx::query_scalar::<_, Id>(
                "SELECT id FROM nodes WHERE id=$1 AND deleted_at IS NULL FOR SHARE",
            )
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .is_some(),
        };
        let source_live = match &input.ending {
            Some(ending) => ending.matches_source(conn).await?,
            None => true,
        };
        // Copying bytes and ending the original are separate first-effect cuts. Keep the
        // same raw/current minima at the controller's atomic filing/ending boundary.
        let ending_deadline = if input.moving() && home_live && source_live {
            match &self.http {
                Some(raw) => match raw.prepare::<Moving>(conn, clock, input.clone()).await {
                    Ok(frame) => {
                        let until = frame.http_origin().and_then(HttpOrigin::until);
                        let parts = frame.into_parts();
                        let principal = parts.principal;
                        parts
                            .first_effect
                            .check(clock)
                            .ok()
                            .map(|()| (principal, until))
                    }
                    Err(Error::Native(error)) => return Err(error),
                    Err(_) => None,
                },
                None => None,
            }
        } else {
            None
        };
        if !home_live || !source_live || input.moving() && ending_deadline.is_none() {
            let id = crate::ids::derived("mcp.duplicate.inverse", &[input.id.as_bytes()]);
            let mut cleanup = Outgoing::new(
                id,
                input.machine,
                SerialKey::Machine,
                &orch_link_contract::commands::McpForget {
                    server_id: input.server(),
                },
            )?;
            let proof =
                super::ForgetCommand::capture_duplicate(conn, clock, self.clone(), &cleanup)
                    .await?;
            cleanup.deadline = proof.deadline();
            let authority = serde_json::to_value(CommandAuthority::mcp_forget(proof))?;
            link::insert_command_with_authority(
                conn,
                &NewCommand {
                    id,
                    machine_id: cleanup.machine,
                    kind: cleanup.kind.as_str(),
                    body: &cleanup.body,
                    serial_key: cleanup.key.wire().as_deref(),
                    deadline_at: cleanup.deadline.ok_or_else(refused)?.get(),
                },
                clock.now(),
                Some(&authority),
            )
            .await?;
            let done = super::existing::Completion {
                server_id: input.server(),
                revision: None,
                selected: false,
                cleanup_queued: true,
            };
            if !link::finish_mcp_import(conn, input.id, &serde_json::to_value(&done)?).await? {
                return Err(refused());
            }
            return Ok(done);
        }
        let now = clock.now();
        let spec = super::ImportRecord {
            source: &input.command.source,
            scope: &json!(input.command.scope),
            directory: input.command.directory.as_deref(),
            name: &input.command.name,
        }
        .spec(reply, self.origin.user(), Time::new(now));
        let row = McpRow {
            imported: true,
            id: input.server(),
            name: input.name.clone(),
            description: input.description.clone(),
            org_id: input.org,
            home_id: input.home,
            owner_id: Some(self.origin.user()),
            machine_id: Some(input.machine),
            environment_id: None,
            spec,
            enabled: true,
            revision: 1,
        };
        rows::insert(conn, &row, now).await?;
        ownership::start_resource(
            conn,
            subject(&row),
            self.origin.user(),
            NewAccess::Defaults,
            Time::new(now),
        )
        .await?;
        if !input.tags.is_empty() {
            grouping::set_tags(conn, ResourceKind::McpServer, row.id, &input.tags, now).await?;
        }
        let mut audit = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(self.origin.user()),
            AuditAction::McpImported.as_str(),
            "mcp_server",
            Some(row.id),
            "Duplicated a stored MCP definition on its native machine",
        );
        audit.history = AuditAction::McpImported.is_history_line();
        audit.details =
            json!({"source":input.source,"command":input.id,"machine":input.machine,"copy":row.id});
        orch_db::audit::append(
            conn,
            &audit,
            &orch_db::audit::Origin {
                as_admin: self.as_admin.then_some(self.origin.user()),
                ..orch_db::audit::Origin::default()
            },
            now,
        )
        .await?;
        if let Some(boundary) = crate::copy_out::inspect(
            conn,
            crate::copy_out::Subject::Object(ObjectRef {
                kind: ObjectKind::McpServer,
                id: input.source,
            }),
            input.home,
        )
        .await
        .map_err(|error| match error {
            crate::copy_out::Error::Native(error) => error,
            _ => refused(),
        })? {
            crate::copy_out::record(
                conn,
                boundary,
                orch_db::audit::Actor::User(self.origin.user()),
                &orch_db::audit::Origin {
                    as_admin: self.as_admin.then_some(self.origin.user()),
                    ..Default::default()
                },
                row.id,
                now,
            )
            .await?;
        }
        if let Some(ending) = &input.ending {
            if ending_deadline.as_ref().is_none_or(|(principal, until)| {
                !principal.caller().key_deadline_allows(clock)
                    || until.is_some_and(|end| clock.time() >= end)
            }) {
                return Err(refused());
            }
            let principal = &ending_deadline.as_ref().ok_or_else(refused)?.0;
            ending.apply(conn, principal, clock).await?;
            if ending_deadline.as_ref().is_none_or(|(principal, until)| {
                !principal.caller().key_deadline_allows(clock)
                    || until.is_some_and(|end| clock.time() >= end)
            }) {
                return Err(refused());
            }
        }
        let done = super::existing::Completion {
            server_id: row.id,
            revision: Some(1),
            selected: true,
            cleanup_queued: false,
        };
        if !link::finish_mcp_import(conn, input.id, &serde_json::to_value(&done)?).await? {
            return Err(refused());
        }
        Ok(done)
    }
}
