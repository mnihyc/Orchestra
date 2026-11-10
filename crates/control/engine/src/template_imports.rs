//! The selected native template proposal and its exact original archive sources.

use crate::operation_scope::{
    blob_sources,
    continuing::{self, Error, Source},
    sources::{self, Facts},
    targets,
};
use crate::{EngineError, commands::Outgoing};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, ResourceKind},
};
use orch_api_contract::{
    operation_sources::Declaration,
    resources::TemplateImport,
    routes::{Endpoint, Route},
    template_import_sources::{self as declared, Source as Role},
};
use orch_authz::{
    Caller, CapturedOperation, Policy, Principal,
    ownership::{self, Making},
    usable::{self, Policies, Usable},
};
use orch_chat::following::BlobOrigin;
use orch_db::{
    Conn, DbError,
    queries::{
        link::{CommandRow, NewCommand},
        resources,
    },
};
use orch_driver_contract::imports::{ImportDef, archive_references};
use orch_link_contract::commands::{
    CommandKind, NodeTemplateImport, NodeTemplateImportResult, RuntimeKind, SerialKey,
};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};

const TEMPLATE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Template);

/// One selected Host/driver and normalized input; none of these facts is authority.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    home: Option<Id>,
    host: Id,
    machine: Id,
    descriptor: Json,
    command: NodeTemplateImport,
}
impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TemplateImportInput([native])")
    }
}
impl Input {
    /// Freeze only the actual native selector's normalized proposal.
    pub fn new(
        home: Option<Id>,
        host: Id,
        machine: Id,
        descriptor: Json,
        command: NodeTemplateImport,
    ) -> Self {
        Self {
            home,
            host,
            machine,
            descriptor,
            command,
        }
    }
    pub fn home(&self) -> Option<Id> {
        self.home
    }
    pub fn host(&self) -> Id {
        self.host
    }
    pub fn machine(&self) -> Id {
        self.machine
    }
    pub fn command(&self) -> &NodeTemplateImport {
        &self.command
    }
    pub fn schema(&self) -> &Json {
        &self.descriptor["spec_schemas"][self.kind()]
    }
    fn kind(&self) -> &'static str {
        match self.command.kind {
            RuntimeKind::Environment => "environment",
            RuntimeKind::Provider => "provider",
        }
    }
    /// Only actual normalized archive fields confer a retained-byte source.
    pub fn archives(&self) -> Result<Vec<Hash>, EngineError> {
        let mut hashes: Vec<_> = archive_references(&self.command.values)
            .map_err(|_| EngineError::unreadable("the native proposal has invalid archive fields"))?
            .into_iter()
            .map(|archive| archive.hash)
            .collect();
        hashes.sort_unstable();
        hashes.dedup();
        Ok(hashes)
    }
}

/// The existing restricted-driver intent before partial HTTP and native preparation.
pub async fn restricted(conn: &mut Conn, driver: &str) -> Result<bool, EngineError> {
    let drivers: Vec<String> = get(conn, Place::System, keys::TEMPLATES_RESTRICTED_DRIVERS)
        .await?
        .unwrap_or_default();
    Ok(drivers.iter().any(|name| name == driver))
}

/// Real non-byte operands; unresolved roles are not fabricated empty here.
pub async fn known_facts(
    conn: &mut Conn,
    home: Option<Id>,
    driver: &str,
    host: Option<Id>,
) -> Result<Vec<Facts<Role>>, EngineError> {
    let mut facts = vec![
        Facts::one(
            Role::Home,
            TEMPLATE,
            match home {
                Some(home) => targets::node(conn, home)
                    .await
                    .map_err(EngineError::unreadable)?,
                None => targets::system(),
            },
        ),
        Facts::collection(
            Role::Administration,
            if restricted(conn, driver).await? {
                vec![(TEMPLATE, vec![targets::system()])]
            } else {
                Vec::new()
            },
        ),
    ];
    if let Some(host) = host {
        facts.push(Facts::one(
            Role::Host,
            KeyTargetKind::Object(ObjectKind::Host),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: host,
                },
            )
            .await
            .map_err(EngineError::unreadable)?,
        ));
    }
    Ok(facts)
}
async fn complete_facts(
    conn: &mut Conn,
    caller: &Caller,
    input: &Input,
) -> Result<Vec<Facts<Role>>, EngineError> {
    let hashes = input.archives()?;
    let mut facts = known_facts(conn, input.home, &input.command.driver, Some(input.host)).await?;
    for (source, _, family) in declared::CONTENT {
        facts.push(Facts::collection(
            source,
            if source == Role::Uploads && !hashes.is_empty() {
                vec![(
                    family,
                    vec![
                        targets::account(conn, caller.user())
                            .await
                            .map_err(EngineError::unreadable)?,
                    ],
                )]
            } else {
                Vec::new()
            },
        ));
    }
    Ok(facts)
}

