//! Primary gateway policy and bounded, non-admitting round-trip measurements.

use crate::reconnect::{DialError, Dialer};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use orch_link_contract::enrollment::Endpoint;
use orch_link_contract::gateways::{
    GatewayChoice, GatewayInstance, GatewayMeasurement, GatewaySelection,
};
use std::time::Duration;

/// A small regional list is measured with at most four concurrent sockets.
pub const PROBE_CONCURRENCY: usize = 4;
/// Three measurements tolerate one scheduling outlier without hiding the minimum median.
pub const PROBE_SAMPLES: usize = 3;

/// Transport-owned, verified probes. A probe never establishes a machine session.
#[async_trait]
pub trait GatewayProber: Send + Sync {
    /// Measure one verified entrance within the ordinary connection bound.
    async fn measure(&self, endpoint: &Endpoint) -> Result<GatewayMeasurement, DialError>;
}

/// Configuration carried by the machine supervisor, distinct from the local endpoint override.
#[derive(Debug, Clone, Default)]
pub struct PrimaryGateway {
    /// The local fixed or automatic choice.
    pub choice: GatewayChoice,
    /// The latest authenticated system catalogue.
    pub catalogue: Vec<GatewayInstance>,
}

/// One candidate and the gateway that must identify itself before authentication.
#[derive(Debug, Clone)]
pub struct GatewayCandidate {
    /// Verified entrance or legacy configured address.
    pub endpoint: Endpoint,
    /// Absent only when an older system has no labelled catalogue.
    pub gateway_id: Option<String>,
}

fn origin(endpoint: &Endpoint) -> Option<String> {
    url::Url::parse(&endpoint.https_base()?)
        .ok()
        .map(|url| url.to_string())
}

impl PrimaryGateway {
    /// Resolve a fixed gateway or rank successful Automatic measurements.
    /// Every reconnect invokes this method again; it never changes a healthy session.
    pub async fn candidates(
        &self,
        dialer: &dyn Dialer,
        endpoints: &[Endpoint],
        last_worked: Option<&str>,
    ) -> Result<(Vec<GatewayCandidate>, Vec<GatewayMeasurement>), String> {
        if self.choice.selection == GatewaySelection::Selected {
            let id = self
                .choice
                .gateway_id
                .as_deref()
                .ok_or("the selected gateway has no identity")?;
            let instance = self
                .catalogue
                .iter()
                .find(|gateway| gateway.id == id)
                .ok_or_else(|| format!("the selected gateway {id} is no longer advertised"))?;
            let mut candidates = endpoints
                .iter()
                .filter(|endpoint| {
                    let key = origin(endpoint);
                    instance
                        .endpoints
                        .iter()
                        .any(|known| key.is_some() && origin(known) == key)
                        || !self
                            .catalogue
                            .iter()
                            .flat_map(|gateway| &gateway.endpoints)
                            .any(|known| key.is_some() && origin(known) == key)
                })
                .cloned()
                .map(|endpoint| GatewayCandidate {
                    endpoint,
                    gateway_id: Some(id.to_owned()),
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                return Err(format!(
                    "the selected gateway {} has no entrance in the local endpoint override",
                    instance.label
                ));
            }
            candidates
                .sort_by_key(|candidate| Some(candidate.endpoint.url.as_str()) != last_worked);
            return Ok((candidates, Vec::new()));
        }
        if self.catalogue.is_empty() {
            return Ok((
                orch_link_contract::enrollment::endpoint_order(endpoints, last_worked)
                    .into_iter()
                    .map(|endpoint| GatewayCandidate {
                        endpoint: endpoint.clone(),
                        gateway_id: None,
                    })
                    .collect(),
                Vec::new(),
            ));
        }
        let timeout =
            Duration::from_secs(u64::from(orch_link_contract::session::CONNECT_TIMEOUT_SECS));
        let mut measured = stream::iter(endpoints.iter().cloned().enumerate().map(
            |(position, endpoint)| async move {
                let result = tokio::time::timeout(timeout, dialer.probe(&endpoint)).await;
                (position, endpoint, result)
            },
        ))
        .buffer_unordered(PROBE_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        measured.sort_by_key(|(position, _, _)| *position);
        let mut good = measured
            .into_iter()
            .filter_map(|(position, endpoint, result)| {
                let Ok(Ok(mut measurement)) = result else {
                    return None;
                };
                let gateway = self
                    .catalogue
                    .iter()
                    .find(|gateway| gateway.id == measurement.gateway_id)?;
                let known = self.catalogue.iter().find(|gateway| {
                    gateway
                        .endpoints
                        .iter()
                        .any(|known| origin(known) == origin(&endpoint))
                });
                if known.is_some_and(|known| known.id != gateway.id) {
                    return None;
                }
                measurement.endpoint = endpoint;
                Some((position, measurement))
            })
            .collect::<Vec<_>>();
        good.sort_by_key(|(position, measurement)| {
            (
                measurement.round_trip_ms,
                Some(measurement.endpoint.url.as_str()) != last_worked,
                *position,
            )
        });
        if good.is_empty() {
            return Err("no advertised gateway has a verified round-trip measurement".to_owned());
        }
        let measurements = good
            .into_iter()
            .map(|(_, measurement)| measurement)
            .collect::<Vec<_>>();
        let candidates = measurements
            .iter()
            .map(|measurement| GatewayCandidate {
                endpoint: measurement.endpoint.clone(),
                gateway_id: Some(measurement.gateway_id.clone()),
            })
            .collect();
        Ok((candidates, measurements))
    }
}
