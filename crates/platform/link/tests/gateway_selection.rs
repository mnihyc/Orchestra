//! Observable primary selection and probe budgets, without machine admission.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]

use async_trait::async_trait;
use orch_link::framing::Connection;
use orch_link::gateways::PrimaryGateway;
use orch_link::reconnect::{DialError, Dialer};
use orch_link_contract::enrollment::{Endpoint, Trust};
use orch_link_contract::gateways::{
    GatewayChoice, GatewayInstance, GatewayMeasurement, GatewaySelection,
};
use std::collections::HashMap;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

struct Probes {
    observations: Mutex<HashMap<String, (String, u64)>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    calls: AtomicUsize,
    delay: Duration,
}

struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Probes {
    fn new(catalogue: &[GatewayInstance]) -> Self {
        Self {
            observations: Mutex::new(
                catalogue
                    .iter()
                    .flat_map(|gateway| {
                        gateway
                            .endpoints
                            .iter()
                            .map(move |endpoint| (endpoint.url.clone(), (gateway.id.clone(), 90)))
                    })
                    .collect(),
            ),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(10),
        }
    }
}

#[async_trait]
impl Dialer for Probes {
    async fn dial(&self, _endpoint: &Endpoint) -> Result<Connection, DialError> {
        Err(DialError::Connect(
            "a probe never authenticates a machine".into(),
        ))
    }
    async fn probe(&self, endpoint: &Endpoint) -> Result<GatewayMeasurement, DialError> {
        let count = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let _active = Active(&self.active);
        self.peak.fetch_max(count, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        let observed = self
            .observations
            .lock()
            .unwrap()
            .get(&endpoint.url)
            .cloned();
        tokio::time::sleep(self.delay).await;
        let (gateway_id, round_trip_ms) =
            observed.ok_or_else(|| DialError::Connect("unreachable".into()))?;
        Ok(GatewayMeasurement {
            gateway_id,
            endpoint: endpoint.clone(),
            round_trip_ms,
        })
    }
}

fn gateways() -> Vec<GatewayInstance> {
    ["eu", "asia", "us"]
        .into_iter()
        .map(|id| GatewayInstance {
            id: id.into(),
            label: id.into(),
            region: id.into(),
            endpoints: [9443, 443]
                .into_iter()
                .map(|port| Endpoint {
                    url: format!("wss://{id}.test:{port}"),
                    trust: Trust::Pinned,
                })
                .collect(),
        })
        .collect()
}

fn entrances(catalogue: &[GatewayInstance]) -> Vec<Endpoint> {
    catalogue
        .iter()
        .flat_map(|gateway| gateway.endpoints.clone())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn automatic_remeasures_and_prefers_round_trip_over_the_last_success() {
    let catalogue = gateways();
    let prober = Probes::new(&catalogue);
    let endpoints = entrances(&catalogue);
    prober
        .observations
        .lock()
        .unwrap()
        .insert(endpoints[2].url.clone(), ("asia".into(), 8));
    let policy = PrimaryGateway {
        choice: GatewayChoice::default(),
        catalogue,
    };
    let (first, _) = policy
        .candidates(&prober, &endpoints, Some(&endpoints[0].url))
        .await
        .unwrap();
    assert_eq!(first[0].gateway_id.as_deref(), Some("asia"));
    prober
        .observations
        .lock()
        .unwrap()
        .insert(endpoints[4].url.clone(), ("us".into(), 3));
    let (next, _) = policy
        .candidates(&prober, &endpoints, Some(&endpoints[2].url))
        .await
        .unwrap();
    assert_eq!(next[0].gateway_id.as_deref(), Some("us"));
    assert_eq!(prober.calls.load(Ordering::SeqCst), 12);
}

#[tokio::test(start_paused = true)]
async fn a_selected_gateway_is_never_substituted_even_when_other_gateways_are_faster() {
    let catalogue = gateways();
    let prober = Probes::new(&catalogue);
    let endpoints = entrances(&catalogue);
    prober
        .observations
        .lock()
        .unwrap()
        .remove(&endpoints[0].url);
    let mut policy = PrimaryGateway {
        choice: GatewayChoice {
            selection: GatewaySelection::Selected,
            gateway_id: Some("eu".into()),
        },
        catalogue,
    };
    let (candidates, _) = policy
        .candidates(&prober, &endpoints, Some(&endpoints[2].url))
        .await
        .unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.gateway_id.as_deref() == Some("eu"))
    );
    assert_eq!(prober.calls.load(Ordering::SeqCst), 0);
    policy.catalogue.remove(0);
    assert!(policy.candidates(&prober, &endpoints, None).await.is_err());
    assert_eq!(policy.choice.gateway_id.as_deref(), Some("eu"));
}

#[tokio::test(start_paused = true)]
async fn automatic_limits_concurrent_probes_and_rejects_another_gateways_entrance() {
    let catalogue = gateways();
    let prober = Probes::new(&catalogue);
    let endpoints = entrances(&catalogue);
    prober
        .observations
        .lock()
        .unwrap()
        .insert(endpoints[0].url.clone(), ("asia".into(), 1));
    let policy = PrimaryGateway {
        choice: GatewayChoice::default(),
        catalogue,
    };
    let (candidates, measurements) = policy.candidates(&prober, &endpoints, None).await.unwrap();
    assert_eq!(prober.peak.load(Ordering::SeqCst), 4);
    assert_eq!(prober.active.load(Ordering::SeqCst), 0);
    assert_eq!(measurements.len(), 5);
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.endpoint != endpoints[0])
    );
}

#[tokio::test(start_paused = true)]
async fn an_unresponsive_probe_expires_at_the_existing_connection_bound() {
    let catalogue = gateways();
    let mut prober = Probes::new(&catalogue);
    prober.delay = Duration::from_secs(11);
    let policy = PrimaryGateway {
        choice: GatewayChoice::default(),
        catalogue: catalogue.clone(),
    };
    assert!(
        policy
            .candidates(&prober, &catalogue[0].endpoints[..1], None)
            .await
            .is_err()
    );
    assert_eq!(prober.active.load(Ordering::SeqCst), 0);
}
