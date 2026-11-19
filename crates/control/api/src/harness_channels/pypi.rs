//! The reader of programs published to PyPI, in a wheel that carries one executable for each processor.
//!
//! The index's JSON API lists a project's releases with the files of each and the SHA-256 the index keeps for them.
//! A release is read when it is one of the newest and not a pre-release the channel does not keep; a file is kept
//! when its name is the manifest's pattern for a processor and it was not yanked.

use orch_agent_programs::channels::Pypi;
use orch_agent_programs::versions::{pep440_of, semver_of};
use orch_api_contract::admin::HarnessBuild;
use orch_types::Json;

use super::{Ask, build, is_sha256, mirrored, wildcard};

/// Whether a version is a pre-release: PEP 440 writes one with a letter after the numbers (`1.0.0rc1`, `2.0a3`,
/// `0.2.1.dev4`), and semver with a dash.
fn is_pre(version: &str) -> bool {
    version.contains('-') || version.chars().any(|each| each.is_ascii_alphabetic())
}

/// The builds of the files of one release: `files` is the index's list of them.
fn release_builds(
    channel: &Pypi,
    kind: &str,
    version: &str,
    files: &Json,
    mirror: Option<&str>,
) -> Vec<HarnessBuild> {
    channel
        .files
        .iter()
        .filter_map(|platform| {
            let file = files.as_array()?.iter().find(|file| {
                file.get("yanked").and_then(Json::as_bool) != Some(true)
                    && file
                        .get("filename")
                        .and_then(Json::as_str)
                        .is_some_and(|name| wildcard(&platform.name, name))
            })?;
            let url = file.get("url").and_then(Json::as_str)?;
            let sha256 = file
                .pointer("/digests/sha256")
                .and_then(Json::as_str)
                .filter(|sha256| is_sha256(sha256))?;
            Some(build(
                kind,
                &semver_of(version),
                platform.arch,
                mirrored(url, mirror),
                sha256,
            ))
        })
        .collect()
}

/// The builds of the newest releases in the project's document (`releases` of the index's answer).
#[must_use]
fn builds(channel: &Pypi, kind: &str, document: &Json, mirror: Option<&str>) -> Vec<HarnessBuild> {
    let Some(releases) = document.get("releases").and_then(Json::as_object) else {
        return Vec::new();
    };
    let mut versions: Vec<&String> = releases
        .keys()
        .filter(|version| channel.prereleases || !is_pre(version))
        .collect();
    versions.sort_by(|first, second| {
        crate::hosting::version_order(&semver_of(second), &semver_of(first))
    });
    versions.truncate(usize::try_from(channel.listed).unwrap_or(usize::MAX));
    versions
        .into_iter()
        .flat_map(|version| release_builds(channel, kind, version, &releases[version], mirror))
        .collect()
}

/// Reads the project's newest releases.
pub(super) async fn read(ask: Ask<'_>, channel: &Pypi) -> (Vec<HarnessBuild>, Vec<String>) {
    let url = format!(
        "{}/pypi/{}/json",
        channel.index.trim_end_matches('/'),
        channel.project
    );
    match ask.json(&url).await {
        Ok(document) => (builds(channel, ask.kind, &document, ask.mirror), Vec::new()),
        Err(error) => (Vec::new(), vec![error]),
    }
}

/// The address of the release of `version` (as Orch spells it or as the index does), at the index's own name for it.
fn exact_url(channel: &Pypi, version: &str) -> String {
    format!(
        "{}/pypi/{}/{}/json",
        channel.index.trim_end_matches('/'),
        channel.project,
        pep440_of(version)
    )
}

