//! Effective browser addresses are a committed system-store read, never a service's startup string.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::state::App;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::public_addresses::{
    PublicAddressProof, PublicAddressSnapshot, PublicAddresses,
};
use orch_db::Conn;
use std::time::Duration;
use tokio::time::Instant;

/// This marker is loaded by the frontend's worker configuration, not returned from a settings write.
pub const FRONTEND_GENERATION_HEADER: &str = "x-orch-frontend-generation";

/// Deployment-owned preparation parameters. The public API never accepts a frontend control address.
#[derive(Debug, Clone)]
pub struct PrepareOptions {
    pub managed_frontend: Option<String>,
    pub timeout: Duration,
    pub poll: Duration,
}

impl PrepareOptions {
    /// The deployment, never the request body, chooses the managed service's fixed control origin.
    pub async fn read(db: &orch_db::Db, managed: bool) -> ApiResult<Self> {
        use orch_settings::{Place, Snapshot, Wanted};
        use orch_settings_contract::registry;
        let definitions = [
            registry::get("public_address.prepare_timeout_secs")
                .ok_or_else(|| ApiError::internal("the address wait is not declared"))?,
            registry::get("public_address.probe_interval_ms")
                .ok_or_else(|| ApiError::internal("the address probe interval is not declared"))?,
        ];
        let mut conn = db.acquire().await?;
        let snapshot = Snapshot::load(&mut conn, Place::System, Wanted::Keys(&definitions))
            .await
            .map_err(ApiError::internal)?;
        let number = |key| -> ApiResult<u64> {
            let value: i64 = crate::sign_in::setting(&snapshot, key)?
                .ok_or_else(|| ApiError::internal("an address preparation value is missing"))?;
            u64::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| ApiError::internal("an address preparation wait is not positive"))
        };
        Ok(Self {
            managed_frontend: managed.then(|| "https://frontend:443".to_owned()),
            timeout: Duration::from_secs(number("public_address.prepare_timeout_secs")?),
            poll: Duration::from_millis(number("public_address.probe_interval_ms")?),
        })
    }
}

/// A successful browser proof still owns serialization until its caller commits or drops it.
pub struct Prepared {
    publication: orch_db::queries::public_addresses::PublicationLock,
    _files: Option<std::fs::File>,
    addresses: PublicAddresses,
    revision: i64,
}

impl Prepared {
    /// A novel local file seed follows the same preparation, then clears the override by revision.
    pub async fn reset(
        &mut self,
        conn: &mut Conn,
        at: chrono::DateTime<chrono::Utc>,
    ) -> ApiResult<PublicAddressSnapshot> {
        self.publication.check().await?;
        match orch_db::queries::public_addresses::reset_prepared(
            conn,
            &self.addresses,
            self.revision,
            at,
        )
        .await?
        {
            orch_db::queries::public_addresses::Write::Written(snapshot) => Ok(snapshot),
            orch_db::queries::public_addresses::Write::Stale => Err(stale()),
        }
    }
    /// Release file serialization before the database lock, after the caller's commit.
    pub async fn release(mut self) -> ApiResult<()> {
        drop(self._files.take());
        Ok(self.publication.release().await?)
    }
    /// Called only inside the caller's final freshly authorized transaction.
    pub async fn publish(
        &mut self,
        conn: &mut Conn,
        by: Option<orch_types::Id>,
        at: chrono::DateTime<chrono::Utc>,
    ) -> ApiResult<PublicAddressSnapshot> {
        self.publication.check().await?;
        match orch_db::queries::public_addresses::publish(
            conn,
            &self.addresses,
            self.revision,
            by,
            at,
        )
        .await?
        {
            orch_db::queries::public_addresses::Write::Written(snapshot) => {
                orch_db::sequences::notify_meta_changed(conn, None).await?;
                Ok(snapshot)
            }
            orch_db::queries::public_addresses::Write::Stale => Err(stale()),
        }
    }
}

fn stale() -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "The public addresses changed while this draft was being prepared",
    )
}

fn busy() -> ApiError {
    ApiError::new(
        ErrorCode::PublicAddressBusy,
        "Another public address change is being prepared",
    )
}

