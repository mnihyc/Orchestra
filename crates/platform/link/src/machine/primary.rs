//! Idle-safe Automatic primary reconsideration through the same bounded verified entrance probes.
use super::*;
use orch_link_contract::gateways::{GatewayMeasurement, GatewaySelection};

fn improvement<'a>(current: &str, samples: &'a [GatewayMeasurement]) -> Option<&'a str> {
    let current_score = samples
        .iter()
        .filter(|sample| sample.gateway_id == current)
        .map(|sample| sample.round_trip_ms)
        .min()?;
    samples
        .iter()
        .filter(|sample| sample.gateway_id != current && sample.round_trip_ms < current_score)
        .min_by_key(|sample| sample.round_trip_ms)
        .map(|sample| sample.gateway_id.as_str())
}

pub(super) async fn reconsider(shared: &Shared, dialer: &dyn Dialer, clock: &dyn Clock) {
    // One heartbeat cadence refreshes hints before the normal live-link freshness bound.
    let mut interval = tokio::time::interval(Duration::from_secs(u64::from(HEARTBEAT_SECS)));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    loop {
        interval.tick().await;
        let policy = shared
            .gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if policy.choice.selection != GatewaySelection::Automatic {
            continue;
        }
        let endpoints = shared
            .endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let active = shared
            .active_endpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Ok((_, samples)) = policy
            .candidates(
                dialer,
                &endpoints,
                active.as_ref().map(|endpoint| endpoint.url.as_str()),
            )
            .await
        else {
            continue;
        };
        *shared
            .gateway_measurements
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = samples.clone();
        *shared
            .gateway_measured_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = (!samples.is_empty()).then(|| clock.time());
        shared
            .gateway_reports
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        let current = shared
            .gateway_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(target) = current
            .as_deref()
            .and_then(|current| improvement(current, &samples))
        else {
            continue;
        };
        // An operator selection made while the asynchronous probe ran always wins.
        if shared
            .gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .choice
            != policy.choice
        {
            continue;
        }
        if !shared.tunnels.pause_if_idle() {
            continue;
        }
        if !shared.auxiliary.prepare_primary(target) {
            shared.tunnels.unpause_admissions();
            continue;
        }
        // The existing supervisor closes that auxiliary before dialing the primary. It rechecks
        // current policy and every trust/admission check; this hint grants no connection.
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_link_contract::enrollment::Trust;
    fn sample(id: &str, rtt: u64) -> GatewayMeasurement {
        GatewayMeasurement {
            gateway_id: id.into(),
            endpoint: Endpoint {
                url: format!("wss://{id}.example.invalid/link"),
                trust: Trust::Pinned,
            },
            round_trip_ms: rtt,
        }
    }
    #[test]
    fn moving_a_healthy_primary_needs_a_strict_measured_improvement() {
        assert_eq!(
            improvement("eu", &[sample("eu", 10), sample("asia", 10)]),
            None
        );
        assert_eq!(improvement("eu", &[sample("asia", 1)]), None);
        assert_eq!(
            improvement("eu", &[sample("eu", 10), sample("asia", 0)]),
            Some("asia")
        );
        assert_eq!(
            improvement("eu", &[sample("eu", 0), sample("asia", 0)]),
            None
        );
    }
}
