//! Server, frontend, connector, node, CLI and deployment payloads.
//!
//! `cargo xtask package --out DIR` builds the files of a release for the
//! architecture Docker runs on, writes the manifest that lists them with their
//! sizes and hashes, and checks the directory again as an updater will:
//!
//! | File | What it is |
//! |------|------------|
//! | `orch-server-linux-<arch>.zip` | the server programs, the schema, the rate catalogue, the connector, the node and the installers for `/downloads` (listed with their SHA-256 for the installers), the licences |
//! | `orch-frontend-dist.tar.gz` | the browser application, built by npm |
//! | `orchctl-…`, `orch-connector-…`, `orch-node-…` | the static programs, one file each |
//! | `orch-deploy-<tag>.tar.gz` | the `deploy/` directory and the operator's pages |
//! | `version.json` | the manifest |
//!
//! The server programs are compiled in the builder image, on the distribution
//! the services run, and the static ones for the musl target in the same image.
//! `--check` validates a directory without building anything, and
//! `--manifest-only` writes the manifest of the files a directory holds, for a
//! release assembled from the outputs of several machines.

use crate::release::archive::{self, EXECUTABLE_MODE, Entry, FILE_MODE};
use crate::release::connectors;
use crate::release::container::{self, Builder};
use crate::release::identity;
use crate::release::image;
use crate::release::licences;
use crate::release::manifest::{
    self, Asset, AssetNames, CONNECTOR_MINIMUM, LinkProtocol, MANIFEST_FILE, Manifest, PROJECT,
    SCHEMA_VERSION,
};
use crate::release::stock;
use crate::release::{ReleaseError, capture, run as run_command};
use orch_link_contract::version::{OLDEST_PROTOCOL_VERSION, PROTOCOL_VERSION};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What `cargo xtask package` is asked.
#[derive(clap::Args, Debug)]
pub struct PackageArgs {
    /// The directory the release is written to. It is made, and refused when it
    /// holds anything, so that a release never mixes two builds.
    #[arg(long, value_name = "DIR")]
    out: PathBuf,
    /// Validate the release in the directory (the manifest, the sizes and
    /// hashes, the contents of each archive) instead of building one.
    #[arg(long)]
    check: bool,
    /// Write the manifest of the files the directory holds, keeping what the
    /// existing manifest says of the release itself.
    #[arg(long, conflicts_with = "check")]
    manifest_only: bool,
    /// Where the release will be published: each asset's address is this and its
    /// name. Without it an address is the bare name, read against the address
    /// of `version.json`.
    #[arg(long, value_name = "URL")]
    base_url: Option<String>,
    /// Do not build the default environment image.
    #[arg(long)]
    skip_image: bool,
    /// Build the default environment image without Docker's layer cache, so that it is
    /// what its recipe says today and not what an earlier build left.
    #[arg(long, conflicts_with = "skip_image")]
    fresh_image: bool,
    /// Use the browser application already built in `frontend/dist` instead of
    /// building it.
    #[arg(long)]
    reuse_frontend: bool,
    /// Build and run one connector on its native runner, with a verified input manifest.
    #[arg(long, value_name = "TARGET", conflicts_with_all = ["check", "manifest_only", "connectors", "require_all_connectors"])]
    native_connector: Option<String>,
    /// Verified native connector build directories to include in the server archive and release.
    #[arg(long, value_name = "DIR", action = clap::ArgAction::Append)]
    connectors: Vec<PathBuf>,
    /// Require all five declared connector targets (the release workflow uses this).
    #[arg(long)]
    require_all_connectors: bool,
}

/// The programs compiled for the glibc the services run on.
const SERVER_PROGRAMS: [&str; 3] = ["orch-api", "orch-gateway", "orch-worker"];
/// The programs compiled static, for the musl target.
const STATIC_PROGRAMS: [&str; 3] = ["orch-connector", "orch-node", "orchctl"];
/// The bundle of public roots inside the builder image: a service image holds
/// none, and the services reach public servers (mail, bots, webhooks, push).
const CA_BUNDLE_IN_IMAGE: &str = "/etc/ssl/certs/ca-certificates.crt";
/// The installers that go to the connectors' download directory.
const INSTALLERS: [&str; 3] = ["install.sh", "install.ps1", "install-node.sh"];
/// The entries the user of an archive reaches in a release.
const TOOLS_DIR_IN_ZIP: &str = "connectors";

