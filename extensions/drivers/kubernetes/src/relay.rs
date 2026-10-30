//! An untrusted exec process relays only authenticated ciphertext to the guard.

#![cfg(target_os = "linux")]

use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::Path;

use orch_driver_contract::driver::DriverError;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;
use tokio::time::Instant;

use crate::encrypted_transport::{read_envelope, write_envelope};

fn unavailable() -> DriverError {
    DriverError::Unavailable("the unit control relay is unavailable".into())
}

/// Only the guard's private directory and its own socket can be used as the relay destination.
fn check_socket(path: &Path) -> Result<(), DriverError> {
    if !path.is_absolute() {
        return Err(unavailable());
    }
    let parent = path.parent().ok_or_else(unavailable)?;
    let directory = std::fs::symlink_metadata(parent).map_err(|_| unavailable())?;
    let socket = std::fs::symlink_metadata(path).map_err(|_| unavailable())?;
    let uid = rustix::process::geteuid().as_raw();
    if !directory.is_dir()
        || directory.uid() != uid
        || directory.permissions().mode() & 0o077 != 0
        || !socket.file_type().is_socket()
        || socket.uid() != uid
        || socket.permissions().mode() & 0o077 != 0
    {
        return Err(unavailable());
    }
    Ok(())
}

/// A compromised relay can delay or corrupt ciphertext, never authenticate a reply or read a secret.
pub(crate) async fn relay<I: AsyncRead + Unpin, O: AsyncWrite + Unpin>(
    socket: &Path,
    input: &mut I,
    output: &mut O,
    deadline: Instant,
) -> Result<(), DriverError> {
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    check_socket(socket)?;
    let (sequence, request) = read_envelope(input, deadline).await?;
    let mut guard = tokio::time::timeout_at(deadline, UnixStream::connect(socket))
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())?;
    write_envelope(&mut guard, sequence, &request, deadline).await?;
    let (reply_sequence, reply) = read_envelope(&mut guard, deadline).await?;
    if reply_sequence != sequence {
        return Err(unavailable());
    }
    write_envelope(output, sequence, &reply, deadline).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authenticated_control::{
        Admission, GuardKey, ReplayGuard, open_response, seal_request,
    };
    use crate::control_protocol::{GuardPhase, GuardReply, GuardRequest};
    use std::time::Duration;
    use tokio::net::UnixListener;

    fn private_directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }

    #[tokio::test]
    async fn a_relay_moves_ciphertext_but_only_the_guard_can_authenticate_the_reply() {
        let directory = private_directory();
        let path = directory.path().join("control.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let key = GuardKey::from_bytes([7; 32]);
        let server_key = key.clone();
        let deadline = Instant::now() + Duration::from_secs(5);
        let server = tokio::spawn(async move {
            let (mut peer, _) = tokio::time::timeout_at(deadline, listener.accept())
                .await
                .unwrap()
                .unwrap();
            let (sequence, envelope) = read_envelope(&mut peer, deadline).await.unwrap();
            let mut guard = ReplayGuard::new("pod-uid", server_key).unwrap();
            let Admission::Execute { request, .. } = guard.begin(sequence, &envelope).unwrap()
            else {
                panic!("first command")
            };
            guard
                .finish(
                    sequence,
                    &request,
                    GuardReply::State {
                        phase: GuardPhase::Stopped,
                    },
                )
                .unwrap();
            let reply = guard.sealed_reply(sequence, &request).unwrap();
            write_envelope(&mut peer, sequence, &reply, deadline)
                .await
                .unwrap();
        });
        let (mut client_input, mut relay_input) = tokio::io::duplex(4096);
        let (mut relay_output, mut client_output) = tokio::io::duplex(4096);
        let relay_path = path.clone();
        let task = tokio::spawn(async move {
            relay(&relay_path, &mut relay_input, &mut relay_output, deadline).await
        });
        let request = serde_json::to_vec(&GuardRequest::Inspect {
            pod_uid: "pod-uid".into(),
        })
        .unwrap();
        let envelope = seal_request(&key, "pod-uid", 1, &request).unwrap();
        write_envelope(&mut client_input, 1, &envelope, deadline)
            .await
            .unwrap();
        let (sequence, reply) = read_envelope(&mut client_output, deadline).await.unwrap();
        assert_eq!(sequence, 1);
        assert_eq!(
            open_response(&key, "pod-uid", sequence, &request, &reply)
                .unwrap()
                .response(),
            &GuardReply::State {
                phase: GuardPhase::Stopped
            }
        );
        task.await.unwrap().unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_untrusted_socket_or_an_expired_deadline_never_relays_a_command() {
        let directory = private_directory();
        let path = directory.path().join("control.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        let (_writer, mut input) = tokio::io::duplex(16);
        let (mut output, _reader) = tokio::io::duplex(16);
        assert!(
            relay(
                &path,
                &mut input,
                &mut output,
                Instant::now() + Duration::from_secs(5)
            )
            .await
            .is_err()
        );
        assert!(matches!(
            relay(&path, &mut input, &mut output, Instant::now()).await,
            Err(DriverError::Timeout)
        ));
    }

    #[tokio::test]
    async fn a_reply_with_another_sequence_is_not_forwarded() {
        use tokio::io::AsyncReadExt;
        let directory = private_directory();
        let path = directory.path().join("control.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let server = tokio::spawn(async move {
            let (mut peer, _) = tokio::time::timeout_at(deadline, listener.accept())
                .await
                .unwrap()
                .unwrap();
            let (sequence, envelope) = read_envelope(&mut peer, deadline).await.unwrap();
            write_envelope(&mut peer, sequence + 1, &envelope, deadline)
                .await
                .unwrap();
        });
        let (mut client_input, mut relay_input) = tokio::io::duplex(4096);
        let (mut relay_output, mut client_output) = tokio::io::duplex(4096);
        let task = tokio::spawn(async move {
            relay(&path, &mut relay_input, &mut relay_output, deadline).await
        });
        let key = GuardKey::from_bytes([8; 32]);
        let request = serde_json::to_vec(&GuardRequest::Inspect {
            pod_uid: "pod-uid".into(),
        })
        .unwrap();
        let envelope = seal_request(&key, "pod-uid", 1, &request).unwrap();
        write_envelope(&mut client_input, 1, &envelope, deadline)
            .await
            .unwrap();
        assert!(task.await.unwrap().is_err());
        let mut byte = [0_u8; 1];
        assert_eq!(client_output.read(&mut byte).await.unwrap(), 0);
        server.await.unwrap();
    }
}
