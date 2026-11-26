//! Stored policy ancestors remain bounds; shipped defaults remain defaults.
use super::*;
use crate::{
    provenance::SourceScope,
    registry::{Class, get, keys},
};
use orch_types::{Id, new_id};
use serde_json::json;

fn definition(key: &str) -> &'static SettingDef {
    get(key).unwrap_or_else(|| panic!("the policy fixture must name a registered key"))
}
fn pair(key: &str, parent: &Json, child: &Json, locked: bool) -> Resolved<Id> {
    resolve(
        definition(key),
        &[
            Layer::at(Scope::System, None, parent, locked),
            Layer::at(Scope::Org, Some(new_id()), child, false),
        ],
    )
}

#[test]
fn numeric_policy_limits_retain_the_stricter_ancestor_and_keep_zero_meanings_distinct() {
    for locked in [false, true] {
        let resolved = pair(keys::THREAD_SIDE_MAX, &json!(4), &json!(8), locked);
        assert_eq!(resolved.value, Some(json!(4)));
        assert_eq!(resolved.source.scope, SourceScope::System);
        assert!(resolved.locked_by.is_none());
        let tighter = pair(keys::THREAD_SIDE_MAX, &json!(8), &json!(4), locked);
        assert_eq!(tighter.value, Some(json!(4)));
        assert_eq!(tighter.source.scope, SourceScope::Org);
        for key in [
            keys::THREAD_GOAL_MAX_TURNS,
            keys::THREAD_GOAL_MAX_MINUTES,
            keys::NOTIFICATIONS_PERSON_RATE_PER_HOUR,
        ] {
            assert_eq!(
                pair(key, &json!(10), &json!(0), locked).value,
                Some(json!(10))
            );
            assert_eq!(
                pair(key, &json!(0), &json!(10), locked).value,
                Some(json!(10))
            );
            assert_eq!(pair(key, &Json::Null, &json!(0), locked).value, None);
        }
        assert_eq!(
            pair(keys::CALLS_REPEAT_LIMIT, &json!(2), &json!(0), locked).value,
            Some(json!(0))
        );
        assert_eq!(
            pair(
                keys::NOTIFICATIONS_PERSON_NOTE_CHARS,
                &json!(20),
                &json!(0),
                locked
            )
            .value,
            Some(json!(0))
        );
        assert_eq!(
            pair(
                keys::SEARCH_SEMANTIC_MAX_DISK_SHARE,
                &json!(100),
                &json!(0),
                locked
            )
            .value,
            Some(json!(0))
        );
        assert_eq!(
            pair(
                keys::SEARCH_SEMANTIC_MAX_DISK_SHARE,
                &json!(20),
                &Json::Null,
                locked
            )
            .value,
            Some(json!(20))
        );
        assert_eq!(
            pair(keys::IMPORT_MAX_BYTES, &json!(1000), &Json::Null, locked).value,
            Some(json!(1000))
        );
    }
    assert_eq!(
        resolve::<Id>(definition(keys::THREAD_GOAL_MAX_TURNS), &[]).value,
        None
    );
    let unlimited = Json::Null;
    assert_eq!(
        resolve::<Id>(
            definition(keys::SEARCH_SEMANTIC_MAX_DISK_SHARE),
            &[Layer::at(Scope::Org, None, &unlimited, false)]
        )
        .value,
        None,
        "an explicit unbounded policy does not restore a shipped finite cap"
    );
}

