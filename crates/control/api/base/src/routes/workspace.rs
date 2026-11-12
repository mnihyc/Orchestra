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
//! `looking` (what agents changed, searching, archives, repositories) and `snapshots` (a
//! thread's directory kept as of a moment, and the jobs).

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use axum::response::IntoResponse;

use axum::response::Response;

use http::HeaderValue;

use http::StatusCode;

use http::header;
orch_api_base::api_family_callback! {
    bind __api_bind_3431b4ebde, __api_install_3431b4ebde, __api_callback_3431b4ebde;
    slot __ApiCallback3431b4ebde, __API_CALLBACK_3431B4EBDE, __api_set_3431b4ebde;
    base orch_api_base::routes::workspace;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
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

#[path = "workspace/door.rs"]
pub mod door;

#[path = "workspace/files.rs"]
pub mod files;

#[path = "workspace/live.rs"]
pub mod live;

#[path = "workspace/pages.rs"]
pub mod pages;

#[path = "workspace/paths.rs"]
pub mod paths;

#[path = "workspace/problems.rs"]
pub mod problems;
