//! Secrets that only one machine reads, and the tickets that allow a tunnel.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use chrono::Duration;
use common::{authority, clock, machine};
use orch_link_contract::tunnels::{OpenErrorCode, Service, TicketSource};
use orch_pki::certificates::LeafKey;
use orch_pki::sealing::{RecipientKey, info_for_command, open, seal};
use orch_pki::tickets::{TicketRequest, TicketSigner, TicketVerifier};
use orch_types::{Clock, new_id};

#[test]
fn a_secret_sealed_to_a_machine_is_opened_by_that_machine() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, mine) = machine(&ca, &clock);
    let command = new_id();
    let sealed = seal(
        &RecipientKey::from_certificate(&mine.chain[0]),
        &info_for_command(command),
        b"sk-live-0123456789",
    )
    .unwrap();
    assert!(!sealed.contains("sk-live"));
    let opened = open(&mine.key, &info_for_command(command), &sealed).unwrap();
    assert_eq!(opened, b"sk-live-0123456789");
}

#[test]
fn a_sealed_secret_is_unreadable_with_any_other_machines_key() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, mine) = machine(&ca, &clock);
    let (_, other) = machine(&ca, &clock);
    let command = new_id();
    let sealed = seal(
        &RecipientKey::from_certificate(&mine.chain[0]),
        &info_for_command(command),
        b"secret",
    )
    .unwrap();
    assert!(open(&other.key, &info_for_command(command), &sealed).is_err());
    assert!(
        open(
            &LeafKey::generate().unwrap(),
            &info_for_command(command),
            &sealed
        )
        .is_err()
    );
}

#[test]
fn a_sealed_secret_is_bound_to_the_command_it_was_made_for() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, mine) = machine(&ca, &clock);
    let sealed = seal(
        &RecipientKey::from_certificate(&mine.chain[0]),
        &info_for_command(new_id()),
        b"secret",
    )
    .unwrap();
    assert!(
        open(&mine.key, &info_for_command(new_id()), &sealed).is_err(),
        "lifted into another command"
    );
}

#[test]
fn a_sealed_secret_that_was_altered_does_not_open() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, mine) = machine(&ca, &clock);
    let command = new_id();
    let info = info_for_command(command);
    let sealed = seal(
        &RecipientKey::from_certificate(&mine.chain[0]),
        &info,
        b"secret",
    )
    .unwrap();
    let mut bytes = sealed.clone().into_bytes();
    let last = bytes.len() - 6;
    bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
    assert!(open(&mine.key, &info, &String::from_utf8(bytes).unwrap()).is_err());
    assert!(open(&mine.key, &info, "").is_err());
    assert!(open(&mine.key, &info, "AAAA").is_err());
}

#[test]
fn sealing_twice_gives_two_different_values() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, mine) = machine(&ca, &clock);
    let recipient = RecipientKey::from_certificate(&mine.chain[0]);
    let info = info_for_command(new_id());
    assert_ne!(
        seal(&recipient, &info, b"x").unwrap(),
        seal(&recipient, &info, b"x").unwrap()
    );
}

#[test]
fn a_machine_can_seal_to_another_given_only_its_public_key_as_the_system_writes_it() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, sender) = machine(&ca, &clock);
    let (_, recipient) = machine(&ca, &clock);
    let written = RecipientKey::from_certificate(&recipient.chain[0]).to_text();
    let info = info_for_command(new_id());
    let sealed = seal(
        &RecipientKey::parse(&written).unwrap(),
        &info,
        b"handed over",
    )
    .unwrap();
    assert_eq!(
        open(&recipient.key, &info, &sealed).unwrap(),
        b"handed over"
    );
    assert!(open(&sender.key, &info, &sealed).is_err());
    assert!(RecipientKey::parse("not a key").is_err());
}

fn ticket_request(signer_src: TicketSource, expires_at: orch_types::Time) -> TicketRequest {
    TicketRequest {
        src: signer_src,
        dst: new_id(),
        service: Service::Exec {
            executor: "codex-exec-server".to_owned(),
        },
        thread_id: Some(new_id()),
        tool_set: None,
        expires_at,
    }
}

