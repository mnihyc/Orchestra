//! The web edge's renewable certificate, issued by the existing authority and published as one complete pair.

use orch_config::ServerConfig;
use orch_pki::{PkiError, ca::CertificateAuthority};
#[cfg(unix)]
use orch_pki::{
    ca::Holder,
    certificates::{Certificate, LeafKey, ServiceName},
};
use orch_types::Clock;
#[cfg(unix)]
use orch_types::new_id;
#[cfg(unix)]
use std::path::Path;

// The API owns the files; only the web edge's supplementary service group may read its server key.
#[cfg(unix)]
const DIRECTORY_MODE: u32 = 0o750;
#[cfg(unix)]
const KEY_MODE: u32 = 0o640;

#[cfg(unix)]
fn io(path: &Path, source: std::io::Error) -> PkiError {
    PkiError::File {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(unix)]
pub(super) fn keep(
    authority: &CertificateAuthority,
    config: &ServerConfig,
    clock: &dyn Clock,
) -> Result<(), PkiError> {
    use std::os::unix::fs::PermissionsExt;
    let address = url::Url::parse(&config.system.public_url)
        .map_err(|error| PkiError::Certificate(error.to_string()))?;
    let hostname = match address.host() {
        Some(url::Host::Domain(name)) => name.to_owned(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return Err(PkiError::Certificate("public_url names no host".to_owned())),
    };
    let mut names = vec![hostname];
    if let Some(domain) = config
        .edge
        .ports_domain
        .as_deref()
        .filter(|domain| !domain.is_empty())
    {
        names.push(format!("*.{domain}"));
    }
    let directory = Path::new(&config.storage.root).join("frontend-tls");
    std::fs::create_dir_all(&directory).map_err(|error| io(&directory, error))?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(DIRECTORY_MODE))
        .map_err(|error| io(&directory, error))?;
    let signature = format!(
        "{}\n{}\n",
        authority.certificate().fingerprint(),
        names.join("\n")
    );
    let current = directory.join("current");
    if std::fs::read_to_string(current.join("names"))
        .ok()
        .as_deref()
        == Some(signature.as_str())
        && let Ok(pem) = std::fs::read_to_string(current.join("chain.pem"))
        && let Ok(certificates) = Certificate::all_from_pem(&pem)
        && let Some(certificate) = certificates.first()
        && certificate.service() == Some(ServiceName::Frontend)
        && !orch_pki::renewal::due(certificate, clock.time())
        && let Ok(key) = std::fs::read_to_string(current.join("key.pem"))
            .and_then(|pem| LeafKey::from_pem(&pem).map_err(std::io::Error::other))
        && certificate.holds(&key)
    {
        return Ok(());
    }
    let (key, certificate) = authority.issue_with_new_key(
        &Holder::Service {
            service: ServiceName::Frontend,
            names,
        },
        config.pki.leaf_validity_days,
        clock,
    )?;
    publish(
        &directory,
        key,
        &certificate,
        &authority.certificate_pem(),
        &signature,
    )
}

#[cfg(not(unix))]
pub(super) fn keep(
    _authority: &CertificateAuthority,
    _config: &ServerConfig,
    _clock: &dyn Clock,
) -> Result<(), PkiError> {
    Err(PkiError::Certificate(
        "frontend certificate publication requires Unix file permissions".to_owned(),
    ))
}

#[cfg(unix)]
fn publish(
    directory: &Path,
    key: LeafKey,
    certificate: &Certificate,
    authority: &str,
    names: &str,
) -> Result<(), PkiError> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let generation = format!("certificate-{}", new_id());
    let path = directory.join(&generation);
    std::fs::create_dir(&path).map_err(|error| io(&path, error))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(DIRECTORY_MODE))
        .map_err(|error| io(&path, error))?;
    let private = path.join("key.pem");
    orch_pki::certificates::write_private_file(&private, &key.to_pem())?;
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(KEY_MODE))
        .map_err(|error| io(&private, error))?;
    orch_pki::certificates::write_file(
        &path.join("chain.pem"),
        &format!("{}{authority}", certificate.pem()),
    )?;
    orch_pki::certificates::write_file(&path.join("names"), names)?;
    let next = directory.join(format!(".current-{}", new_id()));
    symlink(&generation, &next).map_err(|error| io(&next, error))?;
    std::fs::rename(&next, directory.join("current")).map_err(|error| io(&next, error))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use orch_types::ManualClock;

    #[test]
    fn the_existing_authority_issues_and_renews_the_front_with_its_names_as_a_complete_pair() {
        let clock = ManualClock::new(chrono::Utc.with_ymd_and_hms(2026, 10, 25, 0, 0, 0).unwrap());
        let authority = CertificateAuthority::create("Internal", 3650, &clock).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::default();
        config.storage.root = directory.path().to_string_lossy().into_owned();
        config.system.public_url = "https://orch.test:41701".into();
        config.edge.ports_domain = Some("ports.test".into());
        let current = directory.path().join("frontend-tls/current");
        let read = || {
            let chain = Certificate::all_from_pem(
                &std::fs::read_to_string(current.join("chain.pem")).unwrap(),
            )
            .unwrap();
            let key = LeafKey::from_pem(&std::fs::read_to_string(current.join("key.pem")).unwrap())
                .unwrap();
            assert!(chain[0].holds(&key));
            assert_eq!(chain[1], *authority.certificate());
            assert_eq!(chain[0].service(), Some(ServiceName::Frontend));
            assert!(
                !chain[0].allows_client_auth(),
                "the front key has no internal client authority"
            );
            chain[0].clone()
        };
        keep(&authority, &config, &clock).unwrap();
        let first = read();
        assert_eq!(first.dns_names(), ["orch.test", "*.ports.test"]);
        let generation = std::fs::read_link(&current).unwrap();
        keep(&authority, &config, &clock).unwrap();
        assert_eq!(std::fs::read_link(&current).unwrap(), generation);
        clock.advance(chrono::Duration::days(250));
        keep(&authority, &config, &clock).unwrap();
        assert!(read().not_after() > first.not_after());
        assert_ne!(std::fs::read_link(&current).unwrap(), generation);
        config.system.public_url = "https://renamed.test".into();
        keep(&authority, &config, &clock).unwrap();
        assert_eq!(read().dns_names(), ["renamed.test", "*.ports.test"]);
    }
}
