//! Shipped files and documented examples use the same strict readers as the programs.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]

use orch_config::{ConnectorConfig, ServerConfig, load_text};
use std::path::{Path, PathBuf};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
}

fn files(directory: &Path, extension: &str, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        // Runtime state and operator trust files are not deployment-bundle inputs.
        if entry.file_name() == "secrets"
            || entry.path() == root().join("deploy/runtime")
            || entry.path() == root().join("deploy/config/ca-certificates")
        {
            continue;
        }
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            files(&entry.path(), extension, found);
        } else if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|value| value == extension)
        {
            found.push(entry.path());
        }
    }
}

fn accepts(text: &str, origin: &str) {
    let table: toml::Table = toml::from_str(text).unwrap();
    let server = toml::Table::try_from(ServerConfig::default()).unwrap();
    let connector = toml::Table::try_from(ConnectorConfig::default()).unwrap();
    // A machine-only section selects that reader; common sections also fit the server reader.
    if table
        .keys()
        .any(|key| connector.contains_key(key) && !server.contains_key(key))
    {
        load_text::<ConnectorConfig>(text, origin, |_| None).unwrap();
    } else {
        load_text::<ServerConfig>(text, origin, |_| None).unwrap();
    }
}

#[test]
fn every_shipped_configuration_is_accepted_before_a_package_is_built() {
    let text = std::fs::read_to_string(root().join("deploy/config/orch.toml")).unwrap();
    load_text::<ServerConfig>(&text, "deploy/config/orch.toml", |_| None).unwrap();
    let mut configurations = Vec::new();
    files(&root().join("deploy"), "toml", &mut configurations);
    assert!(!configurations.is_empty());
    for path in configurations {
        accepts(
            &std::fs::read_to_string(&path).unwrap(),
            path.to_str().unwrap(),
        );
    }
}

#[test]
fn documented_toml_examples_are_accepted_by_the_program_that_reads_them() {
    let mut pages = Vec::new();
    files(&root().join("docs"), "md", &mut pages);
    let mut checked = 0;
    for page in pages {
        let text = std::fs::read_to_string(&page).unwrap();
        for (index, block) in text.split("```toml\n").skip(1).enumerate() {
            let (example, _) = block.split_once("```").expect("a closed TOML example");
            accepts(
                example,
                &format!("{}: TOML example {}", page.display(), index + 1),
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "the documented configuration examples must be checked"
    );
}
