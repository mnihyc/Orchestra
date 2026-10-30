//! Bootstrap secrets, CA and first-start support.
//!
//! `compose-secrets` makes everything a deployment's secrets directory holds,
//! once: the certificate authority, the master key that seals what the
//! database stores, the key of Web Push, the first certificate of each
//! service, and the token with which the bundled node enrolls. It makes them
//! all or none. A directory that already holds the whole set is left exactly
//! as it is, because replacing the authority or the master key would cut off
//! every machine or make every stored secret unreadable; a directory that
//! holds only a part of the set is refused and named, because whatever was
//! made of it cannot be told from what is missing.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_config::ServerConfig;
use orch_pki::PkiError;
use orch_pki::ca::{CA_CERT_FILE, CA_KEY_FILE, CertificateAuthority, Holder};
use orch_pki::certificates::{LeafKey, ServiceName, write_file, write_private_file};
use orch_pki::enrollment::issue_token;
use orch_types::Clock;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The file of the master key: 32 random bytes in hexadecimal.
pub const MASTER_KEY_FILE: &str = "orch_master_key";
/// The file of the key that signs Web Push messages.
pub const VAPID_KEY_FILE: &str = "orch_vapid_private_key";
/// The file of the token the bundled node enrolls with.
pub const NODE_TOKEN_FILE: &str = "node_enrollment_token";
/// The file of the key that signs who a port's caller is: 32 random bytes in hexadecimal. It is
/// made beside a set, new or older than it, and is not part of what makes a set complete, so that a
/// deployment from before it is never taken for a partial one.
pub const PORT_ASSERTION_KEY_FILE: &str = orch_pki::port_assertions::KEY_FILE;
/// The bytes of the master key.
pub const MASTER_KEY_BYTES: usize = 32;
/// The services that get a certificate of their own, in the order they are
/// written.
pub const SERVICES: [ServiceName; 3] =
    [ServiceName::Api, ServiceName::Gateway, ServiceName::Worker];

/// The file of a service's private key.
#[must_use]
pub fn service_key_file(service: ServiceName) -> String {
    format!("service_{service}_key.pem")
}

/// The file of a service's certificate.
#[must_use]
pub fn service_cert_file(service: ServiceName) -> String {
    format!("service_{service}_cert.pem")
}

/// How a file is protected: what no other user may read, and what anybody
/// may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// Readable by its owner alone.
    Private,
    /// Readable by all.
    Public,
}

/// The files of a complete set, with how each is protected.
#[must_use]
pub fn expected_files() -> Vec<(String, Protection)> {
    let mut files = vec![
        (CA_KEY_FILE.to_owned(), Protection::Private),
        (CA_CERT_FILE.to_owned(), Protection::Public),
        (MASTER_KEY_FILE.to_owned(), Protection::Private),
        (VAPID_KEY_FILE.to_owned(), Protection::Private),
    ];
    for service in SERVICES {
        files.push((service_key_file(service), Protection::Private));
        files.push((service_cert_file(service), Protection::Public));
    }
    files.push((NODE_TOKEN_FILE.to_owned(), Protection::Private));
    files
}

/// What `compose-secrets` is asked to make.
#[derive(Debug, Clone)]
pub struct Options {
    /// The directory the secrets go in.
    pub secrets_dir: PathBuf,
    /// What the system is called, which the authority's name carries.
    pub system_name: String,
    /// The names every service certificate is good for.
    pub service_dns_names: Vec<String>,
    /// How long the authority is good for, in days.
    pub ca_validity_days: u64,
    /// How long a service certificate is good for, in days.
    pub leaf_validity_days: u64,
}

impl Options {
    /// The options a deployment's `orch.toml` gives for the secrets in
    /// `secrets_dir`.
    #[must_use]
    pub fn from_config(secrets_dir: PathBuf, config: &ServerConfig) -> Self {
        Self {
            secrets_dir,
            system_name: config.system.name.clone(),
            service_dns_names: config.pki.service_dns_names.clone(),
            ca_validity_days: config.pki.ca_validity_days,
            leaf_validity_days: config.pki.leaf_validity_days,
        }
    }
}

/// What came of asking for the secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The set was made now.
    Made {
        /// The SHA-256 of the authority's certificate, in hexadecimal.
        ca_fingerprint: String,
        /// The pin that enrollment tokens carry: the SHA-256 of the
        /// authority's public key, in unpadded base64url.
        ca_pin: String,
        /// The files that were written.
        files: Vec<String>,
    },
    /// The whole set was there, and nothing was written.
    AlreadyComposed,
}