#[test]
fn decimal_policy_limits_preserve_precision_and_equal_local_values_remain_effective() {
    let parent = json!("9007199254740993.0000000001");
    let loose = json!("9007199254740993.0000000002");
    let result = pair(keys::THREAD_GOAL_MAX_CREDITS, &parent, &loose, false);
    assert_eq!(result.value, Some(parent));
    assert_eq!(result.source.scope, SourceScope::System);
    assert_eq!(
        pair(
            keys::THREAD_GOAL_MAX_CREDITS,
            &json!("2.5"),
            &json!("0"),
            false
        )
        .value,
        Some(json!("2.5"))
    );
    assert_eq!(
        pair(
            keys::THREAD_GOAL_MAX_CREDITS,
            &json!("0"),
            &json!("00.000"),
            false
        )
        .value,
        None
    );
    let equal = pair(
        keys::THREAD_GOAL_MAX_CREDITS,
        &json!("01.000"),
        &json!("1"),
        false,
    );
    assert_eq!(equal.value, Some(json!("1")));
    assert_eq!(equal.source.scope, SourceScope::Org);
}

#[test]
fn minimum_retention_uses_the_largest_value_without_a_hidden_default_lock() {
    for locked in [false, true] {
        let older = pair(keys::ASK_RETENTION_MIN_DAYS, &json!(30), &json!(1), locked);
        assert_eq!(older.value, Some(json!(30)));
        assert_eq!(older.source.scope, SourceScope::System);
        assert!(older.locked_by.is_none());
        let stronger = pair(keys::ASK_RETENTION_MIN_DAYS, &json!(1), &json!(30), locked);
        assert_eq!(stronger.value, Some(json!(30)));
        assert_eq!(stronger.source.scope, SourceScope::Org);
    }
}

#[test]
fn permissions_need_all_scopes_and_protections_need_any_scope_even_under_a_stored_lock() {
    for locked in [false, true] {
        for (key, strict) in [
            (keys::TERMINAL_ENABLED, false),
            (keys::SHARING_ALLOW_COPY_OUT, false),
            (keys::REDACTION_MASK_SECRETS, true),
        ] {
            let result = pair(key, &json!(strict), &json!(!strict), locked);
            assert_eq!(result.value, Some(json!(strict)));
            assert_eq!(result.source.scope, SourceScope::System);
            assert!(result.locked_by.is_none());
            let tighter = pair(key, &json!(!strict), &json!(strict), locked);
            assert_eq!(tighter.value, Some(json!(strict)));
            assert_eq!(tighter.source.scope, SourceScope::Org);
        }
    }
    for (key, value) in [
        (keys::PROVIDERS_ALLOW_HTTP_UPSTREAMS, true),
        (keys::REDACTION_MASK_SECRETS, false),
    ] {
        let row = json!(value);
        assert_eq!(
            resolve::<Id>(definition(key), &[Layer::at(Scope::Org, None, &row, false)]).value,
            Some(row),
            "a shipped boolean is a default rather than a hidden policy cap"
        );
    }
}

#[test]
fn supplied_defaults_do_not_become_hidden_policy_or_parameter_bounds() {
    let (off, on) = (json!(false), json!(true));
    let policy = definition(keys::PROVIDERS_ALLOW_HTTP_UPSTREAMS);
    let layers = [
        Layer::<Id>::supplied(&off),
        Layer::at(Scope::Org, Some(new_id()), &on, false),
    ];
    assert_eq!(resolve(policy, &layers).value, Some(on.clone()));
    let (supplied, local) = (json!(4), json!(16));
    let layers = [
        Layer::<Id>::supplied(&supplied),
        Layer::at(Scope::Org, Some(new_id()), &local, false),
    ];
    let parameter = definition(keys::AGENTS_SUBAGENTS_AT_ONCE);
    assert_eq!(resolve(parameter, &layers).value, Some(local.clone()));
    let fallback = resolve(parameter, &layers[..1]);
    assert_eq!(fallback.value, Some(supplied.clone()));
    assert_eq!(fallback.source.scope, SourceScope::Environment);
}