async fn pause(deadline: Instant, poll: Duration) -> ApiResult<()> {
    if Instant::now() >= deadline {
        return Err(busy());
    }
    tokio::time::sleep_until(std::cmp::min(
        deadline,
        Instant::now().checked_add(poll).unwrap_or(deadline),
    ))
    .await;
    Ok(())
}

/// Stage the managed certificate and prove its loaded workers, then prove every candidate origin.
/// No address write, authority transaction or general host command crosses the preparation wait.
/// Native address preparation shared by the API and the local operator command.
pub struct Preparer<'a> {
    pub db: &'a orch_db::Db,
    pub config: &'a orch_config::ServerConfig,
    pub clock: &'a dyn orch_types::Clock,
    pub certificates: &'a crate::certificates::CertificateService,
}

pub async fn prepare(
    app: &App,
    candidate: PublicAddresses,
    expected: i64,
    options: PrepareOptions,
) -> ApiResult<Prepared> {
    Preparer {
        db: &app.db,
        config: &app.config,
        clock: app.clock.as_ref(),
        certificates: app.link.certificates(),
    }
    .prepare(candidate, expected, options)
    .await
}

impl Preparer<'_> {
    pub async fn prepare(
        &self,
        candidate: PublicAddresses,
        expected: i64,
        options: PrepareOptions,
    ) -> ApiResult<Prepared> {
        let candidate = candidate
            .validated()
            .map_err(|error| ApiError::invalid(error.field, &error.to_string()))?;
        if options.timeout.is_zero() || options.poll.is_zero() {
            return Err(ApiError::invalid(
                "public_address",
                "Preparation waits must be positive",
            ));
        }
        let deadline = Instant::now().checked_add(options.timeout).ok_or_else(|| {
            ApiError::invalid(
                "public_address.prepare_timeout_secs",
                "The preparation wait exceeds this clock's representation",
            )
        })?;
        let mut publication = loop {
            if let Some(publication) =
                orch_db::queries::public_addresses::try_publication_lock(self.db).await?
            {
                break publication;
            }
            pause(deadline, options.poll).await?;
        };
        let before = {
            let mut conn = self.db.acquire().await?;
            orch_db::queries::public_addresses::read(
                &mut conn,
                &self.config.system.address_seed(),
                self.clock.now(),
            )
            .await?
        };
        if before.revision != expected {
            return Err(stale());
        }
        let service = self.certificates;
        let files = if let Some(frontend) = &options.managed_frontend {
            let files = loop {
                if let Some(files) =
                    crate::certificates::frontend::lock(self.config).map_err(ApiError::internal)?
                {
                    break files;
                }
                pause(deadline, options.poll).await?;
            };
            let generation = service
                .prepare_frontend_certificate(self.config, &before.addresses, &candidate)
                .map_err(|error| {
                    tracing::warn!(%error, "the frontend certificate could not be prepared");
                    ApiError::new(
                        ErrorCode::PublicAddressFrontendNotReady,
                        "The frontend certificate could not be prepared",
                    )
                    .with("field", "public_url")
                })?;
            publication.check().await?;
            service
                .activate_frontend_certificate(self.config, &generation)
                .map_err(ApiError::internal)?;
            wait_for_origin(
                service,
                frontend,
                Some(&generation),
                deadline,
                options.poll,
                "public_url",
            )
            .await?;
            Some(files)
        } else {
            None
        };
        for (field, origin) in std::iter::once(("public_url", &candidate.public_url)).chain(
            candidate
                .alternate_urls
                .iter()
                .map(|origin| ("alternate_urls", origin)),
        ) {
            wait_for_origin(service, origin, None, deadline, options.poll, field).await?;
        }
        publication.check().await?;
        Ok(Prepared {
            publication,
            _files: files,
            addresses: candidate,
            revision: expected,
        })
    }
}

fn unverified(field: &str, reason: &str, frontend: bool) -> ApiError {
    ApiError::new(
        if frontend {
            ErrorCode::PublicAddressFrontendNotReady
        } else {
            ErrorCode::PublicAddressUnverified
        },
        reason,
    )
    .with("field", field)
}

enum Probe {
    Ready,
    Retry(String),
    OtherInstallation,
}