/// Runs the command.
pub fn run(root: &Path, args: &PackageArgs) -> Result<(), ReleaseError> {
    if let Some(target) = &args.native_connector {
        return connectors::build(root, &args.out, target);
    }
    if args.check {
        check_release(&args.out)?;
        if args.require_all_connectors {
            connectors::require_all(
                &Manifest::read(&args.out)?
                    .assets
                    .into_iter()
                    .map(|asset| asset.name)
                    .collect(),
            )?;
        }
        eprintln!("{}: the release is whole", args.out.display());
        return Ok(());
    }
    if args.manifest_only {
        return rewrite_manifest(&args.out, args.base_url.as_deref());
    }
    build(root, args)
}

/// The directory of the build counters the builds are given: the one the
/// environment names, else the tracked one.
fn counters_dir(root: &Path) -> PathBuf {
    std::env::var_os("ORCH_BUILD_NUMBER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("build/build-numbers"))
}

fn io_error(path: &Path) -> impl Fn(std::io::Error) -> ReleaseError + '_ {
    move |source| ReleaseError::Io {
        path: path.to_owned(),
        source,
    }
}

fn git(root: &Path, arguments: &[&str]) -> Result<String, ReleaseError> {
    capture(Command::new("git").arg("-C").arg(root).args(arguments))
}

