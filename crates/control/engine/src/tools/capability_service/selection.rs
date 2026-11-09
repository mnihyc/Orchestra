//! One declaration-based backend choice; these facts grant neither Use nor native command authority.
use crate::EngineError;
use orch_db::Conn;
use orch_provider_contract::capabilities::{BackendRef, Declarations, OfferingRef};
use orch_provider_contract::services::SearchBackendChoice;
use orch_settings::Place;
use orch_settings_contract::registry;
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};

/// The exact stored opt-in and its canonical route, retained across preparation waits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub setting: String,
    pub value: Json,
    pub mode_setting: String,
    pub reference: BackendRef,
    pub executor: String,
}

/// Read a declared setting or a member of its existing map-valued root.
pub async fn setting(conn: &mut Conn, node: Id, key: &str) -> Result<Option<Json>, EngineError> {
    let Some(def) = registry::all()
        .iter()
        .filter(|def| {
            key == def.key
                || key
                    .strip_prefix(def.key)
                    .is_some_and(|tail| tail.starts_with('.'))
        })
        .max_by_key(|def| def.key.len())
    else {
        return Ok(None);
    };
    let value: Option<Json> = orch_settings::get(conn, Place::Node(node), def.key).await?;
    let Some(value) = value else { return Ok(None) };
    if key == def.key {
        return Ok((!value.is_null()).then_some(value));
    }
    let member = key
        .strip_prefix(def.key)
        .and_then(|tail| tail.strip_prefix('.'))
        .ok_or(EngineError::StaleToolSource)?;
    if let Some(found) = value.get(member) {
        return Ok((!found.is_null()).then(|| found.clone()));
    }
    let mut found = &value;
    for component in member.split('.') {
        let Some(next) = found.get(component) else {
            return Ok(None);
        };
        found = next;
    }
    Ok((!found.is_null()).then(|| found.clone()))
}

/// Adapt the original web backend shape at its feature boundary; all execution sees BackendRef.
fn web_reference(
    declarations: &Declarations,
    capability: &str,
    value: &Json,
) -> Option<BackendRef> {
    let choice: SearchBackendChoice = serde_json::from_value(value.clone()).ok()?;
    let (provider, offering, executor) = match &choice {
        SearchBackendChoice::Model {
            provider_id,
            model_name,
        } => (*provider_id, model_name.as_str(), "model-backed"),
        SearchBackendChoice::Api {
            provider_id,
            search_service,
            fetch_service,
        } => {
            let offering = match capability {
                "web_search" => search_service.as_str(),
                "page_fetch" => fetch_service.as_deref()?,
                _ => return None,
            };
            (*provider_id, offering, "api-backed")
        }
    };
    let definition = declarations.capabilities.get(capability)?;
    let route = definition
        .modes
        .get("service")?
        .routes
        .iter()
        .find(|route| {
            declarations
                .routes
                .get(*route)
                .is_some_and(|route| route.executor.as_deref() == Some(executor))
        })?;
    Some(BackendRef {
        route: route.clone(),
        offering: Some(OfferingRef {
            provider,
            offering: offering.into(),
        }),
        ..BackendRef::default()
    })
}

/// A future capability uses the same BackendRef setting and route declarations without a core branch.
pub fn from_value(declarations: &Declarations, capability: &str, value: Json) -> Option<Selection> {
    if value.is_null() {
        return None;
    }
    let definition = declarations.capabilities.get(capability)?;
    let setting = definition.backend_setting.as_ref()?;
    let reference = if value.get("route").is_some() {
        serde_json::from_value::<BackendRef>(value.clone()).ok()?
    } else {
        web_reference(declarations, capability, &value)?
    };
    let route = declarations.routes.get(&reference.route)?;
    if !definition
        .modes
        .get("service")?
        .routes
        .contains(&reference.route)
        || !route.capabilities.iter().any(|name| name == capability)
        || reference.offering.as_ref().is_none_or(|offering| {
            offering.provider.is_nil() || offering.offering.trim().is_empty()
        })
    {
        return None;
    }
    Some(Selection {
        setting: setting.clone(),
        value,
        mode_setting: definition.mode_setting.clone()?,
        reference,
        executor: route.executor.clone()?,
    })
}

pub async fn load(
    conn: &mut Conn,
    node: Id,
    declarations: &Declarations,
    capability: &str,
) -> Result<Option<Selection>, EngineError> {
    let Some(key) = declarations
        .capabilities
        .get(capability)
        .and_then(|capability| capability.backend_setting.as_deref())
    else {
        return Ok(None);
    };
    Ok(setting(conn, node, key)
        .await?
        .and_then(|value| from_value(declarations, capability, value)))
}

impl Selection {
    /// The stored backend and explicit mode are both current; this remains factual preparation only.
    pub async fn is_current(&self, conn: &mut Conn, node: Id) -> Result<bool, EngineError> {
        Ok(
            setting(conn, node, &self.setting).await?.as_ref() == Some(&self.value)
                && setting(conn, node, &self.mode_setting)
                    .await?
                    .as_ref()
                    .and_then(Json::as_str)
                    == Some("service"),
        )
    }
    pub fn offering(&self) -> Option<&OfferingRef> {
        self.reference.offering.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_provider_contract::capabilities::{Capability, Mode, Route};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn canonical_backends_keep_the_exact_route_and_a_new_capability_needs_no_selector_branch()
    -> Result<(), Box<dyn std::error::Error>> {
        let provider = orch_types::new_id();
        let mut declarations = Declarations {
            capabilities: BTreeMap::from([(
                "fixture-capability".into(),
                Capability {
                    backend_setting: Some("fixture.backend".into()),
                    mode_setting: Some("fixture.mode".into()),
                    modes: BTreeMap::from([(
                        "service".into(),
                        Mode {
                            routes: vec!["fixture-route".into()],
                            ..Mode::default()
                        },
                    )]),
                    ..Capability::default()
                },
            )]),
            routes: BTreeMap::from([(
                "fixture-route".into(),
                Route {
                    executor: Some("future-executor".into()),
                    capabilities: vec!["fixture-capability".into()],
                    ..Route::default()
                },
            )]),
            ..Declarations::default()
        };
        let selected=from_value(&declarations,"fixture-capability",json!({"route":"fixture-route","offering":{"provider":provider,"offering":"exact-offering"},"future_member":{"kept":true}})).ok_or("missing declared selection")?;
        assert_eq!(selected.executor, "future-executor");
        assert_eq!(
            selected
                .reference
                .offering
                .as_ref()
                .ok_or("missing offering")?
                .offering,
            "exact-offering"
        );
        assert_eq!(
            selected.reference.extensions["future_member"],
            json!({"kept":true})
        );
        assert!(from_value(&declarations, "fixture-capability", Json::Null).is_none());
        declarations
            .routes
            .get_mut("fixture-route")
            .ok_or("missing route")?
            .capabilities
            .clear();
        assert!(from_value(&declarations, "fixture-capability", selected.value).is_none());
        Ok(())
    }
}
