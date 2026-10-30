//! Bounded RFB client framing keeps watching sockets from sending actions.

use bytes::{Bytes, BytesMut};

/// Matches the browser input budget, bounding clipboard and encoding allocations before waiting.
pub const MAX_INPUT: usize = 256 * 1024;

/// An unknown or overlong message cannot be followed safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the RFB client stream cannot be followed")]
pub struct ProtocolError;

/// One whole message, consumed even when a watching socket must drop it.
pub struct ClientMessage {
    /// The exact protocol bytes.
    pub bytes: Bytes,
    /// Whether this message acts on the display rather than requesting a picture.
    pub action: bool,
    /// Actual work, excluding automatic extended-clipboard capability announcements.
    pub activity: bool,
}

/// A client stream whose WebSocket frames may split or combine RFB messages.
#[derive(Default)]
pub struct Filter {
    pending: BytesMut,
}

impl Filter {
    /// Adds one bounded socket frame; at most one incomplete message is retained between frames.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        if bytes.len() > MAX_INPUT || self.pending.len() + bytes.len() > 2 * MAX_INPUT {
            return Err(ProtocolError);
        }
        self.pending.extend_from_slice(bytes);
        Ok(())
    }

    /// Takes one complete message; no bytes after a dropped action lose their framing.
    pub fn take(&mut self) -> Result<Option<ClientMessage>, ProtocolError> {
        let Some((length, action)) = length(&self.pending)? else {
            return Ok(None);
        };
        if self.pending.len() < length {
            return Ok(None);
        }
        let bytes = self.pending.split_to(length).freeze();
        let clipboard_caps = bytes.first() == Some(&6)
            && bytes.len() >= 12
            && bytes[4] & 0x80 != 0
            && bytes[8] & 1 != 0;
        Ok(Some(ClientMessage {
            bytes,
            action,
            activity: action && !clipboard_caps,
        }))
    }
}

// RFB 3.8 and noVNC 1.6 client messages, including every extension its rfb.js sends:
// 0 pixel format: 20; 2 encodings: 4 + 4*n; 3 update request: 10;
// 4 key: 8; 5 pointer: 6 (7 with the extended-button marker);
// 6 clipboard: 8 + abs(signed length), including extended clipboard caps/requests;
// 150 continuous updates: 10; 248 fence: 9 + payload length (at most 64);
// 250 XVP power: 4; 251 desktop size: 8 + 16*screens;
// 255 QEMU extended key subtype 0: 12. Unknown subtypes close on both ticket kinds.
// Only 0, 2, 3, 150 and 248 set up/request pictures; all other known kinds are actions.
fn length(bytes: &[u8]) -> Result<Option<(usize, bool)>, ProtocolError> {
    let Some(kind) = bytes.first().copied() else {
        return Ok(None);
    };
    let (length, action) = match kind {
        0 => (20, false),
        2 => {
            if bytes.len() < 4 {
                return Ok(None);
            }
            (
                4 + 4 * usize::from(u16::from_be_bytes([bytes[2], bytes[3]])),
                false,
            )
        }
        3 | 150 => (10, false),
        4 => (8, true),
        5 => {
            if bytes.len() < 2 {
                return Ok(None);
            }
            (if bytes[1] & 0x80 == 0 { 6 } else { 7 }, true)
        }
        6 => {
            if bytes.len() < 8 {
                return Ok(None);
            }
            let size = i32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]).unsigned_abs();
            (8 + usize::try_from(size).map_err(|_| ProtocolError)?, true)
        }
        248 => {
            if bytes.len() < 9 {
                return Ok(None);
            }
            if bytes[8] > 64 {
                return Err(ProtocolError);
            }
            (9 + usize::from(bytes[8]), false)
        }
        250 => (4, true),
        251 => {
            if bytes.len() < 8 {
                return Ok(None);
            }
            (8 + 16 * usize::from(bytes[6]), true)
        }
        255 => {
            if bytes.len() < 2 {
                return Ok(None);
            }
            if bytes[1] != 0 {
                return Err(ProtocolError);
            }
            (12, true)
        }
        _ => return Err(ProtocolError),
    };
    if length > MAX_INPUT {
        return Err(ProtocolError);
    }
    Ok(Some((length, action)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn update() -> Vec<u8> {
        vec![3, 1, 0, 0, 0, 0, 0, 80, 0, 60]
    }

    #[test]
    fn keys_clicks_paste_resize_and_power_leave_the_next_picture_request_intact() {
        let actions = [
            vec![4, 1, 0, 0, 0, 0, 0, 65],
            vec![5, 1, 0, 25, 0, 30],
            vec![5, 0x80, 0, 25, 0, 30, 1],
            vec![6, 0, 0, 0, 0, 0, 0, 3, b'a', b'b', b'c'],
            vec![6, 0, 0, 0, 255, 255, 255, 252, 0, 0, 0, 0],
            vec![251, 0, 0, 80, 0, 60, 0, 0],
            vec![250, 0, 1, 2],
            vec![255, 0, 0, 1, 0, 0, 0, 65, 0, 0, 0, 30],
        ];
        for action in actions {
            let mut filter = Filter::default();
            let mut page = Vec::new();
            for byte in action.iter().chain(update().iter()) {
                filter.push(&[*byte]).unwrap();
                while let Some(message) = filter.take().unwrap() {
                    if !message.action {
                        page.extend_from_slice(&message.bytes);
                    }
                }
            }
            assert_eq!(page, update(), "watching passed an action or lost framing");
        }
    }

    #[test]
    fn picture_setup_continuous_updates_and_fences_pass_on_a_watching_stream() {
        let mut format = vec![0; 20];
        format[0] = 0;
        let mut wire = format;
        wire.extend([2, 0, 0, 1, 0, 0, 0, 0]);
        wire.extend(update());
        wire.extend([150, 1, 0, 0, 0, 0, 0, 80, 0, 60]);
        wire.extend([248, 0, 0, 0, 0, 0, 0, 0, 3, b'o', b'n', b'e']);
        let mut filter = Filter::default();
        filter.push(&wire).unwrap();
        let mut passed = Vec::new();
        while let Some(message) = filter.take().unwrap() {
            assert!(!message.action);
            passed.extend_from_slice(&message.bytes);
        }
        assert_eq!(passed, wire);
    }

    #[test]
    fn unknown_and_oversized_messages_end_before_any_following_action_can_be_misread() {
        for wire in [
            vec![7],
            vec![255, 1],
            vec![6, 0, 0, 0, 0x80, 0, 0, 0],
            vec![248, 0, 0, 0, 0, 0, 0, 0, 65],
        ] {
            let mut filter = Filter::default();
            filter.push(&wire).unwrap();
            assert!(filter.take().is_err());
        }
    }
}
