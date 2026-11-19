//! Compose the API, engine, database, PKI, blob store, summarizers and identity providers.
//!
//! Everything concrete is chosen here and nowhere else: the libraries
//! below this program take what they are given.

use anyhow::Context;
use orch_api_core::backplane::{Link, LinkParts};
use orch_api_core::blobs::Blobs;
use orch_api_core::cache::Cache;
use orch_api_core::realtime::Realtime;
use orch_api_core::registry::Registry;
use orch_api_core::server;
use orch_api_core::state::AppState;
use orch_build_info::BuildInfo;
use orch_config::ServerConfig;
use orch_db::Db;
use orch_db::secrets::MasterKey;
use orch_notifier_email::EmailNotifier;
use orch_notifier_push::{PushNotifier, Vapid};
use orch_notifier_telegram::TelegramNotifier;
use orch_notifier_webhook::WebhookNotifier;
use orch_pki::ca::CaFiles;
use orch_pki::certificates::{Certificate, LeafKey};
use orch_pki::verification::TlsIdentity;
use orch_provider_contract::builtins::BuiltinSources;
use orch_telemetry::LogFormat;
use orch_types::{Clock, SystemClock};
use orch_worker_core::channels::Notifiers;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// The file of the master key in the secrets directory.
const MASTER_KEY_FILE: &str = "orch_master_key";
/// The file of the token the bundled node enrolls with.
const NODE_TOKEN_FILE: &str = "node_enrollment_token";
/// The file of the key that signs Web Push messages in the secrets directory.
const VAPID_KEY_FILE: &str = "orch_vapid_private_key";
/// How often the bundled node's token is tried again while there is nobody
/// yet to make it in the name of: the first administrator is a person doing
/// the first thing there is to do, and the try is one query.
const NODE_TOKEN_RETRY: Duration = Duration::from_secs(10);
/// How often the API writes its heartbeat: as often as the worker writes its
/// own (its `heartbeat` job), so that one reading of staleness fits every part.
const API_HEARTBEAT: Duration = Duration::from_secs(15);
/// How often the measured request times are summed up: what the Prometheus exporter does by
/// itself when it serves its own listener.
const MEASUREMENTS_UPKEEP: Duration = Duration::from_secs(5);
/// How often the API looks whether its own certificate is due: a certificate of a year is due for
/// four months of it, so an hour costs nothing and loses nothing.
const CERTIFICATE_LOOK: Duration = Duration::from_secs(3_600);
/// One public observation a second keeps the waiting page live without repeatedly querying the schema.
const MIGRATION_REPORT_SECONDS: u64 = 1;

/// What the API is given beside its configuration.
#[derive(Debug, Clone)]
pub struct Launch<'a> {
    /// The deployment's configuration file.
    pub config: &'a Path,
    /// The directory of the deployment's secrets.
    pub secrets_dir: &'a Path,
    /// Whether to serve the browser API without TLS, for development.
    pub plain_http: bool,
}

/// What the secrets directory gives: the authority, the key that seals what
/// is stored, and the API's own certificate.
struct Secrets {
    authority: CaFiles,
    master_key: MasterKey,
    identity: TlsIdentity,
}

fn read_secrets(dir: &Path) -> anyhow::Result<Secrets> {
    let authority = CaFiles::in_dir(dir);
    let master = orch_config::read_secret_hex::<32>(&dir.join(MASTER_KEY_FILE))
        .context("reading the master key")?;
    let read = |name: &str| {
        std::fs::read_to_string(dir.join(name)).with_context(|| format!("reading {name}"))
    };
    let key = LeafKey::from_pem(&read("service_api_key.pem")?).context("the API's key")?;
    let certificate =
        Certificate::from_pem(&read("service_api_cert.pem")?).context("the API's certificate")?;
    Ok(Secrets {
        authority,
        master_key: MasterKey::new(master),
        identity: TlsIdentity {
            chain: vec![certificate],
            key,
        },
    })
}

