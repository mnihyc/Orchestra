//! Accepted sizing and admission policy for browser and machine file pieces.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// System defaults are negotiated before bytes move and retained by unfinished uploads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferPolicy {
    /// The first measurement's small piece, before bandwidth is known.
    pub initial_piece_bytes: i64,
    /// Desired small initial probes, within the shared request and byte ceilings.
    pub initial_requests: i64,
    /// The smallest ordinary piece; the final piece may be smaller.
    pub min_piece_bytes: i64,
    /// The accepted ceiling for a request's content.
    pub max_piece_bytes: i64,
    /// Desired data duration in the shared window, before round-trip headroom.
    pub target_window_ms: i64,
    /// Round trips of data kept ahead of acknowledgements.
    pub window_rtts: i64,
    /// Maximum bytes admitted across simultaneous files.
    pub max_window_bytes: i64,
    /// Maximum simultaneous piece requests across files.
    pub requests: i64,
}

impl Default for TransferPolicy {
    fn default() -> Self {
        let number = |key: &str| {
            crate::registry::get(key)
                .and_then(crate::registry::SettingDef::default_value)
                .and_then(|value| value.as_i64())
                .unwrap_or(1)
        };
        Self::from_values(number)
    }
}

impl TransferPolicy {
    /// Reads the registry keys from either defaults or one resolved system view.
    pub fn from_values(mut number: impl FnMut(&str) -> i64) -> Self {
        Self {
            initial_piece_bytes: number("storage.transfer_initial_piece_bytes"),
            initial_requests: number("storage.transfer_initial_requests"),
            min_piece_bytes: number("storage.transfer_min_piece_bytes"),
            max_piece_bytes: number("storage.transfer_max_piece_bytes"),
            target_window_ms: number("storage.transfer_target_window_ms"),
            window_rtts: number("storage.transfer_window_rtts"),
            max_window_bytes: number("storage.transfer_max_window_bytes"),
            requests: number("storage.transfer_requests"),
        }
    }

    /// Policy arithmetic stays exact in browsers; request counts fit the server counter.
    #[must_use]
    pub fn valid(self) -> bool {
        // JSON numbers shared with browsers require exact integer representation.
        let exact = (1i64 << 53) - 1;
        let represented = [
            self.initial_requests,
            self.initial_piece_bytes,
            self.min_piece_bytes,
            self.max_piece_bytes,
            self.max_window_bytes,
            self.target_window_ms,
            self.window_rtts,
            self.requests,
        ]
        .into_iter()
        .all(|n| n > 0 && n <= exact);
        represented
            && self.initial_requests > 0
            && self.initial_requests <= i64::from(u32::MAX)
            && self.min_piece_bytes > 0
            && self.initial_piece_bytes >= self.min_piece_bytes
            && self.max_piece_bytes >= self.initial_piece_bytes
            && self.max_window_bytes >= self.max_piece_bytes
            && self.target_window_ms > 0
            && self.window_rtts > 0
            && self.requests > 0
            && self.requests <= i64::from(u32::MAX)
    }

    /// A shared window covers data transmission and several acknowledgement round trips.
    #[must_use]
    pub fn window_bytes(self, bytes_per_second: f64, rtt_seconds: f64) -> i64 {
        if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
            return self
                .initial_piece_bytes
                .saturating_mul(self.initial_requests.min(self.requests))
                .min(self.max_window_bytes);
        }
        (bytes_per_second
            * (self.target_window_ms as f64 / 1000.0 + rtt_seconds * self.window_rtts as f64))
            .ceil() as i64
    }

    /// Dividing the bounded window over requests amortizes round trips without exceeding staging.
    #[must_use]
    pub fn piece_bytes(self, bytes_per_second: f64, rtt_seconds: f64) -> i64 {
        if bytes_per_second <= 0.0 {
            return self.initial_piece_bytes;
        }
        (self
            .window_bytes(bytes_per_second, rtt_seconds)
            .min(self.max_window_bytes)
            / self.requests)
            .clamp(self.min_piece_bytes, self.max_piece_bytes)
    }
}
