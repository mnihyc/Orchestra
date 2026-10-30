//! The reader of programs published as npm packages.
//!
//! The registry's document of a package lists each version with the address of its tarball, an integrity value
//! (`sha512-` and base64 of the tarball) and the registry's ECDSA signature over `name@version:integrity`, made with
//! a key the registry publishes. A version is kept only when its tarball matches that integrity value and, unless the
//! manifest says the registry signs nothing, the signature verifies against one of the registry's keys. The tool cache
//! checks a SHA-256, which the registry does not give, so the build carries the SHA-256 of the tarball as this reader
//! fetched and verified it.

use base64::Engine as _;
use orch_agent_programs::channels::{Npm, Platform};
use orch_api_contract::admin::HarnessBuild;
use orch_types::Json;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;

use super::{Ask, build, fill, mirrored};

/// A key of the registry's, as `/-/npm/v1/keys` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RegistryKey {
    id: String,
    /// The key's DER, from its base64.
    der: Vec<u8>,
}

fn keys_in(document: &Json) -> Vec<RegistryKey> {
    let engine = base64::engine::general_purpose::STANDARD;
    document
        .get("keys")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter(|key| key.get("expires").is_none_or(Json::is_null))
        .filter_map(|key| {
            Some(RegistryKey {
                id: key.get("keyid").and_then(Json::as_str)?.to_owned(),
                der: engine.decode(key.get("key").and_then(Json::as_str)?).ok()?,
            })
        })
        .collect()
}

/// Whether one of the signatures of a version verifies, against the registry key it names.
fn signed(
    keys: &[RegistryKey],
    package: &str,
    version: &str,
    integrity: &str,
    dist: &Json,
) -> bool {
    let engine = base64::engine::general_purpose::STANDARD;
    let message = format!("{package}@{version}:{integrity}");
    dist.get("signatures")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .any(|signature| {
            let (Some(id), Some(sig)) = (
                signature.get("keyid").and_then(Json::as_str),
                signature.get("sig").and_then(Json::as_str),
            ) else {
                return false;
            };
            let Some(key) = keys.iter().find(|key| key.id == id) else {
                return false;
            };
            let (Ok(key), Ok(sig)) = (
                VerifyingKey::from_public_key_der(&key.der),
                engine
                    .decode(sig)
                    .map_err(|_| ())
                    .and_then(|bytes| Signature::from_der(&bytes).map_err(|_| ())),
            ) else {
                return false;
            };
            key.verify(message.as_bytes(), &sig).is_ok()
        })
}

/// The versions to read of a package's document: the tag's, then the newest others, without pre-releases.
fn chosen(channel: &Npm, document: &Json) -> Vec<String> {
    let Some(versions) = document.get("versions").and_then(Json::as_object) else {
        return Vec::new();
    };
    let mut newest: Vec<&String> = versions
        .keys()
        .filter(|version| !version.contains('-'))
        .collect();
    newest.sort_by(|first, second| crate::hosting::version_order(second, first));
    newest.truncate(usize::try_from(channel.listed).unwrap_or(usize::MAX));
    let mut chosen: Vec<String> = Vec::new();
    if let Some(tagged) = document
        .pointer(&format!("/dist-tags/{}", channel.dist_tag))
        .and_then(Json::as_str)
        .filter(|tagged| versions.contains_key(*tagged))
    {
        chosen.push(tagged.to_owned());
    }
    for version in newest {
        if !chosen.contains(version) {
            chosen.push(version.clone());
        }
    }
    chosen
}

/// The address of a package's document: a scoped name's slash is written `%2f`.
fn package_url(channel: &Npm, package: &str) -> String {
    format!(
        "{}/{}",
        channel.registry.trim_end_matches('/'),
        package.replace('/', "%2f")
    )
}

/// Checks one version of a package and makes its build, fetching its tarball.
async fn version_build(
    ask: Ask<'_>,
    channel: &Npm,
    keys: &[RegistryKey],
    platform: Option<&Platform>,
    package: &str,
    version: &str,
    entry: &Json,
) -> Result<HarnessBuild, String> {
    let dist = entry
        .get("dist")
        .ok_or_else(|| format!("{package}@{version} has no dist"))?;
    let (Some(tarball), Some(integrity)) = (
        dist.get("tarball").and_then(Json::as_str),
        dist.get("integrity").and_then(Json::as_str),
    ) else {
        return Err(format!(
            "{package}@{version} gives no tarball or integrity value"
        ));
    };
    if channel.signature_required && !signed(keys, package, version, integrity, dist) {
        return Err(format!(
            "{package}@{version}: no signature of the registry's verifies"
        ));
    }
    let (found, sha256) = ask.digests(tarball).await?;
    if found != integrity {
        return Err(format!(
            "{package}@{version}: the tarball is not what the registry's integrity value says"
        ));
    }
    let arch = platform.map_or(orch_agent_programs::Arch::X86_64, |platform| platform.arch);
    Ok(build(
        ask.kind,
        version,
        arch,
        mirrored(tarball, ask.mirror),
        &sha256,
    ))
}

