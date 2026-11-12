//! Verified gateway discovery and RTT probes, shared by enrollment and reconnect.

use crate::https::{HttpsClient, HttpsError, https_base};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use hyper::{Method, StatusCode};
use orch_link::gateways::{GatewayProber, PROBE_SAMPLES};
use orch_link::reconnect::DialError;
use orch_link_contract::enrollment::{Endpoint, Trust};
use orch_link_contract::gateways::{
    GatewayMeasurement, GatewayProbeReply, PROBE_NONCE_HEADER, PROBE_PATH, probe_signing_input,
};
use orch_pki::certificates::{Certificate, ServiceName, verify_signature};
use orch_pki::verification::{OrchClientVerifier, ServerTrust, crypto, machine_client_config};
use orch_types::Clock;
use std::sync::Arc;
use std::time::Duration;

/// Service-certificate proofs retain the existing control-frame bound.
const MAX_PROBE_REPLY: usize = orch_link_contract::version::MAX_FRAME_BYTES as usize;

/// HTTPS and service-key proof are both checked, including through a public edge.
pub struct HttpsGatewayProber {
    pinned: HttpsClient,
    public: HttpsClient,
    verifier: Arc<OrchClientVerifier>,
    clock: Arc<dyn Clock>,
}

impl HttpsGatewayProber {
    /// Reuse the same HTTPS trust configurations as the machine's session dialer.
    pub fn new(
        pinned: Arc<rustls::ClientConfig>,
        public: Arc<rustls::ClientConfig>,
        authority: &Certificate,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, orch_pki::PkiError> {
        Ok(Self {
            pinned: HttpsClient::new(pinned),
            public: HttpsClient::new(public),
            verifier: OrchClientVerifier::new(authority, Duration::ZERO, &crypto())?,
            clock,
        })
    }

    /// Before enrollment no machine credential exists; probes need none.
    pub fn for_enrollment(
        authority: &Certificate,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, orch_pki::PkiError> {
        let server = || Some(ServerTrust::Authority(Box::new(authority.clone())));
        let pinned =
            machine_client_config(server(), ServiceName::Gateway, None, Arc::clone(&clock))?;
        let public =
            machine_client_config(server(), ServiceName::Frontend, None, Arc::clone(&clock))?;
        Self::new(pinned, public, authority, clock)
    }

    fn verify(&self, bytes: &[u8], nonce: &str, host: &str) -> Result<String, HttpsError> {
        let invalid = || {
            HttpsError::Tls(
                "the gateway probe proof is not from this installation and request".into(),
            )
        };
        let reply: GatewayProbeReply = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        if reply.nonce != nonce {
            return Err(invalid());
        }
        let certificate = Certificate::from_pem(&reply.certificate_pem).map_err(|_| invalid())?;
        if certificate.service() != Some(ServiceName::Gateway) {
            return Err(invalid());
        }
        let now = rustls::pki_types::UnixTime::since_unix_epoch(Duration::from_millis(
            u64::try_from(self.clock.time().unix_millis()).unwrap_or(0),
        ));
        self.verifier
            .check(&certificate.to_rustls(), &[], now)
            .map_err(|_| invalid())?;
        let signature = URL_SAFE_NO_PAD
            .decode(&reply.signature)
            .map_err(|_| invalid())?;
        let input = probe_signing_input(nonce, &reply.gateway_id, host);
        if !verify_signature(certificate.public_point(), input.as_bytes(), &signature) {
            return Err(invalid());
        }
        Ok(reply.gateway_id)
    }

    async fn measure_inner(&self, endpoint: &Endpoint) -> Result<GatewayMeasurement, HttpsError> {
        let base = https_base(&endpoint.url)?;
        let host = match base.host() {
            Some(url::Host::Domain(host)) => host.to_ascii_lowercase(),
            Some(url::Host::Ipv4(ip)) => ip.to_string(),
            Some(url::Host::Ipv6(ip)) => ip.to_string(),
            None => return Err(HttpsError::Address("the gateway has no host".into())),
        };
        let client = match endpoint.trust {
            Trust::Pinned => &self.pinned,
            Trust::Public => &self.public,
        };
        let mut connection = client.connect_https(&base).await?;
        let mut samples = [0_u64; PROBE_SAMPLES];
        let mut gateway_id = None;
        for sample in &mut samples {
            let nonce = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
            let started = tokio::time::Instant::now();
            let reply = connection
                .send_bounded(
                    Method::GET,
                    PROBE_PATH,
                    &[(PROBE_NONCE_HEADER, nonce.clone())],
                    Bytes::new(),
                    MAX_PROBE_REPLY,
                )
                .await?;
            let elapsed = started.elapsed();
            if reply.status != StatusCode::OK {
                return Err(HttpsError::Http(
                    "the gateway has no verified probe service".into(),
                ));
            }
            let observed = self.verify(&reply.body, &nonce, &host)?;
            if gateway_id.as_ref().is_some_and(|id| *id != observed) {
                return Err(HttpsError::Tls(
                    "the entrance changed gateways during its measurements".into(),
                ));
            }
            gateway_id = Some(observed);
            *sample = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        }
        samples.sort_unstable();
        Ok(GatewayMeasurement {
            gateway_id: gateway_id
                .ok_or_else(|| HttpsError::Http("no gateway was measured".into()))?,
            endpoint: endpoint.clone(),
            round_trip_ms: samples[PROBE_SAMPLES / 2],
        })
    }
}

#[async_trait]
impl GatewayProber for HttpsGatewayProber {
    async fn measure(&self, endpoint: &Endpoint) -> Result<GatewayMeasurement, DialError> {
        let timeout =
            Duration::from_secs(u64::from(orch_link_contract::session::CONNECT_TIMEOUT_SECS));
        tokio::time::timeout(timeout, self.measure_inner(endpoint))
            .await
            .map_err(|_| DialError::TimedOut)?
            .map_err(|error| match error {
                HttpsError::Tls(detail) => DialError::Tls(detail),
                HttpsError::TimedOut => DialError::TimedOut,
                other => DialError::Connect(other.to_string()),
            })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use orch_pki::ca::{CertificateAuthority, Holder};
    use orch_types::ManualClock;

    #[test]
    fn public_probe_proofs_are_bound_to_the_authority_gateway_host_and_nonce() {
        let manual = ManualClock::new(chrono::Utc.with_ymd_and_hms(2026, 11, 6, 8, 0, 0).unwrap());
        let clock: Arc<dyn Clock> = Arc::new(manual.clone());
        let own = CertificateAuthority::create("Fixture", 3650, clock.as_ref()).unwrap();
        let foreign =
            CertificateAuthority::create("Foreign fixture", 3650, clock.as_ref()).unwrap();
        let prober =
            HttpsGatewayProber::for_enrollment(own.certificate(), Arc::clone(&clock)).unwrap();
        let signed = |authority: &CertificateAuthority, service| {
            let (key, certificate) = authority
                .issue_with_new_key(
                    &Holder::Service {
                        service,
                        names: vec!["gw.test".into()],
                    },
                    365,
                    clock.as_ref(),
                )
                .unwrap();
            let input = probe_signing_input("nonce", "near", "gw.test");
            GatewayProbeReply {
                gateway_id: "near".into(),
                nonce: "nonce".into(),
                certificate_pem: certificate.pem(),
                signature: URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes())),
            }
        };
        let valid = signed(&own, ServiceName::Gateway);
        let body = serde_json::to_vec(&valid).unwrap();
        assert_eq!(prober.verify(&body, "nonce", "gw.test").unwrap(), "near");
        assert!(prober.verify(&body, "another-nonce", "gw.test").is_err());
        assert!(prober.verify(&body, "nonce", "other.test").is_err());
        for invalid in [
            signed(&foreign, ServiceName::Gateway),
            signed(&own, ServiceName::Worker),
        ] {
            assert!(
                prober
                    .verify(&serde_json::to_vec(&invalid).unwrap(), "nonce", "gw.test")
                    .is_err()
            );
        }
        let mut invalid = valid;
        invalid.signature = URL_SAFE_NO_PAD.encode([0; 64]);
        assert!(
            prober
                .verify(&serde_json::to_vec(&invalid).unwrap(), "nonce", "gw.test")
                .is_err()
        );
        manual.advance(chrono::Duration::days(366));
        assert!(prober.verify(&body, "nonce", "gw.test").is_err());
    }
}