/// Same factual source on HTTP, queue insertion, native delivery and every archive frame.
pub struct Proposal;
impl Source for Proposal {
    type Role = Role;
    type Input = Input;
    type Value = Input;
    const ROUTE: Route = TemplateImport::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declared::IMPORT;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Input, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let host = resources::routes::hosts(conn, &[input.host], caller.user())
                .await?
                .pop()
                .ok_or(Error::Refused)?;
            if host.machine_id != input.machine
                || host.drivers.get(&input.command.driver) != Some(&input.descriptor)
                || resources::machine(conn, input.machine)
                    .await?
                    .is_none_or(|row| !row.is_active())
            {
                return Err(Error::Refused);
            }
            let import: ImportDef = serde_json::from_value(
                input
                    .descriptor
                    .get("imports")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .find(|format| format["format"] == input.command.format)
                    .cloned()
                    .ok_or(Error::Refused)?,
            )
            .map_err(|_| Error::Refused)?;
            if input.schema().is_null() || import.check_values(&input.command.values).is_err() {
                return Err(Error::Refused);
            }
            Ok((input.clone(), complete_facts(conn, caller, input).await?))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _value: &'a Input,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let (members, policy) = match input.home {
                Some(home) => {
                    let words: Vec<String> =
                        get(conn, Place::Node(home), keys::RESOURCES_CREATED_BY_MEMBERS)
                            .await
                            .map_err(EngineError::from)?
                            .unwrap_or_default();
                    (
                        words
                            .iter()
                            .filter_map(|word| word.parse::<ResourceKind>().ok())
                            .collect::<Vec<_>>(),
                        crate::setting_operations::policy_at(conn, home).await?,
                    )
                }
                None => (Vec::new(), Policy::default()),
            };
            if !matches!(
                ownership::may_make(
                    conn,
                    principal,
                    ResourceKind::Template,
                    input.home,
                    &members,
                    policy
                )
                .await?,
                Making::Allowed(_)
            ) || restricted(conn, &input.command.driver).await?
                && !orch_authz::system_administration::on_system(principal, TEMPLATE)
                || !matches!(
                    usable::usable(
                        conn,
                        principal,
                        input.host,
                        input.home,
                        &Policies::default()
                    )
                    .await?,
                    Usable::Yes(_)
                )
            {
                return Err(Error::Refused);
            }
            let hashes = input.archives()?;
            let facts =
                known_facts(conn, input.home, &input.command.driver, Some(input.host)).await?;
            let plan = sources::bind_known(Self::ROUTE, Self::DECLARATION, &facts)?;
            let selected: Vec<_> = hashes
                .iter()
                .map(|hash| (*hash, BlobOrigin::RetainedByCaller))
                .collect();
            blob_sources::prepare_selected(conn, principal, &plan, &hashes, &[], &selected)
                .await
                .map_err(|error| match error {
                    blob_sources::Error::Missing => Error::Refused,
                    error => Error::Native(EngineError::unreadable(error.to_string())),
                })?;
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _value: &'a Input,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if let Some(home) = input.home {
                sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR SHARE")
                    .bind(home)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR SHARE")
                .bind(input.host)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                .bind(input.machine)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let hashes: Vec<Vec<u8>> = input
                .archives()?
                .iter()
                .map(|hash| hash.as_bytes().to_vec())
                .collect();
            sqlx::query("SELECT user_id FROM completed_uploads WHERE user_id=$1 AND hash=ANY($2) ORDER BY hash FOR SHARE")
                .bind(principal.user()).bind(hashes).fetch_all(conn).await.map_err(DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        old: &'a Input,
        fresh: &'a Input,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old == fresh) })
    }
}