#[test]
fn a_ticket_the_system_signed_is_good_for_the_machine_it_names_until_it_expires() {
    let clock = clock();
    let signer = TicketSigner::generate();
    let verifier = TicketVerifier::from_text(&signer.public_key_text()).unwrap();
    let opener = new_id();
    let request = ticket_request(
        TicketSource::Machine(opener),
        (clock.now() + Duration::hours(1)).into(),
    );
    let ticket = signer.issue(&request).unwrap();
    let claims = verifier
        .verify(&ticket, TicketSource::Machine(opener), clock.time())
        .unwrap();
    assert_eq!(claims.dst, request.dst);
    assert_eq!(claims.service, "exec/codex-exec-server");
    assert_eq!(claims.kind, request.service.kind());
    let later = (clock.now() + Duration::hours(2)).into();
    assert_eq!(
        verifier.verify(&ticket, TicketSource::Machine(opener), later),
        Err(OpenErrorCode::Expired)
    );
}

#[test]
fn a_ticket_is_good_for_nobody_but_the_machine_it_names() {
    let clock = clock();
    let signer = TicketSigner::generate();
    let verifier = signer.verifier();
    let opener = new_id();
    let ticket = signer
        .issue(&ticket_request(
            TicketSource::Machine(opener),
            (clock.now() + Duration::hours(1)).into(),
        ))
        .unwrap();
    assert_eq!(
        verifier.verify(&ticket, TicketSource::Machine(new_id()), clock.time()),
        Err(OpenErrorCode::BadTicket)
    );
    assert_eq!(
        verifier.verify(&ticket, TicketSource::Api, clock.time()),
        Err(OpenErrorCode::BadTicket)
    );
}

#[test]
fn a_ticket_for_the_api_to_open_names_the_api() {
    let clock = clock();
    let signer = TicketSigner::generate();
    let ticket = signer
        .issue(&ticket_request(
            TicketSource::Api,
            (clock.now() + Duration::hours(1)).into(),
        ))
        .unwrap();
    signer
        .verifier()
        .verify(&ticket, TicketSource::Api, clock.time())
        .unwrap();
}

#[test]
fn a_ticket_another_key_signed_is_a_bad_ticket_even_when_it_is_also_late() {
    let clock = clock();
    let forger = TicketSigner::generate();
    let system = TicketSigner::generate();
    let opener = new_id();
    let long_ago = (clock.now() - Duration::days(1)).into();
    let ticket = forger
        .issue(&ticket_request(TicketSource::Machine(opener), long_ago))
        .unwrap();
    assert_eq!(
        system
            .verifier()
            .verify(&ticket, TicketSource::Machine(opener), clock.time()),
        Err(OpenErrorCode::BadTicket),
        "only a ticket that is the system's can be told to be late"
    );
}

#[test]
fn a_ticket_whose_claims_were_edited_is_a_bad_ticket() {
    let clock = clock();
    let signer = TicketSigner::generate();
    let opener = new_id();
    let ticket = signer
        .issue(&ticket_request(
            TicketSource::Machine(opener),
            (clock.now() + Duration::hours(1)).into(),
        ))
        .unwrap();
    let mut parts: Vec<String> = ticket.split('.').map(str::to_owned).collect();
    let other = signer
        .issue(&ticket_request(
            TicketSource::Machine(opener),
            (clock.now() + Duration::days(30)).into(),
        ))
        .unwrap();
    parts[1] = other.split('.').nth(1).unwrap().to_owned();
    let spliced = parts.join(".");
    assert_eq!(
        signer
            .verifier()
            .verify(&spliced, TicketSource::Machine(opener), clock.time()),
        Err(OpenErrorCode::BadTicket)
    );
    for garbage in ["", "v1", "v1.e30", "v1.e30.c2ln", "v2.e30.c2ln", "a.b.c.d"] {
        assert_eq!(
            signer
                .verifier()
                .verify(garbage, TicketSource::Machine(opener), clock.time()),
            Err(OpenErrorCode::BadTicket),
            "{garbage}"
        );
    }
}
