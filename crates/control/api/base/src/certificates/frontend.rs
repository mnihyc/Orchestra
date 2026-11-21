//! Immutable frontend certificate generations, staged separately from the workers that adopt them.

use orch_api_contract::public_addresses::PublicAddresses;
use orch_config::ServerConfig;
use orch_pki::{PkiError, ca::CertificateAuthority};
#[cfg(unix)]
use orch_pki::{
    ca::Holder,
    certificates::{Certificate, LeafKey, ServiceName},
};
use orch_types::Clock;
#[cfg(unix)]
use std::path::Path;

/// One immutable complete pair. The public fingerprint proves which pair a fresh TLS client reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub name: String,
    pub fingerprint: String,
}

// Only the API owner writes generations; the frontend's service group reads the server key.
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
fn directory(config: &ServerConfig) -> Result<std::path::PathBuf, PkiError> {
    use std::os::unix::fs::PermissionsExt;
    let path = Path::new(&config.storage.root).join("frontend-tls");
    std::fs::create_dir_all(&path).map_err(|error| io(&path, error))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(DIRECTORY_MODE))
        .map_err(|error| io(&path, error))?;
    Ok(path)
}

/// Keep the filesystem writer fenced even if its database backend is lost during a network probe.
#[cfg(unix)]
pub(crate) fn lock(config: &ServerConfig) -> Result<Option<std::fs::File>, PkiError> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = directory(config)?.join(".change.lock");
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(KEY_MODE)
        .open(&path)
    {
        Ok(file) => file,
        // An authorized local recovery operator may only need the service group's read access.
        // Unix flock permits an exclusive lock on this read-only descriptor.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::File::open(&path).map_err(|error| io(&path, error))?
        }
        Err(error) => return Err(io(&path, error)),
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(io(&path, error)),
    }
}

#[cfg(unix)]
pub(crate) fn prepare(
    authority: &CertificateAuthority,
    config: &ServerConfig,
    addresses: &PublicAddresses,
    clock: &dyn Clock,
    fresh: bool,
) -> Result<Generation, PkiError> {
    use std::os::unix::fs::PermissionsExt;
    let addresses = addresses
        .validated()
        .map_err(|error| PkiError::Certificate(error.to_string()))?;
    let mut names = Vec::new();
    for origin in std::iter::once(&addresses.public_url).chain(&addresses.alternate_urls) {
        let address =
            url::Url::parse(origin).map_err(|error| PkiError::Certificate(error.to_string()))?;
        let hostname = match address.host() {
            Some(url::Host::Domain(name)) => name.to_owned(),
            Some(url::Host::Ipv4(ip)) => ip.to_string(),
            Some(url::Host::Ipv6(ip)) => ip.to_string(),
            None => {
                return Err(PkiError::Certificate(
                    "the public origin names no host".to_owned(),
                ));
            }
        };
        if !names.contains(&hostname) {
            names.push(hostname);
        }
    }
    if let Some(domain) = config
        .edge
        .ports_domain
        .as_deref()
        .filter(|domain| !domain.is_empty())
    {
        names.push(format!("*.{domain}"));
    }
    // The composing service probes its own frontend over authenticated TLS before publishing browser addresses.
    if !names.iter().any(|name| name == "frontend") {
        names.push("frontend".to_owned());
    }
    let directory = directory(config)?;
    // A saved login/link can still use an earlier origin. Retaining its certificate name does
    // not add that origin back to the effective address set or to Ask's current allowlist.
    if let Ok(previous) = std::fs::read_to_string(directory.join("current/names")) {
        let mut retained: Vec<String> = previous
            .lines()
            .skip(1)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        for name in names {
            if !retained.contains(&name) {
                retained.push(name);
            }
        }
        names = retained;
    }
    let signature = format!(
        "{}\n{}\n",
        authority.certificate().fingerprint(),
        names.join("\n")
    );
    let current = directory.join("current");
    if !fresh
        && std::fs::read_to_string(current.join("names"))
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
        && let Ok(name) = std::fs::read_link(&current)
        && let Some(name) = name.to_str().filter(|name| valid_generation(name))
    {
        return Ok(Generation {
            name: name.to_owned(),
            fingerprint: certificate.fingerprint(),
        });
    }
    let (key, certificate) = authority.issue_with_new_key(
        &Holder::Service {
            service: ServiceName::Frontend,
            names,
        },
        config.pki.leaf_validity_days,
        clock,
    )?;
    let generation = Generation {
        name: format!("certificate-{}", orch_types::new_id()),
        fingerprint: certificate.fingerprint(),
    };
    let path = directory.join(&generation.name);
    std::fs::create_dir(&path).map_err(|error| io(&path, error))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(DIRECTORY_MODE))
        .map_err(|error| io(&path, error))?;
    let private = path.join("key.pem");
    orch_pki::certificates::write_private_file(&private, &key.to_pem())?;
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(KEY_MODE))
        .map_err(|error| io(&private, error))?;
    orch_pki::certificates::write_file(
        &path.join("chain.pem"),
        &format!("{}{}", certificate.pem(), authority.certificate_pem()),
    )?;
    orch_pki::certificates::write_file(&path.join("names"), &signature)?;
    std::fs::File::open(&private)
        .and_then(|file| file.sync_all())
        .map_err(|error| io(&private, error))?;
    std::fs::File::open(&path)
        .and_then(|file| file.sync_all())
        .map_err(|error| io(&path, error))?;
    std::fs::File::open(&directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| io(&directory, error))?;
    Ok(generation)
}

