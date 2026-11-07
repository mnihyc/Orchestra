//! One initial source supplier, independent native entry commits, and one completed report.

use super::*;
use crate::key_operations::{
    blob_sources,
    native::{self, continuing, effects, reply},
    publication, targets,
};
use crate::middleware::transaction_guard::{self, HttpIdentityMode};
use crate::routes::access::page::NameBounds;
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    common::ReceiptProjection,
    document_sources::{self as sources, TextCreationSource as Source},
    documents::{
        DocumentImportReceipt, DocumentImportReceiptEntry, DocumentImportResponse,
        DocumentImportResult,
    },
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation};
use orch_chat::following::BlobOrigin;
use orch_engine::{
    EngineError,
    operation_scope::{
        blob_sources as stored,
        continuing::{Error as NativeError, Source as NativeSource},
    },
};
use std::{collections::HashSet, sync::Arc};

const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);
const BYTE_ROLES: &[(stored::SourceKind, Source)] = &[
    (stored::SourceKind::Chat, Source::UploadChats),
    (stored::SourceKind::Thread, Source::UploadThreads),
    (stored::SourceKind::Document, Source::UploadDocuments),
    (stored::SourceKind::Environment, Source::UploadEnvironments),
    (stored::SourceKind::RetainedUpload, Source::Uploads),
];

pub(super) struct Imports;
#[derive(Clone)]
pub(super) struct Sources {
    node: Id,
    user: Id,
    selected: Vec<(Hash, BlobOrigin)>,
}
impl Sources {
    pub(super) fn for_hash(&self, hash: Hash) -> Self {
        let mut source = self.clone();
        source.selected.retain(|(selected, _)| *selected == hash);
        source
    }
}
pub(super) struct Initial {
    body: DocumentImportRequest,
    source: Sources,
    source_errors: Vec<Option<DocumentImportResultError>>,
    limit: i64,
}
#[derive(Clone)]
struct StoredReplay {
    document: Id,
}
#[derive(Clone)]
struct Parsed {
    name: String,
    id: Id,
    hash: Hash,
    replay: Option<StoredReplay>,
    title: String,
    blocks: Vec<NewBlock>,
    files: Vec<rows::HeldFile>,
    warnings: Vec<String>,
    people: Vec<(String, Option<Id>)>,
}
pub(super) struct Fetched {
    source: Sources,
    expanded: i64,
    entries: Vec<Result<Arc<Parsed>, Box<DocumentImportResultEntry>>>,
}
#[derive(Clone)]
struct Entry {
    source: Sources,
    parsed: Arc<Parsed>,
    expanded: i64,
    blocks: Vec<NewBlock>,
    copied: Vec<build::WrittenLabelSource>,
}
pub(super) struct Proposal {
    source: Sources,
    entries: Vec<Result<Entry, Box<DocumentImportResultEntry>>>,
    facts: Vec<native::Facts<Source>>,
}
pub(super) struct Ready {
    entries: Vec<Result<Entry, Box<DocumentImportResultEntry>>>,
    facts: Vec<native::Facts<Source>>,
}
#[derive(Clone)]
struct Outcome {
    name: String,
    document: Option<Id>,
    warnings: Vec<String>,
    error: Option<DocumentImportResultError>,
}
struct Completed {
    outcomes: Vec<Outcome>,
    facts: Vec<native::Facts<Source>>,
}
struct SourceValue {
    replay: Option<Id>,
    facts: Vec<native::Facts<Source>>,
    targets: Vec<KeyResolvedTarget>,
}
struct InputSources;
struct EntrySources;
struct Report;
struct Readback {
    documents: Vec<text_creation::Subject>,
}
struct ReportView {
    documents: Vec<text_creation::View>,
}

