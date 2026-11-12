//! The options an entry carries: models, efforts, arrangement types and the places work may run in on a provider, and the limits of a host and of a template; what a person's entries allow.
//!
//! An entry in the Use row of a provider, a host or a template says what its holder may do with it,
//! and nothing else takes options. A person is let in by several entries at once (their own, a
//! group's, a role's, the work in a place). On a provider an entry is one grant: a person may do
//! something through the provider when one single entry of theirs allows all of it together, the
//! model, the effort, the arrangement type and the place ([`Wanted`], [`entry_allows`]). What is
//! offered (the models, efforts and types a dialog or the chooser lists) is, for what is asked, the
//! union of what the entries that allow it give. Of the entries of a host or a template the options
//! combine member by member as they always did, and of two numbers the larger counts. An entry that
//! sets nothing allows every model, effort, place and storage, and of the arrangement types only
//! those in which the credential stays where it is kept; a type in which it leaves is allowed by an
//! entry that names it. Being an owner, a manager, or a system admin on a resource of the system, is
//! unlimited in everything, the types included.
//!
//! This part also says which of several entries a turn is put down to for the provider's own
//! reports, and what a provider's models and a person's options leave of the effective allowlist.

use orch_access_contract::pages::{Entry, Holder};
use orch_access_contract::permissions::{
    Arrangement, HostUse, Options, Permission, ProviderUse, TemplateUse,
};
use orch_access_contract::whereabouts::Whereabouts;
use orch_agent_contract::discovery::ModelPattern;
use orch_db::queries::resources::ModelFacts;
use orch_types::{Id, Time};

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

/// Of two sets of options for one permission, what the two name together, member by member. It
/// describes what a person's entries name; for a provider it never decides whether they may do
/// something (see [`Wanted`]), since it lets the models of one entry go with the type or the place of
/// another. A set of another kind is ignored, which cannot happen for entries of one row.
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

/// What all of `entries`, which are entries of the row of `permission`, name together (see
/// [`either`]). Nobody let in by no entry has none to combine: that is nothing, not unlimited.
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

/// What a person wants to do through a provider, as far as it is known. What is not known does not
/// narrow: a question that carries no model asks about the entries that allow the place, and so on.
#[derive(Debug, Clone, Copy, Default)]
pub struct Wanted<'a> {
    /// The model, as the provider names it, with the groups of the provider's models it is in.
    pub model: Option<(&'a str, &'a [String])>,
    /// The reasoning effort.
    pub effort: Option<&'a str>,
    /// The service offering, with the groups it is in, which an entry's `services` allows apart
    /// from its models.
    pub service: Option<(&'a str, &'a [String])>,
    /// The arrangement type the work is to be done in.
    pub arrangement: Option<Arrangement>,
    /// Where the work is to run, for work that runs in an environment.
    pub at: Option<&'a Whereabouts>,
}

/// Whether one entry's options allow all of what is wanted together. This is the one decider of
/// whether somebody may do something through a provider: an entry is one grant, so what is
/// allowed is what a single entry allows, never what several allow member by member.
#[must_use]
pub fn entry_allows(options: &ProviderUse, wanted: &Wanted<'_>) -> bool {
    wanted
        .model
        .is_none_or(|(model, groups)| allows_model(options, model, groups))
        && wanted.service.is_none_or(|(service, groups)| {
            options
                .services
                .as_deref()
                .is_none_or(|patterns| ModelPattern::list_names(patterns, service, groups))
        })
        && wanted
            .effort
            .is_none_or(|effort| options.allows_effort(effort))
        && wanted
            .arrangement
            .is_none_or(|arrangement| options.allows_arrangement(arrangement))
        && wanted.at.is_none_or(|at| options.allows_where(at))
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

/// How near an entry is to a turn of a person: what decides which of several entries that allow it
/// the turn is put down to. The smaller is the nearer; of equals the one written first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rank {
    nearness: Nearness,
    created_at: Time,
    entry: Id,
}

impl Rank {
    /// The entry this is the rank of.
    #[must_use]
    pub const fn entry(&self) -> Id {
        self.entry
    }
}