/// Why the secrets were not made.
#[derive(Debug, thiserror::Error)]
pub enum ComposeError {
    /// Part of the set is there. Nothing was written.
    #[error(
        "{dir} holds only part of the secrets (has {present}; lacks {missing}); \
         nothing was written. Remove the part that is there only if the deployment has never been started, \
         or restore the rest from a backup"
    )]
    Partial {
        /// The directory.
        dir: String,
        /// The files that are there.
        present: String,
        /// The files that are not.
        missing: String,
    },
    /// Something could not be made or written.
    #[error("{0}")]
    Pki(#[from] PkiError),
    /// The directory could not be made or read.
    #[error("{path}: {source}")]
    Directory {
        /// The directory.
        path: String,
        /// What went wrong.
        source: std::io::Error,
    },
}

/// The mode of a directory that holds secrets: its owner's alone.
#[cfg(unix)]
const SECRETS_DIR_MODE: u32 = 0o700;

fn make_directory(dir: &Path) -> Result<(), ComposeError> {
    let wrong = |source| ComposeError::Directory {
        path: dir.display().to_string(),
        source,
    };
    let existed = dir.is_dir();
    std::fs::create_dir_all(dir).map_err(wrong)?;
    // A directory this made is closed to others; one that was there is a
    // mount or a directory the operator prepared, and is left as it is.
    #[cfg(unix)]
    if !existed {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(SECRETS_DIR_MODE))
            .map_err(wrong)?;
    }
    Ok(())
}