/// Builds the whole release.
fn build(root: &Path, args: &PackageArgs) -> Result<(), ReleaseError> {
    let mut images = stock::images(root)?;
    let out = &args.out;
    if out.exists() && fs::read_dir(out).map_err(io_error(out))?.next().is_some() {
        return Err(ReleaseError::Invalid(format!(
            "{} is not empty; a release is written to a directory of its own",
            out.display()
        )));
    }
    fs::create_dir_all(out).map_err(io_error(out))?;

    let release = orch_build_support::release_identity(env!("CARGO_PKG_VERSION"))?;
    let version = release.version;
    let tag = release.tag.unwrap_or_else(|| format!("v{version}"));
    let commit = git(root, &["rev-parse", "HEAD"])?;
    let extra_connectors = connectors::read(&args.connectors, &version, &tag, &commit)?;
    if args.require_all_connectors {
        connectors::require_all(
            &extra_connectors
                .iter()
                .map(|(name, _)| name.clone())
                .collect(),
        )?;
    }
    if !git(root, &["status", "--porcelain"])?.is_empty() {
        eprintln!(
            "warning: the working tree has uncommitted changes; the manifest names the commit it started from"
        );
    }
    container::check_compose_distribution(root)?;

    let arch = Builder::architecture()?;
    let names = AssetNames::for_linux(&arch, &tag);
    let triple = format!("{arch}-unknown-linux-musl");
    let target_base =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let builder = Builder::new(root, &target_base.join("container"), &counters_dir(root))?;
    builder.ensure_image()?;

    // The programs. What a build reports of itself is the release's version
    // and tag, so the tag is given to the builds even when it is only the
    // version with a `v`.
    let identity_variables = vec![
        ("ORCH_RELEASE_VERSION".to_owned(), version.clone()),
        ("ORCH_RELEASE_TAG".to_owned(), tag.clone()),
    ];
    let mut server_variables = identity_variables.clone();
    // The web-push crate links OpenSSL; linking it in keeps the services'
    // image free of a library the distribution's slim image does not hold.
    server_variables.extend([
        ("OPENSSL_STATIC".to_owned(), "1".to_owned()),
        (
            "OPENSSL_LIB_DIR".to_owned(),
            format!("/usr/lib/{arch}-linux-gnu"),
        ),
        ("OPENSSL_INCLUDE_DIR".to_owned(), "/usr/include".to_owned()),
    ]);
    let mut server_arguments = vec!["build", "--release", "--locked"];
    for program in SERVER_PROGRAMS {
        server_arguments.extend(["-p", program]);
    }
    builder.cargo(&server_arguments, &server_variables)?;
    let mut static_arguments = vec!["build", "--release", "--locked", "--target", &triple];
    for program in STATIC_PROGRAMS {
        static_arguments.extend(["-p", program]);
    }
    builder.cargo(&static_arguments, &identity_variables)?;
    let glibc_dir = builder.target_dir().join("release");
    let musl_dir = builder.target_dir().join(&triple).join("release");

    // The browser application.
    let dist = frontend_dist(root, args.reuse_frontend, &version, &tag)?;

    // The files that are one program each.
    for (program, name) in [
        ("orchctl", &names.ctl),
        ("orch-connector", &names.connector),
        ("orch-node", &names.node),
    ] {
        let target = out.join(name);
        fs::copy(musl_dir.join(program), &target).map_err(io_error(&target))?;
    }
    for (name, source) in &extra_connectors {
        let target = out.join(name);
        fs::copy(source, &target).map_err(io_error(&target))?;
    }

    let rust_notices = {
        let server_target = format!("{arch}-unknown-linux-gnu");
        let mut targets = vec![server_target.as_str(), triple.as_str()];
        if !extra_connectors.is_empty() {
            targets.extend(["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]);
            targets.extend(connectors::TARGETS.iter().map(|(target, _, _, _)| *target));
        }
        licences::collect(root, &builder, &targets)?
    };
    let web_notices = licences::collect_frontend(root)?;
    let licence_entries = vec![
        Entry::file("LICENSE", &root.join("LICENSE"))?,
        Entry::file("NOTICE", &root.join("NOTICE"))?,
    ];

    // The server archive.
    let mut server = Vec::new();
    for program in SERVER_PROGRAMS {
        server.push(Entry::file(
            format!("bin/{program}"),
            &glibc_dir.join(program),
        )?);
    }
    server.push(Entry::file("bin/orch-node", &musl_dir.join("orch-node"))?);
    // Only the schema's files: the directory also holds a note for people.
    server.extend(archive::walk(
        &root.join("migrations"),
        "migrations/",
        &|path| !path.ends_with(".sql"),
    )?);
    server.push(Entry::file(
        "share/model-rates.json",
        &root.join("deploy/model-rates.json"),
    )?);
    server.push(Entry::bytes(
        "share/ca-certificates.crt",
        builder.read_in_image(CA_BUNDLE_IN_IMAGE)?,
        FILE_MODE,
    ));
    server.extend(connector_entries(
        root,
        &musl_dir,
        &arch,
        &version,
        &tag,
        &commit,
        &extra_connectors,
    )?);
    server.extend(licence_entries.iter().cloned());
    server.push(Entry::bytes(
        "THIRD_PARTY_NOTICES",
        rust_notices.into_bytes(),
        FILE_MODE,
    ));
    archive::write_zip(&out.join(&names.server), &server)?;

    // The browser application's archive.
    let mut frontend = archive::walk(&dist, "dist/", &|_| false)?;
    frontend.extend(licence_entries.iter().cloned().map(|mut entry| {
        entry.path = format!("dist/{}", entry.path);
        entry
    }));
    frontend.push(Entry::bytes(
        "dist/THIRD_PARTY_NOTICES",
        web_notices.into_bytes(),
        FILE_MODE,
    ));
    archive::write_tar_gz(&out.join(&names.frontend), &frontend)?;

    // The deploy bundle.
    let mut deploy = archive::walk(&root.join("deploy"), "deploy/", &|path| {
        skipped_in_deploy(path)
    })?;
    deploy.extend(archive::walk(&root.join("docs"), "docs/", &|_| false)?);
    deploy.push(Entry::bytes(
        "RELEASE_TAG",
        format!("{tag}\n").into_bytes(),
        FILE_MODE,
    ));
    deploy.extend(licence_entries);
    deploy.push(Entry::bytes("THIRD_PARTY_NOTICES", b"The deployment scripts are covered by LICENSE. Third-party program notices accompany the server and frontend archives.\n".to_vec(), FILE_MODE));
    archive::write_tar_gz(&out.join(&names.deploy), &deploy)?;

    if !args.skip_image {
        images.push(image::build(root, &version, args.fresh_image)?);
        // The developer image is made on the one just built, so that the release names both.
        images.push(image::build_developer(root, &version, args.fresh_image)?);
    }

    // The manifest, written from the files as they now are, and read again.
    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        project: PROJECT.to_owned(),
        tag,
        version,
        commit,
        link_protocol: LinkProtocol {
            current: PROTOCOL_VERSION,
            minimum: OLDEST_PROTOCOL_VERSION,
        },
        connector_minimum: CONNECTOR_MINIMUM.to_owned(),
        assets: manifest::assets_of(out, args.base_url.as_deref())?,
        images,
        harness_builds: harness_builds(root)?,
    };
    manifest.write(out)?;
    check_release(out)?;
    for asset in &manifest.assets {
        eprintln!("{:>12} bytes  {}", asset.size, asset.name);
    }
    eprintln!(
        "{}: release {} written and checked",
        out.display(),
        manifest.tag
    );
    Ok(())
}

/// What a deploy bundle leaves out: the state of an installation that lives in
/// the directory, and anything that could hold a secret.
fn skipped_in_deploy(path: &str) -> bool {
    let first = path.split('/').next().unwrap_or_default();
    if first == "runtime" {
        return true;
    }
    if path == ".env" || (path.starts_with(".env.") && path != ".env.example") {
        return true;
    }
    if path.starts_with("config/ca-certificates/")
        && !matches!(
            path,
            "config/ca-certificates/.gitignore" | "config/ca-certificates/README.md"
        )
    {
        return true;
    }
    path.starts_with("config/secrets/") && path != "config/secrets/.gitkeep"
}

/// The builds of the harnesses `deploy/harness-builds.json` pins.
fn harness_builds(root: &Path) -> Result<Vec<serde_json::Value>, ReleaseError> {
    let path = root.join("deploy/harness-builds.json");
    let text = fs::read_to_string(&path).map_err(io_error(&path))?;
    serde_json::from_str(&text).map_err(|source| ReleaseError::Json { path, source })
}

/// The files that go to `connectors/`: the connector builds, the installers, and
/// the manifest of those files that `/downloads` answers from.
fn connector_entries(
    root: &Path,
    musl_dir: &Path,
    arch: &str,
    version: &str,
    tag: &str,
    commit: &str,
    extra_connectors: &[(String, PathBuf)],
) -> Result<Vec<Entry>, ReleaseError> {
    // The connector, and the node a host's installer fetches from the system it joins.
    let mut entries = vec![
        Entry::file(
            format!("{TOOLS_DIR_IN_ZIP}/orch-connector-linux-{arch}-musl"),
            &musl_dir.join("orch-connector"),
        )?,
        Entry::file(
            format!("{TOOLS_DIR_IN_ZIP}/orch-node-linux-{arch}-musl"),
            &musl_dir.join("orch-node"),
        )?,
    ];
    for (name, path) in extra_connectors {
        let wanted = format!("{TOOLS_DIR_IN_ZIP}/{name}");
        let mut entry = Entry::file(&wanted, path)?;
        entry.mode = EXECUTABLE_MODE;
        if let Some(existing) = entries.iter_mut().find(|entry| entry.path == wanted) {
            *existing = entry;
        } else {
            entries.push(entry);
        }
    }
    for installer in INSTALLERS {
        let path = root.join("deploy/install").join(installer);
        if path.is_file() {
            let mut entry = Entry::file(format!("{TOOLS_DIR_IN_ZIP}/{installer}"), &path)?;
            if installer.ends_with(".sh") {
                entry.mode = EXECUTABLE_MODE;
            }
            entries.push(entry);
        }
    }
    let mut assets = Vec::new();
    for entry in &entries {
        let name = entry.path.rsplit('/').next().unwrap_or_default().to_owned();
        let ((size, blake3), sha256) = match &entry.source {
            archive::Source::File(path) => {
                (manifest::describe_file(path)?, manifest::sha256_of(path)?)
            }
            archive::Source::Bytes(_) => continue,
        };
        // Where the system serves each file from (below `/api/v1`).
        let download_url = if let Some(path) = connectors::download_path(&name) {
            path
        } else if name.starts_with("orch-node-") {
            format!("/api/v1/downloads/node/{arch}")
        } else {
            format!("/api/v1/downloads/{name}")
        };
        // The installers check what they fetch by its SHA-256 (no shell has a BLAKE3 tool).
        assets.push(Asset {
            name,
            download_url,
            size,
            blake3,
            sha256: Some(sha256),
        });
    }
    let listing = Manifest {
        schema_version: SCHEMA_VERSION,
        project: PROJECT.to_owned(),
        tag: tag.to_owned(),
        version: version.to_owned(),
        commit: commit.to_owned(),
        link_protocol: LinkProtocol {
            current: PROTOCOL_VERSION,
            minimum: OLDEST_PROTOCOL_VERSION,
        },
        connector_minimum: CONNECTOR_MINIMUM.to_owned(),
        assets,
        images: Vec::new(),
        harness_builds: harness_builds(root)?,
    };
    let mut text = serde_json::to_string_pretty(&listing).map_err(|source| ReleaseError::Json {
        path: MANIFEST_FILE.into(),
        source,
    })?;
    text.push('\n');
    entries.push(Entry::bytes(
        format!("{TOOLS_DIR_IN_ZIP}/{MANIFEST_FILE}"),
        text.into_bytes(),
        FILE_MODE,
    ));
    Ok(entries)
}

/// Builds the browser application, or finds the one that is built.
fn frontend_dist(
    root: &Path,
    reuse: bool,
    version: &str,
    tag: &str,
) -> Result<PathBuf, ReleaseError> {
    let frontend = root.join("frontend");
    let dist = frontend.join("dist");
    if !reuse {
        let wanted = fs::read_to_string(frontend.join(".nvmrc")).map_err(io_error(&frontend))?;
        let wanted = wanted.trim().trim_start_matches('v');
        let running = capture(Command::new("node").arg("--version"))?;
        if running.trim_start_matches('v') != wanted {
            return Err(ReleaseError::Invalid(format!(
                "the application is built with Node {wanted} (frontend/.nvmrc) and this is {running}"
            )));
        }
        // `npm ci` removes and installs everything; it is run when nothing is
        // installed or the lock file has changed since.
        let installed = frontend.join("node_modules/.package-lock.json");
        let stale = match (
            fs::metadata(&installed),
            fs::metadata(frontend.join("package-lock.json")),
        ) {
            (Ok(installed), Ok(lock)) => match (installed.modified(), lock.modified()) {
                (Ok(installed), Ok(lock)) => installed < lock,
                _ => true,
            },
            _ => true,
        };
        if stale {
            run_command(Command::new("npm").arg("ci").current_dir(&frontend))?;
        }
        let asked = identity::identity("frontend")?;
        let mut build = Command::new("npm");
        build.args(["run", "build"]).current_dir(&frontend);
        for (name, value) in asked.environment() {
            // The release's version and tag are the packaging's own.
            let value = match name.as_str() {
                "ORCH_RELEASE_VERSION" => version.to_owned(),
                "ORCH_RELEASE_TAG" => tag.to_owned(),
                _ => value,
            };
            build.env(name, value);
        }
        run_command(&mut build)?;
    }
    if !dist.join("index.html").is_file() {
        return Err(ReleaseError::Invalid(format!(
            "{} holds no built application (no index.html)",
            dist.display()
        )));
    }
    Ok(dist)
}

/// Writes the manifest of the files in `dir`, keeping what the existing
/// manifest says of the release itself.
fn rewrite_manifest(dir: &Path, base_url: Option<&str>) -> Result<(), ReleaseError> {
    let mut manifest = Manifest::read(dir)?;
    manifest.assets = manifest::assets_of(dir, base_url)?;
    manifest.write(dir)?;
    check_release(dir)
}

/// Checks the release in `dir`: the manifest is well formed, the files are
/// what it lists, and each archive holds what an installation reads from it.
pub fn check_release(dir: &Path) -> Result<(), ReleaseError> {
    let manifest = Manifest::read(dir)?;
    let mut problems = manifest.problems();
    problems.extend(manifest::verify_files(dir, &manifest)?);
    problems.extend(content_problems(dir, &manifest)?);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ReleaseError::NotWhole {
            dir: dir.to_owned(),
            problems: problems.join("\n"),
        })
    }
}

