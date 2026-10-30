//! The shaped link the slow-link scenarios run through: delay, rate and loss between browser and edge, at the profile and at its floor.
//!
//! Shaping belongs to an isolated network namespace, never to the host's root qdisc. The release
//! scenario in `scripts/smoke/link-load` uses these same units and records the kernel's actual qdisc.

/// The direction seen by the browser or other client at the public edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the client to the edge.
    Upload,
    /// From the edge to the client.
    Download,
}

/// Packet shaping at the client's boundary; TCP remains responsible for retransmission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    /// One-way delay, so a round trip sees twice this value.
    pub delay_ms: u32,
    /// Rate towards the edge, in decimal kilobits per second.
    pub upload_kbit: u32,
    /// Rate from the edge, in decimal kilobits per second.
    pub download_kbit: u32,
    /// Independently lost packets, as a whole percentage.
    pub loss_percent: u8,
}

/// The slow-link product profile: 300-ms round trip, 750-kbit upload, 1.6-Mbit download and 1% loss.
pub const PROFILE: Profile = Profile {
    delay_ms: 150,
    upload_kbit: 750,
    download_kbit: 1_600,
    loss_percent: 1,
};

/// The floor keeps the same delay and loss, with one tenth of each rate; no speed promise applies.
pub const FLOOR: Profile = Profile {
    upload_kbit: 75,
    download_kbit: 160,
    ..PROFILE
};

impl Profile {
    /// Arguments for the netem leaf, after `qdisc replace dev … parent … handle …`.
    #[must_use]
    pub fn netem(self, direction: Direction) -> Vec<String> {
        let rate = match direction {
            Direction::Upload => self.upload_kbit,
            Direction::Download => self.download_kbit,
        };
        vec![
            "netem".into(),
            "delay".into(),
            format!("{}ms", self.delay_ms),
            "loss".into(),
            format!("{}%", self.loss_percent),
            "rate".into(),
            format!("{rate}kbit"),
            // About eight seconds at the larger rate, in Ethernet-sized packets. Keeping it finite makes
            // a stalled reader apply backpressure instead of turning host memory into a queue.
            "limit".into(),
            "1024".into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{Direction, FLOOR, PROFILE};

    #[test]
    fn the_profile_and_floor_keep_delay_and_loss_in_both_directions() {
        for (profile, upload, download) in [
            (PROFILE, "750kbit", "1600kbit"),
            (FLOOR, "75kbit", "160kbit"),
        ] {
            let up = profile.netem(Direction::Upload);
            let down = profile.netem(Direction::Download);
            assert!(up.windows(2).any(|part| part == ["delay", "150ms"]));
            assert!(down.windows(2).any(|part| part == ["loss", "1%"]));
            assert!(up.windows(2).any(|part| part == ["rate", upload]));
            assert!(down.windows(2).any(|part| part == ["rate", download]));
        }
    }
}
