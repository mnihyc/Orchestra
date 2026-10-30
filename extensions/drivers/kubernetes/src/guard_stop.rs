//! Stop exact workload processes only after the trusted supervisor blocks new starts.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use futures::future::BoxFuture;
use orch_driver_contract::driver::DriverError;
use rustix::process::Signal;
use tokio::time::Instant;

use crate::process_scope::{self, SignalResult};
use crate::subreaper::Children;

/// Rechecking frequently bounds shutdown latency without a busy process scan.
const PROCESS_SCAN_INTERVAL: Duration = Duration::from_millis(50);

/// Only an authenticated, pinned supervisor can answer these operations.
pub(crate) trait SupervisorLink {
    fn freeze_starts(&mut self, deadline: Instant) -> BoxFuture<'_, Result<(), DriverError>>;
    fn children(&mut self, deadline: Instant) -> BoxFuture<'_, Result<Children, DriverError>>;
    fn check_live(&self) -> Result<(), DriverError>;
}

/// Each signal and scan refers only to the original workload container's exact cgroup.
pub(crate) trait ProcessScope {
    type Fence;
    fn members(&self, deadline: Instant) -> Result<Vec<i32>, DriverError>;
    fn signal(
        &self,
        pid: i32,
        signal: Signal,
        deadline: Instant,
    ) -> Result<Option<Self::Fence>, DriverError>;
    fn is_live(&self, fence: &Self::Fence) -> Result<bool, DriverError>;
}

/// The supported containerd/cgroup-v2 scope; the supervisor PID comes from the pinned socket peer.
pub(crate) struct ContainerScope<'a> {
    id: &'a str,
    supervisor_pid: i32,
}

impl<'a> ContainerScope<'a> {
    pub(crate) fn new(
        container_reference: &'a str,
        supervisor_pid: i32,
    ) -> Result<Self, DriverError> {
        let id = process_scope::container_id(container_reference)?;
        let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")
            .map_err(|_| DriverError::Unavailable("the process view cannot be checked".into()))?;
        if supervisor_pid <= 0 || !process_scope::visible_proc(&mountinfo) {
            return Err(DriverError::Unavailable(
                "the workload process view is incomplete".into(),
            ));
        }
        Ok(Self { id, supervisor_pid })
    }
}

impl ProcessScope for ContainerScope<'_> {
    type Fence = std::os::fd::OwnedFd;
    fn members(&self, deadline: Instant) -> Result<Vec<i32>, DriverError> {
        process_scope::members(
            Path::new("/proc"),
            self.id,
            self.supervisor_pid,
            deadline.into_std(),
        )
    }

    fn signal(
        &self,
        pid: i32,
        signal: Signal,
        deadline: Instant,
    ) -> Result<Option<Self::Fence>, DriverError> {
        match process_scope::signal_member(
            pid,
            self.supervisor_pid,
            self.id,
            signal,
            deadline.into_std(),
        )? {
            SignalResult::Sent(handle) => Ok(Some(handle)),
            SignalResult::Gone | SignalResult::Skipped => Ok(None),
        }
    }

    fn is_live(&self, fence: &Self::Fence) -> Result<bool, DriverError> {
        process_scope::process_is_live(fence)
    }
}

