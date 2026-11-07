//! Exact candidate sources in release archives, without changing generic offline verification.

use super::{ReleaseError, archive, connectors, manifest::Manifest, package};
use flate2::read::GzDecoder;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

fn io(path: &Path, source: std::io::Error) -> ReleaseError {
    ReleaseError::Io {
        path: path.to_owned(),
        source,
    }
}

fn compare_zip(
    path: &Path,
    entry: &archive::Entry,
    found: &mut Vec<String>,
) -> Result<(), ReleaseError> {
    if archive::read_zip_entry(path, &entry.path)?.as_deref() != Some(entry.content()?.as_slice()) {
        found.push(format!(
            "{} has missing or different candidate source {}",
            path.display(),
            entry.path
        ));
    }
    Ok(())
}

fn compare_tar(
    path: &Path,
    expected: Vec<archive::Entry>,
    found: &mut Vec<String>,
) -> Result<(), ReleaseError> {
    let expected: BTreeMap<_, _> = expected
        .into_iter()
        .map(|entry| Ok((entry.path.clone(), entry.content()?)))
        .collect::<Result<_, ReleaseError>>()?;
    let mut seen = BTreeSet::new();
    let file = fs::File::open(path).map_err(|error| io(path, error))?;
    let mut archive = tar::Archive::new(GzDecoder::new(file));
    for entry in archive.entries().map_err(|error| io(path, error))? {
        let mut entry = entry.map_err(|error| io(path, error))?;
        let name = entry
            .path()
            .map_err(|error| io(path, error))?
            .to_string_lossy()
            .into_owned();
        if !seen.insert(name.clone()) {
            found.push(format!("{} repeats {name}", path.display()));
        }
        let Some(wanted) = expected.get(&name) else {
            found.push(format!(
                "{} has unexpected candidate entry {name}",
                path.display()
            ));
            continue;
        };
        let mut bytes = Vec::new();
        (&mut entry)
            .take(wanted.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| io(path, error))?;
        if !entry.header().entry_type().is_file() || bytes != *wanted {
            found.push(format!(
                "{} differs from candidate source {name}",
                path.display()
            ));
        }
    }
    for name in expected.keys() {
        if !seen.contains(name) {
            found.push(format!(
                "{} is missing candidate source {name}",
                path.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn problems(
    root: &Path,
    dir: &Path,
    manifest: &Manifest,
    head: &str,
) -> Result<Vec<String>, ReleaseError> {
    let mut found = Vec::new();
    if manifest.commit != head {
        found.push("the release does not name the checked-out candidate commit".into());
    }
    if manifest.harness_builds != package::harness_builds(root)? {
        found.push("the harness pins differ from the candidate".into());
    }
    let migrations = archive::walk(&root.join("migrations"), "migrations/", &|path| {
        !path.ends_with(".sql")
    })?;
    if migrations.is_empty() {
        found.push("the candidate has no migration sources".into());
    }
    let migration_names: BTreeSet<_> = migrations.iter().map(|entry| entry.path.clone()).collect();
    let mut source_files = migrations;
    source_files.push(archive::Entry::file(
        "share/model-rates.json",
        &root.join("deploy/model-rates.json"),
    )?);
    for name in package::INSTALLERS {
        source_files.push(archive::Entry::file(
            format!("connectors/{name}"),
            &root.join("deploy/install").join(name),
        )?);
    }
    for name in ["LICENSE", "NOTICE"] {
        source_files.push(archive::Entry::file(name, &root.join(name))?);
    }
    let mut allowed: BTreeSet<_> = source_files
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    allowed.extend(package::SERVER_PROGRAMS.map(|program| format!("bin/{program}")));
    allowed.extend(
        [
            "bin/orch-node",
            "share/ca-certificates.crt",
            "connectors/version.json",
            "THIRD_PARTY_NOTICES",
        ]
        .map(str::to_owned),
    );
    allowed.extend(
        connectors::TARGETS
            .iter()
            .chain(connectors::NODE_TARGETS.iter())
            .map(|(_, _, _, name)| format!("connectors/{name}")),
    );
    for asset in &manifest.assets {
        if !asset.name.starts_with("orch-server-linux-") || !asset.name.ends_with(".zip") {
            continue;
        }
        let path = dir.join(&asset.name);
        let names = archive::list_zip(&path)?;
        let unique: BTreeSet<_> = names.iter().cloned().collect();
        if unique.len() != names.len() {
            found.push(format!("{} repeats an archive entry", asset.name));
        }
        let actual: BTreeSet<_> = unique
            .iter()
            .filter(|name| name.starts_with("migrations/") && name.ends_with(".sql"))
            .cloned()
            .collect();
        if actual != migration_names {
            found.push(format!(
                "{} does not contain the exact candidate migration set",
                asset.name
            ));
        }
        for name in unique.difference(&allowed) {
            found.push(format!(
                "{} has unexpected candidate entry {name}",
                asset.name
            ));
        }
        for entry in &source_files {
            compare_zip(&path, entry, &mut found)?;
        }
    }
    let mut deploy = archive::walk(&root.join("deploy"), "deploy/", &package::skipped_in_deploy)?;
    deploy.extend(archive::walk(&root.join("docs"), "docs/", &|_| false)?);
    for name in ["LICENSE", "NOTICE"] {
        deploy.push(archive::Entry::file(name, &root.join(name))?);
    }
    deploy.push(archive::Entry::bytes(
        "RELEASE_TAG",
        format!("{}\n", manifest.tag).into_bytes(),
        archive::FILE_MODE,
    ));
    deploy.push(archive::Entry::bytes(
        "THIRD_PARTY_NOTICES",
        package::DEPLOY_NOTICES.as_bytes().to_vec(),
        archive::FILE_MODE,
    ));
    compare_tar(
        &dir.join(format!("orch-deploy-{}.tar.gz", manifest.tag)),
        deploy,
        &mut found,
    )?;
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{Asset, LinkProtocol, PROJECT, SCHEMA_VERSION};
    use super::*;

    fn fixture(root: &Path, out: &Path) -> Manifest {
        fs::create_dir_all(root.join("migrations")).unwrap();
        fs::create_dir_all(root.join("deploy/install")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        for (name, text) in [
            ("0001_first.sql", "SELECT 1;\n"),
            ("0002_second.sql", "SELECT 2;\n"),
        ] {
            fs::write(root.join("migrations").join(name), text).unwrap();
        }
        for (name, bytes) in [
            ("LICENSE", b"licence".as_slice()),
            ("NOTICE", b"notice".as_slice()),
            ("deploy/model-rates.json", b"{}".as_slice()),
            ("deploy/harness-builds.json", b"[]".as_slice()),
            (
                "deploy/nginx.conf",
                b"location /openapi/ { proxy_pass $upstream$request_uri; }".as_slice(),
            ),
        ] {
            fs::write(root.join(name), bytes).unwrap();
        }
        for name in package::INSTALLERS {
            fs::write(root.join("deploy/install").join(name), b"installer").unwrap();
        }
        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            project: PROJECT.into(),
            tag: "v1.2.3".into(),
            version: "1.2.3".into(),
            commit: "a".repeat(40),
            link_protocol: LinkProtocol {
                current: 1,
                minimum: 1,
            },
            connector_minimum: "0.1.0".into(),
            assets: vec![Asset {
                name: "orch-server-linux-x86_64.zip".into(),
                download_url: "server".into(),
                size: 1,
                blake3: "a".repeat(64),
                sha256: None,
            }],
            images: vec![],
            harness_builds: vec![],
        };
        write_archives(root, out, &manifest, false, false);
        manifest
    }

    fn write_archives(
        root: &Path,
        out: &Path,
        manifest: &Manifest,
        omit_sql: bool,
        stale_edge: bool,
    ) {
        let mut server = archive::walk(&root.join("migrations"), "migrations/", &|path| {
            omit_sql && path == "0002_second.sql"
        })
        .unwrap();
        server.push(
            archive::Entry::file(
                "share/model-rates.json",
                &root.join("deploy/model-rates.json"),
            )
            .unwrap(),
        );
        for name in package::INSTALLERS {
            server.push(
                archive::Entry::file(
                    format!("connectors/{name}"),
                    &root.join("deploy/install").join(name),
                )
                .unwrap(),
            );
        }
        for name in ["LICENSE", "NOTICE"] {
            server.push(archive::Entry::file(name, &root.join(name)).unwrap());
        }
        archive::write_zip(&out.join("orch-server-linux-x86_64.zip"), &server).unwrap();
        let mut deploy =
            archive::walk(&root.join("deploy"), "deploy/", &package::skipped_in_deploy).unwrap();
        if stale_edge {
            for entry in &mut deploy {
                if entry.path == "deploy/nginx.conf" {
                    *entry = archive::Entry::bytes(
                        &entry.path,
                        b"location /api/ {}".to_vec(),
                        archive::FILE_MODE,
                    );
                }
            }
        }
        for name in ["LICENSE", "NOTICE"] {
            deploy.push(archive::Entry::file(name, &root.join(name)).unwrap());
        }
        deploy.push(archive::Entry::bytes(
            "RELEASE_TAG",
            format!("{}\n", manifest.tag).into_bytes(),
            archive::FILE_MODE,
        ));
        deploy.push(archive::Entry::bytes(
            "THIRD_PARTY_NOTICES",
            package::DEPLOY_NOTICES.as_bytes().to_vec(),
            archive::FILE_MODE,
        ));
        archive::write_tar_gz(&out.join("orch-deploy-v1.2.3.tar.gz"), &deploy).unwrap();
    }

    #[test]
    fn source_check_accepts_all_schema_and_deployment_sources_of_the_pinned_commit() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let manifest = fixture(root.path(), out.path());
        assert!(
            problems(root.path(), out.path(), &manifest, &"a".repeat(40))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn source_check_refuses_a_missing_or_changed_migration_even_when_some_sql_is_present() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let manifest = fixture(root.path(), out.path());
        write_archives(root.path(), out.path(), &manifest, true, false);
        assert!(
            problems(root.path(), out.path(), &manifest, &manifest.commit)
                .unwrap()
                .join("\n")
                .contains("exact candidate migration set")
        );
        write_archives(root.path(), out.path(), &manifest, false, false);
        fs::write(
            root.path().join("migrations/0001_first.sql"),
            b"SELECT 'new schema';",
        )
        .unwrap();
        assert!(
            problems(root.path(), out.path(), &manifest, &manifest.commit)
                .unwrap()
                .join("\n")
                .contains("0001_first.sql")
        );
    }

    #[test]
    fn source_check_refuses_extra_schema_entries_from_another_release() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let manifest = fixture(root.path(), out.path());
        let path = out.path().join("orch-server-linux-x86_64.zip");
        let mut entries = archive::list_zip(&path)
            .unwrap()
            .into_iter()
            .map(|name| {
                let bytes = archive::read_zip_entry(&path, &name).unwrap().unwrap();
                archive::Entry::bytes(name, bytes, archive::FILE_MODE)
            })
            .collect::<Vec<_>>();
        entries.push(archive::Entry::bytes(
            "migrations/0003_old.sql",
            b"SELECT 'other release';".to_vec(),
            archive::FILE_MODE,
        ));
        archive::write_zip(&path, &entries).unwrap();
        let found = problems(root.path(), out.path(), &manifest, &manifest.commit)
            .unwrap()
            .join("\n");
        assert!(found.contains("exact candidate migration set"));
        assert!(found.contains("unexpected candidate entry migrations/0003_old.sql"));
    }

    #[test]
    fn source_check_refuses_a_stale_key_edge_wrong_commit_or_different_harness_pins() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let mut manifest = fixture(root.path(), out.path());
        write_archives(root.path(), out.path(), &manifest, false, true);
        manifest.harness_builds = vec![serde_json::json!({"different":"pin"})];
        let found = problems(root.path(), out.path(), &manifest, &"b".repeat(40))
            .unwrap()
            .join("\n");
        assert!(found.contains("deploy/nginx.conf"));
        assert!(found.contains("candidate commit"));
        assert!(found.contains("harness pins"));
    }
}
