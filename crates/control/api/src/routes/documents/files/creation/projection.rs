//! Completed file and native refusal sources, without repeating body or writer work.

use super::*;
use crate::key_operations::native::{refusal, reply};
use orch_api_contract::{common::ErrorCode, documents::DocumentFile, nodes::NodeRef};
use orch_types::Time;

#[derive(Clone, PartialEq)]
pub(in crate::routes::documents) struct FileWitness {
    document: Id,
    held: HeldFile,
    generation: String,
}
impl FileWitness {
    pub(in crate::routes::documents) fn held(&self) -> &HeldFile {
        &self.held
    }
    pub(in crate::routes::documents) async fn load(
        conn: &mut Conn,
        document: Id,
        id: Id,
    ) -> ApiResult<Self> {
        live(conn, document).await?;
        let held = rows::file(conn, document, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let generation = sqlx::query_scalar(
            "SELECT xmin::text FROM document_files WHERE document_id=$1 AND id=$2",
        )
        .bind(document)
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(DbError::from)?
        .ok_or_else(ApiError::not_found)?;
        Ok(Self {
            document,
            held,
            generation,
        })
    }
    pub(in crate::routes::documents) async fn current(&self, conn: &mut Conn) -> ApiResult<Self> {
        let fresh = Self::load(conn, self.document, self.held.file.id).await?;
        if fresh != *self {
            return Err(ApiError::not_found());
        }
        Ok(fresh)
    }
}
pub(in crate::routes::documents::files) struct Written {
    source: FileSource,
    direct: bool,
    facts: Vec<native::Facts<Source>>,
}
enum FileSource {
    Accepted(FileWitness),
    Recorded { document: Id, file: Id },
}
impl Written {
    pub(super) async fn capture(
        conn: &mut Conn,
        document: Id,
        file: Id,
        direct: bool,
        facts: Vec<native::Facts<Source>>,
    ) -> ApiResult<Self> {
        Ok(Self {
            source: FileSource::Accepted(FileWitness::load(conn, document, file).await?),
            direct,
            facts,
        })
    }
    pub(super) fn recorded(document: Id, file: Id, direct: bool) -> Self {
        Self {
            source: FileSource::Recorded { document, file },
            direct,
            facts: Vec::new(),
        }
    }
    pub(super) fn document(&self) -> Id {
        match &self.source {
            FileSource::Accepted(witness) => witness.document,
            FileSource::Recorded { document, .. } => *document,
        }
    }
    pub(super) fn file(&self) -> Id {
        match &self.source {
            FileSource::Accepted(witness) => witness.held.file.id,
            FileSource::Recorded { file, .. } => *file,
        }
    }
}
pub(super) struct FileReply;
impl reply::Projection for FileReply {
    type Source = Source;
    type Input = Written;
    type Subject = FileWitness;
    type Value = DocumentFile;
    type Reply = DocumentFile;
    const READ_SOURCE: Source = Source::Document;
    fn declaration(input: &Written) -> &'static Declaration<Source> {
        match &input.source {
            FileSource::Accepted(_) => super::declaration(input.direct),
            FileSource::Recorded { .. } if input.direct => &sources::METADATA_DIRECT,
            FileSource::Recorded { .. } => &sources::METADATA_REFERENCE,
        }
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Written,
    ) -> BoxFuture<'a, ApiResult<(FileWitness, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let current = match &input.source {
                FileSource::Accepted(witness) => witness.current(conn).await?,
                FileSource::Recorded { document, file } => {
                    FileWitness::load(conn, *document, *file).await?
                }
            };
            let mut facts = vec![document_fact(conn, current.document).await?];
            // Finished source reads remain factual provenance, not a new prerequisite
            // for disclosing the file under its independently admitted current Document.
            facts.extend(
                input
                    .facts
                    .iter()
                    .filter(|fact| fact.source != Source::Document)
                    .cloned(),
            );
            Ok((current, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Written,
        _subject: &'a FileWitness,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_document(
            conn,
            principal,
            input.document(),
            Permission::DocView,
        ))
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Written,
        subject: &'a FileWitness,
    ) -> BoxFuture<'a, ApiResult<(DocumentFile, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let file = &subject.held.file;
            let mut wanted = build::Wanted::default();
            if let Some(user) = file.added_by {
                wanted.user(user);
            }
            let cast = build::Cast::load(conn, principal, wanted, Policy::default()).await?;
            let file = DocumentFile {
                id: file.id,
                name: file.name.clone(),
                media_type: file.media_type.clone(),
                size: file.size,
                is_text: file.is_text,
                added_by: cast.author(file.added_by, file.added_by_thread, None),
                added_at: Time::new(file.added_at),
            };
            Ok((
                file,
                vec![native::Facts::collection(Source::LimitNodes, Vec::new())],
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Written,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<DocumentFile>> {
        Box::pin(async move { Ok(prepared.value.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Written,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.subject == fresh.subject && old.value == fresh.value) })
    }
}

#[derive(Clone)]
pub(in crate::routes::documents::files) enum Refusal {
    Conflict(Conflict),
    Limit(LimitFailure),
}
#[derive(Clone)]
pub(in crate::routes::documents::files) struct Conflict {
    witness: FileWitness,
    direct: bool,
}
#[derive(Clone, Copy)]
pub(in crate::routes::documents::files) struct LimitFailure {
    document: Id,
    direct: bool,
    settings: DocSettings,
    limit: Limit,
}
fn conflict() -> ApiError {
    ApiError::new(
        ErrorCode::ConflictExists,
        "the document has a file of that name; ask to overwrite it to replace it",
    )
}
impl Refusal {
    pub(super) async fn conflict(
        conn: &mut Conn,
        document: Id,
        direct: bool,
        held: &HeldFile,
    ) -> ApiResult<Self> {
        Ok(Self::Conflict(Conflict {
            witness: FileWitness::load(conn, document, held.file.id).await?,
            direct,
        }))
    }
    pub(super) fn limit(document: Id, direct: bool, settings: DocSettings, limit: Limit) -> Self {
        Self::Limit(LimitFailure {
            document,
            direct,
            settings,
            limit,
        })
    }
    pub(super) async fn project(self, ctx: &Ctx, attempted: &Caller) -> ApiResult<ApiError> {
        match self {
            Self::Conflict(input) => {
                refusal::project::<ConflictReply>(ctx, attempted, input, conflict()).await
            }
            Self::Limit(input) => {
                let sparse = limits::too_large_receipt(&input.settings, input.limit)
                    .with("scope", Option::<NodeRef>::None);
                if input.settings.scope_of(input.limit).is_none() {
                    publication::refusal(ctx, &sparse)?;
                    Ok(sparse)
                } else {
                    refusal::project::<LimitReply>(ctx, attempted, input, sparse).await
                }
            }
        }
    }
}
struct ConflictReply;
impl reply::Projection for ConflictReply {
    type Source = Source;
    type Input = Conflict;
    type Subject = FileWitness;
    type Value = ();
    type Reply = ApiError;
    const READ_SOURCE: Source = Source::Document;
    fn declaration(input: &Conflict) -> &'static Declaration<Source> {
        if input.direct {
            &sources::CONFLICT_DIRECT
        } else {
            &sources::CONFLICT_REFERENCE
        }
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Conflict,
    ) -> BoxFuture<'a, ApiResult<(FileWitness, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let current = input.witness.current(conn).await?;
            let fact = document_fact(conn, current.document).await?;
            Ok((current, vec![fact]))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Conflict,
        _subject: &'a FileWitness,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_document(
            conn,
            principal,
            input.witness.document,
            Permission::DocView,
        ))
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Conflict,
        _subject: &'a FileWitness,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Conflict,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ApiError>> {
        Box::pin(async move { Ok(conflict().with("existing_id", prepared.subject.held.file.id)) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Conflict,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.subject == fresh.subject) })
    }
}

