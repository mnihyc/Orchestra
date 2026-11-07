//! A machine program's identity is its reported version, including a bounded unknown result.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt as _;
use tokio::process::Command;

/// A machine refresh and a version check at use share the same interactive bound.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// A version is one short line; a different program must not buffer arbitrary output.
const OUTPUT_LIMIT: u64 = 4096;

/// Rechecks the confirmed file directly, with the connector's PATH and no environment values.
pub(super) async fn reported(program: &Path) -> String {
    output(program)
        .await
        .as_deref()
        .and_then(parse)
        .unwrap_or_else(|| "unknown".to_owned())
}

async fn output(program: &Path) -> Option<String> {
    let mut command = Command::new(program);
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    let mut child = command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let text = tokio::time::timeout(PROBE_TIMEOUT, async {
        let mut bytes = Vec::new();
        stdout
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > OUTPUT_LIMIT as usize {
            return None;
        }
        child.wait().await.ok()?.success().then_some(())?;
        String::from_utf8(bytes).ok()
    })
    .await
    .ok()
    .flatten();
    if !matches!(child.try_wait(), Ok(Some(_))) {
        let _ = child.kill().await;
    }
    text
}

fn parse(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|word| {
        semver::Version::parse(word.strip_prefix('v').unwrap_or(word))
            .ok()
            .map(|version| version.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn the_first_semantic_word_drops_exactly_one_leading_v() {
        assert_eq!(parse("tool vv1.2.3 v1.2.4 1.2.5"), Some("1.2.4".into()));
        assert_eq!(parse("tool v1.2.3-rc.1"), Some("1.2.3-rc.1".into()));
        assert_eq!(parse("tool unknown vv1.2.3"), None);
    }
}
