//! Resumable transit pieces over the same authenticated connection as stored content.

use super::{Content, PutRefusal, RelayReply, RelayRequest};
use async_trait::async_trait;
use orch_envhost::blobs::{BlobError, BlobReader, BlobWriter, RelaySink, RelaySource};
use orch_link_contract::content::next_chunk_bytes;
use orch_types::Id;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// The one memory rendezvous named by a command, with its resolved operational allowances.
pub struct NativeRelay {
    content: Arc<Content>,
    command: Id,
    relay: Id,
    idle: Duration,
    retry: Duration,
}

impl Content {
    /// Uses the command's authenticated content endpoint; this creates no stored-content authority.
    pub fn relay_for(
        self: &Arc<Self>,
        command: Id,
        relay: Id,
        idle_secs: u64,
        retry_ms: u64,
    ) -> NativeRelay {
        NativeRelay {
            content: Arc::clone(self),
            command,
            relay,
            idle: Duration::from_secs(idle_secs.max(1)),
            retry: Duration::from_millis(retry_ms.max(1)),
        }
    }
}

fn failure(error: PutRefusal) -> BlobError {
    match error {
        PutRefusal::HashMismatch => BlobError::HashMismatch,
        PutRefusal::Refused(reason) => BlobError::Refused(reason),
        PutRefusal::Failed(_) => BlobError::StoodStill,
        PutRefusal::Offset { received } => BlobError::Refused(format!(
            "the relay destination holds another prefix ({received} bytes)"
        )),
    }
}

fn local(error: std::io::Error) -> BlobError {
    BlobError::Local(error.to_string())
}

impl NativeRelay {
    async fn request(
        &self,
        request: RelayRequest,
        since: Instant,
    ) -> Result<RelayReply, PutRefusal> {
        loop {
            let left = self.idle.saturating_sub(since.elapsed());
            if left.is_zero() {
                return Err(PutRefusal::Failed("the file relay stood still".into()));
            }
            match tokio::time::timeout(
                left,
                self.content
                    .endpoint
                    .relay(self.command, self.relay, request.clone(), left),
            )
            .await
            {
                Ok(Err(PutRefusal::Failed(_))) => {
                    tokio::time::sleep(self.retry.min(self.idle.saturating_sub(since.elapsed())))
                        .await;
                }
                Ok(result) => return result,
                Err(_) => return Err(PutRefusal::Failed("the file relay stood still".into())),
            }
        }
    }

    async fn acknowledge(&self, received: i64, since: Instant) -> Result<(), BlobError> {
        match self
            .request(RelayRequest::Stand { received }, since)
            .await
            .map_err(failure)?
        {
            RelayReply::Progress(held) if held == received => Ok(()),
            _ => Err(BlobError::Refused(
                "the relay did not acknowledge the destination's prefix".into(),
            )),
        }
    }
}

#[async_trait]
impl RelaySource for NativeRelay {
    async fn receive(&self, size: i64, target: &mut dyn BlobWriter) -> Result<(), BlobError> {
        let mut offset = target.received().await.map_err(local)?;
        if offset < 0 || offset > size {
            return Err(BlobError::Refused(
                "the native temporary file has another size than this upload".into(),
            ));
        }
        let mut progress = Instant::now();
        self.acknowledge(offset, progress).await?;
        let mut rate = None;
        while offset < size {
            let len = next_chunk_bytes(rate).min(size - offset);
            let began = Instant::now();
            match self
                .request(RelayRequest::Pull { offset, len }, progress)
                .await
                .map_err(failure)?
            {
                RelayReply::Piece(bytes) if !bytes.is_empty() => {
                    let got = i64::try_from(bytes.len()).map_err(|_| {
                        BlobError::Refused("the relay piece cannot be represented".into())
                    })?;
                    if got > len {
                        return Err(BlobError::Refused(
                            "the relay sent more than the requested range".into(),
                        ));
                    }
                    target.write(offset, bytes).await.map_err(local)?;
                    offset += got;
                    self.content.transfers.note(&mut rate, got, began, false);
                    progress = Instant::now();
                    // Acknowledgement follows the native writer's flush, including the final piece.
                    self.acknowledge(offset, progress).await?;
                }
                RelayReply::Idle => {}
                _ => {
                    return Err(BlobError::Refused(
                        "the relay ended before the upload was complete".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl RelaySink for NativeRelay {
    async fn send(&self, size: i64, source: &mut dyn BlobReader) -> Result<(), BlobError> {
        if size < 0 {
            return Err(BlobError::Refused(
                "a relayed range has a negative size".into(),
            ));
        }
        let mut offset = 0;
        let mut rate = None;
        while offset < size {
            let len = next_chunk_bytes(rate).min(size - offset);
            let bytes = source.read(offset, len).await.map_err(local)?;
            let got = i64::try_from(bytes.len())
                .map_err(|_| BlobError::Refused("the relay piece cannot be represented".into()))?;
            if got == 0 || got > len {
                return Err(BlobError::Refused(
                    "the source did not provide the requested range".into(),
                ));
            }
            let began = Instant::now();
            let sent = self
                .request(RelayRequest::Push { offset, bytes }, began)
                .await;
            let received = match sent {
                Ok(RelayReply::Progress(received)) | Err(PutRefusal::Offset { received }) => {
                    received
                }
                Ok(_) => {
                    return Err(BlobError::Refused(
                        "the relay states no progress for its reader".into(),
                    ));
                }
                Err(error) => return Err(failure(error)),
            };
            if received != offset + got {
                return Err(BlobError::Refused(
                    "the relay reader acknowledged another range".into(),
                ));
            }
            self.content.transfers.note(&mut rate, got, began, true);
            offset = received;
        }
        Ok(())
    }
}
