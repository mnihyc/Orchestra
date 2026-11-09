//! Native controls require method declarations from the same exact verified build.
use orch_types::Json;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub(crate) const END_UNAVAILABLE: &str = "This Codex version cannot end background terminals.";

fn pinned(version: &str) -> Option<&'static Json> {
    static OLDER: OnceLock<Json> = OnceLock::new();
    static CURRENT: OnceLock<Json> = OnceLock::new();
    match version {
        "0.165.0" => Some(OLDER.get_or_init(|| {
            serde_json::from_str(include_str!("../schema/0.165.0/app-server.schema.json"))
                .unwrap_or(Json::Null)
        })),
        "0.167.0" => Some(CURRENT.get_or_init(|| {
            serde_json::from_str(include_str!("../schema/0.167.0/app-server.schema.json"))
                .unwrap_or(Json::Null)
        })),
        _ => None,
    }
}

fn terminal_control(schema: &Json) -> bool {
    [
        "thread/backgroundTerminals/list",
        "thread/backgroundTerminals/terminate",
    ]
    .iter()
    .all(|method| {
        schema
            .pointer("/definitions/ClientRequest/oneOf")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .any(|variant| {
                variant
                    .pointer("/properties/method/enum")
                    .and_then(Json::as_array)
                    .is_some_and(|names| names.iter().any(|name| name.as_str() == Some(*method)))
            })
    })
}

pub(crate) fn pinned_background_control(version: &str) -> bool {
    pinned(version).is_some_and(terminal_control)
}

/// Trusted build metadata, never deserialized from an agent's request, provider key or profile.
#[derive(Debug, Default)]
pub(crate) struct NativeSchemas {
    additional: BTreeMap<String, Json>,
}
impl NativeSchemas {
    pub(crate) fn insert(&mut self, version: String, schema: Json) -> Result<(), String> {
        if semver::Version::parse(&version).is_err() {
            return Err("a native schema needs the exact native build version".into());
        }
        if pinned(&version).is_some() || self.additional.contains_key(&version) {
            return Err("an existing native build schema cannot be replaced".into());
        }
        if !schema
            .pointer("/definitions/ClientRequest/oneOf")
            .is_some_and(Json::is_array)
        {
            return Err("a native schema needs its declared client request methods".into());
        }
        self.additional.insert(version, schema);
        Ok(())
    }

    pub(crate) fn background_control(&self, version: &str) -> bool {
        pinned(version)
            .or_else(|| self.additional.get(version))
            .is_some_and(terminal_control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn both_pins_and_unknown_builds_have_no_terminal_end_without_both_schema_methods() {
        let schemas = NativeSchemas::default();
        for version in ["0.165.0", "0.167.0", "0.170.0"] {
            assert!(!schemas.background_control(version));
        }
        let list = json!({"definitions":{"ClientRequest":{"oneOf":[{"properties":{"method":{"enum":["thread/backgroundTerminals/list"]}}}]}}});
        assert!(!terminal_control(&list));
        let both = json!({"definitions":{"ClientRequest":{"oneOf":[{"properties":{"method":{"enum":["thread/backgroundTerminals/list","thread/backgroundTerminals/terminate"]}}}]}}});
        assert!(terminal_control(&both));
        let mut schemas = schemas;
        assert!(schemas.insert("0.167.0".into(), both.clone()).is_err());
        assert!(schemas.insert("0.170.0".into(), both).is_ok());
        assert!(schemas.background_control("0.170.0"));
        assert!(!schemas.background_control("0.170.1"));
    }
}
