//! A single pinned supervisor channel, permanently fenced by a partial exchange.

use futures::future::BoxFuture;
use orch_driver_contract::driver::DriverError;
use tokio::net::UnixStream;
use tokio::time::Instant;

use crate::guard_stop::SupervisorLink;
use crate::peer::{self, WorkloadPeer};
use crate::process_scope;
use crate::subreaper::Children;
use crate::supervisor_protocol::{self, SupervisorReply, SupervisorRequest};

fn unavailable() -> DriverError {
    DriverError::Unavailable("the pinned workload supervisor is unavailable".into())
}

pub(crate) trait PeerCheck: Send + Sync {
    fn verify(&self) -> Result<(), DriverError>;
}

pub(crate) struct PinnedPeer {
    process: WorkloadPeer,
    container_id: String,
}

impl PeerCheck for PinnedPeer {
    fn verify(&self) -> Result<(), DriverError> {
        if !process_scope::process_is_live(&self.process.pidfd)? {
            return Err(unavailable());
        }
        let cgroup = std::fs::read_to_string(format!("/proc/{}/cgroup", self.process.pid))
            .map_err(|_| unavailable())?;
        if !process_scope::belongs_to_container(&cgroup, &self.container_id)
            || !process_scope::process_is_live(&self.process.pidfd)?
        {
            return Err(unavailable());
        }
        Ok(())
    }
}

/// A channel is never reused after cancellation, a partial frame or a wrong reply.
pub(crate) struct SupervisorChannel<P> {
    stream: UnixStream,
    peer: P,
    poisoned: bool,
}

impl SupervisorChannel<PinnedPeer> {
    /// Call before the workload starts, then remove the shared rendezvous listener.
    pub(crate) fn pin(stream: UnixStream, container_reference: &str) -> Result<Self, DriverError> {
        let id = process_scope::container_id(container_reference)?.to_owned();
        let process = peer::authenticate(&stream, container_reference)?;
        let peer = PinnedPeer {
            process,
            container_id: id,
        };
        peer.verify()?;
        Ok(Self {
            stream,
            peer,
            poisoned: false,
        })
    }

    /// The image-selected identity comes from the authenticated peer, not the Pod's user input.
    pub(crate) fn identity(&self) -> (u32, u32) {
        (self.peer.process.uid, self.peer.process.gid)
    }

    pub(crate) fn supervisor_pid(&self) -> i32 {
        self.peer.process.pid
    }
}

impl<P: PeerCheck> SupervisorChannel<P> {
    fn alive(&self) -> Result<(), DriverError> {
        if self.poisoned {
            return Err(unavailable());
        }
        self.peer.verify()
    }

    async fn exchange<T>(
        &mut self,
        request: SupervisorRequest,
        parse: impl FnOnce(SupervisorReply) -> Result<T, DriverError>,
        deadline: Instant,
    ) -> Result<T, DriverError> {
        self.alive()?;
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        // This is set before the first await: dropping the future also leaves
        // the stream fenced, even if bytes were sent or partially received.
        self.poisoned = true;
        supervisor_protocol::write(&mut self.stream, &request, deadline).await?;
        let reply = supervisor_protocol::read(&mut self.stream, deadline).await?;
        let result = parse(reply)?;
        self.peer.verify()?;
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        self.poisoned = false;
        Ok(result)
    }

    pub(crate) async fn prepare(&mut self, deadline: Instant) -> Result<(), DriverError> {
        self.exchange(
            SupervisorRequest::Prepare,
            |reply| match reply {
                SupervisorReply::Prepared => Ok(()),
                _ => Err(unavailable()),
            },
            deadline,
        )
        .await
    }

    pub(crate) async fn start(&mut self, deadline: Instant) -> Result<(), DriverError> {
        self.exchange(
            SupervisorRequest::Start,
            |reply| match reply {
                SupervisorReply::Started => Ok(()),
                _ => Err(unavailable()),
            },
            deadline,
        )
        .await
    }
}

impl<P: PeerCheck> SupervisorLink for SupervisorChannel<P> {
    fn freeze_starts(&mut self, deadline: Instant) -> BoxFuture<'_, Result<(), DriverError>> {
        Box::pin(async move {
            self.exchange(
                SupervisorRequest::Freeze,
                |reply| match reply {
                    SupervisorReply::Frozen => Ok(()),
                    _ => Err(unavailable()),
                },
                deadline,
            )
            .await
        })
    }

    fn children(&mut self, deadline: Instant) -> BoxFuture<'_, Result<Children, DriverError>> {
        Box::pin(async move {
            self.exchange(
                SupervisorRequest::Children,
                SupervisorReply::children,
                deadline,
            )
            .await
        })
    }

    fn check_live(&self) -> Result<(), DriverError> {
        self.alive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Live;
    impl PeerCheck for Live {
        fn verify(&self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_guard_exchanges_freeze_and_fresh_children_on_one_channel() {
        let (client, mut helper) = UnixStream::pair().unwrap();
        let mut channel = SupervisorChannel {
            stream: client,
            peer: Live,
            poisoned: false,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let helper = tokio::spawn(async move {
            assert_eq!(
                supervisor_protocol::read::<_, SupervisorRequest>(&mut helper, deadline)
                    .await
                    .unwrap(),
                SupervisorRequest::Freeze
            );
            supervisor_protocol::write(&mut helper, &SupervisorReply::Frozen, deadline)
                .await
                .unwrap();
            assert_eq!(
                supervisor_protocol::read::<_, SupervisorRequest>(&mut helper, deadline)
                    .await
                    .unwrap(),
                SupervisorRequest::Children
            );
            supervisor_protocol::write(
                &mut helper,
                &SupervisorReply::Children {
                    none: true,
                    connector_exit: None,
                },
                deadline,
            )
            .await
            .unwrap();
        });
        channel.freeze_starts(deadline).await.unwrap();
        assert!(channel.children(deadline).await.unwrap().none);
        assert!(channel.check_live().is_ok());
        helper.await.unwrap();
    }

    #[tokio::test]
    async fn cancelling_after_a_request_permanently_fences_the_channel() {
        let (client, mut helper) = UnixStream::pair().unwrap();
        let mut channel = SupervisorChannel {
            stream: client,
            peer: Live,
            poisoned: false,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        {
            let mut call = channel.freeze_starts(deadline);
            tokio::select! {
                received = supervisor_protocol::read::<_, SupervisorRequest>(&mut helper, deadline) => {
                    assert_eq!(received.unwrap(), SupervisorRequest::Freeze);
                }
                result = &mut call => panic!("a request without a reply finished: {result:?}"),
            }
        }
        assert!(channel.check_live().is_err());
        assert!(channel.freeze_starts(deadline).await.is_err());
    }

    #[tokio::test]
    async fn an_unexpected_reply_is_never_reused_as_another_result() {
        let (client, mut helper) = UnixStream::pair().unwrap();
        let mut channel = SupervisorChannel {
            stream: client,
            peer: Live,
            poisoned: false,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let helper = tokio::spawn(async move {
            let _: SupervisorRequest = supervisor_protocol::read(&mut helper, deadline)
                .await
                .unwrap();
            supervisor_protocol::write(&mut helper, &SupervisorReply::Started, deadline)
                .await
                .unwrap();
        });
        assert!(channel.freeze_starts(deadline).await.is_err());
        assert!(channel.check_live().is_err());
        helper.await.unwrap();
    }
}
