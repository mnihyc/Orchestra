//! Compose database upkeep and notification channels; no machine links.
//!
//! Everything concrete is chosen here and nowhere else: which notifiers there
//! are, where the key that opens what is sealed in the database comes from, and
//! where the measurements are served. The libraries below this program take
//! what they are given.

use anyhow::Context;
use axum::http::header;
use axum::routing::get;
use orch_build_info::BuildInfo;
use orch_config::ServerConfig;
use orch_db::Db;
use orch_db::secrets::MasterKey;
use orch_notifier_email::EmailNotifier;
use orch_notifier_push::{PushNotifier, Vapid};
use orch_notifier_telegram::TelegramNotifier;
use orch_notifier_webhook::WebhookNotifier;
use orch_telemetry::{LogFormat, Readings};
use orch_types::{Clock, SystemClock};
use orch_worker_core::channels::Notifiers;
use orch_worker_core::heartbeat::{Identity, Metrics};
use orch_worker_core::reconciliation;
use orch_worker_core::schedule::{self, Services};
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// The file of the master key in the secrets directory.
const MASTER_KEY_FILE: &str = "orch_master_key";
/// The file of the key that signs Web Push messages in the secrets directory.
const VAPID_KEY_FILE: &str = "orch_vapid_private_key";
/// The type of the text a collector reads.
const EXPOSITION_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
/// The migration to wait for when the files of the release cannot be read: any,
/// since a database that has applied nothing has no tables to work on.
const ANY_MIGRATION: i64 = 1;

/// What the worker is given beside its configuration.
#[derive(Debug, Clone)]
pub struct Launch<'a> {
    /// The deployment's configuration file.
    pub config: &'a Path,
    /// The directory of the deployment's secrets.
    pub secrets_dir: &'a Path,
}

/// The number of the newest migration in `dir`, whose files are named
/// `<number>_<name>.sql`.
fn newest_migration(dir: &Path) -> anyhow::Result<i64> {
    let mut newest = None;
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let name = entry?.file_name();
        let number = name
            .to_str()
            .and_then(|name| name.strip_suffix(".sql"))
            .and_then(|stem| stem.split('_').next())
            .and_then(|number| number.parse::<i64>().ok());
        newest = newest.max(number);
    }
    newest.with_context(|| format!("{} holds no migrations", dir.display()))
}

/// The notifiers of this deployment. Web Push is among them where the
/// deployment has a key for it.
fn notifiers(config: &ServerConfig, secrets_dir: &Path) -> anyhow::Result<Notifiers> {
    let mut notifiers = Notifiers::new();
    notifiers.register(Arc::new(EmailNotifier::new()))?;
    notifiers.register(Arc::new(WebhookNotifier::new()))?;
    notifiers.register(Arc::new(TelegramNotifier::new()))?;
    match web_push(config, secrets_dir) {
        Ok(Some(push)) => notifiers.register(Arc::new(push))?,
        Ok(None) => tracing::info!("there is no key for Web Push, so browsers are not notified"),
        // A key that is wrong takes Web Push away, not the mail and the bots.
        Err(error) => tracing::error!(error = format!("{error:#}"), "Web Push is not available"),
    }
    Ok(notifiers)
}

/// Web Push signed with the deployment's key, which is told to a push service
/// together with the address of the system.
fn web_push(config: &ServerConfig, secrets_dir: &Path) -> anyhow::Result<Option<PushNotifier>> {
    let file = secrets_dir.join(VAPID_KEY_FILE);
    if !file.is_file() {
        return Ok(None);
    }
    let secret = orch_config::read_secret(&file)?;
    let key = std::str::from_utf8(secret.expose()).context("the key is not text")?;
    let vapid = Vapid::read(key, &config.system.public_url).context(
        "the key, with [system] public_url as the sender's address, which must start with https://",
    )?;
    Ok(Some(PushNotifier::new(vapid)))
}

/// What the process has measured, for a collector to read.
fn measurements(readings: Readings) -> axum::Router {
    axum::Router::new().route(
        "/metrics",
        get(move || {
            let readings = readings.clone();
            async move { ([(header::CONTENT_TYPE, EXPOSITION_TYPE)], readings.render()) }
        }),
    )
}

/// Serves the measurements until `stop`.
async fn serve_measurements(
    listener: tokio::net::TcpListener,
    readings: Readings,
    stop: CancellationToken,
) -> anyhow::Result<()> {
    axum::serve(listener, measurements(readings))
        .with_graceful_shutdown(async move { stop.cancelled().await })
        .await
        .context("serving the measurements")
}

