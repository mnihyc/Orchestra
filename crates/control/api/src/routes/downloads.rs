//! Connector binaries and installers served by the system.
//!
//! A release keeps these files in its payload, in a directory of `connectors` beside the
//! `migrations` the configuration names, and they are served as they are, so that a user's machine
//! never needs a code host. The connector carries its release and its BLAKE3 hash in two headers,
//! which the installers compare with the manifest; the hash is that of the bytes that follow,
//! worked out as they are read and not taken from the manifest. A system that was not started from
//! a release, a development build, has no such directory, and says so plainly instead of
//! answering with nothing.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use axum::body::Body;
use axum::response::Response;
use http::{HeaderValue, StatusCode, header};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::downloads::{
    BLAKE3_HEADER, CONNECTOR_ARCH, CONNECTOR_OS, DownloadConnectorGet, DownloadInstallNodeShGet,
    DownloadInstallPs1Get, DownloadInstallShGet, DownloadNodeGet, DownloadVersionGet,
    VERSION_HEADER,
};
use orch_config::ServerConfig;
use orch_types::Hash;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

/// The directory of a release's payload that holds the connectors, the installers and the
/// manifest, beside the migrations.
const PAYLOAD_DIRECTORY: &str = "connectors";
/// The manifest of the files of the payload.
pub(crate) const MANIFEST_FILE: &str = "version.json";
/// The POSIX installer.
const INSTALL_SH: &str = "install.sh";
/// The PowerShell installer.
const INSTALL_PS1: &str = "install.ps1";
/// The installer of a host's node.
const INSTALL_NODE_SH: &str = "install-node.sh";
/// How many bytes are read at once when a file is hashed: a size that keeps a download of tens of
/// megabytes to a few hundred reads without holding any of it.
const HASH_CHUNK_BYTES: usize = 64 * 1024;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_raw(DownloadConnectorGet, connector)
        .on_raw(DownloadInstallShGet, install_sh)
        .on_raw(DownloadInstallPs1Get, install_ps1)
        .on_raw(DownloadInstallNodeShGet, install_node_sh)
        .on_raw(DownloadNodeGet, node)
        .on_raw(DownloadVersionGet, version);
}

/// Where the files of a release's payload that these requests serve are: the `connectors`
/// directory beside the migrations that the configuration names.
#[must_use]
pub fn payload_dir(config: &ServerConfig) -> PathBuf {
    let migrations = Path::new(&config.database.migrations_dir);
    migrations
        .parent()
        .unwrap_or(migrations)
        .join(PAYLOAD_DIRECTORY)
}

fn nothing_to_serve(what: &str) -> ApiError {
    ApiError::new(ErrorCode::NotFound, what.to_owned())
}

/// The file `name` of the payload, or the plain answer of a system that has none.
async fn file_of(config: &ServerConfig, name: &str) -> ApiResult<(PathBuf, tokio::fs::File)> {
    let dir = payload_dir(config);
    if !dir.is_dir() {
        return Err(nothing_to_serve(
            "this system was not started from a release: it has no files to download",
        ));
    }
    let path = dir.join(name);
    match tokio::fs::File::open(&path).await {
        Ok(file) if path.is_file() => Ok((path, file)),
        _ => Err(nothing_to_serve(&format!(
            "this release has no {name} to download"
        ))),
    }
}

/// The release the payload says it is, or else the release that is running.
async fn release_of(ctx: &Ctx) -> String {
    let manifest =
        tokio::fs::read_to_string(payload_dir(&ctx.app.config).join(MANIFEST_FILE)).await;
    manifest
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|manifest| manifest.get("version")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| ctx.app.build.version.to_owned())
}

/// A file as an answer, streamed from the start and for exactly `length` bytes, with `headers`;
/// `saved_as` names the file a browser saves it as, where it is a file to keep and not to read.
async fn answer(
    mut file: tokio::fs::File,
    length: u64,
    content_type: &'static str,
    saved_as: Option<&str>,
    headers: &[(&'static str, String)],
) -> ApiResult<Response> {
    file.rewind().await.map_err(ApiError::internal)?;
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file.take(length))));
    *response.status_mut() = StatusCode::OK;
    let out = response.headers_mut();
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    out.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Some(name) = saved_as
        && let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
    {
        out.insert(header::CONTENT_DISPOSITION, value);
    }
    for (header, value) in headers {
        if let Ok(value) = HeaderValue::from_str(value) {
            out.insert(*header, value);
        }
    }
    Ok(response)
}

