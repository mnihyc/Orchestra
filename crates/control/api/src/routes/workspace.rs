//! Files with rename, copy and the search inside them, what an archive holds, cache, snapshots with their archives, Git and attached mounts of an environment's workspace, reached by the permissions of the environment's own page.
//!
//! A workspace is reached through an environment, whichever thread, topic or page it is reached from:
//! by whoever holds `files.view` or `files.manage` on the environment's own page (and `terminal.view` or
//! `terminal.manage` for the processes), the Manage of a door being held by whoever manages the
//! environment; a thread that a request names gives nothing and says beside which thread the caller
//! works ([`door`]). Paths are relative to the workspace root and never leave it ([`paths`]). What a machine
//! refuses is passed on as the problem the request names ([`problems`]). A machine is asked when it is
//! there; when it is away, a listing and the names of files are read from the last list of its tree
//! that it uploaded, and everything else says that the source is offline ([`live`]). The requests
//! are in [`files`] (listing, describing, bringing a file to the store and reading it, writing),
//! [`looking`] (what agents changed, searching, archives, repositories) and [`snapshots`] (a
//! thread's directory kept as of a moment, and the jobs).

pub mod door;
pub mod files;
pub mod listing;
pub mod live;
pub mod looking;
pub(crate) mod pages;
pub mod paths;
pub mod problems;
pub mod restores;
pub mod snapshots;
pub mod source;

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Registry;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    files::mount(registry);
    looking::mount(registry);
    snapshots::mount(registry);
    source::mount(registry);
    restores::mount(registry);
}

/// Answers `status` with `value` as JSON: for a request whose body is read by the handler itself and
/// so is bound as a raw one.
pub fn json_reply<T: serde::Serialize>(status: StatusCode, value: &T) -> ApiResult<Response> {
    let bytes = serde_json::to_vec(value).map_err(ApiError::internal)?;
    let mut response = (status, bytes).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}
