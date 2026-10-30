//! The gateway's own certificate, renewed while it runs.
//!
//! Once two thirds of its certificate's life have passed, the gateway makes a new key and asks the system to
//! certify it (`POST /internal/pki/renew` on the API's internal listener, over the mutual-TLS identity it shows
//! now). Its listeners and its connections to the system show the new certificate from their next handshake;
//! what stands goes on. It looks when it starts and every hour after, and after a failure again at the next look,
//! while its certificate still holds.

use std::time::Duration;

use http::StatusCode;
use orch_link_contract::enrollment::{RenewReply, RenewRequest};
use orch_pki::service_renew::{RenewFault, adopt};

use crate::Gateway;

/// How often the gateway looks whether its certificate is due: a certificate of a year is due for four months
/// of it, so an hour costs nothing and loses nothing.
pub const LOOK_EVERY: Duration = Duration::from_secs(3_600);
/// The API's route that certifies a service's new key.
pub const RENEW_PATH: &str = "/internal/pki/renew";
/// The most an answer may hold: a certificate and the authority's.
const ANSWER_BYTES: usize = 64 * 1024;

/// Keeps the gateway's certificate current until the gateway stops.
pub(crate) async fn keep(gateway: Gateway) {
    let mut look = tokio::time::interval(LOOK_EVERY);
    look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = gateway.inner.shutdown.cancelled() => return,
            _ = look.tick() => {}
        }
        match renew_now(&gateway).await {
            Ok(true) => tracing::info!("the gateway's certificate is renewed"),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, "the gateway's certificate could not be renewed; it tries again in an hour");
            }
        }
    }
}

/// Renews the gateway's certificate where it is due now. Returns whether it renewed.
pub async fn renew_now(gateway: &Gateway) -> Result<bool, RenewFault> {
    let now = gateway.inner.clock.time();
    let current = gateway
        .inner
        .identity
        .certificate()
        .ok_or(RenewFault::Wrong("the gateway shows no certificate"))?;
    if !orch_pki::renewal::due(&current, now) {
        return Ok(false);
    }
    // Keep the request future concrete: the spawned renewal loop must carry a Send future,
    // including the lifetime of the gateway borrowed by its TLS request.
    let (key, csr_pem) = orch_pki::renewal::start()?;
    let pem = ask(gateway, csr_pem).await.map_err(RenewFault::Refused)?;
    adopt(&gateway.inner.identity, &current, key, &pem)?;
    Ok(true)
}

/// Has the system certify the key of `csr_pem`; its certificate as PEM.
async fn ask(gateway: &Gateway, csr_pem: String) -> Result<String, String> {
    let body = serde_json::to_vec(&RenewRequest { csr_pem }).map_err(|error| error.to_string())?;
    let (status, answer) =
        crate::content::ask_system(gateway, RENEW_PATH, body, ANSWER_BYTES).await?;
    if status != StatusCode::OK {
        return Err(format!(
            "the system answered {status}: {}",
            String::from_utf8_lossy(&answer)
        ));
    }
    let reply: RenewReply = serde_json::from_slice(&answer)
        .map_err(|error| format!("the answer does not read: {error}"))?;
    Ok(reply.certificate_pem)
}