/// A file of the payload as it is.
async fn plain(ctx: &Ctx, name: &str, content_type: &'static str) -> ApiResult<Response> {
    let (_, file) = file_of(&ctx.app.config, name).await?;
    let length = file.metadata().await.map_err(ApiError::internal)?.len();
    answer(file, length, content_type, None, &[]).await
}

/// The BLAKE3 of the first `length` bytes of the file, in lower case hexadecimal.
async fn blake3_of(file: &mut tokio::fs::File, length: u64) -> ApiResult<String> {
    let mut hasher = Hash::hasher();
    let mut left = length;
    let mut chunk = vec![0_u8; HASH_CHUNK_BYTES];
    while left > 0 {
        let wanted =
            usize::try_from(left).map_or(HASH_CHUNK_BYTES, |left| left.min(HASH_CHUNK_BYTES));
        let read = file
            .read(&mut chunk[..wanted])
            .await
            .map_err(ApiError::internal)?;
        if read == 0 {
            return Err(ApiError::internal("a file of the payload was cut short"));
        }
        hasher.update(&chunk[..read]);
        left -= u64::try_from(read).unwrap_or(left);
    }
    Ok(Hash::finish(&hasher).to_string())
}

/// The names a connector for a system and a processor may be kept under in the payload: the
/// release names its builds by the target they were made for.
pub(crate) fn connector_names(os: &str, arch: &str) -> [String; 3] {
    [
        format!("orch-connector-{os}-{arch}-musl"),
        format!("orch-connector-{os}-{arch}"),
        format!("orch-connector-{os}-{arch}.exe"),
    ]
}

/// The connector binary for an operating system and a processor, with its release and its hash.
async fn connector(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    let (os, arch) = (ctx.param("os")?.to_owned(), ctx.param("arch")?.to_owned());
    if !CONNECTOR_OS.contains(&os.as_str()) || !CONNECTOR_ARCH.contains(&arch.as_str()) {
        return Err(nothing_to_serve(
            "connectors are built for linux, macos and windows, on x86_64 and aarch64",
        ));
    }
    let mut found = None;
    for name in connector_names(&os, &arch) {
        match file_of(&ctx.app.config, &name).await {
            Ok((_, file)) => {
                found = Some((name, file));
                break;
            }
            // The directory itself is not there: that is the answer, whatever the name.
            Err(error) if !payload_dir(&ctx.app.config).is_dir() => return Err(error),
            Err(_) => {}
        }
    }
    let Some((name, mut file)) = found else {
        return Err(nothing_to_serve(&format!(
            "this release has no connector for {os} on {arch}"
        )));
    };
    let length = file.metadata().await.map_err(ApiError::internal)?.len();
    let hash = blake3_of(&mut file, length).await?;
    let release = release_of(&ctx).await;
    answer(
        file,
        length,
        "application/octet-stream",
        Some(&name),
        &[(VERSION_HEADER, release), (BLAKE3_HEADER, hash)],
    )
    .await
}

/// The node of a host for a processor, with its release and its hash: built for Linux alone, and
/// static, as the release names it.
async fn node(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    let arch = ctx.param("arch")?.to_owned();
    if !CONNECTOR_ARCH.contains(&arch.as_str()) {
        return Err(nothing_to_serve(
            "the node is built for Linux, on x86_64 and aarch64",
        ));
    }
    let name = format!("orch-node-linux-{arch}-musl");
    let (_, mut file) = match file_of(&ctx.app.config, &name).await {
        Ok(found) => found,
        Err(error) if !payload_dir(&ctx.app.config).is_dir() => return Err(error),
        Err(_) => {
            return Err(nothing_to_serve(&format!(
                "this release has no node for {arch}"
            )));
        }
    };
    let length = file.metadata().await.map_err(ApiError::internal)?.len();
    let hash = blake3_of(&mut file, length).await?;
    let release = release_of(&ctx).await;
    answer(
        file,
        length,
        "application/octet-stream",
        Some(&name),
        &[(VERSION_HEADER, release), (BLAKE3_HEADER, hash)],
    )
    .await
}

/// The POSIX installer.
async fn install_sh(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    plain(&ctx, INSTALL_SH, "text/plain; charset=utf-8").await
}

/// The PowerShell installer.
async fn install_ps1(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    plain(&ctx, INSTALL_PS1, "text/plain; charset=utf-8").await
}

/// The installer of a host's node.
async fn install_node_sh(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    plain(&ctx, INSTALL_NODE_SH, "text/plain; charset=utf-8").await
}

/// The release manifest, as the release wrote it.
async fn version(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    plain(&ctx, MANIFEST_FILE, "application/json").await
}
