//! Compose the stateless gateway listeners, backplane and tunnel relay.
//!
//! Everything concrete is chosen here and nowhere else: the library below
//! this program takes what it is given.

use anyhow::Context;
use orch_build_info::BuildInfo;
use orch_config::ServerConfig;
use orch_gateway_core::{Gateway, GatewayParts, Settings};
use orch_pki::certificates::{Certificate, LeafKey};
use orch_pki::verification::TlsIdentity;
use orch_telemetry::LogFormat;
use orch_types::SystemClock;
use std::path::Path;
use std::sync::Arc;

/// The file of the authority's certificate in the secrets directory.
const CA_CERT_FILE: &str = "orch_ca_cert.pem";
/// The file of the gateway's key in the secrets directory.
const KEY_FILE: &str = "service_gateway_key.pem";
/// The file of the gateway's certificate in the secrets directory.
const CERT_FILE: &str = "service_gateway_cert.pem";

/// Runs the service until it is asked to stop.
pub async fn run(config_path: &Path, secrets_dir: &Path, build: BuildInfo) -> anyhow::Result<()> {
    let config: ServerConfig = orch_config::load(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    orch_telemetry::init_logging("gateway", LogFormat::named(&config.log.format));
    tracing::info!(%build, "starting");

    let read = |name: &str| {
        std::fs::read_to_string(secrets_dir.join(name)).with_context(|| format!("reading {name}"))
    };
    let authority =
        Certificate::from_pem(&read(CA_CERT_FILE)?).context("the authority's certificate")?;
    let identity = TlsIdentity {
        chain: vec![Certificate::from_pem(&read(CERT_FILE)?).context("the gateway's certificate")?],
        key: LeafKey::from_pem(&read(KEY_FILE)?).context("the gateway's key")?,
    };
    let gateway = Gateway::new(GatewayParts {
        settings: Settings::from_config(&config, build.version),
        identity,
        authority,
        clock: Arc::new(SystemClock),
    })
    .context("making the gateway")?;

    let pinned = tokio::net::TcpListener::bind(&config.gateway.pinned_bind)
        .await
        .with_context(|| format!("listening on {}", config.gateway.pinned_bind))?;
    let edge = tokio::net::TcpListener::bind(&config.gateway.edge_bind)
        .await
        .with_context(|| format!("listening on {}", config.gateway.edge_bind))?;
    tracing::info!(
        pinned = %config.gateway.pinned_bind,
        edge = %config.gateway.edge_bind,
        backplane = %config.gateway.backplane_url,
        "serving machines"
    );
    // What this process measures, for the operator's collector on `[gateway] metrics_bind`, for as
    // long as the gateway runs. A listener that cannot be had is said and the gateway runs on:
    // machines need the gateway, and nothing needs its measurements.
    let readings = orch_telemetry::install_metrics().context("keeping measurements")?;
    let (end_measurements, measurements_ended) = tokio::sync::oneshot::channel::<()>();
    let measurements = match tokio::net::TcpListener::bind(&config.gateway.metrics_bind).await {
        Ok(listener) => {
            tracing::info!(bind = %config.gateway.metrics_bind, "serving measurements");
            Some(tokio::spawn(orch_telemetry::metrics::serve(
                listener,
                readings,
                async move {
                    let _ = measurements_ended.await;
                },
            )))
        }
        Err(error) => {
            tracing::warn!(%error, bind = %config.gateway.metrics_bind, "the measurements are not served");
            None
        }
    };
    tokio::spawn({
        let gateway = gateway.clone();
        async move {
            stopped().await;
            gateway.shutdown();
        }
    });
    gateway.run(pinned, edge).await;
    let _ = end_measurements.send(());
    if let Some(task) = measurements {
        let _ = task.await;
    }
    tracing::info!("stopped");
    Ok(())
}

/// Resolves when the process is asked to stop.
async fn stopped() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let terminate = async {
            if let Ok(mut signal) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                signal.recv().await;
            }
        };
        tokio::select! { _ = interrupt => {}, () = terminate => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = interrupt.await;
    }
}
