//! Resolved values, origin and environment defaults.
//!
//! A resolved value is shown with where it comes from, so that whoever
//! reads a setting can see why it is what it is and where to change it: the
//! registry's default, a default that the thread's environment supplies, a
//! row at the system or at a node on the path, or the own value of the thing
//! itself. Where a lock above decides, the place that locked it is named.
//!
//! [`Resolved`] is what resolving a key yields; [`ResolvedSetting`] is that
//! together with the row at the node asked about and whether the caller may
//! write there, as a reader of settings receives it. Both are generic in how
//! a node is referred to: resolving works with identifiers, and whoever
//! answers a request puts a reference with a name in their place
//! ([`Resolved::map_node`]).

use orch_types::{Json, Revision};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::registry::{Scope, words};

words! {
    /// Where a value comes from.
    pub enum SourceScope ("scope of a source") {
        /// The registry: no scope sets the key.
        Default = "default",
        /// A default that the environment supplies: what a thread runs in,
        /// or for an environment's own keys its template or its machine. It
        /// sits between the registry and the system scope.
        Environment = "environment",
        /// The row at the system.
        System = "system",
        /// The row at an org, at any level.
        Org = "org",
        /// The row at a project.
        Project = "project",
        /// The row at a topic.
        Topic = "topic",
        /// The row at a thread.
        Thread = "thread",
        /// An environment's own value for one of its keys, which comes last.
        EnvironmentOwn = "environment_own",
        /// The own value of a provider's runtime for one of its keys, which
        /// comes last.
        ProviderOwn = "provider_own",
        /// The host's own maintenance value.
        HostOwn = "host_own",
    }
}

impl SourceScope {
    /// How far down it lies among the layers that a key resolves through:
    /// the registry first, the thing's own value last. Orgs lie inside one
    /// another and share one step.
    #[must_use]
    pub const fn depth(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::Environment => 1,
            Self::System => 2,
            Self::Org => 3,
            Self::Project => 4,
            Self::Topic => 5,
            Self::Thread => 6,
            Self::EnvironmentOwn | Self::ProviderOwn | Self::HostOwn => 7,
        }
    }

    /// The scope whose lock it is, where a layer of this scope is a stored
    /// row that can be locked.
    #[must_use]
    pub const fn lock(self) -> Option<LockScope> {
        match self {
            Self::System => Some(LockScope::System),
            Self::Org => Some(LockScope::Org),
            Self::Project => Some(LockScope::Project),
            Self::Topic => Some(LockScope::Topic),
            Self::Thread => Some(LockScope::Thread),
            Self::Default
            | Self::Environment
            | Self::EnvironmentOwn
            | Self::ProviderOwn
            | Self::HostOwn => None,
        }
    }
}

impl From<Scope> for SourceScope {
    /// The source that a value written at `scope` is reported as.
    fn from(scope: Scope) -> Self {
        match scope {
            Scope::System => Self::System,
            Scope::Org => Self::Org,
            Scope::Project => Self::Project,
            Scope::Topic => Self::Topic,
            Scope::Thread => Self::Thread,
            Scope::Environment => Self::EnvironmentOwn,
            Scope::Provider => Self::ProviderOwn,
            Scope::Host => Self::HostOwn,
        }
    }
}

words! {
    /// Where a lock was set: at the system, or at a node.
    pub enum LockScope ("scope of a lock") {
        /// The system.
        System = "system",
        /// An org, at any level.
        Org = "org",
        /// A project.
        Project = "project",
        /// A topic.
        Topic = "topic",
        /// A thread.
        Thread = "thread",
    }
}

/// Where a value comes from: its scope, and for a row at a node that node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Source<N> {
    /// The scope.
    pub scope: SourceScope,
    /// The node, for a row at an org, a project, a topic or a thread.
    pub node: Option<N>,
}

impl<N> Source<N> {
    /// The registry's default.
    #[must_use]
    pub const fn registry() -> Self {
        Self {
            scope: SourceScope::Default,
            node: None,
        }
    }

    /// The same source, with its node referred to in another way.
    pub fn map_node<M>(self, to: impl FnOnce(N) -> M) -> Source<M> {
        Source {
            scope: self.scope,
            node: self.node.map(to),
        }
    }
}

/// The place whose lock decides a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LockedBy<N> {
    /// Where the lock was set.
    pub scope: LockScope,
    /// The node, where it was set at one.
    pub node: Option<N>,
}

impl<N> LockedBy<N> {
    /// The same lock, with its node referred to in another way.
    pub fn map_node<M>(self, to: impl FnOnce(N) -> M) -> LockedBy<M> {
        LockedBy {
            scope: self.scope,
            node: self.node.map(to),
        }
    }
}

