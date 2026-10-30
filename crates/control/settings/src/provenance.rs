//! Explain effective values and their sources.
//!
//! A [`SettingView`] is what resolving a key at a place comes to: the value
//! that holds, where it comes from, every part it was combined from, the row
//! at the place itself, and the lock above that decides it. It is what a
//! settings screen is drawn from. It names nodes by identifier; whoever answers
//! a request puts a reference with a name in their place, and
//! [`SettingView::nodes`] lists the identifiers to look up once for a whole
//! list.

use orch_settings_contract::provenance::{LockedBy, OwnRow, Part, Resolved, ResolvedSetting};
use orch_settings_contract::registry::SettingDef;
use orch_types::{Id, Json};

use crate::locks::{Here, lock_above};

/// A key as it holds at one place, with its provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingView {
    def: &'static SettingDef,
    here: Here,
    settable_here: bool,
    /// The value and where it comes from.
    pub resolved: Resolved<Id>,
    /// The row at the place itself, where there is one.
    pub own: Option<OwnRow>,
}

impl SettingView {
    pub(crate) fn new(
        def: &'static SettingDef,
        here: Here,
        settable_here: bool,
        resolved: Resolved<Id>,
        own: Option<OwnRow>,
    ) -> Self {
        Self {
            def,
            here,
            settable_here,
            resolved,
            own,
        }
    }

    /// The registry's definition of the key.
    #[must_use]
    pub fn def(&self) -> &'static SettingDef {
        self.def
    }

    /// The key.
    #[must_use]
    pub fn key(&self) -> &'static str {
        self.def.key
    }

    /// The value that holds; nothing while the key is unset.
    #[must_use]
    pub fn value(&self) -> Option<&Json> {
        self.resolved.value.as_ref()
    }

    /// Every value that counted, from the top down: what a key that names
    /// things by pattern has to be asked one by one.
    #[must_use]
    pub fn parts(&self) -> &[Part<Id>] {
        &self.resolved.parts
    }

    /// The lock above this place that decides the key, where one does. A lock
    /// at the place itself is not above it.
    #[must_use]
    pub fn lock_above(&self) -> Option<&LockedBy<Id>> {
        lock_above(self.here, self.resolved.locked_by.as_ref())
    }

    /// Whether the key can be written at this place at all, and no lock above
    /// decides it. Whether the caller holds what writing it takes is for the
    /// access engine to say.
    #[must_use]
    pub fn open_to_write(&self) -> bool {
        self.settable_here && self.lock_above().is_none()
    }

    /// The setting as a reader receives it. `permitted` says that the caller
    /// holds what writing the key here takes; the view adds that the key can be
    /// written here and that no lock above applies.
    #[must_use]
    pub fn into_setting(self, permitted: bool) -> ResolvedSetting<Id> {
        let writable = permitted && self.open_to_write();
        self.resolved.into_setting(self.def.key, self.own, writable)
    }

    /// Every node this view names: where the value comes from, where each
    /// part does, and where a lock was set.
    #[must_use]
    pub fn nodes(&self) -> Vec<Id> {
        let mut found: Vec<Id> = self
            .resolved
            .source
            .node
            .iter()
            .chain(
                self.resolved
                    .locked_by
                    .iter()
                    .filter_map(|lock| lock.node.as_ref()),
            )
            .chain(
                self.resolved
                    .parts
                    .iter()
                    .filter_map(|part| part.source.node.as_ref()),
            )
            .chain(
                self.resolved
                    .supplied
                    .iter()
                    .filter_map(|part| part.source.node.as_ref()),
            )
            .copied()
            .collect();
        found.sort_unstable();
        found.dedup();
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_settings_contract::provenance::{LockScope, Source, SourceScope};
    use orch_settings_contract::registry::{get, keys};
    use orch_types::new_id;
    use serde_json::json;

    fn view(here: Here, lock: Option<Id>, settable: bool) -> SettingView {
        let org = lock.unwrap_or_else(new_id);
        let resolved = Resolved {
            value: Some(json!("ask")),
            source: Source {
                scope: SourceScope::Org,
                node: Some(org),
            },
            locked_by: lock.map(|node| LockedBy {
                scope: LockScope::Org,
                node: Some(node),
            }),
            parts: vec![Part {
                source: Source {
                    scope: SourceScope::Org,
                    node: Some(org),
                },
                value: json!("ask"),
            }],
            supplied: Vec::new(),
        };
        SettingView::new(
            get(keys::APPROVAL_MODE).unwrap(),
            here,
            settable,
            resolved,
            None,
        )
    }

    #[test]
    fn a_setting_is_writable_only_with_the_permission_where_it_can_be_and_no_lock_is_above() {
        let (org, project) = (new_id(), new_id());
        assert!(
            view(Here::Node(project), None, true)
                .into_setting(true)
                .writable
        );
        assert!(
            !view(Here::Node(project), None, true)
                .into_setting(false)
                .writable
        );
        assert!(
            !view(Here::Node(project), None, false)
                .into_setting(true)
                .writable
        );
        let locked = view(Here::Node(project), Some(org), true);
        assert!(locked.lock_above().is_some());
        let setting = locked.into_setting(true);
        assert!(!setting.writable);
        assert_eq!(
            setting.locked_by.map(|lock| lock.node),
            Some(Some(org)),
            "the lock is still named"
        );
        assert!(
            view(Here::Node(org), Some(org), true)
                .into_setting(true)
                .writable,
            "where the lock is, the row is changed"
        );
    }

    #[test]
    fn a_view_names_each_node_it_comes_from_once() {
        let org = new_id();
        let nodes = view(Here::Node(new_id()), Some(org), true).nodes();
        assert_eq!(nodes, vec![org]);
    }
}
