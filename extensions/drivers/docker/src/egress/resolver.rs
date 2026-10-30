//! The filtered resolver of a restricted unit: it answers the addresses of the names the unit may reach, looked up by a query of its own, and refuses every other question.
//!
//! The unit's own lookups come here (the daemon's resolver inside the unit forwards to this one alone). A
//! question for a name on neither list is refused without anything being asked of anybody, so a name cannot carry
//! data out through the host's resolvers. For a name that is admitted the resolver makes its own lookup of the
//! canonical name and answers address records alone: no other type, no option and no additional record of the
//! question is passed on. An other name's addresses that its policy refuses are left out of the answer.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;

use super::admission::canonical_name;
use super::proxy::Gate;

/// How long an answer may be kept by whoever asked: short, so that a changed list is soon what the unit sees.
const TTL_SECS: u32 = 30;

/// The largest question read and answer sent: what resolvers agree on without fragments.
const PACKET_MAX: usize = 1232;

/// How long one lookup may take.
const LOOKUP_WAIT: Duration = Duration::from_secs(5);

/// The type of an IPv4 address record.
const TYPE_A: u16 = 1;

/// The type of an IPv6 address record.
const TYPE_AAAA: u16 = 28;

/// The class of the Internet.
const CLASS_IN: u16 = 1;

/// The longest name, in octets of the question.
const NAME_OCTETS_MAX: usize = 255;

/// The longest label.
const LABEL_MAX: usize = 63;

/// The answer codes the resolver gives.
pub mod rcode {
    /// The answer, possibly empty.
    pub const NO_ERROR: u8 = 0;
    /// The question could not be read.
    pub const FORMAT_ERROR: u8 = 1;
    /// The lookup failed.
    pub const SERVER_FAILURE: u8 = 2;
    /// The name has no address.
    pub const NAME_ERROR: u8 = 3;
    /// The name is not one the unit may ask for.
    pub const REFUSED: u8 = 5;
}

/// One question, as read from a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The query's identifier.
    pub id: u16,
    /// Whether recursion was asked for.
    pub recursion_desired: bool,
    /// The name, lower case, without a final dot.
    pub name: String,
    /// The type asked for.
    pub qtype: u16,
    /// The class asked for.
    pub qclass: u16,
    /// The question section as it was asked, which the answer repeats.
    pub section: Vec<u8>,
}

fn word(packet: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*packet.get(at)?, *packet.get(at + 1)?]))
}

/// Reads the one question of a standard query; none for anything else (an answer, another operation, several
/// questions, a compressed or malformed name).
#[must_use]
pub fn read_question(packet: &[u8]) -> Option<Question> {
    let id = word(packet, 0)?;
    let flags = word(packet, 2)?;
    let is_response = flags & 0x8000 != 0;
    let opcode = (flags >> 11) & 0x0f;
    if is_response || opcode != 0 || word(packet, 4)? != 1 {
        return None;
    }
    let mut at = 12;
    let mut labels: Vec<String> = Vec::new();
    loop {
        let length = usize::from(*packet.get(at)?);
        at += 1;
        if length == 0 {
            break;
        }
        // A pointer, or a label longer than labels are: no question of a resolver has either.
        if length > LABEL_MAX {
            return None;
        }
        let label = packet.get(at..at + length)?;
        if !label.iter().all(u8::is_ascii_graphic) {
            return None;
        }
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        at += length;
        if at - 12 > NAME_OCTETS_MAX {
            return None;
        }
    }
    let qtype = word(packet, at)?;
    let qclass = word(packet, at + 2)?;
    let end = at + 4;
    Some(Question {
        id,
        recursion_desired: flags & 0x0100 != 0,
        name: labels.join("."),
        qtype,
        qclass,
        section: packet.get(12..end)?.to_vec(),
    })
}

/// The answer to `question` with the code `code` and the address records of `addresses` that are of the type
/// asked for.
#[must_use]
pub fn answer(question: &Question, code: u8, addresses: &[IpAddr]) -> Vec<u8> {
    let records: Vec<&IpAddr> = addresses
        .iter()
        .filter(|address| match address {
            IpAddr::V4(_) => question.qtype == TYPE_A,
            IpAddr::V6(_) => question.qtype == TYPE_AAAA,
        })
        .collect();
    // A response, recursion available, the asker's own wish for it, and the code.
    let mut flags: u16 = 0x8000 | 0x0080 | u16::from(code & 0x0f);
    if question.recursion_desired {
        flags |= 0x0100;
    }
    let count = u16::try_from(records.len()).unwrap_or(u16::MAX);
    let mut out = Vec::with_capacity(PACKET_MAX);
    out.extend_from_slice(&question.id.to_be_bytes());
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&1_u16.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&question.section);
    for address in records {
        // The name, by a pointer to the question's, which starts right after the header.
        out.extend_from_slice(&0xc00c_u16.to_be_bytes());
        let (kind, data): (u16, Vec<u8>) = match address {
            IpAddr::V4(v4) => (TYPE_A, v4.octets().to_vec()),
            IpAddr::V6(v6) => (TYPE_AAAA, v6.octets().to_vec()),
        };
        out.extend_from_slice(&kind.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out.extend_from_slice(&TTL_SECS.to_be_bytes());
        let length = u16::try_from(data.len()).unwrap_or(0);
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(&data);
        if out.len() > PACKET_MAX {
            break;
        }
    }
    out
}

