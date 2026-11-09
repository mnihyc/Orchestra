//! Native import aggregate progress contains identities, never a cached report.
use super::*;

pub(super) fn resume(
    ctx: &Ctx,
    created: &orch_db::idempotency::creation::Created,
) -> ApiResult<bool> {
    let progress: Progress =
        serde_json::from_value(created.data.clone()).map_err(ApiError::internal)?;
    if created.kind != CREATION_KIND || created.id != ctx.id("id")? {
        return Err(ApiError::not_found());
    }
    publication::receipt(ctx, &receipt(&progress.documents, progress.complete))?;
    Ok(!progress.complete)
}

pub(super) fn receipt(documents: &[Id], complete: bool) -> DocumentImportResponse {
    DocumentImportResponse::Receipt(DocumentImportReceipt {
        projection: ReceiptProjection::Receipt,
        entries: Vec::new(),
        accepted_document_ids: documents.to_vec(),
        complete: Some(complete),
    })
}

pub(super) async fn replay(
    ctx: Ctx,
    body: DocumentImportRequest,
    created: orch_db::idempotency::creation::Created,
) -> ApiResult<DocumentImportResponse> {
    let progress: Progress = serde_json::from_value(created.data).map_err(ApiError::internal)?;
    if created.kind != CREATION_KIND
        || created.id != ctx.id("id")?
        || progress.client_id != body.client_id
    {
        return Err(ApiError::not_found());
    }
    let safe = receipt(&progress.documents, progress.complete);
    publication::receipt(&ctx, &safe)?;
    if !progress.complete {
        return Ok(safe);
    }
    let input = Completed {
        facts: Vec::new(),
        outcomes: progress
            .documents
            .into_iter()
            .map(|id| Outcome {
                name: String::new(),
                document: Some(id),
                warnings: Vec::new(),
                error: None,
                recorded: true,
            })
            .collect(),
    };
    let user = ctx.authenticated_user()?.id();
    Ok(
        match reply::render_completed_replay::<Report>(&ctx, user, input).await? {
            Some(full) => DocumentImportResponse::full(full),
            None => safe,
        },
    )
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    client_id: String,
    documents: Vec<Id>,
    source_bytes: SourceBytes,
    complete: bool,
}

pub(super) async fn accepted_before(
    ctx: &Ctx,
    conn: &mut Conn,
    node: Id,
    client_id: &str,
    files: usize,
) -> ApiResult<(Vec<Id>, SourceBytes)> {
    use orch_db::idempotency::creation::{Record, captures};
    let user = ctx.authenticated_user()?.id();
    if !captures(Some(user), CREATION_KIND) {
        return Ok((Vec::new(), SourceBytes::new()));
    }
    let key = crate::middleware::idempotency::request_key(ctx)?
        .ok_or_else(|| ApiError::internal("a tracked import has no creation key"))?;
    let value: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(user)
            .bind(key)
            .fetch_one(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let record = Record::read(value)?;
    let Some(created) = record.outcome else {
        return Ok((Vec::new(), SourceBytes::new()));
    };
    let progress: Progress = serde_json::from_value(created.data).map_err(ApiError::internal)?;
    if created.kind != CREATION_KIND || created.id != node || progress.client_id != client_id {
        return Err(ApiError::internal(
            "the import progress has a different native identity",
        ));
    }
    if progress
        .source_bytes
        .iter()
        .any(|(index, size)| *index >= files || *size < 0)
    {
        return Err(ApiError::internal(
            "the import progress has invalid source byte counters",
        ));
    }
    Ok((progress.documents, progress.source_bytes))
}

pub(super) async fn observe(
    conn: &mut Conn,
    source: &Sources,
    accepted: &[Id],
    document: Option<(Id, usize)>,
    complete: bool,
) -> ApiResult<()> {
    if !orch_db::idempotency::creation::captures(Some(source.user), CREATION_KIND) {
        return Ok(());
    }
    let mut documents = accepted.to_vec();
    if let Some((document, position)) = document {
        if let Some(old) = documents.iter().position(|id| *id == document) {
            documents.remove(old);
        }
        documents.insert(position.min(documents.len()), document);
    }
    orch_db::idempotency::creation::observe_with(
        conn,
        Some(source.user),
        CREATION_KIND,
        source.node,
        serde_json::to_value(Progress {
            client_id: source.client_id.clone(),
            documents,
            source_bytes: source.source_bytes.clone(),
            complete,
        })
        .map_err(ApiError::internal)?,
    )
    .await?;
    Ok(())
}