/// One of the values a key was resolved from, with where it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part<N> {
    /// Where it stands.
    pub source: Source<N>,
    /// The value there.
    pub value: Json,
}

/// One value that was supplied for a key on the way down, as a reader is
/// shown it: where it stands, whether its row is locked, and whether a lock
/// above leaves it out. A default comes first when it is part of the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SuppliedPart<N> {
    /// Where it stands.
    pub source: Source<N>,
    /// The value there.
    pub value: Json,
    /// Whether the row is locked for everything below it.
    pub locked: bool,
    /// Whether a lock above leaves it out.
    pub ignored: bool,
}

impl<N> SuppliedPart<N> {
    /// The same, with its node referred to in another way.
    pub fn map_node<M>(self, to: impl FnOnce(N) -> M) -> SuppliedPart<M> {
        SuppliedPart {
            source: self.source.map_node(to),
            value: self.value,
            locked: self.locked,
            ignored: self.ignored,
        }
    }
}

/// What a key resolves to through a chain of scopes, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<N> {
    /// The effective value; nothing while the key is unset.
    pub value: Option<Json>,
    /// Where it comes from. For a key whose values combine, the nearest
    /// scope that added to it.
    pub source: Source<N>,
    /// The place whose lock decides it, when one does: the topmost locked
    /// row on the way.
    pub locked_by: Option<LockedBy<N>>,
    /// Every value that counted, from the top down. Under a lock the rows
    /// below it are not among them. A key that names things by pattern
    /// cannot be narrowed to one list: whoever asks whether one thing is
    /// allowed asks each of these.
    pub parts: Vec<Part<N>>,
    /// Every value that was supplied, the default first where it is part of
    /// the value and then from the top down, those under a lock among them
    /// and marked so: what a reader is shown of where each part comes from.
    pub supplied: Vec<SuppliedPart<N>>,
}

impl<N> Resolved<N> {
    /// The same, with every node referred to in another way.
    pub fn map_node<M>(self, mut to: impl FnMut(N) -> M) -> Resolved<M> {
        Resolved {
            value: self.value,
            source: self.source.map_node(&mut to),
            locked_by: self.locked_by.map(|lock| lock.map_node(&mut to)),
            parts: self
                .parts
                .into_iter()
                .map(|part| Part {
                    source: part.source.map_node(&mut to),
                    value: part.value,
                })
                .collect(),
            supplied: self
                .supplied
                .into_iter()
                .map(|part| part.map_node(&mut to))
                .collect(),
        }
    }

    /// Whether a lock that was set above `here` decides the key, so that it
    /// cannot be written there: the lock is at another place than `here`,
    /// which is a node, or `None` for the system. Whoever makes the rules at
    /// the place that set the lock changes the row there.
    #[must_use]
    pub fn locked_above(&self, here: Option<&N>) -> bool
    where
        N: PartialEq,
    {
        self.locked_by
            .as_ref()
            .is_some_and(|lock| lock.node.as_ref() != here)
    }

    /// The setting as a reader receives it: with the row at the node that
    /// was asked about, and whether the caller may write the key there.
    #[must_use]
    pub fn into_setting(
        self,
        key: &str,
        own: Option<OwnRow>,
        writable: bool,
    ) -> ResolvedSetting<N> {
        ResolvedSetting {
            key: key.to_owned(),
            value: self.value,
            source: self.source,
            own,
            locked_by: self.locked_by,
            writable,
            parts: self.supplied,
        }
    }
}

/// The row of a key at the node that was asked about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OwnRow {
    /// The value written there.
    pub value: Json,
    /// Whether it is locked for everything below.
    pub locked: bool,
    /// How many times it was written.
    pub revision: Revision,
}

/// A key as it holds at one node, with where its value comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedSetting<N> {
    /// The key.
    pub key: String,
    /// The value that is effective at this node; `null` while unset.
    pub value: Option<Json>,
    /// Where that value comes from.
    pub source: Source<N>,
    /// The row at this node, when there is one.
    pub own: Option<OwnRow>,
    /// The place whose lock decides the key, when one does.
    pub locked_by: Option<LockedBy<N>>,
    /// Whether the caller may write the key here: they hold what writing it
    /// takes, and no lock above applies.
    pub writable: bool,
    /// Every value that was supplied for the key, as [`Resolved::supplied`]
    /// says.
    pub parts: Vec<SuppliedPart<N>>,
}

