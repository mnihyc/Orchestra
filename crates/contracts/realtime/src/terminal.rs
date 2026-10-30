//! Raw terminal data and control frames; a shell taken up on another screen, and the replay it is taken up with.
//!
//! An interactive shell, and the agent's own terminal, each have a
//! WebSocket of their own, apart from the realtime socket, because they
//! carry raw bytes at interactive latency. It speaks the subprotocol
//! [`TERMINAL_SUBPROTOCOL`] and is authenticated by the single-use ticket
//! in its address; no other credential is read.
//!
//! Bytes travel in binary frames, in both directions: what is typed goes
//! to the shell (an agent terminal's view-only ticket may not type), and
//! what the shell produced comes back. Everything else is a text frame,
//! one JSON object with an `op`: [`TerminalClientFrame`] and
//! [`TerminalServerFrame`]. Output is not redacted and is no part of any
//! timeline.
//!
//! On attach the server first sends the last bytes the shell printed
//! ([`REPLAY_BYTES`] at most), so that a shell that is taken up again shows
//! its screen as it is; [`Replay`] tells those bytes from live output. A
//! shell has one screen at a time: attaching with a new ticket ends an
//! attachment elsewhere, which is told so and loses nothing, as the shell
//! itself is untouched. [`Backpressure`] is the rule by which the server
//! stops reading a shell's output when the browser has fallen behind.
//!
//! On a slow link a shell is slow, and says so: what is typed is sent at
//! once and is drawn when the shell echoes it, and the socket never draws
//! a key by itself. The client measures the round trip with `ping` and
//! `pong`, as for the realtime socket ([`RoundTrips`](crate::version::RoundTrips)).
//!
//! A view-only agent ticket ignores input bytes and resize frames, and still answers ping.
//! Malformed JSON, an unknown operation or member, or a member of the wrong type closes only
//! that attachment with 4400; its shell keeps running. A well-formed size outside the accepted
//! dimensions is ignored. Binary frames on this separate socket are terminal bytes, not JSON.

use crate::frames::{FrameFault, decode_text};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use crate::version::TERMINAL_SUBPROTOCOL;

/// The most bytes of a shell's last output that a server replays when a
/// screen takes the shell up: as many as the connector keeps for as long
/// as the shell runs, and as many as are kept when it closes.
pub const REPLAY_BYTES: i64 = 256 * 1024;
/// How many bytes the browser may be behind before the server stops
/// reading from the environment's tunnel: a mebibyte, some five seconds of
/// output at the rate of a slow link and little enough to hold in memory
/// for every open shell.
pub const PAUSE_AT_BYTES: i64 = 1024 * 1024;
/// How few bytes the browser has to be behind for the server to read from
/// the tunnel again: a quarter of the mark above, so that the reading does
/// not stop and start with every packet.
pub const RESUME_AT_BYTES: i64 = 256 * 1024;

/// A text frame from the browser to the shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TerminalClientFrame {
    /// The screen has a new size.
    Resize(TerminalResizeFrame),
    /// A question whether the line is alive, and how long it takes.
    Ping(TerminalPingFrame),
}

/// A text frame from the shell to the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TerminalServerFrame {
    /// The shell is attached: its size, and how many bytes of replay follow.
    Ready(TerminalReadyFrame),
    /// The process of the shell ended.
    Exit(TerminalExitFrame),
    /// The socket is closed, and why.
    Closed(TerminalClosedFrame),
    /// The answer to a ping.
    Pong(TerminalPongFrame),
}

/// The new size of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct TerminalResizeFrame {
    /// The width, in columns.
    pub cols: u16,
    /// The height, in rows.
    pub rows: u16,
}

/// A question whether the line is alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct TerminalPingFrame {
    /// A number the answer repeats, from which the client measures the
    /// round trip.
    pub n: i64,
}

/// The word that a shell is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct TerminalReadyFrame {
    /// The width of the screen, in columns.
    pub cols: u16,
    /// The height of the screen, in rows.
    pub rows: u16,
    /// How many bytes of the shell's last output follow at once, before
    /// anything it prints now: no more than [`REPLAY_BYTES`].
    pub replay_bytes: i64,
}

/// The end of the shell's process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct TerminalExitFrame {
    /// What the process exited with.
    pub code: i64,
}

/// The word that the socket is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct TerminalClosedFrame {
    /// Why.
    pub reason: TerminalClosedReason,
}

/// The answer to a ping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct TerminalPongFrame {
    /// The number the ping carried.
    pub n: i64,
}

/// Why a terminal's socket was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalClosedReason {
    /// The shell's process ended.
    Exited,
    /// Its person closed it.
    ClosedByUser,
    /// Nobody was attached for as long as shells are kept.
    Detached,
    /// The environment it ran in was stopped.
    EnvironmentStopped,
    /// The connector of its environment was restarted under it.
    LinkLost,
    /// Its person took it up on another screen.
    AttachedElsewhere,
    /// Its opener lost the permission, or a condition under which it was
    /// opened stopped holding.
    Revoked,
}

