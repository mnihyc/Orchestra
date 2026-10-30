//! Native connector builds verified before inclusion in server downloads.

use crate::release::manifest::{
    self, Asset, CONNECTOR_MINIMUM, LinkProtocol, Manifest, PROJECT, SCHEMA_VERSION,
};
use crate::release::{ReleaseError, capture, run};
use orch_link_contract::version::{OLDEST_PROTOCOL_VERSION, PROTOCOL_VERSION};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The supported connector targets and their distribution file names.
pub const TARGETS: [(&str, &str, &str, &str); 5] = [
    (
        "x86_64-unknown-linux-musl",
        "linux",
        "x86_64",
        "orch-connector-linux-x86_64-musl",
    ),
    (
        "aarch64-unknown-linux-musl",
        "linux",
        "aarch64",
        "orch-connector-linux-aarch64-musl",
    ),
    (
        "x86_64-apple-darwin",
        "macos",
        "x86_64",
        "orch-connector-macos-x86_64",
    ),
    (
        "aarch64-apple-darwin",
        "macos",
        "aarch64",
        "orch-connector-macos-aarch64",
    ),
    (
        "x86_64-pc-windows-msvc",
        "windows",
        "x86_64",
        "orch-connector-windows-x86_64.exe",
    ),
];

/// Nodes are shipped for both native Linux architectures alongside the connector inputs.
pub const NODE_TARGETS: [(&str, &str, &str, &str); 2] = [
    (
        "x86_64-unknown-linux-musl",
        "linux",
        "x86_64",
        "orch-node-linux-x86_64-musl",
    ),
    (
        "aarch64-unknown-linux-musl",
        "linux",
        "aarch64",
        "orch-node-linux-aarch64-musl",
    ),
];

/// The system's download address for a supported connector file.
#[must_use]
pub fn download_path(name: &str) -> Option<String> {
    TARGETS
        .iter()
        .find(|(_, _, _, file)| *file == name)
        .map(|(_, os, arch, _)| format!("/api/v1/downloads/connector/{os}/{arch}"))
        .or_else(|| {
            NODE_TARGETS
                .iter()
                .find(|(_, _, _, file)| *file == name)
                .map(|(_, _, arch, _)| format!("/api/v1/downloads/node/{arch}"))
        })
}