/// The notifiers of this deployment, which the requests that set channels up
/// and try them know: the same ones the worker delivers through. Web Push is
/// among them where the deployment has a key for it.
fn notifiers(config: &ServerConfig, secrets_dir: &Path) -> anyhow::Result<Notifiers> {
    let mut notifiers = Notifiers::new();
    notifiers.register(Arc::new(EmailNotifier::new()))?;
    notifiers.register(Arc::new(WebhookNotifier::new()))?;
    notifiers.register(Arc::new(TelegramNotifier::new()))?;
    let key_file = secrets_dir.join(VAPID_KEY_FILE);
    if key_file.is_file() {
        match web_push(config, &key_file) {
            Ok(push) => {
                // Meta tells browsers the public half; the private half stays with the notifier.
                orch_api_core::routes::notifications::install_push_public_key(
                    push.public_key().to_owned(),
                );
                notifiers.register(Arc::new(push))?;
            }
            // A key that is wrong takes Web Push away, not the mail and the bots.
            Err(error) => {
                tracing::error!(error = format!("{error:#}"), "Web Push is not available")
            }
        }
    }
    Ok(notifiers)
}

/// Web Push signed with the deployment's key, which is told to a push service
/// together with the address of the system.
/// The built-in sources of a key's facts this program carries, as the connector's program registers them too:
/// the API names a provider's default source, offers the built-ins and checks a choice of one from this table.
fn builtin_sources() -> anyhow::Result<Arc<BuiltinSources>> {
    Ok(Arc::new(BuiltinSources::new([
        orch_provider_openrouter::builtin(),
        orch_provider_deepseek::builtin(),
    ])?))
}

fn web_push(config: &ServerConfig, key_file: &Path) -> anyhow::Result<PushNotifier> {
    let secret = orch_config::read_secret(key_file)?;
    let key = std::str::from_utf8(secret.expose()).context("the key is not text")?;
    let vapid = Vapid::read(key, &config.system.public_url).context(
        "the key, with [system] public_url as the sender's address, which must start with https://",
    )?;
    Ok(PushNotifier::new(vapid))
}