/// Reads the release of exactly `version`.
pub(super) async fn read_exact(
    ask: Ask<'_>,
    channel: &Pypi,
    version: &str,
) -> Result<Vec<HarnessBuild>, String> {
    let url = exact_url(channel, version);
    let document = ask.json(&url).await?;
    let files = document.get("urls").cloned().unwrap_or(Json::Null);
    Ok(release_builds(
        channel, ask.kind, version, &files, ask.mirror,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_programs::channels::{Arch, PlatformName};
    use serde_json::json;

    const SHA: &str = "3ec55053109608d5038f7e50750f7d25f768dfd740fef071aa710e2559b63aac";

    fn channel() -> Pypi {
        Pypi {
            id: "pypi".to_owned(),
            project: "example-runtime".to_owned(),
            index: "https://pypi.org".to_owned(),
            files: vec![
                PlatformName {
                    arch: Arch::X86_64,
                    name: "*manylinux*_x86_64.whl".to_owned(),
                },
                PlatformName {
                    arch: Arch::Aarch64,
                    name: "*manylinux*_aarch64.whl".to_owned(),
                },
            ],
            listed: 2,
            prereleases: false,
        }
    }

    fn file(name: &str, yanked: bool, sha: &str) -> Json {
        json!({ "filename": name, "url": format!("https://files.example.test/{name}"),
                "digests": { "sha256": sha }, "yanked": yanked })
    }

    #[test]
    fn a_release_gives_a_wheel_for_each_processor_with_the_checksum_the_index_keeps() {
        let document = json!({ "releases": {
            "0.2.5": [ file("p-0.2.5-py3-none-manylinux_2_28_x86_64.whl", false, SHA),
                       file("p-0.2.5-py3-none-manylinux_2_28_aarch64.whl", false, SHA),
                       file("p-0.2.5-py3-none-macosx_11_0_arm64.whl", false, SHA) ],
            "0.2.5rc1": [ file("p-0.2.5rc1-py3-none-manylinux_2_28_x86_64.whl", false, SHA) ],
            "0.2.4": [ file("p-0.2.4-py3-none-manylinux_2_28_x86_64.whl", true, SHA),
                       file("p-0.2.4-py3-none-manylinux_2_28_aarch64.whl", false, "not a checksum") ],
            "0.2.3": [ file("p-0.2.3-py3-none-manylinux_2_28_x86_64.whl", false, SHA) ]
        } });
        let found = builds(&channel(), "dsh", &document, None);
        let listed: Vec<_> = found
            .iter()
            .map(|build| (build.version.as_str(), build.arch))
            .collect();
        assert_eq!(
            listed,
            [
                ("0.2.5", orch_api_contract::admin::HarnessArch::X86_64),
                ("0.2.5", orch_api_contract::admin::HarnessArch::Aarch64),
            ],
            "the two newest releases that are not pre-releases are read, and a yanked file and a file without a checksum are not kept"
        );
        assert_eq!(found[0].sha256, SHA);
        assert_eq!(
            found[0].url,
            "https://files.example.test/p-0.2.5-py3-none-manylinux_2_28_x86_64.whl"
        );
    }

    #[test]
    fn a_release_the_index_names_in_pep_440_is_kept_under_the_name_a_profile_pins() {
        let document = json!({ "releases": {
            "0.2.1rc2": [ file("p-0.2.1rc2-py3-none-manylinux_2_28_x86_64.whl", false, SHA) ],
            "0.2.1rc1": [ file("p-0.2.1rc1-py3-none-manylinux_2_28_x86_64.whl", false, SHA) ]
        } });
        let mut channel = channel();
        channel.prereleases = true;
        let found = builds(&channel, "dsh", &document, None);
        let versions: Vec<_> = found.iter().map(|build| build.version.as_str()).collect();
        assert_eq!(
            versions,
            ["0.2.1-rc.2", "0.2.1-rc.1"],
            "newest first, spelled as the program prints them"
        );
        assert_eq!(
            exact_url(&channel, "0.2.1-rc.2"),
            "https://pypi.org/pypi/example-runtime/0.2.1rc2/json",
            "the index is asked by its own name for the release"
        );
        assert_eq!(
            exact_url(&channel, "0.2.1rc2"),
            exact_url(&channel, "0.2.1-rc.2")
        );
        let files = json!([file(
            "p-0.2.1rc2-py3-none-manylinux_2_28_x86_64.whl",
            false,
            SHA
        )]);
        let exact = release_builds(&channel, "dsh", "0.2.1rc2", &files, None);
        assert_eq!(exact[0].version, "0.2.1-rc.2");
    }

    #[test]
    fn an_exact_release_reads_the_files_the_index_lists_for_it() {
        let files = json!([file(
            "p-1.0.0-py3-none-manylinux_2_17_aarch64.whl",
            false,
            SHA
        )]);
        let found = release_builds(&channel(), "dsh", "1.0.0", &files, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].version, "1.0.0");
    }
}