fn native_error(error: ApiError) -> NativeError {
    NativeError::Native(EngineError::BeforeEffect(Box::new(error)))
}
fn api_error(error: NativeError) -> ApiError {
    match error {
        NativeError::Refused => ApiError::new(
            ErrorCode::AuthzDenied,
            "the original import source is no longer current",
        ),
        NativeError::Binding(error) => crate::key_operations::admission::binding_error(error),
        NativeError::Native(error) => ApiError::engine(error),
    }
}
fn error_info(error: ApiError) -> DocumentImportResultError {
    DocumentImportResultError {
        code: error.code().as_str().into(),
        message: error.problem.detail,
    }
}
fn validate(body: &DocumentImportRequest) -> ApiResult<()> {
    creation::client_id(&body.client_id)?;
    if body.files.is_empty() || body.files.len() > ENTRIES_MAX {
        return Err(invalid("files", "select one to two hundred sources"));
    }
    if body.files.iter().any(|file| file.format == Format::Archive) && body.files.len() != 1 {
        return Err(invalid(
            "files",
            "select several text files or one archive, not both",
        ));
    }
    Ok(())
}
fn byte_facts(bindings: &[stored::Binding]) -> Vec<native::Facts<Source>> {
    BYTE_ROLES
        .iter()
        .map(|(kind, role)| {
            let mut targets = Vec::<KeyResolvedTarget>::new();
            for binding in bindings.iter().filter(|binding| binding.source == *kind) {
                if !targets
                    .iter()
                    .any(|target| target.object == binding.target.object)
                {
                    targets.push(binding.target.clone());
                }
            }
            native::Facts::collection(
                *role,
                if targets.is_empty() {
                    Vec::new()
                } else {
                    vec![(kind.kind(), targets)]
                },
            )
        })
        .collect()
}
async fn selected_bindings(conn: &mut Conn, source: &Sources) -> ApiResult<Vec<stored::Binding>> {
    let mut bindings = Vec::new();
    for (_, origin) in &source.selected {
        let (source_kind, target) = match origin {
            BlobOrigin::RetainedByCaller => (
                stored::SourceKind::RetainedUpload,
                targets::account(conn, source.user).await?,
            ),
            BlobOrigin::Chat(id) => (stored::SourceKind::Chat, targets::chat(conn, *id).await?),
            BlobOrigin::Thread(id) => (
                stored::SourceKind::Thread,
                targets::page_object(
                    conn,
                    orch_access_contract::decisions::ObjectRef {
                        kind: ObjectKind::Thread,
                        id: *id,
                    },
                )
                .await?,
            ),
            BlobOrigin::Document(id) => (
                stored::SourceKind::Document,
                read_operation::identity(conn, *id).await?,
            ),
            BlobOrigin::Environment(id) => (
                stored::SourceKind::Environment,
                targets::page_object(
                    conn,
                    orch_access_contract::decisions::ObjectRef {
                        kind: ObjectKind::Environment,
                        id: *id,
                    },
                )
                .await?,
            ),
        };
        bindings.push(stored::Binding {
            source: source_kind,
            target,
        });
    }
    Ok(bindings)
}
async fn source_value(conn: &mut Conn, source: &Sources) -> ApiResult<SourceValue> {
    let destination = targets::node(conn, source.node).await?;
    let bindings = selected_bindings(conn, source).await?;
    let mut targets = vec![destination.clone()];
    targets.extend(bindings.iter().map(|binding| binding.target.clone()));
    let mut facts = vec![native::Facts::one(
        Source::Destination,
        DOCUMENT,
        destination,
    )];
    facts.extend(byte_facts(&bindings));
    Ok(SourceValue {
        replay: None,
        facts,
        targets,
    })
}
async fn require_inputs(
    conn: &mut Conn,
    principal: &Principal,
    input: &Sources,
    value: &SourceValue,
) -> Result<(), NativeError> {
    if principal.user() != input.user {
        return Err(NativeError::Refused);
    }
    let decision = orch_authz::nodes::check(
        conn,
        principal,
        input.node,
        Permission::DocCreate,
        Act::Use,
        Policy::default(),
    )
    .await?;
    if !decision.is_allowed() {
        return Err(NativeError::Refused);
    }
    let facts = value
        .facts
        .iter()
        .filter(|fact| {
            sources::TEXT_IMPORT_INPUTS
                .bindings
                .iter()
                .any(|binding| binding.source == fact.source)
        })
        .cloned()
        .collect::<Vec<_>>();
    let plan = orch_engine::operation_scope::sources::bind_declared(
        NodeDocumentImport::ROUTE,
        &sources::TEXT_IMPORT_INPUTS,
        &facts,
    )?;
    let hashes = input
        .selected
        .iter()
        .map(|(hash, _)| *hash)
        .collect::<Vec<_>>();
    stored::prepare_selected(conn, principal, &plan, &hashes, &[], &input.selected)
        .await
        .map_err(|_| NativeError::Refused)?;
    Ok(())
}
impl NativeSource for InputSources {
    type Role = Source;
    type Input = Sources;
    type Value = SourceValue;
    const ROUTE: Route = NodeDocumentImport::ROUTE;
    const DECLARATION: &'static Declaration<Source> = &sources::TEXT_IMPORT_INPUTS;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Sources,
        _now: Time,
    ) -> BoxFuture<'a, Result<(SourceValue, Vec<native::Facts<Source>>), NativeError>> {
        Box::pin(async move {
            let value = source_value(conn, input).await.map_err(native_error)?;
            let facts = value.facts.clone();
            Ok((value, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Sources,
        value: &'a SourceValue,
    ) -> BoxFuture<'a, Result<(), NativeError>> {
        Box::pin(require_inputs(conn, principal, input, value))
    }
    fn locks<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Sources,
        _value: &'a SourceValue,
    ) -> BoxFuture<'a, Result<(), NativeError>> {
        Box::pin(async { Ok(()) })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Sources,
        old: &'a SourceValue,
        fresh: &'a SourceValue,
    ) -> BoxFuture<'a, Result<bool, NativeError>> {
        Box::pin(async move { Ok(old.targets == fresh.targets && old.replay == fresh.replay) })
    }
}
pub(super) async fn check_inputs(
    ctx: &Ctx,
    origin: &CapturedOperation,
    source: &Sources,
) -> ApiResult<()> {
    let mut tx = ctx.app.db.begin().await?;
    let (frame, deadline) =
        continuing::prepare_initial_on::<InputSources>(ctx, &mut tx, origin, source.clone())
            .await?;
    frame
        .check_deadline(ctx.app.clock.as_ref())
        .map_err(api_error)?;
    deadline.check()?;
    tx.commit().await.map_err(DbError::from)?;
    deadline.check()
}

