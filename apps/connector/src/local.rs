//! The commands that only read or change what is in the data directory: `status`, `policy`, `env` and
//! `provider set-key`.
//!
//! They work on the files and need no session, so they work with the connector stopped. A connector that
//! is running reads its limits when it starts, when the system hands it a change of a key it was given, and
//! within seconds of a change of its file: a limit changed here is in force at once.

use std::path::Path;

use anyhow::Context;
use orch_connector_core::enrollment::{DataDir, roles_of};
use orch_connector_core::policy::PolicyStore;
use orch_link_contract::enrollment::Trust;
use orch_link_contract::policy::{ALL_KEYS, PolicyKey};
use orch_types::{Id, Json, Time};

use crate::arguments::{Env, Policy};

/// Opens a data directory that holds an identity, or says what to do. A directory that does not is
/// left as it was found: a command that only looks makes nothing.
fn enrolled(dir: &Path) -> anyhow::Result<DataDir> {
    anyhow::ensure!(
        DataDir::enrolled_at(dir),
        "{} holds no identity: enroll first, with `orch-connector enroll --url <address> --token <token>`",
        dir.display()
    );
    DataDir::open(dir).with_context(|| format!("opening {}", dir.display()))
}

/// The moment two thirds of a certificate's life have passed, which is when it is renewed.
#[must_use]
pub fn renewal_from(not_before: Time, not_after: Time) -> Time {
    const PARTS_PASSED: i64 = 2;
    const PARTS_IN_ALL: i64 = 3;
    let whole = not_after.unix_millis() - not_before.unix_millis();
    Time::new(
        not_before.get() + chrono::Duration::milliseconds(whole / PARTS_IN_ALL * PARTS_PASSED),
    )
}

