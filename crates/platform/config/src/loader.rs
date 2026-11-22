//! TOML loading, validation and environment overrides.
//!
//! A file is read as TOML, then every key of it may be replaced from the
//! environment: `ORCH_<SECTION>_<KEY>` in capitals, with the parts of a
//! nested section joined by underscores (`ORCH_NODE_DOCKER_SOCKET`). The
//! variable is read as the type the key has: a number, `true` or `false`,
//! a text as it is, or a list written as TOML (`["a", "b"]`). An unknown
//! key is an error, so that a misspelt one is never silently ignored.

use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

/// Why a configuration could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// What the system said.
        source: std::io::Error,
    },
    /// The text is not TOML, or holds a key nobody defined, or a value of
    /// the wrong type.
    #[error("{origin}: {message}")]
    Parse {
        /// The file, or where else the text came from.
        origin: String,
        /// What is wrong, with the line where it is known.
        message: String,
    },
    /// A variable of the environment does not fit its key.
    #[error("{variable}: {message}")]
    Variable {
        /// The variable.
        variable: String,
        /// What is wrong with its value.
        message: String,
    },
    /// The values do not make sense together.
    #[error("{key}: {message}")]
    Invalid {
        /// The key, as `section.key`.
        key: String,
        /// What is wrong.
        message: String,
    },
}

/// A configuration that can say whether its values make sense together.
pub trait Validate {
    /// Checks what the types cannot.
    fn validate(&self) -> Result<(), ConfigError>;
    /// Retain only the type's explicit public allowlist after validation.
    fn capture_origins(&mut self, _origins: crate::observed::Origins) {}
}

/// Loads the configuration at `path`, with the environment's overrides.
pub fn load<T>(path: &Path) -> Result<T, ConfigError>
where
    T: DeserializeOwned + Serialize + Default + Validate,
{
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_owned(),
        source,
    })?;
    load_text(&text, &path.display().to_string(), |name| {
        std::env::var(name).ok()
    })
}

/// Loads a configuration from `text`. `variable` answers for the
/// environment; `origin` names the text in errors.
pub fn load_text<T>(
    text: &str,
    origin: &str,
    variable: impl Fn(&str) -> Option<String>,
) -> Result<T, ConfigError>
where
    T: DeserializeOwned + Serialize + Default + Validate,
{
    let parse = |message: String| ConfigError::Parse {
        origin: origin.to_owned(),
        message,
    };
    let mut given: toml::Table = text
        .parse()
        .map_err(|error: toml::de::Error| parse(error.to_string()))?;
    // The defaults say which keys exist and of what type each is.
    let shape = toml::Table::try_from(T::default()).map_err(|error| parse(error.to_string()))?;
    let mut origins = crate::observed::Origins::new();
    overlay(&shape, &mut given, "ORCH", "", &variable, &mut origins)?;
    let mut config: T = given
        .try_into()
        .map_err(|error: toml::de::Error| parse(error.to_string()))?;
    config.validate()?;
    config.capture_origins(origins);
    Ok(config)
}

