//! Optional source-declared credit fields reuse the existing member states.
//! Missing declarations preserve legacy unknowns; a reported value proves support.

use crate::facts::{MemberState, ProviderFacts};
use crate::limits::TEXT_MAX_CHARS;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::BTreeMap;

const BALANCE: &[&str] = &[
    "remaining",
    "total",
    "used",
    "unlimited",
    "unit",
    "label",
    "parts",
    "expires_at",
];
const SPEND: &[&str] = &[
    "used",
    "limit",
    "remaining",
    "unit",
    "label",
    "period",
    "period_minutes",
    "resets_at",
];
const EXTRA: &[&str] = &[
    "enabled",
    "used",
    "limit",
    "used_percent",
    "unit",
    "decimal_places",
    "disabled_reason",
    "limit_reached",
];

/// Fields this contract understands for a normalized existing member/item path.
#[must_use]
pub fn known_fields(group: &str) -> Option<&'static [&'static str]> {
    if group == "extra_usage" {
        return Some(EXTRA);
    }
    let (member, id) = group.split_once('.')?;
    if id.is_empty() || id.chars().count() > TEXT_MAX_CHARS {
        return None;
    }
    match member {
        "balances" => Some(BALANCE),
        "spend" => Some(SPEND),
        _ => None,
    }
}

/// The clock of one credit field. Existing item clocks stay `balances.<id>` / `spend.<id>`.
/// New list-field clocks use a separate leading-slash namespace and escaped components,
/// so an arbitrary legal item id cannot be mistaken for another item's field.
/// Singleton extra-use fields retain their existing keys for older adapters.
#[must_use]
pub fn field_time_key(group: &str, field: &str) -> String {
    if let Some((member, id)) = group.split_once('.')
        && matches!(member, "balances" | "spend")
    {
        let escaped = id.replace('~', "~0").replace('/', "~1");
        return format!("/{member}/{escaped}/{field}");
    }
    format!("{group}.{field}")
}