#[test]
fn ordered_policy_choices_use_the_registry_order_and_an_unlocked_parent_still_bounds_them() {
    for (key, loose, strict) in [
        (keys::AUTH_SECOND_STEP, "optional", "required"),
        (keys::CHAT_DIRECT, "members", "off"),
        (keys::CHAT_GROUP_MENTIONS, "writers", "managers"),
        (keys::THREAD_INPUTS_WITHOUT_USE, "allowed", "refused"),
        (keys::THREAD_PLAN_ANSWERED_BY, "writers", "managers"),
    ] {
        let (loose, strict) = (json!(loose), json!(strict));
        for locked in [false, true] {
            let result = pair(key, &strict, &loose, locked);
            assert_eq!(result.value, Some(strict.clone()));
            assert_eq!(result.source.scope, SourceScope::System);
            assert!(result.locked_by.is_none());
            assert_eq!(
                pair(key, &loose, &strict, locked).source.scope,
                SourceScope::Org
            );
        }
    }
    assert_eq!(
        pair(
            keys::THREAD_INPUTS_WITHOUT_USE,
            &json!("allowed"),
            &json!("held"),
            false
        )
        .value,
        Some(json!("held"))
    );
    assert_eq!(
        pair(
            keys::THREAD_INPUTS_WITHOUT_USE,
            &json!("held"),
            &json!("refused"),
            false
        )
        .value,
        Some(json!("refused"))
    );
}

#[test]
fn allowed_lists_keep_every_parent_check_and_name_the_actual_tightening_source() {
    let upper = json!(["link"]);
    let ineffective = json!(["link", "code"]);
    let result = pair(keys::INVITATIONS_FORMS, &upper, &ineffective, false);
    assert_eq!(result.value, Some(upper.clone()));
    assert_eq!(result.source.scope, SourceScope::System);
    assert_eq!(
        result.parts.len(),
        2,
        "all native parent restrictions remain available"
    );
    assert_eq!(
        pair(keys::INVITATIONS_FORMS, &ineffective, &upper, true)
            .source
            .scope,
        SourceScope::Org
    );
    assert_eq!(
        pair(keys::INVITATIONS_FORMS, &upper, &json!(["code"]), false).value,
        Some(json!([]))
    );
}

#[test]
fn preferences_and_parameters_keep_their_existing_replace_choices() {
    for key in [
        keys::CALLS_RULES,
        keys::CALLS_RETURN,
        keys::CALLS_NOTIFY_ON_STOP,
        keys::THREAD_SWITCH_HISTORY,
        keys::SHARING_ON_LINK,
        keys::USAGE_CATALOGUE,
        keys::USAGE_CATALOGUE_SCALE,
        keys::ASK_PAID_BY,
        keys::INVITATIONS_DEFAULT_MINUTES,
        keys::ENV_REMOVED_KEEP_DAYS,
        keys::RETENTION_ARCHIVE_IDLE_DAYS,
        keys::RETENTION_DELETE_ARCHIVED_DAYS,
        keys::RETENTION_AGENT_HISTORY_IDLE_DAYS,
        keys::CALLS_REPEAT_WINDOW,
    ] {
        assert_eq!(definition(key).merge, Merge::Replace, "{key}");
    }
    assert_eq!(definition(keys::AGENT_MODEL).class, Class::Parameter);
    assert_eq!(definition(keys::AGENT_MODEL).merge, Merge::Replace);
    assert_eq!(definition(keys::REDACTION_PATTERNS).merge, Merge::Union);
}

#[test]
fn writing_a_ceiling_uses_the_same_precision_and_unbounded_spellings_as_reading_it() {
    let credits = definition(keys::THREAD_GOAL_MAX_CREDITS);
    let exact = json!("9007199254740993.0000000001");
    assert!(exceeds_ceiling(
        credits,
        &json!("9007199254740993.0000000002"),
        &exact
    ));
    assert!(!exceeds_ceiling(
        credits,
        &json!("9007199254740993.00000000010"),
        &exact
    ));
    assert!(exceeds_ceiling(credits, &json!("0.00"), &exact));
    let bytes = definition(keys::IMPORT_MAX_BYTES);
    assert!(exceeds_ceiling(bytes, &Json::Null, &json!(1000)));
    assert!(!exceeds_ceiling(bytes, &json!(1000), &Json::Null));
    let messages = definition(keys::BROADCAST_THREAD_MESSAGES_PER_HOUR);
    assert!(!exceeds_ceiling(messages, &json!(0), &json!(10)));
    assert!(exceeds_ceiling(messages, &json!(1), &json!(0)));
}