fn invalid(why: impl Into<String>) -> ReleaseError {
    ReleaseError::Invalid(why.into())
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> ReleaseError + '_ {
    move |source| ReleaseError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Checks the file's executable format against the platform its name promises.
pub fn check_program(path: &Path, name: &str) -> Result<(), ReleaseError> {
    let Some((_, os, _, _)) = TARGETS
        .iter()
        .chain(NODE_TARGETS.iter())
        .find(|(_, _, _, file)| *file == name)
    else {
        return Err(invalid(format!("{name} is not a supported connector")));
    };
    let mut head = [0_u8; 4];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .map_err(io(path))?;
    let good = match *os {
        "linux" => head == *b"\x7fELF",
        "macos" => matches!(head, [0xcf, 0xfa, 0xed, 0xfe] | [0xfe, 0xed, 0xfa, 0xcf]),
        "windows" => head.starts_with(b"MZ"),
        _ => false,
    };
    if good {
        Ok(())
    } else {
        Err(invalid(format!("{name} is not a {os} program")))
    }
}

/// Builds and runs one connector on its actual operating system and processor, then records its hashes.
pub fn build(root: &Path, out: &Path, target: &str) -> Result<(), ReleaseError> {
    let Some((_, os, arch, name)) = TARGETS.iter().find(|(triple, _, _, _)| *triple == target)
    else {
        return Err(invalid(format!("unsupported connector target {target:?}")));
    };
    let native_os = if cfg!(target_os = "macos") {
        "macos"
    } else {
        std::env::consts::OS
    };
    if *os != native_os || *arch != std::env::consts::ARCH {
        return Err(invalid(format!(
            "{target} needs its actual {os}/{arch} runner; this is {native_os}/{}",
            std::env::consts::ARCH
        )));
    }
    if out.exists() && fs::read_dir(out).map_err(io(out))?.next().is_some() {
        return Err(invalid(format!("{} is not empty", out.display())));
    }
    fs::create_dir_all(out).map_err(io(out))?;
    let release = orch_build_support::release_identity(env!("CARGO_PKG_VERSION"))?;
    let tag = release
        .tag
        .unwrap_or_else(|| format!("v{}", release.version));
    let commit = capture(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "HEAD"]),
    )?;
    let target_dir =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let built = if *os == "linux" {
        // The pinned builder supplies musl and the native C dependencies; no runner packages are installed.
        let counters = std::env::var_os("ORCH_BUILD_NUMBER_DIR")
            .map_or_else(|| root.join("build/build-numbers"), PathBuf::from);
        let builder = crate::release::container::Builder::new(
            root,
            &target_dir.join("container"),
            &counters,
        )?;
        builder.ensure_image()?;
        let variables = vec![
            ("ORCH_RELEASE_VERSION".to_owned(), release.version.clone()),
            ("ORCH_RELEASE_TAG".to_owned(), tag.clone()),
        ];
        builder.cargo(
            &[
                "test",
                "--lib",
                "--locked",
                "-p",
                "orch-connector-core",
                "--target",
                target,
            ],
            &variables,
        )?;
        builder.cargo(
            &[
                "build",
                "--release",
                "--locked",
                "-p",
                "orch-connector",
                "-p",
                "orch-node",
                "--target",
                target,
            ],
            &variables,
        )?;
        builder.target_dir()
    } else {
        run(Command::new("cargo").current_dir(root).args([
            "test",
            "--lib",
            "--locked",
            "-p",
            "orch-connector-core",
            "--target",
            target,
        ]))?;
        run(Command::new("cargo")
            .current_dir(root)
            .args([
                "build",
                "--release",
                "--locked",
                "-p",
                "orch-connector",
                "--target",
                target,
            ])
            .env("ORCH_RELEASE_VERSION", &release.version)
            .env("ORCH_RELEASE_TAG", &tag))?;
        target_dir
    };
    let program = built
        .join(target)
        .join("release")
        .join(if *os == "windows" {
            "orch-connector.exe"
        } else {
            "orch-connector"
        });
    let said = capture(Command::new(&program).arg("version"))?;
    if !said.starts_with(&format!("connector {} (build ", release.version))
        || !said.contains(&format!("[{tag}]"))
    {
        return Err(invalid(format!(
            "{} reports another release: {said}",
            program.display()
        )));
    }
    let path = out.join(name);
    fs::copy(&program, &path).map_err(io(&path))?;
    check_program(&path, name)?;
    let (size, blake3) = manifest::describe_file(&path)?;
    let mut assets = vec![Asset {
        name: (*name).to_owned(),
        download_url: (*name).to_owned(),
        size,
        blake3,
        sha256: Some(manifest::sha256_of(&path)?),
    }];
    if let Some((_, _, _, node_name)) = NODE_TARGETS
        .iter()
        .find(|(triple, _, _, _)| *triple == target)
    {
        let path = out.join(node_name);
        fs::copy(built.join(target).join("release/orch-node"), &path).map_err(io(&path))?;
        check_program(&path, node_name)?;
        let (size, blake3) = manifest::describe_file(&path)?;
        assets.push(Asset {
            name: (*node_name).into(),
            download_url: (*node_name).into(),
            size,
            blake3,
            sha256: Some(manifest::sha256_of(&path)?),
        });
    }
    Manifest {
        schema_version: SCHEMA_VERSION,
        project: PROJECT.to_owned(),
        tag,
        version: release.version,
        commit,
        link_protocol: LinkProtocol {
            current: PROTOCOL_VERSION,
            minimum: OLDEST_PROTOCOL_VERSION,
        },
        connector_minimum: CONNECTOR_MINIMUM.to_owned(),
        assets,
        images: Vec::new(),
        harness_builds: Vec::new(),
    }
    .write(out)
}