impl TerminalClosedReason {
    /// Every reason.
    pub const ALL: [TerminalClosedReason; 7] = [
        Self::Exited,
        Self::ClosedByUser,
        Self::Detached,
        Self::EnvironmentStopped,
        Self::LinkLost,
        Self::AttachedElsewhere,
        Self::Revoked,
    ];

    /// Whether the shell itself ended. Only an attachment that was taken
    /// over by another screen leaves the shell running.
    #[must_use]
    pub const fn ends_the_shell(self) -> bool {
        !matches!(self, Self::AttachedElsewhere)
    }
}

impl TerminalClientFrame {
    /// Every `op` a client may send.
    pub const OPS: [&'static str; 2] = ["resize", "ping"];

    /// Reads a text frame of the browser.
    pub fn decode(text: &str) -> Result<Self, FrameFault> {
        decode_text(text, &Self::OPS, None)
    }
}

impl TerminalServerFrame {
    /// Every `op` the server sends.
    pub const OPS: [&'static str; 4] = ["ready", "exit", "closed", "pong"];

    /// Reads a text frame of the server.
    pub fn decode(text: &str) -> Result<Self, FrameFault> {
        decode_text(text, &Self::OPS, None)
    }
}

/// How many of the bytes a shell sends are still its replay.
///
/// The server sends `ready`, then `replay_bytes` bytes, then whatever the
/// shell prints from then on, in binary frames whose borders have no
/// meaning. A client that must not treat the replay as news (a bell, a
/// notification, a count of unread output) keeps one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Replay {
    remaining: i64,
}

impl Replay {
    /// The replay that `ready` announced.
    #[must_use]
    pub const fn announced(ready: &TerminalReadyFrame) -> Self {
        Self {
            remaining: if ready.replay_bytes > 0 {
                ready.replay_bytes
            } else {
                0
            },
        }
    }

    /// How many bytes of the replay have yet to arrive.
    #[must_use]
    pub const fn remaining(&self) -> i64 {
        self.remaining
    }

    /// A binary frame of `len` bytes arrived: how many of them belong to
    /// the replay, and how many are live output.
    pub fn split(&mut self, len: i64) -> (i64, i64) {
        let replayed = len.clamp(0, self.remaining);
        self.remaining -= replayed;
        (replayed, len.max(0) - replayed)
    }
}

/// What to do with the environment's tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flow {
    /// Stop reading from it: the browser is far behind.
    Pause,
    /// Read from it again.
    Resume,
}

/// Whether the server reads a shell's output from the environment's tunnel,
/// given how far the browser is behind.
///
/// It stops when the browser is [`PAUSE_AT_BYTES`] behind and reads again
/// when it is [`RESUME_AT_BYTES`] or less behind. In between it does what
/// it was doing, so that it does not switch at every packet; and nothing
/// that was read is dropped, which is why the bytes of a shell come out
/// as the shell sent them, however slow the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Backpressure {
    paused: bool,
}

impl Backpressure {
    /// Reading.
    #[must_use]
    pub const fn new() -> Self {
        Self { paused: false }
    }

    /// Whether reading is stopped.
    #[must_use]
    pub const fn paused(&self) -> bool {
        self.paused
    }

