//! The words for what a check of a build reports: its cases and the features it switches off, in each language.
//!
//! The native conformance cases are shared by every program, and so are most of the features a failure can
//! switch off, so their titles are written once, here. A program's manifest adds a title of its own for a case
//! or a feature only it has.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::localized::Localized;

/// The embedded catalogue.
const CATALOGUE: &str = include_str!("../catalogue.json");

/// Titles for what a check reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Catalogue {
    /// Capability groups and their declaration paths, as the program page displays them.
    pub capability_groups: Vec<CapabilityGroup>,
    /// Reason for a capability the declaration does not claim.
    pub not_supported: Localized,
    /// Reason for a declaration disabled by its check.
    pub off_by_check: Localized,
    /// What each shared case observes, by the case's name.
    pub cases: BTreeMap<String, Localized>,
    /// What each feature is called as the thing that stops working, by the feature's identifier.
    pub features: BTreeMap<String, Localized>,
    /// What an incomplete report is called.
    pub runner: Localized,
    /// What a feature the catalogue does not know is called.
    pub other_feature: Localized,
}

/// One capability title and its path in the adapter declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityEntry {
    /// Capability identifier, also the check feature when it has one.
    pub id: String,
    /// Translated title.
    pub title: Localized,
    /// JSON pointer; a suffix after `:` matches a member of an array.
    pub path: String,
}

/// A wrapping group of related capabilities, in display order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityGroup {
    /// Group identifier.
    pub id: String,
    /// Translated heading.
    pub title: Localized,
    /// Capability entries in display order.
    pub items: Vec<CapabilityEntry>,
}

impl Catalogue {
    /// The catalogue Orch ships.
    #[must_use]
    pub fn builtin() -> &'static Self {
        #[expect(
            clippy::expect_used,
            reason = "the embedded catalogue is data of this package, and its tests read it"
        )]
        fn load() -> Catalogue {
            serde_json::from_str(CATALOGUE).expect("the built-in catalogue is valid")
        }
        static BUILTIN: OnceLock<Catalogue> = OnceLock::new();
        BUILTIN.get_or_init(load)
    }

    /// The title of a feature: the catalogue's, else `own` (the program's), else the catalogue's word for one
    /// it does not know.
    #[must_use]
    pub fn feature<'a>(&'a self, id: &str, own: Option<&'a Localized>) -> &'a Localized {
        self.features.get(id).or(own).unwrap_or(&self.other_feature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_title_is_in_every_language() {
        let catalogue = Catalogue::builtin();
        for (name, title) in catalogue.cases.iter().chain(&catalogue.features) {
            assert!(title.covers_every_language(), "{name}");
        }
        for group in &catalogue.capability_groups {
            assert!(group.title.covers_every_language());
            for item in &group.items {
                assert!(item.title.covers_every_language());
            }
        }
        assert!(catalogue.not_supported.covers_every_language());
        assert!(catalogue.off_by_check.covers_every_language());
        assert!(catalogue.runner.covers_every_language());
        assert!(catalogue.other_feature.covers_every_language());
    }

    #[test]
    fn what_a_shipped_programs_check_can_report_is_titled() {
        let catalogue = Catalogue::builtin();
        for manifest in crate::Programs::builtin().programs() {
            let features = manifest
                .checks
                .interface
                .iter()
                .flat_map(|probe| probe.needs.iter())
                .filter_map(|need| need.capability.as_deref())
                .chain(
                    manifest
                        .checks
                        .conformance
                        .iter()
                        .filter_map(|case| case.feature.as_deref()),
                );
            for feature in features {
                assert!(
                    catalogue.features.contains_key(feature)
                        || manifest.checks.features.contains_key(feature),
                    "{}: no title for the feature {feature}",
                    manifest.kind
                );
            }
            for case in &manifest.checks.conformance {
                assert!(
                    case.title.is_some() || catalogue.cases.contains_key(&case.name),
                    "{}: no title for the case {}",
                    manifest.kind,
                    case.name
                );
            }
            for probe in &manifest.checks.probes {
                assert!(
                    catalogue.cases.contains_key(probe),
                    "{}: no title for the probe {probe}",
                    manifest.kind
                );
            }
        }
    }

    #[test]
    fn a_feature_nobody_titled_is_called_by_the_catalogues_word_for_others() {
        let catalogue = Catalogue::builtin();
        assert_eq!(
            catalogue.feature("steer", None).en(),
            "steering a running turn"
        );
        assert_eq!(
            catalogue.feature("nobody", None).en(),
            "other program functions"
        );
        let own = Localized::english("plugin steering");
        assert_eq!(
            catalogue.feature("plugin.steer", Some(&own)).en(),
            "plugin steering"
        );
    }
}
