//! Supplied conversion and contextual directory/custody consume one native initial result.

use super::super::build::Cast;

use super::super::build::Wanted;

use super::super::limits::DocSettings;

use super::super::limits::Limit;

use super::super::read_operation;

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::HttpIdentityMode;

use crate::routes::documents::files::FileWitness;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::document_sources as sources;

use orch_api_contract::document_sources::ConversionSource as Source;

use orch_api_contract::documents::DocumentConvertBody;

use orch_api_contract::documents::DocumentConvertFormat;

use orch_api_contract::documents::DocumentConvertReceipt;

use orch_api_contract::documents::DocumentConvertReply;

use orch_api_contract::documents::DocumentConvertResponse;

use orch_api_contract::documents::DocumentFile;

use orch_api_contract::documents::ReaderWarning;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_documents::convert as reader;

use orch_documents::convert::Format;

use orch_types::Id;

use orch_types::Time;

use std::sync::Arc;
const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);

const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);

pub(super) struct Conversions;

pub(super) struct Initial {
    body: DocumentConvertBody,
    user: Id,
    people: HashMap<String, Id>,
    files: Vec<rows::FileRow>,
}

#[derive(Clone)]
pub(super) struct Witness {
    files: Vec<(String, Option<FileRef>)>,
    people: Vec<(String, Option<Id>)>,
}

pub(super) struct Fetched {
    user: Id,
    document: Option<Id>,
    text_bytes: i64,
    reply: DocumentConvertReply,
    held: Vec<HeldFile>,
    original_names: HashMap<Id, String>,
    witness: Witness,
}

pub(super) struct Proposal {
    fetched: Arc<Fetched>,
    document: Option<Id>,
    settings: Option<DocSettings>,
    held: Vec<HeldFile>,
    refused: Option<super::refusal::Failure>,
}

pub(super) enum Output {
    Supplied(Arc<Fetched>),
    Context(Arc<Written>),
    Refused(Box<super::refusal::Failure>),
}

pub(super) struct Written {
    document: Id,
    reply: DocumentConvertReply,
    witness: Witness,
    files: Vec<FileWitness>,
}

pub(super) struct ContextReply;

#[derive(PartialEq)]
pub(super) struct Current {
    document: Id,
    node: Id,
    files: Vec<FileWitness>,
}

fn declaration(document: Option<Id>) -> &'static Declaration<Source> {
    if document.is_some() {
        &sources::CONVERSION_DOCUMENT_ROW
    } else {
        &sources::CONVERSION_SUPPLIED
    }
}

async fn facts(
    conn: &mut Conn,
    user: Id,
    document: Option<Id>,
) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![match document {
        Some(id) => native::Facts::one(
            Source::Document,
            DOCUMENT,
            read_operation::identity(conn, id).await?,
        ),
        None => native::Facts::one(
            Source::Account,
            ACCOUNT,
            targets::account(conn, user).await?,
        ),
    }])
}

async fn require(
    conn: &mut Conn,
    principal: &Principal,
    user: Id,
    document: Option<Id>,
) -> ApiResult<()> {
    if principal.user() != user {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the conversion actor changed",
        ));
    }
    if let Some(id) = document {
        authorization::require_document(
            conn,
            principal,
            id,
            Permission::DocEdit,
            Policy::default(),
        )
        .await?;
    }
    Ok(())
}

async fn limit(conn: &mut Conn, document: Option<Id>) -> ApiResult<i64> {
    match document {
        Some(id) => {
            let row = super::super::live(conn, id).await?;
            Ok(DocSettings::at(conn, row.node_id).await?.max_bytes.value)
        }
        None => Ok(orch_settings::get::<i64>(
            conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::DOCS_MAX_BYTES,
        )
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(i64::MAX)),
    }
}

fn text_limit(bytes: i64, limit: i64) -> ApiResult<()> {
    if bytes > limit {
        Err(ApiError::new(
            ErrorCode::RequestTooLarge,
            "the text is larger than a document may be",
        )
        .with("limit", limit))
    } else {
        Ok(())
    }
}

fn changed() -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "the selected conversion directory changed; convert the supplied text again",
    )
}

impl Witness {
    pub(super) async fn check(
        &self,
        conn: &mut Conn,
        document: Id,
        own: &BTreeSet<Id>,
    ) -> ApiResult<()> {
        let files = rows::files(conn, document).await?;
        for (name, expected) in &self.files {
            let current = files
                .iter()
                .find(|file| file.name == *name && !own.contains(&file.id))
                .map(|file| FileRef {
                    id: file.id,
                    media_type: file.media_type.clone(),
                });
            if current != *expected {
                return Err(changed());
            }
        }
        let names = self
            .people
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let current = rows::users_by_username(conn, &names)
            .await?
            .into_iter()
            .map(|(name, id)| (name.to_ascii_lowercase(), id))
            .collect::<HashMap<_, _>>();
        for (name, expected) in &self.people {
            if current.get(name).copied() != *expected {
                return Err(changed());
            }
        }
        Ok(())
    }
}

