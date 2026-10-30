//! Local keys, sealed exchange, shared renewal and handover.
//!
//! **The sealed store.** A value the machine must keep across a restart (the key of an API provider that
//! arrived with `provider.start`, or one a person gave to this machine) is written to a file of `secrets/` in
//! the data directory, mode 0600, and the file does not hold the value: it holds the value sealed to this
//! machine's own key (the same sealing as a value that comes from the system), with the name of the file as
//! the context it is bound to, so that a copied file cannot be read without the machine's key and a file
//! renamed to another name does not open. Nothing else on disk ever holds a key.
//!
//! **A key kept here.** `orch-connector provider set-key` puts the key of a provider into the store under
//! `provider-<id>.key` ([`local_key_name`]) without the connector running; a `provider.start` that carries no
//! key (the key is kept on this machine) makes the provider role read it from there when a request needs it.
//!
//! **A credential on its way between machines**: the machine that keeps it issues it under a lease and seals
//! it to the machine where the harness runs; that machine installs it; for a login that moved, the same
//! machine gives it back sealed to the machine that keeps it, which takes it home. The system relays the
//! sealed text and cannot read it. What the sealing is bound to cannot be the lease: `agent.credential.set`
//! and `provider.credential.accept` carry no lease, so the receiving end could not open a value bound to
//! one. It is bound to what both ends of each command know, the direction, the provider and the form
//! ([`exchange_info`]), so that a value issued for one provider or form does not open as another's, and one
//! that went out does not open as one that came back.
//!
//! **When a copy may be issued** ([`check_issue`]): only under an open lease of the mode that fits the form's
//! custody (a copy that stays the provider's under `shared`, a login that moves under `handover`), only in the
//! form the lease names, only to the machine and the key the lease names, and not within thirty seconds of the
//! last copy of that lease, so that a harness that asks over and over does not drain the key's custodian.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use orch_agent_contract::credentials::{CredentialForm, Custody, IssuedCredential, may_issue};
use orch_link_contract::commands::{
    CommandError, ErrorCode, LeaseMode, MachineKey, Sealed, SealedCredential,
};
use orch_types::{Id, Time};

use crate::leases::Lease;
use crate::roles::Keys;

/// The directory of the store, inside the data directory.
pub const STORE_DIR: &str = "secrets";
/// How a file of the store is bound to its name: the context the sealing is bound to begins with this.
const STORE_CONTEXT: &str = "orch/secret-store/v1/";
/// How a credential that travels is bound: the context begins with this.
const EXCHANGE_CONTEXT: &str = "orch/credential/v1/";
/// The most a name of the store may have.
const MAX_NAME_BYTES: usize = 128;
/// The prefix of the file that holds a provider started here.
const PROVIDER_PREFIX: &str = "provider-";
/// The suffix of the file that holds a key a person gave to this machine for a provider.
const LOCAL_KEY_SUFFIX: &str = ".key";

/// Why the store could not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The name is not one the store keeps.
    #[error("{0:?} is not a name the store keeps")]
    Name(String),
    /// The directory or a file could not be read or written.
    #[error("the store could not be used: {0}")]
    Io(#[from] std::io::Error),
    /// A file does not open with this machine's key under its name.
    #[error("the file {name} does not open with this machine's key")]
    Sealed {
        /// The name of the file.
        name: String,
    },
}

/// The name under which the record of a provider started here is kept.
#[must_use]
pub fn provider_record_name(provider_id: Id) -> String {
    format!("{PROVIDER_PREFIX}{provider_id}")
}

/// The name under which a key a person gave to this machine for a provider is kept.
#[must_use]
pub fn local_key_name(provider_id: Id) -> String {
    format!("{PROVIDER_PREFIX}{provider_id}{LOCAL_KEY_SUFFIX}")
}

/// The sealed record naming the profile to which a provider's issued credential must return.
#[must_use]
pub fn provider_custodian_name(provider_id: Id) -> String {
    format!("custodian-{provider_id}")
}

/// The ids of the providers whose records the store holds.
#[must_use]
pub fn provider_ids(names: &[String]) -> Vec<Id> {
    names
        .iter()
        .filter(|name| !name.ends_with(LOCAL_KEY_SUFFIX))
        .filter_map(|name| name.strip_prefix(PROVIDER_PREFIX)?.parse().ok())
        .collect()
}

