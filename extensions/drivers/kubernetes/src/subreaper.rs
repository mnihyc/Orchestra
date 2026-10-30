//! Child accounting for a dedicated workload supervisor process.
//!
//! Call these only in the supervisor, before any workload starts. The guard
//! must authenticate the supervisor, prevent new starts while stopping and
//! independently verify the workload's cgroup and visibility.

#![cfg(target_os = "linux")]

use std::time::Instant;

use orch_driver_contract::driver::DriverError;
use rustix::io::Errno;
use rustix::process::{
    DumpableBehavior, Pid, WaitOptions, child_subreaper, dumpable_behavior, set_child_subreaper,
    set_dumpable_behavior, wait,
};

/// What happened to the connector, without interpreting a signal as a normal exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// The kernel's current answer after reaping every already-exited child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Children {
    /// True only on ECHILD, not merely when no child has exited yet.
    pub none: bool,
    /// A newly observed connector exit, if this pass reaped it.
    pub connector_exit: Option<Exit>,
}

fn unavailable() -> DriverError {
    DriverError::Unavailable("the workload supervisor cannot account for its children".into())
}

/// Enables and verifies descendant reparenting and protects the supervisor from same-UID tracing.
pub(crate) fn protect() -> Result<(), DriverError> {
    set_dumpable_behavior(DumpableBehavior::NotDumpable).map_err(|_| unavailable())?;
    // Linux expects the integer 1; rustix represents that nonzero argument as Pid::INIT.
    set_child_subreaper(Some(Pid::INIT)).map_err(|_| unavailable())?;
    if dumpable_behavior().map_err(|_| unavailable())? != DumpableBehavior::NotDumpable
        || child_subreaper().map_err(|_| unavailable())? != Some(Pid::INIT)
    {
        return Err(unavailable());
    }
    Ok(())
}

/// Reaps any process group. Only ECHILD establishes that no descendant remains.
/// The caller must block new starts, pin this process and check the guard channel.
pub(crate) fn reap(connector: Option<Pid>, deadline: Instant) -> Result<Children, DriverError> {
    let mut connector_exit = None;
    loop {
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        match wait(WaitOptions::NOHANG) {
            Ok(Some((pid, status))) => {
                if Some(pid) == connector && connector_exit.is_none() {
                    connector_exit = Some(Exit {
                        code: status.exit_status(),
                        signal: status.terminating_signal(),
                    });
                }
            }
            Ok(None) => {
                return Ok(Children {
                    none: false,
                    connector_exit,
                });
            }
            Err(Errno::CHILD) => {
                return Ok(Children {
                    none: true,
                    connector_exit,
                });
            }
            Err(Errno::INTR) => continue,
            Err(_) => return Err(unavailable()),
        }
    }
}
