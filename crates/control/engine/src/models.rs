//! A provider's models: the name that is sent and the name that is shown, what an endpoint's list gives of each, the one pattern rule that allowlists, caps and rates share, and the speeds and access programs a manager enables one by one.
//!
//! A model has two names. Its **routing name** is the exact name sent upstream, opaque to the system and the one
//! everything that counts, limits, rates and allows goes by: what a thread's model setting holds, what a turn
//! and a row of the usage ledger record, what a lease allows. Its **display name** is what people see; it is
//! read from the provider's entry for the model when it is shown, and a model whose entry is gone is shown by
//! its routing name. Nothing is matched, counted or allowed by the display name.
//!
//! One rule names models wherever a pattern does: the allowlist of a place (`models.allowed`), the models of a
//! cap, the rate card and the rate catalogue. It is the agent contract's ([`ModelPattern`]): both sides in lower
//! case, `*` for any run of characters, a pattern without a `/` compared with what follows the last `/` of the
//! name (so that `gpt-6*` covers `gpt-6-sol` and `openai/gpt-6-sol` alike), a pattern with one compared with the
//! whole name, and, where one pattern of a list has to be chosen, the one with the most characters that are not
//! `*`. This module is where the engine asks it, so that no second rule grows beside it.
//!
//! **What a turn's model has to be.** The model a turn states is the one resolved for its thread. It has to be
//! enabled on the provider and within the allowlist of the place, and to list the effort the turn would use;
//! the first two are [`check_allowed`], the third is the entry's own parameters. A turn is blocked with the
//! reason when one does not hold, and a lease lets a request through for the turn's model alone.
//!
//! **Groups.** A provider's managers may put a model in groups of their own naming (`frontier`, `small`), and a
//! list that says which models something covers names all of a group as `@<group>`: the models whose entry at
//! the provider concerned is in it. For a turn that provider is the thread's; for an entry on a provider's page
//! it is that provider's. So every place that asks whether a list names a model asks [`named_by`], with the
//! groups of the model's entry there, and none grows a rule of its own. A group no model is in names nothing.

use orch_agent_contract::discovery::ModelPattern;
use orch_db::queries::resources::thread_engine::ModelRow;

/// Whether a list of patterns, which says which models something covers (an allowlist, a cap, an entry's
/// models), names the model with routing name `model`, whose entry at the provider concerned is in `groups`.
/// An empty list names nothing: whoever asks leaves out a list that was not set.
#[must_use]
pub fn named_by(patterns: &[String], model: &str, groups: &[String]) -> bool {
    ModelPattern::list_names(patterns, model, groups)
}

/// Whether a place that allows `patterns` allows the model with routing name `model`, which its provider's
/// managers put in `groups`. A place that sets no allowlist is not asked: the caller passes `None` for that.
#[must_use]
pub fn allowed_by(patterns: &[String], model: &str, groups: &[String]) -> bool {
    named_by(patterns, model, groups)
}

/// Why a model cannot be run on a provider at a place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotAllowed {
    /// The provider has no entry for it, or the entry is not enabled.
    NotEnabled,
    /// The allowlist of the place leaves it out.
    LeftOutByThePlace,
}

impl NotAllowed {
    /// The sentence a block or a refusal carries, for the model `model`.
    #[must_use]
    pub fn describe(&self, model: &str) -> String {
        match self {
            Self::NotEnabled => format!("the model {model} is not enabled on the provider"),
            Self::LeftOutByThePlace => {
                format!("the model {model} is not among the models allowed here")
            }
        }
    }
}

/// Whether the model with routing name `model` may be run: the provider lists it as enabled (`entry` is its
/// row) and the allowlist of the place, where there is one, names it.
pub fn check_allowed(
    entry: Option<&ModelRow>,
    allowlist: Option<&[String]>,
    model: &str,
) -> Result<(), NotAllowed> {
    if !entry.is_some_and(|entry| entry.enabled) {
        return Err(NotAllowed::NotEnabled);
    }
    if allowlist.is_some_and(|patterns| {
        !entry.is_some_and(|entry| allowed_by(patterns, model, &entry.groups))
    }) {
        return Err(NotAllowed::LeftOutByThePlace);
    }
    Ok(())
}

/// The two names of a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    /// The routing name: what is sent upstream and what everything goes by.
    pub sent: String,
    /// The name people see: the display name of the provider's entry, else the routing name.
    pub shown: String,
}

/// The names of the model `model`, whose entry on the provider is `entry` where there still is one.
#[must_use]
pub fn names(model: &str, entry: Option<&ModelRow>) -> Names {
    let shown = entry
        .map(|entry| entry.display_name.as_str())
        .filter(|shown| !shown.is_empty())
        .unwrap_or(model);
    Names {
        sent: model.to_owned(),
        shown: shown.to_owned(),
    }
}