/// The rank of `entry` for a turn of `person` on a thread whose path is `path` (the nodes from its
/// root org down to it).
#[must_use]
pub fn rank(person: Id, path: &[Id], entry: &Entry) -> Rank {
    let nearness = match entry.holder {
        Holder::User { id } if id == person => Nearness::Person,
        Holder::Node { id } => Nearness::Work {
            distance: path
                .iter()
                .rev()
                .position(|node| *node == id)
                .unwrap_or(usize::MAX),
        },
        _ => Nearness::Collective,
    };
    Rank {
        nearness,
        created_at: entry.created_at,
        entry: entry.id,
    }
}

/// The entry a turn of `person` on a thread whose path is `path` is put down to, among `entries`
/// that let it run: what the provider's own reports and a lease name. The entry that names the
/// person comes first, then a group or a role of theirs, then those that name the work in a place
/// from the nearest place upward; of equals the one written first.
#[must_use]
pub fn attribution(person: Id, path: &[Id], entries: &[&Entry]) -> Option<Id> {
    entries
        .iter()
        .map(|entry| rank(person, path, entry))
        .min()
        .map(|rank| rank.entry)
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

/// One enabled service must satisfy its Use entry and every applying place list.
/// Model lists never narrow services; the caller supplies the actual offering's groups.
#[must_use]
pub fn allows_service(
    options: &ProviderUse,
    enabled: bool,
    service: &str,
    groups: &[String],
    place_setting: Option<&[String]>,
) -> bool {
    enabled
        && options
            .services
            .as_deref()
            .is_none_or(|patterns| ModelPattern::list_names(patterns, service, groups))
        && place_setting.is_none_or(|patterns| ModelPattern::list_names(patterns, service, groups))
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

/// The arrangement types in which work may use a provider through `entries`, which are the entries
/// that allow what is wanted: those its owner allows, within those any of the entries names, or where
/// one names none within those in which the credential stays where it is kept. A use that is held as
/// an owner or a manager (`unlimited`) keeps every type the owner allows. An entry can only narrow
/// what the owner allows.
#[must_use]
pub fn allowed_arrangements(owner_allows: &[String], entries: &[&ProviderUse]) -> Vec<Arrangement> {
    owner_allows
        .iter()
        .filter_map(|word| word.parse::<Arrangement>().ok())
        .filter(|arrangement| {
            entries
                .iter()
                .any(|options| options.allows_arrangement(*arrangement))
        })
        .collect()
}

/// The arrangement types in which work may use a provider through `entries`, whatever its owners
/// allow: those any of the entries allows.
#[must_use]
pub fn arrangements_of(entries: &[&ProviderUse]) -> Vec<Arrangement> {
    Arrangement::ALL
        .iter()
        .copied()
        .filter(|arrangement| {
            entries
                .iter()
                .any(|options| options.allows_arrangement(*arrangement))
        })
        .collect()
}

/// The models a person may run on a provider through `entries`, which are the entries that allow
/// what is wanted: those it has enabled that any of them allows, and the thread's place allows.
#[must_use]
pub fn offered_models<'m>(
    enabled: &'m [ModelFacts],
    entries: &[&ProviderUse],
    thread_setting: Option<&[String]>,
) -> Vec<&'m str> {
    enabled
        .iter()
        .filter(|model| model.enabled)
        .filter(|model| {
            entries
                .iter()
                .any(|options| allows_model(options, &model.model_name, &model.groups))
        })
        .filter(|model| {
            thread_setting.is_none_or(|allowed| {
                ModelPattern::list_names(allowed, &model.model_name, &model.groups)
            })
        })
        .map(|model| model.model_name.as_str())
        .collect()
}