/// Exact original native proposal, kept only in the existing private command channel.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    id: Id,
    origin: CapturedOperation,
    session: Option<Id>,
    until: Option<Time>,
    input: Input,
    canonical: Json,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TemplateImportCommand([private])")
    }
}
impl Command {
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        session: Option<Id>,
        until: Option<Time>,
        input: Input,
        command: &Outgoing,
        now: Time,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let (_, facts) = Proposal::prepare(conn, caller, &input, now)
            .await
            .map_err(source_error)?;
        let required = sources::bind_required(TemplateImport::ROUTE, &declared::IMPORT, &facts)
            .map_err(EngineError::unreadable)?
            .required_facets()
            .map_err(EngineError::unreadable)?;
        if origin.operation() != declared::IMPORT.operation
            || session.is_some() == caller.grant().is_some()
            || session.is_some() && until.is_none()
            || !required
                .iter()
                .all(|fact| caller.admitted_facets().unwrap_or(&[]).contains(fact))
            || command.kind != CommandKind::NodeTemplateImport
            || command.machine != input.machine
            || command.key != SerialKey::Machine
            || command.body != serde_json::to_value(&input.command)?
        {
            return Err(EngineError::unreadable(
                "the proposal has another original native source",
            ));
        }
        Ok(Self {
            id: command.id,
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
    async fn source_callers(
        &self,
        conn: &mut Conn,
        ceiling: Option<&Caller>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
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
        if self.session.is_some() == caller.grant().is_some()
            || self.session.is_some() && self.until.is_none()
        {
            return Ok(None);
        }
        let extra = ceiling.unwrap_or(&caller);
        let caller = extra.clone().with_admin_mode(
            extra.as_admin() && caller.as_admin() && session.is_some_and(|(admin, _)| admin),
        );
        let frame = match continuing::prepare_with_caller::<Proposal>(
            conn,
            clock,
            &self.origin,
            &caller,
            self.input.clone(),
        )
        .await
        {
            Ok(frame) => frame,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        let caller = frame.principal().caller().clone();
        if frame.into_parts().first_effect.check(clock).is_err() {
            return Ok(None);
        }
        let until = match (self.until, session.map(|(_, until)| until)) {
            (Some(old), Some(now)) => Some(old.min(now)),
            (old, now) => old.or(now),
        };
        Ok((caller.key_deadline_allows(clock)
            && until.is_none_or(|until| clock.now() < until.get()))
        .then_some(vec![(Some(caller), until)]))
    }
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.id != self.id
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
        self.source_callers(conn, None, clock).await
    }
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue_callers(
            conn,
            &NewCommand {
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
    /// The native result's compact association witness; no archive or stored content is copied.
    pub fn result_hash(result: &NodeTemplateImportResult) -> Result<Hash, EngineError> {
        crate::jobs::command_identity::hash(&serde_json::to_value(result)?)
    }
    /// Final publication retains the same actual native journal and completed typed result.
    pub async fn current_completed(
        &self,
        conn: &mut Conn,
        caller: &Caller,
        clock: &dyn Clock,
        expected: Hash,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let Some(callers) = self.source_callers(conn, Some(caller), clock).await? else {
            return Ok(None);
        };
        let Some(stored) =
            orch_db::queries::link::lock_command_with_authority(conn, self.id).await?
        else {
            return Ok(None);
        };
        if stored.command.state != "succeeded"
            || stored.authority
                != Some(serde_json::to_value(
                    crate::commands::CommandAuthority::template_import(self.clone()),
                )?)
            || !orch_db::queries::link::command_authority_required(conn, self.id).await?
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
        let Ok(result) = serde_json::from_value::<NodeTemplateImportResult>(result) else {
            return Ok(None);
        };
        if Self::result_hash(&result)? != expected
            || !callers.iter().all(|(caller, until)| {
                caller
                    .as_ref()
                    .is_none_or(|caller| caller.key_deadline_allows(clock))
                    && until.is_none_or(|until| clock.now() < until.get())
            })
        {
            return Ok(None);
        }
        Ok(Some(callers))
    }
    /// Accepted execution still needs the same original source for each new archive byte.
    pub async fn current_bytes(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        hash: &Hash,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if !self.input.archives()?.contains(hash) {
            return Ok(None);
        }
        self.current_callers(conn, command, clock).await
    }
}
pub fn source_error(error: Error) -> EngineError {
    match error {
        Error::Native(error) => error,
        error => EngineError::unreadable(error.to_string()),
    }
}
