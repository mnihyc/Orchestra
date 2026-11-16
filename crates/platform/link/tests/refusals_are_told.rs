//! A frame the session cannot carry is never dropped in silence: where the answer to a command, a command, an
//! event or a report is sent, what the session says about a refusal is read, and somebody is told.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::path::Path;

/// The sends of a frame that may be larger than the session carries, written so that their answer is thrown
/// away. A refusal is something to tell: the answer to a command is replaced by one that says why, a command
/// is failed, an event is reported.
const DISCARDS: &[&str] = &[
    "let _ = session.send(",
    "let _ = session.send_events(",
    "let _ = open.send_events(",
    "let _ = sender.send(Frame::from(",
    "let _ = sender.send(Frame::PortConnectionsReply",
];

/// Where those sends are made: the machine's answers and events, and the gateway's way into a session.
const PLACES: &[&str] = &[
    "src/commands.rs",
    "src/machine.rs",
    "../../control/gateway/src/routing.rs",
];

#[test]
fn no_send_of_a_message_that_may_be_too_large_throws_its_refusal_away() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for place in PLACES {
        let path = manifest.join(place);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
        for (number, line) in source.lines().enumerate() {
            for discard in DISCARDS {
                assert!(
                    !line.contains(discard),
                    "{}:{} throws a refusal away: {}",
                    place,
                    number + 1,
                    line.trim()
                );
            }
        }
    }
}
