//! Exports over HTTP: an archive of what a person may read in a place, planned before anything is made, asked for as a job, listed, fetched and removed by its person alone.
//!
//! What goes in an archive, and for whom, is the engine's ([`orch_engine::exports`]): each thing is written by its
//! person's Export row at the moment it is written. Here a request is read, an answer is made of the engine's, and
//! a refusal becomes the problem a client acts on: `export.too_large` and `export.too_many` with the figure and the
//! limit, `export.nothing`, `429` past what one person may have being written, and `410` for an archive that was
//! removed. The two forms of a single export that need the views of the API (a document with the names a reader is
//! given, a thread as the API shows it) are written by [`ApiForms`], which the engine asks, so that a thing in an
//! archive is the very bytes of its single export.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::documents::export::single_form;
use crate::routes::environments::jobs as job_view;
use crate::sign_in::origin;
use async_trait::async_trait;
use axum::body::Body;
use axum::response::Response;
use chrono::{DateTime, Utc};
use http::{StatusCode, header};
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::exports::{
    Export, ExportDelete, ExportDownload, ExportGet, ExportPlan, ExportRequest, MeExportList,
    NodeExportCreate, NodeExportPreflight,
};
use orch_api_contract::nodes::Node;
use orch_api_contract::threads::Thread;
use orch_api_contract::workspace::Job;
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_engine::exports::{Blocked, DocumentForm, DocumentHow, ExportError, Forms, FormsFault};
use orch_types::Id;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(NodeExportPreflight, preflight)
        .on(NodeExportCreate, create)
        .on(MeExportList, list)
        .on(ExportGet, get)
        .on_raw(ExportDownload, download)
        .on(ExportDelete, delete);
}

// ---------------------------------------------------------------- what only this layer writes

/// Writes the forms of a single export that need the views of the API, for the engine: the same code that answers
/// a person's request for one thing.
#[derive(Debug, Default, Clone, Copy)]
pub struct ApiForms;

fn fault(error: &ApiError) -> FormsFault {
    FormsFault(error.problem.detail.clone())
}

#[async_trait]
impl Forms for ApiForms {
    async fn thread(
        &self,
        conn: &mut Conn,
        asker: &Principal,
        thread: Id,
        now: DateTime<Utc>,
    ) -> Result<Thread, FormsFault> {
        match crate::routes::nodes::view::one(conn, asker, thread, now, Policy::default()).await {
            Ok(Some(Node::Thread(thread))) => Ok(thread),
            Ok(_) => Err(FormsFault("the thread is gone".to_owned())),
            Err(error) => Err(fault(&error)),
        }
    }

    async fn document(
        &self,
        conn: &mut Conn,
        asker: &Principal,
        document: Id,
        how: DocumentHow,
        now: DateTime<Utc>,
    ) -> Result<DocumentForm, FormsFault> {
        single_form(conn, asker, document, &how, now)
            .await
            .map(|(bytes, losses)| DocumentForm { bytes, losses })
            .map_err(|error| fault(&error))
    }
}

// ---------------------------------------------------------------- refusals

/// Which archives of a person are being written, in the words of a sentence: how many, as many as they may have at
/// once.
fn being_written(limit: u32) -> String {
    match limit {
        1 => "one archive of yours is".to_owned(),
        2 => "two archives of yours are".to_owned(),
        3 => "three archives of yours are".to_owned(),
        many => format!("{many} archives of yours are"),
    }
}

/// Why an export could not be planned, asked for, read or removed, as the problem a client acts on.
pub(crate) async fn failed(conn: &mut Conn, error: ExportError) -> ApiError {
    match error {
        ExportError::NotFound => ApiError::not_found(),
        ExportError::Invalid { field, message } => ApiError::invalid(&field, &message),
        ExportError::Blocked {
            by,
            limit,
            asked,
            message,
        } => ApiError::new(
            match by {
                Blocked::Nothing => ErrorCode::ExportNothing,
                Blocked::TooMany => ErrorCode::ExportTooMany,
                Blocked::TooLarge => ErrorCode::ExportTooLarge,
            },
            message,
        )
        .with("limit", limit)
        .with("asked", asked),
        ExportError::TooManyAtOnce { limit } => ApiError::new(
            ErrorCode::RateLimited,
            format!(
                "{} being written, which is as many as may be at once; ask again when one is ready",
                being_written(limit)
            ),
        )
        .with("limit", limit),
        ExportError::Gone => ApiError::new(ErrorCode::Gone, "the archive was removed"),
        ExportError::Suspended { org } => {
            refusals::problem(conn, orch_authz::Error::Suspended { org }).await
        }
        ExportError::Internal(error) => ApiError::engine(error),
    }
}