/// The sealed store of one data directory.
pub struct SecretStore {
    dir: PathBuf,
    keys: Arc<Keys>,
}

impl std::fmt::Debug for SecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretStore")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn make_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

impl SecretStore {
    /// The store in `data_dir`, made if it is not there.
    pub fn open(data_dir: &Path, keys: Arc<Keys>) -> Result<Self, StoreError> {
        let dir = data_dir.join(STORE_DIR);
        make_private_directory(&dir)?;
        Ok(Self { dir, keys })
    }

    fn path(&self, name: &str) -> Result<PathBuf, StoreError> {
        if valid_name(name) {
            Ok(self.dir.join(name))
        } else {
            Err(StoreError::Name(name.to_owned()))
        }
    }

    fn context(name: &str) -> Vec<u8> {
        format!("{STORE_CONTEXT}{name}").into_bytes()
    }

    /// Keeps `value` under `name`, sealed to this machine's key, replacing what was there. The file is
    /// written whole beside its place and moved into it, so that it is never half written.
    pub fn put(&self, name: &str, value: &str) -> Result<(), StoreError> {
        let path = self.path(name)?;
        let sealed = self
            .keys
            .seal_for_self(&Self::context(name), value)
            .map_err(|_| StoreError::Sealed {
                name: name.to_owned(),
            })?;
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        std::io::Write::write_all(&mut file, sealed.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&path).map_err(|error| error.error)?;
        #[cfg(unix)]
        std::fs::File::open(&self.dir)?.sync_all()?;
        Ok(())
    }

    /// What is kept under `name`; `None` where nothing is.
    pub fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
        let path = self.path(name)?;
        let sealed = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        self.keys
            .open_with(&Self::context(name), sealed.trim())
            .map(Some)
            .map_err(|_| StoreError::Sealed {
                name: name.to_owned(),
            })
    }

    /// Forgets what is kept under `name`; nothing is wrong if nothing was.
    pub fn remove(&self, name: &str) -> Result<(), StoreError> {
        let path = self.path(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {
                #[cfg(unix)]
                std::fs::File::open(&self.dir)?.sync_all()?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// The names the store keeps, in order.
    pub fn names(&self) -> Result<Vec<String>, StoreError> {
        let mut names: Vec<String> = std::fs::read_dir(&self.dir)?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| valid_name(name))
            .collect();
        names.sort();
        Ok(names)
    }
}

/// Which way a credential travels between machines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the machine that keeps it to the machine where the harness runs.
    Issued,
    /// Back from the machine where the harness runs.
    Returned,
}

/// What the sealing of a credential that travels is bound to.
#[must_use]
pub fn exchange_info(direction: Direction, provider_id: Id, form: &str) -> Vec<u8> {
    let way = match direction {
        Direction::Issued => "issued",
        Direction::Returned => "returned",
    };
    format!("{EXCHANGE_CONTEXT}{way}/{provider_id}/{form}").into_bytes()
}

fn refuse(code: ErrorCode, detail: impl Into<String>) -> CommandError {
    CommandError::new(code, detail)
}

/// Whether a copy of the credential may be issued now under `lease`, in `form`, to `recipient`. `form` is
/// the declaration of the form by whoever issues it, and `now` the moment.
pub fn check_issue(
    lease: &Lease,
    form: &CredentialForm,
    recipient: &MachineKey,
    now: Time,
) -> Result<(), CommandError> {
    if !lease.is_open() {
        return Err(refuse(
            ErrorCode::LeaseClosed,
            "the lease is closed; nothing is issued under it",
        ));
    }
    let wanted_mode = match form.custody {
        Custody::Shared => LeaseMode::Shared,
        Custody::Transferred => LeaseMode::Handover,
    };
    if lease.mode() != wanted_mode {
        return Err(refuse(
            ErrorCode::Conflict,
            format!(
                "the form {} has custody {:?}, which a lease in mode {:?} does not issue",
                form.form,
                form.custody,
                lease.mode()
            ),
        ));
    }
    if lease.form() != Some(form.form.as_str()) {
        return Err(refuse(
            ErrorCode::Conflict,
            format!(
                "the lease allows the form {:?}, not {:?}",
                lease.form(),
                form.form
            ),
        ));
    }
    if lease.recipient() != Some(recipient) {
        return Err(refuse(
            ErrorCode::Conflict,
            "the lease allows a credential to go to another machine than the one asked for",
        ));
    }
    if !may_issue(false, lease.last_issue(), now) {
        return Err(refuse(
            ErrorCode::Conflict,
            "a copy was issued under this lease less than thirty seconds ago",
        )
        .retryable());
    }
    Ok(())
}