/// Runs the service until it is asked to stop.
pub async fn run(launch: &Launch<'_>, build: BuildInfo) -> anyhow::Result<()> {
    let config: ServerConfig = orch_config::load(launch.config)
        .with_context(|| format!("reading {}", launch.config.display()))?;
    orch_telemetry::init_logging("api", LogFormat::named(&config.log.format));
    tracing::info!(%build, "starting");
    let readings = orch_telemetry::install_metrics().context("keeping measurements")?;

    anyhow::ensure!(
        !config.database.url.is_empty(),
        "database.url is not set (give it as ORCH_DATABASE_URL)"
    );
    // Request connections: the database ends a session idle in a transaction, and cancels a stuck statement.
    let db = Db::connect_with_limits(
        &config.database.url,
        config.api.db_pool,
        Duration::from_millis(config.database.slow_statement_ms),
        Duration::from_millis(config.database.slow_acquire_ms),
        orch_db::pool::Limits::REQUESTS,
    )
    .await
    .context("connecting to the database")?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let status_file = std::env::var_os("ORCH_MIGRATION_STATUS_FILE").map(std::path::PathBuf::from);
    let report_interval = std::env::var("ORCH_MIGRATION_STATUS_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(MIGRATION_REPORT_SECONDS);
    orch_db::migrations::apply_reporting(&db, Path::new(&config.database.migrations_dir), clock.as_ref(),
        Duration::from_secs(report_interval), |progress| {
            tracing::info!(step=%progress.step, migration=?progress.migration, rows_done=?progress.rows_done,
                elapsed_seconds=progress.elapsed_seconds, "database update progress");
            if let Some(path) = &status_file {
                let saved = (|| -> anyhow::Result<()> {
                    let parent = path.parent().context("the migration progress file needs a directory")?;
                    std::fs::create_dir_all(parent)?;
                    let temporary = path.with_extension(format!("{}.tmp",std::process::id()));
                    std::fs::write(&temporary, serde_json::to_vec(progress)?)?;
                    // Only coarse public activity reaches the edge, which runs as its own user.
                    #[cfg(unix)]
                    std::fs::set_permissions(&temporary, {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::Permissions::from_mode(0o644)
                    })?;
                    std::fs::rename(temporary, path)?;
                    Ok(())
                })();
                if let Err(error) = saved { tracing::warn!(%error, "migration progress could not be published"); }
            }
        })
        .await
        .context("applying the migrations")?;
    // When this process started, as its heartbeat says it.
    let started_at = orch_types::Time::new(clock.now());
    {
        let mut conn = db.acquire().await?;
        orch_db::migrations::ensure_partitions(&mut conn, clock.now())
            .await
            .context("making partitions")?;
    }

    first_start(&db, clock.as_ref())
        .await
        .context("writing what a fresh system starts with")?;

    // Without TLS the API serves for development on one machine, and may do
    // so without the secrets of a deployment: it then has no machines.
    let secrets = if launch.plain_http && !launch.secrets_dir.is_dir() {
        tracing::warn!(
            dir = %launch.secrets_dir.display(),
            "there are no secrets; machines cannot connect"
        );
        None
    } else {
        Some(read_secrets(launch.secrets_dir).context("reading the secrets")?)
    };
    if let Some(secrets) = &secrets {
        // Signing in, the settings and the providers seal what they store with the same key.
        orch_api_core::routes::install_master_key(&secrets.master_key);
        // Management lists the services' certificates from the directory they were read from.
        orch_api_core::routes::admin::system::install_secrets_dir(launch.secrets_dir);
        // The key that signs who a port's caller is: the deployment's own, which compose-secrets
        // makes beside the set. Without it this process signs with a key of its own, which nobody
        // can verify after a restart.
        let port_key = launch.secrets_dir.join(orch_pki::port_assertions::KEY_FILE);
        if port_key.exists() {
            let secret = orch_config::read_secret_hex::<32>(&port_key)
                .context("reading the port assertion key")?;
            orch_api_core::routes::port_entry::install_port_signer(
                orch_pki::port_assertions::PortAssertionSigner::from_secret(secret),
            );
        }
    }
    let config = Arc::new(config);
    // Signing in through an identity provider speaks OpenID Connect.
    orch_api_core::routes::auth::oidc::install_kind(Arc::new(
        orch_identity_oidc::OidcKind::new().context("the client of the identity providers")?,
    ));
    // The ways out of the application that channels and destinations can be made from.
    orch_api_core::routes::notifications::install_notifiers(
        notifiers(&config, launch.secrets_dir).context("the notifiers")?,
    );
    let link = match &secrets {
        Some(secrets) => Link::new(LinkParts {
            db: db.clone(),
            clock: Arc::clone(&clock),
            config: Arc::clone(&config),
            release_version: build.version.to_owned(),
            authority: Arc::new(secrets.authority.load().context("loading the authority")?),
            master_key: secrets.master_key.clone(),
        }),
        None => Link::detached(db.clone(), Arc::clone(&clock), Arc::clone(&config))
            .context("making a link without machines")?,
    };
    link.start().await.context("starting the link")?;
    match orch_worker_core::backups::reconcile(
        &db,
        &config.installation.records_dir,
        &link.certificates().authority_certificate().fingerprint(),
        clock.time(),
    )
    .await
    {
        Ok((_, Some(reason))) => {
            tracing::warn!(%reason, "the local backup record was not imported")
        }
        Ok((_, None)) => {}
        Err(error) => tracing::warn!(%error, "the local backup record could not be reconciled"),
    }
    let stop = CancellationToken::new();
    tokio::spawn(heartbeat(
        db.clone(),
        Arc::clone(&clock),
        build.version.to_owned(),
        started_at,
        stop.clone(),
    ));
    if secrets.is_some() {
        let token_file = launch.secrets_dir.join(NODE_TOKEN_FILE);
        if token_file.is_file() {
            tokio::spawn(register_node_token(link.clone(), token_file, stop.clone()));
        }
    }

    let bind = config.api.bind.clone();
    let internal_bind = config.api.internal_bind.clone();
    // Open the shipped local store before starting any resumed thread work. Missing or unwritable
    // storage is a startup error, not an immutable unavailable recording for each pending turn.
    let store = orch_blob_local::LocalBlobStore::open(&config.storage.root)
        .await
        .with_context(|| format!("opening the store of content in {}", config.storage.root))?;
    let blobs = Blobs::new(Arc::new(store), Path::new(&config.storage.root));
    let realtime = Realtime::new(&db, &clock, &config);
    let engine = orch_api_core::thread_engine::start_with_blobs(
        &db,
        &clock,
        &config,
        &link,
        realtime.sink(),
        blobs.store(),
    );
    realtime.start(engine.clone());
    // The last output of a person's shells is put away in it too, with the environment's secrets taken out.
    orch_api_core::recordings::install(
        blobs.store(),
        db.clone(),
        secrets.as_ref().map(|each| each.master_key.clone()),
    );
    let cache = Cache::new(&config);
    let sampling = cache.collect_samples(&link);
    let app = Arc::new(AppState {
        host_ports: Default::default(),
        db,
        config: Arc::clone(&config),
        clock: Arc::clone(&clock),
        link: link.clone(),
        engine: engine.clone(),
        realtime: realtime.clone(),
        build,
        blobs,
        cache,
        builtin_sources: builtin_sources()?,
    });
    let housekeeping = orch_api_core::cache::start_housekeeping(&app, &stop);
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    tracing::info!(requests = registry.bound().len(), "requests bound");
    let router = server::router(Arc::clone(&app), registry);

    tokio::spawn({
        let stop = stop.clone();
        async move {
            stopped().await;
            stop.cancel();
        }
    });

    let browser = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("listening on {bind}"))?;
    let mut serving = tokio::task::JoinSet::new();
    // What this process measures, for the operator's collector on `[api] metrics_bind`, for as
    // long as the API runs. A listener that cannot be had is said and the API runs on: people
    // need the API, and nothing needs its measurements.
    tokio::spawn(orch_worker_core::heartbeat::keep_measurements(
        app.db.clone(),
        Arc::clone(&clock),
        readings.clone(),
        "api",
        config.metrics.store,
        stop.clone(),
    ));
    tokio::spawn({
        let engine = engine.clone();
        let stop = stop.clone();
        async move {
            let mut every = tokio::time::interval(Duration::from_secs(60));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {()=stop.cancelled()=>return,_=every.tick()=>{}}
                if let Err(error) = engine.measure_caps().await {
                    tracing::debug!(%error,"cap measurements could not be refreshed");
                }
            }
        }
    });
    // The agent programs' release channels, read on the schedule `agents.harness_channel_hours` gives.
    tokio::spawn(orch_api_core::harness_channels::keep_reading(
        app.clone(),
        stop.clone(),
    ));
    // The times of requests are kept as samples until they are summed up: summed up every few
    // seconds, they hold no more than that many seconds of requests whether or not anybody reads.
    tokio::spawn({
        let readings = readings.clone();
        let stop = stop.clone();
        async move {
            let mut every = tokio::time::interval(MEASUREMENTS_UPKEEP);
            loop {
                tokio::select! {
                    () = stop.cancelled() => return,
                    _ = every.tick() => readings.upkeep(),
                }
            }
        }
    });
    match tokio::net::TcpListener::bind(&config.api.metrics_bind).await {
        Ok(listener) => {
            tracing::info!(bind = %config.api.metrics_bind, "serving measurements");
            let stop = stop.clone();
            serving.spawn(async move {
                orch_telemetry::metrics::serve(listener, readings, async move {
                    stop.cancelled().await;
                })
                .await
            });
        }
        Err(error) => {
            tracing::warn!(%error, bind = %config.api.metrics_bind, "the measurements are not served");
        }
    }
    if std::env::var_os("ORCH_FRONTEND_HTTPS_BIND").is_some_and(|value| !value.is_empty()) {
        anyhow::ensure!(
            secrets.is_some(),
            "Orch HTTPS needs the existing authority in the secrets directory"
        );
        app.link
            .certificates()
            .keep_frontend_certificate(&config)
            .context("the web edge’s certificate")?;
    }
    // The API's own certificate as its listeners show it, renewed in place while it runs.
    let identity = secrets
        .as_ref()
        .map(|secrets| orch_pki::service_renew::LiveIdentity::new(&secrets.identity))
        .transpose()
        .context("the API's certificate")?;
    let renewing = identity.as_ref().map(|identity| {
        tokio::spawn(keep_certificate(
            Arc::clone(&app),
            Arc::clone(identity),
            Arc::clone(&clock),
            stop.clone(),
        ))
    });
    match (&identity, launch.plain_http) {
        (_, true) => {
            tracing::info!(%bind, "serving without TLS, for development only");
            let stop = stop.clone();
            serving.spawn(async move {
                server::serve_plain(browser, router, async move { stop.cancelled().await }).await
            });
        }
        (Some(identity), false) => {
            let tls = server::browser_tls_config_live(Arc::clone(identity), Arc::clone(&clock))
                .context("the browser listener's TLS")?;
            tracing::info!(%bind, "serving under the system's authority");
            serving.spawn(server::serve_tls(browser, tls, router, stop.clone()));
        }
        (None, false) => anyhow::bail!("the secrets are needed to serve under TLS"),
    }
    if let (Some(secrets), Some(identity)) = (&secrets, &identity) {
        let authority = secrets.authority.load().context("loading the authority")?;
        let tls = server::internal_tls_config_live(
            Arc::clone(identity),
            authority.certificate(),
            Arc::clone(&clock),
        )
        .context("the internal listener's TLS")?;
        let listener = tokio::net::TcpListener::bind(&internal_bind)
            .await
            .with_context(|| format!("listening on {internal_bind}"))?;
        tracing::info!(bind = %internal_bind, "serving the gateways and the machines they hand over");
        let internal = server::internal_router(Arc::clone(&app), orch_api_core::content::routes());
        serving.spawn(server::serve_tls(listener, tls, internal, stop.clone()));
    }
    while let Some(ended) = serving.join_next().await {
        ended.context("a listener failed")??;
    }
    realtime.stop().await;
    engine.stop();
    link.shutdown();
    sampling.abort();
    for task in housekeeping {
        let _ = task.await;
    }
    if let Some(task) = renewing {
        let _ = task.await;
    }
    tracing::info!("stopped");
    Ok(())
}

