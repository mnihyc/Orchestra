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
    overlay(&shape, &mut given, "ORCH", &variable)?;
    let config: T = given
        .try_into()
        .map_err(|error: toml::de::Error| parse(error.to_string()))?;
    config.validate()?;
    Ok(config)
}

fn overlay(
    shape: &toml::Table,
    given: &mut toml::Table,
    prefix: &str,
    variable: &impl Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    for (key, example) in shape {
        let name = format!("{prefix}_{}", key.to_uppercase());
        if let toml::Value::Table(inner) = example {
            let slot = given
                .entry(key.clone())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let toml::Value::Table(slot) = slot {
                overlay(inner, slot, &name, variable)?;
            }
            continue;
        }
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServerConfig;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        let config: ServerConfig = load_text("", "test", none).unwrap();
        assert_eq!(config, ServerConfig::default());
        assert_eq!(config.auth.login_failure_limit, 8);
        assert_eq!(config.database.slow_statement_ms, 1_000);
        assert_eq!(config.database.slow_acquire_ms, 1_000);
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
    }
}
