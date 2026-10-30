//! Acknowledged byte offsets and resumption.
//!
//! A tunnel that loses its session must not look to the program at either end
//! like a connection that dropped. So each end keeps what it sent until the
//! other end has said that it arrived, and when the tunnel is taken up again
//! on a new session the two say how much each has, and each sends again from
//! there.
//!
//! [`ReplayBuffer`] is what is kept: the bytes from the other end's last
//! acknowledgement to the last byte the program wrote, by offset in the
//! channel's direction. It gives back any stretch of them for a frame, and
//! lets go of what is acknowledged.

use bytes::{Bytes, BytesMut};
use std::collections::VecDeque;

/// The bytes of one direction of a channel that are not yet acknowledged.
#[derive(Debug, Default)]
pub struct ReplayBuffer {
    chunks: VecDeque<Bytes>,
    /// The offset of the first byte held.
    start: u64,
    /// How many bytes are held.
    held: u64,
}

impl ReplayBuffer {
    /// An empty buffer at offset 0.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The offset of the first byte held.
    #[must_use]
    pub const fn start(&self) -> u64 {
        self.start
    }

    /// The offset after the last byte held: how many bytes were written in
    /// all.
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.start + self.held
    }

    /// How many bytes are held.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.held
    }

    /// Whether nothing is held.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.held == 0
    }

    /// Adds bytes at the end.
    pub fn push(&mut self, bytes: Bytes) {
        self.held += bytes.len() as u64;
        self.chunks.push_back(bytes);
    }

    /// Lets go of everything before `offset`. Returns how many bytes that
    /// freed.
    pub fn release_to(&mut self, offset: u64) -> u64 {
        let target = offset.clamp(self.start, self.end());
        let mut freed = target - self.start;
        let total = freed;
        while freed > 0 {
            let Some(front) = self.chunks.front_mut() else {
                break;
            };
            let size = front.len() as u64;
            if size <= freed {
                freed -= size;
                self.chunks.pop_front();
            } else {
                let cut = usize::try_from(freed).unwrap_or(usize::MAX);
                *front = front.slice(cut..);
                freed = 0;
            }
        }
        self.start = target;
        self.held -= total;
        total
    }

    /// Up to `max` bytes from `offset`, or `None` when `offset` is not held.
    /// A stretch that lies in one chunk is not copied.
    #[must_use]
    pub fn slice(&self, offset: u64, max: usize) -> Option<Bytes> {
        if offset < self.start || offset >= self.end() || max == 0 {
            return None;
        }
        let mut skip = offset - self.start;
        let mut pieces = self.chunks.iter();
        let mut first = None;
        for chunk in pieces.by_ref() {
            let size = chunk.len() as u64;
            if skip < size {
                first = Some(chunk.slice(usize::try_from(skip).unwrap_or(usize::MAX)..));
                break;
            }
            skip -= size;
        }
        let first = first?;
        if first.len() >= max {
            return Some(first.slice(..max));
        }
        // The stretch runs on into the next chunks; join them.
        let mut joined = BytesMut::with_capacity(max);
        joined.extend_from_slice(&first);
        for chunk in pieces {
            let room = max - joined.len();
            if room == 0 {
                break;
            }
            joined.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
        Some(joined.freeze())
    }
}