/// Stop under one absolute deadline; success requires both ECHILD and an empty verified process scope.
pub(crate) async fn stop(
    link: &mut impl SupervisorLink,
    scope: &impl ProcessScope,
    grace: Duration,
    deadline: Instant,
) -> Result<(), DriverError> {
    let now = Instant::now();
    let soft_end = now.checked_add(grace).ok_or(DriverError::Timeout)?;
    if now >= deadline || soft_end >= deadline {
        return Err(DriverError::Timeout);
    }
    tokio::time::timeout_at(deadline, link.freeze_starts(deadline))
        .await
        .map_err(|_| DriverError::Timeout)??;
    let mut gently_signalled = BTreeMap::new();
    let (children, members) = observe(link, scope, deadline).await?;
    if children.none && members.is_empty() {
        return Ok(());
    }
    for pid in members {
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        if grace.is_zero() {
            let _ = scope.signal(pid, Signal::KILL, deadline)?;
        } else {
            if let Some(fence) = scope.signal(pid, Signal::TERM, deadline)? {
                gently_signalled.insert(pid, fence);
            }
        }
    }
    // Give the full requested grace after the initial signals, not after the handshake began.
    let soft_end = Instant::now()
        .checked_add(grace)
        .ok_or(DriverError::Timeout)?;
    if soft_end >= deadline {
        return Err(DriverError::Timeout);
    }
    loop {
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        let (children, members) = observe(link, scope, deadline).await?;
        if children.none && members.is_empty() {
            return Ok(());
        }
        let hard = Instant::now() >= soft_end;
        let present: BTreeSet<_> = members.iter().copied().collect();
        gently_signalled.retain(|pid, _| present.contains(pid));
        for pid in members {
            if Instant::now() >= deadline {
                return Err(DriverError::Timeout);
            }
            if hard {
                let _ = scope.signal(pid, Signal::KILL, deadline)?;
            } else if !gently_signalled
                .get(&pid)
                .map(|fence| scope.is_live(fence))
                .transpose()?
                .unwrap_or(false)
            {
                if let Some(fence) = scope.signal(pid, Signal::TERM, deadline)? {
                    gently_signalled.insert(pid, fence);
                } else {
                    gently_signalled.remove(&pid);
                }
            }
        }
        let next = Instant::now()
            .checked_add(PROCESS_SCAN_INTERVAL)
            .ok_or(DriverError::Timeout)?;
        tokio::time::sleep_until(
            next.min(deadline)
                .min(if hard { deadline } else { soft_end }),
        )
        .await;
    }
}

