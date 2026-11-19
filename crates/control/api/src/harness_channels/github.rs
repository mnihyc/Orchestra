//! The reader of programs published as GitHub releases.
//!
//! A release is one of the repository's newest; its tag, without the manifest's prefix, is the version. Each of the
//! manifest's assets is found by name in the release, and kept with the SHA-256 GitHub keeps on it (`digest`) or
//! with the one a checksum file of the release gives; an asset without either is left out.

use std::collections::BTreeMap;

use orch_agent_programs::channels::{GithubChecksum, GithubRelease};
use orch_api_contract::admin::HarnessBuild;
use orch_types::Json;

use super::{Ask, build, fill, is_sha256, mirrored, wildcard};

/// The checksums that checksum files of a release gave, by the release's tag and the asset's name.
type Sums = BTreeMap<(String, String), String>;

/// A published release's version: its tag without the prefix, and none for a draft, a pre-release the channel does
/// not keep, or a tag without the prefix.
fn version_of<'a>(channel: &GithubRelease, release: &'a Json) -> Option<(&'a str, &'a str)> {
    let tag = release.get("tag_name").and_then(Json::as_str)?;
    let version = tag.strip_prefix(channel.tag_prefix.as_str())?;
    let draft = release.get("draft").and_then(Json::as_bool) != Some(false);
    let pre = release.get("prerelease").and_then(Json::as_bool) != Some(false);
    if draft || (pre && !channel.prereleases) || (version.contains('-') && !channel.prereleases) {
        return None;
    }
    Some((tag, version))
}

fn assets(release: &Json) -> impl Iterator<Item = &Json> {
    release
        .get("assets")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
}

/// The checksum files of the releases that the channel's assets need, as `(tag, asset, url of its checksum file)`.
fn checksum_files(channel: &GithubRelease, releases: &Json) -> Vec<(String, String, String)> {
    let GithubChecksum::Asset { name } = &channel.checksum else {
        return Vec::new();
    };
    let mut wanted = Vec::new();
    for release in releases.as_array().into_iter().flatten() {
        let Some((tag, version)) = version_of(channel, release) else {
            continue;
        };
        for platform in &channel.assets {
            let pattern = fill(&platform.name, &[("version", version), ("tag", tag)]);
            let Some(asset_name) = assets(release)
                .filter_map(|asset| asset.get("name").and_then(Json::as_str))
                .find(|name| wildcard(&pattern, name))
            else {
                continue;
            };
            let sums = fill(
                name,
                &[("file", asset_name), ("version", version), ("tag", tag)],
            );
            let url = assets(release)
                .find(|asset| asset.get("name").and_then(Json::as_str) == Some(sums.as_str()))
                .and_then(|asset| asset.get("browser_download_url").and_then(Json::as_str));
            if let Some(url) = url {
                wanted.push((tag.to_owned(), asset_name.to_owned(), url.to_owned()));
            }
        }
    }
    wanted
}

/// The SHA-256 a checksum file gives for `file`: a line `<hex>  <name>` for it, or the hex alone.
fn sum_in(text: &str, file: &str) -> Option<String> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.next()?;
    let mut single = first.split_whitespace();
    if let (Some(sum), None) = (single.next(), single.next())
        && is_sha256(sum)
    {
        return Some(sum.to_ascii_lowercase());
    }
    std::iter::once(first).chain(lines).find_map(|line| {
        let (sum, name) = line.split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == file && is_sha256(sum))
            .then(|| sum.to_ascii_lowercase())
    })
}

/// The builds of the published releases in `releases` (an array of GitHub's release objects), each with the
/// SHA-256 its vendor gives; an asset without one is left out.
#[must_use]
pub(super) fn builds(
    channel: &GithubRelease,
    kind: &str,
    releases: &Json,
    sums: &Sums,
    mirror: Option<&str>,
) -> Vec<HarnessBuild> {
    let mut found = Vec::new();
    for release in releases.as_array().into_iter().flatten() {
        let Some((tag, version)) = version_of(channel, release) else {
            continue;
        };
        for platform in &channel.assets {
            let pattern = fill(&platform.name, &[("version", version), ("tag", tag)]);
            let Some(asset) = assets(release).find(|asset| {
                asset
                    .get("name")
                    .and_then(Json::as_str)
                    .is_some_and(|name| wildcard(&pattern, name))
            }) else {
                continue;
            };
            let name = asset.get("name").and_then(Json::as_str).unwrap_or_default();
            let sha256 = match &channel.checksum {
                GithubChecksum::Digest => asset
                    .get("digest")
                    .and_then(Json::as_str)
                    .and_then(|digest| digest.strip_prefix("sha256:"))
                    .filter(|sha256| is_sha256(sha256))
                    .map(str::to_owned),
                GithubChecksum::Asset { .. } => {
                    sums.get(&(tag.to_owned(), name.to_owned())).cloned()
                }
            };
            let (Some(url), Some(sha256)) = (
                asset.get("browser_download_url").and_then(Json::as_str),
                sha256,
            ) else {
                continue;
            };
            found.push(build(
                kind,
                version,
                platform.arch,
                mirrored(url, mirror),
                &sha256,
            ));
        }
    }
    found
}

async fn collect_sums(
    ask: Ask<'_>,
    channel: &GithubRelease,
    releases: &Json,
) -> (Sums, Vec<String>) {
    let mut sums = Sums::new();
    let mut failures = Vec::new();
    for (tag, asset, url) in checksum_files(channel, releases) {
        match ask.text(&url).await {
            Ok(text) => {
                if let Some(sum) = sum_in(&text, &asset) {
                    sums.insert((tag, asset), sum);
                }
            }
            Err(error) => failures.push(error),
        }
    }
    (sums, failures)
}

