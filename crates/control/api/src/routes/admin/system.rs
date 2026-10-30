//! How the system stands: its parts with their heartbeats, its queues, what its blob store holds, the state of its certificates, the document of its requests, and the certificate of its authority.
//!
//! The parts report themselves: each service keeps a row that says it is alive and which build it
//! is, so that nothing here has to reach another service to know. A part is well when its row is
//! no older than three of its beats, which is how the worker itself judges its fellows. The figures
//! of the queues and of the blob store are read from the tables that hold them. The authority's
//! certificate is public: a machine that is told to trust the system fetches it here before it
//! has anything to show.

use super::admin_of;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::resources::view::{environment_refs, machine_refs};
use axum::body::Body;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use http::header;
use orch_api_contract::admin::{
    AdminPkiGet, AdminStorageGet, AdminSystemGet, OpenapiGet, PkiCa, PkiCaGet, PkiInfo,
    PkiInfoMachinesExpiring, PkiService, StorageEnvironment, StorageStats, SystemComponent,
    SystemComponentName, SystemInfo, SystemQueues,
};
use orch_api_contract::common::ErrorCode;
use orch_db::heartbeat;
use orch_db::queries::{jobs, link, metrics, resources};
use orch_pki::certificates::Certificate;
use orch_types::Time;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

/// How long after its last beat a part still counts as well: three beats of fifteen seconds, the
/// same silence after which the worker takes another worker for gone.
const HEARTBEAT_STALE_AFTER: Duration = heartbeat::WORKER_HEARTBEAT_MAX_AGE;
/// How many environments the blob store reports as the ones that hold the most. A screen shows a
/// short list; ten fit a table without a page of their own.
const TOP_ENVIRONMENTS: i64 = 10;
/// How far ahead a machine's certificate is counted as about to end.
const MACHINE_EXPIRY_WINDOW_DAYS: i64 = 30;
/// Bounds the PKI attention list while its separate count covers every matching machine.
const MACHINES_EXPIRING_LIMIT: i64 = 50;
/// Where a deployment keeps its secrets when no directory was installed and the variable is not
/// set: the default of the programs that read them.
const DEFAULT_SECRETS_DIR: &str = orch_config::DEFAULT_SECRETS_DIR;
/// The variable that names that directory for the programs of a deployment.
const SECRETS_DIR_VARIABLE: &str = "ORCH_SECRETS_DIR";
/// The services with a certificate of their own, and the file each is kept in.
const SERVICE_CERTIFICATES: [(&str, &str); 3] = [
    ("api", "service_api_cert.pem"),
    ("gateway", "service_gateway_cert.pem"),
    ("worker", "service_worker_cert.pem"),
];

static SECRETS_DIR: OnceLock<PathBuf> = OnceLock::new();
static MOUNTED: OnceLock<Instant> = OnceLock::new();

/// Gives the directory that holds the services' certificates, for composition and for tests.
/// The first directory given stays.
pub fn install_secrets_dir(dir: &Path) -> bool {
    SECRETS_DIR.set(dir.to_owned()).is_ok()
}

/// The directory of the services' certificates: the one that was given, or else the one the
/// deployment names.
fn secrets_dir() -> PathBuf {
    SECRETS_DIR.get().cloned().unwrap_or_else(|| {
        PathBuf::from(
            std::env::var(SECRETS_DIR_VARIABLE).unwrap_or_else(|_| DEFAULT_SECRETS_DIR.to_owned()),
        )
    })
}

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    let _ = MOUNTED.set(Instant::now());
    registry
        .on(AdminSystemGet, system)
        .on(orch_api_contract::admin::AdminReleaseCheck, check_release)
        .on(
            orch_api_contract::admin::AdminInstallationRecordsGet,
            installation_records,
        )
        .on(
            orch_api_contract::admin::AdminBackupRestoreList,
            backup_restore_list,
        )
        .on(AdminStorageGet, storage)
        .on(AdminPkiGet, pki)
        .on_raw(PkiCaGet, authority)
        .on_raw(OpenapiGet, openapi);
}

/// How one part fares, from the row it reports and what its kind of part can say for itself.
fn component(
    name: SystemComponentName,
    reported: Option<&heartbeat::Heartbeat>,
    now: DateTime<Utc>,
) -> SystemComponent {
    let (ok, detail) = match reported {
        None => (false, Some("it has not reported yet".to_owned())),
        Some(beat) if now - beat.seen_at > HEARTBEAT_STALE_AFTER => (
            false,
            Some(format!(
                "it reported last {} seconds ago",
                (now - beat.seen_at).num_seconds()
            )),
        ),
        Some(_) => (true, None),
    };
    SystemComponent {
        name,
        ok,
        detail,
        seen_at: reported.map(|beat| Time::new(beat.seen_at)),
    }
}

