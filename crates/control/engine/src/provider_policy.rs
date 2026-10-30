//! Provider address policies checked at binding, switch destinations and every turn preflight.

use crate::error::EngineError;
use orch_agent_contract::discovery::ModelPattern;
use orch_db::Conn;
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::provenance::{Part, Source};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Json};

/// The provider and the contributing scope whose patterns refuse its current Orch address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The selected provider.
    pub provider: Id,
    /// Its current Orch address, never an upstream URL.
    pub address: String,
    /// The first controlling scope, in hierarchy order.
    pub source: Source<Id>,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "provider {} is not allowed by {}",
            self.address,
            self.source.scope.as_str()
        )?;
        if let Some(node) = self.source.node {
            write!(f, " {node}")?;
        }
        Ok(())
    }
}

fn refusing_part<'a>(parts: &'a [Part<Id>], address: &str) -> Option<&'a Part<Id>> {
    parts.iter().find(|part| {
        part.value.as_array().is_some_and(|patterns| {
            !patterns
                .iter()
                .filter_map(Json::as_str)
                .any(|pattern| ModelPattern::new(pattern).matches(address))
        })
    })
}

/// The first contributing scope whose patterns refuse an address, for paged choices sharing a snapshot.
#[must_use]
pub fn refusing_scope(parts: &[Part<Id>], address: &str) -> Option<Source<Id>> {
    refusing_part(parts, address).map(|part| part.source.clone())
}

/// Whether a contributing policy refuses the current address. Unset permits every otherwise usable
/// provider; an empty list permits none. Pattern sets are intersected by testing each contributing list,
/// so `team.*` together with `team.work` admits `team.work`.
pub async fn forbidden(
    conn: &mut Conn,
    place: Id,
    provider: Id,
) -> Result<Option<Refused>, EngineError> {
    let def = registry::get(keys::PROVIDERS_ALLOWED)
        .ok_or_else(|| EngineError::unreadable("the provider address policy is not registered"))?;
    let snapshot = Snapshot::load(conn, Place::Node(place), Wanted::Keys(&[def])).await?;
    let policy = snapshot.resolve(def);
    if policy.parts.is_empty() {
        return Ok(None);
    }
    let address = orch_authz::resources::addresses(conn, &[provider])
        .await?
        .into_iter()
        .find(|(id, _)| *id == provider)
        .map(|(_, address)| address)
        .ok_or_else(|| {
            EngineError::unreadable("the selected provider has no current Orch address")
        })?;
    Ok(refusing_part(&policy.parts, &address).map(|part| Refused {
        provider,
        address: address.clone(),
        source: part.source.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_settings_contract::provenance::SourceScope;
    use serde_json::json;

    #[test]
    fn overlapping_patterns_admit_an_address_and_the_first_refusing_scope_is_named() {
        let root = orch_types::new_id();
        let project = orch_types::new_id();
        let mut parts = vec![
            Part {
                source: Source {
                    scope: SourceScope::Org,
                    node: Some(root),
                },
                value: json!(["acme.*"]),
            },
            Part {
                source: Source {
                    scope: SourceScope::Project,
                    node: Some(project),
                },
                value: json!(["acme.work"]),
            },
        ];
        assert!(refusing_part(&parts, "acme.work").is_none());
        assert_eq!(
            refusing_part(&parts, "acme.other").unwrap().source.node,
            Some(project)
        );
        assert_eq!(
            refusing_part(&parts, "system.work").unwrap().source.node,
            Some(root)
        );
        parts[1].value = json!([]);
        assert_eq!(
            refusing_part(&parts, "acme.work").unwrap().source.node,
            Some(project)
        );
        assert!(refusing_part(&[], "anywhere.work").is_none());
    }
}
