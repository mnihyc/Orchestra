//! Exports over HTTP: an archive of what a person may read in a place, planned before anything is made, asked for as a job, listed, fetched and removed by its person alone.
//!
//! What goes in an archive, and for whom, is the engine's ([`orch_engine::exports`]): each thing is written by its
//! person's Export row at the moment it is written. Here a request is read, an answer is made of the engine's, and
//! a refusal becomes the problem a client acts on: `export.too_large` and `export.too_many` with the figure and the
//! limit, `export.nothing`, `429` past what one person may have being written, and `410` for an archive that was
//! removed. The two forms of a single export that need the views of the API (a document with the names a reader is
//! given, a thread as the API shows it) are written by [`ApiForms`], which the engine asks, so that a thing in an
//! archive is the very bytes of its single export.

use crate::middleware::authorization::refusals;
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
    Export, ExportDelete, ExportDownload, ExportGet, ExportRequest, MeExportList, NodeExportCreate,
    NodeExportPreflight,
};
use orch_api_contract::nodes::Node;
use orch_api_contract::threads::Thread;
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_engine::exports::{Blocked, DocumentForm, DocumentHow, ExportError, Forms, FormsFault};
use orch_types::Id;

mod byte_sources;
mod creation;
mod planning_sources;
mod read_sources;
mod removal_sources;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(NodeExportCreate, creation::replay_binding())
        .on_snapshot_body_read_with_optional_operation(
            NodeExportPreflight,
            crate::key_operations::native::selected_body::<planning_sources::Plan, (), ExportRequest>,
            crate::key_operations::native::selected_body_read::<planning_sources::Plan, (), ExportRequest>,
        )
        .on_with_operation(NodeExportCreate,
            crate::key_operations::native::effects::prepare::<creation::Create>, creation::run)
        .on_snapshot_read_with_optional_operation(
            MeExportList,
            crate::key_operations::native::selected::<read_sources::Archives>,
            crate::key_operations::native::selected_read::<read_sources::Archives>,
        )
        .on_snapshot_read_with_optional_operation(
            ExportGet,
            crate::key_operations::native::selected::<read_sources::Archive>,
            crate::key_operations::native::selected_read::<read_sources::Archive>,
        )
        .on_raw_read_with_operation(
            ExportDownload,
            crate::key_operations::native::effects::prepare::<byte_sources::Open>,
            byte_sources::download,
        )
        .on_with_operation(
            ExportDelete,
            crate::key_operations::native::effects::prepare::<removal_sources::Delete>,
            delete,
        );
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
    async fn sources(
        &self,
        conn: &mut Conn,
        asker: &Principal,
        form: orch_engine::exports::Form,
    ) -> Result<Option<orch_engine::exports::FormSources>, FormsFault> {
        use orch_access_contract::permissions::ObjectKind;
        use orch_engine::exports::{Form, FormReference, FormSources};
        let references = match form {
            Form::Thread { id } => {
                let prepared = crate::routes::nodes::content_sources::prepare(conn, asker, id)
                    .await
                    .map_err(|error| fault(&error))?;
                let side = orch_engine::views::thread_metadata_sources(conn, id)
                    .await
                    .map_err(|error| FormsFault(error.to_string()))?
                    .and_then(|source| source.side_of);
                prepared
                    .objects
                    .into_iter()
                    .map(|(object, id)| {
                        if side == Some(id)
                            || matches!(
                                object,
                                ObjectKind::Agent
                                    | ObjectKind::Provider
                                    | ObjectKind::Machine
                                    | ObjectKind::Host
                                    | ObjectKind::Environment
                            )
                        {
                            FormReference::Published { object, id }
                        } else {
                            FormReference::Object { object, id }
                        }
                    })
                    .collect()
            }
            Form::Document { id, json, comments } => {
                crate::routes::documents::export::form_sources(conn, asker, id, json, comments)
                    .await
                    .map_err(|error| fault(&error))?
            }
        };
        Ok(Some(FormSources { references }))
    }
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

/// Removes an archive now; a job still writing it is cancelled.
async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (tx, _guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    crate::key_operations::native::effects::apply_owned::<removal_sources::Delete>(&ctx, tx)
        .await?;
    Ok(())
}
