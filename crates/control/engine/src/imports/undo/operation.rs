//! Actual undo sources and retained original-operation checks for queued undo work.

use super::{Prepared, Subject};
use crate::{
    Engine, EngineError,
    operation_scope::{
        continuing::{self, Error},
        sources::Facts,
        targets,
    },
};
use orch_access_contract::{
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::import_undo_sources::Source;
use orch_authz::engine::Graph;
use orch_db::{Conn, queries::chats::ChatRow};
use orch_types::Id;
use std::collections::BTreeMap;

fn refused() -> Error {
    Error::Refused
}
fn target<T>(result: Result<T, targets::TargetError>) -> Result<T, Error> {
    match result {
        Ok(value) => Ok(value),
        Err(targets::TargetError::Missing) => Err(Error::Refused),
        Err(targets::TargetError::Db(error)) => Err(error.into()),
        Err(targets::TargetError::Authz(error)) => Err(error.into()),
    }
}
async fn chat_targets(
    conn: &mut Conn,
    rows: &[ChatRow],
) -> Result<std::collections::HashMap<Id, KeyResolvedTarget>, Error> {
    let nodes = rows
        .iter()
        .map(|row| row.node_id.unwrap_or(row.org_id))
        .collect::<Vec<_>>();
    let graph = Graph::load(conn, &nodes).await?;
    rows.iter()
        .map(|row| target(targets::from_chat_graph(&graph, row)).map(|target| (row.id, target)))
        .collect()
}

pub async fn facts(
    conn: &mut Conn,
    native: &Prepared,
) -> Result<
    (
        Vec<Facts<Source>>,
        BTreeMap<(KeyTargetKind, Id), KeyResolvedTarget>,
    ),
    Error,
> {
    let import = target(targets::from_import(conn, &native.import).await)?;
    let mut found = BTreeMap::new();
    found.insert((import.object.kind, native.import.id), import.clone());
    let mut facts = vec![Facts::one(Source::Import, import.object.kind, import)];
    let nodes = native
        .subjects
        .iter()
        .filter_map(|subject| match subject {
            Subject::Node(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut graph = Graph::load(conn, &nodes).await.map_err(EngineError::from)?;
    let ids = native
        .subjects
        .iter()
        .filter_map(|subject| match subject {
            Subject::Document(id) | Subject::Folder(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut documents = BTreeMap::new();
    for document in orch_authz::documents::read_into(conn, &mut graph, &ids)
        .await
        .map_err(EngineError::from)?
    {
        let site = orch_authz::documents::site_of(&graph, &document).ok_or_else(refused)?;
        documents.insert(
            (document.kind, document.id),
            targets::from_site(&graph, &site).ok_or(Error::Refused)?,
        );
    }
    let ids = native
        .subjects
        .iter()
        .filter_map(|subject| match subject {
            Subject::Chat(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let rows = orch_db::queries::chats::chats(conn, &ids).await?;
    let chats = chat_targets(conn, &rows).await?;
    let mut groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>> = BTreeMap::new();
    for subject in &native.subjects {
        let target = match subject {
            Subject::Node(id) => targets::from_node_identity(&graph, *id).ok_or_else(refused)?,
            Subject::Document(id) => documents
                .get(&(ObjectKind::Document, *id))
                .cloned()
                .ok_or_else(refused)?,
            Subject::Folder(id) => documents
                .get(&(ObjectKind::DocumentContainer, *id))
                .cloned()
                .ok_or_else(refused)?,
            Subject::Chat(id) => chats.get(id).cloned().ok_or_else(refused)?,
            Subject::Resource { kind, id } => target(
                targets::resource_identity(
                    conn,
                    orch_access_contract::decisions::ObjectRef {
                        kind: *kind,
                        id: *id,
                    },
                )
                .await,
            )?,
        };
        let id = target.object.id.ok_or(Error::Refused)?;
        found.insert((target.object.kind, id), target.clone());
        groups.entry(target.object.kind).or_default().push(target);
    }
    facts.push(Facts::collection(
        Source::Subjects,
        groups.into_iter().collect(),
    ));
    Ok((facts, found))
}

use crate::operation_scope::{
    continuing::Source as NativeSource,
    request_origin::{self, Credential},
};
use futures::future::BoxFuture;
use orch_api_contract::{
    import_undo_sources::{self as declarations, Source as Role},
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::queries::{imports as rows, jobs};
use orch_types::{Clock, Hash, Json, Time};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const REQUEST: &str = "undo_operation";
const REQUIRED: &str = "undo_operation_required";

/// The private accepted undo association, never an HTTP body or an authority lease.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    job: Id,
    import: Id,
    user: Id,
    node: Id,
    request: Hash,
    origin: CapturedOperation,
    credential: Credential,
    until: Option<Time>,
}
impl Request {
    /// Retain the real newly queued job and the actual original raw credential on its transaction.
    pub async fn retain(
        conn: &mut Conn,
        made: &crate::imports::Made,
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        if origin.operation() != declarations::CREATE.operation
            || !credential.matches(caller)
            || made.import.user_id != Some(caller.user())
            || made.import.undo_job_id != Some(made.job.id)
            || made.job.created_by != Some(caller.user())
            || made.job.kind != jobs::kind::IMPORT
        {
            return Err(EngineError::unreadable(
                "the accepted native undo association changed",
            ));
        }
        let request = Self {
            job: made.job.id,
            import: made.import.id,
            user: caller.user(),
            node: made
                .import
                .node_id
                .ok_or_else(|| EngineError::unreadable("the undo destination is missing"))?,
            request: Hash::of(&serde_json::to_vec(&made.import.request)?),
            origin,
            credential,
            until,
        };
        jobs::merge_plan(
            conn,
            made.job.id,
            &serde_json::json!({(REQUEST):&request,(REQUIRED):true}),
            made.job.updated_at,
        )
        .await?;
        Ok(request)
    }
    /// Present invalid provenance is never treated as an ordinary legacy job.
    pub fn from_job(row: &jobs::JobRow) -> Result<Option<Self>, EngineError> {
        let Some(value) = row.plan.get(REQUEST) else {
            return if row.plan.get(REQUIRED).and_then(Json::as_bool) == Some(true) {
                Err(EngineError::unreadable(
                    "the original undo source is missing",
                ))
            } else {
                Ok(None)
            };
        };
        let request: Self = serde_json::from_value(value.clone())?;
        if request.job != row.id
            || request.origin.operation() != declarations::CREATE.operation
            || row.kind != jobs::kind::IMPORT
            || row.created_by != Some(request.user)
            || request.user != request.origin.user()
            || row.plan["undo"] != true
            || row.plan["import"] != serde_json::json!(request.import)
            || row.plan["user"] != serde_json::json!(request.user)
            || row.plan["node"] != serde_json::json!(request.node)
        {
            return Err(EngineError::unreadable(
                "the retained native undo source changed",
            ));
        }
        Ok(Some(request))
    }
    async fn verify(&self, conn: &mut Conn) -> Result<rows::ImportRow, Error> {
        let job = jobs::job(conn, self.job).await?.ok_or(Error::Refused)?;
        let same = Self::from_job(&job)
            .map_err(Error::Native)?
            .ok_or(Error::Refused)?;
        let row = rows::get(conn, self.import).await?.ok_or(Error::Refused)?;
        if same != *self
            || row.user_id != Some(self.user)
            || row.node_id != Some(self.node)
            || row.undo_job_id != Some(self.job)
            || Hash::of(&serde_json::to_vec(&row.request).map_err(EngineError::from)?)
                != self.request
            || row.state != rows::state::UNDOING
        {
            return Err(Error::Refused);
        }
        Ok(row)
    }
    /// Restore the real credential before observing native inputs; this does not admit a write.
    pub async fn caller(&self, conn: &mut Conn, clock: &dyn Clock) -> Result<Caller, Error> {
        self.verify(conn).await?;
        let (caller, _) = request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.until,
            declarations::CREATE.operation,
        )
        .await?;
        Ok(caller)
    }
    /// A fresh original-operation frame after all native source locks, on the caller's transaction.
    pub async fn frame(&self, engine: &Engine, conn: &mut Conn, work: Work) -> Result<Cut, Error> {
        let (caller, until) = request_origin::current(
            conn,
            engine.inner.clock.as_ref(),
            &self.origin,
            &self.credential,
            self.until,
            declarations::CREATE.operation,
        )
        .await?;
        let frame = continuing::prepare_with_caller::<Entry>(
            conn,
            engine.inner.clock.as_ref(),
            &self.origin,
            &caller,
            Input {
                engine: engine.clone(),
                request: self.clone(),
                subjects: work.subjects(),
                deleting: matches!(work, Work::Node(_)),
            },
        )
        .await?;
        if until.is_some_and(|until| engine.inner.clock.time().get() >= until.get()) {
            return Err(Error::Refused);
        }
        let parts = frame.into_parts();
        Ok(Cut {
            principal: parts.principal,
            first: Some(parts.first_effect),
            until,
        })
    }
}

/// The actual live cut kept on the still-open native transaction through its writes.
pub struct Cut {
    principal: Principal,
    first: Option<continuing::FirstEffect>,
    until: Option<Time>,
}
impl Cut {
    pub fn principal(&self) -> &Principal {
        &self.principal
    }
    /// Called immediately before each native SQL effect, including the actual first one.
    pub fn before(&mut self, clock: &dyn Clock) -> Result<(), Error> {
        self.deadline(clock)?;
        if let Some(first) = self.first.take() {
            first.check(clock)?;
        }
        Ok(())
    }
    pub fn deadline(&self, clock: &dyn Clock) -> Result<(), Error> {
        if !self.principal.caller().key_deadline_allows(clock)
            || self
                .until
                .is_some_and(|until| clock.time().get() >= until.get())
        {
            return Err(Error::Refused);
        }
        Ok(())
    }
}

/// The native phase that produced a page, never a client-selected action label.
pub enum Work {
    /// Created documents selected by the native undo pager.
    Documents(Vec<Id>),
    /// Created document folders selected by the native undo pager.
    Folders(Vec<Id>),
    /// A created place whose native emptiness premise permits deleting it.
    Node(Id),
    /// Imported rows of an existing chat.
    Chat(Id),
    /// Imported history of an existing thread; this never deletes its home resources.
    Thread(Id),
}
impl Work {
    fn subjects(&self) -> Vec<Subject> {
        match self {
            Self::Documents(ids) => ids.iter().copied().map(Subject::Document).collect(),
            Self::Folders(ids) => ids.iter().copied().map(Subject::Folder).collect(),
            Self::Node(id) | Self::Thread(id) => vec![Subject::Node(*id)],
            Self::Chat(id) => vec![Subject::Chat(*id)],
        }
    }
}

pub struct Entry;
pub struct Input {
    engine: Engine,
    request: Request,
    subjects: Vec<Subject>,
    deleting: bool,
}
pub struct Value {
    native: Prepared,
    targets: BTreeMap<(KeyTargetKind, Id), KeyResolvedTarget>,
}
impl NativeSource for Entry {
    type Role = Role;
    type Input = Input;
    type Value = Value;
    const ROUTE: Route = orch_api_contract::imports::ImportUndo::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::CREATE;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        _: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let import = input.request.verify(conn).await?;
            if caller.user() != input.request.user {
                return Err(Error::Refused);
            }
            let mut nodes = Vec::new();
            let mut documents = Vec::new();
            let mut folders = Vec::new();
            let mut chats = Vec::new();
            for subject in &input.subjects {
                match subject {
                    Subject::Node(id) => nodes.push(*id),
                    Subject::Document(id) => documents.push(*id),
                    Subject::Folder(id) => folders.push(*id),
                    Subject::Chat(id) => chats.push(*id),
                    Subject::Resource { .. } => return Err(Error::Refused),
                }
            }
            if !rows::undo_subjects_present(
                conn,
                input.request.import,
                &nodes,
                &documents,
                &folders,
                &chats,
            )
            .await?
            {
                return Err(Error::Refused);
            }
            let mut native = Prepared {
                import,
                subjects: input.subjects.clone(),
            };
            for subject in &input.subjects {
                if input.deleting
                    && let Subject::Node(id) = subject
                {
                    let deletion = input
                        .engine
                        .inspect_home_deletion_in(conn, *id)
                        .await
                        .map_err(|error| EngineError::unreadable(error.to_string()))?;
                    for resource in deletion.resources() {
                        native.subjects.push(Subject::Resource {
                            kind: resource.kind.parse().map_err(EngineError::unreadable)?,
                            id: resource.id,
                        });
                    }
                }
            }
            native.subjects.sort_unstable();
            native.subjects.dedup();
            let (mut facts, targets) = facts(conn, &native).await?;
            facts.push(creator(conn, caller.user()).await?);
            Ok((Value { native, targets }, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if principal.user() != input.request.user {
                return Err(Error::Refused);
            }
            let user =
                orch_db::queries::transaction_guard::lock_native_actor(conn, principal.user())
                    .await?
                    .ok_or(Error::Refused)?;
            if let Some(hold) = crate::operation_scope::account_policy::session_hold(
                conn,
                &user,
                input.request.credential.browser_session(),
            )
            .await?
            {
                let held = hold.orgs.into_iter().collect::<BTreeSet<_>>();
                if value.targets.values().any(|target| {
                    target.node_ancestors.iter().any(|node| {
                        node.kind == KeyTargetKind::Object(ObjectKind::Org)
                            && node.id.is_some_and(|id| held.contains(&id))
                    })
                }) {
                    return Err(Error::Refused);
                }
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        _: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            rows::lock(conn, input.request.import)
                .await?
                .ok_or(Error::Refused)?;
            for subject in input.subjects.iter().copied().collect::<BTreeSet<_>>() {
                match subject {
                    Subject::Document(id) => {
                        orch_db::queries::documents::containers::lock_document_scope(conn, id)
                            .await?
                    }
                    Subject::Folder(id) => {
                        orch_db::queries::documents::containers::lock(conn, id)
                            .await?
                            .ok_or(Error::Refused)?;
                    }
                    Subject::Node(id) if input.deleting => {
                        let prepared = input
                            .engine
                            .prepare_home_deletion_in(conn, id, None)
                            .await
                            .map_err(|error| EngineError::unreadable(error.to_string()))?;
                        orch_db::queries::documents::containers::lock_scopes(
                            conn,
                            &prepared.node_ids(),
                        )
                        .await?;
                    }
                    Subject::Node(id) => {
                        orch_db::queries::threads::tools::lock_threads(conn, &[id]).await?
                    }
                    Subject::Chat(id) => {
                        orch_db::queries::chats::lock(conn, id)
                            .await?
                            .ok_or(Error::Refused)?;
                    }
                    Subject::Resource { .. } => return Err(Error::Refused),
                }
            }
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(opened.targets == fresh.targets
                && opened.native.import.request == fresh.native.import.request
                && opened.native.subjects == fresh.native.subjects)
        })
    }
}

/// The actual existing account named by the native job, independent of generated job fields.
pub async fn creator(conn: &mut Conn, user: Id) -> Result<Facts<Role>, Error> {
    let target = target(targets::account(conn, user).await)?;
    Ok(Facts::collection(
        Role::Creator,
        vec![(target.object.kind, vec![target])],
    ))
}