async fn observe(
    link: &mut impl SupervisorLink,
    scope: &impl ProcessScope,
    deadline: Instant,
) -> Result<(Children, Vec<i32>), DriverError> {
    link.check_live()?;
    let children = tokio::time::timeout_at(deadline, link.children(deadline))
        .await
        .map_err(|_| DriverError::Timeout)??;
    let members = scope.members(deadline)?;
    link.check_live()?;
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok((children, members))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct Link {
        frozen: bool,
        observations: VecDeque<Children>,
        alive: bool,
    }
    impl SupervisorLink for Link {
        fn freeze_starts(&mut self, _deadline: Instant) -> BoxFuture<'_, Result<(), DriverError>> {
            Box::pin(async move {
                self.frozen = true;
                Ok(())
            })
        }
        fn children(&mut self, _deadline: Instant) -> BoxFuture<'_, Result<Children, DriverError>> {
            Box::pin(async move {
                self.observations
                    .pop_front()
                    .ok_or_else(|| DriverError::Unavailable("no observation".into()))
            })
        }
        fn check_live(&self) -> Result<(), DriverError> {
            if self.alive {
                Ok(())
            } else {
                Err(DriverError::Unavailable("peer ended".into()))
            }
        }
    }
    struct Scope {
        snapshots: Mutex<VecDeque<Vec<i32>>>,
        sent: Mutex<Vec<(i32, Signal)>>,
    }
    impl ProcessScope for Scope {
        type Fence = i32;
        fn members(&self, _deadline: Instant) -> Result<Vec<i32>, DriverError> {
            self.snapshots
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| DriverError::Unavailable("no process view".into()))
        }
        fn signal(
            &self,
            pid: i32,
            signal: Signal,
            _deadline: Instant,
        ) -> Result<Option<Self::Fence>, DriverError> {
            self.sent.lock().unwrap().push((pid, signal));
            Ok(Some(pid))
        }
        fn is_live(&self, _fence: &Self::Fence) -> Result<bool, DriverError> {
            Ok(true)
        }
    }
    fn children(none: bool) -> Children {
        Children {
            none,
            connector_exit: None,
        }
    }

    #[tokio::test]
    async fn a_no_child_reply_does_not_hide_a_process_in_the_verified_scope() {
        let mut link = Link {
            frozen: false,
            observations: VecDeque::from([children(true), children(true)]),
            alive: true,
        };
        let scope = Scope {
            snapshots: Mutex::new(VecDeque::from([vec![42], vec![]])),
            sent: Mutex::new(vec![]),
        };
        stop(
            &mut link,
            &scope,
            Duration::ZERO,
            Instant::now() + Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(link.frozen);
        assert_eq!(*scope.sent.lock().unwrap(), vec![(42, Signal::KILL)]);
    }

    #[tokio::test]
    async fn an_expired_deadline_or_a_dead_supervisor_never_reports_a_stop() {
        let mut link = Link {
            frozen: false,
            observations: VecDeque::from([children(true)]),
            alive: true,
        };
        let scope = Scope {
            snapshots: Mutex::new(VecDeque::from([vec![]])),
            sent: Mutex::new(vec![]),
        };
        assert!(matches!(
            stop(
                &mut link,
                &scope,
                Duration::from_secs(5),
                Instant::now() + Duration::from_secs(1)
            )
            .await,
            Err(DriverError::Timeout)
        ));
        assert!(!link.frozen);
        link.alive = false;
        assert!(
            stop(
                &mut link,
                &scope,
                Duration::ZERO,
                Instant::now() + Duration::from_secs(5)
            )
            .await
            .is_err()
        );
        assert!(scope.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_reused_pid_receives_its_own_termination_signal() {
        struct Reused {
            generation: AtomicUsize,
            sent: Mutex<Vec<(usize, Signal)>>,
        }
        impl ProcessScope for Reused {
            type Fence = usize;
            fn members(&self, _deadline: Instant) -> Result<Vec<i32>, DriverError> {
                let generation = self.generation.fetch_add(1, Ordering::SeqCst);
                Ok(if generation < 2 { vec![42] } else { vec![] })
            }
            fn signal(
                &self,
                _pid: i32,
                signal: Signal,
                _deadline: Instant,
            ) -> Result<Option<Self::Fence>, DriverError> {
                let generation = self.generation.load(Ordering::SeqCst);
                self.sent.lock().unwrap().push((generation, signal));
                Ok(Some(generation))
            }
            fn is_live(&self, fence: &Self::Fence) -> Result<bool, DriverError> {
                Ok(*fence == self.generation.load(Ordering::SeqCst))
            }
        }
        let mut link = Link {
            frozen: false,
            observations: VecDeque::from([children(true), children(true), children(true)]),
            alive: true,
        };
        let scope = Reused {
            generation: AtomicUsize::new(0),
            sent: Mutex::new(vec![]),
        };
        stop(
            &mut link,
            &scope,
            Duration::from_secs(1),
            Instant::now() + Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(
            *scope.sent.lock().unwrap(),
            vec![(1, Signal::TERM), (2, Signal::TERM)]
        );
    }

    #[tokio::test]
    async fn force_waits_for_the_full_grace_after_the_initial_termination_signal() {
        struct LiveLink(Arc<AtomicBool>);
        impl SupervisorLink for LiveLink {
            fn freeze_starts(
                &mut self,
                _deadline: Instant,
            ) -> BoxFuture<'_, Result<(), DriverError>> {
                Box::pin(async { Ok(()) })
            }
            fn children(
                &mut self,
                _deadline: Instant,
            ) -> BoxFuture<'_, Result<Children, DriverError>> {
                Box::pin(async move { Ok(children(self.0.load(Ordering::SeqCst))) })
            }
            fn check_live(&self) -> Result<(), DriverError> {
                Ok(())
            }
        }
        struct LiveScope {
            stopped: Arc<AtomicBool>,
            sent: Mutex<Vec<(Signal, Instant)>>,
        }
        impl ProcessScope for LiveScope {
            type Fence = i32;
            fn members(&self, _deadline: Instant) -> Result<Vec<i32>, DriverError> {
                Ok(if self.stopped.load(Ordering::SeqCst) {
                    vec![]
                } else {
                    vec![42]
                })
            }
            fn signal(
                &self,
                pid: i32,
                signal: Signal,
                _deadline: Instant,
            ) -> Result<Option<Self::Fence>, DriverError> {
                self.sent.lock().unwrap().push((signal, Instant::now()));
                if signal == Signal::KILL {
                    self.stopped.store(true, Ordering::SeqCst);
                }
                Ok(Some(pid))
            }
            fn is_live(&self, _fence: &Self::Fence) -> Result<bool, DriverError> {
                Ok(true)
            }
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let mut link = LiveLink(Arc::clone(&stopped));
        let scope = LiveScope {
            stopped,
            sent: Mutex::new(vec![]),
        };
        let grace = Duration::from_millis(150);
        stop(
            &mut link,
            &scope,
            grace,
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .unwrap();
        let sent = scope.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, Signal::TERM);
        assert_eq!(sent[1].0, Signal::KILL);
        assert!(sent[1].1.duration_since(sent[0].1) >= grace);
    }
}