/// The credential sealed to the machine it goes to, as `provider.credential.issue` answers.
pub fn seal_issued(
    keys: &Keys,
    provider_id: Id,
    issued: &IssuedCredential,
    recipient: &MachineKey,
) -> Result<SealedCredential, CommandError> {
    let sealed = keys.seal_with(
        &recipient.public_key,
        &exchange_info(Direction::Issued, provider_id, &issued.form),
        issued.credential.expose(),
    )?;
    Ok(SealedCredential {
        form: issued.form.clone(),
        credential: Sealed { value: sealed },
        expires_at: issued.expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_pki::certificates::LeafKey;
    use orch_types::new_id;

    fn store() -> (SecretStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(Keys::new(LeafKey::generate().unwrap()));
        (SecretStore::open(dir.path(), keys).unwrap(), dir)
    }

    #[test]
    fn a_value_is_kept_sealed_and_read_back_and_the_file_does_not_hold_it() {
        let (store, dir) = store();
        store.put("provider-1", "sk-secret-key").unwrap();
        assert_eq!(
            store.get("provider-1").unwrap().as_deref(),
            Some("sk-secret-key")
        );
        let on_disk = std::fs::read_to_string(dir.path().join("secrets/provider-1")).unwrap();
        assert!(!on_disk.contains("sk-secret-key"));
        assert_eq!(store.get("absent").unwrap(), None);
        store.remove("provider-1").unwrap();
        assert_eq!(store.get("provider-1").unwrap(), None);
        store.remove("provider-1").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_store_and_its_files_are_for_the_owner_alone() {
        use std::os::unix::fs::PermissionsExt;
        let (store, dir) = store();
        store.put("provider-1", "sk-secret-key").unwrap();
        let mode = |path: std::path::PathBuf| {
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode(dir.path().join("secrets")), 0o700);
        assert_eq!(mode(dir.path().join("secrets/provider-1")), 0o600);
    }

    #[test]
    fn a_file_moved_to_another_name_or_read_with_another_key_does_not_open() {
        let (store, dir) = store();
        store.put("provider-1", "sk-secret-key").unwrap();
        std::fs::copy(
            dir.path().join("secrets/provider-1"),
            dir.path().join("secrets/provider-2"),
        )
        .unwrap();
        assert!(matches!(
            store.get("provider-2"),
            Err(StoreError::Sealed { .. })
        ));
        let other = SecretStore::open(
            dir.path(),
            Arc::new(Keys::new(LeafKey::generate().unwrap())),
        )
        .unwrap();
        assert!(matches!(
            other.get("provider-1"),
            Err(StoreError::Sealed { .. })
        ));
    }

    #[test]
    fn only_plain_names_are_kept() {
        let (store, _dir) = store();
        for bad in ["", "../x", "a/b", ".hidden", "with space"] {
            assert!(
                matches!(store.put(bad, "x"), Err(StoreError::Name(_))),
                "{bad:?}"
            );
        }
        assert!(store.put(&local_key_name(new_id()), "x").is_ok());
    }

    #[test]
    fn the_records_of_providers_are_told_from_the_keys_a_person_gave() {
        let (store, _dir) = store();
        let (kept, keyed) = (new_id(), new_id());
        store.put(&provider_record_name(kept), "{}").unwrap();
        store.put(&local_key_name(keyed), "sk").unwrap();
        store.put("other", "x").unwrap();
        assert_eq!(provider_ids(&store.names().unwrap()), [kept]);
    }

    #[test]
    fn a_credential_that_went_out_does_not_open_as_one_that_came_back() {
        let provider = new_id();
        assert_ne!(
            exchange_info(Direction::Issued, provider, "access_token"),
            exchange_info(Direction::Returned, provider, "access_token")
        );
        assert_ne!(
            exchange_info(Direction::Issued, provider, "access_token"),
            exchange_info(Direction::Issued, new_id(), "access_token")
        );
        assert_ne!(
            exchange_info(Direction::Issued, provider, "access_token"),
            exchange_info(Direction::Issued, provider, "oauth_token")
        );
    }
}