/// The adapters this build has built in: every program of the registry. Each gets a profile that people then
/// change or disable like any other.
fn adapters() -> Vec<orch_db::seeds::BuiltInAdapter<'static>> {
    orch_agent_programs::Programs::builtin()
        .programs()
        .map(|manifest| orch_db::seeds::BuiltInAdapter {
            kind: manifest.kind.as_str(),
            title: manifest.name(),
            colocated: manifest.placement.colocated,
        })
        .collect()
}

/// Where a release keeps the rate catalogue it comes with, beside its
/// programs; and where a checkout of the repository has it.
const CATALOGUE_PLACES: &[&str] = &["../share/model-rates.json", "../../deploy/model-rates.json"];

/// Writes what a system that was just made starts with, and brings up to
/// date what a release replaces: capability definitions, the two
/// templates, a profile for each built-in adapter, the system's defaults
/// page as it ships, and the rate catalogue of the release. Everything is
/// data that people change afterwards; nothing they changed is written
/// over. Services that start together do it once.
async fn first_start(db: &Db, clock: &dyn Clock) -> anyhow::Result<()> {
    let now = clock.now();
    let adapters = adapters();
    let release = orch_db::seeds::Release {
        version: env!("ORCH_RELEASE_VERSION"),
        adapters: &adapters,
    };
    let mut tx = db.begin().await?;
    let seeded = orch_db::seeds::seed(&mut tx, &release, now).await?;
    let defaults = orch_db::seeds::system_defaults(&mut tx, now).await?;
    let catalogue = std::env::current_exe().ok().and_then(|program| {
        let dir = program.parent()?.to_owned();
        CATALOGUE_PLACES
            .iter()
            .map(|place| dir.join(place))
            .chain(std::iter::once(PathBuf::from("deploy/model-rates.json")))
            .find(|path| path.is_file())
    });
    let rates = match &catalogue {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            Some(orch_db::seeds::load_shipped_catalogue(&mut tx, &text, now).await?)
        }
        None => None,
    };
    tx.commit().await?;
    tracing::info!(
        ?seeded,
        defaults,
        ?rates,
        "a fresh system's data is in place"
    );
    Ok(())
}

