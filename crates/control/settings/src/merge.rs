//! Replace, union, intersect, map, append and minimum.
//!
//! How the values of several scopes combine is the settings contract's rule
//! for each of these modes, and it is not restated here. This is what feeds
//! that rule: the rows read for a chain of nodes, the own values of an
//! environment or of a provider's runtime, and a default that something other
//! than the registry supplies, put in layers from the top down. A row at a
//! scope that the key does not list counts for nothing, so that a registry
//! that narrows a key later does not give old rows a say.

use std::collections::HashMap;

use orch_db::queries::settings::SettingRow;
use orch_settings_contract::merge::{Layer, resolve};
use orch_settings_contract::provenance::Resolved;
use orch_settings_contract::registry::{Scope, SettingDef};
use orch_types::{Id, Json};

/// One node on the way from the root org down to the place that is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    /// The node.
    pub id: Id,
    /// The scope its rows stand at: the kind of the node.
    pub scope: Scope,
}

/// The stored rows read for a chain, found by node and key.
#[derive(Debug, Clone, Default)]
pub struct Rows {
    by_node: HashMap<Option<Id>, HashMap<String, SettingRow>>,
}

impl Rows {
    /// The rows given, which belong to the system (no node) and to the nodes
    /// of the chain.
    #[must_use]
    pub fn new(rows: Vec<SettingRow>) -> Self {
        let mut found = Self::default();
        for row in rows {
            found.put(row);
        }
        found
    }

    /// The row of `key` at `node`, or at the system when none.
    #[must_use]
    pub fn get(&self, node: Option<Id>, key: &str) -> Option<&SettingRow> {
        self.by_node.get(&node)?.get(key)
    }

    /// Puts a row, in place of the one there was.
    pub fn put(&mut self, row: SettingRow) {
        self.by_node
            .entry(row.node_id)
            .or_default()
            .insert(row.key.clone(), row);
    }

    /// Takes the row of `key` at `node` away.
    pub fn take(&mut self, node: Option<Id>, key: &str) -> Option<SettingRow> {
        self.by_node.get_mut(&node)?.remove(key)
    }
}

/// The own values of the thing a key is resolved for, when it is an
/// environment or a provider's runtime: the innermost scope.
#[derive(Debug, Clone)]
pub struct OwnValues {
    /// `environment` or `provider`.
    pub scope: Scope,
    /// An object: a value by key.
    pub values: Json,
    /// How many times the thing was changed.
    pub revision: i64,
}

impl OwnValues {
    /// The value of `key`, where there is one.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Json> {
        self.values.get(key)
    }
}

/// What stands in the registry's place for a key that has no fixed default.
#[derive(Debug, Clone, Copy)]
pub enum Supply<'a> {
    /// What the environment a thread runs in supplies: it is reported as
    /// coming from the environment.
    Environment(&'a Json),
    /// What the installation was seeded with, what the model or the system's
    /// setup gives, or what another key resolves to: it is reported as the
    /// registry's.
    Default(&'a Json),
}

/// Everything read for a place that a key is resolved from.
#[derive(Debug, Clone, Copy)]
pub struct Stack<'a> {
    /// The rows of the system and of the chain.
    pub rows: &'a Rows,
    /// The nodes from the root org down.
    pub chain: &'a [Link],
    /// The own values of the environment or the provider.
    pub own: Option<&'a OwnValues>,
}

/// The layers of `def` in the order they are given to the contract: the
/// system, each node of the chain from the root org down, the own value, and
/// the supplied default.
#[must_use]
pub fn layers<'a>(
    def: &SettingDef,
    stack: &Stack<'a>,
    supply: Option<Supply<'a>>,
) -> Vec<Layer<'a, Id>> {
    let mut found = Vec::new();
    if def.set_at(Scope::System)
        && let Some(row) = stack.rows.get(None, def.key)
    {
        found.push(Layer::at(Scope::System, None, &row.value, row.locked));
    }
    for link in stack.chain {
        if def.set_at(link.scope)
            && let Some(row) = stack.rows.get(Some(link.id), def.key)
        {
            found.push(Layer::at(link.scope, Some(link.id), &row.value, row.locked));
        }
    }
    if let Some(own) = stack.own
        && def.set_at(own.scope)
        && let Some(value) = own.get(def.key)
    {
        found.push(Layer::at(own.scope, None, value, false));
    }
    match supply {
        Some(Supply::Environment(value)) => found.push(Layer::supplied(value)),
        Some(Supply::Default(value)) => found.push(Layer::default(value)),
        None => {}
    }
    found
}