/// Reads the repository's newest releases.
pub(super) async fn read(
    ask: Ask<'_>,
    channel: &GithubRelease,
) -> (Vec<HarnessBuild>, Vec<String>) {
    let url = format!(
        "{}/repos/{}/releases?per_page={}",
        channel.api.trim_end_matches('/'),
        channel.repository,
        channel.listed
    );
    let releases = match ask.json(&url).await {
        Ok(releases) => releases,
        Err(error) => return (Vec::new(), vec![error]),
    };
    let (sums, failures) = collect_sums(ask, channel, &releases).await;
    (
        builds(channel, ask.kind, &releases, &sums, ask.mirror),
        failures,
    )
}

/// Reads the release of exactly `version`.
pub(super) async fn read_exact(
    ask: Ask<'_>,
    channel: &GithubRelease,
    version: &str,
) -> Result<Vec<HarnessBuild>, String> {
    let url = format!(
        "{}/repos/{}/releases/tags/{}{version}",
        channel.api.trim_end_matches('/'),
        channel.repository,
        channel.tag_prefix
    );
    let release = ask.json(&url).await?;
    let releases = serde_json::json!([release]);
    let (sums, _) = collect_sums(ask, channel, &releases).await;
    Ok(builds(channel, ask.kind, &releases, &sums, ask.mirror))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_programs::{Channel, Programs};
    use orch_api_contract::admin::{HarnessArch, HarnessBuildSource, HarnessBuildStatus};
    use serde_json::json;

    const SHA: &str = "3ec55053109608d5038f7e50750f7d25f768dfd740fef071aa710e2559b63aac";

    fn codex() -> GithubRelease {
        let program = Programs::builtin().program("codex").unwrap();
        match &program.builds.channels[0] {
            Channel::GithubRelease(channel) => channel.clone(),
            other => panic!("not a GitHub channel: {other:?}"),
        }
    }

    #[test]
    fn releases_give_their_published_linux_packages_with_the_checksums_github_keeps() {
        let releases = json!([
            { "tag_name": "rust-v0.165.0-alpha.1", "draft": false, "prerelease": true, "assets": [
                { "name": "codex-package-x86_64-unknown-linux-musl.tar.gz", "digest": format!("sha256:{SHA}"),
                  "browser_download_url": "https://github.com/openai/codex/releases/download/rust-v0.165.0-alpha.1/x" } ] },
            { "tag_name": "rust-v0.164.0", "draft": false, "prerelease": false, "assets": [
                { "name": "codex-package-x86_64-unknown-linux-musl.tar.gz", "digest": format!("sha256:{SHA}"),
                  "browser_download_url": "https://github.com/openai/codex/releases/download/rust-v0.164.0/codex-package-x86_64-unknown-linux-musl.tar.gz" },
                { "name": "codex-package-aarch64-unknown-linux-musl.tar.gz", "digest": null,
                  "browser_download_url": "https://github.com/openai/codex/releases/download/rust-v0.164.0/a" },
                { "name": "codex-package-x86_64-unknown-linux-musl-bazel.tar.gz", "digest": format!("sha256:{SHA}"),
                  "browser_download_url": "https://github.com/x" } ] }
        ]);
        let found = builds(&codex(), "codex", &releases, &Sums::new(), None);
        assert_eq!(
            found.len(),
            1,
            "a pre-release, an asset without a checksum and other packages are left out"
        );
        assert_eq!(
            (found[0].version.as_str(), found[0].arch),
            ("0.164.0", HarnessArch::X86_64)
        );
        assert_eq!(found[0].sha256, SHA);
        assert_eq!(found[0].source, HarnessBuildSource::Channel);
        assert_eq!(found[0].status, HarnessBuildStatus::Untested);
        let under = builds(
            &codex(),
            "codex",
            &releases,
            &Sums::new(),
            Some("https://mirror.example.invalid/releases/"),
        );
        assert_eq!(
            under[0].url,
            "https://mirror.example.invalid/releases/github.com/openai/codex/releases/download/rust-v0.164.0/codex-package-x86_64-unknown-linux-musl.tar.gz"
        );
    }

    #[test]
    fn a_checksum_file_of_the_release_stands_for_the_digest_where_the_manifest_says_so() {
        let mut channel = codex();
        channel.checksum = GithubChecksum::Asset {
            name: "SHA256SUMS".to_owned(),
        };
        let releases = json!([
            { "tag_name": "rust-v1.0.0", "draft": false, "prerelease": false, "assets": [
                { "name": "codex-package-x86_64-unknown-linux-musl.tar.gz", "digest": null,
                  "browser_download_url": "https://example.test/pkg" },
                { "name": "SHA256SUMS", "browser_download_url": "https://example.test/sums" } ] }
        ]);
        let wanted = checksum_files(&channel, &releases);
        assert_eq!(
            wanted,
            [(
                "rust-v1.0.0".to_owned(),
                "codex-package-x86_64-unknown-linux-musl.tar.gz".to_owned(),
                "https://example.test/sums".to_owned()
            )]
        );
        let text = format!(
            "{SHA}  codex-package-x86_64-unknown-linux-musl.tar.gz\n{}  other\n",
            "0".repeat(64)
        );
        let sum = sum_in(&text, "codex-package-x86_64-unknown-linux-musl.tar.gz").unwrap();
        let mut sums = Sums::new();
        sums.insert(
            (
                "rust-v1.0.0".to_owned(),
                "codex-package-x86_64-unknown-linux-musl.tar.gz".to_owned(),
            ),
            sum,
        );
        let found = builds(&channel, "codex", &releases, &sums, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sha256, SHA);
        assert_eq!(sum_in(&format!("{SHA}\n"), "any"), Some(SHA.to_owned()));
        assert_eq!(sum_in("nothing here", "any"), None);
    }
}