impl effects::Initial for Conversions {
    type Input = Initial;
    type Fetched = Fetched;
    fn declaration(input: &Initial) -> &'static Declaration<Source> {
        declaration(input.body.document_id)
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: DocumentConvertBody,
    ) -> BoxFuture<'a, ApiResult<(Initial, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let facts = facts(conn, caller.user(), body.document_id).await?;
            let plan = native::bind_required(ctx, declaration(body.document_id), &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            require(conn, &principal, caller.user(), body.document_id).await?;
            text_limit(
                i64::try_from(body.text.len()).unwrap_or(i64::MAX),
                limit(conn, body.document_id).await?,
            )?;
            let files = match body.document_id {
                Some(id) => rows::files(conn, id).await?,
                None => Vec::new(),
            };
            let names =
                if body.document_id.is_some() && body.format == DocumentConvertFormat::Markdown {
                    markdown::mentioned_names(&body.text)
                } else {
                    Vec::new()
                };
            let people = if names.is_empty() {
                HashMap::new()
            } else {
                rows::users_by_username(conn, &names)
                    .await?
                    .into_iter()
                    .map(|(name, id)| (name.to_ascii_lowercase(), id))
                    .collect()
            };
            Ok((
                Initial {
                    body,
                    user: principal.user(),
                    people,
                    files,
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
        Box::pin(require(conn, principal, input.user, input.body.document_id))
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        _caller: &'a Caller,
        input: Initial,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let user = input.user;
            let document = input.body.document_id;
            let text_bytes = i64::try_from(input.body.text.len()).unwrap_or(i64::MAX);
            let mut taken = input
                .files
                .iter()
                .map(|file| file.name.clone())
                .collect::<HashSet<_>>();
            let (mut read, witness) = tokio::task::spawn_blocking(move || {
                let format = match input.body.format {
                    DocumentConvertFormat::Markdown => Format::Markdown,
                    DocumentConvertFormat::Html => Format::Html,
                    DocumentConvertFormat::Text => Format::Text,
                };
                let prepared = reader::prepare(format, input.body.text.as_bytes())
                    .map_err(|error| invalid("text", &error.to_string()))?;
                let directory = Held {
                    people: input.people,
                    files: input
                        .files
                        .iter()
                        .map(|file| {
                            (
                                file.name.clone(),
                                FileRef {
                                    id: file.id,
                                    media_type: file.media_type.clone(),
                                },
                            )
                        })
                        .collect(),
                    used_files: RefCell::new(BTreeSet::new()),
                    used_people: RefCell::new(BTreeSet::new()),
                };
                let read = prepared.finish(&directory);
                let files = directory
                    .used_files
                    .borrow()
                    .iter()
                    .map(|name| {
                        (
                            name.clone(),
                            directory
                                .files
                                .iter()
                                .find(|(held, _)| held == name)
                                .map(|(_, file)| file.clone()),
                        )
                    })
                    .collect();
                let people = directory
                    .used_people
                    .borrow()
                    .iter()
                    .map(|name| (name.clone(), directory.people.get(name).copied()))
                    .collect();
                Ok::<_, ApiError>((read, Witness { files, people }))
            })
            .await
            .map_err(ApiError::internal)??;
            let original_names = read
                .embedded
                .iter()
                .map(|file| (file.id, file.name.clone()))
                .collect();
            let held = if document.is_some() {
                hold(ctx, &read.embedded, &mut taken, user, ctx.now).await?
            } else {
                if !read.embedded.is_empty() {
                    let gone = read.embedded.iter().map(|file| file.id).collect();
                    without_files(&mut read.blocks, &gone);
                    orch_documents::warnings::note(
                        &mut read.warnings,
                        orch_documents::warnings::Warning::new("reader.pictures_left_out", &[]),
                    );
                }
                Vec::new()
            };
            let reply = DocumentConvertReply {
                blocks: read.blocks,
                warnings: read
                    .warnings
                    .into_iter()
                    .map(|warning| ReaderWarning {
                        code: warning.code,
                        params: warning.params,
                        message: warning.message,
                    })
                    .collect(),
                files: Vec::new(),
            };
            Ok(Fetched {
                user,
                document,
                text_bytes,
                reply,
                held,
                original_names,
                witness,
            })
        })
    }
}