struct LimitReply;
impl reply::Projection for LimitReply {
    type Source = Source;
    type Input = LimitFailure;
    type Subject = Id;
    type Value = Option<NodeRef>;
    type Reply = ApiError;
    const READ_SOURCE: Source = Source::LimitNodes;
    fn declaration(input: &LimitFailure) -> &'static Declaration<Source> {
        if input.direct {
            &sources::LIMIT_DIRECT
        } else {
            &sources::LIMIT_REFERENCE
        }
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a LimitFailure,
    ) -> BoxFuture<'a, ApiResult<(Id, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = live(conn, input.document).await?;
            let current = DocSettings::at(conn, row.node_id).await?;
            if current.scope_of(input.limit) != input.settings.scope_of(input.limit)
                || current.value_of(input.limit) != input.settings.value_of(input.limit)
            {
                return Err(ApiError::not_found());
            }
            let id = current
                .scope_of(input.limit)
                .ok_or_else(ApiError::not_found)?;
            let target = crate::key_operations::targets::node(conn, id).await?;
            let fact = native::Facts::collection(
                Source::LimitNodes,
                vec![(target.object.kind, vec![target])],
            );
            Ok((id, vec![fact]))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a LimitFailure,
        _subject: &'a Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        // The original native name renderer below decides ordinary visibility.
        Box::pin(async { Ok(()) })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a LimitFailure,
        subject: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(Option<NodeRef>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let refs =
                crate::routes::nodes::view::refs(conn, principal, &[*subject], Policy::default())
                    .await?;
            Ok((refs.get(subject).cloned(), Vec::new()))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a LimitFailure,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ApiError>> {
        Box::pin(async move {
            Ok(limits::too_large_receipt(&input.settings, input.limit)
                .with("scope", &prepared.value))
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a LimitFailure,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.subject == fresh.subject && old.value == fresh.value) })
    }
}
