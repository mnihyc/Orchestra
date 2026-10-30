//! The licences of what is compiled into a release's programs.
//! Release-time notices use package files and offline SPDX fallback texts.

use crate::release::ReleaseError;
use crate::release::container::Builder;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

/// The programs a release ships; their dependencies are what is listed.
const SHIPPED_PROGRAMS: [&str; 6] = [
    "orch-api",
    "orch-gateway",
    "orch-worker",
    "orch-node",
    "orch-connector",
    "orchctl",
];
/// The beginnings of the names of files that hold a licence or a notice, in
/// lowercase.
const LICENCE_FILE_PREFIXES: [&str; 5] = ["license", "licence", "copying", "notice", "unlicense"];
/// Where the container sees Cargo's downloads.
const CONTAINER_CARGO_HOME: &str = "/cargo-home/";

/// A third-party crate that is built into the programs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Crate {
    name: String,
    version: String,
    licence: String,
    repository: String,
    source_dir: String,
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn rows(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

/// The third-party crates reachable from the shipped programs for one target.
fn crates_in(metadata: &Value, programs: &[&str]) -> BTreeMap<String, Crate> {
    let packages: BTreeMap<_, _> = rows(&metadata["packages"])
        .iter()
        .map(|p| (text(p, "id"), p))
        .collect();
    let nodes: BTreeMap<_, _> = rows(&metadata["resolve"]["nodes"])
        .iter()
        .map(|node| (text(node, "id"), node))
        .collect();
    let mut queue: VecDeque<_> = packages
        .iter()
        .filter(|(_, p)| p["source"].is_null() && programs.contains(&text(p, "name").as_str()))
        .map(|(id, _)| id.clone())
        .collect();
    let mut found = BTreeMap::new();
    let mut seen = BTreeSet::new();
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(p) = packages.get(&id)
            && !p["source"].is_null()
        {
            found.insert(
                id.clone(),
                Crate {
                    name: text(p, "name"),
                    version: text(p, "version"),
                    licence: text(p, "license"),
                    repository: text(p, "repository"),
                    source_dir: Path::new(&text(p, "manifest_path"))
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                },
            );
        }
        if let Some(node) = nodes.get(&id) {
            for dep in rows(&node["deps"]) {
                if rows(&dep["dep_kinds"])
                    .iter()
                    .any(|kind| kind["kind"] != "dev")
                {
                    queue.push_back(text(dep, "pkg"));
                }
            }
        }
    }
    found
}

fn read(path: &Path) -> Result<Vec<u8>, ReleaseError> {
    std::fs::read(path).map_err(|source| ReleaseError::Io {
        path: path.to_owned(),
        source,
    })
}

fn files(dir: &Path) -> Result<Vec<PathBuf>, ReleaseError> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some(".git" | "node_modules" | "target")
            )
        })
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_type().is_file() => Some(Ok(entry.into_path())),
            Ok(_) => None,
            Err(error) => Some(Err(ReleaseError::Invalid(error.to_string()))),
        })
        .collect()
}

fn usual(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        LICENCE_FILE_PREFIXES.iter().any(|prefix| {
            name.to_string_lossy()
                .to_ascii_lowercase()
                .starts_with(prefix)
        })
    }) || path.components().any(|part| {
        part.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("LICENSES")
    })
}

#[derive(serde::Deserialize)]
struct Policy {
    licenses: LicensePolicy,
}
#[derive(serde::Deserialize)]
struct LicensePolicy {
    allow: BTreeSet<String>,
    #[serde(default)]
    exceptions: Vec<Exception>,
}
#[derive(serde::Deserialize)]
struct Exception {
    #[serde(rename = "crate")]
    package: String,
    allow: BTreeSet<String>,
}

fn policy(root: &Path) -> Result<Policy, ReleaseError> {
    toml::from_str(&String::from_utf8_lossy(&read(&root.join("deny.toml"))?))
        .map_err(|error| ReleaseError::Invalid(error.to_string()))
}

fn fault(package: &Crate, message: impl std::fmt::Display) -> ReleaseError {
    ReleaseError::Invalid(format!("{} {}: {message}", package.name, package.version))
}

