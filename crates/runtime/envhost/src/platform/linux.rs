//! Linux paths, process groups and PTYs.
//!
//! What Linux adds to the POSIX parts: the exact moment a process started, read from the kernel's
//! own table (to the hundredth of a second, so that a signal can name the process it means), a
//! process handle (`pidfd`) that makes checking a process and signalling it one step, the path of
//! an open file, and the limits and counters of the control group a unit runs in.

#![cfg(target_os = "linux")]

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

/// File names differ by case on the file systems Linux is installed on.
pub const CASE_SENSITIVE_BY_DEFAULT: bool = true;

/// Where an open file is now, as the kernel says: the way to check that a file that was opened
/// by a path is where the path said it would be.
pub fn path_of_file(file: &File) -> io::Result<PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

use std::sync::OnceLock;

use super::Sig;

/// The fields of `/proc/<pid>/stat` after the name of the program, which is in parentheses and
/// may itself hold spaces and parentheses: the fields begin after the last `)`.
fn fields_after_name(text: &str) -> Option<std::str::SplitWhitespace<'_>> {
    text.rfind(')').map(|at| text[at + 1..].split_whitespace())
}

/// When the machine booted, in seconds since 1970, as the kernel counts it.
fn boot_time_seconds() -> Option<i64> {
    static BOOT: OnceLock<Option<i64>> = OnceLock::new();
    *BOOT.get_or_init(|| {
        let text = std::fs::read_to_string("/proc/stat").ok()?;
        text.lines()
            .find_map(|line| line.strip_prefix("btime ")?.trim().parse().ok())
    })
}

/// When the process `pid` started, in milliseconds since 1970, to the clock tick of the kernel
/// (a hundredth of a second). The same call gives the same figure for as long as the process
/// lives, which is what a signal that names it by its start relies on.
#[must_use]
pub fn exact_start_ms(pid: u32) -> Option<i64> {
    let bytes = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // After the name: state, parent, group, session, terminal, ..., and the start time as the 20th.
    let ticks: u64 = fields_after_name(&text)?.nth(19)?.parse().ok()?;
    let per_second = rustix::param::clock_ticks_per_second().max(1);
    let since_boot_ms = i64::try_from(ticks.saturating_mul(1000) / per_second).ok()?;
    Some(
        boot_time_seconds()?
            .saturating_mul(1000)
            .saturating_add(since_boot_ms),
    )
}

/// The process group `pid` is in, from the kernel's table: the system call answers 0 for kernel
/// threads, and a library that does not expect that is not asked.
#[must_use]
pub fn process_group_of(pid: u32) -> Option<u32> {
    let bytes = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    fields_after_name(&text)?
        .nth(2)?
        .parse()
        .ok()
        .filter(|group| *group > 0)
}

/// The session `pid` is in, from the kernel's table.
#[must_use]
pub fn session_of(pid: u32) -> Option<u32> {
    let bytes = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    fields_after_name(&text)?
        .nth(3)?
        .parse()
        .ok()
        .filter(|session| *session > 0)
}

/// The state letter of the process `pid` as the kernel keeps it (`R`, `S`, `D`, `T`, `Z`, ...).
#[must_use]
pub fn state_of(pid: u32) -> Option<char> {
    let bytes = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    fields_after_name(&text)?.next()?.chars().next()
}

/// Checks that `pid` is the process that started at `expect_start_ms` and sends it `sig`, through a
/// handle on that very process: the number cannot be reused between the check and the signal. `None`
/// where the kernel offers no handles, and the caller signals by number.
#[must_use]
pub fn signal_via_handle(
    pid: u32,
    expect_start_ms: Option<i64>,
    sig: Sig,
) -> Option<std::io::Result<()>> {
    let number = rustix::process::Pid::from_raw(i32::try_from(pid).ok()?)?;
    let handle = match rustix::process::pidfd_open(number, rustix::process::PidfdFlags::empty()) {
        Ok(handle) => handle,
        Err(rustix::io::Errno::SRCH) => {
            return Some(Err(std::io::Error::from(std::io::ErrorKind::NotFound)));
        }
        // An old kernel, or a sandbox that does not allow it.
        Err(_) => return None,
    };
    if let Some(expected) = expect_start_ms
        && exact_start_ms(pid) != Some(expected)
    {
        return Some(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "that number now names another process",
        )));
    }
    Some(
        rustix::process::pidfd_send_signal(&handle, super::posix::rustix_signal(sig))
            .map_err(|errno| std::io::Error::from_raw_os_error(errno.raw_os_error())),
    )
}

/// The table of every process of the machine as the kernel keeps it: one small read of
/// `/proc/<pid>/stat` for each. A process that ends while the table is read is left out.
#[must_use]
pub fn table() -> Option<Vec<super::ProcRow>> {
    let boot = boot_time_seconds()?;
    let per_second = rustix::param::clock_ticks_per_second().max(1);
    let mut rows = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(bytes) = std::fs::read(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let Some(mut fields) = fields_after_name(&text) else {
            continue;
        };
        let state = fields
            .next()
            .and_then(|field| field.chars().next())
            .unwrap_or('?');
        let mut number = |at: usize| {
            fields
                .nth(at)
                .and_then(|field| field.parse::<u32>().ok())
                .unwrap_or(0)
        };
        // After the state: parent (+0), group (+1), session (+2) ... then the 16th field after that is the start time.
        let ppid = number(0);
        let group = number(0);
        let session = number(0);
        let ticks = fields
            .nth(15)
            .and_then(|field| field.parse::<u64>().ok())
            .unwrap_or(0);
        let since_boot_ms = i64::try_from(ticks.saturating_mul(1000) / per_second).unwrap_or(0);
        rows.push(super::ProcRow {
            pid,
            ppid,
            group,
            session,
            start_ms: boot.saturating_mul(1000).saturating_add(since_boot_ms),
            state,
        });
    }
    Some(rows)
}

/// The control group the host runs in (version 2 of the control groups, the unified hierarchy),
/// where it is mounted: in a container that is the container's own, the unit a driver made.
#[must_use]
pub fn own_cgroup_dir() -> Option<PathBuf> {
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = text.lines().find_map(|line| line.strip_prefix("0::"))?;
    let dir = PathBuf::from("/sys/fs/cgroup").join(path.trim().trim_start_matches('/'));
    dir.is_dir().then_some(dir)
}
