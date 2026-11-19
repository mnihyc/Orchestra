//! Bounded recovery bursts with a quiet cooldown, outside database transactions.

use orch_db::Db;
use orch_settings::Place;
use orch_settings_contract::registry::{self, keys};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Policy {
    attempts: u64,
    initial: Duration,
    recovery: Duration,
}

impl Policy {
    pub async fn load(db: &Db) -> Self {
        let default = |key| {
            registry::get(key)
                .and_then(|def| def.default_value())
                .and_then(|value| value.as_u64())
                .unwrap_or_default()
        };
        let mut attempts = default(keys::DATABASE_RETRY_ATTEMPTS);
        let mut initial = default(keys::DATABASE_RETRY_INITIAL_MS);
        let mut recovery = default(keys::DATABASE_RECOVERY_SECS);
        if let Ok(mut conn) = db.acquire().await {
            if let Ok(Some(value)) =
                orch_settings::get::<u64>(&mut conn, Place::System, keys::DATABASE_RETRY_ATTEMPTS)
                    .await
            {
                attempts = value;
            }
            if let Ok(Some(value)) =
                orch_settings::get::<u64>(&mut conn, Place::System, keys::DATABASE_RETRY_INITIAL_MS)
                    .await
            {
                initial = value;
            }
            if let Ok(Some(value)) =
                orch_settings::get::<u64>(&mut conn, Place::System, keys::DATABASE_RECOVERY_SECS)
                    .await
            {
                recovery = value;
            }
        }
        Self {
            attempts,
            initial: Duration::from_millis(initial),
            recovery: Duration::from_secs(recovery),
        }
    }

    pub fn episode(&self) -> Episode {
        Episode {
            policy: self.clone(),
            failures: 0,
            reported: 0,
            last_report: None,
            retry_at: None,
            rejected: false,
        }
    }
}

#[derive(Debug)]
pub struct Episode {
    policy: Policy,
    failures: u64,
    reported: u64,
    last_report: Option<Instant>,
    retry_at: Option<Instant>,
    rejected: bool,
}

impl Episode {
    /// Only a completed operation ends the failure episode, not a new caller or a timer wake.
    pub fn recovered(&mut self) {
        self.failures = 0;
        self.reported = 0;
        self.last_report = None;
        self.retry_at = None;
        self.rejected = false;
    }

    /// Repeated callers and timer wakes cannot shorten the current pause.
    pub fn ready(&self) -> bool {
        !self.rejected
            && (self.failures == 0 || self.retry_at.is_some_and(|at| Instant::now() >= at))
    }

    /// A deterministic representation is reconsidered only after fresh state, not a retry timer.
    pub(crate) fn reject(&mut self) {
        self.rejected = true;
    }
    pub(crate) fn is_rejected(&self) -> bool {
        self.rejected
    }

    /// Fresh state allows another look without starting a new failure or reporting episode.
    pub(crate) fn reconsider(&mut self) {
        self.rejected = false;
    }

    /// Returns the next delay and, at most once per recovery interval, a counted summary.
    pub fn failed(&mut self) -> (Duration, Option<u64>) {
        self.failed_at(Instant::now())
    }

    fn failed_at(&mut self, now: Instant) -> (Duration, Option<u64>) {
        self.failures = self.failures.saturating_add(1);
        self.reported = self.reported.saturating_add(1);
        let report = if self
            .last_report
            .is_none_or(|at| now.saturating_duration_since(at) >= self.policy.recovery)
        {
            self.last_report = Some(now);
            Some(std::mem::take(&mut self.reported))
        } else {
            None
        };
        let burst = self.policy.attempts.saturating_add(1);
        let at = (self.failures - 1) % burst;
        let pause = if at >= self.policy.attempts {
            self.policy.recovery
        } else if self.policy.initial.is_zero() {
            Duration::ZERO
        } else {
            let mut pause = self.policy.initial;
            for _ in 0..at {
                pause = pause.saturating_mul(2).min(self.policy.recovery);
                if pause >= self.policy.recovery {
                    break;
                }
            }
            pause.min(self.policy.recovery)
        };
        self.retry_at = now.checked_add(pause);
        (pause, report)
    }
}

/// An unrepresentably distant deadline stays pending until shutdown instead of wrapping or panicking.
pub async fn wait(pause: Duration, stop: &tokio_util::sync::CancellationToken) -> bool {
    let Some(deadline) = tokio::time::Instant::now().checked_add(pause) else {
        stop.cancelled().await;
        return false;
    };
    tokio::select! {
        ()=tokio::time::sleep_until(deadline)=>true,
        ()=stop.cancelled()=>false,
    }
}

#[cfg(test)]
mod tests;