/// The release, the parts, the machines connected, the threads running, and what waits.
async fn system(ctx: Ctx, (): (), (): ()) -> ApiResult<SystemInfo> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    let beats = heartbeat::all(&mut conn).await?;
    let beat = |component: &str| beats.iter().find(|beat| beat.component == component);
    // The API answers, so it is well, and so is the database it has just read.
    let answering = |name| SystemComponent {
        name,
        ok: true,
        detail: None,
        seen_at: Some(Time::new(ctx.now)),
    };
    let components = vec![
        answering(SystemComponentName::Api),
        component(SystemComponentName::Gateway, beat("gateway"), ctx.now),
        component(SystemComponentName::Worker, beat("worker"), ctx.now),
        answering(SystemComponentName::Database),
    ];
    // The API writes its own row (its heartbeat, every fifteen seconds); until the first beat of
    // this process it started as long ago as it has been mounted.
    let started_at = beat("api").map_or_else(
        || {
            let up = MOUNTED.get().map_or(Duration::zero(), |since| {
                Duration::from_std(since.elapsed()).unwrap_or_else(|_| Duration::zero())
            });
            ctx.now - up
        },
        |beat| beat.started_at,
    );
    Ok(SystemInfo {
        version: ctx.app.build.version.to_owned(),
        build: ctx.app.build.build_number.to_owned(),
        started_at: Time::new(started_at),
        components,
        machines_online: metrics::machines_online(&mut conn).await?,
        threads_running: metrics::running_threads(&mut conn).await?,
        queues: SystemQueues {
            outbox_pending: metrics::outbox_pending(&mut conn).await?,
            commands_open: link::count_open_commands(&mut conn).await?,
            jobs_running: jobs::running_jobs(&mut conn).await?,
        },
    })
}

