//! Imports over HTTP: what a person brings into a place from an Orch package or from plain files, planned before anything is made, asked for as a job, listed, read with its report and taken back as a whole, all of it its person's alone.
//!
//! What an import does, and for whom, is the engine's ([`orch_engine::imports`]): each thing is made by its person's own rights
//! at the moment it is made. Here a request is read, the uploads it names are checked to be the person's own, an answer is made of
//! the engine's, and a refusal becomes the problem a client acts on: `import.too_large` and `import.too_many` with the figure and the
//! limit, `import.nothing`, `422 import.invalid` for a source that is not a package or files that can be read, `429` past what one
//! person may have running, and `409 import.undone` for an import that is taken back already.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::environments::jobs as job_view;
use axum::body::Body;
use axum::response::Response;
use http::{StatusCode, header};
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::imports::{
    Import, ImportFileList, ImportGet, ImportLine, ImportPlan, ImportPreflightRequest,
    ImportPreview, ImportPreviewRequest, ImportReportDownload, ImportReportDownloadQuery,
    ImportReportFormat, ImportReportList, ImportReportListQuery, ImportReportRow, ImportRequest,
    ImportUndo, ImportUndoBody, ImportUndoPlan, ImportUndoPreflight, MeImportList,
    NodeImportCreate, NodeImportPreflight, NodeImportPreview,
};
use orch_api_contract::workspace::Job;
use orch_db::{Conn, DbError};
use orch_engine::imports::ImportError;

mod creation;
mod read_sources;
mod report_stream;
mod source_planning;
mod source_preview;
mod undo_sources;
mod undoing;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(NodeImportCreate, creation::replay_binding())
        .creation(ImportUndo, undoing::replay_binding())
        .on_snapshot_body_read_with_optional_initial_operation(
            NodeImportPreflight,
            crate::key_operations::native::effects::initial::<source_planning::Planning>,
            crate::key_operations::native::selected_body::<
                source_planning::Planning,
                (),
                ImportPreflightRequest,
            >,
            crate::key_operations::native::selected_body_read::<
                source_planning::Planning,
                (),
                ImportPreflightRequest,
            >,
        )
        .on_snapshot_body_read_with_optional_initial_operation(
            NodeImportPreview,
            crate::key_operations::native::effects::initial::<source_preview::Preview>,
            crate::key_operations::native::selected_body::<
                source_preview::Preview,
                (),
                ImportPreviewRequest,
            >,
            crate::key_operations::native::selected_body_read::<
                source_preview::Preview,
                (),
                ImportPreviewRequest,
            >,
        )
        .on_with_initial_operation(
            NodeImportCreate,
            crate::key_operations::native::effects::initial_common::<creation::Create>,
            crate::key_operations::native::effects::prepare::<creation::Create>,
            creation::run,
        )
        .on_snapshot_read_with_optional_operation(
            MeImportList,
            crate::key_operations::native::selected::<read_sources::History>,
            crate::key_operations::native::selected_read::<read_sources::History>,
        )
        .on_snapshot_read_with_optional_operation(
            ImportGet,
            crate::key_operations::native::selected::<read_sources::Imported>,
            crate::key_operations::native::selected_read::<read_sources::Imported>,
        )
        .on_snapshot_read_with_optional_operation(
            ImportFileList,
            crate::key_operations::native::selected::<read_sources::Files>,
            crate::key_operations::native::selected_read::<read_sources::Files>,
        )
        .on_snapshot_read_with_optional_operation(
            ImportReportList,
            crate::key_operations::native::selected::<read_sources::Report>,
            crate::key_operations::native::selected_read::<read_sources::Report>,
        )
        .on_raw_read_with_optional_operation(
            ImportReportDownload,
            crate::key_operations::native::selected::<report_stream::Opening>,
            report_stream::download,
        )
        .on_snapshot_post_read_with_optional_operation(
            ImportUndoPreflight,
            crate::key_operations::native::selected::<undo_sources::Preview>,
            crate::key_operations::native::selected_read::<undo_sources::Preview>,
        )
        .on_with_operation(
            ImportUndo,
            crate::key_operations::native::effects::prepare::<undoing::Undo>,
            undoing::run,
        );
}

/// The longest import identity: as for any request that is repeated safely.
const CLIENT_ID_MAX_CHARS: usize = 200;

/// Why an import could not be planned, asked for, read or taken back, as the problem a client acts on.
pub(crate) async fn failed(conn: &mut Conn, error: ImportError) -> ApiError {
    match error {
        ImportError::NotFound => ApiError::not_found(),
        ImportError::Invalid { field, message } => ApiError::invalid(&field, &message),
        ImportError::Source(message) => ApiError::new(ErrorCode::ImportInvalid, message),
        ImportError::Blocked {
            code,
            limit,
            asked,
            message,
        } => ApiError::new(
            match code {
                "import.too_large" => ErrorCode::ImportTooLarge,
                "import.too_many" => ErrorCode::ImportTooMany,
                _ => ErrorCode::ImportNothing,
            },
            message,
        )
        .with("limit", limit)
        .with("asked", asked),
        ImportError::TooManyAtOnce { limit } => ApiError::new(
            ErrorCode::RateLimited,
            format!("{limit} imports of yours are running, which is as many as may be at once; ask again when one has ended"),
        )
        .with("limit", limit),
        ImportError::Undone(message) => ApiError::new(ErrorCode::ImportUndone, message),
        ImportError::Suspended { org } => {
            refusals::problem(conn, orch_authz::Error::Suspended { org }).await
        }
        ImportError::Internal(error) => ApiError::engine(error),
    }
}

/// A name that can be put in a header, and the name as the browser should save it.
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