async fn read_platform(
    ask: Ask<'_>,
    channel: &Npm,
    platform: Option<&Platform>,
    only: Option<&str>,
) -> (Vec<HarnessBuild>, Vec<String>) {
    let package = match platform {
        Some(platform) => fill(&channel.package, &[("arch", &platform.word)]),
        None => channel.package.clone(),
    };
    let document = match ask.json(&package_url(channel, &package)).await {
        Ok(document) => document,
        Err(error) => return (Vec::new(), vec![error]),
    };
    let keys = if channel.signature_required {
        let url = format!("{}/-/npm/v1/keys", channel.registry.trim_end_matches('/'));
        match ask.json(&url).await {
            Ok(keys) => keys_in(&keys),
            Err(error) => return (Vec::new(), vec![error]),
        }
    } else {
        Vec::new()
    };
    let wanted: Vec<String> = match only {
        Some(version) => vec![version.to_owned()],
        None => chosen(channel, &document),
    };
    let mut found = Vec::new();
    let mut failures = Vec::new();
    let arch = platform.map_or(orch_agent_programs::Arch::X86_64, |platform| platform.arch);
    for version in wanted {
        // A newest-versions read downloads a tarball to know its checksum, so it leaves alone the versions it has.
        if only.is_none() && ask.has(&version, arch) {
            continue;
        }
        let Some(entry) = document.pointer(&format!("/versions/{}", version.replace('/', "~1")))
        else {
            failures.push(format!("{package} has no version {version}"));
            continue;
        };
        match version_build(ask, channel, &keys, platform, &package, &version, entry).await {
            Ok(build) => found.push(build),
            Err(error) => failures.push(error),
        }
    }
    (found, failures)
}

/// Reads the package's newest versions, once for each processor where the package is by processor.
pub(super) async fn read(ask: Ask<'_>, channel: &Npm) -> (Vec<HarnessBuild>, Vec<String>) {
    let mut found = Vec::new();
    let mut failures = Vec::new();
    if channel.platforms.is_empty() {
        return read_platform(ask, channel, None, None).await;
    }
    for platform in &channel.platforms {
        let (builds, failed) = read_platform(ask, channel, Some(platform), None).await;
        found.extend(builds);
        failures.extend(failed);
    }
    (found, failures)
}

/// Reads exactly `version`.
pub(super) async fn read_exact(
    ask: Ask<'_>,
    channel: &Npm,
    version: &str,
) -> Result<Vec<HarnessBuild>, String> {
    let mut found = Vec::new();
    let mut failures = Vec::new();
    let platforms: Vec<Option<&Platform>> = if channel.platforms.is_empty() {
        vec![None]
    } else {
        channel.platforms.iter().map(Some).collect()
    };
    for platform in platforms {
        let (builds, failed) = read_platform(ask, channel, platform, Some(version)).await;
        found.extend(builds);
        failures.extend(failed);
    }
    if found.is_empty()
        && let Some(first) = failures.into_iter().next()
    {
        return Err(first);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::SigningKey;
    use p256::ecdsa::signature::Signer;
    use p256::pkcs8::EncodePublicKey;
    use serde_json::json;
    use sha2::{Digest, Sha512};

    /// The integrity value of a tarball as a registry writes it: `sha512-` and the base64 of its SHA-512.
    fn integrity_of(bytes: &[u8]) -> String {
        format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes))
        )
    }

    fn channel() -> Npm {
        Npm {
            id: "npm".to_owned(),
            package: "@vendor/tool".to_owned(),
            registry: "https://registry.npmjs.org".to_owned(),
            platforms: Vec::new(),
            dist_tag: "latest".to_owned(),
            listed: 2,
            signature_required: true,
        }
    }

    #[test]
    fn a_version_counts_only_when_one_of_the_registrys_keys_verifies_its_signature() {
        let engine = base64::engine::general_purpose::STANDARD;
        let signing = SigningKey::from_slice(&[9; 32]).unwrap();
        let der = signing.verifying_key().to_public_key_der().unwrap();
        let keys = keys_in(&json!({ "keys": [
            { "keyid": "SHA256:abc", "key": engine.encode(der.as_bytes()), "expires": null },
            { "keyid": "SHA256:old", "key": engine.encode(der.as_bytes()), "expires": "2020-01-01T00:00:00.000Z" } ] }));
        assert_eq!(
            keys.len(),
            1,
            "a key that has expired is not one to verify with"
        );
        let integrity = integrity_of(b"tarball");
        let signature: Signature =
            signing.sign(format!("@vendor/tool@1.2.3:{integrity}").as_bytes());
        let dist = json!({ "signatures": [ { "keyid": "SHA256:abc", "sig": engine.encode(signature.to_der().as_bytes()) } ] });
        assert!(signed(&keys, "@vendor/tool", "1.2.3", &integrity, &dist));
        assert!(
            !signed(&keys, "@vendor/tool", "1.2.4", &integrity, &dist),
            "another version is not what was signed"
        );
        assert!(!signed(
            &keys,
            "@vendor/tool",
            "1.2.3",
            &integrity_of(b"other"),
            &dist
        ));
        let unknown = json!({ "signatures": [ { "keyid": "SHA256:nobody", "sig": "AAAA" } ] });
        assert!(!signed(
            &keys,
            "@vendor/tool",
            "1.2.3",
            &integrity,
            &unknown
        ));
    }

    #[test]
    fn the_tags_version_comes_first_then_the_newest_without_pre_releases() {
        let document = json!({
            "dist-tags": { "latest": "1.0.0" },
            "versions": { "1.0.0": {}, "1.1.0": {}, "1.2.0-beta.1": {}, "0.9.0": {} } });
        assert_eq!(chosen(&channel(), &document), ["1.0.0", "1.1.0"]);
    }

    #[test]
    fn a_scoped_package_is_asked_for_with_its_slash_written_out() {
        assert_eq!(
            package_url(&channel(), "@vendor/tool"),
            "https://registry.npmjs.org/@vendor%2ftool"
        );
    }
}
