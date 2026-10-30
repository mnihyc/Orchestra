//! Bounded transport for encrypted guard messages on an untrusted exec stream.

use orch_driver_contract::driver::DriverError;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

use crate::authenticated_control::{Envelope, NONCE_BYTES, TAG_BYTES};
use crate::control_protocol::MAX_FRAME_BYTES;

const SEQUENCE_BYTES: usize = 8;
/// The length covers a sequence, a nonce, ciphertext and its authentication tag.
const MAX_SEALED_BYTES: usize = SEQUENCE_BYTES + NONCE_BYTES + MAX_FRAME_BYTES + TAG_BYTES;
const MIN_SEALED_BYTES: usize = SEQUENCE_BYTES + NONCE_BYTES + TAG_BYTES + 1;

fn unavailable() -> DriverError {
    DriverError::Unavailable(
        "the Kubernetes unit helper could not exchange an encrypted message".into(),
    )
}

/// A partial write invalidates the stream; the caller closes it instead of reusing it.
pub(crate) async fn write_envelope<W: AsyncWrite + Unpin>(
    stream: &mut W,
    sequence: u64,
    envelope: &Envelope,
    deadline: Instant,
) -> Result<(), DriverError> {
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    let length = SEQUENCE_BYTES
        .checked_add(NONCE_BYTES)
        .and_then(|length| length.checked_add(envelope.ciphertext.len()))
        .filter(|length| (MIN_SEALED_BYTES..=MAX_SEALED_BYTES).contains(length))
        .ok_or_else(unavailable)?;
    let length = u32::try_from(length).map_err(|_| unavailable())?;
    if sequence == 0 {
        return Err(unavailable());
    }
    tokio::time::timeout_at(deadline, async {
        stream.write_all(&length.to_be_bytes()).await?;
        stream.write_all(&sequence.to_be_bytes()).await?;
        stream.write_all(&envelope.nonce).await?;
        stream.write_all(&envelope.ciphertext).await?;
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

/// One absolute deadline covers the header and body, even when a peer trickles bytes.
pub(crate) async fn read_envelope<R: AsyncRead + Unpin>(
    stream: &mut R,
    deadline: Instant,
) -> Result<(u64, Envelope), DriverError> {
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    let result = tokio::time::timeout_at(deadline, async {
        let mut header = [0_u8; 4];
        stream
            .read_exact(&mut header)
            .await
            .map_err(|_| unavailable())?;
        let length = usize::try_from(u32::from_be_bytes(header)).map_err(|_| unavailable())?;
        if !(MIN_SEALED_BYTES..=MAX_SEALED_BYTES).contains(&length) {
            return Err(unavailable());
        }
        let mut sequence = [0_u8; SEQUENCE_BYTES];
        let mut nonce = [0_u8; NONCE_BYTES];
        stream
            .read_exact(&mut sequence)
            .await
            .map_err(|_| unavailable())?;
        stream
            .read_exact(&mut nonce)
            .await
            .map_err(|_| unavailable())?;
        let sequence = u64::from_be_bytes(sequence);
        if sequence == 0 {
            return Err(unavailable());
        }
        let mut ciphertext = vec![0; length - SEQUENCE_BYTES - NONCE_BYTES];
        stream
            .read_exact(&mut ciphertext)
            .await
            .map_err(|_| unavailable())?;
        Ok((sequence, Envelope { nonce, ciphertext }))
    })
    .await
    .map_err(|_| DriverError::Timeout)??;
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authenticated_control::{Admission, GuardKey, ReplayGuard, seal_request};
    use crate::control_protocol::GuardRequest;
    use std::time::Duration;

    #[tokio::test]
    async fn a_frame_round_trips_only_as_an_authenticated_envelope() {
        let key = GuardKey::from_bytes([7; 32]);
        let request = GuardRequest::Prepare {
            pod_uid: "uid-1".into(),
            ca: "test-ca".into(),
            enrollment: "synthetic-once".into(),
            environment: "{}".into(),
            mounts: None,
        };
        let bytes = serde_json::to_vec(&request).unwrap();
        let envelope = seal_request(&key, "uid-1", 1, &bytes).unwrap();
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        let deadline = Instant::now() + Duration::from_secs(5);
        write_envelope(&mut writer, 1, &envelope, deadline)
            .await
            .unwrap();
        let (sequence, decoded) = read_envelope(&mut reader, deadline).await.unwrap();
        let mut guard = ReplayGuard::new("uid-1", key).unwrap();
        assert!(matches!(
            guard.begin(sequence, &decoded),
            Ok(Admission::Execute {
                command: GuardRequest::Prepare { .. },
                ..
            })
        ));
        assert!(!format!("{decoded:?}").contains("synthetic-once"));
    }

    #[tokio::test]
    async fn an_oversized_or_truncated_frame_is_refused_without_allocating_the_claimed_size() {
        let (mut writer, mut reader) = tokio::io::duplex(32);
        writer.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        assert!(read_envelope(&mut reader, deadline).await.is_err());
        let (mut writer, mut reader) = tokio::io::duplex(32);
        writer
            .write_all(&u32::try_from(MIN_SEALED_BYTES).unwrap().to_be_bytes())
            .await
            .unwrap();
        writer.write_all(&[0, 0]).await.unwrap();
        drop(writer);
        assert!(read_envelope(&mut reader, deadline).await.is_err());
    }

    #[tokio::test]
    async fn a_peer_that_stalls_before_the_header_is_stopped_at_the_absolute_deadline() {
        let (_writer, mut reader) = tokio::io::duplex(32);
        let deadline = Instant::now() + Duration::from_millis(100);
        assert!(matches!(
            read_envelope(&mut reader, deadline).await,
            Err(DriverError::Timeout)
        ));
    }

    #[test]
    fn an_encrypted_command_with_unknown_fields_is_refused() {
        let key = GuardKey::from_bytes([7; 32]);
        let bytes = br#"{"operation":"inspect","pod_uid":"uid-1","unexpected":true}"#;
        let envelope = seal_request(&key, "uid-1", 1, bytes).unwrap();
        let mut guard = ReplayGuard::new("uid-1", key).unwrap();
        assert!(guard.begin(1, &envelope).is_err());
    }
}
