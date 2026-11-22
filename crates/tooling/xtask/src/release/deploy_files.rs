//! The deployment files of a release: what the bundle says of them, and what `update.sh` reads.
//!
//! The files of `deploy/` (but `config/`) follow the release: an update writes them, a rollback puts them back. The bundle's
//! `deploy/FILE_SETS` is what the updater goes by: the generation of the files in the release, the oldest generation its
//! programs work with, the set of files that must agree with each other, and the SHA-256 of every file of every published
//! release, so that a file that equals none of them is known for the operator's edit. The copy in the repository holds the
//! published releases' rows; packaging adds the release's own, and this module is the one place that does it and the one
//! place that reads it again, as the updater will.

use crate::release::ReleaseError;
use crate::release::archive::{self, Entry, FILE_MODE};
use crate::release::manifest::{self, DeployFiles, Manifest, UPDATER};
use std::collections::BTreeMap;
use std::path::Path;

/// The table, as an entry of the bundle.
pub const TABLE: &str = "deploy/FILE_SETS";
/// The updater, as an entry of the bundle.
const UPDATER_ENTRY: &str = "deploy/update.sh";

/// One file of one published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The release.
    pub tag: String,
    /// The generation of the files in it.
    pub generation: u32,
    /// The file's SHA-256, in lowercase hexadecimal.
    pub sha256: String,
    /// Its path below `deploy/`.
    pub path: String,
}

/// What `FILE_SETS` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// The generation of the files in the release.
    pub generation: u32,
    /// The oldest generation of the main set the release's programs work with.
    pub minimum: u32,
    /// The files that must agree with each other.
    pub main: Vec<String>,
    /// What each published release shipped.
    pub rows: Vec<Row>,
}

/// Whether an update brings the file (a path below `deploy/`): everything but `config/` and the table itself.
#[must_use]
pub fn followed(path: &str) -> bool {
    !path.starts_with("config/") && path != "FILE_SETS"
}

/// Reads a `FILE_SETS`; one sentence says what is wrong with it.
pub fn parse(text: &str) -> Result<Table, String> {
    let (mut format, mut generation, mut minimum) = (None, None, None);
    let mut main = Vec::new();
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some((kind, rest)) = words.split_first() else {
            continue;
        };
        if kind.starts_with('#') {
            continue;
        }
        let wrong = || format!("line {} cannot be read", index + 1);
        match (*kind, rest) {
            ("format", [value]) => format = Some(number(value).ok_or_else(wrong)?),
            ("generation", [value]) => generation = Some(number(value).ok_or_else(wrong)?),
            ("minimum", [value]) => minimum = Some(number(value).ok_or_else(wrong)?),
            ("set", [name, members @ ..]) if !members.is_empty() => {
                if *name == "main" {
                    main = members.iter().map(|member| (*member).to_owned()).collect();
                }
            }
            ("file", [tag, generation, sha256, path]) => {
                let sha = sha256.len() == 64
                    && sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
                if !sha || !is_tag(tag) {
                    return Err(wrong());
                }
                rows.push(Row {
                    tag: (*tag).to_owned(),
                    generation: number(generation).ok_or_else(wrong)?,
                    sha256: (*sha256).to_owned(),
                    path: (*path).to_owned(),
                });
            }
            _ => return Err(wrong()),
        }
    }
    let (Some(1), Some(generation), Some(minimum)) = (format, generation, minimum) else {
        return Err("it needs format 1, a generation and a minimum".to_owned());
    };
    if main.is_empty() {
        return Err("it names no main set".to_owned());
    }
    if minimum > generation {
        return Err("it asks for a minimum generation above its own".to_owned());
    }
    Ok(Table {
        generation,
        minimum,
        main,
        rows,
    })
}

fn number(word: &str) -> Option<u32> {
    word.parse::<u32>().ok()
}

/// `vMAJOR.MINOR.PATCH` with an optional suffix, as every tag is.
fn is_tag(text: &str) -> bool {
    text.strip_prefix('v')
        .is_some_and(manifest::is_release_version)
}