// ---------------------------------------------------------------- the requests

/// What an archive of a place would hold for the caller.
async fn preflight(ctx: Ctx, (): (), body: ExportRequest) -> ApiResult<ExportPlan> {
    let place = ctx.id("id")?;
    let caller = authorization::caller_of(ctx.user()?)?;
    match ctx.app.engine.plan_export(&caller, place, &body).await {
        Ok(plan) => Ok(plan),
        Err(error) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(failed(&mut conn, error).await)
        }
    }
}

/// Asks for an archive of a place: the job that writes it.
async fn create(ctx: Ctx, (): (), body: ExportRequest) -> ApiResult<Job> {
    let place = ctx.id("id")?;
    let caller = authorization::caller_of(ctx.user()?)?;
    let made = ctx
        .app
        .engine
        .make_export(&caller, &origin(&ctx), place, &body)
        .await;
    let mut conn = ctx.app.db.acquire().await?;
    match made {
        Ok(made) => job_view::view(&mut conn, &made.job).await,
        Err(error) => Err(failed(&mut conn, error).await),
    }
}

/// The caller's own archives, newest first.
async fn list(ctx: Ctx, query: PageQuery, (): ()) -> ApiResult<Page<Export>> {
    let user = ctx.user()?.id();
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let engine = &ctx.app.engine;
    let (rows, next_cursor) = match engine
        .exports_of(
            &mut conn,
            user,
            query.cursor.as_deref(),
            i64::from(query.limit()),
        )
        .await
    {
        Ok(found) => found,
        Err(error) => return Err(failed(&mut conn, error).await),
    };
    match engine.export_views(&mut conn, &principal, &rows).await {
        Ok(items) => Ok(Page { items, next_cursor }),
        Err(error) => Err(failed(&mut conn, error).await),
    }
}

/// One of the caller's archives.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Export> {
    let id = ctx.id("id")?;
    let user = ctx.user()?.id();
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let engine = &ctx.app.engine;
    let row = match engine.export_of(&mut conn, user, id).await {
        Ok(row) => row,
        Err(error) => return Err(failed(&mut conn, error).await),
    };
    match engine.export_views(&mut conn, &principal, &[row]).await {
        Ok(mut views) => views.pop().ok_or_else(ApiError::not_found),
        Err(error) => Err(failed(&mut conn, error).await),
    }
}

/// A name that can be put in a header: the characters a header takes, the rest an underscore, and the name as the
/// browser should save it in the form that carries any character.
fn attachment(name: &str) -> String {
    let plain: String = name
        .chars()
        .map(|c| {
            if (c.is_ascii_graphic() || c == ' ') && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = form_urlencoded::byte_serialize(name.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{encoded}")
}

/// Fetches an archive. It is not used up: it can be fetched again until it is removed.
async fn download(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    let id = ctx.id("id")?;
    let user = ctx.user()?.id();
    let engine = &ctx.app.engine;
    let opened = engine.open_export(&origin(&ctx), user, id).await;
    let mut conn = ctx.app.db.acquire().await?;
    let (row, read) = match opened {
        Ok(opened) => opened,
        Err(error) => return Err(failed(&mut conn, error).await),
    };
    let name = match engine.export_file_name(&mut conn, &row).await {
        Ok(name) => name,
        Err(error) => return Err(failed(&mut conn, error).await),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, attachment(&name))
        .header(header::CONTENT_LENGTH, read.meta.size)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from_stream(read.body))
        .map_err(ApiError::internal)
}

/// Removes an archive now; a job still writing it is cancelled.
async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let user = ctx.user()?.id();
    match ctx.app.engine.remove_export(user, id).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(failed(&mut conn, error).await)
        }
    }
}
