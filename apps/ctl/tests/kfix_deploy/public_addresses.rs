//! Readiness uses each committed address and preserves an explicit operator route.

use super::*;

fn readiness(override_url: Option<&str>) {
    let root = tempfile::tempdir().unwrap();
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/update.sh"
    ))
    .unwrap();
    let functions = source.rsplit_once("main ").unwrap().0;
    std::fs::write(root.path().join("functions.sh"), functions).unwrap();
    std::fs::create_dir_all(root.path().join("runtime/cli/current")).unwrap();
    std::fs::create_dir(root.path().join("config")).unwrap();
    std::fs::write(
        root.path().join("config/orch.toml"),
        r#"[system]
public_url = "https://file-seed.invalid"
"#,
    )
    .unwrap();
    let cli = root.path().join("runtime/cli/current/orchctl");
    std::fs::write(
        &cli,
        r#"#!/bin/bash
set -eu
test "$1" = doctor
test "$2" = --url
if [ "$3" = https://before.example.test ]; then
  : > advanced
  exit 1
fi
test "$3" = "$EXPECTED_EDGE"
printf 'ok    edge-api         release 1.2.3 (build 7)\n'
"#,
    )
    .unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    let script = root.path().join("case.sh");
    std::fs::write(&script, r#"#!/bin/bash
set -euo pipefail
source ./functions.sh
READY_TIMEOUT=2
env_get() { :; }
sleep() { :; }
compose() {
  case "$*" in
    'config --services') printf 'api\nfrontend\n';;
    *'public-address get'*)
      printf 'get\n' >> get-calls
      if [ -e advanced ]; then printf 'https://after.example.test\n'; else printf 'https://before.example.test\n'; fi;;
    *'doctor --expect-version'*) printf 'ok    api              release 1.2.3\n';;
    *) printf 'unexpected compose call\n' >&2; return 1;;
  esac
}
wait_ready v1.2.3
"#).unwrap();
    let expected = override_url.unwrap_or("https://after.example.test");
    let output = Command::new("bash")
        .arg(&script)
        .current_dir(root.path())
        .env("EXPECTED_EDGE", expected)
        .env("ORCH_READY_URL", override_url.unwrap_or(""))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = std::fs::read_to_string(root.path().join("get-calls")).unwrap_or_default();
    if override_url.is_some() {
        assert!(calls.is_empty(), "the operator selected a route explicitly");
    } else {
        assert_eq!(
            calls.lines().count(),
            2,
            "a new readiness attempt reads the current committed address"
        );
    }
}

#[test]
fn updater_readiness_reads_the_effective_address_each_attempt_instead_of_the_file_seed() {
    readiness(None);
    readiness(Some("https://operator-route.example.test"));
}

#[test]
fn first_start_prints_the_committed_browser_address_instead_of_an_obsolete_file_seed() {
    let root = tempfile::tempdir().unwrap();
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/update.sh"
    ))
    .unwrap();
    std::fs::write(
        root.path().join("functions.sh"),
        source.rsplit_once("main ").unwrap().0,
    )
    .unwrap();
    std::fs::create_dir(root.path().join("config")).unwrap();
    for file in ["compose.yml", "nginx.conf"] {
        std::fs::write(root.path().join(file), "").unwrap();
    }
    std::fs::write(
        root.path().join("config/orch.toml"),
        "[system]\npublic_url = \"https://old-file.example.test\"\n",
    )
    .unwrap();
    std::fs::write(root.path().join("case.sh"), r#"#!/bin/bash
set -euo pipefail
source ./functions.sh
RESTORE_DATABASE=
load_facts_for_first_start() { :; }
check_password() { :; }
prepare_dirs() { :; }
fetch_release() { TAG=v1.2.3; }
unpack() { UNPACKED="$1-release"; }
link_set() { :; }
ensure_images() { :; }
jset() { :; }
ensure_secrets() { :; }
check_installation() { :; }
database_start() { :; }
take_dump() { :; }
app_start() { :; }
wait_ready() { :; }
state_save() { :; }
journal_clear() { :; }
publish_storage_metadata() { :; }
authority_fingerprint() { printf 'synthetic-public-fingerprint\n'; }
env_get() { :; }
compose() {
  case "$*" in *'public-address get'*) printf 'https://committed.example.test\n';; *) return 1;; esac
}
cmd_first_start v1.2.3
"#).unwrap();
    let output = Command::new("bash")
        .arg(root.path().join("case.sh"))
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let message = String::from_utf8(output.stdout).unwrap();
    assert!(
        message.contains("Open https://committed.example.test"),
        "{message}"
    );
    assert!(!message.contains("old-file.example.test"));
}
