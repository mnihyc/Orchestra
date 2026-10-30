//! Local supervisor transitions; the guard still verifies process scope and channel identity.

#![cfg(target_os = "linux")]

use orch_driver_contract::driver::DriverError;
use rustix::process::Pid;

use crate::subreaper::{Children, Exit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Unprepared,
    Stopped,
    Running,
    Stopping,
    Failed,
}

/// State is local to the pinned supervisor and cannot by itself prove a unit is stopped.
pub(crate) struct SupervisorState {
    phase: Phase,
    connector: Option<Pid>,
    exit: Option<Exit>,
}

impl Default for SupervisorState {
    fn default() -> Self {
        Self {
            phase: Phase::Unprepared,
            connector: None,
            exit: None,
        }
    }
}

impl SupervisorState {
    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }
    pub(crate) fn connector(&self) -> Option<Pid> {
        self.connector
    }
    pub(crate) fn exit(&self) -> Option<Exit> {
        self.exit
    }

    /// The authenticated guard prepares a supervisor only when the kernel reports no children.
    pub(crate) fn prepare(&mut self, children: Children) -> Result<(), DriverError> {
        if self.phase != Phase::Unprepared || !children.none || children.connector_exit.is_some() {
            return Err(DriverError::Unavailable(
                "the workload supervisor is not empty".into(),
            ));
        }
        self.phase = Phase::Stopped;
        Ok(())
    }

    /// Checks the state before spawning; the caller serializes the guard channel around this operation.
    /// The closure must return the PID of any child it successfully creates.
    pub(crate) fn start(
        &mut self,
        before: Children,
        spawn: impl FnOnce() -> Result<Pid, DriverError>,
    ) -> Result<(), DriverError> {
        if self.phase != Phase::Stopped || !before.none || before.connector_exit.is_some() {
            return Err(DriverError::Busy(
                "the workload supervisor is not ready to start".into(),
            ));
        }
        let connector = spawn()?;
        self.connector = Some(connector);
        self.exit = None;
        self.phase = Phase::Running;
        Ok(())
    }

    /// Closes the start path before the guard signals any workload process.
    pub(crate) fn begin_stop(&mut self) -> Result<(), DriverError> {
        match self.phase {
            Phase::Running | Phase::Failed => {
                self.phase = Phase::Stopping;
                Ok(())
            }
            Phase::Stopping | Phase::Stopped => Ok(()),
            Phase::Unprepared => Err(DriverError::Unavailable(
                "the workload supervisor is not prepared".into(),
            )),
        }
    }

    /// Consumes an actual subreaper observation, not an empty process-list snapshot.
    pub(crate) fn observe(&mut self, children: Children) -> Result<(), DriverError> {
        if let Some(exit) = children.connector_exit {
            self.exit = Some(exit);
            self.connector = None;
            if self.phase == Phase::Running {
                self.phase = Phase::Failed;
            }
        }
        match self.phase {
            Phase::Stopping if children.none => {
                self.connector = None;
                self.phase = Phase::Stopped;
            }
            Phase::Running if children.none => {
                self.connector = None;
                self.phase = Phase::Failed;
            }
            Phase::Stopped if !children.none => {
                self.phase = Phase::Failed;
                return Err(DriverError::Unavailable(
                    "the stopped supervisor still has children".into(),
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn children(none: bool, connector_exit: Option<Exit>) -> Children {
        Children {
            none,
            connector_exit,
        }
    }

    #[test]
    fn a_stop_cannot_complete_or_start_again_while_any_descendant_remains() {
        let mut supervisor = SupervisorState::default();
        supervisor.prepare(children(true, None)).unwrap();
        let connector = Pid::from_raw(42).unwrap();
        supervisor
            .start(children(true, None), || Ok(connector))
            .unwrap();
        supervisor.begin_stop().unwrap();
        let exit = Exit {
            code: Some(0),
            signal: None,
        };
        supervisor.observe(children(false, Some(exit))).unwrap();
        assert_eq!(supervisor.phase(), Phase::Stopping);
        assert_eq!(supervisor.exit(), Some(exit));
        let mut spawned = false;
        assert!(
            supervisor
                .start(children(true, None), || {
                    spawned = true;
                    Ok(connector)
                })
                .is_err()
        );
        assert!(!spawned);
        supervisor.observe(children(true, None)).unwrap();
        assert_eq!(supervisor.phase(), Phase::Stopped);
        assert_eq!(supervisor.connector(), None);
    }

    #[test]
    fn an_unexpected_connector_exit_does_not_look_like_an_ordinary_stop() {
        let mut supervisor = SupervisorState::default();
        supervisor.prepare(children(true, None)).unwrap();
        supervisor
            .start(children(true, None), || Ok(Pid::from_raw(42).unwrap()))
            .unwrap();
        supervisor
            .observe(children(
                false,
                Some(Exit {
                    code: Some(1),
                    signal: None,
                }),
            ))
            .unwrap();
        assert_eq!(supervisor.phase(), Phase::Failed);
        supervisor.begin_stop().unwrap();
        supervisor.observe(children(true, None)).unwrap();
        assert_eq!(supervisor.phase(), Phase::Stopped);
    }

    #[test]
    fn an_unprepared_or_inconsistently_stopped_supervisor_never_accepts_a_start() {
        let mut supervisor = SupervisorState::default();
        assert!(supervisor.begin_stop().is_err());
        assert!(supervisor.prepare(children(false, None)).is_err());
        supervisor.prepare(children(true, None)).unwrap();
        assert!(supervisor.observe(children(false, None)).is_err());
        assert_eq!(supervisor.phase(), Phase::Failed);
        assert!(
            supervisor
                .start(children(true, None), || Ok(Pid::from_raw(42).unwrap()))
                .is_err()
        );
    }

    #[test]
    fn a_failed_spawn_does_not_claim_the_connector_started() {
        let mut supervisor = SupervisorState::default();
        supervisor.prepare(children(true, None)).unwrap();
        assert!(
            supervisor
                .start(children(true, None), || Err(DriverError::Unavailable(
                    "no executable".into()
                )))
                .is_err()
        );
        assert_eq!(supervisor.phase(), Phase::Stopped);
        assert_eq!(supervisor.connector(), None);
    }
}
