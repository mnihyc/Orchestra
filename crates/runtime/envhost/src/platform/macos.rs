//! macOS filesystem, processes and PTYs.
//!
//! What macOS adds to the POSIX parts, which are shared with Linux: the default of its file
//! system for the case of names, and the absence of process handles (a signal names a process by
//! number, after its start was checked against the process table). Process tables, resource
//! samples and pseudo-terminals come from the libraries that are portable.
//!
//! This module has been type-checked and linted for `aarch64-apple-darwin` and never run.

#![cfg(target_os = "macos")]

/// The file system macOS is installed on is case-insensitive unless the person who set it up
/// chose otherwise; a directory is probed, and this is the answer when it cannot be.
pub const CASE_SENSITIVE_BY_DEFAULT: bool = false;

use super::Sig;

/// There is no kernel table with a finer start time than the process table of the portable
/// library gives, which is in seconds.
#[must_use]
pub fn exact_start_ms(_pid: u32) -> Option<i64> {
    None
}

/// macOS has no process handles: the caller signals by number.
#[must_use]
pub fn signal_via_handle(
    _pid: u32,
    _expect_start_ms: Option<i64>,
    _sig: Sig,
) -> Option<std::io::Result<()>> {
    None
}

/// The process group `pid` is in.
#[must_use]
pub fn process_group_of(pid: u32) -> Option<u32> {
    super::posix::group_by_call(pid)
}

/// The session `pid` is in.
#[must_use]
pub fn session_of(pid: u32) -> Option<u32> {
    super::posix::session_by_call(pid)
}

/// macOS has no table to read here: the portable library gives the table.
#[must_use]
pub fn table() -> Option<Vec<super::ProcRow>> {
    None
}

/// macOS has no control groups.
#[must_use]
pub fn own_cgroup_dir() -> Option<std::path::PathBuf> {
    None
}