/// The followed files of the tree's `deploy/`, with their bytes, by path below `deploy/`.
fn tree_files(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, ReleaseError> {
    let mut files = BTreeMap::new();
    for entry in archive::walk(
        &root.join("deploy"),
        "deploy/",
        &super::package::skipped_in_deploy,
    )? {
        if let Some(path) = entry.path.strip_prefix("deploy/")
            && followed(path)
        {
            files.insert(path.to_owned(), entry.content()?);
        }
    }
    Ok(files)
}

/// The `FILE_SETS` the bundle of `tag` carries: the repository's, with this release's own rows added, and what the manifest
/// says of it.
pub fn amended(root: &Path, tag: &str) -> Result<(Entry, DeployFiles), ReleaseError> {
    let path = root.join(TABLE);
    let text = std::fs::read_to_string(&path).map_err(|source| ReleaseError::Io {
        path: path.clone(),
        source,
    })?;
    let invalid = |why: String| ReleaseError::Invalid(format!("{TABLE}: {why}"));
    let table = parse(&text).map_err(invalid)?;
    if table.rows.iter().any(|row| row.tag == tag) {
        return Err(invalid(format!(
            "it holds rows for {tag} already; packaging adds a release's own"
        )));
    }
    let files = tree_files(root)?;
    for member in &table.main {
        if !files.contains_key(member) {
            return Err(invalid(format!(
                "the main set names {member}, and deploy/ has no such file"
            )));
        }
    }
    if !files.contains_key("update.sh") {
        return Err(invalid("deploy/update.sh is not there".to_owned()));
    }
    use std::fmt::Write as _;
    let mut out = text.trim_end_matches('\n').to_owned();
    out.push('\n');
    for (name, bytes) in &files {
        let _ = writeln!(
            out,
            "file {tag} {} {} {name}",
            table.generation,
            manifest::sha256_of_bytes(bytes)
        );
    }
    Ok((
        Entry::bytes(TABLE, out.into_bytes(), FILE_MODE),
        DeployFiles {
            generation: table.generation,
            minimum: table.minimum,
        },
    ))
}

/// What is wrong with the bundle's table or the updater beside it, one sentence each; nothing when the manifest says nothing
/// of deployment files.
pub fn problems(dir: &Path, manifest: &Manifest) -> Result<Vec<String>, ReleaseError> {
    let mut found = Vec::new();
    let Some(said) = manifest.deploy_files else {
        return Ok(found);
    };
    let name = format!("orch-deploy-{}.tar.gz", manifest.tag);
    let Some(asset) = manifest.asset(&name) else {
        found.push(format!(
            "the manifest has deployment files and does not list {name}"
        ));
        return Ok(found);
    };
    let files = archive::read_tar_gz_files(&dir.join(&asset.name), "deploy/")?;
    let Some(text) = files.get(TABLE) else {
        found.push(format!("{} holds no {TABLE}", asset.name));
        return Ok(found);
    };
    let table = match parse(&String::from_utf8_lossy(text)) {
        Ok(table) => table,
        Err(why) => {
            found.push(format!("{TABLE} in {}: {why}", asset.name));
            return Ok(found);
        }
    };
    if table.generation != said.generation || table.minimum != said.minimum {
        found.push(format!(
            "{TABLE} says generation {} and minimum {}, and the manifest says {} and {}",
            table.generation, table.minimum, said.generation, said.minimum
        ));
    }
    for member in &table.main {
        if !files.contains_key(&format!("deploy/{member}")) {
            found.push(format!(
                "{} holds no {member}, which the main set names",
                asset.name
            ));
        }
    }
    let own: BTreeMap<&str, &Row> = table
        .rows
        .iter()
        .filter(|row| row.tag == manifest.tag)
        .map(|row| (row.path.as_str(), row))
        .collect();
    for (entry, bytes) in &files {
        let Some(path) = entry.strip_prefix("deploy/") else {
            continue;
        };
        if !followed(path) {
            continue;
        }
        match own.get(path) {
            None => found.push(format!("{TABLE} has no row of {} for {path}", manifest.tag)),
            Some(row) if row.sha256 != manifest::sha256_of_bytes(bytes) => found.push(format!(
                "{TABLE} gives {path} a SHA-256 that is not the file's"
            )),
            Some(row) if row.generation != table.generation => found.push(format!(
                "{TABLE} gives {path} generation {}, and the files are generation {}",
                row.generation, table.generation
            )),
            Some(_) => {}
        }
    }
    for path in own.keys() {
        if !files.contains_key(&format!("deploy/{path}")) {
            found.push(format!(
                "{TABLE} has a row for {path}, which the bundle does not hold"
            ));
        }
    }
    match manifest.asset(UPDATER) {
        None => found.push(format!(
            "{UPDATER} is not published beside the bundle, and the release's notes give its SHA-256"
        )),
        Some(listed) => {
            let beside =
                std::fs::read(dir.join(&listed.name)).map_err(|source| ReleaseError::Io {
                    path: dir.join(&listed.name),
                    source,
                })?;
            if files.get(UPDATER_ENTRY) != Some(&beside) {
                found.push(format!(
                    "{UPDATER} is not the {UPDATER_ENTRY} of the bundle"
                ));
            }
            if listed.sha256.as_deref() != Some(manifest::sha256_of_bytes(&beside).as_str()) {
                found.push(format!(
                    "{UPDATER} is listed without its SHA-256, or with another"
                ));
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn text(generation: u32, minimum: u32) -> String {
        format!(
            "# a comment\nformat 1\ngeneration {generation}\nminimum {minimum}\nset main compose.yml nginx.conf\nfile v0.1.2 1 {SHA} nginx.conf\n"
        )
    }

    #[test]
    fn a_table_is_read_as_the_updater_reads_it() {
        let table = parse(&text(2, 1)).expect("a table");
        assert_eq!((table.generation, table.minimum), (2, 1));
        assert_eq!(table.main, ["compose.yml", "nginx.conf"]);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].tag, "v0.1.2");
        assert_eq!(table.rows[0].path, "nginx.conf");
    }

    #[test]
    fn a_table_that_an_updater_would_not_read_is_refused_here() {
        assert!(parse(&text(1, 2)).unwrap_err().contains("minimum"));
        assert!(
            parse("generation 1\nminimum 1\nset main a\n")
                .unwrap_err()
                .contains("format 1")
        );
        assert!(
            parse("format 1\ngeneration 1\nminimum 1\n")
                .unwrap_err()
                .contains("main set")
        );
        let bad = format!("{}file v0.1.2 1 nothex nginx.conf\n", text(1, 1));
        assert!(parse(&bad).unwrap_err().contains("line 7"));
        assert!(
            parse(&format!("{}surprise 1\n", text(1, 1)))
                .unwrap_err()
                .contains("line 7")
        );
    }

    #[test]
    fn the_files_of_config_and_the_table_are_not_followed() {
        assert!(followed("nginx.conf"));
        assert!(followed("hosts/runtimes/gvisor.sh"));
        assert!(!followed("config/orch.toml"));
        assert!(!followed("FILE_SETS"));
    }

    fn tree(root: &Path, table: &str) {
        for (name, text) in [
            ("deploy/compose.yml", "name: orch\n"),
            ("deploy/nginx.conf", "events {}\n"),
            ("deploy/update.sh", "#!/bin/sh\n"),
            ("deploy/config/orch.toml", "version = 1\n"),
            ("deploy/FILE_SETS", table),
        ] {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    #[test]
    fn packaging_adds_the_rows_of_its_own_release_and_no_file_of_config() {
        let dir = tempfile::tempdir().expect("a directory");
        tree(dir.path(), &text(2, 1));
        let (entry, said) = amended(dir.path(), "v0.1.4").expect("an amended table");
        assert_eq!(
            said,
            DeployFiles {
                generation: 2,
                minimum: 1
            }
        );
        let table = parse(&String::from_utf8(entry.content().unwrap()).unwrap()).expect("a table");
        let own: Vec<&str> = table
            .rows
            .iter()
            .filter(|row| row.tag == "v0.1.4")
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(own, ["compose.yml", "nginx.conf", "update.sh"]);
        assert!(
            table
                .rows
                .iter()
                .filter(|row| row.tag == "v0.1.4")
                .all(|row| row.generation == 2)
        );
        assert_eq!(
            table.rows.iter().filter(|row| row.tag == "v0.1.2").count(),
            1,
            "what the published releases shipped stays"
        );
    }

    #[test]
    fn a_table_that_already_has_the_rows_of_the_release_is_refused() {
        let dir = tempfile::tempdir().expect("a directory");
        tree(
            dir.path(),
            &format!("{}file v0.1.4 2 {SHA} nginx.conf\n", text(2, 1)),
        );
        let refused = amended(dir.path(), "v0.1.4").unwrap_err().to_string();
        assert!(refused.contains("rows for v0.1.4 already"), "{refused}");
    }

    /// A release in `dir`: a bundle made from the tree, the updater beside it, and a manifest that lists both.
    fn release(dir: &Path, root: &Path, updater: &[u8], listed_sha: Option<String>) -> Manifest {
        let (table, said) = amended(root, "v0.1.4").expect("an amended table");
        let mut entries = vec![table];
        for name in ["compose.yml", "nginx.conf", "update.sh"] {
            entries.push(
                Entry::file(format!("deploy/{name}"), &root.join("deploy").join(name)).unwrap(),
            );
        }
        archive::write_tar_gz(&dir.join("orch-deploy-v0.1.4.tar.gz"), &entries).expect("a bundle");
        std::fs::write(dir.join("update.sh"), updater).unwrap();
        let mut assets = vec![serde_json::json!({
            "name": "orch-deploy-v0.1.4.tar.gz", "download_url": "orch-deploy-v0.1.4.tar.gz",
            "size": 1, "blake3": "a".repeat(64)
        })];
        let mut listed = serde_json::json!({
            "name": "update.sh", "download_url": "update.sh", "size": 1, "blake3": "a".repeat(64)
        });
        if let Some(sha) = listed_sha {
            listed["sha256"] = serde_json::Value::String(sha);
        }
        assets.push(listed);
        serde_json::from_value(serde_json::json!({
            "schema_version": 1, "project": "orch", "tag": "v0.1.4", "version": "0.1.4",
            "commit": "a".repeat(40), "link_protocol": {"current": 1, "minimum": 1},
            "connector_minimum": "0.1.0", "assets": assets, "images": [], "harness_builds": [],
            "deploy_files": {"generation": said.generation, "minimum": said.minimum}
        }))
        .expect("a manifest")
    }

    #[test]
    fn a_bundle_and_the_updater_beside_it_that_agree_have_nothing_to_say() {
        let root = tempfile::tempdir().expect("a tree");
        tree(root.path(), &text(2, 1));
        let dir = tempfile::tempdir().expect("a release");
        let updater = std::fs::read(root.path().join("deploy/update.sh")).unwrap();
        let manifest = release(
            dir.path(),
            root.path(),
            &updater,
            Some(manifest::sha256_of_bytes(&updater)),
        );
        assert_eq!(
            problems(dir.path(), &manifest).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn another_updater_beside_the_bundle_or_none_listed_or_no_digest_is_a_problem() {
        let root = tempfile::tempdir().expect("a tree");
        tree(root.path(), &text(2, 1));
        let dir = tempfile::tempdir().expect("a release");
        let found = problems(
            dir.path(),
            &release(
                dir.path(),
                root.path(),
                b"#!/bin/sh\n# changed\n",
                Some(manifest::sha256_of_bytes(b"#!/bin/sh\n# changed\n")),
            ),
        )
        .unwrap()
        .join("\n");
        assert!(
            found.contains("is not the deploy/update.sh of the bundle"),
            "{found}"
        );
        let updater = std::fs::read(root.path().join("deploy/update.sh")).unwrap();
        let found = problems(
            dir.path(),
            &release(dir.path(), root.path(), &updater, None),
        )
        .unwrap()
        .join("\n");
        assert!(found.contains("without its SHA-256"), "{found}");
        let mut manifest = release(
            dir.path(),
            root.path(),
            &updater,
            Some(manifest::sha256_of_bytes(&updater)),
        );
        manifest.assets.retain(|asset| asset.name != "update.sh");
        assert!(
            problems(dir.path(), &manifest)
                .unwrap()
                .join("\n")
                .contains("not published beside the bundle")
        );
    }

    #[test]
    fn a_manifest_that_says_another_generation_than_the_bundle_is_a_problem() {
        let root = tempfile::tempdir().expect("a tree");
        tree(root.path(), &text(2, 1));
        let dir = tempfile::tempdir().expect("a release");
        let updater = std::fs::read(root.path().join("deploy/update.sh")).unwrap();
        let mut manifest = release(
            dir.path(),
            root.path(),
            &updater,
            Some(manifest::sha256_of_bytes(&updater)),
        );
        manifest.deploy_files = Some(DeployFiles {
            generation: 3,
            minimum: 1,
        });
        let found = problems(dir.path(), &manifest).unwrap().join("\n");
        assert!(
            found.contains("says generation 2 and minimum 1, and the manifest says 3 and 1"),
            "{found}"
        );
    }

    #[test]
    fn a_manifest_that_says_nothing_of_deployment_files_is_not_asked_for_any() {
        let dir = tempfile::tempdir().expect("a release");
        let mut manifest: Manifest = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "project": "orch", "tag": "v0.1.3", "version": "0.1.3",
            "commit": "a".repeat(40), "link_protocol": {"current": 1, "minimum": 1},
            "connector_minimum": "0.1.0", "assets": [], "images": [], "harness_builds": []
        }))
        .expect("a manifest");
        assert!(manifest.deploy_files.is_none());
        assert!(problems(dir.path(), &manifest).unwrap().is_empty());
        manifest.deploy_files = Some(DeployFiles {
            generation: 1,
            minimum: 1,
        });
        assert!(
            problems(dir.path(), &manifest)
                .unwrap()
                .join("\n")
                .contains("does not list orch-deploy-v0.1.3.tar.gz")
        );
    }

    #[test]
    fn a_main_set_that_names_a_file_the_tree_lacks_is_refused() {
        let dir = tempfile::tempdir().expect("a directory");
        tree(
            dir.path(),
            &text(1, 1).replace("compose.yml nginx.conf", "compose.yml missing.conf"),
        );
        let refused = amended(dir.path(), "v0.1.4").unwrap_err().to_string();
        assert!(refused.contains("missing.conf"), "{refused}");
    }
}