/// Reads verified connector builds of this exact release, refusing mixed commits, duplicate targets or damaged bytes.
pub fn read(
    dirs: &[PathBuf],
    version: &str,
    tag: &str,
    commit: &str,
) -> Result<Vec<(String, PathBuf)>, ReleaseError> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    for dir in dirs {
        let listing = Manifest::read(dir)?;
        let mut faults = listing.problems();
        faults.extend(manifest::verify_files(dir, &listing)?);
        if listing.version != version || listing.tag != tag || listing.commit != commit {
            faults.push("connector inputs are of another release or commit".to_owned());
        }
        if listing.link_protocol.current != PROTOCOL_VERSION
            || listing.link_protocol.minimum != OLDEST_PROTOCOL_VERSION
        {
            faults.push("connector inputs speak another link protocol".to_owned());
        }
        if !faults.is_empty() {
            return Err(invalid(format!("{}: {}", dir.display(), faults.join("; "))));
        }
        for asset in listing.assets {
            if download_path(&asset.name).is_none() {
                return Err(invalid(format!(
                    "{} is not a supported connector input",
                    asset.name
                )));
            }
            if !seen.insert(asset.name.clone()) {
                return Err(invalid(format!("{} is supplied twice", asset.name)));
            }
            let path = dir.join(&asset.name);
            let metadata = fs::symlink_metadata(&path).map_err(io(&path))?;
            if !metadata.file_type().is_file() {
                return Err(invalid(format!("{} is not a regular file", path.display())));
            }
            if asset.sha256.as_deref() != Some(manifest::sha256_of(&path)?.as_str()) {
                return Err(invalid(format!(
                    "{} does not match its required SHA-256",
                    asset.name
                )));
            }
            check_program(&path, &asset.name)?;
            found.push((asset.name, path));
        }
    }
    found.sort();
    Ok(found)
}

/// Refuses a release that does not supply every declared connector platform.
pub fn require_all(names: &BTreeSet<String>) -> Result<(), ReleaseError> {
    let missing: Vec<_> = TARGETS
        .iter()
        .chain(NODE_TARGETS.iter())
        .filter(|(_, _, _, name)| !names.contains(*name))
        .map(|(_, _, _, name)| *name)
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(invalid(format!(
            "missing connector builds: {}",
            missing.join(", ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(dir: &Path, name: &str, bytes: &[u8]) {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        Manifest {
            schema_version: SCHEMA_VERSION,
            project: PROJECT.to_owned(),
            tag: "v1.2.3".to_owned(),
            version: "1.2.3".to_owned(),
            commit: "a".repeat(40),
            link_protocol: LinkProtocol {
                current: PROTOCOL_VERSION,
                minimum: OLDEST_PROTOCOL_VERSION,
            },
            connector_minimum: CONNECTOR_MINIMUM.to_owned(),
            assets: vec![Asset {
                name: name.to_owned(),
                download_url: name.to_owned(),
                size: bytes.len() as u64,
                blake3: blake3::hash(bytes).to_hex().to_string(),
                sha256: Some(manifest::sha256_of_bytes(bytes)),
            }],
            images: Vec::new(),
            harness_builds: Vec::new(),
        }
        .write(dir)
        .unwrap();
    }

    #[test]
    fn a_native_connector_of_this_release_has_its_own_download_address() {
        let dir = tempfile::tempdir().unwrap();
        input(
            dir.path(),
            "orch-connector-macos-aarch64",
            b"\xcf\xfa\xed\xfe connector",
        );
        let files = read(&[dir.path().to_owned()], "1.2.3", "v1.2.3", &"a".repeat(40)).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            download_path(&files[0].0).unwrap(),
            "/api/v1/downloads/connector/macos/aarch64"
        );
        assert_eq!(
            fs::read(&files[0].1).unwrap(),
            b"\xcf\xfa\xed\xfe connector"
        );
    }

    #[test]
    fn damaged_or_mixed_or_duplicate_connector_builds_are_refused_before_packaging() {
        let dir = tempfile::tempdir().unwrap();
        let name = "orch-connector-windows-x86_64.exe";
        input(dir.path(), name, b"MZ windows connector");
        assert!(
            read(&[dir.path().to_owned()], "1.2.3", "v1.2.3", &"b".repeat(40))
                .unwrap_err()
                .to_string()
                .contains("another release or commit")
        );
        assert!(
            read(
                &[dir.path().to_owned(), dir.path().to_owned()],
                "1.2.3",
                "v1.2.3",
                &"a".repeat(40)
            )
            .unwrap_err()
            .to_string()
            .contains("supplied twice")
        );
        fs::write(dir.path().join(name), b"MZ corrupted connector").unwrap();
        assert!(read(&[dir.path().to_owned()], "1.2.3", "v1.2.3", &"a".repeat(40)).is_err());
        input(dir.path(), name, b"\x7fELF misnamed connector");
        assert!(
            read(&[dir.path().to_owned()], "1.2.3", "v1.2.3", &"a".repeat(40))
                .unwrap_err()
                .to_string()
                .contains("not a windows program")
        );
    }
}
