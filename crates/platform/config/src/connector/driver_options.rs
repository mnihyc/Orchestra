//! Lossless JSON connection options in the machine policy file.

use orch_link_contract::policy::{MachineDrivers, PolicyKey, PolicyValues};
use serde::{Deserialize, Serialize, de::Error as _, ser::Error as _};
use serde_json::Value as Json;

fn object(value: Json) -> Result<Json, &'static str> {
    let value = match value {
        Json::String(text) => serde_json::from_str(&text)
            .map_err(|_| "driver connection settings contain invalid JSON")?,
        value => value,
    };
    if value.is_object() {
        Ok(value)
    } else {
        Err("driver connection settings are an object")
    }
}

/// Writes opaque options as JSON text because TOML cannot represent null.
pub(super) fn serialize_drivers<S: serde::Serializer>(
    drivers: &MachineDrivers,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut value = serde_json::to_value(drivers).map_err(S::Error::custom)?;
    if let Some(options) = value
        .get_mut("configurations")
        .and_then(Json::as_object_mut)
    {
        for option in options.values_mut() {
            *option = Json::String(serde_json::to_string(&*option).map_err(S::Error::custom)?);
        }
    }
    value.serialize(serializer)
}

/// Reads canonical JSON text and the earlier TOML tables without coercing their contents.
pub(super) fn deserialize_drivers<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<MachineDrivers, D::Error> {
    let mut value = Json::deserialize(deserializer)?;
    if let Some(options) = value
        .get_mut("configurations")
        .and_then(Json::as_object_mut)
    {
        for option in options.values_mut() {
            *option = object(std::mem::take(option)).map_err(D::Error::custom)?;
        }
    }
    serde_json::from_value(value).map_err(D::Error::custom)
}

/// Keeps a handed-over driver value just as lossless as a locally kept one.
pub(super) fn serialize_values<S: serde::Serializer>(
    values: &PolicyValues,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut values = values.clone();
    if let Some(value) = values.get_mut(PolicyKey::Drivers.as_str()) {
        *value = Json::String(serde_json::to_string(&*value).map_err(S::Error::custom)?);
    }
    values.serialize(serializer)
}

/// Restores the ordinary JSON policy value before its declared schema is checked.
pub(super) fn deserialize_values<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<PolicyValues, D::Error> {
    let mut values = PolicyValues::deserialize(deserializer)?;
    if let Some(value) = values.get_mut(PolicyKey::Drivers.as_str()) {
        *value = object(std::mem::take(value)).map_err(D::Error::custom)?;
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use crate::connector::ConnectorConfig;
    use serde_json::json;

    #[test]
    fn opaque_driver_options_keep_nulls_and_nested_arrays_when_a_machine_file_is_reloaded() {
        let options =
            json!({"unset":null,"items":[null,{"unset":null}],"text":"保留","enabled":true});
        let mut config = ConnectorConfig::default();
        config.drivers.allowed = vec!["cluster-test".into()];
        config
            .drivers
            .configurations
            .insert("cluster-test".into(), options.clone());
        config
            .policy
            .values
            .insert("drivers".into(), json!(config.drivers));
        let written = toml::to_string(&config).unwrap();
        let read: ConnectorConfig = toml::from_str(&written).unwrap();
        assert_eq!(read.drivers.configurations["cluster-test"], options);
        assert_eq!(
            read.policy.values["drivers"]["configurations"]["cluster-test"],
            options
        );
        assert_eq!(
            serde_json::to_value(&read.drivers).unwrap()["configurations"]["cluster-test"],
            options,
            "the wire policy still carries an object, not the file's encoded text"
        );
    }

    #[test]
    fn existing_driver_tables_keep_their_restrictive_rules_and_configuration() {
        let read: ConnectorConfig = toml::from_str("[drivers]\nallowed=['cluster-test']\n[drivers.docker]\nimages=['allowed:1']\nbuilds=false\n[drivers.configurations.cluster-test]\nendpoint='local'\n").unwrap();
        assert_eq!(read.drivers.images.images, ["allowed:1"]);
        assert!(!read.drivers.images.builds);
        assert_eq!(
            read.drivers.configurations["cluster-test"],
            json!({"endpoint":"local"})
        );
        let next: ConnectorConfig = toml::from_str(&toml::to_string(&read).unwrap()).unwrap();
        assert_eq!(next.drivers, read.drivers);
    }

    #[test]
    fn a_file_never_turns_a_non_object_or_invalid_json_into_default_driver_options() {
        for encoded in ["null", "[]", "false", "{broken"] {
            let text = format!("[drivers.configurations]\ncluster-test = '{encoded}'\n");
            assert!(
                toml::from_str::<ConnectorConfig>(&text).is_err(),
                "{encoded}"
            );
        }
    }
}