/// The efforts a person may run on `model` through `entries`, which are the entries that allow what
/// is wanted: those it lists that any of them allows.
#[must_use]
pub fn offered_efforts<'m>(model: &'m ModelFacts, entries: &[&ProviderUse]) -> Vec<&'m str> {
    model
        .efforts
        .iter()
        .map(String::as_str)
        .filter(|effort| entries.iter().any(|options| options.allows_effort(effort)))
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
            renewed_at: None,
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
    fn services_require_enablement_and_both_service_lists_while_model_limits_stay_separate() {
        let groups = vec!["web".into()];
        let model_only = ProviderUse {
            models: Some(vec!["@small".into()]),
            ..ProviderUse::default()
        };
        assert!(allows_service(&model_only, true, "search", &groups, None));
        assert!(!allows_service(&model_only, false, "search", &groups, None));
        let service_only = ProviderUse {
            services: Some(vec!["@web".into(), "image-*".into()]),
            ..ProviderUse::default()
        };
        assert!(allows_service(&service_only, true, "search", &groups, None));
        assert!(allows_service(
            &service_only,
            true,
            "image-large",
            &[],
            None
        ));
        assert!(!allows_service(&service_only, true, "other", &[], None));
        assert!(!allows_service(
            &service_only,
            true,
            "search",
            &groups,
            Some(&["images".into()])
        ));
        assert!(!allows_service(
            &service_only,
            true,
            "search",
            &groups,
            Some(&[])
        ));
        assert!(allows_service(
            &service_only,
            true,
            "search",
            &groups,
            Some(&["search".into()])
        ));
        let models = [model("large", &[], true)];
        assert_eq!(allowed_models(&models, &service_only, None), ["large"]);
        let none = ProviderUse {
            services: Some(Vec::new()),
            ..ProviderUse::default()
        };
        assert!(!allows_service(&none, true, "search", &groups, None));
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

    fn provider_use(options: serde_json::Value) -> ProviderUse {
        match Options::parse(Permission::ProviderUse, &options).unwrap() {
            Options::ProviderUse(options) => options,
            other => panic!("the use of a provider takes options: {other:?}"),
        }
    }

    /// Whether one single entry of the person allows all of what is wanted.
    fn one_allows(entries: &[ProviderUse], wanted: &Wanted<'_>) -> bool {
        entries.iter().any(|entry| entry_allows(entry, wanted))
    }

    fn in_model(name: &str) -> Wanted<'_> {
        Wanted {
            model: Some((name, &[])),
            ..Wanted::default()
        }
    }

    fn at(environment: Id) -> Whereabouts {
        Whereabouts {
            environment: Some(environment),
            ..Whereabouts::default()
        }
    }

    #[test]
    fn two_entries_that_each_allow_a_different_model_anywhere_allow_both_models() {
        let entries = [
            provider_use(json!({ "models": ["big"] })),
            provider_use(json!({ "models": ["small"] })),
        ];
        assert!(one_allows(&entries, &in_model("big")));
        assert!(one_allows(&entries, &in_model("small")));
        assert!(!one_allows(&entries, &in_model("huge")));
        let (here, there) = (at(new_id()), at(new_id()));
        for place in [&here, &there] {
            let wanted = Wanted {
                at: Some(place),
                ..in_model("big")
            };
            assert!(one_allows(&entries, &wanted), "anywhere");
        }
    }

    #[test]
    fn a_narrowed_entry_and_an_unnarrowed_one_give_the_unnarrowed_one_its_own_everywhere_and_the_narrowed_one_its_place()
     {
        let (e1, e2) = (new_id(), new_id());
        let place = |id: Id| json!({ "kind": "environment", "id": id });
        let entries = [
            provider_use(json!({ "models": ["small"] })),
            provider_use(json!({ "models": ["big"], "where": [place(e1)] })),
        ];
        let (first, second) = (at(e1), at(e2));
        let wanted = |model: &'static str, place: &Whereabouts| {
            one_allows(
                &entries,
                &Wanted {
                    at: Some(place),
                    ..in_model(model)
                },
            )
        };
        assert!(wanted("small", &first));
        assert!(wanted("small", &second));
        assert!(wanted("big", &first));
        assert!(
            !wanted("big", &second),
            "the model of the entry that names a place is the place's alone: the place of one entry does not join the models of another"
        );
    }

    #[test]
    fn the_copy_of_an_entry_that_names_a_place_is_not_allowed_elsewhere_beside_an_entry_that_names_nothing()
     {
        let (e1, e2) = (new_id(), new_id());
        let entries = [
            provider_use(json!({})),
            provider_use(json!({
                "arrangements": ["credential_shared"],
                "where": [{ "kind": "environment", "id": e1 }]
            })),
        ];
        fn copy(place: &Whereabouts) -> Wanted<'_> {
            Wanted {
                arrangement: Some(Arrangement::CredentialShared),
                at: Some(place),
                ..Wanted::default()
            }
        }
        fn staying(place: &Whereabouts) -> Wanted<'_> {
            Wanted {
                arrangement: Some(Arrangement::Separated),
                at: Some(place),
                ..Wanted::default()
            }
        }
        assert!(one_allows(&entries, &copy(&at(e1))));
        assert!(
            !one_allows(&entries, &copy(&at(e2))),
            "no entry names the copy anywhere but in the first environment"
        );
        assert!(one_allows(&entries, &staying(&at(e2))));
        // The types offered for the place are those of the entries that allow it.
        let offered = |place: &Whereabouts| {
            let wanted = Wanted {
                at: Some(place),
                ..Wanted::default()
            };
            let allowing: Vec<&ProviderUse> = entries
                .iter()
                .filter(|entry| entry_allows(entry, &wanted))
                .collect();
            arrangements_of(&allowing)
        };
        assert_eq!(
            offered(&at(e1)),
            [
                Arrangement::Separated,
                Arrangement::KeySeparated,
                Arrangement::CredentialShared
            ]
        );
        assert_eq!(
            offered(&at(e2)),
            [Arrangement::Separated, Arrangement::KeySeparated]
        );
    }

    #[test]
    fn the_model_the_effort_the_type_and_the_place_have_to_be_allowed_by_one_entry_together() {
        let (e1, e2) = (new_id(), new_id());
        let entries = [
            provider_use(json!({ "models": ["small"], "efforts": ["low"] })),
            provider_use(json!({
                "models": ["big"], "efforts": ["high"],
                "arrangements": ["credential_shared"],
                "where": [{ "kind": "environment", "id": e1 }]
            })),
        ];
        let together = |model: &str, effort: &str, arrangement, place: &Whereabouts| {
            one_allows(
                &entries,
                &Wanted {
                    model: Some((model, &[])),
                    effort: Some(effort),
                    arrangement: Some(arrangement),
                    at: Some(place),
                    ..Wanted::default()
                },
            )
        };
        let (first, second) = (at(e1), at(e2));
        assert!(together("small", "low", Arrangement::Separated, &second));
        assert!(together(
            "big",
            "high",
            Arrangement::CredentialShared,
            &first
        ));
        assert!(
            !together("small", "high", Arrangement::Separated, &second),
            "the model of one entry with the effort of another"
        );
        assert!(
            !together("big", "high", Arrangement::Separated, &first),
            "the model of the second entry with the type of the first"
        );
        assert!(!together(
            "big",
            "high",
            Arrangement::CredentialShared,
            &second
        ));
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
    fn an_entry_that_names_no_type_allows_only_those_in_which_the_credential_stays_and_an_owner_keeps_all()
     {
        let owner = [
            "separated".to_owned(),
            "key_separated".to_owned(),
            "credential_shared".to_owned(),
            "handed_over".to_owned(),
        ];
        assert_eq!(
            allowed_arrangements(&owner, &[&ProviderUse::default()]),
            [Arrangement::Separated, Arrangement::KeySeparated],
            "left out gives the staying types"
        );
        let named = ProviderUse {
            arrangements: Some(vec![Arrangement::Separated, Arrangement::CredentialShared]),
            ..ProviderUse::default()
        };
        assert_eq!(
            allowed_arrangements(&owner, &[&named]),
            [Arrangement::Separated, Arrangement::CredentialShared],
            "named gives the type that leaves, and nothing else is added"
        );
        assert_eq!(
            allowed_arrangements(&owner, &[&ProviderUse::unlimited()]),
            [
                Arrangement::Separated,
                Arrangement::KeySeparated,
                Arrangement::CredentialShared,
                Arrangement::HandedOver
            ],
            "whoever uses the provider as its owner keeps every type it allows"
        );
        let left_out = ProviderUse::default();
        let sharing = ProviderUse {
            arrangements: Some(vec![Arrangement::CredentialShared]),
            ..ProviderUse::default()
        };
        assert_eq!(
            allowed_arrangements(&owner, &[&left_out, &sharing]),
            [
                Arrangement::Separated,
                Arrangement::KeySeparated,
                Arrangement::CredentialShared
            ],
            "an entry that leaves the type out still gives the staying types beside one that names a copy"
        );
    }

    #[test]
    fn an_entry_narrows_the_arrangements_the_owner_allows_and_never_widens_them() {
        let owner = ["separated".to_owned(), "key_separated".to_owned()];
        assert_eq!(
            allowed_arrangements(&owner, &[&ProviderUse::default()]),
            [Arrangement::Separated, Arrangement::KeySeparated]
        );
        let narrowed = ProviderUse {
            arrangements: Some(vec![Arrangement::KeySeparated, Arrangement::HandedOver]),
            ..ProviderUse::default()
        };
        assert_eq!(
            allowed_arrangements(&owner, &[&narrowed]),
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