impl effects::Effect for Conversions {
    type Source = Source;
    type Query = ();
    type Body = DocumentConvertBody;
    type Value = Proposal;
    type Output = Output;
    fn declaration(value: &Proposal) -> &'static Declaration<Source> {
        declaration(value.document)
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: DocumentConvertBody,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Conversions>(ctx)?;
            if fetched.document != body.document_id || fetched.user != caller.user() {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the conversion context changed",
                ));
            }
            let actual = facts(conn, caller.user(), body.document_id).await?;
            let plan = native::bind_required(ctx, declaration(body.document_id), &actual)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            require(conn, &principal, fetched.user, fetched.document).await?;
            let mut held = fetched.held.clone();
            let mut refused = None;
            let settings = if let Some(id) = body.document_id {
                let row = super::super::live_locked(conn, id).await?;
                let settings = DocSettings::at(conn, row.node_id).await?;
                text_limit(fetched.text_bytes, settings.max_bytes.value)?;
                fetched.witness.check(conn, id, &BTreeSet::new()).await?;
                let mut names = rows::files(conn, id)
                    .await?
                    .into_iter()
                    .map(|file| file.name)
                    .collect::<HashSet<_>>();
                for file in &mut held {
                    let base = fetched.original_names.get(&file.file.id).ok_or_else(|| {
                        ApiError::internal("a native embedded file lost its source name")
                    })?;
                    file.file.name = orch_documents::files::unique_name(&mut names, base);
                }
                let total = held.iter().fold(row.files_bytes, |total, file| {
                    total.saturating_add(file.file.size)
                });
                if held
                    .iter()
                    .any(|file| file.file.size > settings.file_max_bytes.value)
                {
                    refused = Some(super::refusal::Failure::new(id, settings, Limit::FileBytes));
                } else if total > settings.files_max_bytes.value {
                    refused = Some(super::refusal::Failure::new(
                        id,
                        settings,
                        Limit::FilesBytes,
                    ));
                }
                if refused.is_none() {
                    let hashes = held.iter().map(|file| file.blob).collect::<Vec<_>>();
                    orch_db::queries::uploads::lock_blobs(conn, &hashes).await?;
                }
                Some(settings)
            } else {
                text_limit(fetched.text_bytes, limit(conn, None).await?)?;
                None
            };
            Ok((
                Proposal {
                    fetched,
                    document: body.document_id,
                    settings,
                    held,
                    refused,
                },
                actual,
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
            require(conn, principal, value.fetched.user, value.document).await?;
            if let Some(id) = value.document {
                value
                    .fetched
                    .witness
                    .check(conn, id, &BTreeSet::new())
                    .await?;
            }
            Ok(())
        })
    }
}

impl effects::ApplyOwned for Conversions {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Output>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            if let Some(failure) = value.refused {
                deadline.check()?;
                tx.rollback().await.map_err(DbError::from)?;
                return Ok(effects::Outcome::Unchanged(Output::Refused(Box::new(
                    failure,
                ))));
            }
            let Some(document) = value.document else {
                deadline.check()?;
                tx.commit().await.map_err(DbError::from)?;
                return Ok(effects::Outcome::Unchanged(Output::Supplied(Arc::clone(
                    &value.fetched,
                ))));
            };
            let settings = value
                .settings
                .ok_or_else(|| ApiError::internal("the contextual conversion lost its settings"))?;
            if !value.held.is_empty() {
                register_checked(
                    ctx,
                    &mut tx,
                    &value.held,
                    principal.now().get(),
                    move || before.check(),
                )
                .await?;
                rows::copy::install_files(
                    &mut tx,
                    document,
                    &value.held,
                    Some(principal.user()),
                    None,
                    principal.now().get(),
                )
                .await
                .map_err(ApiError::internal)?;
                for file in &value.held {
                    rows::history::record_file(
                        &mut tx,
                        document,
                        &Writer::User(principal.user()),
                        DocumentFileChange {
                            name: file.file.name.clone(),
                            kind: DocumentFileChangeKind::Added,
                        },
                        settings.version_rules(ctx.app.config.documents.version_window_secs),
                        principal.now().get(),
                    )
                    .await?;
                }
                rows::touch(&mut tx, document).await?;
            }
            let mut files = Vec::new();
            let mut witnesses = Vec::new();
            let mut wanted = Wanted::default();
            wanted.user(principal.user());
            let cast = Cast::load(&mut tx, principal, wanted, Policy::default()).await?;
            for file in &value.held {
                let witness = FileWitness::load(&mut tx, document, file.file.id).await?;
                let row = &witness.held().file;
                files.push(DocumentFile {
                    id: row.id,
                    name: row.name.clone(),
                    media_type: row.media_type.clone(),
                    size: row.size,
                    is_text: row.is_text,
                    added_by: cast.author(row.added_by, row.added_by_thread, None),
                    added_at: Time::new(row.added_at),
                });
                witnesses.push(witness);
            }
            let mut generated = value.fetched.reply.clone();
            generated.files = files;
            deadline.check()?;
            let identity =
                transaction_guard::current_http_identity(ctx, &mut tx, HttpIdentityMode::Read)
                    .await?;
            identity.check_deadline(&ctx.app)?;
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            let output = Output::Context(Arc::new(Written {
                document,
                reply: generated,
                witness: value.fetched.witness.clone(),
                files: witnesses,
            }));
            Ok(if value.held.is_empty() {
                effects::Outcome::Unchanged(output)
            } else {
                effects::Outcome::Applied(output)
            })
        })
    }
}