/// The existing group/item named by an unambiguous finer clock, never an ambiguous
/// dotted list-item key. Original item characters are bounded by the existing id limit.
#[must_use]
pub fn field_time_group(key: &str) -> Option<String> {
    if let Some(field) = key.strip_prefix("extra_usage.") {
        return EXTRA.contains(&field).then(|| "extra_usage".to_owned());
    }
    let path = key.strip_prefix('/')?;
    let mut parts = path.split('/');
    let (Some(member), Some(id), Some(field), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let fields = match member {
        "balances" => BALANCE,
        "spend" => SPEND,
        _ => return None,
    };
    if !fields.contains(&field) {
        return None;
    }
    let mut chars = id.chars();
    let mut decoded = String::new();
    let mut count = 0;
    while let Some(character) = chars.next() {
        decoded.push(if character == '~' {
            match chars.next()? {
                '0' => '~',
                '1' => '/',
                _ => return None,
            }
        } else {
            character
        });
        count += 1;
        if count > TEXT_MAX_CHARS {
            return None;
        }
    }
    (count > 0).then(|| format!("{member}.{decoded}"))
}

/// Existing member/item clocks and exactly encoded new field clocks can carry fresh facts.
#[must_use]
pub fn is_credit_time_key(key: &str) -> bool {
    ["balances", "spend", "extra_usage"].iter().any(|member| {
        key == *member
            || key
                .strip_prefix(*member)
                .is_some_and(|tail| tail.starts_with('.'))
    }) || field_time_group(key).is_some()
}

/// Forward-compatible source declarations ignore names this contract cannot interpret.
pub fn deserialize_reports<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Vec<String>>, D::Error> {
    let given = BTreeMap::<String, Vec<String>>::deserialize(deserializer)?;
    Ok(given
        .into_iter()
        .filter_map(|(group, fields)| {
            let known = known_fields(&group)?;
            let mut fields: Vec<_> = fields
                .into_iter()
                .filter(|field| known.contains(&field.as_str()))
                .collect();
            fields.sort();
            fields.dedup();
            Some((group, fields))
        })
        .collect())
}

fn groups(facts: &ProviderFacts) -> BTreeMap<String, Value> {
    let mut groups = BTreeMap::new();
    for balance in &facts.balances {
        if let Ok(value) = serde_json::to_value(balance) {
            groups.insert(format!("balances.{}", balance.id), value);
        }
    }
    for spend in &facts.spend {
        if let Ok(value) = serde_json::to_value(spend) {
            groups.insert(format!("spend.{}", spend.id), value);
        }
    }
    if let Some(extra) = &facts.extra_usage
        && let Ok(value) = serde_json::to_value(extra)
    {
        groups.insert("extra_usage".to_owned(), value);
    }
    groups
}

/// Add positive field evidence using its existing group clock only when no finer clock was supplied.
/// An explicit older quantity time remains authoritative during a label-only item refresh.
pub(crate) fn remember_told(facts: &mut ProviderFacts) {
    for (group, value) in groups(facts) {
        let member = group.split('.').next().unwrap_or(&group);
        let Some(at) = facts
            .field_times
            .get(&group)
            .or_else(|| facts.field_times.get(member))
            .copied()
        else {
            continue;
        };
        let Some(fields) = known_fields(&group) else {
            continue;
        };
        for field in fields {
            if value.get(*field).is_some_and(|value| !value.is_null()) {
                let key = field_time_key(&group, field);
                facts.field_times.entry(key).or_insert(at);
            }
        }
    }
}

/// Resolved nested states, using the same four meanings as whole facts members.
#[must_use]
pub fn states(
    facts: &ProviderFacts,
    reports: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, MemberState> {
    let mut groups = groups(facts);
    for group in reports.keys().filter(|group| known_fields(group).is_some()) {
        groups.entry(group.clone()).or_insert(Value::Null);
    }
    let mut states = BTreeMap::new();
    for (group, value) in groups {
        let Some(fields) = known_fields(&group) else {
            continue;
        };
        let member = group.split('.').next().unwrap_or(&group);
        for field in fields {
            let path = format!("{group}.{field}");
            let supported = reports
                .get(&group)
                .is_none_or(|fields| fields.iter().any(|known| known.as_str() == *field));
            let holds = value.get(*field).is_some_and(|value| !value.is_null());
            let told = facts
                .field_times
                .contains_key(&field_time_key(&group, field))
                || supported
                    && (facts.field_times.contains_key(&group)
                        || facts.field_times.contains_key(member));
            states.insert(path, MemberState::of(holds, told, supported));
        }
    }
    states
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::normalize;
    use serde_json::json;

    #[test]
    fn any_source_can_declare_exact_fields_without_hiding_supported_missing_amounts() {
        let mut facts: ProviderFacts = serde_json::from_value(json!({"balances":[{"id":"third-party","unit":"points","remaining":null}],"field_times":{"balances":"2026-11-03T00:00:00Z"}})).unwrap();
        let fields = [(
            "balances.third-party".to_owned(),
            vec!["remaining".to_owned(), "unit".to_owned()],
        )]
        .into();
        let resolved = states(&facts, &fields);
        assert_eq!(
            resolved["balances.third-party.remaining"],
            MemberState::Empty
        );
        assert_eq!(
            resolved["balances.third-party.total"],
            MemberState::Unsupported
        );
        assert_ne!(
            states(&facts, &BTreeMap::new())["balances.third-party.total"],
            MemberState::Unsupported
        );
        facts.balances[0].total = Some("0".to_owned());
        normalize(&mut facts);
        assert_eq!(
            states(&facts, &fields)["balances.third-party.total"],
            MemberState::Told
        );
        facts.balances[0].total = None;
        assert_eq!(
            states(&facts, &fields)["balances.third-party.total"],
            MemberState::Empty
        );
    }

    #[test]
    fn declaration_absence_and_a_sparse_enabled_event_do_not_make_amounts_fresh() {
        let mut facts: ProviderFacts = serde_json::from_value(json!({"extra_usage":{"enabled":true,"used":"1.20","limit":null,"used_percent":null,"unit":"USD","decimal_places":2,"disabled_reason":null,"limit_reached":null},"field_times":{"extra_usage":"2026-11-03T00:00:00Z","extra_usage.enabled":"2026-11-03T01:00:00Z"}})).unwrap();
        normalize(&mut facts);
        assert_eq!(
            facts.field_times["extra_usage.used"],
            "2026-11-03T00:00:00Z".parse().unwrap()
        );
        assert_eq!(
            facts.field_times["extra_usage.enabled"],
            "2026-11-03T01:00:00Z".parse().unwrap()
        );
    }

    #[test]
    fn third_party_descriptor_fields_are_bounded_known_and_optional() {
        let descriptor: crate::SourceDescriptor = serde_json::from_value(json!({"name":"third-party","title":"Independent source","summary":"Reports points","contract":1,"facts":{"reports":["balances"],"field_reports":{"balances.account":["remaining","unit","future_field"],"future_member.account":["x"]}}})).unwrap();
        descriptor.check().unwrap();
        assert_eq!(
            descriptor.facts.as_ref().unwrap().field_reports["balances.account"],
            vec!["remaining", "unit"]
        );
        assert_eq!(descriptor.facts.as_ref().unwrap().field_reports.len(), 1);
        let legacy: crate::SourceDescriptor = serde_json::from_value(json!({"name":"old-source","title":"Old source","summary":"No nested declarations","contract":1,"facts":{"reports":["balances"]}})).unwrap();
        legacy.check().unwrap();
        assert!(legacy.facts.unwrap().field_reports.is_empty());
    }

    #[test]
    fn a_fresh_collection_does_not_retime_its_older_item() {
        let mut facts: ProviderFacts = serde_json::from_value(json!({"balances":[{"id":"old","unit":"points","remaining":"10"},{"id":"fresh","unit":"points","remaining":"20"}],"field_times":{"balances":"2026-11-03T02:00:00Z","balances.old":"2026-11-03T01:00:00Z","balances.fresh":"2026-11-03T02:00:00Z"}})).unwrap();
        normalize(&mut facts);
        assert_eq!(
            facts.field_times["/balances/old/remaining"],
            "2026-11-03T01:00:00Z".parse().unwrap()
        );
        assert_eq!(
            facts.field_times["/balances/fresh/remaining"],
            "2026-11-03T02:00:00Z".parse().unwrap()
        );
    }

    #[test]
    fn a_dotted_item_name_is_not_evidence_for_an_unsupported_field_on_another_item() {
        let mut facts:ProviderFacts=serde_json::from_value(json!({"balances":[{"id":"plan","unit":"points","remaining":"10"},{"id":"plan.total","unit":"points","remaining":"20"}],"field_times":{"balances":"2026-11-03T00:00:00Z","balances.plan":"2026-11-03T00:00:00Z","balances.plan.total":"2026-11-03T00:00:00Z"}})).unwrap();
        normalize(&mut facts);
        let fields = [(
            "balances.plan".to_owned(),
            vec!["remaining".to_owned(), "unit".to_owned()],
        )]
        .into();
        assert_eq!(
            states(&facts, &fields)["balances.plan.total"],
            MemberState::Unsupported
        );
        assert_eq!(
            states(&facts, &fields)["balances.plan.total.remaining"],
            MemberState::Told
        );
    }
}
