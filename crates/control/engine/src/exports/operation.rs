//! Actual caller and native source checkpoints retained by a queued place export.
use super::*;
use crate::operation_scope::{
    continuing::{self, Error, Source},
    request_origin::{self, Credential},
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    export_sources::{self as declarations, Source as Role},
    exports::NodeExportCreate,
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::CapturedOperation;
use orch_types::Clock;
use serde::{Deserialize, Serialize};

const MEMBER: &str = "export_operation";
/// The existing native form, kept typed until its bytes are serialized by the archive writer.
pub enum Rendered {
    Thread(Box<Thread>),
    Document(Box<DocumentForm>),
}
fn refused() -> Error {
    Error::Refused
}
/// A concrete source entry selected by the native exporter, never a submitted governor hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Subject {
    Node(Id),
    NodePath(Id),
    NodeAccess(Id),
    Thread(Id),
    ThreadCount(Id),
    Chat(Id),
    ChatCount(Id),
    Document(Id),
    DocumentCount(Id),
    DocumentAccess(Id),
    Folder(Id),
    FolderPath(Id),
    Creator,
    Email(Id),
    /// Protected references selected by the native form before serialization.
    Reference(FormReference),
    /// A transient form association while its bytes are being delivered.
    Form(Form, [u8; 32]),
}
/// Server-private original request; this carries association and ceilings, never admission.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    job: Id,
    export: Id,
    user: Id,
    node: Id,
    request_hash: String,
    origin: CapturedOperation,
    credential: Credential,
    until: Option<Time>,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExportRequestOrigin([private])")
    }
}
impl Request {
    /// Read one native form on the same source transaction; references are checked before formatting.
    pub async fn form(
        &self,
        engine: &Engine,
        form: Form,
        how: Option<DocumentHow>,
    ) -> Result<(Rendered, Vec<Subject>), Error> {
        let forms = engine.forms().ok_or_else(|| {
            Error::Native(EngineError::unreadable("the export form is unavailable"))
        })?;
        let mut tx = engine.db().begin().await?;
        let (caller, until) = self.caller(&mut tx, engine.inner.clock.as_ref()).await?;
        let primary = match form {
            Form::Thread { id } => Subject::Thread(id),
            Form::Document { id, .. } => Subject::Document(id),
        };
        let frame = continuing::preparation::prepare_with_caller::<Body>(
            &mut tx,
            engine.inner.clock.as_ref(),
            &self.origin,
            &caller,
            Input {
                engine: engine.clone(),
                request: self.clone(),
                subjects: vec![primary],
            },
        )
        .await?;
        let parts = frame.into_parts();
        if let Form::Thread { id } = form {
            sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR SHARE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(DbError::from)?;
        }
        let frame = parts
            .opened
            .refresh_with_caller(&mut tx, engine.inner.clock.as_ref(), &caller)
            .await?;
        let principal = frame.into_parts().principal;
        let revision = form_revision(&mut tx, form).await?;
        let sources = forms
            .sources(&mut tx, &principal, form)
            .await
            .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
        if sources.is_none()
            && caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
        {
            return Err(Error::Native(EngineError::unreadable(
                "the export form has no native source handoff",
            )));
        }
        let mut subjects = vec![primary, Subject::Form(form, revision)];
        if let Some(sources) = sources {
            subjects.extend(sources.references.into_iter().map(Subject::Reference));
        }
        let frame = continuing::prepare_with_caller::<Entry>(
            &mut tx,
            engine.inner.clock.as_ref(),
            &self.origin,
            &caller,
            Input {
                engine: engine.clone(),
                request: self.clone(),
                subjects: subjects.clone(),
            },
        )
        .await?;
        let principal = frame.principal().clone();
        if let Some(fresh) = forms
            .sources(&mut tx, &principal, form)
            .await
            .map_err(|error| Error::Native(EngineError::unreadable(error)))?
        {
            let old = subjects
                .iter()
                .filter_map(|subject| match subject {
                    Subject::Reference(reference) => Some(*reference),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            if old != fresh.references.into_iter().collect()
                || revision != form_revision(&mut tx, form).await?
            {
                return Err(refused());
            }
        }
        let rendered = match form {
            Form::Thread { id } => Rendered::Thread(Box::new(
                forms
                    .thread(&mut tx, &principal, id, engine.inner.clock.now())
                    .await
                    .map_err(|error| Error::Native(EngineError::unreadable(error)))?,
            )),
            Form::Document { id, .. } => Rendered::Document(Box::new(
                forms
                    .document(
                        &mut tx,
                        &principal,
                        id,
                        how.ok_or_else(|| {
                            Error::Native(EngineError::unreadable("the document form is missing"))
                        })?,
                        engine.inner.clock.now(),
                    )
                    .await
                    .map_err(|error| Error::Native(EngineError::unreadable(error)))?,
            )),
        };
        frame.check_deadline(engine.inner.clock.as_ref())?;
        if until.is_some_and(|until| until <= engine.inner.clock.time()) {
            return Err(refused());
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok((rendered, subjects))
    }
    /// Attach provenance to this actual new native Export/Job before its caller's commit.
    pub async fn retain(
        conn: &mut Conn,
        made: &Made,
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
        now: Time,
    ) -> Result<Self, ExportError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let node = made.export.node_id.ok_or(ExportError::NotFound)?;
        if made.job.kind != kind::EXPORT
            || made.job.created_by != Some(caller.user())
            || made.export.user_id != Some(caller.user())
            || made.export.job_id != Some(made.job.id)
            || origin.operation() != declarations::CREATE.operation
            || !credential.matches(caller)
        {
            return Err(ExportError::NotFound);
        }
        let value = Self {
            job: made.job.id,
            export: made.export.id,
            user: caller.user(),
            node,
            request_hash: sqlx::query_scalar("SELECT encode(sha256(convert_to(request::text,'UTF8')),'hex') FROM exports WHERE id=$1")
                .bind(made.export.id).fetch_one(&mut *conn).await.map_err(DbError::from)?,
            origin,
            credential,
            until,
        };
        job_rows::merge_plan(
            conn,
            made.job.id,
            &json!({(MEMBER):value,"export_operation_required":true}),
            now.get(),
        )
        .await?;
        Ok(value)
    }
    /// Present invalid provenance cannot select the historical native path.
    pub fn from_job(row: &JobRow) -> Result<Option<Self>, EngineError> {
        match row.plan.get(MEMBER) {
            Some(value) => {
                let value: Self = serde_json::from_value(value.clone())?;
                if value.job != row.id
                    || row.kind != kind::EXPORT
                    || row.created_by != Some(value.user)
                    || row.plan["export"] != json!(value.export)
                    || row.plan["node"] != json!(value.node)
                    || row.plan["user"] != json!(value.user)
                {
                    return Err(EngineError::unreadable(
                        "the export request does not belong to this job",
                    ));
                }
                Ok(Some(value))
            }
            None if row
                .plan
                .get("export_operation_required")
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
                    "the export's original request is missing",
                ))
            }
            None => Ok(None),
        }
    }
    async fn verify(&self, conn: &mut Conn) -> Result<(), Error> {
        #[derive(sqlx::FromRow)]
        struct Stored {
            export_job: Option<Id>,
            user_id: Option<Id>,
            node_id: Option<Id>,
            state: String,
            request_hash: String,
            kind: String,
            created_by: Option<Id>,
            origin: Option<Json>,
            planned_export: Option<String>,
            planned_node: Option<String>,
            planned_user: Option<String>,
        }
        let actual: Stored = sqlx::query_as("SELECT e.job_id export_job,e.user_id,e.node_id,e.state,encode(sha256(convert_to(e.request::text,'UTF8')),'hex') request_hash,
            j.kind,j.created_by,j.plan->$2 origin,j.plan->>'export' planned_export,j.plan->>'node' planned_node,j.plan->>'user' planned_user
            FROM exports e JOIN jobs j ON j.id=$3 WHERE e.id=$1")
            .bind(self.export).bind(MEMBER).bind(self.job).fetch_optional(conn).await.map_err(DbError::from)?.ok_or_else(refused)?;
        let retained: Self = serde_json::from_value(actual.origin.ok_or_else(refused)?)
            .map_err(EngineError::from)?;
        if retained != *self
            || actual.export_job != Some(self.job)
            || actual.user_id != Some(self.user)
            || actual.node_id != Some(self.node)
            || actual.request_hash != self.request_hash
            || actual.state != rows::state::PENDING
            || actual.kind != kind::EXPORT
            || actual.created_by != Some(self.user)
            || actual.planned_export.as_deref() != Some(self.export.to_string().as_str())
            || actual.planned_node.as_deref() != Some(self.node.to_string().as_str())
            || actual.planned_user.as_deref() != Some(self.user.to_string().as_str())
        {
            return Err(refused());
        }
        Ok(())
    }
    /// The original/current raw credential minimum on the owner's supplied connection.
    pub async fn caller(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<(Caller, Option<Time>), Error> {
        self.verify(conn).await?;
        request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.until,
            declarations::CREATE.operation,
        )
        .await
    }
    /// Current native root selection permission; this creates no lease over later entries.
    pub async fn principal(&self, engine: &Engine) -> Result<Principal, Error> {
        let mut tx = engine.db().begin().await?;
        let (caller, until) = self.caller(&mut tx, engine.inner.clock.as_ref()).await?;
        let frame = continuing::preparation::prepare_with_caller::<Root>(
            &mut tx,
            engine.inner.clock.as_ref(),
            &self.origin,
            &caller,
            Input {
                engine: engine.clone(),
                request: self.clone(),
                subjects: Vec::new(),
            },
        )
        .await?;
        frame.check_deadline(engine.inner.clock.as_ref())?;
        if until.is_some_and(|until| until <= engine.inner.clock.time()) {
            return Err(refused());
        }
        let principal = frame.into_parts().principal;
        tx.commit().await.map_err(DbError::from)?;
        Ok(principal)
    }
    /// Run one actual native piece after its last supplier/backpressure wait on the pinned current frame.
    pub async fn effect<T>(
        &self,
        engine: &Engine,
        subjects: &[Subject],
        write: impl FnOnce() -> T + Send,
    ) -> Result<T, Error> {
        let mut tx = engine.db().begin().await?;
        let (caller, until) = self.caller(&mut tx, engine.inner.clock.as_ref()).await?;
        let input = Input {
            engine: engine.clone(),
            request: self.clone(),
            subjects: subjects.to_vec(),
        };
        let frame = continuing::prepare_with_caller::<Entry>(
            &mut tx,
            engine.inner.clock.as_ref(),
            &self.origin,
            &caller,
            input,
        )
        .await?;
        let parts = frame.into_parts();
        parts.first_effect.check(engine.inner.clock.as_ref())?;
        if until.is_some_and(|until| until <= engine.inner.clock.time()) {
            return Err(refused());
        }
        let value = write();
        tx.commit().await.map_err(DbError::from)?;
        Ok(value)
    }
    /// Native scratch setup uses only its actual known root; future contents stay unresolved.
    pub async fn setup<T>(
        &self,
        engine: &Engine,
        write: impl FnOnce() -> T + Send,
    ) -> Result<T, Error> {
        let mut tx = engine.db().begin().await?;
        let (caller, until) = self.caller(&mut tx, engine.inner.clock.as_ref()).await?;
        let frame = continuing::preparation::prepare_with_caller::<Root>(
            &mut tx,
            engine.inner.clock.as_ref(),
            &self.origin,
            &caller,
            Input {
                engine: engine.clone(),
                request: self.clone(),
                subjects: Vec::new(),
            },
        )
        .await?;
        frame
            .into_parts()
            .first_effect
            .check(engine.inner.clock.as_ref())?;
        if until.is_some_and(|until| until <= engine.inner.clock.time()) {
            return Err(refused());
        }
        let value = write();
        tx.commit().await.map_err(DbError::from)?;
        Ok(value)
    }
    /// The actual originally requested root, selected by native persisted rows.
    pub fn node(&self) -> Id {
        self.node
    }

    /// Resolve the same actual native candidates used at the HTTP creation boundary.
    pub async fn selected(
        &self,
        engine: &Engine,
        conn: &mut Conn,
        principal: &Principal,
    ) -> Result<Selected, ExportError> {
        let row = rows::get(conn, self.export)
            .await?
            .ok_or(ExportError::NotFound)?;
        let request: ExportRequest =
            serde_json::from_value(row.request).map_err(EngineError::from)?;
        let prepared = engine
            .prepare_export_plan(conn, principal, self.node, &request)
            .await?;
        let root = targets::node_identity(conn, self.node)
            .await
            .map_err(EngineError::unreadable)?;
        let plan = crate::operation_scope::sources::bind_known(
            NodeExportCreate::ROUTE,
            &declarations::CREATE,
            &[Facts::one(Role::Place, root.object.kind, root)],
        )
        .map_err(EngineError::unreadable)?;
        let (prepared, _) = engine
            .bound_export_plan(
                conn,
                principal.caller(),
                prepared,
                &plan,
                &declarations::CREATE,
                None,
            )
            .await?;
        engine
            .finish_export_selection(conn, &prepared.plan.selection, &prepared.plan.wanted)
            .await
    }

    /// A metadata candidate remains subject to the exact archive operation's native role.
    pub async fn permits_metadata(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        target: &orch_access_contract::key_scope::KeyResolvedTarget,
    ) -> Result<bool, Error> {
        let (caller, _) = self.caller(conn, clock).await?;
        let plan = crate::operation_scope::Prepared::new(
            NodeExportCreate::ROUTE,
            declarations::CREATE.operation,
        )?;
        super::scope::permits(
            &plan,
            &declarations::CREATE,
            &caller,
            Role::Contents,
            orch_api_contract::operations::FacetUsage::Required,
            target,
        )
        .map_err(|error| Error::Native(EngineError::unreadable(error)))
    }

    /// Whether a package may include this actual account field under its own operation.
    pub async fn permits_person(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        user: Id,
        creator: bool,
    ) -> Result<bool, Error> {
        let (caller, _) = self.caller(conn, clock).await?;
        let row = orch_db::queries::identity::user_by_id(conn, user)
            .await?
            .ok_or_else(refused)?;
        let target = targets::from_account(conn, &row)
            .await
            .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
        let plan = crate::operation_scope::Prepared::new(
            NodeExportCreate::ROUTE,
            declarations::CREATE.operation,
        )?;
        super::scope::permits(
            &plan,
            &declarations::CREATE,
            &caller,
            if creator { Role::Creator } else { Role::People },
            orch_api_contract::operations::FacetUsage::ResponseContent,
            &target,
        )
        .map_err(|error| Error::Native(EngineError::unreadable(error)))
    }

    /// Scope-only selection over a native metadata target; the byte writer still takes a fresh full cut.
    pub fn permits_field(
        &self,
        caller: &Caller,
        role: Role,
        usage: orch_api_contract::operations::FacetUsage,
        target: &orch_access_contract::key_scope::KeyResolvedTarget,
    ) -> Result<bool, Error> {
        let plan = crate::operation_scope::Prepared::new(
            NodeExportCreate::ROUTE,
            declarations::CREATE.operation,
        )?;
        super::scope::permits(&plan, &declarations::CREATE, caller, role, usage, target)
            .map_err(|error| Error::Native(EngineError::unreadable(error)))
    }

    /// Current complete entry sources on the writer's real transaction, after its native waits.
    pub async fn cut(
        &self,
        engine: &Engine,
        conn: &mut Conn,
        subjects: &[Subject],
    ) -> Result<Cut, Error> {
        let (caller, until) = self.caller(conn, engine.inner.clock.as_ref()).await?;
        let frame = continuing::prepare_with_caller::<Entry>(
            conn,
            engine.inner.clock.as_ref(),
            &self.origin,
            &caller,
            Input {
                engine: engine.clone(),
                request: self.clone(),
                subjects: subjects.to_vec(),
            },
        )
        .await?;
        let parts = frame.into_parts();
        Ok(Cut {
            first: Some(parts.first_effect),
            caller: parts.principal.caller().clone(),
            until,
        })
    }
}