#[test]
fn personal_read_tools_a_place_above_switched_off_stay_off_below() {
    for locked in [false, true] {
        let held = pair(
            keys::ORCH_TOOLS_OWNER_READS,
            &json!(false),
            &json!(true),
            locked,
        );
        assert_eq!(held.value, Some(json!(false)));
        assert_eq!(held.source.scope, SourceScope::System);
        let narrowed = pair(
            keys::ORCH_TOOLS_OWNER_READS,
            &json!(true),
            &json!(false),
            locked,
        );
        assert_eq!(narrowed.value, Some(json!(false)));
        assert_eq!(narrowed.source.scope, SourceScope::Org);
    }
    let allowed = pair(
        keys::ORCH_TOOLS_OWNER_READS,
        &json!(true),
        &json!(true),
        false,
    );
    assert_eq!(allowed.value, Some(json!(true)));
}

#[test]
fn every_policy_a_lower_place_can_set_narrows_or_is_a_reviewed_choice() {
    // A permission, a limit or a protection that a place below the system can set only narrows what the places above
    // allow. These are the policies reviewed as having no wider side: choices of a payer, of a way to recover or carry
    // history, of the tabs a document's readers see, of where file content passes, the default of a person's own choice,
    // an accounting estimate; periods whose longer and shorter values each loosen something, which a lock above holds;
    // a rate card, program versions, added patterns and terms.
    let reviewed: std::collections::BTreeSet<&str> = [
        keys::ASK_PAID_BY,
        keys::CALLS_RULES,
        keys::CALLS_REPEAT_WINDOW,
        keys::CALLS_NOTIFY_ON_STOP,
        keys::CALLS_RETURN,
        keys::THREAD_SWITCH_HISTORY,
        keys::DOCUMENTS_LINKS_SHOWN,
        keys::SHARING_ON_LINK,
        keys::USAGE_CATALOGUE,
        keys::USAGE_CATALOGUE_SCALE,
        keys::USAGE_RATES,
        keys::INVITATIONS_DEFAULT_MINUTES,
        keys::CLEANUP_UNUSED_KEEP_DAYS,
        keys::ENV_REMOVED_KEEP_DAYS,
        keys::RETENTION_ARCHIVE_IDLE_DAYS,
        keys::RETENTION_DELETE_ARCHIVED_DAYS,
        keys::RETENTION_AGENT_HISTORY_IDLE_DAYS,
        keys::STORAGE_RELAY_TRANSFERS,
        keys::AGENTS_HARNESS_VERSIONS,
        keys::REDACTION_PATTERNS,
        keys::ORG_TERMS,
    ]
    .into_iter()
    .collect();
    let found: std::collections::BTreeSet<&str> = crate::registry::all()
        .iter()
        .filter(|def| {
            def.class == Class::Policy
                && def.scopes.iter().any(|scope| *scope != Scope::System)
                && !matches!(
                    def.merge,
                    Merge::Min
                        | Merge::Max
                        | Merge::All
                        | Merge::Any
                        | Merge::Stricter
                        | Merge::MapStricter
                        | Merge::Intersect
                )
        })
        .map(|def| def.key)
        .collect();
    assert_eq!(
        found, reviewed,
        "a new policy that a lower place can set narrows, or is reviewed here"
    );
}

#[test]
fn a_recording_a_place_above_requires_cannot_be_stopped_below() {
    for locked in [false, true] {
        let held = pair(keys::TERMINAL_RECORD, &json!(true), &json!(false), locked);
        assert_eq!(held.value, Some(json!(true)));
        assert_eq!(held.source.scope, SourceScope::System);
        let required = pair(keys::TERMINAL_RECORD, &json!(false), &json!(true), locked);
        assert_eq!(required.value, Some(json!(true)));
        assert_eq!(required.source.scope, SourceScope::Org);
    }
}

