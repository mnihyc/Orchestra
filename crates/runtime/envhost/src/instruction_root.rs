//! Empty workspace instruction-root preparation without following links or replacing user content.

use std::ffi::OsStr;

use orch_agent_contract::session::INSTRUCTION_ROOT_MARKER;
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
use orch_types::Time;

use crate::environment::GateState;
use crate::error::{HostError, Result};
use crate::paths::Workspace;

/// Owner-readable and writable control metadata; the process umask may narrow these bits.
const MARKER_MODE: u32 = 0o700;

/// Makes or verifies the root control directory, leaving any conflicting content untouched.
pub fn prepare(workspace: &Workspace, gates: &GateState) -> Result<FileStat> {
    gates.require_remote_write()?;
    let root = workspace.root_dir()?;
    let name = OsStr::new(INSTRUCTION_ROOT_MARKER);
    match root.mkdir(name, MARKER_MODE) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(HostError::io("preparing the instruction root", error)),
    }
    let conflict = || {
        HostError::Exists("the instruction-root marker must be an empty directory, not a file or link; existing content was left untouched".to_owned())
    };
    let before = root.lstat(name).map_err(|_| conflict())?;
    if !before.is_dir() {
        return Err(conflict());
    }
    let marker = root.open_child(name).map_err(|_| conflict())?;
    let held = marker.stat_self().map_err(|_| conflict())?;
    if held.identity() != before.identity()
        || !marker.names().map_err(|_| conflict())?.is_empty()
        || root.lstat(name).map_err(|_| conflict())?.identity() != held.identity()
    {
        return Err(conflict());
    }
    Ok(FileStat {
        entry: DirEntry {
            name: INSTRUCTION_ROOT_MARKER.to_owned(),
            path: INSTRUCTION_ROOT_MARKER.to_owned(),
            kind: DirEntryKind::Dir,
            size: 0,
            mtime: Time::new(chrono::DateTime::from_timestamp_nanos(held.mtime_ns)),
            mode: held.mode,
            link_target: None,
            cached: false,
        },
        hash: None,
        media_type: crate::media::DIRECTORY.to_owned(),
        is_text: false,
    })
}