/// The synchronous current-authority cut for one archive entry or completed blob.
pub struct Cut {
    first: Option<continuing::FirstEffect>,
    caller: Caller,
    until: Option<Time>,
}
impl Cut {
    /// Check immediately at the actual write, retaining the same deadlines through SQL waits.
    pub fn before(&mut self, clock: &dyn Clock) -> Result<(), Error> {
        self.deadline(clock)?;
        if let Some(first) = self.first.take() {
            first.check(clock)?;
        }
        Ok(())
    }
    /// Retain all accepted/current keys and the original raw credential until commit.
    pub fn deadline(&self, clock: &dyn Clock) -> Result<(), Error> {
        if !self.caller.key_deadline_allows(clock)
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
    subjects: Vec<Subject>,
}
pub struct Value {
    targets: Vec<orch_access_contract::key_scope::KeyResolvedTarget>,
    threads: Vec<Id>,
    access_nodes: Vec<Id>,
    access_documents: Vec<Id>,
    read_threads: Vec<Id>,
    read_chats: Vec<Id>,
    read_documents: Vec<Id>,
    chats: Vec<Id>,
    documents: Vec<Id>,
    folders: Vec<Id>,
    nodes: Vec<Id>,
    paths: Vec<orch_access_contract::key_scope::KeyResolvedTarget>,
    folder_paths: Vec<orch_access_contract::key_scope::KeyResolvedTarget>,
    chat_ids: Vec<Id>,
    creator: Option<orch_access_contract::key_scope::KeyResolvedTarget>,
    people: Vec<orch_access_contract::key_scope::KeyResolvedTarget>,
    references: Vec<(
        FormReference,
        orch_access_contract::key_scope::KeyResolvedTarget,
    )>,
    forms: Vec<(Form, [u8; 32])>,
}
pub struct Root;
pub struct Entry;
const ROOT_DECLARATION: Declaration<Role> = Declaration {
    operation: declarations::CREATE.operation,
    bindings: &[declarations::CREATE.bindings[0]],
};
const BODY_DECLARATION: Declaration<Role> = Declaration {
    operation: declarations::CREATE.operation,
    bindings: &[
        declarations::CREATE.bindings[0],
        declarations::CREATE.bindings[1],
    ],
};
pub struct Body;

/// A compact native association stamp; never a target, credential or admission.
pub async fn form_revision(conn: &mut Conn, form: Form) -> Result<[u8; 32], Error> {
    use sha2::{Digest, Sha256};
    let value = match form {
        Form::Thread { id } => {
            let revision: Option<i64> =
                sqlx::query_scalar("SELECT revision FROM nodes WHERE id=$1 AND deleted_at IS NULL")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            let source = crate::views::thread_metadata_sources(conn, id)
                .await?
                .ok_or_else(refused)?;
            let readmes = orch_db::queries::documents::readmes_of(conn, &[id]).await?;
            json!([
                revision.ok_or_else(refused)?,
                source.environment,
                source.side_of,
                source.binding,
                source.sandbox_machine,
                readmes
                    .iter()
                    .map(|row| (row.id, row.doc_seq))
                    .collect::<Vec<_>>()
            ])
        }
        Form::Document { id, .. } => {
            let revision: Option<i64> = sqlx::query_scalar(
                "SELECT doc_seq FROM documents WHERE id=$1 AND deleted_at IS NULL",
            )
            .bind(id)
            .fetch_optional(conn)
            .await
            .map_err(DbError::from)?;
            json!(revision.ok_or_else(refused)?)
        }
    };
    Ok(Sha256::digest(serde_json::to_vec(&value).map_err(EngineError::from)?).into())
}
async fn prepare(
    conn: &mut Conn,
    input: &Input,
    root_only: bool,
) -> Result<(Value, Vec<Facts<Role>>), Error> {
    use orch_access_contract::key_scope::KeyResolvedTarget;
    use orch_authz::engine::Graph;
    input.request.verify(conn).await?;
    let subjects = if root_only {
        Vec::new()
    } else {
        input
            .subjects
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    };
    let chat_ids = subjects
        .iter()
        .filter_map(|subject| match subject {
            Subject::Chat(id) | Subject::ChatCount(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let chats = orch_db::queries::chats::chats(conn, &chat_ids).await?;
    let mut nodes = vec![input.request.node];
    nodes.extend(subjects.iter().filter_map(|subject| match subject {
        Subject::Node(id)
        | Subject::Thread(id)
        | Subject::ThreadCount(id)
        | Subject::NodePath(id)
        | Subject::NodeAccess(id) => Some(*id),
        _ => None,
    }));
    nodes.extend(chats.iter().filter_map(|chat| chat.node_id));
    let mut graph = Graph::load(conn, &nodes).await?;
    let accounts = subjects
        .iter()
        .filter_map(|subject| match subject {
            Subject::Creator => Some(input.request.user),
            Subject::Email(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let accounts: Vec<(Id, String, Option<Id>)> =
        sqlx::query_as("SELECT id,kind,service_org_id FROM users WHERE id=ANY($1)")
            .bind(accounts)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    graph
        .extend(
            conn,
            &accounts
                .iter()
                .filter_map(|(_, _, org)| *org)
                .collect::<Vec<_>>(),
        )
        .await?;

    let ids = subjects
        .iter()
        .filter_map(|subject| match subject {
            Subject::Document(id)
            | Subject::DocumentCount(id)
            | Subject::DocumentAccess(id)
            | Subject::Folder(id)
            | Subject::FolderPath(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let documents = orch_authz::documents::read_into(conn, &mut graph, &ids).await?;
    let root = targets::from_node_identity(&graph, input.request.node).ok_or_else(refused)?;
    let fact = Facts::one(Role::Place, root.object.kind, root.clone());
    let mut groups = BTreeMap::<_, Vec<KeyResolvedTarget>>::new();
    let mut value = Value {
        targets: vec![root],
        threads: Vec::new(),
        access_nodes: Vec::new(),
        access_documents: Vec::new(),
        read_threads: Vec::new(),
        read_chats: Vec::new(),
        read_documents: Vec::new(),
        chats: Vec::new(),
        documents: Vec::new(),
        folders: Vec::new(),
        nodes: Vec::new(),
        paths: Vec::new(),
        folder_paths: Vec::new(),
        chat_ids: Vec::new(),
        creator: None,
        people: Vec::new(),
        references: Vec::new(),
        forms: Vec::new(),
    };
    for subject in &subjects {
        let target = match *subject {
            Subject::Node(id)
            | Subject::NodePath(id)
            | Subject::Thread(id)
            | Subject::ThreadCount(id)
            | Subject::NodeAccess(id) => {
                let target = targets::from_node_identity(&graph, id).ok_or_else(refused)?;
                match subject {
                    Subject::Thread(_) => {
                        if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
                            return Err(refused());
                        }
                        value.threads.push(id);
                    }
                    Subject::ThreadCount(_) => {
                        if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
                            return Err(refused());
                        }
                        value.read_threads.push(id);
                    }
                    Subject::Node(_) => value.nodes.push(id),
                    Subject::NodeAccess(_) => value.access_nodes.push(id),
                    _ => value.paths.push(target.clone()),
                }
                target
            }
            Subject::Chat(id) | Subject::ChatCount(id) => {
                let chat = chats
                    .iter()
                    .find(|chat| chat.id == id)
                    .ok_or_else(refused)?;
                let node = chat.node_id.ok_or_else(refused)?;
                if matches!(subject, Subject::ChatCount(_)) {
                    value.read_chats.push(node);
                } else {
                    value.chats.push(node);
                }
                value.chat_ids.push(id);
                targets::from_chat_graph(&graph, chat)
                    .map_err(|error| Error::Native(EngineError::unreadable(error)))?
            }
            Subject::Document(id)
            | Subject::DocumentCount(id)
            | Subject::DocumentAccess(id)
            | Subject::Folder(id)
            | Subject::FolderPath(id) => {
                let kind = if matches!(
                    subject,
                    Subject::Document(_) | Subject::DocumentCount(_) | Subject::DocumentAccess(_)
                ) {
                    ObjectKind::Document
                } else {
                    ObjectKind::DocumentContainer
                };
                let row = documents
                    .iter()
                    .find(|row| row.id == id && row.kind == kind && !row.deleted)
                    .ok_or_else(refused)?;
                let site = orch_authz::documents::site_of(&graph, row).ok_or_else(refused)?;
                let target = targets::from_site(&graph, &site).ok_or_else(refused)?;
                match subject {
                    Subject::Document(_) => value.documents.push(id),
                    Subject::DocumentCount(_) => value.read_documents.push(id),
                    Subject::DocumentAccess(_) => value.access_documents.push(id),
                    Subject::Folder(_) => value.folders.push(id),
                    _ => value.folder_paths.push(target.clone()),
                }
                target
            }
            Subject::Form(form, revision) => {
                if form_revision(conn, form).await? != revision {
                    return Err(refused());
                }
                value.forms.push((form, revision));
                continue;
            }
            Subject::Reference(reference) => {
                let object = match reference {
                    FormReference::Object { object, id }
                    | FormReference::Published { object, id } => {
                        Some(ObjectRef { kind: object, id })
                    }
                    _ => None,
                };
                let target = match (reference, object) {
                    (FormReference::Chat { id }, _) => targets::chat(conn, id).await,
                    (_, Some(object)) if object.kind == ObjectKind::Machine => {
                        targets::machine_subject(conn, object.id).await
                    }
                    (_, Some(object)) => targets::page_object(conn, object).await,
                    _ => return Err(refused()),
                }
                .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
                value.references.push((reference, target));
                continue;
            }
            Subject::Creator | Subject::Email(_) => {
                let id = match subject {
                    Subject::Email(id) => *id,
                    _ => input.request.user,
                };
                let (_, kind, org) = accounts
                    .iter()
                    .find(|(user, _, _)| *user == id)
                    .ok_or_else(refused)?;
                let target =
                    targets::from_account_identity(&graph, id, kind, *org).ok_or_else(refused)?;
                if matches!(subject, Subject::Email(_)) {
                    value.people.push(target);
                } else {
                    value.creator = Some(target);
                }
                continue;
            }
        };
        if !target
            .node_ancestors
            .iter()
            .any(|node| node.id == Some(input.request.node))
        {
            return Err(refused());
        }
        groups
            .entry(target.object.kind)
            .or_default()
            .push(target.clone());
        value.targets.push(target);
    }
    // Bare structural path metadata is reachable only through actual readable/exportable descendants.
    for path in &value.paths {
        if !value.targets.iter().any(|target| {
            matches!(
                target.object.kind,
                KeyTargetKind::Object(ObjectKind::Document | ObjectKind::Thread)
                    | KeyTargetKind::Identity(
                        orch_access_contract::key_scope::KeyIdentityKind::Chat
                    )
            ) && target.object != path.object
                && target.node_ancestors.contains(&path.object)
        }) {
            return Err(refused());
        }
    }
    for path in &value.folder_paths {
        if !value.targets.iter().any(|target| {
            target.object.kind == KeyTargetKind::Object(ObjectKind::Document)
                && target.document_ancestors.contains(&path.object)
        }) {
            return Err(refused());
        }
    }
    let mut facts = vec![fact];
    if !root_only {
        facts.push(Facts::collection(
            Role::Contents,
            groups.into_iter().collect(),
        ));
    }
    if let Some(creator) = &value.creator {
        facts.push(Facts::one(
            Role::Creator,
            creator.object.kind,
            creator.clone(),
        ));
    }
    if !value.people.is_empty() {
        facts.push(Facts::collection(
            Role::People,
            vec![(
                KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::Account),
                value.people.clone(),
            )],
        ));
    }
    if !value.references.is_empty() {
        let mut groups = BTreeMap::<_, Vec<_>>::new();
        for (reference, target) in &value.references {
            let kind = match reference {
                FormReference::Chat { .. } => {
                    KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::Chat)
                }
                FormReference::Object { object, .. } | FormReference::Published { object, .. } => {
                    KeyTargetKind::Object(*object)
                }
            };
            groups.entry(kind).or_default().push(target.clone());
        }
        facts.push(Facts::collection(
            Role::References,
            groups.into_iter().collect(),
        ));
    }
    Ok((value, facts))
}
async fn require(
    conn: &mut Conn,
    principal: &Principal,
    input: &Input,
    value: &Value,
) -> Result<(), Error> {
    input
        .engine
        .export_place_seen(conn, principal, input.request.node)
        .await
        .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
    let threads = value
        .threads
        .iter()
        .chain(&value.read_threads)
        .copied()
        .collect::<Vec<_>>();
    let chats = value
        .chats
        .iter()
        .chain(&value.read_chats)
        .copied()
        .collect::<Vec<_>>();
    let documents = value
        .documents
        .iter()
        .chain(&value.read_documents)
        .copied()
        .collect::<Vec<_>>();
    let access = Access::of(conn, principal, &threads, &chats, &documents)
        .await
        .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
    if value.threads.iter().any(|id| {
        !access.reads_conversations.contains(id) || !access.exports_conversations.contains(id)
    }) || value
        .chats
        .iter()
        .any(|id| !access.reads_chats.contains(id) || !access.exports_chats.contains(id))
        || value.documents.iter().any(|id| {
            !access.reads_documents.contains(id) || !access.exports_documents.contains(id)
        })
    {
        return Err(refused());
    }
    if value
        .read_threads
        .iter()
        .any(|id| !access.reads_conversations.contains(id))
        || value
            .read_chats
            .iter()
            .any(|id| !access.reads_chats.contains(id))
        || value
            .read_documents
            .iter()
            .any(|id| !access.reads_documents.contains(id))
    {
        return Err(refused());
    }
    let nodes = holding(
        conn,
        principal,
        &value.nodes,
        PermissionName::OfNode(NodePermission::View),
    )
    .await
    .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
    if value.nodes.iter().any(|id| !nodes.contains(id)) {
        return Err(refused());
    }
    if !value.folders.is_empty() {
        let graph = crate::document_tree::load_entries_in(
            conn,
            crate::document_tree::Actor::Person(principal),
            &[],
            &value
                .folders
                .iter()
                .copied()
                .map(crate::document_tree::Item::Container)
                .collect::<Vec<_>>(),
            input.engine.inner.clock.time(),
        )
        .await
        .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
        let readable = graph
            .entries
            .into_iter()
            .filter_map(|listed| match listed.entry.item {
                crate::document_tree::Item::Container(id) => Some(id),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        if value.folders.iter().any(|id| !readable.contains(id)) {
            return Err(refused());
        }
    }
    if let Some(creator) = &value.creator {
        let plan = crate::operation_scope::Prepared::new(
            NodeExportCreate::ROUTE,
            declarations::CREATE.operation,
        )?;
        if !super::scope::permits(
            &plan,
            &declarations::CREATE,
            principal.caller(),
            Role::Creator,
            orch_api_contract::operations::FacetUsage::ResponseContent,
            creator,
        )
        .map_err(|error| Error::Native(EngineError::unreadable(error)))?
        {
            return Err(refused());
        }
    }
    for person in &value.people {
        if person.object.id != Some(principal.user()) && !principal.is_system_admin() {
            return Err(refused());
        }
        let plan = crate::operation_scope::Prepared::new(
            NodeExportCreate::ROUTE,
            declarations::CREATE.operation,
        )?;
        if !super::scope::permits(
            &plan,
            &declarations::CREATE,
            principal.caller(),
            Role::People,
            orch_api_contract::operations::FacetUsage::ResponseContent,
            person,
        )
        .map_err(|error| Error::Native(EngineError::unreadable(error)))?
        {
            return Err(refused());
        }
    }
    if !value.access_nodes.is_empty() {
        let held = holding(
            conn,
            principal,
            &value.access_nodes,
            PermissionName::Exact(Permission::AccessManage),
        )
        .await
        .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
        if value.access_nodes.iter().any(|id| !held.contains(id)) {
            return Err(refused());
        }
    }
    if !value.access_documents.is_empty() {
        let held = orch_authz::documents::listed(
            conn,
            principal,
            &value.access_documents,
            Policy::default(),
        )
        .await?;
        if value.access_documents.iter().any(|id| {
            !held
                .iter()
                .any(|row| row.document.id == *id && row.holds(Permission::AccessManage))
        }) {
            return Err(refused());
        }
    }
    let mut named_nodes = Vec::new();
    let mut named_documents = Vec::new();
    let mut named_chats = Vec::new();
    for (reference, target) in &value.references {
        let plan = crate::operation_scope::Prepared::new(
            NodeExportCreate::ROUTE,
            declarations::CREATE.operation,
        )?;
        let binding = declarations::CREATE
            .bindings
            .iter()
            .find(|binding| binding.source == Role::References)
            .ok_or_else(refused)?;
        let business = match reference {
            FormReference::Chat { .. } => {
                KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::Chat)
            }
            FormReference::Object { object, .. } | FormReference::Published { object, .. } => {
                KeyTargetKind::Object(*object)
            }
        };
        for index in crate::operation_scope::sources::indices(&plan, binding)? {
            if !plan.permits_binding(
                principal.caller(),
                index,
                business,
                std::slice::from_ref(target),
            )? {
                return Err(refused());
            }
        }
        match reference {
            FormReference::Chat { id } => named_chats.push(*id),
            FormReference::Object {
                object: ObjectKind::Document | ObjectKind::DocumentContainer,
                id,
            } => named_documents.push(*id),
            FormReference::Object { id, .. } => named_nodes.push(*id),
            FormReference::Published { .. } => {}
        }
    }
    if !named_nodes.is_empty() {
        let graph = orch_authz::engine::Graph::load(conn, &named_nodes).await?;
        let sites = named_nodes
            .iter()
            .filter_map(|id| graph.site_of(*id))
            .collect::<Vec<_>>();
        let mut permissions = Vec::new();
        for site in &sites {
            for permission in site.kind.permissions() {
                if !permissions.contains(&permission) {
                    permissions.push(permission);
                }
            }
        }
        let frame = orch_authz::engine::Frame::load(
            conn,
            principal,
            Policy::default(),
            graph,
            &sites,
            &permissions,
        )
        .await?;
        if sites.len() != named_nodes.len() || sites.iter().any(|site| !frame.holds_any(site)) {
            return Err(refused());
        }
    }
    if !named_documents.is_empty() {
        let readable =
            orch_authz::documents::listed(conn, principal, &named_documents, Policy::default())
                .await?;
        if named_documents.iter().any(|id| {
            !readable
                .iter()
                .any(|row| row.document.id == *id && row.holds(Permission::DocView))
        }) {
            return Err(refused());
        }
    }
    if !named_chats.is_empty() {
        let rows = orch_db::queries::chats::chats(conn, &named_chats).await?;
        let readable = orch_authz::chats::listed_access(conn, principal, &rows).await?;
        if named_chats.iter().any(|id| !readable.contains_key(id)) {
            return Err(refused());
        }
    }
    let user = orch_db::queries::identity::user_by_id(conn, principal.user())
        .await?
        .ok_or_else(refused)?;
    if let Some(hold) = crate::operation_scope::account_policy::session_hold(
        conn,
        &user,
        input.request.credential.browser_session(),
    )
    .await?
        && value
            .targets
            .iter()
            .chain(value.references.iter().map(|(_, target)| target))
            .any(|target| {
                target.node_ancestors.iter().any(|node| {
                    node.kind == KeyTargetKind::Object(ObjectKind::Org)
                        && node.id.is_some_and(|id| hold.orgs.contains(&id))
                })
            })
    {
        return Err(refused());
    }
    Ok(())
}
async fn locks(conn: &mut Conn, value: &Value) -> Result<(), Error> {
    let ids = value
        .targets
        .iter()
        .chain(value.creator.iter())
        .chain(value.people.iter())
        .chain(value.references.iter().map(|(_, target)| target))
        .flat_map(|target| target.node_ancestors.iter().filter_map(|node| node.id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM documents WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(
            value
                .documents
                .iter()
                .chain(&value.read_documents)
                .chain(&value.access_documents)
                .copied()
                .collect::<Vec<_>>(),
        )
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let folders = value
        .targets
        .iter()
        .flat_map(|target| {
            target
                .document_ancestors
                .iter()
                .filter_map(|parent| parent.id)
        })
        .chain(value.folders.iter().copied())
        .chain(
            value
                .folder_paths
                .iter()
                .filter_map(|target| target.object.id),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query("SELECT id FROM document_containers WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(folders)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM chats WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(&value.chat_ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let ids = |kind| {
        value
            .references
            .iter()
            .filter_map(|(reference, _)| match reference {
                FormReference::Object { object, id } | FormReference::Published { object, id }
                    if *object == kind =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    sqlx::query("SELECT id FROM documents WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(ids(ObjectKind::Document))
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM document_containers WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(ids(ObjectKind::DocumentContainer))
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM agents WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(ids(ObjectKind::Agent))
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM providers WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(ids(ObjectKind::Provider))
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM machines WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(ids(ObjectKind::Machine))
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM environments WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(ids(ObjectKind::Environment))
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let chats = value
        .references
        .iter()
        .filter_map(|(reference, _)| match reference {
            FormReference::Chat { id } => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    sqlx::query("SELECT id FROM chats WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(chats)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let accounts = value
        .creator
        .iter()
        .chain(&value.people)
        .filter_map(|target| target.object.id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query("SELECT id FROM users WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(accounts)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}
macro_rules! source {
    ($ty:ty,$decl:expr,$root:expr) => {
        impl Source for $ty {
            type Role = Role;
            type Input = Input;
            type Value = Value;
            const ROUTE: Route = NodeExportCreate::ROUTE;
            const DECLARATION: &'static Declaration<Role> = &$decl;
            fn prepare<'a>(
                conn: &'a mut Conn,
                _: &'a Caller,
                input: &'a Input,
                _: Time,
            ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
                Box::pin(prepare(conn, input, $root))
            }
            fn require<'a>(
                conn: &'a mut Conn,
                principal: &'a Principal,
                input: &'a Input,
                value: &'a Value,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(require(conn, principal, input, value))
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
                    Ok(old.targets == fresh.targets
                        && old.creator == fresh.creator
                        && old.people == fresh.people
                        && old.references == fresh.references
                        && old.forms == fresh.forms)
                })
            }
        }
    };
}
source!(Root, ROOT_DECLARATION, true);
source!(Body, BODY_DECLARATION, false);
source!(Entry, declarations::CREATE, false);