impl effects::Initial for Imports {
    type Input = Initial;
    type Fetched = Fetched;
    fn declaration(_input: &Initial) -> &'static Declaration<Source> {
        &sources::TEXT_IMPORT_INPUTS
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: DocumentImportRequest,
    ) -> BoxFuture<'a, ApiResult<(Initial, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            validate(&body)?;
            let node = ctx.id("id")?;
            let target = native::Facts::one(
                Source::Destination,
                DOCUMENT,
                targets::node(conn, node).await?,
            );
            let known = native::bind_known(
                ctx,
                &sources::TEXT_IMPORT_INPUTS,
                std::slice::from_ref(&target),
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            authorization::require(
                conn,
                &principal,
                node,
                exact(Permission::DocCreate),
                Act::Use,
                Policy::default(),
            )
            .await?;
            placed(conn, node).await?;
            let mut bindings = Vec::new();
            let mut selected = Vec::new();
            let mut source_errors = Vec::new();
            for file in &body.files {
                match blob_sources::prepare(ctx, conn, caller, &known, &[file.blob], &[]).await {
                    Ok(found) => {
                        bindings.extend(found.bindings);
                        for item in found.selected {
                            if !selected.contains(&item) {
                                selected.push(item);
                            }
                        }
                        source_errors.push(None);
                    }
                    Err(error) => source_errors.push(Some(error_info(error))),
                }
            }
            let mut facts = vec![target];
            facts.extend(byte_facts(&bindings));
            let limit = allowance(conn, node).await?;
            Ok((
                Initial {
                    body,
                    source: Sources {
                        node,
                        user: caller.user(),
                        selected,
                    },
                    source_errors,
                    limit,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Initial,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let value = source_value(conn, &input.source).await?;
            require_inputs(conn, principal, &input.source, &value)
                .await
                .map_err(api_error)
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Initial,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let origin = CapturedOperation::capture(caller).map_err(ApiError::internal)?;
            let selected = inputs(
                ctx,
                &input.body,
                input.limit,
                &origin,
                &input.source,
                &input.source_errors,
            )
            .await?;
            let expanded = selected.iter().fold(0_i64, |size, file| {
                size.saturating_add(i64::try_from(file.bytes.len()).unwrap_or(i64::MAX))
            });
            let companions = Arc::new(
                selected
                    .iter()
                    .filter(|file| file.companion && file.error.is_none())
                    .map(|file| {
                        (
                            file.name.clone(),
                            Companion {
                                bytes: file.bytes.clone(),
                                media_type: orch_types::media::media_type(&file.name, &[]).into(),
                            },
                        )
                    })
                    .collect::<HashMap<_, _>>(),
            );
            let siblings = Arc::new(
                selected
                    .iter()
                    .enumerate()
                    .filter(|(_, file)| {
                        file.error.is_none()
                            && !file.companion
                            && reader_format(file.format).is_some()
                    })
                    .map(|(index, file)| {
                        (
                            file.name.clone(),
                            rows::copy::identity(
                                input.source.user,
                                input.source.node,
                                &input.body.client_id,
                                &format!("import:{index}:{}", file.name),
                            ),
                        )
                    })
                    .collect::<HashMap<_, _>>(),
            );
            let mut entries = Vec::new();
            for mut file in selected {
                if let Some(error) = file.error.take() {
                    entries.push(Err(Box::new(refused(file.name, error))));
                    continue;
                }
                if file.companion {
                    continue;
                }
                let name = file.name.clone();
                let file_source = input.source.for_hash(file.source);
                let parsed = async {
                    let format = reader_format(file.format).ok_or_else(|| {
                        invalid(
                            "files",
                            "an archive entry must be Markdown, HTML, Word or text",
                        )
                    })?;
                    check_inputs(ctx, &origin, &file_source).await?;
                    let prepared =
                        tokio::task::spawn_blocking(move || convert::prepare(format, &file.bytes))
                            .await
                            .map_err(ApiError::internal)?
                            .map_err(|error| invalid("files", &error.to_string()))?;
                    let id = *siblings.get(&name).ok_or_else(|| {
                        ApiError::internal("the native import lost its stable identity")
                    })?;
                    let mut tx = ctx.app.db.begin().await?;
                    let (frame, deadline) = continuing::prepare_initial_on::<InputSources>(
                        ctx,
                        &mut tx,
                        &origin,
                        file_source.clone(),
                    )
                    .await?;
                    let replay: Option<Id> =
                        sqlx::query_scalar("SELECT id FROM documents WHERE id=$1")
                            .bind(id)
                            .fetch_optional(&mut *tx)
                            .await
                            .map_err(DbError::from)?;
                    if let Some(document) = replay {
                        frame
                            .check_deadline(ctx.app.clock.as_ref())
                            .map_err(api_error)?;
                        deadline.check()?;
                        tx.commit().await.map_err(DbError::from)?;
                        deadline.check()?;
                        // Preserve native import replay before finishing or embedded suppliers.
                        // The full entry frame reopens this real stored identity and native door.
                        return Ok(Arc::new(Parsed {
                            name: name.clone(),
                            id,
                            hash: file.source,
                            replay: Some(StoredReplay { document }),
                            title: String::new(),
                            blocks: Vec::new(),
                            files: Vec::new(),
                            warnings: Vec::new(),
                            people: Vec::new(),
                        }));
                    }
                    let names = prepared.mentioned_names();
                    let found = rows::users_by_username(&mut tx, &names).await?;
                    let people = found
                        .into_iter()
                        .map(|(name, id)| (name.to_ascii_lowercase(), id))
                        .collect::<HashMap<_, _>>();
                    let witness = names
                        .into_iter()
                        .map(|name| {
                            let name = name.to_ascii_lowercase();
                            let id = people.get(&name).copied();
                            (name, id)
                        })
                        .collect();
                    frame
                        .check_deadline(ctx.app.clock.as_ref())
                        .map_err(api_error)?;
                    deadline.check()?;
                    tx.commit().await.map_err(DbError::from)?;
                    deadline.check()?;

                    let siblings = Arc::clone(&siblings);
                    let companions = Arc::clone(&companions);
                    let source_name = name.clone();
                    let (mut blocks, mut warnings, embedded) =
                        tokio::task::spawn_blocking(move || {
                            let directory = Entries {
                                people,
                                base: source_name.rsplit_once('/').map_or("", |(base, _)| base),
                                siblings: &siblings,
                                companions: &companions,
                                asked: std::sync::Mutex::new(HashMap::new()),
                            };
                            let read = prepared.finish(&directory);
                            let mut embedded = read.embedded;
                            embedded.extend(directory.named_files(&read.blocks));
                            (
                                read.blocks,
                                read.warnings
                                    .into_iter()
                                    .map(|warning| warning.message)
                                    .collect::<Vec<_>>(),
                                embedded,
                            )
                        })
                        .await
                        .map_err(ApiError::internal)?;
                    check_inputs(ctx, &origin, &file_source).await?;
                    let files = super::super::convert::hold(
                        ctx,
                        &embedded,
                        &mut HashSet::new(),
                        input.source.user,
                        ctx.now,
                    )
                    .await?;
                    check_inputs(ctx, &origin, &file_source).await?;
                    let title = title(&name, &mut blocks, &mut warnings);
                    Ok::<_, ApiError>(Arc::new(Parsed {
                        name: name.clone(),
                        id,
                        hash: file.source,
                        replay: None,
                        title,
                        blocks,
                        files,
                        warnings,
                        people: witness,
                    }))
                }
                .await;
                entries.push(parsed.map_err(|error| Box::new(refused(name, error))));
            }
            let used = entries
                .iter()
                .filter_map(|entry| entry.as_ref().ok().map(|entry| entry.hash))
                .collect::<HashSet<_>>();
            let mut source = input.source;
            source.selected.retain(|(hash, _)| used.contains(hash));
            Ok(Fetched {
                source,
                expanded,
                entries,
            })
        })
    }
}