fn overlay(
    shape: &toml::Table,
    given: &mut toml::Table,
    prefix: &str,
    path: &str,
    variable: &impl Fn(&str) -> Option<String>,
    origins: &mut crate::observed::Origins,
) -> Result<(), ConfigError> {
    for (key, example) in shape {
        let name = format!("{prefix}_{}", key.to_uppercase());
        let public_key = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        if let toml::Value::Table(inner) = example {
            match given.get_mut(key) {
                Some(toml::Value::Table(section)) => {
                    overlay(inner, section, &name, &public_key, variable, origins)?;
                }
                None => {
                    // Preserve field defaults when neither the file nor the environment sets this section.
                    let mut overrides = toml::Table::new();
                    overlay(inner, &mut overrides, &name, &public_key, variable, origins)?;
                    if !overrides.is_empty() {
                        given.insert(key.clone(), toml::Value::Table(overrides));
                    }
                }
                Some(_) => {}
            }
            continue;
        }
        origins.insert(
            public_key.clone(),
            if given.contains_key(key) {
                crate::observed::Origin::File
            } else {
                crate::observed::Origin::Compiled
            },
        );
        let Some(text) = variable(&name) else {
            continue;
        };
        // Compose leaves this optional value empty when .env has no source. The configured TOML
        // source still serves both the updater and the read-only checks in that case.
        if name == "ORCH_RELEASE_URL" && text.is_empty() {
            continue;
        }
        let fault = |message: &str| ConfigError::Variable {
            variable: name.clone(),
            message: message.to_owned(),
        };
        let value = match example {
            toml::Value::String(_) => toml::Value::String(text),
            toml::Value::Integer(_) => toml::Value::Integer(
                text.trim()
                    .parse()
                    .map_err(|_| fault("must be a whole number"))?,
            ),
            toml::Value::Float(_) => {
                toml::Value::Float(text.trim().parse().map_err(|_| fault("must be a number"))?)
            }
            toml::Value::Boolean(_) => match text.trim() {
                "true" => toml::Value::Boolean(true),
                "false" => toml::Value::Boolean(false),
                _ => return Err(fault("must be true or false")),
            },
            _ => {
                let table: toml::Table = format!("v = {text}")
                    .parse()
                    .map_err(|_| fault("must be written as TOML, as [\"a\", \"b\"]"))?;
                table
                    .get("v")
                    .cloned()
                    .ok_or_else(|| fault("must be written as TOML"))?
            }
        };
        given.insert(key.clone(), value);
        origins.insert(public_key, crate::observed::Origin::Environment);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RuntimeComponent, ServerConfig};

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        let config: ServerConfig = load_text("", "test", none).unwrap();
        assert_eq!(
            toml::to_string(&config).unwrap(),
            toml::to_string(&ServerConfig::default()).unwrap()
        );
        assert!(
            config
                .startup_facts
                .as_ref()
                .unwrap()
                .values
                .iter()
                .all(|fact| fact.origin == Some(crate::observed::Origin::Compiled))
        );
        assert_eq!(config.auth.login_failure_limit, 8);
        assert_eq!(config.database.slow_statement_ms, 1_000);
        assert_eq!(config.database.slow_acquire_ms, 1_000);
    }

    #[test]
    fn a_connector_can_enroll_without_a_platform_review_in_its_configuration() {
        use crate::ConnectorConfig;
        use orch_agent_programs::confirmation::PlatformConsent;

        for text in ["", "[exec]\noffers = []\n"] {
            let config: ConnectorConfig = load_text(text, "connector", none).unwrap();
            assert_eq!(config.exec.platform, PlatformConsent::Off);
        }
        // An explicitly present decision still needs its state and an approval needs its build.
        assert!(load_text::<ConnectorConfig>("[exec.platform]\n", "connector", none).is_err());
        assert!(
            load_text::<ConnectorConfig>("", "connector", |key| {
                (key == "ORCH_EXEC_PLATFORM_STATE").then(|| "approved".into())
            })
            .is_err()
        );
        let config: ConnectorConfig = load_text("", "connector", |key| {
            (key == "ORCH_EXEC_PLATFORM_STATE").then(|| "off".into())
        })
        .unwrap();
        assert_eq!(config.exec.platform, PlatformConsent::Off);
    }

    #[test]
    fn runtime_worker_stacks_default_to_64_mib_and_accept_smaller_and_larger_overrides() {
        let components = [
            RuntimeComponent::Api,
            RuntimeComponent::Gateway,
            RuntimeComponent::Worker,
            RuntimeComponent::Node,
        ];
        let defaults: ServerConfig = load_text("", "test", none).unwrap();
        for component in components {
            assert_eq!(
                defaults.worker_stack_bytes(component).unwrap(),
                64 * 1024 * 1024
            );
        }
        let configured: ServerConfig = load_text(
            "[api]\nworker_stack_mib = 2\n[gateway]\nworker_stack_mib = 96\n\
             [worker]\nworker_stack_mib = 8\n[node]\nworker_stack_mib = 128\n",
            "test",
            none,
        )
        .unwrap();
        let environment: ServerConfig = load_text("", "test", |name| {
            match name {
                "ORCH_API_WORKER_STACK_MIB" => Some("4"),
                "ORCH_GATEWAY_WORKER_STACK_MIB" => Some("192"),
                "ORCH_WORKER_WORKER_STACK_MIB" => Some("3"),
                "ORCH_NODE_WORKER_STACK_MIB" => Some("256"),
                _ => None,
            }
            .map(str::to_owned)
        })
        .unwrap();
        for ((component, file_mib), env_mib) in components
            .into_iter()
            .zip([2_usize, 96, 8, 128])
            .zip([4_usize, 192, 3, 256])
        {
            assert_eq!(
                configured.worker_stack_bytes(component).unwrap(),
                file_mib * 1024 * 1024
            );
            assert_eq!(
                environment.worker_stack_bytes(component).unwrap(),
                env_mib * 1024 * 1024
            );
        }
    }

    #[test]
    fn runtime_worker_stack_zero_and_byte_overflow_are_named_before_start() {
        let overflow_mib = usize::MAX / (1024 * 1024) + 1;
        for section in ["api", "gateway", "worker", "node"] {
            for mib in [0, overflow_mib] {
                let error = load_text::<ServerConfig>(
                    &format!("[{section}]\nworker_stack_mib = {mib}\n"),
                    "test",
                    none,
                )
                .unwrap_err();
                assert!(
                    matches!(&error, ConfigError::Invalid { key, .. } if key == &format!("{section}.worker_stack_mib")),
                    "{error}"
                );
            }
        }
    }

    #[test]
    fn the_database_watch_uses_milliseconds_from_the_file_and_environment() {
        let config: ServerConfig = load_text(
            "[database]\nslow_statement_ms = 25\nslow_acquire_ms = 50\n",
            "test",
            |name| (name == "ORCH_DATABASE_SLOW_ACQUIRE_MS").then(|| "75".to_owned()),
        )
        .unwrap();
        assert_eq!(config.database.slow_statement_ms, 25);
        assert_eq!(config.database.slow_acquire_ms, 75);
    }

    #[test]
    fn startup_facts_keep_the_source_even_when_the_value_equals_the_compiled_default() {
        let config: ServerConfig = load_text(
            "[database]\nacquire_timeout_ms=10000\n",
            "orch.toml",
            |name| (name == "ORCH_DATABASE_REQUEST_STATEMENT_TIMEOUT_MS").then(|| "15000".into()),
        )
        .unwrap();
        let facts = config.startup_facts.as_ref().unwrap();
        let fact = |key| facts.values.iter().find(|fact| fact.key == key).unwrap();
        assert_eq!(
            fact("database.acquire_timeout_ms").origin,
            Some(crate::observed::Origin::File)
        );
        assert_eq!(
            fact("database.request_statement_timeout_ms").origin,
            Some(crate::observed::Origin::Environment)
        );
        assert_eq!(
            fact("database.idle_transaction_timeout_ms").origin,
            Some(crate::observed::Origin::Compiled)
        );
        assert_eq!(facts.values.len(), 4);
        assert!(
            facts
                .values
                .iter()
                .all(|value| !value.key.contains("url") && !value.key.contains("secret"))
        );
        let serialized = toml::to_string(&config).unwrap();
        assert!(!serialized.contains("startup_facts"));
    }

    #[test]
    fn startup_facts_are_frozen_before_later_configuration_edits() {
        let mut config: ServerConfig =
            load_text("[database]\nacquire_timeout_ms=30000\n", "orch.toml", none).unwrap();
        let original = config.startup_facts.clone().unwrap();
        config.database.acquire_timeout_ms = 90000;
        assert_eq!(config.startup_facts.as_ref().unwrap(), &original);
        assert_eq!(
            original
                .values
                .iter()
                .find(|fact| fact.key == "database.acquire_timeout_ms")
                .unwrap()
                .milliseconds,
            30000
        );
        assert!(ServerConfig::default().startup_facts.is_none());
    }

    #[test]
    fn database_deadlines_are_startup_defaults_replaced_by_file_and_environment() {
        let text = "[database]\nacquire_timeout_ms=30000\nrequest_statement_timeout_ms=60000\nbackground_statement_timeout_ms=90000\nidle_transaction_timeout_ms=45000\n";
        let file: ServerConfig = load_text(text, "orch.toml", none).unwrap();
        let written = toml::Value::try_from(file).unwrap();
        assert_eq!(
            written["database"]["acquire_timeout_ms"].as_integer(),
            Some(30000)
        );
        assert_eq!(
            written["database"]["request_statement_timeout_ms"].as_integer(),
            Some(60000)
        );
        let environment: ServerConfig = load_text(text, "orch.toml", |name| match name {
            "ORCH_DATABASE_ACQUIRE_TIMEOUT_MS" => Some("70000".into()),
            "ORCH_DATABASE_REQUEST_STATEMENT_TIMEOUT_MS" => Some("0".into()),
            "ORCH_DATABASE_IDLE_TRANSACTION_TIMEOUT_MS" => Some("0".into()),
            _ => None,
        })
        .unwrap();
        let written = toml::Value::try_from(environment).unwrap();
        assert_eq!(
            written["database"]["acquire_timeout_ms"].as_integer(),
            Some(70000)
        );
        assert_eq!(
            written["database"]["request_statement_timeout_ms"].as_integer(),
            Some(0)
        );
        assert_eq!(
            written["database"]["idle_transaction_timeout_ms"].as_integer(),
            Some(0)
        );
        assert_eq!(
            written["database"]["background_statement_timeout_ms"].as_integer(),
            Some(90000)
        );
    }

    #[test]
    fn database_deadlines_reject_only_an_unusable_wait_or_the_native_integer_overflow() {
        for text in [
            "[database]\nacquire_timeout_ms=0\n",
            "[database]\nrequest_statement_timeout_ms=2147483648\n",
            "[database]\nbackground_statement_timeout_ms=2147483648\n",
            "[database]\nidle_transaction_timeout_ms=2147483648\n",
        ] {
            assert!(
                load_text::<ServerConfig>(text, "orch.toml", none).is_err(),
                "{text}"
            );
        }
        assert!(load_text::<ServerConfig>("[database]\nacquire_timeout_ms=1000000\nrequest_statement_timeout_ms=2147483647\nbackground_statement_timeout_ms=0\nidle_transaction_timeout_ms=0\n", "orch.toml", none).is_ok());
    }

    #[test]
    fn a_key_nobody_defined_stops_the_start() {
        let error =
            load_text::<ServerConfig>("[auth]\nlogin_failure_limt = 3\n", "orch.toml", none)
                .unwrap_err();
        assert!(error.to_string().contains("login_failure_limt"), "{error}");
    }

    #[test]
    fn the_environment_replaces_a_key_of_the_file_by_its_type() {
        let variable = |name: &str| match name {
            "ORCH_AUTH_LOGIN_FAILURE_LIMIT" => Some("5".to_owned()),
            "ORCH_NODE_DOCKER_SOCKET" => Some("/run/user/1000/docker.sock".to_owned()),
            "ORCH_NODE_DRIVERS" => Some("[\"host\"]".to_owned()),
            "ORCH_METRICS_STORE" => Some("false".to_owned()),
            _ => None,
        };
        let config: ServerConfig =
            load_text("[auth]\nlogin_failure_limit = 3\n", "test", variable).unwrap();
        assert_eq!(config.auth.login_failure_limit, 5);
        assert_eq!(config.node.docker.socket, "/run/user/1000/docker.sock");
        assert_eq!(config.node.drivers, ["host"]);
        assert!(!config.metrics.store);
    }

    #[test]
    fn a_variable_of_the_wrong_type_is_named() {
        let variable = |name: &str| (name == "ORCH_API_DB_POOL").then(|| "many".to_owned());
        let error = load_text::<ServerConfig>("", "test", variable).unwrap_err();
        assert!(error.to_string().starts_with("ORCH_API_DB_POOL"), "{error}");
    }

    #[test]
    fn values_that_cannot_work_together_are_refused() {
        let error =
            load_text::<ServerConfig>("[link]\nheartbeat_secs = 90\n", "test", none).unwrap_err();
        assert!(
            error.to_string().contains("link.dead_after_secs"),
            "{error}"
        );
        let error = load_text::<ServerConfig>("[link]\nmax_message_bytes = 1000\n", "test", none)
            .unwrap_err();
        assert!(
            error.to_string().contains("link.max_message_bytes"),
            "{error}"
        );
        let config =
            load_text::<ServerConfig>("[link]\nmax_message_bytes = 1048576\n", "test", none)
                .unwrap();
        assert_eq!(config.link.max_message_bytes, 1_048_576);
        assert_eq!(
            load_text::<ServerConfig>("", "test", none)
                .unwrap()
                .link
                .max_message_bytes,
            16 * 1024 * 1024
        );
    }
}