#[cfg(unix)]
fn valid_generation(name: &str) -> bool {
    name.strip_prefix("certificate-")
        .is_some_and(|id| id.parse::<orch_types::Id>().is_ok())
}

#[cfg(unix)]
pub(crate) fn activate(config: &ServerConfig, generation: &Generation) -> Result<(), PkiError> {
    use std::os::unix::fs::symlink;
    if !valid_generation(&generation.name) {
        return Err(PkiError::Certificate(
            "the frontend generation is invalid".to_owned(),
        ));
    }
    let directory = directory(config)?;
    let path = directory.join(&generation.name);
    let chain =
        std::fs::read_to_string(path.join("chain.pem")).map_err(|error| io(&path, error))?;
    let certificates = Certificate::all_from_pem(&chain)?;
    let certificate = certificates.first().ok_or_else(|| {
        PkiError::Certificate("the frontend generation contains no certificate".to_owned())
    })?;
    let key = std::fs::read_to_string(path.join("key.pem")).map_err(|error| io(&path, error))?;
    if certificate.fingerprint() != generation.fingerprint
        || !certificate.holds(&LeafKey::from_pem(&key)?)
    {
        return Err(PkiError::Certificate(
            "the frontend generation is not its complete certificate pair".to_owned(),
        ));
    }
    let current = directory.join("current");
    if std::fs::read_link(&current).ok().as_deref() == Some(Path::new(&generation.name)) {
        return Ok(());
    }
    let next = directory.join(format!(".current-{}", orch_types::new_id()));
    symlink(&generation.name, &next).map_err(|error| io(&next, error))?;
    std::fs::rename(&next, current).map_err(|error| io(&next, error))?;
    std::fs::File::open(&directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| io(&directory, error))
}

pub(super) fn keep(
    authority: &CertificateAuthority,
    config: &ServerConfig,
    addresses: &PublicAddresses,
    clock: &dyn Clock,
) -> Result<(), PkiError> {
    let generation = prepare(authority, config, addresses, clock, false)?;
    activate(config, &generation)
}

#[cfg(not(unix))]
pub(crate) fn lock(_config: &ServerConfig) -> Result<Option<std::fs::File>, PkiError> {
    Err(PkiError::Certificate(
        "frontend certificate publication requires Unix file permissions".to_owned(),
    ))
}
#[cfg(not(unix))]
pub(crate) fn prepare(
    _authority: &CertificateAuthority,
    _config: &ServerConfig,
    _addresses: &PublicAddresses,
    _clock: &dyn Clock,
    _fresh: bool,
) -> Result<Generation, PkiError> {
    Err(PkiError::Certificate(
        "frontend certificate publication requires Unix file permissions".to_owned(),
    ))
}
#[cfg(not(unix))]
pub(crate) fn activate(_config: &ServerConfig, _generation: &Generation) -> Result<(), PkiError> {
    Err(PkiError::Certificate(
        "frontend certificate publication requires Unix file permissions".to_owned(),
    ))
}

#[cfg(all(any(test, feature = "test-support"), unix))]
pub mod tests {

    use super::*;

    use chrono::TimeZone;

    use orch_types::ManualClock;
    #[cfg(any(test, feature = "test-support"))]
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
        keep(&authority, &config, &config.system.address_seed(), &clock).unwrap();
        let first = read();
        assert_eq!(first.dns_names(), ["orch.test", "*.ports.test", "frontend"]);
        let generation = std::fs::read_link(&current).unwrap();
        keep(&authority, &config, &config.system.address_seed(), &clock).unwrap();
        assert_eq!(std::fs::read_link(&current).unwrap(), generation);
        clock.advance(chrono::Duration::days(250));
        keep(&authority, &config, &config.system.address_seed(), &clock).unwrap();
        assert!(read().not_after() > first.not_after());
        assert_ne!(std::fs::read_link(&current).unwrap(), generation);
        config.system.public_url = "https://renamed.test".into();
        keep(&authority, &config, &config.system.address_seed(), &clock).unwrap();
        assert_eq!(
            read().dns_names(),
            ["orch.test", "*.ports.test", "frontend", "renamed.test"]
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_the_existing_authority_issues_and_renews_the_front_with_its_names_as_a_complete_pair()
     {
        self::the_existing_authority_issues_and_renews_the_front_with_its_names_as_a_complete_pair()
    }
}
