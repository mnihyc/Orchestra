//! Two-phase acknowledgement for a private staged tree.
//!
//! The Kubernetes API connection and the Pod/container identity checks remain
//! the caller's responsibility. This challenge only enforces message ordering.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use orch_driver_contract::driver::DriverError;
use rand::{TryRng, rngs::SysRng};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

use crate::stage_protocol::StageHeader;
use crate::stage_worker;

// The original header allows 256 KiB; the challenge and framing need less than 1 KiB more.
const MAX_FRAME: usize = 257 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedReceipt {
    pub header: StageHeader,
    pub challenge: [u8; 32],
}

struct Bounded(Vec<u8>);

impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .0
            .len()
            .checked_add(bytes.len())
            .is_none_or(|size| size > MAX_FRAME)
        {
            return Err(io::Error::other("staging acknowledgement too large"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn unavailable() -> DriverError {
    DriverError::Unavailable("the staged transfer could not be acknowledged".into())
}

async fn write_frame<W: AsyncWrite + Unpin>(
    output: &mut W,
    value: &PreparedReceipt,
    budget: Duration,
    deadline: Instant,
) -> Result<(), DriverError> {
    if budget.is_zero() || Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    let mut bytes = Bounded(Vec::new());
    serde_json::to_writer(&mut bytes, value).map_err(|_| unavailable())?;
    let length = u32::try_from(bytes.0.len()).map_err(|_| unavailable())?;
    tokio::time::timeout_at(
        deadline,
        tokio::time::timeout(budget, async {
            output.write_all(&length.to_be_bytes()).await?;
            output.write_all(&bytes.0).await?;
            output.flush().await
        }),
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

async fn read_frame<R: AsyncRead + Unpin>(
    input: &mut R,
    budget: Duration,
    deadline: Instant,
) -> Result<PreparedReceipt, DriverError> {
    if budget.is_zero() || Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    let bytes = tokio::time::timeout_at(
        deadline,
        tokio::time::timeout(budget, async {
            let mut length = [0_u8; 4];
            input
                .read_exact(&mut length)
                .await
                .map_err(|_| unavailable())?;
            let length = usize::try_from(u32::from_be_bytes(length)).map_err(|_| unavailable())?;
            if length == 0 || length > MAX_FRAME {
                return Err(unavailable());
            }
            let mut bytes = vec![0; length];
            input
                .read_exact(&mut bytes)
                .await
                .map_err(|_| unavailable())?;
            Ok(bytes)
        }),
    )
    .await
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| DriverError::Timeout)??;
    let receipt = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok(receipt)
}

/// Receiver half: no final marker is made until the node echoes the fresh challenge
/// and the exact prepared header. A disconnect or mismatched release leaves the tree private.
pub(crate) async fn receive<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    root: &Path,
    supervisor: &Path,
    pod_uid: &str,
    input: &mut R,
    output: &mut W,
    stall: Duration,
    deadline: Instant,
) -> Result<StageHeader, DriverError> {
    let prepared =
        stage_worker::prepare_transfer(root, supervisor, pod_uid, input, stall, deadline).await?;
    let mut challenge = [0_u8; 32];
    SysRng
        .try_fill_bytes(&mut challenge)
        .map_err(|_| unavailable())?;
    let receipt = PreparedReceipt {
        header: prepared.header().clone(),
        challenge,
    };
    write_frame(output, &receipt, stall, deadline).await?;
    let release = read_frame(input, stall, deadline).await?;
    if release != receipt {
        return Err(unavailable());
    }
    prepared.release(&release.header, deadline)
}

/// Node half: call only over the verified original Pod's exec stream. Its caller
/// must recheck Pod, worker and init identities after receiving this receipt and
/// persist the expected digest before invoking `release`.
pub(crate) async fn prepared<R: AsyncRead + Unpin>(
    input: &mut R,
    expected: &StageHeader,
    pod_uid: &str,
    budget: Duration,
    deadline: Instant,
) -> Result<PreparedReceipt, DriverError> {
    expected.check(pod_uid)?;
    let value = read_frame(input, budget, deadline).await?;
    if &value.header != expected {
        return Err(unavailable());
    }
    Ok(value)
}

/// The exact received receipt includes the challenge, so a release cannot be sent
/// before the receiver prepared its verified tree.
pub(crate) async fn release<W: AsyncWrite + Unpin>(
    output: &mut W,
    prepared: &PreparedReceipt,
    budget: Duration,
    deadline: Instant,
) -> Result<(), DriverError> {
    write_frame(output, prepared, budget, deadline).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use tokio::io::AsyncWriteExt;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, Vec<u8>, StageHeader) {
        let parent = tempfile::tempdir().unwrap();
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let cache = parent.path().join("cache");
        let connector = cache.join("orch-connector").join("fixture");
        fs::create_dir_all(&connector).unwrap();
        for path in [
            cache.clone(),
            cache.join("orch-connector"),
            connector.clone(),
        ] {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(connector.join(".complete"), format!("{}\n", "a".repeat(64))).unwrap();
        fs::set_permissions(
            connector.join(".complete"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        fs::write(connector.join("orch-connector"), b"synthetic connector").unwrap();
        fs::set_permissions(
            connector.join("orch-connector"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let mut archive = Vec::new();
        let receipt = crate::staging::archive(
            &cache,
            &connector,
            &[],
            &mut archive,
            std::time::Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        let header = StageHeader {
            pod_uid: "fixture-pod".into(),
            receipt,
            tools: Vec::new(),
        };
        let encoded = serde_json::to_vec(&header).unwrap();
        let mut stream = u32::try_from(encoded.len()).unwrap().to_be_bytes().to_vec();
        stream.extend(encoded);
        stream.extend(archive);
        let root = parent.path().join("stage");
        fs::create_dir(&root).unwrap();
        stage_worker::prepare(&root).unwrap();
        let supervisor = parent.path().join("supervisor");
        fs::write(&supervisor, b"synthetic supervisor").unwrap();
        fs::set_permissions(&supervisor, fs::Permissions::from_mode(0o755)).unwrap();
        (parent, root, supervisor, stream, header)
    }

    #[tokio::test]
    async fn init_cannot_finish_until_the_node_releases_the_exact_prepared_receipt() {
        let (_parent, root, supervisor, stream, header) = fixture();
        let (mut node_input, mut helper_input) = tokio::io::duplex(65536);
        let (mut helper_output, mut node_output) = tokio::io::duplex(65536);
        let deadline = Instant::now() + Duration::from_secs(10);
        let helper_root = root.clone();
        let helper = tokio::spawn(async move {
            receive(
                &helper_root,
                &supervisor,
                "fixture-pod",
                &mut helper_input,
                &mut helper_output,
                Duration::from_secs(3),
                deadline,
            )
            .await
        });
        node_input.write_all(&stream).await.unwrap();
        let receipt = prepared(
            &mut node_output,
            &header,
            "fixture-pod",
            Duration::from_secs(3),
            deadline,
        )
        .await
        .unwrap();
        assert!(stage_worker::ready(&root, "fixture-pod").unwrap().is_none());
        assert!(!helper.is_finished());
        release(&mut node_input, &receipt, Duration::from_secs(3), deadline)
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout_at(deadline, helper)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            header
        );
        assert_eq!(
            stage_worker::ready(&root, "fixture-pod").unwrap(),
            Some(header)
        );
    }

    #[tokio::test]
    async fn an_altered_challenge_or_disconnect_cannot_release_the_staged_tree() {
        for altered in [true, false] {
            let (_parent, root, supervisor, stream, header) = fixture();
            let (mut node_input, mut helper_input) = tokio::io::duplex(65536);
            let (mut helper_output, mut node_output) = tokio::io::duplex(65536);
            let deadline = Instant::now() + Duration::from_secs(10);
            let helper_root = root.clone();
            let helper = tokio::spawn(async move {
                receive(
                    &helper_root,
                    &supervisor,
                    "fixture-pod",
                    &mut helper_input,
                    &mut helper_output,
                    Duration::from_secs(3),
                    deadline,
                )
                .await
            });
            node_input.write_all(&stream).await.unwrap();
            let mut receipt = prepared(
                &mut node_output,
                &header,
                "fixture-pod",
                Duration::from_secs(3),
                deadline,
            )
            .await
            .unwrap();
            if altered {
                receipt.challenge[0] ^= 1;
                release(&mut node_input, &receipt, Duration::from_secs(3), deadline)
                    .await
                    .unwrap();
            } else {
                drop(node_input);
            }
            assert!(
                tokio::time::timeout_at(deadline, helper)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            assert!(stage_worker::ready(&root, "fixture-pod").unwrap().is_none());
            assert!(!root.join("bin").exists());
        }
    }
}