/// What `def` resolves to through the stack, with where each part of it comes
/// from.
#[must_use]
pub fn resolve_key<'a>(
    def: &SettingDef,
    stack: &Stack<'a>,
    supply: Option<Supply<'a>>,
) -> Resolved<Id> {
    resolve(def, &layers(def, stack, supply))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use orch_settings_contract::provenance::SourceScope;
    use orch_settings_contract::registry::{get, keys};
    use orch_types::new_id;
    use serde_json::json;

    fn row(node: Option<Id>, key: &str, value: Json, locked: bool) -> SettingRow {
        SettingRow {
            node_id: node,
            key: key.to_owned(),
            value,
            locked,
            revision: 1,
            updated_by: None,
            updated_at: Utc.with_ymd_and_hms(2026, 10, 8, 14, 0, 0).unwrap(),
        }
    }

    #[test]
    fn a_row_at_a_scope_the_key_does_not_list_counts_for_nothing() {
        let (project, topic) = (new_id(), new_id());
        let rows = Rows::new(vec![
            row(None, keys::AGENT_MODEL, json!("at-the-system"), false),
            row(
                Some(project),
                keys::AGENT_MODEL,
                json!("at-the-project"),
                false,
            ),
        ]);
        let chain = [
            Link {
                id: project,
                scope: Scope::Project,
            },
            Link {
                id: topic,
                scope: Scope::Topic,
            },
        ];
        let stack = Stack {
            rows: &rows,
            chain: &chain,
            own: None,
        };
        let resolved = resolve_key(get(keys::AGENT_MODEL).unwrap(), &stack, None);
        assert_eq!(resolved.value, Some(json!("at-the-project")));
        assert_eq!(
            resolved.parts.len(),
            1,
            "the model is not set at the system"
        );
    }

    #[test]
    fn the_own_value_comes_after_the_rows_and_a_supplied_default_before_them() {
        let (org, thread) = (new_id(), new_id());
        let rows = Rows::new(vec![row(
            Some(org),
            keys::ENV_UPDATE,
            json!("on_start"),
            false,
        )]);
        let chain = [Link {
            id: org,
            scope: Scope::Org,
        }];
        let own = OwnValues {
            scope: Scope::Environment,
            values: json!({ "env.update": "manual" }),
            revision: 4,
        };
        let stack = Stack {
            rows: &rows,
            chain: &chain,
            own: Some(&own),
        };
        let def = get(keys::ENV_UPDATE).unwrap();
        let resolved = resolve_key(def, &stack, None);
        assert_eq!(resolved.value, Some(json!("manual")));
        assert_eq!(resolved.source.scope, SourceScope::EnvironmentOwn);

        let by_the_environment = json!("auto");
        let lone = Stack {
            rows: &Rows::default(),
            chain: &[Link {
                id: thread,
                scope: Scope::Thread,
            }],
            own: None,
        };
        let approval = get(keys::APPROVAL_MODE).unwrap();
        let supplied = resolve_key(
            approval,
            &lone,
            Some(Supply::Environment(&by_the_environment)),
        );
        assert_eq!(supplied.value, Some(json!("auto")));
        assert_eq!(supplied.source.scope, SourceScope::Environment);
    }

    #[test]
    fn rows_are_kept_apart_by_node_and_replaced_by_the_next_write() {
        let node = new_id();
        let mut rows = Rows::new(vec![row(Some(node), "a.b", json!(1), false)]);
        assert!(rows.get(None, "a.b").is_none() && rows.get(Some(new_id()), "a.b").is_none());
        rows.put(row(Some(node), "a.b", json!(2), true));
        assert_eq!(
            rows.get(Some(node), "a.b")
                .map(|row| (row.value.clone(), row.locked)),
            Some((json!(2), true))
        );
        assert!(rows.take(Some(node), "a.b").is_some() && rows.get(Some(node), "a.b").is_none());
    }
}
