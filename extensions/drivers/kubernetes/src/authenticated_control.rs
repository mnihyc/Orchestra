//! Protect guard messages independently of the short-lived exec relay.
//!
//! The key must be delivered while no untrusted workload exists, kept private
//! by the node and guard, and bound to one verified Pod and guard incarnation.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use orch_driver_contract::driver::DriverError;
use orch_types::Hash;
use rand::{TryRng, rngs::SysRng};
use sha2::{Digest, Sha256};

use crate::control_protocol::{GuardReply, GuardRequest, MAX_FRAME_BYTES};

/// Separates these envelopes from other uses of the same primitive.
const DOMAIN: &[u8] = b"orch-kubernetes-guard-v1\0";
const KEY_BYTES: usize = 32;
pub(crate) const NONCE_BYTES: usize = 24;
/// XChaCha20-Poly1305 appends one 16-byte authentication tag.
pub(crate) const TAG_BYTES: usize = 16;

/// A guard session key. It has no serialization or public debug representation.
#[derive(Clone)]
pub(crate) struct GuardKey([u8; KEY_BYTES]);

impl std::fmt::Debug for GuardKey {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("GuardKey([secret])")
    }
}

impl GuardKey {
    /// A key generated and persisted by the node before the first untrusted start.
    pub(crate) fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(bytes)
    }

    /// A fresh key; the caller must durably protect it before bootstrap.
    pub(crate) fn generate() -> Result<Self, DriverError> {
        Ok(Self(random_bytes()?))
    }

    /// The node persists this value privately and sends it only during protected bootstrap.
    pub(crate) fn expose(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }
}

/// Opaque bytes can cross an exec relay without revealing or changing a request.
#[derive(Clone)]
pub(crate) struct Envelope {
    pub nonce: [u8; NONCE_BYTES],
    pub ciphertext: Vec<u8>,
}

impl std::fmt::Debug for Envelope {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Envelope([sealed])")
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Request,
    Response,
}

fn invalid() -> DriverError {
    DriverError::Unavailable(
        "the Kubernetes unit helper could not authenticate its control message".into(),
    )
}

fn random_bytes<const N: usize>() -> Result<[u8; N], DriverError> {
    let mut bytes = [0; N];
    SysRng.try_fill_bytes(&mut bytes).map_err(|_| invalid())?;
    Ok(bytes)
}

fn digest(request: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(&Sha256::digest(request));
    out
}

fn associated_data(
    direction: Direction,
    pod_uid: &str,
    sequence: u64,
    request_hash: Option<[u8; 32]>,
) -> Result<Vec<u8>, DriverError> {
    if pod_uid.is_empty() || sequence == 0 {
        return Err(invalid());
    }
    let uid_len = u64::try_from(pod_uid.len()).map_err(|_| invalid())?;
    let mut data = Vec::with_capacity(DOMAIN.len() + 1 + 8 + pod_uid.len() + 8 + 32);
    data.extend_from_slice(DOMAIN);
    data.push(match direction {
        Direction::Request => 0,
        Direction::Response => 1,
    });
    data.extend_from_slice(&uid_len.to_be_bytes());
    data.extend_from_slice(pod_uid.as_bytes());
    data.extend_from_slice(&sequence.to_be_bytes());
    data.extend_from_slice(&request_hash.unwrap_or([0; 32]));
    Ok(data)
}

fn seal(
    key: &GuardKey,
    direction: Direction,
    pod_uid: &str,
    sequence: u64,
    request_hash: Option<[u8; 32]>,
    plaintext: &[u8],
) -> Result<Envelope, DriverError> {
    if plaintext.is_empty() || plaintext.len() > MAX_FRAME_BYTES {
        return Err(invalid());
    }
    let aad = associated_data(direction, pod_uid, sequence, request_hash)?;
    // A fresh 192-bit nonce lets an exact retry use the same sequence without nonce reuse.
    let nonce: [u8; NONCE_BYTES] = random_bytes()?;
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| invalid())?;
    Ok(Envelope { nonce, ciphertext })
}

