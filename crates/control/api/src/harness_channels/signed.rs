//! The reader of programs whose vendor publishes a manifest for each version.
//!
//! Each channel of the manifest is a file that names its newest version as text. A version's manifest lists the
//! files of each platform with a SHA-256; where the vendor also publishes a detached signature of the manifest, the
//! manifest counts only when it verifies. A platform without a well-formed SHA-256 is left out.

use orch_agent_programs::channels::{ManifestSignature, SignedManifest};
use orch_api_contract::admin::HarnessBuild;
use orch_types::Json;

use super::{Ask, build, fill, is_sha256, mirrored};

/// The member of a manifest at `pointer`, a JSON pointer.
fn at<'a>(manifest: &'a Json, pointer: &str) -> Option<&'a Json> {
    manifest.pointer(pointer)
}

/// The builds a version's manifest names, each with the SHA-256 the manifest gives.
#[must_use]
pub(super) fn builds(
    channel: &SignedManifest,
    kind: &str,
    manifest: &Json,
    mirror: Option<&str>,
) -> Vec<HarnessBuild> {
    let Some(version) = at(manifest, &channel.version_pointer).and_then(Json::as_str) else {
        return Vec::new();
    };
    channel
        .platforms
        .iter()
        .filter_map(|platform| {
            let entry = manifest.get("platforms")?.get(&platform.word)?;
            let sha256 = entry
                .get(&channel.checksum_member)
                .and_then(Json::as_str)
                .filter(|sha256| is_sha256(sha256))?;
            let file = entry
                .get(&channel.file_member)
                .and_then(Json::as_str)
                .unwrap_or(channel.file_default.as_str());
            let url = fill(
                &channel.file,
                &[
                    ("base", channel.base.trim_end_matches('/')),
                    ("version", version),
                    ("arch", &platform.word),
                    ("file", file),
                ],
            );
            Some(build(
                kind,
                version,
                platform.arch,
                mirrored(&url, mirror),
                sha256,
            ))
        })
        .collect()
}

/// Whether `signature` (base64, raw) is the vendor's ed25519 signature of `manifest`.
fn verified(spec: &ManifestSignature, manifest: &[u8], signature: &str) -> bool {
    use base64::Engine as _;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let engine = base64::engine::general_purpose::STANDARD;
    let (Ok(key), Ok(signature)) = (
        engine.decode(spec.public_key.trim()),
        engine.decode(signature.trim()),
    ) else {
        return false;
    };
    let (Ok(key), Ok(signature)) = (
        <[u8; 32]>::try_from(key.as_slice())
            .ok()
            .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok())
            .ok_or(()),
        Signature::from_slice(&signature).map_err(|_| ()),
    ) else {
        return false;
    };
    key.verify(manifest, &signature).is_ok()
}

fn manifest_url(channel: &SignedManifest, version: &str) -> String {
    fill(
        &channel.manifest,
        &[
            ("base", channel.base.trim_end_matches('/')),
            ("version", version),
        ],
    )
}

/// Reads one version's manifest, and checks its signature where the vendor publishes one.
async fn manifest_of(
    ask: Ask<'_>,
    channel: &SignedManifest,
    version: &str,
) -> Result<Vec<HarnessBuild>, String> {
    let url = manifest_url(channel, version);
    let bytes = ask.bytes(&url).await?;
    if let Some(spec) = &channel.signature {
        let signature = ask.text(&fill(&spec.url, &[("manifest", &url)])).await?;
        if !verified(spec, &bytes, &signature) {
            return Err(format!("{url}: its signature does not verify"));
        }
    }
    let manifest: Json =
        serde_json::from_slice(&bytes).map_err(|error| format!("{version}'s manifest: {error}"))?;
    Ok(builds(channel, ask.kind, &manifest, ask.mirror))
}

/// Reads the newest version of each channel.
pub(super) async fn read(
    ask: Ask<'_>,
    channel: &SignedManifest,
) -> (Vec<HarnessBuild>, Vec<String>) {
    let mut found = Vec::new();
    let mut failures = Vec::new();
    let mut versions = Vec::new();
    for name in &channel.channels {
        let url = format!("{}/{name}", channel.base.trim_end_matches('/'));
        match ask.text(&url).await {
            Ok(version) if !version.trim().is_empty() => versions.push(version.trim().to_owned()),
            Ok(_) => failures.push(format!(
                "the {name} channel of {} names no version",
                ask.kind
            )),
            Err(error) => failures.push(error),
        }
    }
    versions.sort();
    versions.dedup();
    for version in versions {
        match manifest_of(ask, channel, &version).await {
            Ok(builds) => found.extend(builds),
            Err(error) => failures.push(error),
        }
    }
    (found, failures)
}

/// Reads the manifest of exactly `version`.
pub(super) async fn read_exact(
    ask: Ask<'_>,
    channel: &SignedManifest,
    version: &str,
) -> Result<Vec<HarnessBuild>, String> {
    manifest_of(ask, channel, version).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_programs::{Channel, Programs};
    use serde_json::json;

    const SHA: &str = "3ec55053109608d5038f7e50750f7d25f768dfd740fef071aa710e2559b63aac";

    fn claude() -> SignedManifest {
        let program = Programs::builtin().program("claude-code").unwrap();
        match &program.builds.channels[0] {
            Channel::SignedManifest(channel) => channel.clone(),
            other => panic!("not a manifest channel: {other:?}"),
        }
    }

    #[test]
    fn a_manifest_gives_its_linux_programs_with_their_checksums() {
        let manifest = json!({ "version": "2.1.309", "platforms": {
            "linux-x64": { "binary": "claude", "checksum": SHA, "size": 1 },
            "linux-arm64": { "binary": "claude", "checksum": "not a checksum", "size": 1 },
            "darwin-arm64": { "binary": "claude", "checksum": SHA, "size": 1 } } });
        let found = builds(&claude(), "claude-code", &manifest, None);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].url,
            "https://downloads.claude.ai/claude-code-releases/2.1.309/linux-x64/claude"
        );
        assert_eq!(
            (found[0].kind.as_str(), found[0].version.as_str()),
            ("claude-code", "2.1.309")
        );
        assert!(builds(&claude(), "claude-code", &json!({}), None).is_empty());
    }

    #[test]
    fn a_vendors_signature_counts_only_when_it_verifies() {
        use base64::Engine as _;
        use ed25519_dalek::{Signer, SigningKey};
        let engine = base64::engine::general_purpose::STANDARD;
        let key = SigningKey::from_bytes(&[7; 32]);
        let spec = ManifestSignature {
            scheme: "ed25519".to_owned(),
            public_key: engine.encode(key.verifying_key().to_bytes()),
            url: "{manifest}.sig".to_owned(),
        };
        let manifest = b"{\"version\":\"1.0.0\"}";
        let good = engine.encode(key.sign(manifest).to_bytes());
        assert!(verified(&spec, manifest, &good));
        assert!(!verified(&spec, b"{\"version\":\"1.0.1\"}", &good));
        assert!(!verified(&spec, manifest, "not base64 !"));
    }
}
