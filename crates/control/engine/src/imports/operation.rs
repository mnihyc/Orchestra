//! Actual accepted HTTP identity and native source/governor checkpoints of an import job.
use super::*;
use crate::operation_scope::{
    blob_sources,
    continuing::{self, Error, Source as NativeSource},
    http_origin::HttpOrigin,
    sources::Facts,
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyResolvedTarget;
use orch_api_contract::{
    import_creation_sources::{self as declarations, Source},
    imports::{ImportAuthors, NodeImportCreate},
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_chat::following::BlobOrigin;
use orch_db::DbError;
use orch_db::queries::jobs::JobRow;
use orch_types::Clock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
const MEMBER: &str = "import_operation";
fn refused() -> Error {
    Error::Refused
}
fn native(error: impl std::fmt::Display) -> Error {
    Error::Native(EngineError::unreadable(error))
}
/// Private association with the actual native queue; it contains no reusable source admission.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    import: Id,
    job: Id,
    user: Id,
    node: Id,
    request_hash: String,
    http: HttpOrigin,
    selected: Vec<(Hash, BlobOrigin)>,
    authors: ImportAuthors,
    access: bool,
    #[serde(skip)]
    observed: Arc<std::sync::Mutex<Option<HttpOrigin>>>,
}
impl Request {
    /// Attach only a fully admitted native outcome on its actual queue transaction.
    pub async fn retain(
        conn: &mut Conn,
        made: &Made,
        http: HttpOrigin,
        selected: Vec<(Hash, BlobOrigin)>,
        plan: &Plan,
        now: Time,
    ) -> Result<Self, ImportError> {
        if made.job.kind != kind::IMPORT
            || made.job.created_by != Some(http.origin().user())
            || made.import.user_id != Some(http.origin().user())
            || made.import.job_id != Some(made.job.id)
            || made.import.node_id != Some(plan.destination.id)
        {
            return Err(ImportError::NotFound);
        }
        let actual = declarations::CANDIDATES
            [usize::from(plan.access.restored || plan.authors == ImportAuthors::Accounts)]
        .operation;
        if http.origin().operation() != actual {
            return Err(ImportError::NotFound);
        }
        let request_hash=sqlx::query_scalar("SELECT encode(sha256(convert_to(request::text,'UTF8')),'hex') FROM imports WHERE id=$1")
            .bind(made.import.id).fetch_one(&mut *conn).await.map_err(DbError::from)?;
        let value = Self {
            import: made.import.id,
            job: made.job.id,
            user: http.origin().user(),
            node: plan.destination.id,
            request_hash,
            http,
            selected,
            authors: plan.authors,
            access: plan.access.restored,
            observed: Arc::default(),
        };
        job_rows::merge_plan(
            conn,
            made.job.id,
            &json!({(MEMBER):value,"import_operation_required":true}),
            now.get(),
        )
        .await?;
        Ok(value)
    }
    /// A missing or malformed modern origin is closed, never a reconstructed Session.
    pub fn from_job(row: &JobRow) -> Result<Option<Self>, EngineError> {
        if row.plan.get("undo").and_then(Json::as_bool) == Some(true) {
            return Ok(None);
        }
        match row.plan.get(MEMBER) {
            Some(value) => {
                let value: Self = serde_json::from_value(value.clone())?;
                if row.kind != kind::IMPORT
                    || row.id != value.job
                    || row.created_by != Some(value.user)
                    || row.plan["import"] != json!(value.import)
                    || row.plan["node"] != json!(value.node)
                    || row.plan["user"] != json!(value.user)
                {
                    return Err(EngineError::unreadable(
                        "the import request does not belong to this job",
                    ));
                }
                Ok(Some(value))
            }
            None if row
                .plan
                .get("import_operation_required")
                .and_then(Json::as_bool)
                == Some(true)
                || row
                    .plan
                    .get("credential")
                    .and_then(|value| value.get("operation_scoped"))
                    .and_then(Json::as_bool)
                    == Some(true) =>
            {
                Err(EngineError::unreadable(
                    "the import's original request is missing",
                ))
            }
            None => Ok(None),
        }
    }
    async fn verified(&self, conn: &mut Conn) -> Result<(HttpOrigin, ImportRequest), Error> {
        #[derive(sqlx::FromRow)]
        struct Stored {
            user_id: Option<Id>,
            node_id: Option<Id>,
            job_id: Option<Id>,
            request: Json,
            request_hash: String,
            origin: Option<Json>,
            kind: String,
            created_by: Option<Id>,
        }
        let stored:Stored=sqlx::query_as("SELECT i.user_id,i.node_id,i.job_id,i.request,encode(sha256(convert_to(i.request::text,'UTF8')),'hex') request_hash,j.plan->$3 origin,j.kind,j.created_by FROM imports i JOIN jobs j ON j.id=$2 WHERE i.id=$1")
            .bind(self.import).bind(self.job).bind(MEMBER).fetch_optional(&mut *conn).await.map_err(DbError::from)?.ok_or_else(refused)?;
        let actual: Self = serde_json::from_value(stored.origin.ok_or_else(refused)?)
            .map_err(EngineError::from)?;
        if stored.user_id != Some(self.user)
            || stored.node_id != Some(self.node)
            || stored.job_id != Some(self.job)
            || stored.kind != kind::IMPORT
            || stored.created_by != Some(self.user)
            || stored.request_hash != self.request_hash
            || actual.import != self.import
            || actual.job != self.job
            || actual.user != self.user
            || actual.node != self.node
            || actual.request_hash != self.request_hash
            || actual.selected != self.selected
            || actual.authors != self.authors
            || actual.access != self.access
        {
            return Err(refused());
        }
        let mut http = self.http.intersect(&actual.http)?;
        if let Some(observed) = self.observed.lock().map_err(native)?.as_ref() {
            http = http.intersect(observed)?;
        }
        let request: ImportRequest =
            serde_json::from_value(stored.request).map_err(EngineError::from)?;
        let expected = request
            .source
            .files
            .iter()
            .map(|file| file.blob)
            .collect::<std::collections::HashSet<_>>();
        if self
            .selected
            .iter()
            .map(|(hash, _)| *hash)
            .collect::<std::collections::HashSet<_>>()
            != expected
        {
            return Err(refused());
        }
        Ok((http, request))
    }
    /// Preserve the server's resolved intent instead of acquiring a later broader author/access default.
    pub fn execution_request(&self, mut request: ImportRequest) -> ImportRequest {
        request.options.authors = Some(self.authors);
        request.options.access = Some(self.access);
        request
    }
    pub fn live_deadline(&self, clock: &dyn Clock) -> Result<(), Error> {
        let observed = self.observed.lock().map_err(native)?;
        if self.http.until().is_some_and(|until| until <= clock.time())
            || observed
                .as_ref()
                .and_then(HttpOrigin::until)
                .is_some_and(|until| until <= clock.time())
        {
            return Err(refused());
        }
        Ok(())
    }
    pub fn operation(&self) -> &str {
        self.http.origin().operation()
    }
    async fn narrow(
        &self,
        conn: &mut Conn,
        http: &HttpOrigin,
        caller: &Caller,
        now: Time,
    ) -> Result<(), Error> {
        let (stored, _) = self.verified(conn).await?;
        let http = stored.intersect(http)?.accepted(caller, http.until())?;
        let mut next = self.clone();
        next.http = http;
        job_rows::merge_plan(conn, self.job, &json!({(MEMBER):next}), now.get()).await?;
        Ok(())
    }
    fn note(&self, http: &HttpOrigin, caller: &Caller, until: Option<Time>) -> Result<(), Error> {
        let next = http.accepted(caller, until)?;
        let mut observed = self.observed.lock().map_err(native)?;
        *observed = Some(match observed.as_ref() {
            Some(old) => old.intersect(&next)?,
            None => next,
        });
        Ok(())
    }
    /// Persist only restrictive observations once native work has stopped emitting progress on this job.
    pub async fn flush(&self, engine: &Engine, conn: &mut Conn) -> Result<(), Error> {
        let (http, _) = self.verified(conn).await?;
        let current = http.current(conn, engine.inner.clock.as_ref()).await?;
        self.narrow(conn, &http, current.caller(), engine.inner.clock.time())
            .await?;
        current.check(engine.inner.clock.as_ref())
    }
    /// Retain the actual raw/key minimum at commit without re-admitting completed domain writes.
    pub async fn deadline(&self, engine: &Engine, conn: &mut Conn) -> Result<(), Error> {
        let (http, _) = self.verified(conn).await?;
        let current = http.current(conn, engine.inner.clock.as_ref()).await?;
        current.check(engine.inner.clock.as_ref())
    }
    /// Current raw identity and source preparation before native archive IO; no target Write is lent.
    pub async fn principal(&self, engine: &Engine) -> Result<Principal, Error> {
        let mut tx = engine.db().begin().await?;
        let (http, _) = self.verified(&mut tx).await?;
        let frame = http
            .scope(continuing::preparation::prepare::<Reading>(
                &mut tx,
                engine.inner.clock.as_ref(),
                http.origin(),
                Input {
                    engine: engine.clone(),
                    request: self.clone(),
                    plan: None,
                    object: None,
                },
            ))
            .await?;
        frame.check_deadline(engine.inner.clock.as_ref())?;
        let parts = frame.into_parts();
        parts.first_effect.check(engine.inner.clock.as_ref())?;
        self.narrow(
            &mut tx,
            &http,
            parts.principal.caller(),
            engine.inner.clock.time(),
        )
        .await?;
        let current = http.current(&mut tx, engine.inner.clock.as_ref()).await?;
        current.check(engine.inner.clock.as_ref())?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(parts.principal)
    }
    /// The actual native object's complete current footprint, after its final preparation/row waits.
    pub async fn cut(
        &self,
        engine: &Engine,
        conn: &mut Conn,
        plan: Arc<Plan>,
        object: Id,
    ) -> Result<Cut, Error> {
        let (http, _) = self.verified(conn).await?;
        let frame = http
            .prepare::<Entry>(
                conn,
                engine.inner.clock.as_ref(),
                Input {
                    engine: engine.clone(),
                    request: self.clone(),
                    plan: Some(plan),
                    object: Some(object),
                },
            )
            .await?;
        frame.check_deadline(engine.inner.clock.as_ref())?;
        let parts = frame.into_parts();

        let current = http.current(conn, engine.inner.clock.as_ref()).await?;
        Ok(Cut {
            first: Some(parts.first_effect),
            request: self.clone(),
            http,
            principal: parts.principal,
            until: current.until(),
        })
    }
}
/// One current native cut held by the actual writer's still-open transaction.
pub struct Cut {
    request: Request,
    http: HttpOrigin,
    first: Option<continuing::FirstEffect>,
    pub principal: Principal,
    until: Option<Time>,
}
impl Cut {
    pub fn before(&mut self, clock: &dyn Clock) -> Result<(), Error> {
        self.deadline(clock)?;
        if let Some(first) = self.first.take() {
            first.check(clock)?;
            self.request
                .note(&self.http, self.principal.caller(), self.until)?;
        }
        Ok(())
    }
    pub fn deadline(&self, clock: &dyn Clock) -> Result<(), Error> {
        if !self.principal.caller().key_deadline_allows(clock)
            || self.until.is_some_and(|until| until <= clock.time())
        {
            return Err(refused());
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct Input {
    engine: Engine,
    request: Request,
    plan: Option<Arc<Plan>>,
    object: Option<Id>,
}
pub struct Value {
    targets: Vec<KeyResolvedTarget>,
    physical: Vec<KeyResolvedTarget>,
}
pub struct Reading;
pub struct Entry;
const CONTENT_READ: Declaration<Source> = Declaration {
    operation: declarations::CANDIDATES[0].operation,
    bindings: &[
        declarations::CANDIDATES[0].bindings[0],
        declarations::CANDIDATES[0].bindings[1],
        declarations::CANDIDATES[0].bindings[2],
        declarations::CANDIDATES[0].bindings[3],
        declarations::CANDIDATES[0].bindings[4],
    ],
};
const RESTORE_READ: Declaration<Source> = Declaration {
    operation: declarations::CANDIDATES[1].operation,
    bindings: CONTENT_READ.bindings,
};
fn declaration(input: &Input, read: bool) -> &'static Declaration<Source> {
    if input.request.operation() == declarations::CANDIDATES[0].operation {
        if read {
            &CONTENT_READ
        } else {
            &declarations::CANDIDATES[0]
        }
    } else {
        if read {
            &RESTORE_READ
        } else {
            &declarations::CANDIDATES[1]
        }
    }
}
async fn prepare(
    conn: &mut Conn,
    caller: &Caller,
    input: &Input,
    now: Time,
    read: bool,
) -> Result<(Value, Vec<Facts<Source>>), Error> {
    let (_, request) = input.request.verified(conn).await?;
    let principal = Principal::load(conn, caller.clone(), now).await?;
    let base =
        crate::operation_scope::Prepared::new(NodeImportCreate::ROUTE, input.request.operation())?;
    let hashes = request
        .source
        .files
        .iter()
        .map(|file| file.blob)
        .collect::<Vec<_>>();
    let bytes = blob_sources::prepare_selected(
        conn,
        &principal,
        &base,
        &hashes,
        &[],
        &input.request.selected,
    )
    .await
    .map_err(|error| match error {
        blob_sources::Error::Missing
        | blob_sources::Error::Target(crate::operation_scope::targets::TargetError::Missing)
        | blob_sources::Error::Binding(crate::operation_scope::BindingError::Refused) => refused(),
        other => native(other),
    })?;
    if bytes.selected != input.request.selected {
        return Err(refused());
    }
    let mut facts = super::scope::byte_facts(&bytes.bindings);
    if !read {
        let plan = input.plan.as_ref().ok_or_else(refused)?;
        facts.extend(
            super::scope::facts(conn, plan, input.object)
                .await
                .map_err(native)?,
        );
    }
    let targets = facts
        .iter()
        .flat_map(|fact| {
            fact.groups
                .iter()
                .flat_map(|(_, targets)| targets.iter().cloned())
        })
        .collect();
    let mut nodes = vec![input.request.node];
    let object = input
        .plan
        .as_ref()
        .and_then(|plan| input.object.and_then(|id| plan.get(id)));
    if let Some(object) = object {
        nodes.push(object.lies_in);
    }
    let mut graph = orch_authz::engine::Graph::load(conn, &nodes).await?;
    let mut physical = Vec::new();
    for node in nodes {
        let target = crate::operation_scope::targets::from_node_identity(&graph, node)
            .ok_or_else(refused)?;
        if !target
            .node_ancestors
            .iter()
            .any(|parent| parent.id == Some(input.request.node))
        {
            return Err(refused());
        }
        physical.push(target);
    }
    if let Some(folder) = object.and_then(|object| object.container) {
        let rows = orch_authz::documents::read_into(conn, &mut graph, &[folder]).await?;
        let row = rows
            .iter()
            .find(|row| row.id == folder && !row.deleted)
            .ok_or_else(refused)?;
        let site = orch_authz::documents::site_of(&graph, row).ok_or_else(refused)?;
        let target =
            crate::operation_scope::targets::from_site(&graph, &site).ok_or_else(refused)?;
        if !target
            .node_ancestors
            .iter()
            .any(|parent| parent.id == Some(input.request.node))
        {
            return Err(refused());
        }
        physical.push(target);
    }
    Ok((Value { targets, physical }, facts))
}
async fn require(conn: &mut Conn, principal: &Principal, input: &Input) -> Result<(), Error> {
    input
        .engine
        .import_place_seen(conn, principal, input.request.node)
        .await
        .map_err(native)?;
    if let Some(plan) = &input.plan {
        let (_, authors, access) = super::plan::authority_of(
            conn,
            principal,
            input.request.node,
            &plan.wants,
            plan.opened.access.is_some(),
        )
        .await
        .map_err(native)?;
        if authors != input.request.authors || access.restored != input.request.access {
            return Err(refused());
        }
        if let Some(id) = input.object {
            let object = plan.get(id).ok_or_else(refused)?;
            if !super::make::may_still_on(conn, principal, object)
                .await
                .map_err(native)?
            {
                return Err(refused());
            }
        }
    }
    Ok(())
}
async fn locks(conn: &mut Conn, value: &Value) -> Result<(), Error> {
    use orch_access_contract::{
        key_scope::{KeyIdentityKind, KeyTargetKind},
        permissions::ObjectKind,
    };
    let nodes = value
        .targets
        .iter()
        .chain(&value.physical)
        .flat_map(|target| target.node_ancestors.iter().filter_map(|node| node.id))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    orch_db::queries::documents::containers::lock_scopes(conn, &nodes).await?;
    sqlx::query("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(&nodes)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let ids = |kind| {
        value
            .targets
            .iter()
            .chain(&value.physical)
            .filter_map(|target| {
                (target.object.kind == kind)
                    .then_some(target.object.id)
                    .flatten()
            })
            .collect::<Vec<_>>()
    };
    sqlx::query("SELECT node_id FROM threads WHERE node_id=ANY($1) ORDER BY node_id FOR UPDATE")
        .bind(ids(KeyTargetKind::Object(ObjectKind::Thread)))
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM documents WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(ids(KeyTargetKind::Object(ObjectKind::Document)))
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM document_containers WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(ids(KeyTargetKind::Object(ObjectKind::DocumentContainer)))
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM chats WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(ids(KeyTargetKind::Identity(KeyIdentityKind::Chat)))
        .fetch_all(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}
macro_rules! source {
    ($ty:ty,$read:expr) => {
        impl NativeSource for $ty {
            type Role = Source;
            type Input = Input;
            type Value = Value;
            const ROUTE: Route = NodeImportCreate::ROUTE;
            const DECLARATION: &'static Declaration<Source> = &declarations::CANDIDATES[0];
            fn declaration(input: &Input, _: &Value) -> &'static Declaration<Source> {
                declaration(input, $read)
            }
            fn prepare<'a>(
                conn: &'a mut Conn,
                caller: &'a Caller,
                input: &'a Input,
                now: Time,
            ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Source>>), Error>> {
                Box::pin(prepare(conn, caller, input, now, $read))
            }
            fn require<'a>(
                conn: &'a mut Conn,
                principal: &'a Principal,
                input: &'a Input,
                _: &'a Value,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(require(conn, principal, input))
            }
            fn locks<'a>(
                conn: &'a mut Conn,
                _: &'a Principal,
                _: &'a Input,
                value: &'a Value,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(locks(conn, value))
            }
            fn check_opened<'a>(
                _: &'a mut Conn,
                _: &'a Principal,
                _: &'a Input,
                old: &'a Value,
                fresh: &'a Value,
            ) -> BoxFuture<'a, Result<bool, Error>> {
                Box::pin(async move {
                    Ok(old.targets == fresh.targets && old.physical == fresh.physical)
                })
            }
        }
    };
}
source!(Reading, true);
source!(Entry, false);

impl Request {
    /// Each physical supplier/delivery wait uses a fresh known-source frame, never a retained lease.
    pub fn stream(
        &self,
        engine: Engine,
        stream: orch_blob_contract::store::ByteStream,
    ) -> orch_blob_contract::store::ByteStream {
        use futures::StreamExt;
        let initial = (self.clone(), engine, stream, false);
        futures::stream::unfold(initial, |(request, engine, mut stream, ended)| async move {
            if ended {
                return None;
            }
            let piece = match request.principal(&engine).await {
                Err(_) => Err(orch_blob_contract::store::BlobError::Io(
                    "The original access no longer permits this import.".into(),
                )),
                Ok(_) => match stream.next().await {
                    None => return None,
                    Some(Err(error)) => Err(error),
                    Some(Ok(piece)) => match request.principal(&engine).await {
                        Ok(_) => Ok(piece),
                        Err(_) => Err(orch_blob_contract::store::BlobError::Io(
                            "The original access no longer permits this import.".into(),
                        )),
                    },
                },
            };
            let ended = piece.is_err();
            Some((piece, (request, engine, stream, ended)))
        })
        .boxed()
    }
}