    /// The browser is `behind` bytes behind. `Some` when that changes
    /// whether the server reads.
    pub fn update(&mut self, behind: i64) -> Option<Flow> {
        if !self.paused && behind >= PAUSE_AT_BYTES {
            self.paused = true;
            Some(Flow::Pause)
        } else if self.paused && behind <= RESUME_AT_BYTES {
            self.paused = false;
            Some(Flow::Resume)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_the_browser_sends_as_text_is_a_resize_or_a_ping() {
        let resize =
            TerminalClientFrame::decode(r#"{"op":"resize","cols":120,"rows":32}"#).unwrap();
        assert_eq!(
            resize,
            TerminalClientFrame::Resize(TerminalResizeFrame {
                cols: 120,
                rows: 32
            })
        );
        let ping = TerminalClientFrame::decode(r#"{"op":"ping","n":7}"#).unwrap();
        assert_eq!(ping, TerminalClientFrame::Ping(TerminalPingFrame { n: 7 }));
        assert_eq!(
            serde_json::to_value(resize).unwrap(),
            json!({ "op": "resize", "cols": 120, "rows": 32 })
        );
        assert_eq!(
            serde_json::to_value(ping).unwrap(),
            json!({ "op": "ping", "n": 7 })
        );
    }

    #[test]
    fn what_the_server_sends_as_text_is_ready_exit_closed_and_pong() {
        let frames = [
            (
                json!({ "op": "ready", "cols": 120, "rows": 32, "replay_bytes": 262_144 }),
                TerminalServerFrame::Ready(TerminalReadyFrame {
                    cols: 120,
                    rows: 32,
                    replay_bytes: 262_144,
                }),
            ),
            (
                json!({ "op": "exit", "code": 130 }),
                TerminalServerFrame::Exit(TerminalExitFrame { code: 130 }),
            ),
            (
                json!({ "op": "closed", "reason": "attached_elsewhere" }),
                TerminalServerFrame::Closed(TerminalClosedFrame {
                    reason: TerminalClosedReason::AttachedElsewhere,
                }),
            ),
            (
                json!({ "op": "pong", "n": 7 }),
                TerminalServerFrame::Pong(TerminalPongFrame { n: 7 }),
            ),
        ];
        for (written, frame) in frames {
            assert_eq!(
                TerminalServerFrame::decode(&written.to_string()).unwrap(),
                frame
            );
            assert_eq!(serde_json::to_value(&frame).unwrap(), written);
        }
    }

    #[test]
    fn a_process_that_was_killed_exits_with_a_negative_code_as_well() {
        let frame = TerminalServerFrame::decode(r#"{"op":"exit","code":-9}"#).unwrap();
        assert_eq!(
            frame,
            TerminalServerFrame::Exit(TerminalExitFrame { code: -9 })
        );
    }

    #[test]
    fn a_text_frame_the_protocol_does_not_have_is_refused_by_either_side() {
        assert!(matches!(
            TerminalClientFrame::decode(r#"{"op":"ready","cols":1,"rows":1,"replay_bytes":0}"#),
            Err(FrameFault::UnknownOp(_))
        ));
        assert!(matches!(
            TerminalServerFrame::decode(r#"{"op":"resize","cols":1,"rows":1}"#),
            Err(FrameFault::UnknownOp(_))
        ));
        assert!(matches!(
            TerminalClientFrame::decode(r#"{"op":"resize","cols":120}"#),
            Err(FrameFault::Members { .. })
        ));
        assert!(matches!(
            TerminalClientFrame::decode(r#"{"op":"resize","cols":120,"rows":32,"px":9}"#),
            Err(FrameFault::Members { .. })
        ));
        assert!(matches!(
            TerminalClientFrame::decode("typed bytes"),
            Err(FrameFault::NotJson(_))
        ));
        assert!(matches!(
            TerminalClientFrame::decode("[1]"),
            Err(FrameFault::NoOp)
        ));
    }

    #[test]
    fn a_screen_taken_over_by_another_leaves_the_shell_running_and_every_other_close_ends_it() {
        for reason in TerminalClosedReason::ALL {
            assert_eq!(
                reason.ends_the_shell(),
                reason != TerminalClosedReason::AttachedElsewhere,
                "{reason:?}"
            );
        }
        assert_eq!(
            serde_json::to_value(TerminalClosedReason::ALL).unwrap(),
            json!([
                "exited",
                "closed_by_user",
                "detached",
                "environment_stopped",
                "link_lost",
                "attached_elsewhere",
                "revoked"
            ])
        );
    }

    #[test]
    fn the_replay_is_the_first_bytes_after_ready_and_a_frame_may_hold_the_end_of_it() {
        let mut replay = Replay::announced(&TerminalReadyFrame {
            cols: 80,
            rows: 24,
            replay_bytes: 1_000,
        });
        assert_eq!(replay.split(600), (600, 0));
        assert_eq!(replay.remaining(), 400);
        // One frame that holds the last of the replay and the first live output.
        assert_eq!(replay.split(500), (400, 100));
        assert_eq!(replay.remaining(), 0);
        assert_eq!(replay.split(50), (0, 50));
    }

    #[test]
    fn a_shell_with_nothing_to_replay_has_live_output_from_the_first_byte() {
        let mut replay = Replay::announced(&TerminalReadyFrame {
            cols: 80,
            rows: 24,
            replay_bytes: 0,
        });
        assert_eq!(replay.split(10), (0, 10));
        let mut odd = Replay::announced(&TerminalReadyFrame {
            cols: 80,
            rows: 24,
            replay_bytes: -5,
        });
        assert_eq!(odd.split(10), (0, 10));
    }

    #[test]
    fn reading_stops_at_a_mebibyte_behind_and_goes_on_at_a_quarter_of_that() {
        let mut flow = Backpressure::new();
        assert_eq!(flow.update(0), None);
        assert_eq!(flow.update(1_024 * 1_024 - 1), None);
        assert_eq!(flow.update(1_024 * 1_024), Some(Flow::Pause));
        assert!(flow.paused());
        // While it is stopped the figure between the two marks changes nothing.
        assert_eq!(flow.update(600 * 1_024), None);
        assert_eq!(flow.update(256 * 1_024 + 1), None);
        assert_eq!(flow.update(256 * 1_024), Some(Flow::Resume));
        assert!(!flow.paused());
        // And while it reads, the same figures change nothing either.
        assert_eq!(flow.update(600 * 1_024), None);
    }
}
