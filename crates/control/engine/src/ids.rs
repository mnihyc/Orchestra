//! The identifiers of the commands the engine sends.
//!
//! A command asked for twice under one identifier is made once, however
//! many times and from whichever process it is asked. The engine therefore
//! never invents the identifier of a command it must not repeat: it derives
//! it from the things the command is about, so that a process that starts
//! again, or a second one, asks for the same command under the same
//! identifier and nothing is sent twice.

use orch_types::{Hash, Id};

/// How many bytes an identifier has.
const ID_BYTES: usize = 16;
/// The byte of an identifier that holds its version, and the one that holds
/// its variant.
const VERSION_BYTE: usize = 6;
const VARIANT_BYTE: usize = 8;
/// The version and variant bits of an identifier made by hashing (version 8
/// of the standard, which is for exactly this).
const VERSION_8: u8 = 0x80;
const VERSION_MASK: u8 = 0x0F;
const VARIANT_BITS: u8 = 0x80;
const VARIANT_MASK: u8 = 0x3F;

/// The identifier derived from a word that says what the command is and the
/// things it is about, in order.
#[must_use]
pub fn derived(word: &str, parts: &[&[u8]]) -> Id {
    let mut hasher = Hash::hasher();
    hasher.update(b"orch-engine-command\0");
    hasher.update(word.as_bytes());
    for part in parts {
        // A part's length leads it, so that two lists of parts that join to
        // the same bytes are not the same list.
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    let digest = Hash::finish(&hasher);
    let mut bytes = [0u8; ID_BYTES];
    bytes.copy_from_slice(&digest.as_bytes()[..ID_BYTES]);
    bytes[VERSION_BYTE] = (bytes[VERSION_BYTE] & VERSION_MASK) | VERSION_8;
    bytes[VARIANT_BYTE] = (bytes[VARIANT_BYTE] & VARIANT_MASK) | VARIANT_BITS;
    Id::from_bytes(bytes)
}

fn of(word: &str, ids: &[Id]) -> Id {
    let parts: Vec<&[u8]> = ids.iter().map(|id| id.as_bytes().as_slice()).collect();
    derived(word, &parts)
}

/// `turn.start` of a turn, the `attempt`th time it is asked (a second time
/// after the session had to be opened again).
#[must_use]
pub fn turn_start(turn: Id, attempt: u32) -> Id {
    derived(
        "turn.start",
        &[turn.as_bytes().as_slice(), &attempt.to_le_bytes()],
    )
}

/// `turn.steer` for an input.
#[must_use]
pub fn turn_steer(input: Id) -> Id {
    of("turn.steer", &[input])
}

/// `turn.interrupt` of a turn.
#[must_use]
pub fn turn_interrupt(turn: Id) -> Id {
    of("turn.interrupt", &[turn])
}

/// `approval.resolve` for an approval, the `attempt`th time it is sent.
#[must_use]
pub fn approval_resolve(approval: Id, attempt: u32) -> Id {
    derived(
        "approval.resolve",
        &[approval.as_bytes().as_slice(), &attempt.to_le_bytes()],
    )
}

/// `session.open` for a segment, the `attempt`th time it must be opened (a
/// number that is the same in a process that starts again).
#[must_use]
pub fn session_open(segment: Id, attempt: u32) -> Id {
    derived(
        "session.open",
        &[segment.as_bytes().as_slice(), &attempt.to_le_bytes()],
    )
}

/// `agent.start` for the turn that needs the harness.
#[must_use]
pub fn agent_start(turn: Id) -> Id {
    of("agent.start", &[turn])
}

/// `fs.mkdir` of the directory a turn's thread works in.
#[must_use]
pub fn thread_directory(turn: Id) -> Id {
    of("fs.mkdir.thread", &[turn])
}

/// Verification of the instruction-root directory for this turn's session-open attempt.
#[must_use]
pub fn instruction_root(turn: Id, attempt: u32) -> Id {
    derived(
        "fs.mkdir.instruction-root",
        &[turn.as_bytes().as_slice(), &attempt.to_le_bytes()],
    )
}

/// `session.close` after the forced end of a turn.
#[must_use]
pub fn session_close(turn: Id) -> Id {
    of("session.close", &[turn])
}

/// `session.resync` of a segment whose session was adopted: asked once when it is first opened.
#[must_use]
pub fn session_resync(segment: Id) -> Id {
    of("session.resync", &[segment])
}

/// `lease.open` for a lease, the `attempt`th time it is asked (a second time after the provider had to be
/// started again because its machine no longer held it).
#[must_use]
pub fn lease_open(lease: Id, attempt: u32) -> Id {
    derived(
        "lease.open",
        &[lease.as_bytes().as_slice(), &attempt.to_le_bytes()],
    )
}

/// `provider.start` of a provider on a machine, for the configuration `digest` stands for, the `attempt`th
/// time it is asked: a start that failed is not repeated under its own identifier, which would answer with
/// the failure again, and one that succeeded is never asked for again while the configuration is the same.
#[must_use]
pub fn provider_start(provider: Id, machine: Id, digest: &str, attempt: u32) -> Id {
    derived(
        "provider.start",
        &[
            provider.as_bytes().as_slice(),
            machine.as_bytes().as_slice(),
            digest.as_bytes(),
            &attempt.to_le_bytes(),
        ],
    )
}

/// `provider.start` asked again for the lease that found the machine without the provider.
#[must_use]
pub fn provider_restart(provider: Id, machine: Id, digest: &str, lease: Id) -> Id {
    derived(
        "provider.restart",
        &[
            provider.as_bytes().as_slice(),
            machine.as_bytes().as_slice(),
            digest.as_bytes(),
            lease.as_bytes().as_slice(),
        ],
    )
}

/// `provider.stop` of a provider on a machine, for the act (a revision of the provider) that switched it off.
#[must_use]
pub fn provider_stop(provider: Id, machine: Id, revision: i64) -> Id {
    derived(
        "provider.stop",
        &[
            provider.as_bytes().as_slice(),
            machine.as_bytes().as_slice(),
            &revision.to_le_bytes(),
        ],
    )
}

/// The row of the usage ledger for what a lease's forwarder counted beyond the ledger when the lease
/// closed: one for a lease, however often it is closed.
#[must_use]
pub fn usage_close(lease: Id) -> Id {
    derived("usage.close", &[lease.as_bytes().as_slice()])
}

/// The row of the usage ledger for one model request of a lease that the upstream named: a request that is
/// reported twice makes one row.
#[must_use]
pub fn usage_request(lease: Id, request_ref: &str) -> Id {
    derived(
        "usage.request",
        &[lease.as_bytes().as_slice(), request_ref.as_bytes()],
    )
}

/// The row of the usage ledger for one event of a provider's stream, for a request the upstream did not
/// name: the event is the identity, and an event that is delivered twice makes one row.
#[must_use]
pub fn usage_event(machine: Id, stream: &str, seq: i64) -> Id {
    derived(
        "usage.event",
        &[
            machine.as_bytes().as_slice(),
            stream.as_bytes(),
            &seq.to_le_bytes(),
        ],
    )
}

/// The row of the usage ledger for the total a harness reported for a turn that went through no forwarder:
/// a turn has one, however many times it is looked at.
#[must_use]
pub fn harness_usage(turn: Id) -> Id {
    of("usage.harness", &[turn])
}

/// `lease.close` for a lease.
#[must_use]
pub fn lease_close(lease: Id) -> Id {
    of("lease.close", &[lease])
}

/// `orch.tool_result` for a call of a thread's agent.
#[must_use]
pub fn tool_result(thread: Id, call_ref: &str) -> Id {
    derived(
        "orch.tool_result",
        &[thread.as_bytes().as_slice(), call_ref.as_bytes()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn the_same_things_give_the_same_identifier_and_other_things_another() {
        let (turn, other) = (new_id(), new_id());
        assert_eq!(turn_start(turn, 0), turn_start(turn, 0));
        assert_ne!(turn_start(turn, 0), turn_start(other, 0));
        assert_ne!(turn_start(turn, 0), turn_start(turn, 1));
        assert_ne!(turn_start(turn, 0), turn_interrupt(turn));
        assert_ne!(session_open(turn, 1), session_open(turn, 2));
    }

    #[test]
    fn a_provider_is_started_again_only_when_it_changed_or_a_start_failed() {
        let (provider, machine) = (new_id(), new_id());
        assert_eq!(
            provider_start(provider, machine, "a", 0),
            provider_start(provider, machine, "a", 0)
        );
        assert_ne!(
            provider_start(provider, machine, "a", 0),
            provider_start(provider, machine, "b", 0)
        );
        assert_ne!(
            provider_start(provider, machine, "a", 0),
            provider_start(provider, machine, "a", 1)
        );
        assert_ne!(
            provider_stop(provider, machine, 3),
            provider_stop(provider, machine, 4)
        );
    }

    #[test]
    fn a_use_is_one_row_however_often_it_is_reported() {
        let (lease, machine) = (new_id(), new_id());
        assert_eq!(
            usage_request(lease, "resp_1"),
            usage_request(lease, "resp_1")
        );
        assert_ne!(
            usage_request(lease, "resp_1"),
            usage_request(lease, "resp_2")
        );
        assert_eq!(
            usage_event(machine, "provider:p", 7),
            usage_event(machine, "provider:p", 7)
        );
        assert_ne!(
            usage_event(machine, "provider:p", 7),
            usage_event(machine, "provider:p", 8)
        );
    }

    #[test]
    fn parts_that_join_to_the_same_bytes_are_not_the_same_command() {
        assert_ne!(
            derived("w", &[b"ab".as_slice(), b"c".as_slice()]),
            derived("w", &[b"a".as_slice(), b"bc".as_slice()])
        );
    }

    #[test]
    fn a_derived_identifier_is_one_the_rest_of_the_system_reads() {
        let id = turn_start(new_id(), 0);
        assert_eq!(id.get_version_num(), 8);
        assert_eq!(id.to_string().parse::<Id>().unwrap(), id);
    }
}