fn append(
    output: &mut String,
    package: &Crate,
    dir: &Path,
    policy: &Policy,
) -> Result<(), ReleaseError> {
    let paths = files(dir)?;

    let mut texts = Vec::new();
    for path in paths.iter().filter(|path| usual(path)) {
        texts.push((
            path.strip_prefix(dir).unwrap_or(path),
            String::from_utf8_lossy(&read(path)?).into_owned(),
        ));
    }
    let inferred = texts.iter().find_map(|(_, file)| {
        let title = file.lines().next()?.trim_start_matches("The ");
        spdx::license_id(title.split_whitespace().next()?)
            .or_else(|| spdx::imprecise_license_id(title).map(|(id, _)| id))
            .map(|id| id.name)
    });
    let declared = (!package.licence.is_empty())
        .then_some(package.licence.as_str())
        .or(inferred)
        .ok_or_else(|| fault(package, "has no licence declaration"))?;
    let expression = spdx::Expression::parse_mode(declared, spdx::ParseMode::LAX)
        .map_err(|error| fault(package, error))?;
    let mut allowed = policy.licenses.allow.clone();
    for exception in &policy.licenses.exceptions {
        if exception.package == format!("{}@{}", package.name, package.version) {
            allowed.extend(exception.allow.iter().cloned());
        }
    }
    // Preserve the web viewer's existing exact noVNC approval.
    if package.name == "@novnc/novnc" && package.version == "1.7.0" {
        allowed.insert("MPL-2.0".into());
    }
    let permits = |req: &spdx::LicenseReq| allowed.contains(&req.to_string());
    if !expression.evaluate(permits) {
        return Err(fault(
            package,
            format!("licence {declared} is outside deny.toml"),
        ));
    }
    output.push_str(&format!(
        "\n=== {} {} ({declared}) ===\n{}\n",
        package.name, package.version, package.repository
    ));

    if texts.is_empty() {
        for id in expression
            .requirements()
            .filter(|entry| permits(&entry.req))
            .filter_map(|entry| entry.req.license.id())
            .collect::<BTreeSet<_>>()
        {
            output.push_str(id.text());
            output.push('\n');
        }
    }
    for (path, file) in &texts {
        output.push_str(&format!("--- {} ---\n{file}\n", path.display()));
    }
    let mut names = BTreeSet::new();
    for path in &paths {
        let font = path
            .extension()
            .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("ttf"));
        if !font && !texts.is_empty() {
            continue;
        }
        let bytes = read(path)?;
        if font {
            names.extend(super::font_names::read(&bytes).unwrap_or_default());
        } else if let Ok(source) = std::str::from_utf8(&bytes) {
            names.extend(
                source
                    .lines()
                    .filter(|line| {
                        line.to_ascii_lowercase().contains("copyright") || line.contains('©')
                    })
                    .map(str::to_owned),
            );
        }
    }
    if names.iter().any(|name| name.contains("Open Font License"))
        && !texts
            .iter()
            .any(|(_, file)| file.contains("SIL OPEN FONT LICENSE"))
        && let Some(id) = spdx::license_id("OFL-1.1")
    {
        output.push_str(id.text());
    }
    for name in names {
        output.push_str(&name);
        output.push('\n');
    }
    Ok(())
}

/// Generate Rust notices offline from the source cache used by this artifact.
pub fn collect(root: &Path, builder: &Builder, triples: &[&str]) -> Result<String, ReleaseError> {
    let mut all = BTreeMap::new();
    for triple in triples {
        // The build fetched only what it compiled; the workspace's metadata reads every package that resolves for
        // the platform, so the others are fetched first, at the versions the lock file names.
        builder.cargo(&["fetch", "--locked", "--target", triple], &[])?;
        let printed = builder.cargo_output(&[
            "metadata",
            "--locked",
            "--offline",
            "--format-version",
            "1",
            "--filter-platform",
            triple,
        ])?;
        let metadata: Value =
            serde_json::from_str(&printed).map_err(|source| ReleaseError::Json {
                path: "cargo metadata".into(),
                source,
            })?;
        let programs: &[&str] = if triple.contains("linux") {
            &SHIPPED_PROGRAMS
        } else {
            &["orch-connector"]
        };
        all.extend(crates_in(&metadata, programs));
    }
    let policy = policy(root)?;
    let mut output = String::from(
        "THIRD PARTY NOTICES\nOriginal package licence and notice files; standard SPDX texts where a package carries none.\n",
    );
    for package in all.into_values() {
        let dir = package
            .source_dir
            .strip_prefix(CONTAINER_CARGO_HOME)
            .map_or_else(
                || PathBuf::from(&package.source_dir),
                |rest| builder.cargo_home().join(rest),
            );
        append(&mut output, &package, &dir, &policy)?;
        output.push_str(&format!(
            "\nSource package: https://crates.io/api/v1/crates/{}/{}/download\n",
            package.name, package.version
        ));
    }
    Ok(output)
}

