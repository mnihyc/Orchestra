//! Build identities from the installed release and the native results recorded for those bytes.
use orch_api_contract::admin::{HarnessBuild, HarnessBuildSource, HarnessBuildStatus};
use orch_db::{Conn, DbError};
use orch_types::Json;
use std::path::Path;

/// The builds of harnesses the release lists, from the manifest in its payload. A release without
/// one lists none, and an entry that does not read as a build is not one.
pub async fn manifest(dir: &Path) -> Vec<HarnessBuild> {
    let Ok(text) = tokio::fs::read_to_string(dir.join("version.json")).await else {
        return Vec::new();
    };
    let Ok(manifest) = serde_json::from_str::<Json>(&text) else {
        return Vec::new();
    };
    manifest
        .get("harness_builds")
        .and_then(Json::as_array)
        .map(|listed| {
            listed
                .iter()
                .filter_map(|entry| serde_json::from_value::<HarnessBuild>(entry.clone()).ok())
                // The builds a release is made and tested with are its checked ones.
                .map(|build| HarnessBuild {
                    source: HarnessBuildSource::Release,
                    status: HarnessBuildStatus::Checked,
                    ..build
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Separate release authority from channel reads and administrator overrides.
pub fn key(build: &HarnessBuild) -> String {
    let (kind, os, arch) = super::platform(build);
    format!(
        "harness_release:{kind}:{}:{os}:{arch}:{}",
        build.version, build.sha256
    )
}

/// Register the packaged checksums together. Restarting keeps completed evidence and running claims.
pub async fn register(
    conn: &mut Conn,
    builds: &[HarnessBuild],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbError> {
    if builds.is_empty() {
        return Ok(());
    }
    let records = builds
        .iter()
        .map(|build| {
            let mut value = serde_json::to_value(build).map_err(super::native_error)?;
            value["checksum_origin"] = Json::from("release");
            Ok(serde_json::json!({"key": key(build), "value": value}))
        })
        .collect::<Result<Vec<_>, DbError>>()?;
    sqlx::query(
        "INSERT INTO settings (node_id,key,value,locked,revision,updated_at)
        SELECT NULL,row->>'key',row->'value',false,1,$2 FROM jsonb_array_elements($1) row
        ON CONFLICT DO NOTHING",
    )
    .bind(Json::from(records))
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// Read results for just the fixed identities supplied by the payload in one query.
pub async fn with_results(
    conn: &mut Conn,
    mut builds: Vec<HarnessBuild>,
) -> Result<Vec<HarnessBuild>, DbError> {
    if builds.is_empty() {
        return Ok(builds);
    }
    let keys: Vec<_> = builds.iter().map(key).collect();
    let rows: Vec<(String, Json)> =
        sqlx::query_as("SELECT key,value FROM settings WHERE node_id IS NULL AND key=ANY($1)")
            .bind(&keys)
            .fetch_all(conn)
            .await?;
    for (build, id) in builds.iter_mut().zip(keys) {
        if let Some((_, stored)) = rows.iter().find(|(k, _)| k == &id) {
            let record: HarnessBuild =
                serde_json::from_value(stored.clone()).map_err(super::native_error)?;
            build.status = record.status;
            build.check = record.check;
        }
    }
    Ok(builds)
}

/// Load native check identities before the service starts accepting thread requests.
pub async fn initialize(app: &crate::state::App) -> Result<(), DbError> {
    let builds = manifest(&crate::routes::downloads::payload_dir(&app.config)).await;
    let mut tx = app.db.begin().await?;
    let keys: Vec<_> = builds.iter().map(key).collect();
    sqlx::query("DELETE FROM settings WHERE node_id IS NULL AND starts_with(key,'harness_release:') AND NOT (key=ANY($1))")
        .bind(&keys).execute(&mut *tx).await?;
    register(&mut tx, &builds, app.clock.now()).await?;
    tx.commit().await?;
    Ok(())
}