/// The models a lease lets a request name for a turn that runs `model`: that one alone. The allowlist of the
/// place and the enabled entries bound what a turn may be started with (see [`check_allowed`]); a harness that
/// reaches for another model on its own, a helper step or a sub-agent, is refused on the wire and the refusal
/// is reported, since only the model that was checked for the person who sent the input is theirs to spend.
#[must_use]
pub fn lease_models(model: &str) -> Vec<String> {
    vec![model.to_owned()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::Json;

    fn patterns(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn entry(display: &str, enabled: bool) -> ModelRow {
        ModelRow {
            model_name: "openai/gpt-6-sol".to_owned(),
            display_name: display.to_owned(),
            parameters: Json::Null,
            groups: Vec::new(),
            enabled,
        }
    }

    fn grouped(groups: &[&str]) -> ModelRow {
        ModelRow {
            groups: groups.iter().map(|group| (*group).to_owned()).collect(),
            ..entry("", true)
        }
    }

    #[test]
    fn a_model_is_named_by_its_own_name_or_by_a_family() {
        let allowed = patterns(&["gpt-6-sol", "claude-*"]);
        assert!(allowed_by(&allowed, "gpt-6-sol", &[]));
        assert!(allowed_by(&allowed, "claude-opus-5", &[]));
        assert!(!allowed_by(&allowed, "gpt-6-luna", &[]));
        assert!(!allowed_by(&patterns(&[]), "gpt-6-sol", &[]));
    }

    #[test]
    fn the_rule_is_the_contracts_so_case_the_slash_and_a_star_anywhere_are_as_it_says() {
        assert!(allowed_by(&patterns(&["GPT-6*"]), "gpt-6-sol", &[]));
        assert!(allowed_by(&patterns(&["gpt-6*"]), "openai/gpt-6-sol", &[]));
        assert!(allowed_by(
            &patterns(&["openai/*"]),
            "openai/gpt-6-sol",
            &[]
        ));
        assert!(!allowed_by(&patterns(&["openai/*"]), "gpt-6-sol", &[]));
        assert!(allowed_by(&patterns(&["gpt-*-sol"]), "gpt-6-sol", &[]));
        assert!(!allowed_by(&patterns(&["gpt-6*"]), "openai/o9", &[]));
    }

    #[test]
    fn a_model_runs_only_where_the_provider_enables_it_and_the_place_allows_it() {
        let enabled = entry("", true);
        assert_eq!(
            check_allowed(Some(&enabled), None, "openai/gpt-6-sol"),
            Ok(())
        );
        let listed = patterns(&["gpt-6*"]);
        assert_eq!(
            check_allowed(Some(&enabled), Some(&listed), "openai/gpt-6-sol"),
            Ok(())
        );
        let other = patterns(&["claude-*"]);
        assert_eq!(
            check_allowed(Some(&enabled), Some(&other), "openai/gpt-6-sol"),
            Err(NotAllowed::LeftOutByThePlace)
        );
        let off = entry("", false);
        assert_eq!(
            check_allowed(Some(&off), None, "openai/gpt-6-sol"),
            Err(NotAllowed::NotEnabled)
        );
        assert_eq!(
            check_allowed(None, None, "openai/gpt-6-sol"),
            Err(NotAllowed::NotEnabled)
        );
    }

    #[test]
    fn a_group_is_named_against_the_entry_of_the_model_at_the_provider_concerned() {
        let frontier = grouped(&["frontier"]);
        let listed = patterns(&["@frontier"]);
        assert_eq!(
            check_allowed(Some(&frontier), Some(&listed), "openai/gpt-6-sol"),
            Ok(())
        );
        assert_eq!(
            check_allowed(
                Some(&grouped(&["small"])),
                Some(&listed),
                "openai/gpt-6-sol"
            ),
            Err(NotAllowed::LeftOutByThePlace),
            "the same name, in another group at this provider"
        );
        assert_eq!(
            check_allowed(Some(&grouped(&[])), Some(&listed), "openai/gpt-6-sol"),
            Err(NotAllowed::LeftOutByThePlace),
            "a group no model is in names nothing"
        );
        let mixed = patterns(&["claude-*", "@frontier"]);
        assert!(named_by(&mixed, "claude-opus-5", &[]));
        assert!(named_by(&mixed, "gpt-6-sol", &["frontier".to_owned()]));
        assert!(!named_by(&mixed, "gpt-6-sol", &["small".to_owned()]));
    }

    #[test]
    fn a_model_is_shown_by_its_display_name_and_by_its_routing_name_when_it_has_none() {
        let named = names("openai/gpt-6-sol", Some(&entry("GPT-6 Sol (A)", true)));
        assert_eq!(named.sent, "openai/gpt-6-sol");
        assert_eq!(named.shown, "GPT-6 Sol (A)");
        assert_eq!(
            names("openai/gpt-6-sol", Some(&entry("", true))).shown,
            "openai/gpt-6-sol"
        );
        assert_eq!(names("gone", None).shown, "gone");
    }

    #[test]
    fn a_lease_lets_through_the_model_of_the_turn_and_no_other() {
        assert_eq!(lease_models("gpt-6-sol"), ["gpt-6-sol"]);
    }
}