/// Generate notices for exactly the npm package locations the web build emits.
pub fn collect_frontend(root: &Path) -> Result<String, ReleaseError> {
    let lock_path = root.join("frontend/package-lock.json");
    let lock: Value =
        serde_json::from_slice(&read(&lock_path)?).map_err(|source| ReleaseError::Json {
            path: lock_path,
            source,
        })?;
    let inventory = root.join("frontend/dist/.vite/notice-packages.json");
    let paths: Vec<String> =
        serde_json::from_slice(&read(&inventory)?).map_err(|source| ReleaseError::Json {
            path: inventory,
            source,
        })?;
    let policy = policy(root)?;
    let mut output = String::new();
    for path in paths {
        let installed = root.join("frontend").join(&path);
        let row = lock
            .get("packages")
            .and_then(|packages| packages.get(&path))
            .ok_or_else(|| {
                ReleaseError::Invalid(format!("emitted npm package {path} is not locked"))
            })?;
        let package = Crate {
            name: path
                .rsplit_once("node_modules/")
                .map_or(path.as_str(), |(_, name)| name)
                .into(),
            version: text(row, "version"),
            licence: text(row, "license"),
            repository: text(row, "resolved"),
            source_dir: String::new(),
        };
        append(&mut output, &package, &installed, &policy)?;
    }
    for path in files(&root.join("licenses/novnc-1.7.0"))?
        .into_iter()
        .filter(|path| usual(path) || path.file_name().is_some_and(|name| name == "AUTHORS"))
    {
        output.push_str(&format!(
            "\n--- vendored noVNC: {} ---\n{}\n",
            path.file_name().unwrap_or_default().to_string_lossy(),
            String::from_utf8_lossy(&read(&path)?)
        ));
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shipped_dependencies_keep_versions_and_licences_without_test_or_lock_only_crates() {
        let metadata = json!({"packages":[
            {"id":"app","name":"orch-api","source":null},
            {"id":"serde","name":"serde","version":"1.0.0","source":"registry+x","license":"MIT"},
            {"id":"jsonschema","name":"jsonschema","source":"registry+x"},
            {"id":"borrow","name":"borrow-or-share","version":"0.2.4","source":"registry+x","license":"MIT-0"},
            {"id":"proptest","name":"proptest","source":"registry+x"},
            {"id":"termwiz","name":"termwiz","source":"registry+x"},
            {"id":"terminfo","name":"terminfo","source":"registry+x"}
        ],"resolve":{"nodes":[
            {"id":"app","deps":[{"pkg":"serde","dep_kinds":[{"kind":null}]},{"pkg":"jsonschema","dep_kinds":[{"kind":null}]},{"pkg":"proptest","dep_kinds":[{"kind":"dev"}]}]},
            {"id":"jsonschema","deps":[{"pkg":"borrow","dep_kinds":[{"kind":null}]}]},
            {"id":"termwiz","deps":[{"pkg":"terminfo","dep_kinds":[{"kind":null}]}]}
        ]}});
        let found = crates_in(&metadata, &SHIPPED_PROGRAMS);
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            ["borrow", "jsonschema", "serde"]
        );
        assert_eq!(found["serde"].name, "serde");
        assert_eq!(found["borrow"].licence, "MIT-0");
        assert_eq!(found["borrow"].version, "0.2.4");
        assert!(
            !found
                .values()
                .any(|row| matches!(row.name.as_str(), "termwiz" | "terminfo" | "proptest"))
        );
    }

    #[test]
    fn crate_and_npm_files_are_collected_and_missing_files_use_standard_spdx_text() {
        let directory = tempfile::tempdir().expect("fixture");
        let policy: Policy = toml::from_str("[licenses]\nallow = ['MIT']").expect("policy");
        for name in ["fixture-crate", "fixture-npm"] {
            let package = Crate {
                name: name.into(),
                version: "1.0.0".into(),
                licence: "MIT".into(),
                repository: "https://example.org/source".into(),
                source_dir: String::new(),
            };
            let dir = directory.path().join(name);
            std::fs::create_dir_all(dir.join("LICENSES")).expect("directory");
            std::fs::write(dir.join("LICENSES/MIT.txt"), "original MIT terms").expect("terms");
            std::fs::write(dir.join("NOTICE"), "original attribution").expect("notice");
            let mut output = String::new();
            append(&mut output, &package, &dir, &policy).expect("notices");
            assert!(output.contains(name));
            assert!(output.contains("LICENSES/MIT.txt"));
            assert!(output.contains("original MIT terms"));
            assert!(output.contains("original attribution"));
        }
        let empty = directory.path().join("without-files");
        std::fs::create_dir(&empty).expect("directory");
        std::fs::write(empty.join("lib.rs"), "// Copyright 2026 Example authors")
            .expect("source header");
        let mut package = Crate {
            name: "without-files".into(),
            version: "1.0.0".into(),
            licence: "MIT".into(),
            repository: String::new(),
            source_dir: String::new(),
        };
        let mut output = String::new();
        append(&mut output, &package, &empty, &policy).expect("SPDX fallback");
        assert!(output.contains("Permission is hereby granted"));
        assert!(output.contains("Copyright 2026 Example authors"));
        package.licence.clear();
        assert!(
            append(&mut String::new(), &package, &empty, &policy)
                .expect_err("undeclared")
                .to_string()
                .contains("without-files 1.0.0: has no licence declaration")
        );
        package.licence = "MIT AND GPL-3.0-only".into();
        assert!(
            append(&mut String::new(), &package, &empty, &policy)
                .expect_err("outside policy")
                .to_string()
                .contains("outside deny.toml")
        );
    }
}
