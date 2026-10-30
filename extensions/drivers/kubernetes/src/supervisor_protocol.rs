//! Messages on the private, pinned guard-to-supervisor connection.
//!
//! The channel is authenticated by its original peer identity before any
//! workload is started; a partial frame or disconnect invalidates it.

use orch_driver_contract::driver::DriverError;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{self, Write};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

use crate::subreaper::{Children, Exit};

/// These messages have no user content; this bound is far above their fixed fields.
const MAX_MESSAGE: usize = 4096;

struct Bounded(Vec<u8>);

impl Write for Bounded {
    fn write(&mut self, chunk: &[u8]) -> io::Result<usize> {
        if self
            .0
            .len()
            .checked_add(chunk.len())
            .is_none_or(|size| size > MAX_MESSAGE)
        {
            return Err(io::Error::other("supervisor message too large"));
        }
        self.0.extend_from_slice(chunk);
        Ok(chunk.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SupervisorRequest {
    /// Establish the initially empty stopped state before the first start.
    Prepare,
    /// Start the connector using guard-prepared private files.
    Start,
    /// Refuse all starts until the current stop has finished.
    Freeze,
    /// Reap children and return the kernel's current ECHILD observation.
    Children,
}

impl<'de> Deserialize<'de> for SupervisorRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            operation: Operation,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Operation {
            Prepare,
            Start,
            Freeze,
            Children,
        }
        Ok(match Wire::deserialize(deserializer)?.operation {
            Operation::Prepare => Self::Prepare,
            Operation::Start => Self::Start,
            Operation::Freeze => Self::Freeze,
            Operation::Children => Self::Children,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExitWire {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SupervisorReply {
    Prepared,
    Started,
    Frozen,
    Children {
        none: bool,
        connector_exit: Option<ExitWire>,
    },
    Refused,
}

impl<'de> Deserialize<'de> for SupervisorReply {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Prepared {},
            Started {},
            Frozen {},
            Refused {},
            Children {
                none: bool,
                connector_exit: Option<ExitWire>,
            },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Prepared {} => Self::Prepared,
            Wire::Started {} => Self::Started,
            Wire::Frozen {} => Self::Frozen,
            Wire::Refused {} => Self::Refused,
            Wire::Children {
                none,
                connector_exit,
            } => Self::Children {
                none,
                connector_exit,
            },
        })
    }
}

impl SupervisorReply {
    /// Only the Children reply can supply a fresh descendant observation.
    pub(crate) fn children(self) -> Result<Children, DriverError> {
        let Self::Children {
            none,
            connector_exit,
        } = self
        else {
            return Err(unavailable());
        };
        if connector_exit.as_ref().is_some_and(|exit| {
            exit.code.is_some() == exit.signal.is_some()
                || exit.code.is_some_and(|code| !(0..=255).contains(&code))
                || exit.signal.is_some_and(|signal| signal <= 0)
        }) {
            return Err(unavailable());
        }
        Ok(Children {
            none,
            connector_exit: connector_exit.map(|exit| Exit {
                code: exit.code,
                signal: exit.signal,
            }),
        })
    }
}

fn unavailable() -> DriverError {
    DriverError::Unavailable("the pinned workload supervisor could not exchange a message".into())
}

/// A caller closes and quarantines the channel after any error, including a partial write.
pub(crate) async fn write<W: AsyncWrite + Unpin, T: Serialize>(
    stream: &mut W,
    message: &T,
    deadline: Instant,
) -> Result<(), DriverError> {
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    let mut bytes = Bounded(Vec::new());
    serde_json::to_writer(&mut bytes, message).map_err(|_| unavailable())?;
    let bytes = bytes.0;
    if bytes.is_empty() || bytes.len() > MAX_MESSAGE {
        return Err(unavailable());
    }
    let length = u32::try_from(bytes.len()).map_err(|_| unavailable())?;
    tokio::time::timeout_at(deadline, async {
        stream.write_all(&length.to_be_bytes()).await?;
        stream.write_all(&bytes).await?;
        stream.flush().await
    })
    .await
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| unavailable())?;
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok(())
}

/// A peer cannot cause an arbitrary allocation or keep the operation past its absolute deadline.
pub(crate) async fn read<R: AsyncRead + Unpin, T: DeserializeOwned>(
    stream: &mut R,
    deadline: Instant,
) -> Result<T, DriverError> {
    read_from(stream, None, deadline).await
}

/// Waits up to `idle` for a message to begin and answers its first byte, or `None` when no byte came. The read that
/// waits has taken nothing when the time runs out, so nothing is lost; a closed connection is an error, not silence.
/// A service that has other things to look after while it waits must wait this way and not on the connection's
/// readiness: a connection that was read to the last byte of its last message still reports itself readable until a
/// read finds nothing, and a read that then waits for the next message holds the service until its deadline passes.
pub(crate) async fn begins<R: AsyncRead + Unpin>(
    stream: &mut R,
    idle: std::time::Duration,
) -> Result<Option<u8>, DriverError> {
    let mut first = [0_u8; 1];
    match tokio::time::timeout(idle, stream.read(&mut first)).await {
        Err(_) => Ok(None),
        Ok(Ok(1)) => Ok(Some(first[0])),
        Ok(_) => Err(unavailable()),
    }
}

/// Reads a message of which `begins` has already taken the first byte (`None` when it has taken none), under the
/// same bounds as [`read`].
pub(crate) async fn read_from<R: AsyncRead + Unpin, T: DeserializeOwned>(
    stream: &mut R,
    first: Option<u8>,
    deadline: Instant,
) -> Result<T, DriverError> {
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    let bytes = tokio::time::timeout_at(deadline, async {
        let mut prefix = [0_u8; 4];
        let taken = usize::from(first.is_some());
        if let Some(byte) = first {
            prefix[0] = byte;
        }
        stream
            .read_exact(&mut prefix[taken..])
            .await
            .map_err(|_| unavailable())?;
        let length = usize::try_from(u32::from_be_bytes(prefix)).map_err(|_| unavailable())?;
        if length == 0 || length > MAX_MESSAGE {
            return Err(unavailable());
        }
        let mut bytes = vec![0; length];
        stream
            .read_exact(&mut bytes)
            .await
            .map_err(|_| unavailable())?;
        Ok(bytes)
    })
    .await
    .map_err(|_| DriverError::Timeout)??;
    let message = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn every_supervisor_control_message_rejects_unknown_fields() {
        for operation in ["prepare", "start", "freeze", "children"] {
            assert!(
                serde_json::from_value::<SupervisorRequest>(
                    serde_json::json!({"operation":operation})
                )
                .is_ok()
            );
            assert!(
                serde_json::from_value::<SupervisorRequest>(
                    serde_json::json!({"operation":operation,"extra":true})
                )
                .is_err()
            );
        }
        for result in ["prepared", "started", "frozen", "refused"] {
            assert!(
                serde_json::from_value::<SupervisorReply>(serde_json::json!({"result":result}))
                    .is_ok()
            );
            assert!(
                serde_json::from_value::<SupervisorReply>(
                    serde_json::json!({"result":result,"extra":true})
                )
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn a_fresh_children_observation_round_trips_without_turning_an_ack_into_one() {
        let (mut sender, mut receiver) = tokio::io::duplex(1024);
        let deadline = Instant::now() + Duration::from_secs(5);
        let message = SupervisorReply::Children {
            none: false,
            connector_exit: Some(ExitWire {
                code: Some(1),
                signal: None,
            }),
        };
        write(&mut sender, &message, deadline).await.unwrap();
        let received: SupervisorReply = read(&mut receiver, deadline).await.unwrap();
        let children = received.children().unwrap();
        assert!(!children.none);
        assert_eq!(children.connector_exit.unwrap().code, Some(1));
        assert!(SupervisorReply::Started.children().is_err());
        assert!(
            SupervisorReply::Children {
                none: true,
                connector_exit: Some(ExitWire {
                    code: Some(0),
                    signal: Some(9)
                }),
            }
            .children()
            .is_err()
        );
    }

    #[tokio::test]
    async fn oversized_truncated_and_expired_frames_are_refused() {
        let (mut sender, mut receiver) = tokio::io::duplex(64);
        sender.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        assert!(
            read::<_, SupervisorRequest>(&mut receiver, Instant::now() + Duration::from_secs(5))
                .await
                .is_err()
        );
        let (mut sender, mut receiver) = tokio::io::duplex(64);
        sender.write_all(&10_u32.to_be_bytes()).await.unwrap();
        sender.write_all(b"short").await.unwrap();
        drop(sender);
        assert!(
            read::<_, SupervisorRequest>(&mut receiver, Instant::now() + Duration::from_secs(5))
                .await
                .is_err()
        );
        assert!(
            write(
                &mut tokio::io::sink(),
                &"x".repeat(MAX_MESSAGE),
                Instant::now() + Duration::from_secs(5)
            )
            .await
            .is_err()
        );
        assert!(matches!(
            write(
                &mut tokio::io::sink(),
                &SupervisorRequest::Start,
                Instant::now()
            )
            .await,
            Err(DriverError::Timeout)
        ));
        assert!(
            serde_json::from_value::<SupervisorRequest>(
                serde_json::json!({"operation":"start", "extra":true})
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn the_readiness_of_a_connection_stays_set_after_a_frame_read_to_its_last_byte() {
        // Why a service does not wait for a message on `readable()`: it answers at once here, with nothing to read.
        let (mut guard, mut supervisor) = tokio::net::UnixStream::pair().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        write(&mut guard, &SupervisorRequest::Children, deadline)
            .await
            .unwrap();
        let request: SupervisorRequest = read(&mut supervisor, deadline).await.unwrap();
        assert_eq!(request, SupervisorRequest::Children);
        tokio::time::timeout(Duration::from_millis(500), supervisor.readable())
            .await
            .expect("the connection reports itself readable at once")
            .unwrap();
        let mut nothing = [0_u8; 1];
        assert!(matches!(
            supervisor.try_read(&mut nothing),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }

    #[tokio::test]
    async fn a_connection_read_to_its_last_byte_is_idle_and_not_ready() {
        // The two ends are those of the guard and the supervisor. Every frame is read with exact reads, so the
        // supervisor's connection keeps reporting itself readable after the last one, which a wait on its
        // readiness takes for a message that has begun.
        let (mut guard, mut supervisor) = tokio::net::UnixStream::pair().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        write(&mut guard, &SupervisorRequest::Children, deadline)
            .await
            .unwrap();
        let first = begins(&mut supervisor, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(first.is_some());
        let request: SupervisorRequest = read_from(&mut supervisor, first, deadline).await.unwrap();
        assert_eq!(request, SupervisorRequest::Children);
        // Nothing comes: the wait ends at its time, with no message and no byte taken.
        let waited = std::time::Instant::now();
        assert_eq!(
            begins(&mut supervisor, Duration::from_millis(100))
                .await
                .unwrap(),
            None
        );
        assert!(waited.elapsed() >= Duration::from_millis(90));
        // A message that comes after the silence is whole.
        write(&mut guard, &SupervisorRequest::Start, deadline)
            .await
            .unwrap();
        let first = begins(&mut supervisor, Duration::from_secs(5))
            .await
            .unwrap();
        let request: SupervisorRequest = read_from(&mut supervisor, first, deadline).await.unwrap();
        assert_eq!(request, SupervisorRequest::Start);
        // A closed connection is an error and not more silence.
        drop(guard);
        assert!(
            begins(&mut supervisor, Duration::from_secs(5))
                .await
                .is_err()
        );
    }
}
