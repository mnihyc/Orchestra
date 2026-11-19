//! Exact native search interest retained through query preparation and provider dispatch.
use crate::EngineError;
use crate::access::ProviderUse;
use crate::commands::Outgoing;
use crate::operation_scope::{
    continuing::{self, Error, Source},
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyTargetKind},
    permissions::{NodePermission, ObjectKind, Permission},
};
use orch_api_contract::{
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    search::SearchStreamList,
    search_sources::{self, Source as Role},
};
use orch_authz::{Act, Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn,
    queries::{chats, link::CommandRow, semantic as rows},
};
use orch_link_contract::commands::{CommandKind, SerialKey};
use orch_types::{Clock, Hash, Id, Time};
use serde::{Deserialize, Serialize};

/// One actual native interest per selected semantic org, never an old admission or result lease.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryOrigin {
    origin: CapturedOperation,
    org: Id,
    sources: Vec<KeyObjectRef>,
}
impl std::fmt::Debug for QueryOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QueryOrigin([private])")
    }
}
impl QueryOrigin {
    /// Only native selected facts reach this producer; ancestry is freshly loaded later.
    pub fn new(
        origin: CapturedOperation,
        org: Id,
        sources: Vec<KeyResolvedTarget>,
    ) -> Result<Self, EngineError> {
        if origin.operation() != search_sources::STREAM.operation || sources.is_empty() {
            return Err(EngineError::unreadable(
                "semantic query has no original native interest",
            ));
        }
        Ok(Self {
            origin,
            org,
            sources: sources.into_iter().map(|source| source.object).collect(),
        })
    }
    /// Retain the successful frame's complete original/current key intersection.
    pub fn narrowed_with(mut self, caller: &Caller) -> Result<Self, EngineError> {
        if caller.user() != self.origin.user()
            || caller.admitted_operation() != Some(search_sources::STREAM.operation)
        {
            return Err(EngineError::unreadable(
                "semantic source changed its accepted operation",
            ));
        }
        self.origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        Ok(self)
    }
    pub(super) fn work(&self, generation: &rows::GenerationRow, writer: Id) -> QueryWork {
        QueryWork {
            read: self.clone(),
            generation: Generation::of(generation),
            writer,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Generation {
    id: Id,
    org: Id,
    provider: Id,
    fingerprint: Vec<u8>,
    model: String,
    dimensions: Option<i32>,
    asked_dimensions: Option<i32>,
    query_prefix: String,
    document_prefix: String,
}
impl Generation {
    fn of(row: &rows::GenerationRow) -> Self {
        Self {
            id: row.id,
            org: row.org_id,
            provider: row.provider_id,
            fingerprint: row.provider_fingerprint.clone(),
            model: row.model_name.clone(),
            dimensions: row.dimensions,
            asked_dimensions: row.asked_dimensions,
            query_prefix: row.query_prefix.clone(),
            document_prefix: row.document_prefix.clone(),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueryWork {
    read: QueryOrigin,
    generation: Generation,
    writer: Id,
}
pub(super) struct QueryValue {
    targets: Vec<KeyResolvedTarget>,
    generation: Generation,
}
pub(super) struct QuerySource;
impl Source for QuerySource {
    type Role = Role;
    type Input = QueryWork;
    type Value = QueryValue;
    const ROUTE: Route = SearchStreamList::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &search_sources::STREAM;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a QueryWork,
        _now: Time,
    ) -> BoxFuture<'a, Result<(QueryValue, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            if input.read.org != input.generation.org || input.read.sources.is_empty() {
                return Err(Error::Refused);
            }
            let row = rows::generations(conn, input.read.org)
                .await?
                .into_iter()
                .find(|row| row.id == input.generation.id && row.state == "active")
                .ok_or(Error::Refused)?;
            let generation = Generation::of(&row);
            if generation != input.generation {
                return Err(Error::Refused);
            }
            let mut selected = Vec::new();
            for source in &input.read.sources {
                let id = source.id.ok_or(Error::Refused)?;
                let target = match source.kind {
                    KeyTargetKind::Identity(KeyIdentityKind::Chat) => targets::chat(conn, id).await,
                    KeyTargetKind::Object(kind)
                        if kind.node().is_some() || kind == ObjectKind::Document =>
                    {
                        targets::page_object(conn, ObjectRef { kind, id }).await
                    }
                    _ => return Err(Error::Refused),
                }
                .map_err(target_error)?;
                if target.object != *source
                    || !target
                        .node_ancestors
                        .iter()
                        .chain(std::iter::once(&target.object))
                        .any(|object| {
                            object.kind == KeyTargetKind::Object(ObjectKind::Org)
                                && object.id == Some(input.read.org)
                        })
                {
                    return Err(Error::Refused);
                }
                selected.push(target);
            }
            let mut groups = std::collections::BTreeMap::new();
            for target in &selected {
                groups
                    .entry(target.object.kind)
                    .or_insert_with(Vec::new)
                    .push(target.clone());
            }
            Ok((
                QueryValue {
                    targets: selected,
                    generation,
                },
                vec![Facts::collection(
                    Role::Results,
                    groups.into_iter().collect(),
                )],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a QueryWork,
        value: &'a QueryValue,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            for source in &value.targets {
                let id = source.object.id.ok_or(Error::Refused)?;
                let readable = match source.object.kind {
                    KeyTargetKind::Object(ObjectKind::Document) => orch_authz::documents::check(
                        conn,
                        principal,
                        id,
                        Permission::DocView,
                        Policy::default(),
                    )
                    .await?
                    .is_allowed(),
                    KeyTargetKind::Object(kind) => {
                        let permission = kind
                            .node()
                            .and_then(|kind| NodePermission::View.on(kind))
                            .ok_or(Error::Refused)?;
                        orch_authz::nodes::check(
                            conn,
                            principal,
                            id,
                            permission,
                            Act::Use,
                            Policy::default(),
                        )
                        .await?
                        .is_allowed()
                    }
                    KeyTargetKind::Identity(KeyIdentityKind::Chat) => {
                        let row = chats::chat(conn, id).await?.ok_or(Error::Refused)?;
                        orch_authz::chats::listed_access(conn, principal, &[row])
                            .await?
                            .contains_key(&id)
                    }
                    _ => return Err(Error::Refused),
                };
                if !readable {
                    return Err(Error::Refused);
                }
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a QueryWork,
        value: &'a QueryValue,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            sqlx::query("SELECT id FROM providers WHERE id=$1 FOR SHARE")
                .bind(input.generation.provider)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?
                .ok_or(Error::Refused)?;
            sqlx::query("SELECT id FROM search_generations WHERE id=$1 AND org_id=$2 FOR SHARE")
                .bind(input.generation.id)
                .bind(input.read.org)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?
                .ok_or(Error::Refused)?;
            let mut sources = value
                .targets
                .iter()
                .map(|target| target.object)
                .collect::<Vec<_>>();
            sources.sort_unstable_by_key(|source| (source.kind, source.id));
            for source in sources {
                let id = source.id.ok_or(Error::Refused)?;
                let sql = match source.kind {
                    KeyTargetKind::Object(ObjectKind::Document) => {
                        "SELECT id FROM documents WHERE id=$1 FOR SHARE"
                    }
                    KeyTargetKind::Identity(KeyIdentityKind::Chat) => {
                        "SELECT id FROM chats WHERE id=$1 FOR SHARE"
                    }
                    KeyTargetKind::Object(_) => "SELECT id FROM nodes WHERE id=$1 FOR SHARE",
                    _ => return Err(Error::Refused),
                };
                sqlx::query(sql)
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?
                    .ok_or(Error::Refused)?;
            }
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a QueryWork,
        old: &'a QueryValue,
        fresh: &'a QueryValue,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(
            async move { Ok(old.targets == fresh.targets && old.generation == fresh.generation) },
        )
    }
}
impl QueryWork {
    pub(super) async fn prepare(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<continuing::Frame<QuerySource>, EngineError> {
        continuing::prepare::<QuerySource>(conn, clock, &self.read.origin, self.clone())
            .await
            .map_err(frame_error)
    }
}
fn frame_error(error: Error) -> EngineError {
    match error {
        Error::Native(error) => error,
        other => EngineError::BeforeEffect(Box::new(other)),
    }
}

/// One canonical generated query, in the existing server-private durable command channel.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryAuthority {
    work: QueryWork,
    call: Id,
    machine: Id,
    body: Hash,
}
impl std::fmt::Debug for QueryAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QueryAuthority([private])")
    }
}
impl QueryAuthority {
    pub(super) fn capture(work: QueryWork, command: &Outgoing) -> Result<Self, EngineError> {
        if command.kind != CommandKind::ProviderEmbed || command.key != SerialKey::None {
            return Err(EngineError::unreadable(
                "semantic query origin has a different native command",
            ));
        }
        Ok(Self {
            work,
            call: command.id,
            machine: command.machine,
            body: Hash::of(&serde_json::to_vec(&command.body)?),
        })
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.id != self.call
            || command.machine_id != self.machine
            || command.kind != "provider.embed"
            || command.serial_key().is_some()
            || Hash::of(&serde_json::to_vec(&command.body)?) != self.body
        {
            return Ok(None);
        }
        let call: Option<(Id, Id, Option<Id>)> = sqlx::query_as("SELECT node_id,provider_id,person_id FROM summary_calls WHERE id=$1 AND purpose='embeddings' AND cancel_reason IS NULL")
            .bind(self.call).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if call
            != Some((
                self.work.read.org,
                self.work.generation.provider,
                Some(self.work.writer),
            ))
        {
            return Ok(None);
        }
        let frame = match continuing::prepare::<QuerySource>(
            conn,
            clock,
            &self.work.read.origin,
            self.work.clone(),
        )
        .await
        {
            Ok(frame) => frame,
            Err(Error::Refused | Error::Binding(crate::operation_scope::BindingError::Refused)) => {
                return Ok(None);
            }
            Err(error) => return Err(frame_error(error)),
        };
        let allowed = access
            .background_provider_use(
                conn,
                &ProviderUse {
                    person: self.work.writer,
                    node: self.work.read.org,
                    provider: self.work.generation.provider,
                    model: Some(&self.work.generation.model),
                    effort: None,
                },
            )
            .await?;
        if !allowed.is_allowed() {
            return Ok(None);
        }
        let present = orch_db::queries::resources::thread_engine::provider(
            conn,
            self.work.generation.provider,
        )
        .await?
        .is_some_and(|row| {
            row.deleted_at.is_none()
                && crate::providers::request_status_available(&row.status)
                && row.key_machine_id == Some(self.machine)
        });
        if !present
            || rows::endpoint_fingerprint(conn, self.work.generation.provider)
                .await?
                .as_deref()
                != Some(self.work.generation.fingerprint.as_slice())
        {
            return Ok(None);
        }
        if frame.check_deadline(clock).is_err() {
            return Ok(None);
        }
        let parts = frame.into_parts();
        if parts.first_effect.check(clock).is_err() {
            return Ok(None);
        }
        let caller = parts.principal.caller().clone();
        let end = caller.grant().and_then(orch_authz::TokenGrant::expires_at);
        Ok(Some(vec![(Some(caller), end)]))
    }
}

fn target_error(error: targets::TargetError) -> Error {
    match error {
        targets::TargetError::Missing
        | targets::TargetError::Authz(orch_authz::Error::NotFound(_)) => Error::Refused,
        targets::TargetError::Db(error) => error.into(),
        targets::TargetError::Authz(error) => error.into(),
    }
}