/// Writes, every [`API_HEARTBEAT`] until `stop`, that this API process is
/// alive: a run of its own, its release and the moment it started, so that the
/// system page and `orchctl doctor` read the API's row as they read the
/// worker's and the gateway's. A beat that cannot be written is logged and the
/// next one tried.
async fn heartbeat(
    db: Db,
    clock: Arc<dyn Clock>,
    version: String,
    started_at: orch_types::Time,
    stop: CancellationToken,
) {
    let run = orch_types::new_id();
    let mut every = tokio::time::interval(API_HEARTBEAT);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = every.tick() => {}
        }
        let now = clock.now();
        let written = async {
            let mut conn = db.acquire().await?;
            orch_db::heartbeat::beat(&mut conn, "api", run, &version, started_at.get(), now)
                .await?;
            orch_worker_core::gauges::operational(&mut conn, now).await
        }
        .await;
        if let Err(error) = written {
            tracing::warn!(%error, "the API's heartbeat could not be written");
        }
    }
}

/// Registers the token of the bundled node, and waits for the first
/// administrator if there is none yet, so that a system that has just been
/// started needs no second start to have its node.
/// Keeps the API's own certificate current until `stop`: once two thirds of its life have passed,
/// the API issues itself another from the authority beside it, which its listeners show from their
/// next handshake while the connections that stand keep theirs. A failure is tried again at the
/// next look, while the certificate still holds.
async fn keep_certificate(
    app: Arc<AppState>,
    identity: Arc<orch_pki::service_renew::LiveIdentity>,
    clock: Arc<dyn Clock>,
    stop: CancellationToken,
) {
    use orch_pki::service_renew::{RenewFault, adopt};
    let mut look = tokio::time::interval(CERTIFICATE_LOOK);
    look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = look.tick() => {}
        }
        if std::env::var_os("ORCH_FRONTEND_HTTPS_BIND").is_some_and(|value| !value.is_empty())
            && let Err(error) = app
                .link
                .certificates()
                .keep_frontend_certificate(&app.config)
        {
            tracing::warn!(%error, "the web edge’s certificate could not be renewed");
        }
        let Some(current) = identity.certificate() else {
            continue;
        };
        if !orch_pki::renewal::due(&current, clock.time()) {
            continue;
        }
        let renewed = app
            .link
            .certificates()
            .issue_service(orch_pki::certificates::ServiceName::Api)
            .map_err(RenewFault::from)
            .and_then(|(key, certificate)| adopt(&identity, &current, key, &certificate.pem()));
        match renewed {
            Ok(()) => tracing::info!("the API's certificate is renewed"),
            Err(error) => tracing::warn!(
                %error,
                "the API's certificate could not be renewed; it tries again in an hour"
            ),
        }
    }
}

