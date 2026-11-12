//! Where work may run: the names an entry gives to narrow a resource to some places, and whether an environment falls under them.
//!
//! An entry in a Use row may say where the work done through it may run, beside what else it
//! narrows. The member is written `where`, a list of [`WhereName`]s, each a kind of place and its
//! identifier; left out, the work may run anywhere, and an entry that names places can only
//! narrow what its row gives. This part holds everything the member is, once: its names, how an
//! environment's place is described ([`Whereabouts`]) and whether a list covers it ([`allows`]);
//! the permission an entry is in only chooses to carry the member, and the lists of several entries
//! combine as every list option of an entry does.
//!
//! A host or a machine that is named covers every environment on it, those made later included.
//! A kind of place that does not exist yet (a pool of environments, a pool of hosts) is one more
//! [`WhereKind`] and one more case of [`WhereName::covers`]; what is stored for the kinds there
//! are does not change.

use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::roles::words;

words! {
    /// A kind of place an entry can name.
    pub enum WhereKind ("kind of place") {
        /// One environment.
        Environment = "environment",
        /// A host: every environment on it.
        Host = "host",
        /// A physical machine: every environment on it.
        Machine = "machine",
    }
}

/// One place an entry names: a kind and the identifier of one of that kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WhereName {
    /// What kind of place it is.
    pub kind: WhereKind,
    /// Which one.
    pub id: Id,
}

/// Where an environment is, as far as the names can tell: itself, the host it is on and the
/// machine it is on. An environment that is not made yet has no identifier of its own, and its
/// host or machine is where it is to be made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Whereabouts {
    /// The environment, once there is one.
    pub environment: Option<Id>,
    /// The host it is on, where it is on one.
    pub host: Option<Id>,
    /// The physical machine it is on, where it is known.
    pub machine: Option<Id>,
}

impl Whereabouts {
    /// The most particular place that names this one: the environment where there is one, else
    /// the host, else the machine. What a refusal names as the place the work was to run in.
    #[must_use]
    pub fn name(&self) -> Option<WhereName> {
        let (kind, id) = match (self.environment, self.host, self.machine) {
            (Some(id), _, _) => (WhereKind::Environment, id),
            (None, Some(id), _) => (WhereKind::Host, id),
            (None, None, Some(id)) => (WhereKind::Machine, id),
            (None, None, None) => return None,
        };
        Some(WhereName { kind, id })
    }
}

impl WhereName {
    /// Whether the place named holds the environment at `at`.
    #[must_use]
    pub fn covers(&self, at: &Whereabouts) -> bool {
        let found = match self.kind {
            WhereKind::Environment => at.environment,
            WhereKind::Host => at.host,
            WhereKind::Machine => at.machine,
        };
        found == Some(self.id)
    }
}

/// Whether work at `at` is allowed by a list of names: it is when the list is left out, and
/// otherwise when one of the names covers it. A name of something that is gone covers nothing.
#[must_use]
pub fn allows(list: Option<&[WhereName]>, at: &Whereabouts) -> bool {
    list.is_none_or(|names| names.iter().any(|name| name.covers(at)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn name(kind: WhereKind, id: Id) -> WhereName {
        WhereName { kind, id }
    }

    #[test]
    fn a_list_that_is_left_out_allows_everywhere() {
        assert!(allows(None, &Whereabouts::default()));
        assert!(allows(
            None,
            &Whereabouts {
                environment: Some(new_id()),
                ..Whereabouts::default()
            }
        ));
    }

    #[test]
    fn an_environment_that_is_named_is_covered_and_another_is_not() {
        let (one, other) = (new_id(), new_id());
        let list = [name(WhereKind::Environment, one)];
        let at = |environment| Whereabouts {
            environment: Some(environment),
            ..Whereabouts::default()
        };
        assert!(allows(Some(&list), &at(one)));
        assert!(!allows(Some(&list), &at(other)));
        assert!(!allows(Some(&list), &Whereabouts::default()));
    }

    #[test]
    fn a_host_or_a_machine_covers_every_environment_on_it_those_made_later_included() {
        let (host, machine) = (new_id(), new_id());
        let on_host = Whereabouts {
            environment: Some(new_id()),
            host: Some(host),
            machine: Some(machine),
        };
        let later = Whereabouts {
            environment: Some(new_id()),
            ..on_host
        };
        let to_be_made = Whereabouts {
            environment: None,
            ..on_host
        };
        let elsewhere = Whereabouts {
            environment: Some(new_id()),
            host: Some(new_id()),
            machine: Some(new_id()),
        };
        for list in [
            [name(WhereKind::Host, host)],
            [name(WhereKind::Machine, machine)],
        ] {
            assert!(allows(Some(&list), &on_host));
            assert!(allows(Some(&list), &later));
            assert!(allows(Some(&list), &to_be_made));
            assert!(!allows(Some(&list), &elsewhere));
        }
        assert!(
            !allows(Some(&[name(WhereKind::Host, machine)]), &on_host),
            "an identifier names a place only as the kind it is written with"
        );
    }

    #[test]
    fn a_list_that_names_nothing_allows_nowhere_and_a_name_of_something_gone_covers_nothing() {
        let list: Vec<WhereName> = Vec::new();
        assert!(!allows(
            Some(&list),
            &Whereabouts {
                environment: Some(new_id()),
                ..Whereabouts::default()
            }
        ));
        let gone = [name(WhereKind::Environment, new_id())];
        assert!(!allows(
            Some(&gone),
            &Whereabouts {
                environment: Some(new_id()),
                host: Some(new_id()),
                machine: Some(new_id()),
            }
        ));
    }

    #[test]
    fn a_refusal_names_the_most_particular_place() {
        let (environment, host, machine) = (new_id(), new_id(), new_id());
        let all = Whereabouts {
            environment: Some(environment),
            host: Some(host),
            machine: Some(machine),
        };
        assert_eq!(all.name(), Some(name(WhereKind::Environment, environment)));
        let unmade = Whereabouts {
            environment: None,
            ..all
        };
        assert_eq!(unmade.name(), Some(name(WhereKind::Host, host)));
        let bare = Whereabouts {
            host: None,
            ..unmade
        };
        assert_eq!(bare.name(), Some(name(WhereKind::Machine, machine)));
        assert_eq!(Whereabouts::default().name(), None);
    }

    #[test]
    fn a_name_is_written_as_its_kind_and_identifier_and_nothing_else_is_read() {
        let id = new_id();
        let written = serde_json::json!({ "kind": "host", "id": id });
        let read: WhereName = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(read, name(WhereKind::Host, id));
        assert_eq!(serde_json::to_value(read).unwrap(), written);
        assert!(
            serde_json::from_value::<WhereName>(serde_json::json!({ "kind": "pool", "id": id }))
                .is_err()
        );
        assert!(
            serde_json::from_value::<WhereName>(
                serde_json::json!({ "kind": "host", "id": id, "name": "x" })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<WhereName>(serde_json::json!({ "kind": "host", "id": "x" }))
                .is_err()
        );
    }
}
