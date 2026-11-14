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
