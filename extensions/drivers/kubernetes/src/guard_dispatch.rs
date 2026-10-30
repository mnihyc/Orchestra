//! Serialize authenticated guard commands without turning an interrupted operation into success.

use futures::future::BoxFuture;
use orch_driver_contract::driver::DriverError;
use tokio::time::Instant;

use crate::authenticated_control::{Admission, Envelope, GuardKey, ReplayGuard};
use crate::control_protocol::{GuardReply, GuardRequest};

/// A reply may be returned only when the operation is settled and its stated observation is verified.
pub(crate) trait GuardOperations: Send {
    /// Return a nonblocking future; any spawned work must be joined or leave the unit fenced.
    fn execute(
        &mut self,
        command: GuardRequest,
        deadline: Instant,
    ) -> BoxFuture<'_, Result<GuardReply, DriverError>>;
}

/// One dispatcher belongs to one pinned guard and is never shared with another Pod incarnation.
pub(crate) struct GuardDispatcher {
    replay: ReplayGuard,
}

impl GuardDispatcher {
    pub(crate) fn new(pod_uid: &str, key: GuardKey) -> Result<Self, DriverError> {
        Ok(Self {
            replay: ReplayGuard::new(pod_uid, key)?,
        })
    }

    /// An interrupted operation stays pending. A retried reply is historical, so a caller needs
    /// a new authenticated inspection before presenting current state.
    pub(crate) async fn handle(
        &mut self,
        sequence: u64,
        envelope: &Envelope,
        operations: &mut impl GuardOperations,
        deadline: Instant,
    ) -> Result<Envelope, DriverError> {
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        match self.replay.begin(sequence, envelope)? {
            Admission::Retry { request } => {
                let reply = self.replay.sealed_reply(sequence, &request)?;
                if Instant::now() >= deadline {
                    return Err(DriverError::Timeout);
                }
                Ok(reply)
            }
            Admission::Execute { command, request } => {
                if Instant::now() >= deadline {
                    return Err(DriverError::Timeout);
                }
                let reply = tokio::time::timeout_at(deadline, async {
                    operations.execute(command, deadline).await
                })
                .await
                .map_err(|_| DriverError::Timeout)??;
                self.replay.finish(sequence, &request, reply)?;
                if Instant::now() >= deadline {
                    return Err(DriverError::Timeout);
                }
                let envelope = self.replay.sealed_reply(sequence, &request)?;
                if Instant::now() >= deadline {
                    return Err(DriverError::Timeout);
                }
                Ok(envelope)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authenticated_control::{open_response, seal_request};
    use crate::control_protocol::GuardPhase;
    use std::time::Duration;

    struct Operations {
        calls: usize,
        fail: bool,
    }
    impl GuardOperations for Operations {
        fn execute(
            &mut self,
            command: GuardRequest,
            _deadline: Instant,
        ) -> BoxFuture<'_, Result<GuardReply, DriverError>> {
            self.calls += 1;
            let fail = self.fail;
            Box::pin(async move {
                if fail {
                    return Err(DriverError::Unavailable(
                        "fixture operation was interrupted".into(),
                    ));
                }
                let phase = match command {
                    GuardRequest::Start { .. } => GuardPhase::Running,
                    _ => GuardPhase::Stopped,
                };
                Ok(GuardReply::State { phase })
            })
        }
    }

    #[tokio::test]
    async fn an_exact_retry_returns_the_old_reply_without_running_the_command_again() {
        let key = GuardKey::from_bytes([7; 32]);
        let mut guard = GuardDispatcher::new("pod-uid", key.clone()).unwrap();
        let mut operations = Operations {
            calls: 0,
            fail: false,
        };
        let request = serde_json::to_vec(&GuardRequest::Start {
            pod_uid: "pod-uid".into(),
        })
        .unwrap();
        let envelope = seal_request(&key, "pod-uid", 1, &request).unwrap();
        assert!(matches!(
            guard
                .handle(1, &envelope, &mut operations, Instant::now())
                .await,
            Err(DriverError::Timeout)
        ));
        assert_eq!(operations.calls, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        for _ in 0..2 {
            let reply = guard
                .handle(1, &envelope, &mut operations, deadline)
                .await
                .unwrap();
            assert_eq!(
                open_response(&key, "pod-uid", 1, &request, &reply)
                    .unwrap()
                    .response(),
                &GuardReply::State {
                    phase: GuardPhase::Running
                }
            );
        }
        assert_eq!(operations.calls, 1);
        let inspect = serde_json::to_vec(&GuardRequest::Inspect {
            pod_uid: "pod-uid".into(),
        })
        .unwrap();
        let observation = seal_request(&key, "pod-uid", 2, &inspect).unwrap();
        let reply = guard
            .handle(2, &observation, &mut operations, deadline)
            .await
            .unwrap();
        assert_eq!(
            open_response(&key, "pod-uid", 2, &inspect, &reply)
                .unwrap()
                .response(),
            &GuardReply::State {
                phase: GuardPhase::Stopped
            }
        );
        assert!(
            guard
                .handle(1, &envelope, &mut operations, deadline)
                .await
                .is_err()
        );
        assert_eq!(operations.calls, 2);
    }

    #[tokio::test]
    async fn an_interrupted_operation_fences_later_commands() {
        let key = GuardKey::from_bytes([9; 32]);
        let mut guard = GuardDispatcher::new("pod-uid", key.clone()).unwrap();
        let mut operations = Operations {
            calls: 0,
            fail: true,
        };
        let start = serde_json::to_vec(&GuardRequest::Start {
            pod_uid: "pod-uid".into(),
        })
        .unwrap();
        let envelope = seal_request(&key, "pod-uid", 1, &start).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        assert!(
            guard
                .handle(1, &envelope, &mut operations, deadline)
                .await
                .is_err()
        );
        let inspect = serde_json::to_vec(&GuardRequest::Inspect {
            pod_uid: "pod-uid".into(),
        })
        .unwrap();
        let later = seal_request(&key, "pod-uid", 2, &inspect).unwrap();
        assert!(matches!(
            guard.handle(2, &later, &mut operations, deadline).await,
            Err(DriverError::Busy(_))
        ));
        assert_eq!(operations.calls, 1);
    }
}