/// The architectures of the server archives a manifest lists.
fn architectures(manifest: &Manifest) -> Vec<String> {
    manifest
        .assets
        .iter()
        .filter_map(|asset| {
            asset
                .name
                .strip_prefix("orch-server-linux-")?
                .strip_suffix(".zip")
                .map(str::to_owned)
        })
        .collect()
}

fn content_problems(dir: &Path, manifest: &Manifest) -> Result<Vec<String>, ReleaseError> {
    let mut found = Vec::new();
    for asset in &manifest.assets {
        if !manifest::is_release_asset_name(&asset.name) {
            found.push(format!(
                "{} is not the name of a file a release has",
                asset.name
            ));
        }
    }
    let architectures = architectures(manifest);
    if architectures.is_empty() {
        found.push("there is no server archive".to_owned());
    }
    for arch in &architectures {
        let names = AssetNames::for_linux(arch, &manifest.tag);
        for name in names.all() {
            if manifest.asset(name).is_none() {
                found.push(format!(
                    "{name} is not listed, and a release for {arch} has it"
                ));
            }
        }
        check_server_archive(dir, &names, manifest, &mut found)?;
        for program in [&names.ctl, &names.connector, &names.node] {
            check_program(dir, program, &mut found)?;
        }
    }
    if let Some(asset) = manifest.asset("orch-frontend-dist.tar.gz") {
        let (names, _) = archive::list_tar_gz(&dir.join(&asset.name), None)?;
        if !names.iter().any(|name| name == "dist/index.html") {
            found.push(format!("{} holds no dist/index.html", asset.name));
        }
    }
    let deploy = format!("orch-deploy-{}.tar.gz", manifest.tag);
    if let Some(asset) = manifest.asset(&deploy) {
        let (names, tag) = archive::list_tar_gz(&dir.join(&asset.name), Some("RELEASE_TAG"))?;
        for wanted in [
            "deploy/update.sh",
            "deploy/compose.yml",
            "deploy/nginx.conf",
        ] {
            if !names.iter().any(|name| name == wanted) {
                found.push(format!("{} holds no {wanted}", asset.name));
            }
        }
        let tag = String::from_utf8_lossy(&tag.unwrap_or_default())
            .trim()
            .to_owned();
        if tag != manifest.tag {
            found.push(format!(
                "{} says it is {tag:?}, and the manifest says {:?}",
                asset.name, manifest.tag
            ));
        }
        if let (_, Some(compose)) =
            archive::list_tar_gz(&dir.join(&asset.name), Some("deploy/compose.yml"))?
        {
            match stock::compose_images(&String::from_utf8_lossy(&compose)) {
                Ok(images) => {
                    for selected in images {
                        if !manifest.images.iter().any(|image| {
                            image.reference == selected.reference && image.digest == selected.digest
                        }) {
                            found.push(format!("{} selects {}, which the manifest does not record with the same index digest", asset.name, selected.reference));
                        }
                    }
                }
                Err(error) => {
                    found.push(format!("{} has unpinned stock images: {error}", asset.name))
                }
            }
        }
        // The configuration that ships has to be one the programs accept: a key
        // they do not know stops them.
        if let (_, Some(config)) =
            archive::list_tar_gz(&dir.join(&asset.name), Some("deploy/config/orch.toml"))?
        {
            let text = String::from_utf8_lossy(&config);
            if let Err(error) = orch_config::load_text::<orch_config::ServerConfig>(
                &text,
                "deploy/config/orch.toml",
                |_| None,
            ) {
                found.push(format!(
                    "{} holds a deploy/config/orch.toml that the programs refuse: {error}",
                    asset.name
                ));
            }
        } else {
            found.push(format!("{} holds no deploy/config/orch.toml", asset.name));
        }
    }
    Ok(found)
}

