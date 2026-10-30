//! A bounded, Pod-bound header for the trusted tool-staging transfer.
//! The header is not authentication: the caller must protect the exec channel and Pod identity.

#![cfg_attr(
    not(target_os = "linux"),
    allow(
        dead_code,
        reason = "the stage runs on Linux; receipts keep its header everywhere"
    )
)]

use std::io::{self, Write};

use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::ToolRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

use crate::staging::{ArchiveReceipt, validate_receipt};

/// Enough for many named tool versions without letting an untrusted peer allocate an arbitrary header.
const MAX_HEADER_BYTES: usize = 256 * 1024;
/// A 64 KiB buffer keeps memory fixed without making large tool transfers use tiny writes.
const TRANSFER_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StageHeader {
    pub pod_uid: String,
    pub receipt: ArchiveReceipt,
    #[serde(deserialize_with = "strict_tools")]
    pub tools: Vec<ToolRef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictTool {
    name: String,
    version: String,
}

fn strict_tools<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ToolRef>, D::Error> {
    Vec::<StrictTool>::deserialize(deserializer).map(|tools| {
        tools
            .into_iter()
            .map(|tool| ToolRef {
                name: tool.name,
                version: tool.version,
            })
            .collect()
    })
}

impl StageHeader {
    /// The expected UID comes from the stager's own Pod identity, not the transfer.
    pub(crate) fn check(&self, expected_uid: &str) -> Result<(), DriverError> {
        if expected_uid.trim().is_empty() || self.pod_uid != expected_uid {
            return Err(DriverError::InvalidSpec(
                "the staging transfer names another Pod".into(),
            ));
        }
        validate_receipt(&self.receipt, &self.tools)
    }
}

struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let size = self
            .0
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("staging header too large"))?;
        if size > MAX_HEADER_BYTES {
            return Err(io::Error::other("staging header too large"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn unavailable() -> DriverError {
    DriverError::Unavailable("the staging header could not be exchanged".into())
}

pub(crate) async fn write_header<W: AsyncWrite + Unpin>(
    output: &mut W,
    header: &StageHeader,
    expected_uid: &str,
    deadline: Instant,
) -> Result<(), DriverError> {
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    header.check(expected_uid)?;
    let mut bytes = Bounded(Vec::new());
    serde_json::to_writer(&mut bytes, header).map_err(|_| unavailable())?;
    let length = u32::try_from(bytes.0.len()).map_err(|_| unavailable())?;
    tokio::time::timeout_at(deadline, async {
        output.write_all(&length.to_be_bytes()).await?;
        output.write_all(&bytes.0).await?;
        output.flush().await
    })
    .await
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| unavailable())?;
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok(())
}

pub(crate) async fn read_header<R: AsyncRead + Unpin>(
    input: &mut R,
    expected_uid: &str,
    deadline: Instant,
) -> Result<StageHeader, DriverError> {
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    let bytes = tokio::time::timeout_at(deadline, async {
        let mut length = [0_u8; 4];
        input
            .read_exact(&mut length)
            .await
            .map_err(|_| unavailable())?;
        let length = usize::try_from(u32::from_be_bytes(length)).map_err(|_| unavailable())?;
        if length == 0 || length > MAX_HEADER_BYTES {
            return Err(unavailable());
        }
        let mut bytes = vec![0; length];
        input
            .read_exact(&mut bytes)
            .await
            .map_err(|_| unavailable())?;
        Ok(bytes)
    })
    .await
    .map_err(|_| DriverError::Timeout)??;
    let header: StageHeader = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    header.check(expected_uid)?;
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok(header)
}

/// Copy exactly the announced archive, with a stall limit and one absolute deadline.
/// The stream carries one archive and is closed afterwards, not reused for another command.
/// On any error the caller must discard the partial, still-private output.
pub(crate) async fn transfer_archive<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    input: &mut R,
    output: &mut W,
    header: &StageHeader,
    expected_uid: &str,
    stall_timeout: std::time::Duration,
    deadline: Instant,
) -> Result<(), DriverError> {
    if Instant::now() >= deadline || stall_timeout.is_zero() {
        return Err(DriverError::Timeout);
    }
    header.check(expected_uid)?;
    let mut remaining = header.receipt.bytes;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; TRANSFER_CHUNK_BYTES];
    while remaining > 0 {
        let limit = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
            .map_err(|_| unavailable())?;
        let count = tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(stall_timeout, input.read(&mut buffer[..limit])),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())?;
        if count == 0 {
            return Err(unavailable());
        }
        digest.update(&buffer[..count]);
        tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(stall_timeout, output.write_all(&buffer[..count])),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())?;
        remaining -= u64::try_from(count).map_err(|_| unavailable())?;
    }
    if hex::encode(digest.finalize()) != header.receipt.sha256 {
        return Err(unavailable());
    }
    tokio::time::timeout_at(
        deadline,
        tokio::time::timeout(stall_timeout, output.flush()),
    )
    .await
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| unavailable())?;
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn header() -> StageHeader {
        StageHeader {
            pod_uid: "pod-uid".into(),
            receipt: ArchiveReceipt {
                sha256: "a".repeat(64),
                bytes: 1024,
            },
            tools: vec![ToolRef {
                name: "codex".into(),
                version: "1.0.0".into(),
            }],
        }
    }

    #[tokio::test]
    async fn a_transfer_header_round_trips_and_is_bound_to_the_expected_pod() {
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        let deadline = Instant::now() + Duration::from_secs(5);
        write_header(&mut writer, &header(), "pod-uid", deadline)
            .await
            .unwrap();
        assert_eq!(
            read_header(&mut reader, "pod-uid", deadline).await.unwrap(),
            header()
        );
        assert!(header().check("other-pod").is_err());
        let mut invalid = header();
        invalid.receipt.sha256 = "not-a-digest".into();
        assert!(invalid.check("pod-uid").is_err());
    }

    #[tokio::test]
    async fn oversized_or_truncated_headers_are_refused_under_the_deadline() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        writer.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        assert!(read_header(&mut reader, "pod-uid", deadline).await.is_err());
        let (mut writer, mut reader) = tokio::io::duplex(16);
        writer.write_all(&10_u32.to_be_bytes()).await.unwrap();
        writer.write_all(b"short").await.unwrap();
        drop(writer);
        assert!(read_header(&mut reader, "pod-uid", deadline).await.is_err());
        let (mut writer, _reader) = tokio::io::duplex(16);
        assert!(matches!(
            write_header(&mut writer, &header(), "pod-uid", Instant::now()).await,
            Err(DriverError::Timeout)
        ));
        let mut unexpected = serde_json::to_value(header()).unwrap();
        unexpected["tools"][0]["unexpected"] = serde_json::json!(true);
        let bytes = serde_json::to_vec(&unexpected).unwrap();
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        writer
            .write_all(&u32::try_from(bytes.len()).unwrap().to_be_bytes())
            .await
            .unwrap();
        writer.write_all(&bytes).await.unwrap();
        assert!(read_header(&mut reader, "pod-uid", deadline).await.is_err());
    }

    #[tokio::test]
    async fn the_archive_transfer_checks_length_digest_and_deadline() {
        let data = b"fixture archive";
        let mut header = header();
        header.receipt.bytes = u64::try_from(data.len()).unwrap();
        header.receipt.sha256 = hex::encode(Sha256::digest(data));
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut sender, mut input) = tokio::io::duplex(4096);
        sender.write_all(data).await.unwrap();
        let (mut output, mut receiver) = tokio::io::duplex(4096);
        transfer_archive(
            &mut input,
            &mut output,
            &header,
            "pod-uid",
            Duration::from_secs(1),
            deadline,
        )
        .await
        .unwrap();
        let mut copied = vec![0; data.len()];
        receiver.read_exact(&mut copied).await.unwrap();
        assert_eq!(copied.as_slice(), &data[..]);

        let (mut sender, mut input) = tokio::io::duplex(4096);
        sender.write_all(b"fixture archivE").await.unwrap();
        assert!(
            transfer_archive(
                &mut input,
                &mut tokio::io::sink(),
                &header,
                "pod-uid",
                Duration::from_secs(1),
                deadline
            )
            .await
            .is_err()
        );
        let (mut sender, mut input) = tokio::io::duplex(4096);
        sender.write_all(b"short").await.unwrap();
        drop(sender);
        assert!(
            transfer_archive(
                &mut input,
                &mut tokio::io::sink(),
                &header,
                "pod-uid",
                Duration::from_secs(1),
                deadline
            )
            .await
            .is_err()
        );
        assert!(matches!(
            transfer_archive(
                &mut input,
                &mut tokio::io::sink(),
                &header,
                "pod-uid",
                Duration::from_secs(1),
                Instant::now()
            )
            .await,
            Err(DriverError::Timeout)
        ));
    }
}