/// Makes the secrets of a deployment, or says that they are there.
pub fn compose_secrets(options: &Options, clock: &dyn Clock) -> Result<Outcome, ComposeError> {
    let files = expected_files();
    let (present, missing): (Vec<_>, Vec<_>) = files
        .iter()
        .partition(|(name, _)| options.secrets_dir.join(name).exists());
    if missing.is_empty() {
        add_port_assertion_key(&options.secrets_dir)?;
        return Ok(Outcome::AlreadyComposed);
    }
    if !present.is_empty() {
        let names = |list: &[&(String, Protection)]| {
            list.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(ComposeError::Partial {
            dir: options.secrets_dir.display().to_string(),
            present: names(&present),
            missing: names(&missing),
        });
    }

    // Everything is made before anything is written, so that a failure to
    // make one leaves the directory as it was.
    let authority =
        CertificateAuthority::create(&options.system_name, options.ca_validity_days, clock)?;
    let mut contents: Vec<(String, Protection, String)> = vec![
        (
            CA_KEY_FILE.to_owned(),
            Protection::Private,
            authority.key_pem(),
        ),
        (
            CA_CERT_FILE.to_owned(),
            Protection::Public,
            authority.certificate_pem(),
        ),
        (
            MASTER_KEY_FILE.to_owned(),
            Protection::Private,
            master_key(),
        ),
        (
            VAPID_KEY_FILE.to_owned(),
            Protection::Private,
            LeafKey::generate()?.to_pem(),
        ),
    ];
    for service in SERVICES {
        let (key, certificate) = authority.issue_with_new_key(
            &Holder::Service {
                service,
                names: options.service_dns_names.clone(),
            },
            options.leaf_validity_days,
            clock,
        )?;
        contents.push((service_key_file(service), Protection::Private, key.to_pem()));
        contents.push((
            service_cert_file(service),
            Protection::Public,
            certificate.pem(),
        ));
    }
    // A token carries no expiry of its own: the row that the system keeps
    // for it says until when it is good, once the system registers it.
    let token = issue_token(&authority, Duration::ZERO, clock);
    contents.push((
        NODE_TOKEN_FILE.to_owned(),
        Protection::Private,
        format!("{}\n", token.token),
    ));

    make_directory(&options.secrets_dir)?;
    // The keys go first and the token last: a set that was cut short is a
    // partial one whichever file is missing, and is refused next time.
    for (name, protection, text) in &contents {
        let path = options.secrets_dir.join(name);
        match protection {
            Protection::Private => write_private_file(&path, text)?,
            Protection::Public => write_file(&path, text)?,
        }
    }
    add_port_assertion_key(&options.secrets_dir)?;
    Ok(Outcome::Made {
        ca_fingerprint: authority.certificate().fingerprint(),
        ca_pin: URL_SAFE_NO_PAD.encode(authority.pin()),
        files: contents.into_iter().map(|(name, _, _)| name).collect(),
    })
}

/// Makes the port assertion key beside a set that has none yet; one that is there stays.
fn add_port_assertion_key(dir: &Path) -> Result<(), ComposeError> {
    let path = dir.join(PORT_ASSERTION_KEY_FILE);
    if path.exists() {
        return Ok(());
    }
    write_private_file(&path, &master_key())?;
    Ok(())
}

/// A master key: random bytes in hexadecimal, with a line break after.
fn master_key() -> String {
    let mut bytes = [0u8; MASTER_KEY_BYTES];
    rand::fill(&mut bytes[..]);
    format!("{}\n", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_pki::certificates::Certificate;
    use orch_types::SystemClock;

    fn options(dir: &Path) -> Options {
        Options {
            secrets_dir: dir.to_owned(),
            system_name: "Test".to_owned(),
            service_dns_names: vec!["api".to_owned(), "gateway.orch.internal".to_owned()],
            ca_validity_days: 3_650,
            leaf_validity_days: 365,
        }
    }

    fn read(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    #[test]
    fn a_deployment_gets_its_authority_its_keys_its_service_certificates_and_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = dir.path().join("secrets");
        let Outcome::Made { files, ca_pin, .. } =
            compose_secrets(&options(&secrets), &SystemClock).unwrap()
        else {
            panic!("nothing was made");
        };
        assert_eq!(files.len(), expected_files().len());
        let authority = CertificateAuthority::from_pem(
            &read(&secrets, CA_CERT_FILE),
            &read(&secrets, CA_KEY_FILE),
        )
        .unwrap();
        for service in SERVICES {
            let certificate =
                Certificate::from_pem(&read(&secrets, &service_cert_file(service))).unwrap();
            let key = LeafKey::from_pem(&read(&secrets, &service_key_file(service))).unwrap();
            assert!(certificate.holds(&key), "{service} has its own key");
            assert_eq!(certificate.service(), Some(service));
            assert!(
                certificate
                    .dns_names()
                    .contains(&"gateway.orch.internal".to_owned())
            );
            assert!(certificate.allows_server_auth() && certificate.allows_client_auth());
        }
        let token: orch_link_contract::enrollment::EnrollmentToken =
            read(&secrets, NODE_TOKEN_FILE).trim().parse().unwrap();
        assert!(
            token.pins(&authority.pin()),
            "the token carries the authority's pin"
        );
        assert_eq!(URL_SAFE_NO_PAD.encode(token.pin()), ca_pin);
        let master = read(&secrets, MASTER_KEY_FILE);
        assert_eq!(master.trim().len(), MASTER_KEY_BYTES * 2);
        assert!(master.trim().bytes().all(|byte| byte.is_ascii_hexdigit()));
        LeafKey::from_pem(&read(&secrets, VAPID_KEY_FILE)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn keys_and_tokens_are_for_their_owner_alone_and_certificates_for_all() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let secrets = dir.path().join("secrets");
        compose_secrets(&options(&secrets), &SystemClock).unwrap();
        for (name, protection) in expected_files() {
            let mode = std::fs::metadata(secrets.join(&name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            let expected = match protection {
                Protection::Private => 0o600,
                Protection::Public => 0o644,
            };
            assert_eq!(mode, expected, "{name}");
        }
        let directory = std::fs::metadata(&secrets).unwrap().permissions().mode() & 0o777;
        assert_eq!(directory, 0o700);
    }

    #[test]
    fn a_set_from_before_the_port_assertion_key_gets_one_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        compose_secrets(&options(dir.path()), &SystemClock).unwrap();
        let first = read(dir.path(), PORT_ASSERTION_KEY_FILE);
        assert_eq!(first.trim().len(), MASTER_KEY_BYTES * 2);
        std::fs::remove_file(dir.path().join(PORT_ASSERTION_KEY_FILE)).unwrap();
        assert_eq!(
            compose_secrets(&options(dir.path()), &SystemClock).unwrap(),
            Outcome::AlreadyComposed
        );
        let added = read(dir.path(), PORT_ASSERTION_KEY_FILE);
        assert_eq!(added.trim().len(), MASTER_KEY_BYTES * 2);
        compose_secrets(&options(dir.path()), &SystemClock).unwrap();
        assert_eq!(read(dir.path(), PORT_ASSERTION_KEY_FILE), added);
    }

    #[test]
    fn a_complete_set_is_left_exactly_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        compose_secrets(&options(dir.path()), &SystemClock).unwrap();
        let before: Vec<String> = expected_files()
            .iter()
            .map(|(name, _)| read(dir.path(), name))
            .collect();
        assert_eq!(
            compose_secrets(&options(dir.path()), &SystemClock).unwrap(),
            Outcome::AlreadyComposed
        );
        let after: Vec<String> = expected_files()
            .iter()
            .map(|(name, _)| read(dir.path(), name))
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn a_partial_set_is_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        compose_secrets(&options(dir.path()), &SystemClock).unwrap();
        std::fs::remove_file(dir.path().join(MASTER_KEY_FILE)).unwrap();
        let ca_before = read(dir.path(), CA_KEY_FILE);
        let error = compose_secrets(&options(dir.path()), &SystemClock).unwrap_err();
        let message = error.to_string();
        assert!(message.contains(MASTER_KEY_FILE), "{message}");
        assert!(matches!(error, ComposeError::Partial { .. }));
        assert!(!dir.path().join(MASTER_KEY_FILE).exists());
        assert_eq!(read(dir.path(), CA_KEY_FILE), ca_before);
    }

    #[test]
    fn a_directory_with_other_files_in_it_still_gets_a_set() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README"), "kept").unwrap();
        compose_secrets(&options(dir.path()), &SystemClock).unwrap();
        assert_eq!(read(dir.path(), "README"), "kept");
    }
}
