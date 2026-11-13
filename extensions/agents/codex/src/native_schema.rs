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
        // The app-server protocol of every build at or above the oldest proven one is taken to be the one published for
        // 0.167.2 (the same bytes as for 0.167.0) until the program itself answers otherwise at the use of an act.
        _ if crate::descriptor::is_pinned_build(version) => Some(CURRENT.get_or_init(|| {
            serde_json::from_str(include_str!("../schema/0.167.2/app-server.schema.json"))
                .unwrap_or(Json::Null)
        })),
        _ => None,
    }
}

fn experimental(version: &str) -> Option<&'static Json> {
    static CURRENT: OnceLock<Json> = OnceLock::new();
    let observed = crate::descriptor::is_pinned_build(version).then(|| {
        CURRENT.get_or_init(|| {
            serde_json::from_str(include_str!("../schema/0.167.2/experimental-controls.json"))
                .unwrap_or(Json::Null)
        })
    })?;
    (observed
        .get("version")
        .and_then(Json::as_str)
        .is_some_and(crate::descriptor::is_pinned_build)
        && observed.get("experimental_api").and_then(Json::as_bool) == Some(true))
    .then_some(observed)
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
        || experimental(version).is_some_and(terminal_control)
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
        pinned_background_control(version)
            || self.additional.get(version).is_some_and(terminal_control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_the_exact_observed_build_or_a_verified_schema_can_offer_terminal_end() {
        let schemas = NativeSchemas::default();
        for version in ["0.165.0", "0.166.9"] {
            assert!(!schemas.background_control(version));
        }
        for version in [
            "0.167.0", "0.167.2", "0.167.3", "0.168.0", "0.170.0", "1.0.0",
        ] {
            assert!(schemas.background_control(version), "{version}");
        }
        assert!(
            !pinned("0.167.2").is_some_and(terminal_control),
            "the published stable schema remains unchanged"
        );
        assert!(experimental("0.165.0").is_none());
        let list = json!({"definitions":{"ClientRequest":{"oneOf":[{"properties":{"method":{"enum":["thread/backgroundTerminals/list"]}}}]}}});
        assert!(!terminal_control(&list));
        let both = json!({"definitions":{"ClientRequest":{"oneOf":[{"properties":{"method":{"enum":["thread/backgroundTerminals/list","thread/backgroundTerminals/terminate"]}}}]}}});
        assert!(terminal_control(&both));
        let mut schemas = schemas;
        assert!(schemas.insert("0.167.2".into(), both.clone()).is_err());
        assert!(schemas.insert("0.170.0".into(), both.clone()).is_err());
        assert!(schemas.insert("0.166.0".into(), both).is_ok());
        assert!(schemas.background_control("0.166.0"));
        assert!(!schemas.background_control("0.166.1"));
    }
}