/// Runs the service until it is asked to stop.
pub async fn run(launch: &Launch<'_>, build: BuildInfo) -> anyhow::Result<()> {
    let config: ServerConfig = orch_config::load(launch.config)
        .with_context(|| format!("reading {}", launch.config.display()))?;
    orch_telemetry::init_logging("worker", LogFormat::named(&config.log.format));
    tracing::info!(%build, "starting");

    anyhow::ensure!(
        !config.database.url.is_empty(),
        "database.url is not set (give it as ORCH_DATABASE_URL)"
    );
    let master = MasterKey::new(
        orch_config::read_secret_hex::<32>(&launch.secrets_dir.join(MASTER_KEY_FILE))
            .context("reading the master key")?,
    );
    let notifiers = notifiers(&config, launch.secrets_dir)?;
    tracing::info!(
        notifiers = ?notifiers.descriptors().iter().map(|each| each.name.as_str()).collect::<Vec<_>>(),
        "the ways out of the application"
    );

    let readings = orch_telemetry::install_metrics().context("keeping measurements")?;
    let stop = CancellationToken::new();
    tokio::spawn({
        let stop = stop.clone();
        async move {
            stopped().await;
            stop.cancel();
        }
    });
    let bind = config.worker.metrics_bind.clone();
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("listening on {bind}"))?;
    tracing::info!(%bind, "serving measurements");
    let serving = tokio::spawn(serve_measurements(listener, readings.clone(), stop.clone()));

    let db = Db::connect_with_slow_thresholds(
        &config.database.url,
        config.worker.db_pool,
        std::time::Duration::from_millis(config.database.slow_statement_ms),
        std::time::Duration::from_millis(config.database.slow_acquire_ms),
    )
    .await
    .context("connecting to the database")?;
    // The API applies the schema; a worker that started first waits for it.
    let migrations = Path::new(&config.database.migrations_dir);
    let shipped = newest_migration(migrations).unwrap_or_else(|error| {
        tracing::warn!(
            error = format!("{error:#}"),
            "the migrations of the release cannot be read"
        );
        ANY_MIGRATION
    });
    if !reconciliation::wait_for_schema(&db, shipped, reconciliation::SCHEMA_POLL, &stop)
        .await
        .context("looking for the schema")?
    {
        tracing::info!("stopped before the schema was ready");
        return Ok(());
    }
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    {
        let mut conn = db.acquire().await?;
        orch_db::migrations::ensure_partitions(&mut conn, clock.now())
            .await
            .context("making partitions")?;
    }

    let who = Identity::begin(build.version, &*clock);
    tokio::spawn(orch_worker_core::heartbeat::keep_measurements(
        db.clone(),
        Arc::clone(&clock),
        readings,
        "worker",
        config.metrics.store,
        stop.clone(),
    ));
    let ran = schedule::run(
        Services {
            db,
            clock,
            config: Arc::new(config),
            notifiers,
            master,
            who,
            metrics: Arc::new(Metrics::new()),
        },
        stop.clone(),
    )
    .await;
    stop.cancel();
    serving.await.context("the measurements listener")??;
    ran.context("the jobs")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[test]
    fn the_newest_migration_is_the_highest_number_among_the_files_and_other_files_are_not_migrations()
     {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            newest_migration(dir.path()).is_err(),
            "a directory without migrations is a fault"
        );
        for name in [
            "0001_initial.sql",
            "0012_later.sql",
            "0003_between.sql",
            "README.md",
            "notes.sql",
            "x_1.sql",
        ] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        assert_eq!(newest_migration(dir.path()).unwrap(), 12);
        assert!(newest_migration(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn web_push_is_there_only_with_a_key_and_an_https_address_to_sign_it_with() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::default();
        config.system.public_url = "https://orch.example".to_owned();
        assert!(
            web_push(&config, dir.path()).unwrap().is_none(),
            "no key, no Web Push"
        );
        let key = "IQ9Ur0ykXoHS9gzfYX0aBjy9lvdrjx_PFUXmie9YRcY";
        std::fs::write(dir.path().join(VAPID_KEY_FILE), format!("{key}\n")).unwrap();
        let push = web_push(&config, dir.path())
            .unwrap()
            .expect("a key is enough");
        assert_eq!(
            push.public_key(),
            "BMjQIp55pdbU8pfCBKyXcZjlmER_mXt5LqNrN1hrXbdBS5EnhIbMu3Au-RV53iIpztzNXkGI56BFB1udQ8Bq_H4"
        );
        config.system.public_url = "http://localhost:8080".to_owned();
        assert!(
            web_push(&config, dir.path()).is_err(),
            "a push service is told who sends; an address without TLS says nothing"
        );
        std::fs::write(dir.path().join(VAPID_KEY_FILE), "not a key").unwrap();
        config.system.public_url = "https://orch.example".to_owned();
        assert!(web_push(&config, dir.path()).is_err());
        let all = notifiers(&config, dir.path()).unwrap();
        let names: Vec<_> = all
            .descriptors()
            .into_iter()
            .map(|each| each.name)
            .collect();
        assert_eq!(
            names,
            ["email", "telegram", "webhook"],
            "a wrong key takes Web Push away and nothing else"
        );
    }

    #[tokio::test]
    async fn the_measurements_are_served_as_text_for_a_collector() {
        let readings = orch_telemetry::install_metrics().unwrap();
        metrics_for_the_test();
        let answer = measurements(readings)
            .oneshot(
                axum::http::Request::get("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(answer.status(), 200);
        assert_eq!(answer.headers()[header::CONTENT_TYPE], EXPOSITION_TYPE);
        let body = answer.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains(
                "orch_route_messages_total{channel=\"email\",form=\"single\",result=\"sent\"} 1"
            ),
            "{text}"
        );
    }

    fn metrics_for_the_test() {
        Metrics::new().route_message(
            "email",
            orch_notifier_contract::templates::Form::Single,
            "sent",
        );
    }
}