fn open(
    key: &GuardKey,
    direction: Direction,
    pod_uid: &str,
    sequence: u64,
    request_hash: Option<[u8; 32]>,
    envelope: &Envelope,
) -> Result<Vec<u8>, DriverError> {
    if envelope.ciphertext.len() <= TAG_BYTES
        || envelope.ciphertext.len() > MAX_FRAME_BYTES + TAG_BYTES
    {
        return Err(invalid());
    }
    let aad = associated_data(direction, pod_uid, sequence, request_hash)?;
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    cipher
        .decrypt(
            &XNonce::from(envelope.nonce),
            Payload {
                msg: &envelope.ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| invalid())
}

/// Seals exact request bytes before they cross the untrusted relay.
pub(crate) fn seal_request(
    key: &GuardKey,
    pod_uid: &str,
    sequence: u64,
    request: &[u8],
) -> Result<Envelope, DriverError> {
    seal(key, Direction::Request, pod_uid, sequence, None, request)
}

/// Opens a reply only for the exact request and Pod that produced it.
pub(crate) struct VerifiedReply {
    pod_uid: String,
    sequence: u64,
    request_hash: Hash,
    key_hash: Hash,
    reply: GuardReply,
}

impl VerifiedReply {
    /// The observation or refusal the authenticated guard actually returned.
    pub(crate) fn response(&self) -> &GuardReply {
        &self.reply
    }

    /// Binds completion to the same Pod, key, sequence and request kept in the journal.
    pub(crate) fn matches(
        &self,
        pod_uid: &str,
        sequence: u64,
        request: &[u8],
        key: &[u8; KEY_BYTES],
    ) -> bool {
        self.pod_uid == pod_uid
            && self.sequence == sequence
            && self.request_hash == Hash::of(request)
            && self.key_hash == Hash::of(key)
    }
}

pub(crate) fn open_response(
    key: &GuardKey,
    pod_uid: &str,
    sequence: u64,
    request: &[u8],
    envelope: &Envelope,
) -> Result<VerifiedReply, DriverError> {
    let bytes = open(
        key,
        Direction::Response,
        pod_uid,
        sequence,
        Some(digest(request)),
        envelope,
    )?;
    let reply = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    Ok(VerifiedReply {
        pod_uid: pod_uid.to_owned(),
        sequence,
        request_hash: Hash::of(request),
        key_hash: Hash::of(key.expose()),
        reply,
    })
}

/// The plaintext request is retained only to bind its eventual response; do not log it.
pub(crate) enum Admission {
    Execute {
        command: GuardRequest,
        request: Vec<u8>,
    },
    Retry {
        request: Vec<u8>,
    },
}

/// The guard serializes its operations through this state for the lifetime of one Pod.
pub(crate) struct ReplayGuard {
    pod_uid: String,
    key: GuardKey,
    last: Option<(u64, [u8; 32], GuardReply)>,
    pending: Option<(u64, [u8; 32])>,
}

impl ReplayGuard {
    /// `pod_uid` comes from the guard's own Pod identity, never from the relay.
    pub(crate) fn new(pod_uid: &str, key: GuardKey) -> Result<Self, DriverError> {
        if pod_uid.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            pod_uid: pod_uid.to_owned(),
            key,
            last: None,
            pending: None,
        })
    }

    /// Old requests are refused; the exact last request may receive its saved result.
    pub(crate) fn begin(
        &mut self,
        sequence: u64,
        envelope: &Envelope,
    ) -> Result<Admission, DriverError> {
        let request = open(
            &self.key,
            Direction::Request,
            &self.pod_uid,
            sequence,
            None,
            envelope,
        )?;
        let command: GuardRequest = serde_json::from_slice(&request).map_err(|_| invalid())?;
        if command.pod_uid() != self.pod_uid {
            return Err(invalid());
        }
        if self.pending.is_some() {
            return Err(DriverError::Busy(
                "the unit helper is completing a command".into(),
            ));
        }
        let hash = digest(&request);
        if let Some((last, previous, _)) = &self.last {
            if sequence < *last || sequence == *last && hash != *previous {
                return Err(invalid());
            }
            if sequence == *last {
                return Ok(Admission::Retry { request });
            }
            if last.checked_add(1) != Some(sequence) {
                return Err(invalid());
            }
        } else if sequence != 1 {
            return Err(invalid());
        }
        self.pending = Some((sequence, hash));
        Ok(Admission::Execute { command, request })
    }

    /// Save the terminal reply before accepting another command. An interrupted pending command stays fenced.
    pub(crate) fn finish(
        &mut self,
        sequence: u64,
        request: &[u8],
        reply: GuardReply,
    ) -> Result<(), DriverError> {
        let hash = digest(request);
        if self.pending != Some((sequence, hash)) {
            return Err(invalid());
        }
        self.last = Some((sequence, hash, reply));
        self.pending = None;
        Ok(())
    }

    /// Seals only the saved result of the exact completed command.
    pub(crate) fn sealed_reply(
        &self,
        sequence: u64,
        request: &[u8],
    ) -> Result<Envelope, DriverError> {
        if self.pending.is_some() {
            return Err(DriverError::Busy(
                "the unit helper is completing a command".into(),
            ));
        }
        let (last, previous, reply) = self.last.as_ref().ok_or_else(invalid)?;
        if sequence != *last || digest(request) != *previous {
            return Err(invalid());
        }
        let bytes = serde_json::to_vec(reply).map_err(|_| invalid())?;
        seal(
            &self.key,
            Direction::Response,
            &self.pod_uid,
            sequence,
            Some(*previous),
            &bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_protocol::{GuardPhase, GuardRefusal};

    #[test]
    fn a_reply_is_bound_to_the_exact_unit_sequence_request_and_direction() {
        let key = GuardKey::from_bytes([7; KEY_BYTES]);
        assert!(!format!("{key:?}").contains("7, 7"));
        let reply = GuardReply::State {
            phase: GuardPhase::Stopped,
        };
        let bytes = serde_json::to_vec(&reply).unwrap();
        let envelope = seal(
            &key,
            Direction::Response,
            "uid-1",
            9,
            Some(digest(b"stop")),
            &bytes,
        )
        .unwrap();
        assert_eq!(
            open_response(&key, "uid-1", 9, b"stop", &envelope)
                .unwrap()
                .response(),
            &reply
        );
        assert!(open_response(&key, "uid-2", 9, b"stop", &envelope).is_err());
        assert!(open_response(&key, "uid-1", 10, b"stop", &envelope).is_err());
        assert!(open_response(&key, "uid-1", 9, b"start", &envelope).is_err());
        assert!(open(&key, Direction::Request, "uid-1", 9, None, &envelope).is_err());
        let mut tampered = envelope;
        tampered.ciphertext[0] ^= 1;
        assert!(open_response(&key, "uid-1", 9, b"stop", &tampered).is_err());
    }

    #[test]
    fn a_relay_cannot_read_the_token_or_replay_an_old_preparation() {
        let key = GuardKey::from_bytes([7; KEY_BYTES]);
        let mut guard = ReplayGuard::new("uid-1", key.clone()).unwrap();
        let prepared = serde_json::to_vec(&GuardRequest::Prepare {
            pod_uid: "uid-1".into(),
            ca: "test-ca".into(),
            enrollment: "synthetic-once".into(),
            environment: "{\"TOKEN\":\"synthetic-env-secret\"}".into(),
            mounts: None,
        })
        .unwrap();
        let envelope = seal_request(&key, "uid-1", 1, &prepared).unwrap();
        assert!(
            !envelope
                .ciphertext
                .windows(b"synthetic-once".len())
                .any(|part| part == b"synthetic-once")
        );
        assert!(
            !envelope
                .ciphertext
                .windows(b"synthetic-env-secret".len())
                .any(|part| part == b"synthetic-env-secret")
        );
        assert!(matches!(
            guard.begin(1, &envelope),
            Ok(Admission::Execute {
                command: GuardRequest::Prepare { .. },
                ..
            })
        ));
        assert!(matches!(
            guard.begin(1, &envelope),
            Err(DriverError::Busy(_))
        ));
        let reply = GuardReply::State {
            phase: GuardPhase::Stopped,
        };
        guard.finish(1, &prepared, reply.clone()).unwrap();
        let sealed = guard.sealed_reply(1, &prepared).unwrap();
        assert_eq!(
            open_response(&key, "uid-1", 1, &prepared, &sealed)
                .unwrap()
                .response(),
            &reply
        );
        assert!(matches!(
            guard.begin(1, &envelope),
            Ok(Admission::Retry { .. })
        ));
        let changed = serde_json::to_vec(&GuardRequest::Inspect {
            pod_uid: "uid-1".into(),
        })
        .unwrap();
        let reused = seal_request(&key, "uid-1", 1, &changed).unwrap();
        assert!(guard.begin(1, &reused).is_err());
        let wrong = serde_json::to_vec(&GuardRequest::Inspect {
            pod_uid: "uid-2".into(),
        })
        .unwrap();
        let wrong_envelope = seal_request(&key, "uid-1", 2, &wrong).unwrap();
        assert!(guard.begin(2, &wrong_envelope).is_err());
        let next = seal_request(&key, "uid-1", 2, &changed).unwrap();
        assert!(matches!(
            guard.begin(2, &next),
            Ok(Admission::Execute {
                command: GuardRequest::Inspect { .. },
                ..
            })
        ));
        assert!(matches!(
            guard.sealed_reply(1, &prepared),
            Err(DriverError::Busy(_))
        ));
        assert!(
            guard
                .finish(
                    2,
                    &prepared,
                    GuardReply::Refused {
                        reason: GuardRefusal::Invalid
                    }
                )
                .is_err()
        );
        assert!(matches!(guard.begin(2, &next), Err(DriverError::Busy(_))));
    }

    #[test]
    fn an_old_start_cannot_be_replayed_as_a_fresh_observation() {
        let key = GuardKey::from_bytes([9; KEY_BYTES]);
        let mut guard = ReplayGuard::new("uid-1", key.clone()).unwrap();
        let start = serde_json::to_vec(&GuardRequest::Start {
            pod_uid: "uid-1".into(),
        })
        .unwrap();
        let envelope = seal_request(&key, "uid-1", 1, &start).unwrap();
        let skipped = seal_request(&key, "uid-1", 2, &start).unwrap();
        assert!(guard.begin(2, &skipped).is_err());
        assert!(matches!(
            guard.begin(1, &envelope),
            Ok(Admission::Execute {
                command: GuardRequest::Start { .. },
                ..
            })
        ));
        guard
            .finish(
                1,
                &start,
                GuardReply::State {
                    phase: GuardPhase::Running,
                },
            )
            .unwrap();
        let inspect = serde_json::to_vec(&GuardRequest::Inspect {
            pod_uid: "uid-1".into(),
        })
        .unwrap();
        let skipped = seal_request(&key, "uid-1", 3, &inspect).unwrap();
        assert!(guard.begin(3, &skipped).is_err());
        let observation = seal_request(&key, "uid-1", 2, &inspect).unwrap();
        assert!(matches!(
            guard.begin(2, &observation),
            Ok(Admission::Execute { .. })
        ));
        guard
            .finish(
                2,
                &inspect,
                GuardReply::State {
                    phase: GuardPhase::Stopped,
                },
            )
            .unwrap();
        assert!(guard.begin(1, &envelope).is_err());
    }
}
