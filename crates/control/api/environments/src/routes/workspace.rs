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

use crate::registry::Registry;

pub use orch_api_base::routes::workspace::json_reply;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    files::mount(registry);
    looking::mount(registry);
    snapshots::mount(registry);
    source::mount(registry);
    restores::mount(registry);
}
orch_api_base::__api_bind_3431b4ebde!();

#[path = "workspace/door.rs"]
pub mod door;

#[path = "workspace/files.rs"]
pub mod files;

#[path = "workspace/listing.rs"]
pub mod listing;

#[path = "workspace/live.rs"]
pub mod live;

#[path = "workspace/looking.rs"]
pub mod looking;

#[path = "workspace/pages.rs"]
pub mod pages;

#[path = "workspace/paths.rs"]
pub mod paths;

#[path = "workspace/problems.rs"]
pub mod problems;

#[path = "workspace/restores.rs"]
pub mod restores;

#[path = "workspace/snapshot_read.rs"]
pub mod snapshot_read;

#[path = "workspace/snapshots.rs"]
pub mod snapshots;

#[path = "workspace/source.rs"]
pub mod source;
