//! The options an entry carries: models, efforts and arrangement types on a provider, and the limits of a host and of a template; how several entries combine.
//!
//! An entry in the Use row of a provider, a host or a template says what its holder may do with it,
//! and nothing else takes options. A person is let in by several entries at once (their own, a
//! group's, a role's, the work in a place), and they combine permissively: a model, an effort, an
//! arrangement type or a storage that any of them allows is allowed, and of two numbers the
//! larger counts. An entry that sets nothing allows everything, so one such entry makes the
//! person's use unlimited. Being an owner, or a system admin on a resource of the system, is
//! unlimited too.
//!
//! This part also says which of several entries a turn is put down to for the provider's own
//! reports, and what a provider's models and a person's options leave of the effective allowlist.

use orch_access_contract::pages::{Entry, Holder};
use orch_access_contract::permissions::{
    Arrangement, HostUse, Options, Permission, ProviderUse, TemplateUse,
};
use orch_agent_contract::discovery::ModelPattern;
use orch_db::queries::resources::ModelFacts;
use orch_types::Id;

use crate::Error;

/// What a person's use of a resource is limited to when no entry limits it: the options as they
/// are for a permission that takes some, and none for one that takes none.
#[must_use]
pub fn unlimited(permission: Permission) -> Options {
    match permission {
        Permission::ProviderUse => Options::ProviderUse(ProviderUse::default()),
        Permission::HostUse => Options::HostUse(HostUse::default()),
        Permission::TemplateUse => Options::TemplateUse(TemplateUse::default()),
        _ => Options::None,
    }
}

/// Reads the options an entry stores for the row of `permission`. What was stored passed the
/// check when it was written, so a fault here is a fault of the stored data.
pub fn read(permission: Permission, entry: &Entry) -> Result<Options, Error> {
    Options::parse(permission, &entry.options).map_err(|faults| {
        let what = faults.first().map_or_else(String::new, |fault| {
            format!("{}: {}", fault.path, fault.message)
        });
        Error::Stored(format!(
            "the options of entry {} do not read for {permission}: {what}",
            entry.id
        ))
    })
}

/// Of two sets of options for one permission, what somebody has whom both let in. A set of another
/// kind is ignored, which cannot happen for entries of one row.
#[must_use]
pub fn either(one: &Options, other: &Options) -> Options {
    match (one, other) {
        (Options::ProviderUse(one), Options::ProviderUse(other)) => {
            Options::ProviderUse(one.or(other))
        }
        (Options::HostUse(one), Options::HostUse(other)) => Options::HostUse(one.or(other)),
        (Options::TemplateUse(one), Options::TemplateUse(other)) => {
            Options::TemplateUse(one.or(other))
        }
        (one, _) => one.clone(),
    }
}

/// The options of somebody whom all of `entries` let in, which are entries of the row of
/// `permission`. Nobody let in by no entry has none to combine: that is nothing, not unlimited.
pub fn combine<'a>(
    permission: Permission,
    entries: impl IntoIterator<Item = &'a Entry>,
) -> Result<Option<Options>, Error> {
    let mut combined: Option<Options> = None;
    for entry in entries {
        let read = read(permission, entry)?;
        combined = Some(match combined {
            None => read,
            Some(so_far) => either(&so_far, &read),
        });
    }
    Ok(combined)
}

/// The use of a provider that a set of options comes to; unlimited for options of another kind.
#[must_use]
pub fn of_provider(options: &Options) -> ProviderUse {
    match options {
        Options::ProviderUse(options) => options.clone(),
        _ => ProviderUse::default(),
    }
}

/// How an entry stands to the person who is let in by it, for deciding which one a turn is put down
/// to: the entry that names the person, then one that names a group or a role of theirs or
/// everyone, then one that names the work in a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Nearness {
    /// It names the person.
    Person,
    /// It names a group of theirs, a role they hold, or everyone.
    Collective,
    /// It names the work in a place; the nearer the place is to the thread, the nearer the entry.
    Work { distance: usize },
}

/// The entry a turn of `person` on a thread whose path is `path` (the nodes from its root org
/// down to it) is put down to, among `entries` that let it run: what the provider's own reports
/// and a lease name. The entry that names the person comes first, then a group or a role of
/// theirs, then those that name the work in a place from the nearest place upward; of equals the
/// one written first.
#[must_use]
pub fn attribution(person: Id, path: &[Id], entries: &[&Entry]) -> Option<Id> {
    let rank = |entry: &Entry| -> Nearness {
        match entry.holder {
            Holder::User { id } if id == person => Nearness::Person,
            Holder::Node { id } => Nearness::Work {
                distance: path
                    .iter()
                    .rev()
                    .position(|node| *node == id)
                    .unwrap_or(usize::MAX),
            },
            _ => Nearness::Collective,
        }
    };
    entries
        .iter()
        .min_by_key(|entry| (rank(entry), entry.created_at, entry.id))
        .map(|entry| entry.id)
}

