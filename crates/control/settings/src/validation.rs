//! Schema, scope and current eligibility checks.
//!
//! Whether a value can be written is three questions: does it fit the key's
//! schema, can the key be set at that kind of place, and is the place one
//! where it can be written now (no lock above, no stale revision; that part
//! is [`crate::write`]). Whether the caller **may** write is not asked here:
//! the access engine decides, and this says what to ask it ([`Needs`]): the
//! permission, and the object it is held on, which for the two keys about
//! where a thread runs is its environment.
//!
//! The own values of an environment and of a provider's runtime are written
//! with the environment and the provider, not with a setting; the
//! [`check_own_values`] they are checked with is the same.

use orch_access_contract::permissions::Permission;
use orch_db::Conn;
use orch_db::queries::{hierarchy, settings as queries};
use orch_settings_contract::provenance::SourceScope;
use orch_settings_contract::registry::{self, Invalid, Scope, SettingDef, Writer, locked_by};
use orch_types::fields::FieldError;
use orch_types::{Id, Json};

use crate::Target;
use crate::resolver::chain_of;
use crate::write::WriteError;

/// Whether `value` can be written for `key` at `scope`: the key exists, it is
/// set at that scope, and the value fits its schema.
pub fn check_value(key: &str, scope: Scope, value: &Json) -> Result<&'static SettingDef, Invalid> {
    registry::validate_at(key, scope, value)
}

/// Whether the key is shown for a place at `scope`: at the system, an
/// environment or a provider's runtime, those that are set there; at a node,
/// those that are set there or at any node below it, so that the screen of an
/// org shows what its projects, topics and threads start from.
#[must_use]
pub fn listed(def: &SettingDef, scope: Scope) -> bool {
    match scope {
        Scope::System | Scope::Environment | Scope::Provider | Scope::Host => def.set_at(scope),
        Scope::Org | Scope::Project | Scope::Topic | Scope::Thread => {
            let depth = SourceScope::from(scope).depth();
            def.scopes.iter().any(|other| {
                other.has_rows()
                    && *other != Scope::System
                    && SourceScope::from(*other).depth() >= depth
            })
        }
    }
}

/// The keys shown for a place at `scope`, in the registry's order.
pub fn keys_at(scope: Scope) -> impl Iterator<Item = &'static SettingDef> {
    registry::all().iter().filter(move |def| listed(def, scope))
}

/// What a write asks of whoever makes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// To be a system admin.
    SystemAdmin,
    /// To hold this permission on this node.
    OnNode {
        /// The permission.
        permission: Permission,
        /// The node.
        node: Id,
    },
    /// To hold this permission on this environment.
    OnEnvironment {
        /// The permission.
        permission: Permission,
        /// The environment.
        environment: Id,
    },
    /// To hold this permission on this provider.
    OnProvider {
        /// The permission.
        permission: Permission,
        /// The provider.
        provider: Id,
    },
}

/// What writing a key at a place asks, and what setting or lifting a lock
/// there asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Needs {
    /// To write the value, or to take it back.
    pub value: Need,
    /// To set or lift the lock of the row.
    pub lock: Need,
}

/// What writing `def` at the node `node` of scope `scope` asks. For a key
/// about where a thread runs, written at a thread, it is the environment's
/// manager when the thread is in an environment (`thread_environment`), and
/// whoever manages the thread when it is in none. Nothing where the key is not
/// set at that scope.
#[must_use]
pub fn need_for_node(
    def: &SettingDef,
    scope: Scope,
    node: Id,
    thread_environment: Option<Id>,
) -> Option<Need> {
    Some(match def.written_by(scope)? {
        Writer::SystemAdmin => Need::SystemAdmin,
        Writer::OnTheNode(permission) => Need::OnNode { permission, node },
        Writer::OnTheThreadsEnvironment => match thread_environment {
            Some(environment) => Need::OnEnvironment {
                permission: Permission::EnvironmentManage,
                environment,
            },
            None => Need::OnNode {
                permission: Permission::ThreadManage,
                node,
            },
        },
        Writer::OnTheEnvironment | Writer::OnTheProvider | Writer::OnTheHost => return None,
    })
}