/// Check current browser/admin authority in a completed short transaction. External installation
/// reads occur between checks, with no transaction or pooled connection retained by this task.
async fn installation_read_authority(ctx: &Ctx) -> ApiResult<Time> {
    let (tx, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
    let now = guard.now();
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(now)
}

async fn check_release(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<orch_api_contract::admin::ReleaseCheck> {
    admin_of(&ctx)?;
    installation_read_authority(&ctx).await?;
    let result = orch_worker_core::releases::check(
        &ctx.app.db,
        &*ctx.app.clock,
        &ctx.app.config.release,
        ctx.app.build.version,
        orch_worker_core::releases::Wanted::Manual,
    )
    .await
    .map_err(ApiError::internal);
    installation_read_authority(&ctx).await?;
    result?.ok_or_else(|| ApiError::internal("another release check is in progress; retry shortly"))
}

async fn installation_records(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<orch_api_contract::admin::InstallationRecords> {
    admin_of(&ctx)?;
    let now = installation_read_authority(&ctx).await?;
    let result: ApiResult<orch_api_contract::admin::InstallationRecords> = async {
        let system_id = ctx
            .app
            .link
            .certificates()
            .authority_certificate()
            .fingerprint();
        let (backup, backup_error) = orch_worker_core::backups::reconcile(
            &ctx.app.db,
            &ctx.app.config.installation.records_dir,
            &system_id,
            now,
        )
        .await
        .map_err(ApiError::internal)?;
        let release = orch_worker_core::releases::check(
            &ctx.app.db,
            &*ctx.app.clock,
            &ctx.app.config.release,
            ctx.app.build.version,
            orch_worker_core::releases::Wanted::SystemPage,
        )
        .await
        .map_err(ApiError::internal)?;
        let days = {
            let mut conn = ctx.app.db.acquire().await?;
            orch_settings::get::<i64>(&mut conn, orch_settings::Place::System, "backup.warn_days")
                .await
                .map_err(ApiError::internal)?
                .unwrap_or(2)
        };
        let backup_warning =
            orch_worker_core::backups::warn(ctx.app.clock.time(), backup.as_ref(), days);
        Ok(orch_api_contract::admin::InstallationRecords {
            release,
            backup,
            backup_warning,
            backup_error,
        })
    }
    .await;
    installation_read_authority(&ctx).await?;
    result
}

async fn backup_restore_list(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<Option<orch_api_contract::admin::BackupRestoreList>> {
    admin_of(&ctx)?;
    let now = installation_read_authority(&ctx).await?;
    let result: ApiResult<Option<orch_api_contract::admin::BackupRestoreList>> = async {
        let system_id = ctx
            .app
            .link
            .certificates()
            .authority_certificate()
            .fingerprint();
        let dir = ctx.app.config.installation.records_dir.clone();
        let (backup, error) =
            orch_worker_core::backups::reconcile(&ctx.app.db, &dir, &system_id, now)
                .await
                .map_err(ApiError::internal)?;
        if let Some(error) = error {
            return Err(ApiError::new(ErrorCode::SourceOffline, error));
        }
        tokio::task::spawn_blocking(move || {
            orch_worker_core::backups::inventory::read(&dir, &system_id, backup.as_ref())
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(|error| ApiError::new(ErrorCode::SourceOffline, error.to_string()))
    }
    .await;
    installation_read_authority(&ctx).await?;
    result
}

/// The bytes free on the disk that holds `dir`, or the nearest directory above it that exists:
/// where the blob store will be, if it is not yet.
#[cfg(unix)]
fn free_bytes(dir: &Path) -> i64 {
    let mut at = dir;
    loop {
        if let Ok(found) = rustix::fs::statvfs(at) {
            return i64::try_from(found.f_bavail.saturating_mul(found.f_frsize))
                .unwrap_or(i64::MAX);
        }
        match at.parent() {
            Some(above) => at = above,
            None => return 0,
        }
    }
}

#[cfg(not(unix))]
fn free_bytes(_dir: &Path) -> i64 {
    0
}

/// What the blob store holds, from its index and from the disk.
async fn storage(ctx: Ctx, (): (), (): ()) -> ApiResult<StorageStats> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    let totals = metrics::blob_totals(&mut conn).await?;
    let heaviest = metrics::heaviest_environments(&mut conn, TOP_ENVIRONMENTS).await?;
    let ids: Vec<_> = heaviest.iter().map(|each| each.environment_id).collect();
    let named = environment_refs(&mut conn, &ids, &[], orch_types::Time::new(ctx.now)).await?;
    let top_environments = heaviest
        .iter()
        .filter_map(|each| {
            named
                .iter()
                .find(|(_, found)| found.id == each.environment_id)
                .map(|(_, environment)| StorageEnvironment {
                    environment: environment.clone(),
                    cache_bytes: each.cache_bytes,
                    pinned_bytes: each.pinned_bytes,
                })
        })
        .collect();
    Ok(StorageStats {
        blob_count: totals.count,
        blob_bytes: totals.bytes,
        pinned_bytes: totals.pinned_bytes,
        cache_bytes: totals.cache_bytes,
        disk_free_bytes: free_bytes(Path::new(&ctx.app.config.storage.root)),
        top_environments,
    })
}

/// The certificates of the services that this process can read in the secrets directory, until
/// when each is good. A file that is not there or does not read as a certificate is left out: it
/// is not this process's to see.
async fn service_certificates(dir: &Path) -> Vec<PkiService> {
    let mut found = Vec::new();
    for (name, file) in SERVICE_CERTIFICATES {
        let Ok(text) = tokio::fs::read_to_string(dir.join(file)).await else {
            continue;
        };
        if let Ok(certificate) = Certificate::from_pem(&text) {
            found.push(PkiService {
                name: name.to_owned(),
                not_after: certificate.not_after(),
            });
        }
    }
    found
}

/// The authority and the certificates that end soon.
async fn pki(ctx: Ctx, (): (), (): ()) -> ApiResult<PkiInfo> {
    admin_of(&ctx)?;
    let ca = ctx.app.link.certificates().authority_certificate();
    let mut conn = ctx.app.db.acquire().await?;
    let before = ctx.now + Duration::days(MACHINE_EXPIRY_WINDOW_DAYS);
    let machines_expiring_30d =
        resources::management::machines_with_certificates_ending_by(&mut conn, before).await?;
    let expiring = resources::management::expiring_machine_certificates(
        &mut conn,
        before,
        MACHINES_EXPIRING_LIMIT,
    )
    .await?;
    let ids: Vec<_> = expiring.iter().map(|row| row.id).collect();
    let machines = machine_refs(&ctx.app, &mut conn, &ids).await?;
    let machines_expiring = expiring
        .into_iter()
        .filter_map(|row| {
            machines
                .get(&row.id)
                .cloned()
                .map(|machine| PkiInfoMachinesExpiring {
                    machine,
                    not_after: Time::new(row.not_after),
                })
        })
        .collect();
    Ok(PkiInfo {
        ca: PkiCa {
            subject: ca.subject_common_name().unwrap_or_default().to_owned(),
            not_before: ca.not_before(),
            not_after: ca.not_after(),
            fingerprint_sha256: ca.fingerprint(),
        },
        services: service_certificates(&secrets_dir()).await,
        machines_expiring_30d,
        machines_expiring,
    })
}

/// The certificate of the system's authority, for whoever is to trust it.
async fn authority(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    Ok((
        [(header::CONTENT_TYPE, "application/x-pem-file")],
        ctx.app.link.certificates().authority_certificate().pem(),
    )
        .into_response())
}

/// The OpenAPI document of the requests of this release, for any signed-in person.
async fn openapi(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    let document = crate::openapi::served(ctx.app.build.version);
    if document.is_empty() {
        return Err(ApiError::internal("the document of the requests is empty"));
    }
    Ok(([(header::CONTENT_TYPE, "application/json")], document).into_response())
}