/// What the owner of the machine sees of it, as lines.
pub fn status(dir: &Path, adapters: &[&str]) -> anyhow::Result<String> {
    let data = enrolled(dir)?;
    let record = data
        .record()
        .context("reading the record of the enrollment")?;
    let identity = data.identity().context("reading the identity")?;
    let config = data
        .config(|name| std::env::var(name).ok())
        .context("reading config.toml")?;
    let mut lines = vec![
        format!("Machine      {} ({})", record.name, record.machine_id),
        format!(
            "System       {} ({}), enrolled {}{}",
            record.system.name,
            record.system.public_url,
            record.enrolled_at,
            if record.managed {
                ", made by the system"
            } else {
                ""
            }
        ),
        format!("Data         {}", dir.display()),
        format!(
            "Certificate  valid until {}, renewed from {}",
            identity.certificate.not_after(),
            renewal_from(
                identity.certificate.not_before(),
                identity.certificate.not_after()
            )
        ),
    ];
    let endpoints = data.endpoints(&config)?;
    lines.push(format!(
        "Gateway      {}",
        if endpoints.is_empty() {
            "(none)".to_owned()
        } else {
            endpoints
                .iter()
                .map(|e| format!("{} ({})", e.url, trust_name(e.trust)))
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    lines.push(format!(
        "Roles        {}",
        roles_of(&config)
            .iter()
            .map(orch_connector_core::enrollment::role_name)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    lines.push(format!("Adapters     {}", adapters.join(", ")));
    lines.push(format!(
        "Harnesses    {}",
        if config.agent.harnesses.is_empty() {
            "none named; `policy set agent.harnesses '[{\"kind\":\"…\",\"path\":\"…\"}]'` names them".to_owned()
        } else {
            config
                .agent
                .harnesses
                .iter()
                .map(|h| format!("{} at {}", h.kind, h.path))
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    lines.push("Session      known to a running connector only; asking it needs the console, which is not built yet".to_owned());
    lines.push(String::new());
    lines.push(policy_lines(&PolicyStore::connected(config, None)?).join("\n"));
    Ok(lines.join("\n"))
}

/// How an endpoint is trusted, as the configuration writes it.
const fn trust_name(trust: Trust) -> &'static str {
    match trust {
        Trust::Pinned => "pinned",
        Trust::Public => "public",
    }
}

/// Every key of the policy with its value, and a mark on those handed to the system.
fn policy_lines(store: &PolicyStore) -> Vec<String> {
    store.describe()
}

/// What a person typed for a value, as the policy reads it: JSON where it is JSON; for a key that holds
/// a list, a plain word or words separated by commas; otherwise the text itself.
#[must_use]
pub fn value_of(text: &str, current: &Json) -> Json {
    orch_connector_core::policy::value_from_text(text, current)
}

/// A locally kept machine fact, deliberately outside the delegable policy catalogue.
pub fn dedicated(dir: &Path, value: bool) -> anyhow::Result<String> {
    let data = enrolled(dir)?;
    data.update_config(|config| config.connector.dedicated = value)?;
    Ok(if value {
        "Isolated: this machine is dedicated to Orch"
    } else {
        "This machine is not dedicated to Orch; its directories use the harness sandbox"
    }
    .into())
}

/// The local operator may set roles independently of handover; the next session reports them.
pub fn roles(dir: &Path, command: &crate::arguments::Roles) -> anyhow::Result<String> {
    let data = enrolled(dir)?;
    match command {
        crate::arguments::Roles::Show => Ok(data.config(|_| None)?.connector.roles.join(",")),
        crate::arguments::Roles::Set { roles } => {
            let selected = crate::enrollment_options::connector_roles(&roles.join(","))?;
            let names = selected
                .iter()
                .map(orch_connector_core::enrollment::role_name)
                .collect::<Vec<_>>();
            data.update_config(|config| config.connector.roles = names.clone())?;
            Ok(format!(
                "Roles saved: {}. Restart the connector to activate them; stop and start its per-user service, or restart the hand-started program.",
                names.join(",")
            ))
        }
    }
}

/// `policy`: shows or changes the limits in the file.
pub fn policy(dir: &Path, command: &Policy) -> anyhow::Result<String> {
    let data = enrolled(dir)?;
    // The file's own contents: what the environment overrides is not to be written into it.
    let config = data.config(|_| None).context("reading config.toml")?;
    let store = PolicyStore::connected(config, Some(data.config_path()))?;
    if store.is_managed() {
        anyhow::bail!("this machine has no limits of its own");
    }
    match command {
        Policy::Show => {}
        Policy::Set { key, value } => {
            let key: PolicyKey = key.parse().map_err(|error| anyhow::anyhow!("{error}"))?;
            let current = store.policy().map_or(Json::Null, |policy| policy.get(key));
            store.set_here(key, &value_of(value, &current))?;
        }
        Policy::Delegate { key } => store.delegate(&[keys_of(key)])?,
        Policy::Undelegate { key } => store.undelegate(&[keys_of(key)])?,
    }
    Ok(policy_lines(&store).join("\n"))
}

/// `env add-root`, `remove-root` and `list-roots`: the directories environments may be made in, which is the
/// key `environment.roots` of the owner's limits. A directory is added as its absolute path, and only where it
/// is one; one that is gone can still be removed as it was written. The answer is the list as it stands.
pub fn env_roots(dir: &Path, command: &Env) -> anyhow::Result<String> {
    let data = enrolled(dir)?;
    // The file's own contents: what the environment overrides is not to be written into it.
    let config = data.config(|_| None).context("reading config.toml")?;
    let store = PolicyStore::connected(config, Some(data.config_path()))?;
    let Some(policy) = store.policy() else {
        anyhow::bail!("this machine has no limits of its own");
    };
    let mut roots = policy.environment_roots;
    match command {
        Env::ListRoots => {}
        Env::AddRoot { dir: root } => {
            let absolute = std::fs::canonicalize(root)
                .with_context(|| format!("{} is no directory here", root.display()))?;
            anyhow::ensure!(absolute.is_dir(), "{} is no directory", absolute.display());
            let text = absolute.to_string_lossy().into_owned();
            if !roots.contains(&text) {
                roots.push(text);
                store.set_here(PolicyKey::EnvironmentRoots, &Json::from(roots.clone()))?;
            }
        }
        Env::RemoveRoot { dir: root } => {
            let written = root.to_string_lossy().into_owned();
            let absolute = std::fs::canonicalize(root)
                .ok()
                .map(|path| path.to_string_lossy().into_owned());
            let before = roots.len();
            roots.retain(|each| *each != written && Some(each) != absolute.as_ref());
            anyhow::ensure!(
                roots.len() < before,
                "{} is not one of the directories environments may be made in",
                root.display()
            );
            store.set_here(PolicyKey::EnvironmentRoots, &Json::from(roots.clone()))?;
        }
    }
    Ok(if roots.is_empty() {
        "No directory: no environment can be made on this machine until one is added \
         (`orch-connector env add-root <dir>`)."
            .to_owned()
    } else {
        roots.join("\n")
    })
}

/// The most a key may have of bytes: what the system itself takes of a secret, so that a key kept here
/// is one the system could have been given.
const KEY_MAX_BYTES: usize = 65_536;

/// `provider set-key`: keeps the key read from `input` for the provider `provider`, sealed to this machine
/// under the name the provider role looks for, in the sealed store of the data directory. Nothing is sent
/// anywhere, and the key is not shown: a provider started here without a key (the key is kept on this
/// machine) finds it there when a request needs it. A key kept before is replaced.
pub fn set_key(
    dir: &Path,
    provider: &str,
    mut input: impl std::io::Read,
) -> anyhow::Result<String> {
    use orch_connector_core::credentials::{SecretStore, local_key_name};
    use orch_connector_core::roles::Keys;
    use std::io::Read;
    use std::sync::Arc;

    let provider: Id = provider.trim().parse().map_err(|_| {
        anyhow::anyhow!("{provider:?} is not a provider's identifier; the system shows it on the provider's page")
    })?;
    let data = enrolled(dir)?;
    let mut text = String::new();
    input
        .by_ref()
        .take(u64::try_from(KEY_MAX_BYTES).unwrap_or(u64::MAX) + 1)
        .read_to_string(&mut text)
        .context("reading the key from standard input")?;
    // A newline that a pipe or a shell adds after the key is no part of it.
    let key = text.trim_end_matches(['\r', '\n']);
    anyhow::ensure!(
        !key.is_empty(),
        "no key was given: pipe it in, such as `printf '%s' \"$KEY\" | orch-connector provider set-key --provider {provider}`"
    );
    anyhow::ensure!(
        key.len() <= KEY_MAX_BYTES,
        "a key has at most {KEY_MAX_BYTES} bytes"
    );
    let identity = data.identity().context("reading the identity")?;
    let store = SecretStore::open(dir, Arc::new(Keys::new(identity.key)))
        .context("opening the store of keys")?;
    store
        .put(&local_key_name(provider), key)
        .context("keeping the key")?;
    Ok(format!(
        "The key of the provider {provider} is kept on this machine, sealed to it. It is not sent to the system."
    ))
}

/// The entry of `policy.delegated` a word stands for: `all` is every key.
fn keys_of(word: &str) -> String {
    if word == "all" {
        ALL_KEYS.to_owned()
    } else {
        word.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_value_is_json_where_it_is_json_a_list_of_words_for_a_list_and_a_text_otherwise() {
        let list = json!([]);
        let flag = json!(true);
        let text = json!("workspace");
        assert_eq!(value_of("true", &flag), json!(true));
        assert_eq!(value_of("[\"/a\",\"/b\"]", &list), json!(["/a", "/b"]));
        assert_eq!(value_of("/a, /b,", &list), json!(["/a", "/b"]));
        assert_eq!(value_of("\"/a\"", &list), json!(["/a"]));
        assert_eq!(value_of("open", &text), json!("open"));
        assert_eq!(value_of("\"open\"", &text), json!("open"));
    }

    #[test]
    fn the_renewal_begins_two_thirds_into_the_life() {
        let from = renewal_from(
            "2026-01-01T00:00:00.000Z".parse().unwrap(),
            "2026-01-31T00:00:00.000Z".parse().unwrap(),
        );
        assert_eq!(from.to_string(), "2026-01-21T00:00:00.000Z");
    }

    #[test]
    fn local_roles_change_the_saved_configuration_but_cannot_turn_this_program_into_a_node() {
        let directory = tempfile::tempdir().unwrap();
        let identity = directory.path().join("identity");
        std::fs::create_dir_all(&identity).unwrap();
        std::fs::write(identity.join(orch_pki::enrollment::KEY_FILE), b"").unwrap();
        std::fs::write(
            identity.join(orch_connector_core::enrollment::RECORD_FILE),
            b"",
        )
        .unwrap();
        let said = roles(
            directory.path(),
            &crate::arguments::Roles::Set {
                roles: vec!["environment".into(), "provider".into()],
            },
        )
        .unwrap();
        assert!(said.contains("Restart the connector"));
        let data = DataDir::open(directory.path()).unwrap();
        assert_eq!(
            data.config(|_| None).unwrap().connector.roles,
            ["environment", "provider"]
        );
        let before = std::fs::read(data.config_path()).unwrap();
        assert_eq!(
            roles(
                directory.path(),
                &crate::arguments::Roles::Set {
                    roles: vec!["node".into()]
                }
            )
            .unwrap_err()
            .to_string(),
            orch_link_contract::enrollment_choices::CONNECTOR_NODE_REFUSAL
        );
        assert_eq!(
            std::fs::read(data.config_path()).unwrap(),
            before,
            "the refused role leaves consent unchanged"
        );
    }

    #[test]
    fn a_directory_without_an_identity_says_what_to_do_and_is_left_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let error = status(dir.path(), &[]).unwrap_err().to_string();
        assert!(error.contains("orch-connector enroll"), "{error}");
        let error = policy(dir.path(), &Policy::Show).unwrap_err().to_string();
        assert!(error.contains("holds no identity"), "{error}");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "nothing was made in it"
        );
    }

    #[test]
    fn a_root_is_added_as_its_absolute_path_once_and_removed_as_written() {
        let data = tempfile::tempdir().unwrap();
        // What `enrolled` looks for; the commands below read and write the configuration file alone.
        let identity = data.path().join("identity");
        std::fs::create_dir_all(&identity).unwrap();
        std::fs::write(identity.join(orch_pki::enrollment::KEY_FILE), b"").unwrap();
        std::fs::write(
            identity.join(orch_connector_core::enrollment::RECORD_FILE),
            b"",
        )
        .unwrap();
        let work = tempfile::tempdir().unwrap();
        let absolute = std::fs::canonicalize(work.path()).unwrap();
        let written = absolute.to_string_lossy().into_owned();
        let roots = |command: &Env| env_roots(data.path(), command).unwrap();

        let listed = roots(&Env::AddRoot {
            dir: work.path().join("."),
        });
        assert!(listed.lines().any(|line| line == written), "{listed}");
        let again = roots(&Env::AddRoot {
            dir: work.path().to_owned(),
        });
        assert_eq!(
            again.lines().filter(|line| *line == written).count(),
            1,
            "added once: {again}"
        );
        assert!(
            env_roots(
                data.path(),
                &Env::AddRoot {
                    dir: work.path().join("absent")
                }
            )
            .is_err(),
            "a directory that is not there is not added"
        );
        assert_eq!(roots(&Env::ListRoots), again);
        let left = roots(&Env::RemoveRoot {
            dir: absolute.clone(),
        });
        assert!(!left.lines().any(|line| line == written), "{left}");
        assert!(
            env_roots(data.path(), &Env::RemoveRoot { dir: absolute }).is_err(),
            "what is not listed cannot be removed"
        );
    }
}