async fn register_node_token(link: Link, file: PathBuf, stop: CancellationToken) {
    use orch_api_core::certificates::Adopted;
    loop {
        match link.certificates().adopt_token_file(&file).await {
            Ok(Adopted::WaitingForAdministrator) => {
                tracing::info!("the token of the bundled node waits for the first administrator");
            }
            Ok(adopted) => {
                tracing::info!(?adopted, "the token of the bundled node");
                return;
            }
            Err(fault) => {
                tracing::warn!(%fault.detail, "the token of the bundled node was not registered");
                return;
            }
        }
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(NODE_TOKEN_RETRY) => {}
        }
    }
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
    use orch_provider_contract::choice::{Keeper, SourceElement, builtin, default_source};

    #[test]
    fn the_program_registers_each_vendor_reader_and_a_key_at_its_address_reads_it_by_default() {
        let table = builtin_sources().unwrap();
        assert_eq!(
            table.iter().map(|entry| entry.name).collect::<Vec<_>>(),
            builtin::KEY_FACTS
        );
        for (address, name) in [
            ("https://openrouter.ai/api/v1", builtin::OPENROUTER),
            ("https://api.deepseek.com/anthropic", builtin::DEEPSEEK),
        ] {
            let endpoints = [url::Url::parse(address).unwrap()];
            assert_eq!(
                default_source(SourceElement::Facts, Keeper::Key, &endpoints, &table),
                Some(name)
            );
            assert_eq!(
                default_source(SourceElement::Facts, Keeper::NoKey, &endpoints, &table),
                None,
                "a vendor's read needs the key"
            );
            assert_eq!((table.get(name).unwrap().descriptor)().name, name);
        }
    }
}