async fn written_targets(
    conn: &mut Conn,
    copied: &[build::WrittenLabelSource],
) -> ApiResult<Vec<(build::WrittenLabelSource, KeyResolvedTarget)>> {
    let mut found = Vec::new();
    for source in copied {
        found.push((
            *source,
            super::super::operation::source_target(conn, *source).await?,
        ));
    }
    Ok(found)
}
impl effects::Effect for Imports {
    type Source = Source;
    type Query = ();
    type Body = DocumentImportRequest;
    type Value = Proposal;
    type Output = Ready;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::TEXT_IMPORT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: DocumentImportRequest,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            validate(&body)?;
            let fetched = effects::initial_value::<Imports>(ctx)?;
            if fetched.source.user != caller.user() || fetched.source.node != ctx.id("id")? {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the import actor or destination changed",
                ));
            }
            let mut source = fetched.source.clone();
            let destination = native::Facts::one(
                Source::Destination,
                DOCUMENT,
                targets::node(conn, source.node).await?,
            );
            let preliminary = native::bind_known(
                ctx,
                &sources::TEXT_IMPORT,
                std::slice::from_ref(&destination),
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &preliminary).await?;
            let directory = creation::parent(ctx, conn, &principal).await?;
            if directory != document_tree_domain::Directory::NodeDocs(source.node) {
                return Err(ApiError::not_found());
            }
            let mut entries = Vec::new();
            let mut copied = Vec::new();
            let mut selected = Vec::new();
            for raw in &fetched.entries {
                match raw {
                    Err(refused) => entries.push(Err(refused.clone())),
                    Ok(parsed) => {
                        let mut entry_source = fetched.source.for_hash(parsed.hash);
                        if parsed.replay.is_some() {
                            entry_source.selected.clear();
                        }
                        let prepared = async {
                            let current = source_value(conn, &entry_source).await?;
                            let known =
                                native::bind_known(ctx, &sources::TEXT_IMPORT, &current.facts)?;
                            let principal =
                                native::preparation_principal(ctx, conn, caller, &known).await?;
                            require_inputs(conn, &principal, &entry_source, &current)
                                .await
                                .map_err(api_error)?;
                            if let Some(replay) = &parsed.replay {
                                super::super::live(conn, replay.document).await?;
                                authorization::require_document(
                                    conn,
                                    &principal,
                                    replay.document,
                                    Permission::DocView,
                                    Policy::default(),
                                )
                                .await?;
                                return Ok((Vec::new(), Vec::new()));
                            }
                            let (labels, sources) = super::super::operation::prepare_written(
                                ctx,
                                conn,
                                caller,
                                known,
                                Some(&parsed.blocks),
                            )
                            .await?;
                            let blocks = labels.apply(parsed.blocks.clone())?;
                            Ok::<_, ApiError>((blocks, sources))
                        }
                        .await;
                        match prepared {
                            Ok((blocks, sources)) => {
                                copied.extend(sources.iter().cloned());
                                selected.extend(entry_source.selected.iter().copied());
                                entries.push(Ok(Entry {
                                    source: entry_source,
                                    parsed: Arc::clone(parsed),
                                    expanded: fetched.expanded,
                                    blocks,
                                    copied: sources.into_iter().map(|(source, _)| source).collect(),
                                }));
                            }
                            Err(error) => {
                                entries.push(Err(Box::new(refused(parsed.name.clone(), error))))
                            }
                        }
                    }
                }
            }
            selected.sort_by_key(|(hash, _)| hash.to_string());
            selected.dedup();
            source.selected = selected;
            let current = source_value(conn, &source).await?;
            copied.sort_by_key(|(source, _)| *source);
            copied.dedup_by_key(|(source, _)| *source);
            let mut facts = current.facts;
            facts.extend(text_creation::copied_facts(&copied));
            Ok((
                Proposal {
                    source,
                    entries,
                    facts: facts.clone(),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let current = source_value(conn, &value.source).await?;
            require_inputs(conn, principal, &value.source, &current)
                .await
                .map_err(api_error)
        })
    }
}
impl effects::ApplyOwned for Imports {
    fn apply_owned<'a>(
        _ctx: &'a Ctx,
        tx: orch_db::Tx,
        _principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Ready>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            deadline.check()?;
            Ok(effects::Outcome::Unchanged(Ready {
                entries: value.entries.clone(),
                facts: value.facts.clone(),
            }))
        })
    }
}
impl NativeSource for EntrySources {
    type Role = Source;
    type Input = Entry;
    type Value = SourceValue;
    const ROUTE: Route = NodeDocumentImport::ROUTE;
    const DECLARATION: &'static Declaration<Source> = &sources::TEXT_IMPORT;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Entry,
        _now: Time,
    ) -> BoxFuture<'a, Result<(SourceValue, Vec<native::Facts<Source>>), NativeError>> {
        Box::pin(async move {
            let mut value = source_value(conn, &input.source)
                .await
                .map_err(native_error)?;
            if let Some(replay) = &input.parsed.replay {
                let row = super::super::live(conn, replay.document)
                    .await
                    .map_err(native_error)?;
                if row.id != input.parsed.id {
                    return Err(NativeError::Refused);
                }
                value.replay = Some(row.id);
                value.targets.push(
                    read_operation::identity(conn, row.id)
                        .await
                        .map_err(native_error)?,
                );
            }
            let copied = written_targets(conn, &input.copied)
                .await
                .map_err(native_error)?;
            value
                .targets
                .extend(copied.iter().map(|(_, target)| target.clone()));
            value.facts.extend(text_creation::copied_facts(&copied));
            let facts = value.facts.clone();
            Ok((value, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Entry,
        value: &'a SourceValue,
    ) -> BoxFuture<'a, Result<(), NativeError>> {
        Box::pin(async move {
            require_inputs(conn, principal, &input.source, value).await?;
            if let Some(replay) = &input.parsed.replay {
                if value.replay != Some(replay.document) {
                    return Err(NativeError::Refused);
                }
                authorization::require_document(
                    conn,
                    principal,
                    replay.document,
                    Permission::DocView,
                    Policy::default(),
                )
                .await
                .map_err(native_error)?;
                return Ok(());
            }
            placed(conn, input.source.node)
                .await
                .map_err(native_error)?;
            let settings = DocSettings::at(conn, input.source.node)
                .await
                .map_err(native_error)?;
            if !settings.enabled {
                return Err(native_error(
                    limits::disabled(conn, principal, &settings, Policy::default()).await,
                ));
            }
            if input.expanded
                > allowance(conn, input.source.node)
                    .await
                    .map_err(native_error)?
            {
                return Err(native_error(invalid(
                    "files",
                    "the import exceeds the current expanded byte allowance",
                )));
            }
            let names = input
                .parsed
                .people
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            let current = rows::users_by_username(conn, &names)
                .await?
                .into_iter()
                .map(|(name, id)| (name.to_ascii_lowercase(), id))
                .collect::<HashMap<_, _>>();
            if input
                .parsed
                .people
                .iter()
                .any(|(name, id)| current.get(name).copied() != *id)
            {
                return Err(NativeError::Refused);
            }
            let labels = build::WrittenLabels::capture_allowed(
                conn,
                principal,
                &input.parsed.blocks,
                &input.copied,
            )
            .await
            .map_err(native_error)?;
            if labels
                .apply(input.parsed.blocks.clone())
                .map_err(native_error)?
                != input.blocks
            {
                return Err(NativeError::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Entry,
        _value: &'a SourceValue,
    ) -> BoxFuture<'a, Result<(), NativeError>> {
        Box::pin(async move {
            let hashes = input
                .parsed
                .files
                .iter()
                .map(|file| file.blob)
                .collect::<Vec<_>>();
            orch_db::queries::uploads::lock_blobs(conn, &hashes).await?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Entry,
        old: &'a SourceValue,
        fresh: &'a SourceValue,
    ) -> BoxFuture<'a, Result<bool, NativeError>> {
        Box::pin(async move { Ok(old.targets == fresh.targets && old.replay == fresh.replay) })
    }
}

async fn make(ctx: &Ctx, origin: &CapturedOperation, entry: Entry) -> ApiResult<(Id, Vec<String>)> {
    let mut tx = ctx.app.db.begin().await?;
    let (frame, deadline) =
        continuing::prepare_on::<EntrySources>(ctx, &mut tx, origin, entry).await?;
    let parts = frame.into_parts();
    if let Some(replay) = &parts.input.parsed.replay {
        let warnings:Option<Json>=sqlx::query_scalar("SELECT COALESCE(details->'import_warnings','[]'::jsonb) FROM audit_log WHERE action='document.created' AND object_id=$1 ORDER BY at,id LIMIT 1").bind(replay.document).fetch_optional(&mut *tx).await.map_err(DbError::from)?;
        let warnings = warnings
            .map(serde_json::from_value::<Vec<String>>)
            .transpose()
            .map_err(ApiError::internal)?
            .unwrap_or_default();
        deadline.check()?;
        let (frame, current_deadline) =
            continuing::refresh_on::<EntrySources>(ctx, &mut tx, &parts.opened).await?;
        frame
            .check_deadline(ctx.app.clock.as_ref())
            .map_err(api_error)?;
        current_deadline.check()?;
        deadline.check()?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok((replay.document, warnings));
    }
    let prepared = document_tree_domain::prepare_text_creation_in(
        &mut tx,
        document_tree_domain::Actor::Person(&parts.principal),
        document_tree_domain::Directory::NodeDocs(parts.input.source.node),
        parts.input.parsed.id,
        &parts.input.parsed.title,
        None,
        parts.principal.now(),
    )
    .await;
    let prepared = crate::routes::document_tree::result(&mut tx, prepared).await?;
    // Header preparation owns the native directory/namespace footprint. Refresh exact origin and
    // source after those waits, then consume that same owned native header once.
    let (frame, current_deadline) =
        continuing::refresh_on::<EntrySources>(ctx, &mut tx, &parts.opened).await?;
    let current = frame.into_parts();
    let mut action = ctx.clone();
    action.now = current.principal.now().get();
    let created = document_tree_domain::apply_text_creation_in(
        &mut tx,
        document_tree_domain::Actor::Person(&current.principal),
        prepared,
        current.principal.now(),
        || {
            deadline.check()?;
            current_deadline.check()?;
            current
                .first_effect
                .check(ctx.app.clock.as_ref())
                .map_err(api_error)
        },
    )
    .await?;
    let created = crate::routes::document_tree::result(&mut tx, created).await?;
    let warnings = if created.created {
        let settings = DocSettings::at(&mut tx, current.input.source.node).await?;
        install_created(
            &action,
            &mut tx,
            &current.principal,
            CreationMaterial {
                id: created.document.id,
                node: created.document.node_id,
                title: &current.input.parsed.title,
                access: None,
                blocks: current.input.blocks.clone(),
                copied_files: &current.input.parsed.files,
                receipt_warnings: &current.input.parsed.warnings,
                settings,
            },
        )
        .await?;
        current.input.parsed.warnings.clone()
    } else {
        let warnings:Option<orch_types::Json>=sqlx::query_scalar("SELECT COALESCE(details->'import_warnings','[]'::jsonb) FROM audit_log WHERE action='document.created' AND object_id=$1 ORDER BY at,id LIMIT 1").bind(created.document.id).fetch_optional(&mut *tx).await.map_err(DbError::from)?;
        warnings
            .map(serde_json::from_value::<Vec<String>>)
            .transpose()
            .map_err(ApiError::internal)?
            .unwrap_or_default()
    };
    let identity =
        transaction_guard::current_http_identity(ctx, &mut tx, HttpIdentityMode::Read).await?;
    identity.check_deadline(&ctx.app)?;
    deadline.check()?;
    current_deadline.check()?;
    tx.commit().await.map_err(DbError::from)?;
    Ok((created.document.id, warnings))
}

fn created_fact(targets: Vec<KeyResolvedTarget>) -> native::Facts<Source> {
    native::Facts::collection(
        Source::Created,
        if targets.is_empty() {
            Vec::new()
        } else {
            vec![(DOCUMENT, targets)]
        },
    )
}
impl reply::Projection for Report {
    type Source = Source;
    type Input = Completed;
    type Subject = Readback;
    type Value = ReportView;
    type Reply = DocumentImportResult;
    const READ_SOURCE: Source = Source::Created;
    fn declaration(_input: &Completed) -> &'static Declaration<Source> {
        &sources::TEXT_IMPORT
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Completed,
    ) -> BoxFuture<'a, ApiResult<(Readback, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mut documents = Vec::new();
            let mut targets = Vec::new();
            for id in input.outcomes.iter().filter_map(|outcome| outcome.document) {
                let row = super::super::live(conn, id).await?;
                let directory = super::super::stored_directory(conn, &row).await?;
                targets.push(read_operation::identity(conn, id).await?);
                documents.push(text_creation::Subject { row, directory });
            }
            let mut facts = input.facts.clone();
            facts.push(created_fact(targets));
            Ok((Readback { documents }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Completed,
        subject: &'a Readback,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for document in &subject.documents {
                authorization::require_document(
                    conn,
                    principal,
                    document.row.id,
                    Permission::DocView,
                    Policy::default(),
                )
                .await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Completed,
        subject: &'a Readback,
    ) -> BoxFuture<'a, ApiResult<(ReportView, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mut facts = input.facts.clone();
            let mut targets = Vec::new();
            for document in &subject.documents {
                targets.push(read_operation::identity(conn, document.row.id).await?);
            }
            facts.push(created_fact(targets));
            let mut documents = Vec::new();
            let mut all_names = NameBounds::default();
            for document in &subject.documents {
                let listed =
                    build::standing(conn, principal, document.row.id, Policy::default()).await?;
                let mut known = native::bind_known(ctx, &sources::TEXT_IMPORT, &facts)?;
                let (names, sources) = read_operation::completed_sources(
                    conn,
                    principal,
                    &mut known,
                    &document.row,
                    &listed,
                )
                .await?;
                all_names.nodes.extend(&names.nodes);
                all_names.containers.extend(&names.containers);
                let cut = crate::routes::document_tree::metadata::cut(
                    conn,
                    principal,
                    document.row.node_id,
                    principal.now(),
                )
                .await?;
                documents.push(text_creation::View {
                    listed,
                    names,
                    sources,
                    cut,
                });
            }
            Ok((
                ReportView { documents },
                text_creation::response_facts(conn, &all_names).await?,
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Completed,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<DocumentImportResult>> {
        Box::pin(async move {
            let mut documents = HashMap::new();
            for (subject, view) in prepared
                .subject
                .documents
                .iter()
                .zip(&prepared.value.documents)
            {
                let document =
                    text_creation::render_readback(conn, principal, subject, view).await?;
                documents.insert(subject.row.id, document.base);
            }
            let entries = input
                .outcomes
                .iter()
                .map(|outcome| DocumentImportResultEntry {
                    name: outcome.name.clone(),
                    document: outcome.document.and_then(|id| documents.remove(&id)),
                    warnings: outcome.warnings.clone(),
                    error: outcome.error.clone(),
                })
                .collect();
            Ok(DocumentImportResult { entries })
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Completed,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            if old.subject.documents.len() != fresh.subject.documents.len() {
                return Ok(false);
            }
            for (((before, view), after), current) in old
                .subject
                .documents
                .iter()
                .zip(&old.value.documents)
                .zip(&fresh.subject.documents)
                .zip(&fresh.value.documents)
            {
                if !text_creation::check_readback(conn, principal, before, view, after, current)
                    .await?
                {
                    return Ok(false);
                }
            }
            Ok(true)
        })
    }
}
pub(super) async fn import(
    ctx: Ctx,
    (): (),
    _body: DocumentImportRequest,
) -> ApiResult<DocumentImportResponse> {
    let (tx, _guard) = transaction_guard::begin(&ctx).await?;
    let ready = effects::apply_owned::<Imports>(&ctx, tx).await?;
    let origin = CapturedOperation::capture(&ready.accepted).map_err(ApiError::internal)?;
    let mut outcomes = Vec::new();
    for entry in ready.output.entries {
        let result = match entry {
            Err(refused) => Outcome {
                name: refused.name,
                document: None,
                warnings: refused.warnings,
                error: refused.error,
            },
            Ok(entry) => {
                let name = entry.parsed.name.clone();
                match make(&ready.ctx, &origin, entry).await {
                    Ok((id, warnings)) => Outcome {
                        name,
                        document: Some(id),
                        warnings,
                        error: None,
                    },
                    Err(error) => Outcome {
                        name,
                        document: None,
                        warnings: Vec::new(),
                        error: Some(error_info(error)),
                    },
                }
            }
        };
        outcomes.push(result);
        // Keep every committed identity before any later entry can wait or lose authority.
        let safe = DocumentImportResponse::Receipt(DocumentImportReceipt {
            projection: ReceiptProjection::Receipt,
            entries: outcomes
                .iter()
                .map(|outcome| DocumentImportReceiptEntry {
                    name: outcome.name.clone(),
                    document_id: outcome.document,
                    error: outcome.error.as_ref().map(|error| error.code.clone()),
                })
                .collect(),
        });
        publication::receipt(&ready.ctx, &safe)?;
    }
    let completed = Completed {
        outcomes,
        facts: ready.output.facts,
    };
    let safe = DocumentImportResponse::Receipt(DocumentImportReceipt {
        projection: ReceiptProjection::Receipt,
        entries: completed
            .outcomes
            .iter()
            .map(|outcome| DocumentImportReceiptEntry {
                name: outcome.name.clone(),
                document_id: outcome.document,
                error: outcome.error.as_ref().map(|error| error.code.clone()),
            })
            .collect(),
    });
    publication::receipt(&ready.ctx, &safe)?;
    Ok(
        match reply::render::<Report>(&ready.ctx, &ready.accepted, completed).await? {
            Some(full) => DocumentImportResponse::full(full),
            None => safe,
        },
    )
}