fn check_program(dir: &Path, name: &str, found: &mut Vec<String>) -> Result<(), ReleaseError> {
    let path = dir.join(name);
    if !path.is_file() {
        return Ok(());
    }
    let mut start = [0_u8; 4];
    let read = {
        use std::io::Read;
        fs::File::open(&path)
            .and_then(|mut file| file.read(&mut start))
            .map_err(io_error(&path))?
    };
    if read < start.len() || start != *b"\x7fELF" {
        found.push(format!("{name} is not a Linux program"));
    }
    Ok(())
}

/// The server archive holds the programs, the schema, the catalogue and the
/// connectors, and the manifest of the connectors says what they are.
fn check_server_archive(
    dir: &Path,
    names: &AssetNames,
    manifest: &Manifest,
    found: &mut Vec<String>,
) -> Result<(), ReleaseError> {
    let path = dir.join(&names.server);
    if !path.is_file() {
        return Ok(());
    }
    let listed = archive::list_zip(&path)?;
    let has = |wanted: &str| listed.iter().any(|name| name == wanted);
    for wanted in [
        "bin/orch-api",
        "bin/orch-gateway",
        "bin/orch-worker",
        "bin/orch-node",
        "share/model-rates.json",
        "share/ca-certificates.crt",
        "connectors/version.json",
        "LICENSE",
        "THIRD_PARTY_NOTICES",
    ] {
        if !has(wanted) {
            found.push(format!("{} holds no {wanted}", names.server));
        }
    }
    if !listed
        .iter()
        .any(|name| name.starts_with("migrations/") && name.ends_with(".sql"))
    {
        found.push(format!("{} holds no migration", names.server));
    }
    if let Some(bytes) = archive::read_zip_entry(&path, "connectors/version.json")? {
        match serde_json::from_slice::<Manifest>(&bytes) {
            Err(error) => found.push(format!("connectors/version.json cannot be read: {error}")),
            Ok(connectors) => {
                if connectors.tag != manifest.tag || connectors.commit != manifest.commit {
                    found.push("connectors/version.json is of another release".to_owned());
                }
                found.extend(connectors.problems());
                let supplied: BTreeSet<_> = connectors
                    .assets
                    .iter()
                    .map(|asset| asset.name.clone())
                    .collect();
                for asset in manifest.assets.iter().filter(|asset| {
                    crate::release::connectors::download_path(&asset.name).is_some()
                }) {
                    if !supplied.contains(&asset.name) {
                        found.push(format!(
                            "{} holds no connector {} listed in the release",
                            names.server, asset.name
                        ));
                    }
                }
                for asset in &connectors.assets {
                    if let Some(wanted) = crate::release::connectors::download_path(&asset.name)
                        && asset.download_url != wanted
                    {
                        found.push(format!(
                            "{} is served from the wrong platform download address",
                            asset.name
                        ));
                    }
                    let wanted = format!("connectors/{}", asset.name);
                    match archive::read_zip_entry(&path, &wanted)? {
                        None => found.push(format!("{} holds no {wanted}", names.server)),
                        Some(content) => {
                            let hash = blake3::hash(&content).to_hex().to_string();
                            let sha256_differs = asset.sha256.as_ref().is_some_and(|listed| {
                                *listed != manifest::sha256_of_bytes(&content)
                            });
                            if content.len() as u64 != asset.size
                                || hash != asset.blake3
                                || sha256_differs
                            {
                                found.push(format!(
                                    "{wanted} is not what connectors/version.json lists"
                                ));
                            }
                            if let Some(published) = manifest.asset(&asset.name)
                                && (published.size != asset.size
                                    || published.blake3 != asset.blake3)
                            {
                                found
                                    .push(format!("{wanted} differs from the release's connector"));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::manifest::Manifest;

    /// A release with every file of one, as small as can be, for the architecture
    /// and tag given, written to a directory.
    fn small_release(dir: &Path, tag: &str) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("the repository");
        let compose = fs::read(root.join("deploy/compose.yml")).expect("the shipped Compose file");
        let names = AssetNames::for_linux("x86_64", tag);
        let program =
            |name: &str| fs::write(dir.join(name), b"\x7fELF and some more").expect("a program");
        for name in [&names.ctl, &names.connector, &names.node] {
            program(name);
        }
        let connector = b"\x7fELF connector".to_vec();
        fs::write(dir.join(&names.connector), &connector)
            .expect("the same connector outside and inside the server");
        let programs = tempfile::tempdir().expect("program inputs");
        fs::write(programs.path().join("orch-connector"), &connector).expect("connector input");
        fs::copy(dir.join(&names.node), programs.path().join("orch-node")).expect("node input");
        let downloads = connector_entries(
            root,
            programs.path(),
            "x86_64",
            tag.trim_start_matches('v'),
            tag,
            &"a".repeat(40),
            &[],
        )
        .expect("the production download bundle");
        let listing: Manifest = downloads
            .iter()
            .find_map(|entry| {
                if entry.path != "connectors/version.json" {
                    return None;
                }
                match &entry.source {
                    archive::Source::Bytes(bytes) => {
                        Some(serde_json::from_slice(bytes).expect("download manifest"))
                    }
                    archive::Source::File(_) => None,
                }
            })
            .expect("the download bundle includes its manifest");
        let file = |path: &str, bytes: &[u8]| Entry::bytes(path, bytes.to_vec(), FILE_MODE);
        let mut server = vec![
            file("bin/orch-api", b"x"),
            file("bin/orch-gateway", b"x"),
            file("bin/orch-worker", b"x"),
            file("bin/orch-node", b"x"),
            file("migrations/0001_initial.sql", b"SELECT 1;"),
            file("share/model-rates.json", b"{}"),
            file("share/ca-certificates.crt", b"x"),
            file("LICENSE", b"x"),
            file("THIRD_PARTY_NOTICES", b"x"),
        ];
        server.extend(downloads);
        archive::write_zip(&dir.join(&names.server), &server).expect("a server archive");
        archive::write_tar_gz(
            &dir.join(&names.frontend),
            &[file("dist/index.html", b"<html>")],
        )
        .expect("a frontend archive");
        archive::write_tar_gz(
            &dir.join(&names.deploy),
            &[
                file("deploy/update.sh", b"#!/bin/sh"),
                file("deploy/compose.yml", &compose),
                file("deploy/nginx.conf", b"events {}"),
                file("deploy/config/orch.toml", b"version = 1\n"),
                file("RELEASE_TAG", format!("{tag}\n").as_bytes()),
            ],
        )
        .expect("a deploy bundle");
        let manifest = Manifest {
            assets: manifest::assets_of(dir, None).expect("assets"),
            images: stock::images(root).expect("the selected stock images"),
            ..listing
        };
        manifest.write(dir).expect("a manifest");
    }

    fn problems(dir: &Path) -> String {
        match check_release(dir) {
            Ok(()) => String::new(),
            Err(ReleaseError::NotWhole { problems, .. }) => problems,
            Err(other) => other.to_string(),
        }
    }

    #[test]
    fn a_release_cannot_record_a_different_stock_index_than_its_compose_file_selects() {
        let dir = tempfile::tempdir().expect("a directory");
        small_release(dir.path(), "v0.1.0");
        let mut manifest = Manifest::read(dir.path()).expect("a manifest");
        manifest.images[0].digest = format!("sha256:{}", "b".repeat(64));
        manifest.write(dir.path()).expect("a manifest");
        assert!(
            problems(dir.path())
                .contains("the manifest does not record with the same index digest")
        );
    }

    #[test]
    fn a_whole_release_passes_and_a_damaged_piece_is_named() {
        let dir = tempfile::tempdir().expect("a directory");
        small_release(dir.path(), "v0.1.0");
        assert_eq!(problems(dir.path()), "");

        // A program that is no program of this system.
        fs::write(dir.path().join("orchctl-linux-x86_64-musl"), b"#!/bin/sh").expect("changed");
        let found = problems(dir.path());
        assert!(
            found.contains("orchctl-linux-x86_64-musl is 9 bytes"),
            "{found}"
        );
        assert!(found.contains("is not a Linux program"), "{found}");
    }

    #[test]
    fn the_server_serves_native_macos_and_windows_connectors_with_their_exact_installer_hashes() {
        let dir = tempfile::tempdir().expect("a directory");
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("the repository");
        fs::write(
            dir.path().join("orch-connector"),
            b"\x7fELF linux connector",
        )
        .unwrap();
        fs::write(dir.path().join("orch-node"), b"\x7fELF linux node").unwrap();
        let mac = dir.path().join("orch-connector-macos-aarch64");
        let win = dir.path().join("orch-connector-windows-x86_64.exe");
        fs::write(&mac, b"\xcf\xfa\xed\xfe native macOS bytes").unwrap();
        fs::write(&win, b"MZ native Windows bytes").unwrap();
        let extra = vec![
            ("orch-connector-macos-aarch64".to_owned(), mac),
            ("orch-connector-windows-x86_64.exe".to_owned(), win),
        ];
        let entries = connector_entries(
            root,
            dir.path(),
            "x86_64",
            "1.2.3",
            "v1.2.3",
            &"a".repeat(40),
            &extra,
        )
        .unwrap();
        let archive = dir.path().join("server.zip");
        archive::write_zip(&archive, &entries).unwrap();
        let bytes = archive::read_zip_entry(&archive, "connectors/version.json")
            .unwrap()
            .unwrap();
        let listing: Manifest = serde_json::from_slice(&bytes).unwrap();
        for (name, os, arch, expected) in [
            (
                "orch-connector-macos-aarch64",
                "macos",
                "aarch64",
                b"\xcf\xfa\xed\xfe native macOS bytes".as_slice(),
            ),
            (
                "orch-connector-windows-x86_64.exe",
                "windows",
                "x86_64",
                b"MZ native Windows bytes".as_slice(),
            ),
        ] {
            let asset = listing.asset(name).unwrap();
            assert_eq!(
                asset.download_url,
                format!("/api/v1/downloads/connector/{os}/{arch}")
            );
            assert_eq!(
                asset.sha256.as_deref(),
                Some(manifest::sha256_of_bytes(expected).as_str())
            );
            assert_eq!(
                archive::read_zip_entry(&archive, &format!("connectors/{name}"))
                    .unwrap()
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn a_release_whose_deploy_bundle_names_another_tag_or_holds_a_bad_configuration_is_refused() {
        let dir = tempfile::tempdir().expect("a directory");
        small_release(dir.path(), "v0.1.0");
        let names = AssetNames::for_linux("x86_64", "v0.1.0");
        let file = |path: &str, bytes: &[u8]| Entry::bytes(path, bytes.to_vec(), FILE_MODE);
        archive::write_tar_gz(
            &dir.path().join(&names.deploy),
            &[
                file("deploy/update.sh", b"x"),
                file("deploy/compose.yml", b"x"),
                file("deploy/nginx.conf", b"x"),
                file(
                    "deploy/config/orch.toml",
                    b"[auth]\nlogin_failure_limt = 3\n",
                ),
                file("RELEASE_TAG", b"v9.9.9\n"),
            ],
        )
        .expect("a deploy bundle");
        manifest::assets_of(dir.path(), None).expect("assets");
        let mut manifest = Manifest::read(dir.path()).expect("read");
        manifest.assets = manifest::assets_of(dir.path(), None).expect("assets");
        manifest.write(dir.path()).expect("written");
        let found = problems(dir.path());
        assert!(found.contains("says it is \"v9.9.9\""), "{found}");
        assert!(found.contains("that the programs refuse"), "{found}");
        assert!(found.contains("login_failure_limt"), "{found}");
    }

    #[test]
    fn a_release_that_lacks_a_file_of_the_platform_is_refused() {
        let dir = tempfile::tempdir().expect("a directory");
        small_release(dir.path(), "v0.1.0");
        fs::remove_file(dir.path().join("orch-node-linux-x86_64-musl")).expect("removed");
        let mut manifest = Manifest::read(dir.path()).expect("read");
        manifest.assets = manifest::assets_of(dir.path(), None).expect("assets");
        manifest.write(dir.path()).expect("written");
        let found = problems(dir.path());
        assert!(
            found.contains("orch-node-linux-x86_64-musl is not listed"),
            "{found}"
        );
    }

    #[test]
    fn what_an_installation_keeps_or_hides_is_not_in_the_bundle() {
        for path in [
            "runtime/postgres/data/PG_VERSION",
            ".env",
            ".env.local",
            "config/secrets/orch_master_key",
            "config/ca-certificates/company.pem",
            "config/ca-certificates/01234567.0",
        ] {
            assert!(skipped_in_deploy(path), "{path}");
        }
        for path in [
            "update.sh",
            ".env.example",
            "config/orch.toml",
            "config/secrets/.gitkeep",
            "config/ca-certificates/.gitignore",
            "config/ca-certificates/README.md",
            "install/install.sh",
        ] {
            assert!(!skipped_in_deploy(path), "{path}");
        }
    }
}