/// Whether a person whose options are `options` may run `model` of the provider whose entry for it
/// is in `groups`. The `models` option names models the way every list of models does: by name, by a
/// family (`gpt-6*`), and by a group of the provider's models (`@small`), the provider being the one
/// the entry is on. An option that is not set allows every model.
#[must_use]
pub fn allows_model(options: &ProviderUse, model: &str, groups: &[String]) -> bool {
    options
        .models
        .as_deref()
        .is_none_or(|patterns| ModelPattern::list_names(patterns, model, groups))
}

/// The models a person may run on a provider: those it has enabled, within the models their
/// options allow and within the models the thread's place allows (the setting `models.allowed`
/// resolved at the thread, where one is set). Both are lists of the same kind: names, families
/// and groups of the provider's models.
#[must_use]
pub fn allowed_models<'m>(
    enabled: &'m [ModelFacts],
    options: &ProviderUse,
    thread_setting: Option<&[String]>,
) -> Vec<&'m str> {
    enabled
        .iter()
        .filter(|model| model.enabled)
        .filter(|model| allows_model(options, &model.model_name, &model.groups))
        .filter(|model| {
            thread_setting.is_none_or(|allowed| {
                ModelPattern::list_names(allowed, &model.model_name, &model.groups)
            })
        })
        .map(|model| model.model_name.as_str())
        .collect()
}

/// The efforts a person may run on `model`: those it lists, within the efforts their options
/// allow.
#[must_use]
pub fn allowed_efforts<'m>(model: &'m ModelFacts, options: &ProviderUse) -> Vec<&'m str> {
    model
        .efforts
        .iter()
        .map(String::as_str)
        .filter(|effort| options.allows_effort(effort))
        .collect()
}