/// What writing the own value of `def` for the environment `environment`
/// asks. Nothing where the key is not one of an environment's own.
#[must_use]
pub fn need_for_environment(def: &SettingDef, environment: Id) -> Option<Need> {
    match def.written_by(Scope::Environment)? {
        Writer::OnTheEnvironment => Some(Need::OnEnvironment {
            permission: Permission::EnvironmentManage,
            environment,
        }),
        _ => None,
    }
}

/// What writing the own value of `def` for the runtime of the provider
/// `provider` asks. Nothing where the key is not one of a runtime's own.
#[must_use]
pub fn need_for_provider(def: &SettingDef, provider: Id) -> Option<Need> {
    match def.written_by(Scope::Provider)? {
        Writer::OnTheProvider => Some(Need::OnProvider {
            permission: Permission::ProviderManage,
            provider,
        }),
        _ => None,
    }
}

/// What setting or lifting the lock of a row at `scope` asks: whoever makes
/// the rules there, whatever the key's class. Nothing for the own values of
/// an environment and of a provider's runtime, which carry no lock.
#[must_use]
pub fn need_to_lock(scope: Scope, node: Option<Id>) -> Option<Need> {
    Some(match locked_by(scope)? {
        Writer::SystemAdmin => Need::SystemAdmin,
        Writer::OnTheNode(permission) => Need::OnNode {
            permission,
            node: node?,
        },
        Writer::OnTheThreadsEnvironment
        | Writer::OnTheEnvironment
        | Writer::OnTheProvider
        | Writer::OnTheHost => {
            return None;
        }
    })
}

/// What writing `key` at `target` asks, for the caller to put to the access
/// engine before it writes. A key that does not exist, or is not set at that
/// kind of place, is refused as a write would be.
pub async fn needs(conn: &mut Conn, key: &str, target: Target) -> Result<Needs, WriteError> {
    let def = registry::get(key).ok_or_else(|| Invalid::UnknownKey(key.to_owned()))?;
    let (scope, node) = match target {
        Target::System => (Scope::System, None),
        Target::Node(id) => {
            let nodes = hierarchy::with_ancestors(conn, &[id]).await?;
            let chain = chain_of(&nodes, id).map_err(|_| WriteError::NotFound)?;
            let here = chain.last().ok_or(WriteError::NotFound)?;
            (here.scope, Some(id))
        }
    };
    if !def.set_at(scope) {
        return Err(Invalid::WrongScope {
            key: def.key,
            scope,
        }
        .into());
    }
    let thread_environment = match node {
        Some(node) if scope == Scope::Thread && def.is_about_where_a_thread_runs() => {
            queries::thread_environment(conn, node)
                .await?
                .map(|found| found.environment_id)
        }
        _ => None,
    };
    let value = match node {
        Some(node) => need_for_node(def, scope, node, thread_environment),
        None => def.written_by(scope).map(|_| Need::SystemAdmin),
    };
    Ok(Needs {
        value: value.ok_or(Invalid::WrongScope {
            key: def.key,
            scope,
        })?,
        lock: need_to_lock(scope, node).unwrap_or(Need::SystemAdmin),
    })
}