async fn wait_for_origin(
    service: &crate::certificates::CertificateService,
    origin: &str,
    generation: Option<&crate::certificates::frontend::Generation>,
    deadline: Instant,
    poll: Duration,
    field: &str,
) -> ApiResult<()> {
    let mut reason = "The browser origin could not be reached".to_owned();
    loop {
        let Some(remaining) = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
        else {
            return Err(unverified(field, &reason, generation.is_some()));
        };
        match tokio::time::timeout_at(deadline, probe(service, origin, generation, remaining)).await
        {
            Ok(Ok(Probe::Ready)) => return Ok(()),
            Ok(Ok(Probe::OtherInstallation)) => {
                return Err(ApiError::new(
                    ErrorCode::PublicAddressMismatch,
                    "The browser origin does not prove this installation",
                )
                .with("field", field));
            }
            Ok(Ok(Probe::Retry(next))) => reason = next,
            Ok(Err(error)) => {
                reason = format!("The browser origin could not complete its proof: {error}")
            }
            Err(_) => return Err(unverified(field, &reason, generation.is_some())),
        }
        tokio::time::sleep_until(std::cmp::min(
            deadline,
            Instant::now().checked_add(poll).unwrap_or(deadline),
        ))
        .await;
    }
}

async fn probe(
    service: &crate::certificates::CertificateService,
    origin: &str,
    generation: Option<&crate::certificates::frontend::Generation>,
    timeout: Duration,
) -> Result<Probe, Box<dyn std::error::Error + Send + Sync>> {
    let challenge = orch_types::new_id();
    let expected = service.public_address_proof(challenge)?;
    let root = reqwest::Certificate::from_pem(service.authority_certificate().pem().as_bytes())?;
    // A fresh client has neither an old worker connection nor an old TLS-resumption context.
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .tls_certs_merge([root])
        .tls_info(true)
        .http1_only()
        .timeout(timeout)
        .build()?;
    let mut response = client
        .get(format!(
            "{}/api/v1/public-address/proof",
            origin.trim_end_matches('/')
        ))
        .query(&orch_api_contract::public_addresses::PublicAddressProofQuery { challenge })
        .header(http::header::CACHE_CONTROL, "no-store")
        .header(http::header::CONNECTION, "close")
        .send()
        .await?;
    if !response.status().is_success() {
        return Ok(Probe::Retry(format!(
            "The browser origin answered {}",
            response.status()
        )));
    }
    if let Some(generation) = generation {
        if response
            .headers()
            .get(FRONTEND_GENERATION_HEADER)
            .and_then(|value| value.to_str().ok())
            != Some(generation.name.as_str())
        {
            return Ok(Probe::Retry(
                "The frontend has not loaded the prepared certificate generation".to_owned(),
            ));
        }
        let certificate = response
            .extensions()
            .get::<reqwest::tls::TlsInfo>()
            .and_then(reqwest::tls::TlsInfo::peer_certificate)
            .and_then(|der| orch_pki::certificates::Certificate::from_der(der).ok());
        if certificate.is_none_or(|certificate| certificate.fingerprint() != generation.fingerprint)
        {
            return Ok(Probe::Retry(
                "The frontend has not served the prepared TLS certificate".to_owned(),
            ));
        }
    }
    // Both fields have fixed wire representations. A genuine endpoint emits this exact-size compact JSON.
    // Do not buffer an arbitrary response from a mistyped or hostile origin.
    let expected_bytes = serde_json::to_vec(&expected)?;
    let mut bytes = Vec::with_capacity(expected_bytes.len());
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > expected_bytes.len().saturating_sub(bytes.len()) {
            return Ok(Probe::OtherInstallation);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(
        if serde_json::from_slice::<PublicAddressProof>(&bytes)
            .ok()
            .as_ref()
            == Some(&expected)
        {
            Probe::Ready
        } else {
            Probe::OtherInstallation
        },
    )
}

/// Read on the caller's connection, including inside the operation that admits a link or flow.
pub async fn read(app: &App, conn: &mut Conn) -> ApiResult<PublicAddressSnapshot> {
    Ok(orch_db::queries::public_addresses::read(
        conn,
        &app.config.system.address_seed(),
        app.clock.now(),
    )
    .await?)
}

/// Read for a producer that does not already hold a database connection.
pub async fn current(app: &App) -> ApiResult<PublicAddressSnapshot> {
    let mut conn = app.db.acquire().await?;
    read(app, &mut conn).await
}

/// A browser origin is one argument in a command copied into a POSIX shell.
#[must_use]
pub fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