/// The arrangement types in which work done through an entry may use a provider: those its owner
/// allows, within those the entry names. The entry can only narrow what the owner allows.
#[must_use]
pub fn allowed_arrangements(owner_allows: &[String], options: &ProviderUse) -> Vec<Arrangement> {
    owner_allows
        .iter()
        .filter_map(|word| word.parse::<Arrangement>().ok())
        .filter(|arrangement| options.allows_arrangement(*arrangement))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_access_contract::pages::EntrySource;
    use orch_types::{Time, new_id};
    use serde_json::json;

    fn entry(holder: Holder, options: serde_json::Value, at: &str) -> Entry {
        Entry {
            id: new_id(),
            holder,
            options,
            expires_at: None,
            note: String::new(),
            source: EntrySource::Page,
            created_by: None,
            created_at: at.parse::<Time>().unwrap(),
            removed_at: None,
            revision: 1,
        }
    }

    fn model(name: &str, efforts: &[&str], enabled: bool) -> ModelFacts {
        ModelFacts {
            model_name: name.to_owned(),
            enabled,
            efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
            groups: Vec::new(),
        }
    }

    fn grouped(name: &str, groups: &[&str]) -> ModelFacts {
        ModelFacts {
            groups: groups.iter().map(|group| (*group).to_owned()).collect(),
            ..model(name, &[], true)
        }
    }

    #[test]
    fn an_entry_that_names_a_group_lets_its_holder_run_the_models_the_providers_managers_put_in_it()
    {
        let models = [
            grouped("gpt-6-sol", &["frontier"]),
            grouped("gpt-6-luna", &["small"]),
            grouped("gpt-5.6-sol", &[]),
        ];
        let entry = entry(
            Holder::Everyone,
            json!({ "models": ["@frontier", "gpt-5*"] }),
            "2026-10-01T00:00:00Z",
        );
        let Some(Options::ProviderUse(options)) =
            combine(Permission::ProviderUse, [&entry]).unwrap()
        else {
            panic!("an entry of the Use row of a provider");
        };
        assert_eq!(
            allowed_models(&models, &options, None),
            ["gpt-6-sol", "gpt-5.6-sol"],
            "a group and a family, and not the model that is in another group"
        );
        assert_eq!(
            allowed_models(
                &models,
                &options,
                Some(&["@small".to_owned(), "gpt-6-sol".to_owned()])
            ),
            ["gpt-6-sol"],
            "the place's own list is a list of the same kind, and both have to allow a model"
        );
        assert!(allows_model(&ProviderUse::default(), "anything", &[]));
        assert!(!allows_model(&options, "gpt-6-luna", &["small".to_owned()]));
    }

    #[test]
    fn two_entries_that_limit_models_let_a_person_run_the_models_of_either() {
        let one = entry(
            Holder::Everyone,
            json!({ "models": ["big"] }),
            "2026-10-01T00:00:00Z",
        );
        let two = entry(
            Holder::Everyone,
            json!({ "models": ["small"] }),
            "2026-10-01T00:00:00Z",
        );
        let Some(Options::ProviderUse(both)) =
            combine(Permission::ProviderUse, [&one, &two]).unwrap()
        else {
            panic!("two entries combine into the use of a provider");
        };
        assert_eq!(
            both.models.as_deref(),
            Some(&["big".to_owned(), "small".to_owned()][..])
        );
        assert!(
            both.allows_model("big") && both.allows_model("small") && !both.allows_model("huge")
        );
    }

    #[test]
    fn an_entry_that_sets_nothing_makes_the_use_unlimited_whatever_another_limits() {
        let limited = entry(
            Holder::Everyone,
            json!({ "models": ["big"], "efforts": ["low"] }),
            "2026-10-01T00:00:00Z",
        );
        let open = entry(Holder::Everyone, json!({}), "2026-10-01T00:00:00Z");
        let Some(Options::ProviderUse(use_)) =
            combine(Permission::ProviderUse, [&limited, &open]).unwrap()
        else {
            panic!("combined");
        };
        assert_eq!(use_, ProviderUse::default());
    }

    #[test]
    fn of_two_numbers_the_larger_counts_and_a_cap_left_out_is_none() {
        let two = entry(
            Holder::Everyone,
            json!({ "max_environments": 2 }),
            "2026-10-01T00:00:00Z",
        );
        let five = entry(
            Holder::Everyone,
            json!({ "max_environments": 5 }),
            "2026-10-01T00:00:00Z",
        );
        let none = entry(Holder::Everyone, json!({}), "2026-10-01T00:00:00Z");
        let cap = |entries: &[&Entry]| match combine(Permission::HostUse, entries.iter().copied())
            .unwrap()
        {
            Some(Options::HostUse(host)) => host.max_environments,
            other => panic!("{other:?}"),
        };
        assert_eq!(cap(&[&two, &five]), Some(5));
        assert_eq!(cap(&[&two]), Some(2));
        assert_eq!(
            cap(&[&two, &none]),
            None,
            "an entry with no cap lets in without one"
        );
    }

    #[test]
    fn nobody_let_in_by_no_entry_has_no_options_to_combine() {
        assert_eq!(combine(Permission::ProviderUse, []).unwrap(), None);
    }

    #[test]
    fn a_turn_is_put_down_to_the_entry_that_names_the_person_then_a_group_then_the_nearest_place() {
        let person = new_id();
        let (root, project, topic, thread) = (new_id(), new_id(), new_id(), new_id());
        let path = [root, project, topic, thread];
        let by_root = entry(Holder::Node { id: root }, json!({}), "2026-10-01T00:00:00Z");
        let by_topic = entry(
            Holder::Node { id: topic },
            json!({}),
            "2026-10-02T00:00:00Z",
        );
        let by_group = entry(
            Holder::Group { id: new_id() },
            json!({}),
            "2026-10-03T00:00:00Z",
        );
        let by_name = entry(
            Holder::User { id: person },
            json!({}),
            "2026-10-04T00:00:00Z",
        );
        let all = [&by_root, &by_topic, &by_group, &by_name];
        assert_eq!(attribution(person, &path, &all), Some(by_name.id));
        assert_eq!(attribution(person, &path, &all[..3]), Some(by_group.id));
        assert_eq!(
            attribution(person, &path, &all[..2]),
            Some(by_topic.id),
            "the nearer place"
        );
        assert_eq!(attribution(person, &path, &all[..1]), Some(by_root.id));
        assert_eq!(attribution(person, &path, &[]), None);
    }

    #[test]
    fn the_allowlist_is_the_enabled_models_within_the_options_and_within_the_threads_setting() {
        let models = [
            model("big", &["low", "high"], true),
            model("small", &[], true),
            model("old", &[], false),
        ];
        let nothing = ProviderUse::default();
        assert_eq!(
            allowed_models(&models, &nothing, None),
            ["big", "small"],
            "not the disabled one"
        );
        let one = ProviderUse {
            models: Some(vec!["small".into(), "old".into()]),
            ..ProviderUse::default()
        };
        assert_eq!(allowed_models(&models, &one, None), ["small"]);
        let setting = ["big".to_owned()];
        assert_eq!(allowed_models(&models, &nothing, Some(&setting)), ["big"]);
        assert!(
            allowed_models(&models, &one, Some(&setting)).is_empty(),
            "the narrower of both"
        );
        let efforts = ProviderUse {
            efforts: Some(vec!["high".into(), "max".into()]),
            ..ProviderUse::default()
        };
        assert_eq!(allowed_efforts(&models[0], &nothing), ["low", "high"]);
        assert_eq!(
            allowed_efforts(&models[0], &efforts),
            ["high"],
            "only what the model lists"
        );
    }

    #[test]
    fn an_entry_narrows_the_arrangements_the_owner_allows_and_never_widens_them() {
        let owner = ["separated".to_owned(), "key_separated".to_owned()];
        assert_eq!(
            allowed_arrangements(&owner, &ProviderUse::default()),
            [Arrangement::Separated, Arrangement::KeySeparated]
        );
        let narrowed = ProviderUse {
            arrangements: Some(vec![Arrangement::KeySeparated, Arrangement::HandedOver]),
            ..ProviderUse::default()
        };
        assert_eq!(
            allowed_arrangements(&owner, &narrowed),
            [Arrangement::KeySeparated]
        );
    }

    #[test]
    fn options_that_do_not_read_are_a_fault_of_the_stored_data() {
        let bad = entry(
            Holder::Everyone,
            json!({ "models": "big" }),
            "2026-10-01T00:00:00Z",
        );
        assert!(matches!(
            combine(Permission::ProviderUse, [&bad]),
            Err(Error::Stored(_))
        ));
    }
}
