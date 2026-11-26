//! The content policy of the web edge: byte for byte what it always was where no human check is chosen, and with
//! exactly the hosts each service's own backend names where one is.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The policy the edge sent before any human check was a choice, as the shipped file wrote it.
const POLICY_BEFORE: &str = "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'nonce-$request_id' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self'";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// What the entrypoint's script writes for `service`: the variables it sets.
fn variables(service: &str) -> BTreeMap<String, String> {
    let output = Command::new("sh")
        .arg(root().join("deploy/frontend-csp.sh"))
        .arg(service)
        .output()
        .expect("sh runs the script");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    text.lines()
        .map(|line| {
            let line = line
                .strip_prefix("set $")
                .unwrap()
                .strip_suffix(';')
                .unwrap();
            let (name, value) = line.split_once(' ').unwrap();
            (
                name.to_owned(),
                value
                    .strip_prefix('"')
                    .unwrap()
                    .strip_suffix('"')
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect()
}

/// The policy the shipped nginx file sends, with its variables given those of `service`.
fn policy(service: &str) -> String {
    let nginx = std::fs::read_to_string(root().join("deploy/nginx.conf")).unwrap();
    let line = nginx
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("add_header Content-Security-Policy")
        })
        .expect("the edge sends a content policy");
    let start = line.find('"').unwrap() + 1;
    let end = line.rfind('"').unwrap();
    let mut text = line[start..end].to_owned();
    for (name, value) in variables(service) {
        text = text.replace(&format!("${name}"), &value);
    }
    assert!(
        !text.replace("$request_id", "").contains('$'),
        "a variable is left unset: {text}"
    );
    text
}

/// The sources one directive names, or none where the policy has no such directive.
fn sources(policy: &str, directive: &str) -> Option<Vec<String>> {
    policy.split(';').map(str::trim).find_map(|part| {
        let mut words = part.split_whitespace();
        (words.next() == Some(directive)).then(|| words.map(str::to_owned).collect())
    })
}

#[test]
fn with_no_service_chosen_the_policy_is_byte_for_byte_what_it_always_was() {
    assert_eq!(policy(""), POLICY_BEFORE);
}

#[test]
fn each_service_adds_exactly_the_hosts_its_backend_names_and_nothing_else() {
    for (service, kind, alternate) in [
        ("hcaptcha", "hcaptcha", false),
        ("recaptcha", "recaptcha", false),
        ("recaptcha-net", "recaptcha", true),
    ] {
        let hosts = orch_human_check::edge_hosts_of(kind, alternate).unwrap();
        let rendered = policy(service);
        for (directive, base, added) in [
            (
                "script-src",
                vec!["'self'", "'nonce-$request_id'", "'wasm-unsafe-eval'"],
                hosts.script,
            ),
            ("style-src", vec!["'self'", "'unsafe-inline'"], hosts.style),
            ("connect-src", vec!["'self'"], hosts.connect),
        ] {
            let mut expected: Vec<String> = base.into_iter().map(str::to_owned).collect();
            expected.extend(added.iter().map(|host| (*host).to_owned()));
            assert_eq!(
                sources(&rendered, directive),
                Some(expected),
                "{service} {directive}"
            );
        }
        // Frames fell back to 'self' before; a directive of their own keeps it.
        let mut frames = vec!["'self'".to_owned()];
        frames.extend(hosts.frame.iter().map(|host| (*host).to_owned()));
        assert_eq!(
            sources(&rendered, "frame-src"),
            Some(frames),
            "{service} frame-src"
        );
        // Every other directive is as it was.
        for directive in [
            "default-src",
            "img-src",
            "media-src",
            "font-src",
            "worker-src",
            "manifest-src",
            "base-uri",
            "object-src",
            "frame-ancestors",
            "form-action",
        ] {
            assert_eq!(
                sources(&rendered, directive),
                sources(POLICY_BEFORE, directive),
                "{service} {directive}"
            );
        }
    }
}

#[test]
fn a_service_the_edge_does_not_know_stops_the_entrypoint() {
    let output = Command::new("sh")
        .arg(root().join("deploy/frontend-csp.sh"))
        .arg("turnstile")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ORCH_FRONTEND_HUMAN_CHECK"));
}

#[test]
fn the_whole_policy_for_a_kept_older_file_is_what_the_entrypoint_makes_of_the_variable() {
    for (service, kind, alternate) in [
        ("hcaptcha", "hcaptcha", false),
        ("recaptcha", "recaptcha", false),
        ("recaptcha-net", "recaptcha", true),
    ] {
        assert_eq!(
            orch_human_check::policy_of(kind, alternate).unwrap(),
            policy(service),
            "{service}"
        );
        // And the line that carries it is one the edge's file could hold.
        let line = orch_human_check::nginx_line_of(kind, alternate).unwrap();
        assert!(line.starts_with("add_header Content-Security-Policy \""));
        assert!(line.ends_with("\" always;"));
        // What it sends lets the widget through, and what the edge always sent does not.
        let hosts = orch_human_check::edge_hosts_of(kind, alternate).unwrap();
        assert!(orch_human_check::lacking(&[policy(service)], &hosts).is_empty());
        assert!(!orch_human_check::lacking(&[POLICY_BEFORE.to_owned()], &hosts).is_empty());
    }
    assert!(orch_human_check::policy_of("turnstile", false).is_none());
}

#[test]
fn the_shipped_file_starts_without_the_file_an_older_entrypoint_would_not_write() {
    let nginx = std::fs::read_to_string(root().join("deploy/nginx.conf")).unwrap();
    let before = nginx.find("include /tmp/frontend-csp").unwrap();
    // The variables are given their empty values before the include; the include is a mask, which may match nothing.
    for name in ["csp_script", "csp_style", "csp_connect", "csp_frame"] {
        let at = nginx
            .find(&format!("set ${name} \"\";"))
            .unwrap_or_else(|| panic!("{name} has no empty default"));
        assert!(at < before, "{name} is set after the include");
    }
    assert!(nginx.contains("include /tmp/frontend-csp*.conf;"));
}

#[test]
fn the_operator_guide_carries_the_lines_for_a_kept_file_exactly_as_they_are_made() {
    for page in [
        "docs/operator/human-check.md",
        "docs/operator/zh-CN/human-check.md",
    ] {
        let text = std::fs::read_to_string(root().join(page)).unwrap();
        for (kind, alternate) in [
            ("hcaptcha", false),
            ("recaptcha", false),
            ("recaptcha", true),
        ] {
            let line = orch_human_check::nginx_line_of(kind, alternate).unwrap();
            assert!(
                text.contains(&line),
                "{page} does not carry the line for {kind} (alternate {alternate}): {line}"
            );
        }
    }
}