impl reply::Projection for ContextReply {
    type Source = Source;
    type Input = Arc<Written>;
    type Subject = Current;
    type Value = Vec<DocumentFile>;
    type Reply = DocumentConvertReply;
    const READ_SOURCE: Source = Source::Document;
    fn declaration(_input: &Self::Input) -> &'static Declaration<Source> {
        &sources::CONVERSION_DOCUMENT_ROW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<(Current, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let document = super::super::live(conn, input.document).await?;
            let own = input.files.iter().map(|file| file.held().file.id).collect();
            input.witness.check(conn, input.document, &own).await?;
            let mut files = Vec::new();
            for file in &input.files {
                files.push(file.current(conn).await?);
            }
            Ok((
                Current {
                    document: input.document,
                    node: document.node_id,
                    files,
                },
                vec![native::Facts::one(
                    Source::Document,
                    DOCUMENT,
                    read_operation::identity(conn, input.document).await?,
                )],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        _subject: &'a Current,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(authorization::require_document(
            conn,
            principal,
            input.document,
            Permission::DocView,
            Policy::default(),
        ))
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Self::Input,
        subject: &'a Current,
    ) -> BoxFuture<'a, ApiResult<(Vec<DocumentFile>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mut wanted = Wanted::default();
            for file in &subject.files {
                if let Some(user) = file.held().file.added_by {
                    wanted.user(user);
                }
            }
            let cast = Cast::load(conn, principal, wanted, Policy::default()).await?;
            let files = subject
                .files
                .iter()
                .map(|file| {
                    let row = &file.held().file;
                    DocumentFile {
                        id: row.id,
                        name: row.name.clone(),
                        media_type: row.media_type.clone(),
                        size: row.size,
                        is_text: row.is_text,
                        added_by: cast.author(row.added_by, row.added_by_thread, None),
                        added_at: Time::new(row.added_at),
                    }
                })
                .collect();
            Ok((
                files,
                vec![native::Facts::collection(Source::LimitNodes, Vec::new())],
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Self::Input,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<DocumentConvertReply>> {
        Box::pin(async move {
            let mut full = input.reply.clone();
            full.files = prepared.value.clone();
            Ok(full)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Self::Input,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.subject == fresh.subject && old.value == fresh.value) })
    }
}

fn receipt(input: &Written) -> DocumentConvertResponse {
    DocumentConvertResponse::Receipt(DocumentConvertReceipt {
        projection: ReceiptProjection::Receipt,
        document_id: input.document,
        file_ids: input.files.iter().map(|file| file.held().file.id).collect(),
    })
}

pub(super) async fn convert_text(
    ctx: Ctx,
    (): (),
    _body: DocumentConvertBody,
) -> ApiResult<DocumentConvertResponse> {
    let (tx, _guard) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Conversions>(&ctx, tx).await?;
    match completed.output {
        Output::Refused(failure) => Err((*failure)
            .project(&completed.ctx, &completed.accepted)
            .await?),
        Output::Context(input) => {
            let safe = receipt(&input);
            publication::receipt(&completed.ctx, &safe)?;
            Ok(
                match reply::render::<ContextReply>(&completed.ctx, &completed.accepted, input)
                    .await?
                {
                    Some(full) => DocumentConvertResponse::full(full),
                    None => safe,
                },
            )
        }
        Output::Supplied(input) => {
            let actor = input.user;
            publication::retain(
                &completed.ctx,
                &completed.accepted,
                actor,
                |ctx, conn, current, actor| {
                    Box::pin(async move {
                        if current.user() != *actor {
                            return Err(ApiError::new(
                                ErrorCode::AuthzDenied,
                                "the supplied conversion actor changed",
                            ));
                        }
                        let _admitted = native::bind_required(
                            ctx,
                            &sources::CONVERSION_SUPPLIED,
                            &facts(conn, *actor, None).await?,
                        )?
                        .admit(current.caller())?;
                        Ok(())
                    })
                },
            )?;
            Ok(DocumentConvertResponse::full(input.reply.clone()))
        }
    }
}