/// Checks the own values of an environment (`scope` is
/// [`Scope::Environment`]) or of a provider's runtime ([`Scope::Provider`]),
/// which are an object of values by key: each key must be one that is set at
/// that kind of thing and each value must fit its schema. Everything that does
/// not fit is returned, each with its place, the key first.
#[must_use]
pub fn check_own_values(scope: Scope, values: &Json) -> Vec<FieldError> {
    let fault = |path: &str, message: &str| FieldError {
        path: path.to_owned(),
        message: message.to_owned(),
    };
    let Json::Object(members) = values else {
        return vec![fault("", "must be an object")];
    };
    let mut faults = Vec::new();
    for (key, value) in members {
        let Some(def) = registry::get(key) else {
            faults.push(fault(key, "is no setting"));
            continue;
        };
        if !def.set_at(scope) {
            faults.push(fault(key, &format!("is not set at the scope {scope}")));
            continue;
        }
        for inner in def.faults(value) {
            let path = if inner.path.is_empty() {
                key.clone()
            } else {
                format!("{key}.{}", inner.path)
            };
            faults.push(FieldError {
                path,
                message: inner.message,
            });
        }
    }
    faults
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_settings_contract::registry::{get, keys};
    use orch_types::new_id;
    use serde_json::json;

    fn listed_at(scope: Scope) -> Vec<&'static str> {
        keys_at(scope).map(|def| def.key).collect()
    }

    #[test]
    fn a_node_shows_what_is_set_there_or_below_and_the_system_what_is_set_at_the_system() {
        let org = listed_at(Scope::Org);
        assert!(org.contains(&keys::AGENT_MODEL) && org.contains(&keys::THREAD_CWD));
        assert!(
            !org.contains(&keys::HOSTS_FULL_PERCENT),
            "that is the system's alone"
        );
        assert!(
            !org.contains(&keys::EXEC_OFFERS),
            "that is an environment's alone"
        );
        let thread = listed_at(Scope::Thread);
        assert!(thread.contains(&keys::AGENT_MODEL) && thread.contains(&keys::THREAD_CWD));
        assert!(
            !thread.contains(&keys::TERMINAL_ENABLED),
            "nothing is set at a thread that is set at the topic and above only"
        );
        let system = listed_at(Scope::System);
        assert!(system.contains(&keys::HOSTS_FULL_PERCENT) && !system.contains(&keys::AGENT_MODEL));
        assert_eq!(
            listed_at(Scope::Environment)
                .iter()
                .filter(|key| **key == keys::EXEC_OFFERS)
                .count(),
            1
        );
        assert!(listed_at(Scope::Provider).contains(&keys::RUNTIME_RENEW_DAYS));
    }

    #[test]
    fn a_key_about_where_a_thread_runs_asks_for_its_environment_and_otherwise_for_the_thread() {
        let (thread, environment) = (new_id(), new_id());
        let cwd = get(keys::THREAD_CWD).unwrap();
        assert_eq!(
            need_for_node(cwd, Scope::Thread, thread, Some(environment)),
            Some(Need::OnEnvironment {
                permission: Permission::EnvironmentManage,
                environment
            })
        );
        assert_eq!(
            need_for_node(cwd, Scope::Thread, thread, None),
            Some(Need::OnNode {
                permission: Permission::ThreadManage,
                node: thread
            })
        );
        let model = get(keys::AGENT_MODEL).unwrap();
        assert_eq!(
            need_for_node(model, Scope::Thread, thread, Some(environment)),
            Some(Need::OnNode {
                permission: Permission::ThreadManage,
                node: thread
            }),
            "the model is the session's, whatever environment it is in"
        );
        assert_eq!(
            need_for_node(model, Scope::System, thread, None),
            None,
            "not set there"
        );
    }

    #[test]
    fn a_lock_is_asked_of_whoever_makes_the_rules_whatever_the_class() {
        let node = new_id();
        assert_eq!(
            need_to_lock(Scope::Project, Some(node)),
            Some(Need::OnNode {
                permission: Permission::SettingsManage,
                node
            })
        );
        assert_eq!(need_to_lock(Scope::System, None), Some(Need::SystemAdmin));
        assert_eq!(need_to_lock(Scope::Environment, None), None);
    }

    #[test]
    fn the_own_values_of_an_environment_are_keys_of_its_scope_with_values_that_fit() {
        let ok = json!({ "exec.offers": ["plain"], "env.update": "on_start" });
        assert!(check_own_values(Scope::Environment, &ok).is_empty());
        let bad = json!({
            "env.update": "sometimes",
            "agent.model": "gpt-x",
            "no.such.key": 1,
            "env.mounts": { "scratch": true }
        });
        let faults: Vec<_> = check_own_values(Scope::Environment, &bad)
            .into_iter()
            .map(|fault| fault.path)
            .collect();
        assert_eq!(
            faults,
            [
                "env.update",
                "agent.model",
                "no.such.key",
                "env.mounts.scratch"
            ]
        );
        assert_eq!(check_own_values(Scope::Environment, &json!([])).len(), 1);
        assert!(
            !check_own_values(Scope::Provider, &json!({ "exec.offers": [] })).is_empty(),
            "a provider's runtime has no exec node"
        );
    }
}