#[test]
fn a_feature_a_place_above_switched_off_stays_off_below_member_by_member() {
    for locked in [false, true] {
        let merged = pair(
            keys::AGENT_FEATURES,
            &json!({"web": false, "plan": true}),
            &json!({"web": true, "plan": false, "mcp": true}),
            locked,
        );
        assert_eq!(
            merged.value,
            Some(json!({"web": false, "plan": false, "mcp": true}))
        );
    }
}

#[test]
fn a_capability_decision_and_its_approver_held_above_cannot_be_loosened_below() {
    let upper = json!({
        "kvm": {"decision": "deny", "approver_scope": "system"},
        "port_place": {"decision": "ask", "approver_scope": "org"},
    });
    let lower = json!({
        "kvm": {"decision": "allow", "approver_scope": "environment"},
        "port_place": {"decision": "deny", "approver_scope": "environment"},
        "gpu": {"decision": "allow", "approver_scope": "environment"},
    });
    for locked in [false, true] {
        let merged = pair(keys::CAPABILITY_POLICY, &upper, &lower, locked)
            .value
            .unwrap();
        assert_eq!(
            merged["kvm"],
            json!({"decision": "deny", "approver_scope": "system"})
        );
        assert_eq!(
            merged["port_place"],
            json!({"decision": "deny", "approver_scope": "org"})
        );
        assert_eq!(
            merged["gpu"],
            json!({"decision": "allow", "approver_scope": "environment"})
        );
    }
}

#[test]
fn a_runtime_a_place_above_forbids_or_leaves_out_of_its_requirement_stays_closed_below() {
    let cases = [
        // What the place above forbids with its wildcard, a place below cannot open by naming it.
        (
            json!({"*": "forbid", "runc": "allow"}),
            json!({"kata": "allow"}),
            json!({"*": "forbid", "runc": "allow", "kata": "forbid"}),
        ),
        // A requirement above leaves every other runtime out; a requirement below cannot add one.
        (
            json!({"runc": "require"}),
            json!({"kata": "require"}),
            json!({"*": "forbid", "runc": "forbid", "kata": "forbid"}),
        ),
        (
            json!({"runc": "require", "kata": "require"}),
            json!({"kata": "forbid"}),
            json!({"*": "forbid", "runc": "require", "kata": "forbid"}),
        ),
        // A place below can still forbid or require within what is allowed above.
        (
            json!({"gvisor": "allow"}),
            json!({"gvisor": "forbid"}),
            json!({"gvisor": "forbid"}),
        ),
        (
            json!({"runc": "allow"}),
            json!({"runc": "require"}),
            json!({"*": "forbid", "runc": "require"}),
        ),
    ];
    for (upper, lower, expected) in cases {
        for locked in [false, true] {
            assert_eq!(
                pair(keys::ENVIRONMENTS_RUNTIMES, &upper, &lower, locked).value,
                Some(expected.clone()),
                "{upper} above {lower}"
            );
        }
    }
}

#[test]
fn every_member_wise_strictest_policy_declares_members_whose_strictness_can_be_said() {
    let ranked = |schema: &Schema| matches!(schema, Schema::Bool | Schema::Choice { .. });
    for def in crate::registry::all()
        .iter()
        .filter(|def| def.merge == Merge::MapStricter)
    {
        let Schema::Map { values, .. } = def.schema else {
            panic!("{} keeps the strictest member, so it is a map", def.key);
        };
        assert!(
            ranked(values)
                || matches!(values, Schema::Object { members, open: false }
                    if members.iter().all(|member| ranked(&member.schema))),
            "{}: a switch, a choice listed strictest last, or a closed object of those",
            def.key
        );
    }
}