/// What the resolver answers to `question` under the policy in force: the code and the addresses.
pub async fn resolve(gate: &Gate, question: &Question) -> (u8, Vec<IpAddr>) {
    let Some(admission) = gate.current() else {
        return (rcode::REFUSED, Vec::new());
    };
    let Ok(name) = canonical_name(&question.name) else {
        return (rcode::REFUSED, Vec::new());
    };
    if question.qclass != CLASS_IN || !admission.names(&name) {
        return (rcode::REFUSED, Vec::new());
    }
    if question.qtype != TYPE_A && question.qtype != TYPE_AAAA {
        return (rcode::NO_ERROR, Vec::new());
    }
    let trusted = admission
        .always
        .iter()
        .any(|transport| transport.authority == name);
    let found: Vec<SocketAddr> = match tokio::time::timeout(
        LOOKUP_WAIT,
        tokio::net::lookup_host((name.as_str(), 0)),
    )
    .await
    {
        Ok(Ok(found)) => found.collect(),
        Ok(Err(_)) => return (rcode::NAME_ERROR, Vec::new()),
        Err(_) => return (rcode::SERVER_FAILURE, Vec::new()),
    };
    let addresses: Vec<IpAddr> = found
        .into_iter()
        .map(|address| address.ip())
        .filter(|address| trusted || !admission.refuses(*address))
        .collect();
    (rcode::NO_ERROR, addresses)
}

/// Serves the resolver on `socket` for as long as the program runs.
pub async fn serve(socket: Arc<UdpSocket>, gate: Arc<Gate>) {
    let mut packet = vec![0_u8; PACKET_MAX];
    loop {
        let (length, from) = match socket.recv_from(&mut packet).await {
            Ok(received) => received,
            Err(error) => {
                tracing::debug!(%error, "the resolver could not read a question");
                continue;
            }
        };
        let Some(question) = read_question(&packet[..length]) else {
            // A query that cannot be read is told so when at least its identifier can be.
            if let Some(id) = word(&packet[..length], 0) {
                let refused = Question {
                    id,
                    recursion_desired: false,
                    name: String::new(),
                    qtype: 0,
                    qclass: 0,
                    section: Vec::new(),
                };
                let mut out = answer(&refused, rcode::FORMAT_ERROR, &[]);
                // No question is repeated.
                out[4..6].copy_from_slice(&0_u16.to_be_bytes());
                let _ = socket.send_to(&out, from).await;
            }
            continue;
        };
        let socket = socket.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            let (code, addresses) = resolve(&gate, &question).await;
            let out = answer(&question, code, &addresses);
            if let Err(error) = socket.send_to(&out, from).await {
                tracing::debug!(%error, "the resolver could not answer");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::admission::Admission;

    /// A query for `name` of type `qtype`, with recursion desired and an option record after it.
    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut out = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 1];
        for label in name.split('.') {
            out.push(u8::try_from(label.len()).unwrap());
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0);
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        // An option record of the asker's, which is never passed on.
        out.extend_from_slice(&[0, 0, 41, 4, 208, 0, 0, 0, 0, 0, 0]);
        out
    }

    #[test]
    fn a_question_is_read_in_lower_case_and_its_options_are_left_behind() {
        let question = read_question(&query("PyPI.org", TYPE_A)).unwrap();
        assert_eq!(question.id, 0x1234);
        assert!(question.recursion_desired);
        assert_eq!(question.name, "pypi.org");
        assert_eq!(question.qtype, TYPE_A);
        assert_eq!(question.section.len(), 1 + 4 + 1 + 3 + 1 + 4);
    }

    #[test]
    fn answers_pointers_and_several_questions_are_not_read() {
        let mut response = query("pypi.org", TYPE_A);
        response[2] |= 0x80;
        assert!(read_question(&response).is_none());
        let mut two = query("pypi.org", TYPE_A);
        two[5] = 2;
        assert!(read_question(&two).is_none());
        let mut pointer = query("pypi.org", TYPE_A);
        pointer[12] = 0xc0;
        assert!(read_question(&pointer).is_none());
        assert!(read_question(&[0x12]).is_none());
    }

    #[test]
    fn an_answer_holds_the_records_of_the_type_asked_and_nothing_else() {
        let question = read_question(&query("pypi.org", TYPE_A)).unwrap();
        let addresses: Vec<IpAddr> = vec![
            "151.101.0.223".parse().unwrap(),
            "2a04:4e42::223".parse().unwrap(),
        ];
        let out = answer(&question, rcode::NO_ERROR, &addresses);
        assert_eq!(&out[0..2], &[0x12, 0x34]);
        assert_eq!(word(&out, 2).unwrap() & 0x8000, 0x8000, "a response");
        assert_eq!(word(&out, 6), Some(1), "one A record and no AAAA");
        assert_eq!(word(&out, 10), Some(0), "no additional record");
        assert_eq!(&out[out.len() - 4..], &[151, 101, 0, 223]);
        let refused = answer(&question, rcode::REFUSED, &[]);
        assert_eq!(word(&refused, 2).unwrap() & 0x0f, u16::from(rcode::REFUSED));
        assert_eq!(word(&refused, 6), Some(0));
    }

    #[tokio::test]
    async fn a_name_on_neither_list_is_refused_and_a_closed_gate_answers_nothing() {
        let gate = Gate::new();
        let question = read_question(&query("pypi.org", TYPE_A)).unwrap();
        assert_eq!(
            resolve(&gate, &question).await,
            (rcode::REFUSED, Vec::new())
        );
        gate.set(Some(Admission {
            allowed_hosts: vec!["crates.io".into()],
            ..Admission::default()
        }));
        assert_eq!(
            resolve(&gate, &question).await,
            (rcode::REFUSED, Vec::new())
        );
        let other_type = read_question(&query("crates.io", 16)).unwrap();
        assert_eq!(
            resolve(&gate, &other_type).await,
            (rcode::NO_ERROR, Vec::new())
        );
    }
}