impl<N> ResolvedSetting<N> {
    /// The same, with every node referred to in another way.
    pub fn map_node<M>(self, mut to: impl FnMut(N) -> M) -> ResolvedSetting<M> {
        ResolvedSetting {
            key: self.key,
            value: self.value,
            source: self.source.map_node(&mut to),
            own: self.own,
            locked_by: self.locked_by.map(|lock| lock.map_node(&mut to)),
            writable: self.writable,
            parts: self
                .parts
                .into_iter()
                .map(|part| part.map_node(&mut to))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::{Id, new_id};
    use serde_json::json;

    #[test]
    fn a_resolved_setting_is_written_with_where_its_value_comes_from() {
        let org = new_id();
        let setting = Resolved {
            value: Some(json!("ask")),
            source: Source {
                scope: SourceScope::Org,
                node: Some(org),
            },
            locked_by: Some(LockedBy {
                scope: LockScope::Org,
                node: Some(org),
            }),
            parts: Vec::new(),
            supplied: Vec::new(),
        }
        .into_setting(
            "approval.mode",
            Some(OwnRow {
                value: json!("auto"),
                locked: false,
                revision: 3,
            }),
            false,
        );
        let written = serde_json::to_value(&setting).unwrap();
        assert_eq!(
            written,
            json!({
                "key": "approval.mode",
                "value": "ask",
                "source": { "scope": "org", "node": org },
                "own": { "value": "auto", "locked": false, "revision": 3 },
                "locked_by": { "scope": "org", "node": org },
                "writable": false,
                "parts": []
            })
        );
        assert_eq!(
            serde_json::from_value::<ResolvedSetting<Id>>(written).unwrap(),
            setting
        );
    }

    #[test]
    fn an_unset_key_is_written_as_null_from_the_default() {
        let unset: ResolvedSetting<Id> = Resolved {
            value: None,
            source: Source::registry(),
            locked_by: None,
            parts: Vec::new(),
            supplied: Vec::new(),
        }
        .into_setting("agent.model", None, true);
        assert_eq!(
            serde_json::to_value(&unset).unwrap(),
            json!({
                "key": "agent.model", "value": null, "source": { "scope": "default", "node": null },
                "own": null, "locked_by": null, "writable": true, "parts": []
            })
        );
    }

    #[test]
    fn a_node_is_referred_to_as_whoever_answers_needs_it() {
        let org = new_id();
        let resolved = Resolved {
            value: Some(json!(true)),
            source: Source {
                scope: SourceScope::Org,
                node: Some(org),
            },
            locked_by: Some(LockedBy {
                scope: LockScope::Org,
                node: Some(org),
            }),
            parts: vec![Part {
                source: Source {
                    scope: SourceScope::Org,
                    node: Some(org),
                },
                value: json!(true),
            }],
            supplied: Vec::new(),
        };
        let named = resolved
            .map_node(|id| json!({ "id": id, "kind": "org", "name": "Acme", "redacted": false }));
        let written = serde_json::to_value(named.into_setting("chat.enabled", None, true)).unwrap();
        assert_eq!(written["source"]["node"]["name"], "Acme");
        assert_eq!(written["locked_by"]["node"]["id"], json!(org));
    }

    #[test]
    fn a_key_is_written_where_its_lock_is_and_not_below() {
        let (org, topic) = (new_id(), new_id());
        let locked_at_the_org = Resolved {
            value: Some(json!(false)),
            source: Source {
                scope: SourceScope::Org,
                node: Some(org),
            },
            locked_by: Some(LockedBy {
                scope: LockScope::Org,
                node: Some(org),
            }),
            parts: Vec::new(),
            supplied: Vec::new(),
        };
        assert!(locked_at_the_org.locked_above(Some(&topic)));
        assert!(
            !locked_at_the_org.locked_above(Some(&org)),
            "where the lock is, the row is changed"
        );
        let free: Resolved<Id> = Resolved {
            value: None,
            source: Source::registry(),
            locked_by: None,
            parts: Vec::new(),
            supplied: Vec::new(),
        };
        assert!(!free.locked_above(Some(&topic)));
    }

    #[test]
    fn a_things_own_value_is_reported_as_its_own() {
        assert_eq!(
            SourceScope::from(Scope::Environment),
            SourceScope::EnvironmentOwn
        );
        assert_eq!(SourceScope::from(Scope::Provider).as_str(), "provider_own");
        assert_eq!(SourceScope::from(Scope::Topic), SourceScope::Topic);
        assert_eq!(
            SourceScope::EnvironmentOwn.lock(),
            None,
            "the innermost value carries no lock"
        );
        assert_eq!(SourceScope::Environment.lock(), None);
        assert_eq!(SourceScope::System.lock(), Some(LockScope::System));
        assert!(SourceScope::Environment.depth() < SourceScope::System.depth());
        assert!(SourceScope::Thread.depth() < SourceScope::EnvironmentOwn.depth());
    }
}
