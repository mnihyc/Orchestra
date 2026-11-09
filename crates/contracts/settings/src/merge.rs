//! Declared merge modes and lock semantics.
//!
//! A key resolves through one hierarchy, from the top down: the registry's
//! default, a default that the environment supplies, the system, each org
//! on the path from the root org inward, the project, the topic, the thread,
//! and last the own value of the thing it is resolved for. Each of those
//! that has a value is a [`Layer`]; [`resolve`] combines them as the key's
//! merge mode says and reports where the result comes from.
//!
//! A row may be **locked**. A lock takes the choice away from everything
//! under it: of several, the one furthest up decides, and the rows below it
//! count for nothing while it holds. Where lower scopes can only narrow, as
//! for a list that is intersected or a number of which the smallest counts,
//! a lock changes nothing.

use orch_types::Json;

use crate::provenance::{LockedBy, Part, Resolved, Source, SourceScope, SuppliedPart};
use crate::registry::{Merge, Scope, SettingDef};

/// What separates the texts of two scopes that are joined: one blank line.
const BLANK_LINE: &str = "\n\n";

/// One value that a key has on the way down.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer<'a, N> {
    /// Where it stands.
    pub scope: SourceScope,
    /// The node, for a row at one.
    pub node: Option<N>,
    /// The value.
    pub value: &'a Json,
    /// Whether the row is locked. Only a row at the system or at a node
    /// carries a lock.
    pub locked: bool,
}

impl<'a, N> Layer<'a, N> {
    /// A value written at `scope`: a row at the system, where `node` is
    /// `None`, or at a node; or the own value of an environment or of a
    /// provider's runtime, which carries no lock.
    #[must_use]
    pub fn at(scope: Scope, node: Option<N>, value: &'a Json, locked: bool) -> Self {
        let scope = SourceScope::from(scope);
        Self {
            scope,
            node,
            value,
            locked: locked && scope.lock().is_some(),
        }
    }

    /// A default that the environment supplies, in place of a fixed one. It
    /// sits between the registry and the system scope: any row overrides it.
    #[must_use]
    pub const fn supplied(value: &'a Json) -> Self {
        Self {
            scope: SourceScope::Environment,
            node: None,
            value,
            locked: false,
        }
    }

    /// A default that stands in the registry's place: what an installation
    /// was seeded with, or what the model or another key gives.
    #[must_use]
    pub const fn default(value: &'a Json) -> Self {
        Self {
            scope: SourceScope::Default,
            node: None,
            value,
            locked: false,
        }
    }

    fn source(&self) -> Source<N>
    where
        N: Clone,
    {
        Source {
            scope: self.scope,
            node: self.node.clone(),
        }
    }
}

/// Resolves the key `def` through `layers`.
///
/// The layers are put in order from the top down by their scope; the orgs
/// of a path keep the order in which they are given, which is from the root
/// org inward. A scope that has no value has no layer.
///
/// - `replace`: the nearest layer wins. A locked row overrides everything
///   below it.
/// - `union`: the lists of all layers together, in order from the top, each
///   item once. Rows below a locked row are left out.
/// - `intersect`: what every layer that sets the key names; with none, the
///   default, which when unset means no restriction. A lock changes nothing.
/// - `map`: the objects merged member by member, a lower layer overriding
///   the same member. Rows below a locked row are left out.
/// - `append`: the texts joined from the top down, separated by one blank
///   line. Rows below a locked row are left out.
/// - `min`: the smallest number among the layers that set the key; with
///   none, the default, which when unset means no limit. A lock changes
///   nothing.
/// - `max`: the greatest minimum set by a scope. A lower scope cannot reduce it.
/// - `all`: every setting scope must allow the permission.
/// - `any`: any setting scope can require the protection.
/// - `stricter`: the highest-ranked choice in the declared schema, strictest last.
///
/// Shipped defaults are fallback values, never implicit constraints. A setting's
/// declared unbounded zero and nullable bounds resolve as unset when no finite
/// scope bounds them; an ordinary zero cap remains a real cap. Equal constraints
/// name the nearest matching source, so an equal local value is not ineffective.
///
/// A layer whose value is not of the kind its mode combines is passed over.
#[must_use]
pub fn resolve<N: Clone>(def: &SettingDef, layers: &[Layer<'_, N>]) -> Resolved<N> {
    let mut ordered: Vec<&Layer<'_, N>> = layers.iter().collect();
    ordered.sort_by_key(|layer| layer.scope.depth());
    let (defaults, rows): (Vec<_>, Vec<_>) = ordered.into_iter().partition(|layer| {
        layer.scope == SourceScope::Default
            || !def.merge.honours_locks() && layer.scope == SourceScope::Environment
    });
    let mut default = defaults
        .last()
        .map(|layer| layer.value.clone())
        .or_else(|| def.default_value());
    if matches!(def.merge, Merge::Min | Merge::Max)
        && def.zero_means_no_limit()
        && default.as_ref().is_some_and(is_zero)
    {
        default = None;
    }

    let lock = if def.merge.honours_locks() {
        rows.iter().position(|layer| layer.locked)
    } else {
        None
    };
    let counted = lock.map_or(&rows[..], |index| &rows[..=index]);
    let locked_by = lock.and_then(|index| {
        let row = rows[index];
        row.scope.lock().map(|scope| LockedBy {
            scope,
            node: row.node.clone(),
        })
    });

    let default_source = defaults
        .last()
        .map_or_else(Source::registry, |layer| layer.source());
    let given = default.clone();
    let Merged {
        value,
        used,
        source,
    } = match def.merge {
        Merge::Replace => replace(default, counted),
        Merge::Union => union(default, counted),
        Merge::Intersect => intersect(default, counted),
        Merge::Map => map(default, counted),
        Merge::Append => append(default, counted),
        Merge::Min => bound(def, default, counted, false),
        Merge::Max => bound(def, default, counted, true),
        Merge::All => boolean(default, counted, false),
        Merge::Any => boolean(default, counted, true),
        Merge::Stricter => stricter(def, default, counted),
    };
    // The default is a part of the value where the mode joins it with the rows, and where no row
    // counted; otherwise the rows replace it.
    let joins_default =
        matches!(def.merge, Merge::Union | Merge::Map | Merge::Append) || used.is_empty();
    let supplied: Vec<SuppliedPart<N>> = given
        .filter(|value| joins_default && !value.is_null())
        .map(|value| SuppliedPart {
            source: default_source.clone(),
            value,
            locked: false,
            ignored: false,
        })
        .into_iter()
        .chain(rows.iter().enumerate().map(|(index, layer)| SuppliedPart {
            source: layer.source(),
            value: layer.value.clone(),
            locked: layer.locked,
            ignored: lock.is_some_and(|at| index > at),
        }))
        .collect();
    Resolved {
        value: value.filter(|value| !value.is_null()),
        source: source.map_or_else(|| default_source.clone(), |layer| layer.source()),
        locked_by,
        parts: used
            .iter()
            .map(|layer| Part {
                source: layer.source(),
                value: layer.value.clone(),
            })
            .collect(),
        supplied,
    }
}

/// What a merge comes to, and the layers that counted for it.
struct Merged<'r, 'a, N> {
    value: Option<Json>,
    used: Vec<&'r Layer<'a, N>>,
    source: Option<&'r Layer<'a, N>>,
}

impl<'r, 'a, N> Merged<'r, 'a, N> {
    fn new(value: Option<Json>, used: Vec<&'r Layer<'a, N>>) -> Self {
        Self {
            value,
            source: used.last().copied(),
            used,
        }
    }
}

fn replace<'r, 'a, N>(default: Option<Json>, rows: &[&'r Layer<'a, N>]) -> Merged<'r, 'a, N> {
    match rows.last() {
        Some(nearest) => Merged::new(Some(nearest.value.clone()), vec![*nearest]),
        None => Merged::new(default, Vec::new()),
    }
}

fn union<'r, 'a, N>(default: Option<Json>, rows: &[&'r Layer<'a, N>]) -> Merged<'r, 'a, N> {
    let lists: Vec<_> = rows
        .iter()
        .copied()
        .filter(|layer| layer.value.is_array())
        .collect();
    if lists.is_empty() {
        return Merged::new(default, lists);
    }
    let mut all: Vec<Json> = Vec::new();
    let given = default
        .as_ref()
        .and_then(Json::as_array)
        .into_iter()
        .flatten();
    for item in given.chain(
        lists
            .iter()
            .filter_map(|layer| layer.value.as_array())
            .flatten(),
    ) {
        if !all.contains(item) {
            all.push(item.clone());
        }
    }
    Merged::new(Some(Json::Array(all)), lists)
}

fn intersect<'r, 'a, N>(default: Option<Json>, rows: &[&'r Layer<'a, N>]) -> Merged<'r, 'a, N> {
    let lists: Vec<_> = rows
        .iter()
        .copied()
        .filter(|layer| layer.value.is_array())
        .collect();
    let mut kept: Option<Vec<Json>> = None;
    let mut source = None;
    for layer in &lists {
        let Some(values) = layer.value.as_array() else {
            continue;
        };
        if let Some(kept) = &mut kept {
            let before = kept.len();
            kept.retain(|item| values.contains(item));
            if kept.len() != before || values.iter().all(|item| kept.contains(item)) {
                source = Some(*layer);
            }
        } else {
            let mut first = Vec::new();
            for item in values {
                if !first.contains(item) {
                    first.push(item.clone());
                }
            }
            kept = Some(first);
            source = Some(*layer);
        }
    }
    Merged {
        value: kept.map(Json::Array).or(default),
        used: lists,
        source,
    }
}

fn map<'r, 'a, N>(default: Option<Json>, rows: &[&'r Layer<'a, N>]) -> Merged<'r, 'a, N> {
    let objects: Vec<_> = rows
        .iter()
        .copied()
        .filter(|layer| layer.value.is_object())
        .collect();
    if objects.is_empty() {
        return Merged::new(default, objects);
    }
    let mut merged = default
        .as_ref()
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    for members in objects.iter().filter_map(|layer| layer.value.as_object()) {
        for (name, value) in members {
            merged.insert(name.clone(), value.clone());
        }
    }
    Merged::new(Some(Json::Object(merged)), objects)
}

/// What a scope adds to a joined text: its text without the line breaks
/// around it, or nothing when it says nothing.
fn piece(value: &Json) -> Option<&str> {
    let text = value.as_str()?.trim_matches(['\n', '\r']);
    (!text.trim().is_empty()).then_some(text)
}

fn append<'r, 'a, N>(default: Option<Json>, rows: &[&'r Layer<'a, N>]) -> Merged<'r, 'a, N> {
    let texts: Vec<_> = rows
        .iter()
        .copied()
        .filter(|layer| piece(layer.value).is_some())
        .collect();
    if texts.is_empty() {
        return Merged::new(default, texts);
    }
    let given = default.as_ref().and_then(piece);
    let pieces: Vec<&str> = given
        .into_iter()
        .chain(texts.iter().filter_map(|layer| piece(layer.value)))
        .collect();
    Merged::new(Some(Json::String(pieces.join(BLANK_LINE))), texts)
}

fn decimal_parts(value: &str) -> Option<(&str, &str)> {
    let (whole, fraction) = match value.split_once('.') {
        Some((whole, fraction)) if !fraction.is_empty() => (whole, fraction),
        Some(_) => return None,
        None => (value, ""),
    };
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let whole = whole.trim_start_matches('0');
    Some((
        if whole.is_empty() { "0" } else { whole },
        fraction.trim_end_matches('0'),
    ))
}

fn is_zero(value: &Json) -> bool {
    value.as_i64() == Some(0)
        || value.as_f64() == Some(0.0)
        || value
            .as_str()
            .and_then(decimal_parts)
            .is_some_and(|parts| parts == ("0", ""))
}

fn number_cmp(one: &Json, other: &Json) -> Option<std::cmp::Ordering> {
    if let (Some(one), Some(other)) = (one.as_i64(), other.as_i64()) {
        return Some(one.cmp(&other));
    }
    if let (Some(one), Some(other)) = (one.as_u64(), other.as_u64()) {
        return Some(one.cmp(&other));
    }
    if let (Some(one), Some(other)) = (one.as_str(), other.as_str()) {
        let ((a, af), (b, bf)) = (decimal_parts(one)?, decimal_parts(other)?);
        return Some(a.len().cmp(&b.len()).then_with(|| a.cmp(b)).then_with(|| {
            let places = af.len().max(bf.len());
            af.bytes()
                .chain(std::iter::repeat(b'0'))
                .take(places)
                .cmp(bf.bytes().chain(std::iter::repeat(b'0')).take(places))
        }));
    }
    one.as_f64()?.partial_cmp(&other.as_f64()?)
}

fn bound<'r, 'a, N>(
    def: &SettingDef,
    default: Option<Json>,
    rows: &[&'r Layer<'a, N>],
    largest: bool,
) -> Merged<'r, 'a, N> {
    use crate::registry::{Form, Schema};
    let mut schema = &def.schema;
    while let Schema::Nullable { of } = schema {
        schema = of;
    }
    let decimal = matches!(
        schema,
        Schema::Text {
            form: Form::Decimal | Form::PositiveDecimal,
            ..
        }
    );
    let nullable = matches!(def.schema, Schema::Nullable { .. }) || def.zero_means_no_limit();
    let mut winner: Option<&'r Layer<'a, N>> = None;
    let mut unbounded = None;
    for layer in rows.iter().copied() {
        if nullable && layer.value.is_null() || def.zero_means_no_limit() && is_zero(layer.value) {
            unbounded = Some(layer);
            continue;
        }
        let numeric = if decimal {
            layer.value.as_str().and_then(decimal_parts).is_some()
        } else {
            layer.value.is_number()
        };
        if !numeric {
            continue;
        }
        let wins = winner.is_none_or(|previous| {
            number_cmp(layer.value, previous.value).is_some_and(|order| {
                order == std::cmp::Ordering::Equal
                    || if largest {
                        order.is_gt()
                    } else {
                        order.is_lt()
                    }
            })
        });
        if wins {
            winner = Some(layer);
        }
    }
    if let Some(winner) = winner {
        Merged::new(Some(winner.value.clone()), vec![winner])
    } else if let Some(unbounded) = unbounded {
        Merged::new(None, vec![unbounded])
    } else {
        Merged::new(default, Vec::new())
    }
}

fn boolean<'r, 'a, N>(
    default: Option<Json>,
    rows: &[&'r Layer<'a, N>],
    strict: bool,
) -> Merged<'r, 'a, N> {
    let mut winner: Option<(&'r Layer<'a, N>, bool)> = None;
    for layer in rows.iter().copied() {
        let Some(value) = layer.value.as_bool() else {
            continue;
        };
        if winner.is_none_or(|(_, previous)| value == strict || value == previous) {
            winner = Some((layer, value));
        }
    }
    match winner {
        Some((layer, value)) => Merged::new(Some(Json::Bool(value)), vec![layer]),
        None => Merged::new(default, Vec::new()),
    }
}

fn stricter<'r, 'a, N>(
    def: &SettingDef,
    default: Option<Json>,
    rows: &[&'r Layer<'a, N>],
) -> Merged<'r, 'a, N> {
    let crate::registry::Schema::Choice { values } = def.schema else {
        return Merged::new(default, Vec::new());
    };
    let winner = rows
        .iter()
        .copied()
        .filter_map(|layer| {
            let value = layer.value.as_str()?;
            values
                .iter()
                .position(|choice| *choice == value)
                .map(|rank| (layer, rank))
        })
        .max_by_key(|(_, rank)| *rank);
    match winner {
        Some((layer, _)) => Merged::new(Some(layer.value.clone()), vec![layer]),
        None => Merged::new(default, Vec::new()),
    }
}

#[cfg(test)]
#[path = "merge/narrowing_tests.rs"]
mod narrowing_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::LockScope;
    use crate::registry::{Class, DefaultOf, Group, Schema, Unit, get, keys};
    use orch_types::{Id, new_id};
    use serde_json::json;

    fn def(key: &str) -> &'static SettingDef {
        get(key).unwrap()
    }

    fn at_node<'a>(scope: Scope, node: Id, value: &'a Json) -> Layer<'a, Id> {
        Layer::at(scope, Some(node), value, false)
    }

    fn locked<'a>(scope: Scope, node: Option<Id>, value: &'a Json) -> Layer<'a, Id> {
        Layer::at(scope, node, value, true)
    }

    #[test]
    fn ask_model_policy_intersects_identities_in_parent_order_and_unset_stays_absent() {
        // EN: system Ask models A/B > org tries C or reverses B/A; only parent
        // identities survive, in system order. Remove A to choose B as default.
        // 中文：系统允许 A/B > 组织尝试 C 或改为 B/A；只能保留父级允许项及顺序，移除 A 才能默认 B。
        let first = json!({"provider_id":new_id(),"model":"first"});
        let second = json!({"provider_id":new_id(),"model":"second"});
        let extra = json!({"provider_id":new_id(),"model":"extra"});
        let parent = json!([first, second]);
        let child = json!([second, first, extra]);
        let node = new_id();
        let definition = def(keys::ASK_MODELS);
        assert_eq!(definition.class, Class::Policy);
        assert_eq!(definition.merge, Merge::Intersect);
        assert_eq!(
            resolve(
                definition,
                &[
                    Layer::at(Scope::System, None, &parent, false),
                    at_node(Scope::Org, node, &child)
                ]
            )
            .value,
            Some(parent.clone())
        );
        let narrowed = json!([second, extra]);
        assert_eq!(
            resolve(
                definition,
                &[
                    Layer::at(Scope::System, None, &parent, false),
                    at_node(Scope::Org, node, &narrowed)
                ]
            )
            .value,
            Some(json!([second]))
        );
        assert_eq!(resolve::<Id>(definition, &[]).value, None);
        assert!(
            crate::registry::validate_at(
                keys::ASK_MODELS,
                Scope::Org,
                &json!([{"provider_id":new_id(),"model":"first","params":{"effort":"high"}}])
            )
            .is_err()
        );
    }

    #[test]
    fn an_unset_key_takes_the_registrys_default() {
        let shells = resolve::<Id>(def(keys::TERMINAL_MAX_PER_PERSON), &[]);
        assert_eq!(shells.value, Some(json!(4)));
        assert_eq!(shells.source, Source::registry());
        assert_eq!(shells.locked_by, None);
        let model = resolve::<Id>(def(keys::AGENT_MODEL), &[]);
        assert_eq!(
            model.value, None,
            "there is no model until somebody chooses one"
        );
    }

    #[test]
    fn the_most_specific_scope_wins_for_a_replace_key() {
        let (org, project, thread) = (new_id(), new_id(), new_id());
        let (at_org, at_project, at_thread) =
            (json!("model-a"), json!("model-b"), json!("model-c"));
        let layers = [
            at_node(Scope::Org, org, &at_org),
            at_node(Scope::Project, project, &at_project),
            at_node(Scope::Thread, thread, &at_thread),
        ];
        let resolved = resolve(def(keys::AGENT_MODEL), &layers);
        assert_eq!(resolved.value, Some(json!("model-c")));
        assert_eq!(
            resolved.source,
            Source {
                scope: SourceScope::Thread,
                node: Some(thread)
            }
        );
        let at_the_topic = resolve(def(keys::AGENT_MODEL), &layers[..2]);
        assert_eq!(
            at_the_topic.value,
            Some(json!("model-b")),
            "a topic that sets nothing takes its project's"
        );
        assert_eq!(at_the_topic.source.node, Some(project));
    }

    #[test]
    fn of_the_orgs_on_a_path_the_inner_one_is_nearer() {
        let (root, department) = (new_id(), new_id());
        let (outer, inner) = (json!(false), json!(true));
        let layers = [
            at_node(Scope::Org, root, &outer),
            at_node(Scope::Org, department, &inner),
        ];
        let resolved = resolve(def(keys::CHAT_ENABLED), &layers);
        assert_eq!(resolved.value, Some(json!(true)));
        assert_eq!(resolved.source.node, Some(department));
    }

    #[test]
    fn layers_are_read_from_the_top_down_in_whatever_order_they_are_given() {
        let (org, topic) = (new_id(), new_id());
        let (at_system, at_org, at_topic) = (json!(10), json!(20), json!(30));
        let layers = [
            at_node(Scope::Topic, topic, &at_topic),
            Layer::at(Scope::System, None, &at_system, false),
            at_node(Scope::Org, org, &at_org),
        ];
        let resolved = resolve(def(keys::ENV_IDLE_STOP_MINUTES), &layers);
        assert_eq!(resolved.value, Some(json!(30)));
        assert_eq!(resolved.source.scope, SourceScope::Topic);
    }

    #[test]
    fn a_lock_above_decides_a_replace_key_whatever_is_set_below() {
        let (org, project, thread) = (new_id(), new_id(), new_id());
        let (at_org, at_project, at_thread) = (json!("ask"), json!("on_request"), json!("auto"));
        let layers = [
            locked(Scope::Org, Some(org), &at_org),
            at_node(Scope::Project, project, &at_project),
            at_node(Scope::Thread, thread, &at_thread),
        ];
        let resolved = resolve(def(keys::APPROVAL_MODE), &layers);
        assert_eq!(resolved.value, Some(json!("ask")));
        assert_eq!(
            resolved.source,
            Source {
                scope: SourceScope::Org,
                node: Some(org)
            }
        );
        assert_eq!(
            resolved.locked_by,
            Some(LockedBy {
                scope: LockScope::Org,
                node: Some(org)
            })
        );
        assert_eq!(
            resolved.parts.len(),
            1,
            "what is stored below a lock counts for nothing"
        );
    }

    #[test]
    fn of_two_locks_the_one_further_up_decides() {
        let (org, project) = (new_id(), new_id());
        let (at_system, at_org, at_project) = (json!(false), json!(true), json!(true));
        let layers = [
            locked(Scope::System, None, &at_system),
            locked(Scope::Org, Some(org), &at_org),
            at_node(Scope::Project, project, &at_project),
        ];
        let resolved = resolve(def(keys::TERMINAL_ENABLED), &layers);
        assert_eq!(resolved.value, Some(json!(false)));
        assert_eq!(
            resolved.locked_by,
            Some(LockedBy {
                scope: LockScope::System,
                node: None
            })
        );
    }

    #[test]
    fn a_lock_does_not_bind_what_is_above_it() {
        let (org, project) = (new_id(), new_id());
        let (at_org, at_project) = (json!(30), json!(5));
        let layers = [
            at_node(Scope::Org, org, &at_org),
            locked(Scope::Project, Some(project), &at_project),
        ];
        let resolved = resolve(def(keys::ENV_IDLE_STOP_MINUTES), &layers);
        assert_eq!(resolved.value, Some(json!(5)));
        assert_eq!(
            resolved.locked_by.map(|lock| lock.scope),
            Some(LockScope::Project)
        );
        let at_the_org = resolve(def(keys::ENV_IDLE_STOP_MINUTES), &layers[..1]);
        assert_eq!(
            (at_the_org.value, at_the_org.locked_by),
            (Some(json!(30)), None)
        );
    }

    #[test]
    fn a_list_key_merges_by_union_where_the_registry_says_so() {
        let (org, project, topic) = (new_id(), new_id(), new_id());
        let (at_org, at_project, at_topic) = (
            json!(["target/", "*.log"]),
            json!(["node_modules/", "*.log"]),
            json!(["dist/"]),
        );
        let layers = [
            at_node(Scope::Org, org, &at_org),
            at_node(Scope::Project, project, &at_project),
            at_node(Scope::Topic, topic, &at_topic),
        ];
        let resolved = resolve(def(keys::WORKSPACE_IGNORE), &layers);
        assert_eq!(
            resolved.value,
            Some(json!(["target/", "*.log", "node_modules/", "dist/"])),
            "in order from the top, each once"
        );
        assert_eq!(resolved.source.scope, SourceScope::Topic);
        assert_eq!(resolved.parts.len(), 3);
        assert_eq!(
            resolve::<Id>(def(keys::WORKSPACE_IGNORE), &[]).value,
            Some(json!([]))
        );
    }

    #[test]
    fn rows_below_a_lock_add_nothing_to_a_union() {
        let (org, topic) = (new_id(), new_id());
        let (at_org, at_topic) = (json!(["API_KEY"]), json!(["DEPLOY_TOKEN"]));
        let layers = [
            locked(Scope::Org, Some(org), &at_org),
            at_node(Scope::Topic, topic, &at_topic),
        ];
        let resolved = resolve(def(keys::ENV_SECRET_NAMES), &layers);
        assert_eq!(resolved.value, Some(json!(["API_KEY"])));
        assert_eq!(resolved.locked_by.and_then(|lock| lock.node), Some(org));
    }

    #[test]
    fn an_intersect_key_keeps_what_every_scope_that_sets_it_names() {
        let (org, project) = (new_id(), new_id());
        let (at_system, at_org, at_project) = (
            json!(["ask", "on_request", "auto"]),
            json!(["on_request", "ask"]),
            json!(["auto", "ask"]),
        );
        let layers = [
            Layer::at(Scope::System, None, &at_system, false),
            at_node(Scope::Org, org, &at_org),
            at_node(Scope::Project, project, &at_project),
        ];
        let resolved = resolve(def(keys::APPROVAL_ALLOWED_MODES), &layers);
        assert_eq!(resolved.value, Some(json!(["ask"])));
        let two = resolve(def(keys::APPROVAL_ALLOWED_MODES), &layers[..2]);
        assert_eq!(
            two.value,
            Some(json!(["ask", "on_request"])),
            "in the order of the topmost list"
        );
        assert_eq!(
            resolved.parts.len(),
            3,
            "each list is kept for whoever has to ask them one by one"
        );
    }

    #[test]
    fn an_intersect_key_that_nobody_set_restricts_nothing() {
        assert_eq!(resolve::<Id>(def(keys::MODELS_ALLOWED), &[]).value, None);
        assert_eq!(resolve::<Id>(def(keys::BINDING_ALLOWED), &[]).value, None);
        let modes = resolve::<Id>(def(keys::APPROVAL_ALLOWED_MODES), &[]);
        assert_eq!(
            modes.value,
            Some(json!(["ask", "on_request", "auto", "bypass"]))
        );
        let org = new_id();
        let narrowed = json!(["provider", "mount"]);
        let resolved = resolve(
            def(keys::RESOURCES_CREATED_BY_MEMBERS),
            &[at_node(Scope::Org, org, &narrowed)],
        );
        assert_eq!(
            resolved.value,
            Some(json!(["provider", "mount"])),
            "the default is no scope and narrows nothing"
        );
    }

    #[test]
    fn a_lock_changes_nothing_where_lower_scopes_can_only_narrow() {
        let (org, project) = (new_id(), new_id());
        let (at_org, at_project) = (json!(["separated", "key_separated"]), json!(["separated"]));
        let layers = [
            locked(Scope::Org, Some(org), &at_org),
            at_node(Scope::Project, project, &at_project),
        ];
        let resolved = resolve(def(keys::BINDING_ALLOWED), &layers);
        assert_eq!(resolved.value, Some(json!(["separated"])));
        assert_eq!(resolved.locked_by, None);
    }

    #[test]
    fn a_map_key_merges_member_by_member_and_a_lower_scope_overrides_the_same_member() {
        let (org, topic) = (new_id(), new_id());
        let (github, jira) = (new_id().to_string(), new_id().to_string());
        let (at_org, at_topic) = (
            json!({ &github: true, &jira: true }),
            json!({ &jira: false }),
        );
        let layers = [
            at_node(Scope::Org, org, &at_org),
            at_node(Scope::Topic, topic, &at_topic),
        ];
        let resolved = resolve(def(keys::MCP_ENABLED), &layers);
        assert_eq!(resolved.value, Some(json!({ &github: true, &jira: false })));
        assert_eq!(
            resolve::<Id>(def(keys::MCP_ENABLED), &[]).value,
            Some(json!({}))
        );
    }

    #[test]
    fn rows_below_a_lock_change_no_member_of_a_map() {
        let (org, topic) = (new_id(), new_id());
        let (at_org, at_topic) = (
            json!({ "CI": "true" }),
            json!({ "CI": "false", "DEBUG": "1" }),
        );
        let layers = [
            locked(Scope::Org, Some(org), &at_org),
            at_node(Scope::Topic, topic, &at_topic),
        ];
        assert_eq!(
            resolve(def(keys::ENV_VARIABLES), &layers).value,
            Some(json!({ "CI": "true" }))
        );
    }

    #[test]
    fn instructions_are_joined_from_the_top_down_with_one_blank_line() {
        let (org, project, thread) = (new_id(), new_id(), new_id());
        let (at_org, at_project, at_thread) = (
            json!("Write in English.\n"),
            json!(""),
            json!("\nKeep changes small."),
        );
        let layers = [
            at_node(Scope::Org, org, &at_org),
            at_node(Scope::Project, project, &at_project),
            at_node(Scope::Thread, thread, &at_thread),
        ];
        let resolved = resolve(def(keys::AGENT_INSTRUCTIONS), &layers);
        assert_eq!(
            resolved.value,
            Some(json!("Write in English.\n\nKeep changes small."))
        );
        assert_eq!(
            resolved.parts.len(),
            2,
            "a scope that says nothing adds nothing"
        );
        assert_eq!(
            resolve::<Id>(def(keys::AGENT_INSTRUCTIONS), &[]).value,
            Some(json!(""))
        );
        let under_a_lock = [
            locked(Scope::Org, Some(org), &at_org),
            at_node(Scope::Thread, thread, &at_thread),
        ];
        assert_eq!(
            resolve(def(keys::AGENT_INSTRUCTIONS), &under_a_lock).value,
            Some(json!("Write in English."))
        );
    }

    #[test]
    fn of_a_min_key_the_smallest_number_counts_and_unset_means_no_limit() {
        const SCOPES: &[Scope] = &[Scope::System, Scope::Org, Scope::Project];
        let cap = SettingDef {
            key: "example.cap",
            example: "20",
            title: "Cap",
            description: "A number of which the smallest counts.",
            group: Group::Limits,
            schema: Schema::Int {
                min: Some(0),
                max: None,
                unit: Unit::Count,
            },
            class: Class::Policy,
            scopes: SCOPES,
            merge: Merge::Min,
            default: DefaultOf::Unset,
        };
        let (org, project) = (new_id(), new_id());
        let (at_system, at_org, at_project) = (json!(50), json!(20), json!(80));
        let layers = [
            Layer::at(Scope::System, None, &at_system, false),
            locked(Scope::Org, Some(org), &at_org),
            at_node(Scope::Project, project, &at_project),
        ];
        let resolved = resolve(&cap, &layers);
        assert_eq!(resolved.value, Some(json!(20)));
        assert_eq!(resolved.source.node, Some(org));
        assert_eq!(
            resolved.locked_by, None,
            "a lower scope can only narrow, so a lock changes nothing"
        );
        assert_eq!(resolve::<Id>(&cap, &[]).value, None);
        assert_eq!(resolve(&cap, &layers[2..]).value, Some(json!(80)));
    }

    #[test]
    fn places_can_raise_the_shipped_subagent_default_but_a_lower_scope_cannot_raise_an_ancestor_limit()
     {
        let (org, topic, thread) = (new_id(), new_id(), new_id());
        for (key, shipped, raised, least) in [
            (keys::AGENTS_SUBAGENTS_AT_ONCE, 8, 16, 3),
            (keys::AGENTS_SUBAGENT_DEPTH, 1, 4, 2),
        ] {
            let definition = def(key);
            assert_eq!(resolve::<Id>(definition, &[]).value, Some(json!(shipped)));
            let upper = json!(raised);
            assert_eq!(
                resolve(definition, &[at_node(Scope::Org, org, &upper)]).value,
                Some(json!(raised)),
                "the shipped value is not an invisible upper lock"
            );
            let lower = json!(least);
            let raised_again = json!(raised);
            let resolved = resolve(
                definition,
                &[
                    at_node(Scope::Org, org, &upper),
                    at_node(Scope::Topic, topic, &lower),
                    at_node(Scope::Thread, thread, &raised_again),
                ],
            );
            assert_eq!(resolved.value, Some(json!(least)));
            assert_eq!(resolved.source.node, Some(topic));
        }
    }

    #[test]
    fn a_default_that_the_environment_supplies_sits_under_every_row() {
        let (topic, on_machine, chosen) = (new_id(), json!("on_request"), json!("auto"));
        let supplied = resolve::<Id>(def(keys::APPROVAL_MODE), &[Layer::supplied(&on_machine)]);
        assert_eq!(supplied.value, Some(json!("on_request")));
        assert_eq!(
            supplied.source,
            Source {
                scope: SourceScope::Environment,
                node: None
            }
        );
        let layers = [
            at_node(Scope::Topic, topic, &chosen),
            Layer::supplied(&on_machine),
        ];
        let resolved = resolve(def(keys::APPROVAL_MODE), &layers);
        assert_eq!(resolved.value, Some(json!("auto")));
        assert_eq!(resolved.source.scope, SourceScope::Topic);
        assert_eq!(
            resolve::<Id>(def(keys::APPROVAL_MODE), &[]).value,
            None,
            "the registry has no mode of its own"
        );
    }

    #[test]
    fn a_things_own_value_comes_last() {
        let (project, for_project, own) = (new_id(), json!("on_start"), json!("manual"));
        let layers = [
            Layer::at(Scope::Environment, None, &own, false),
            at_node(Scope::Project, project, &for_project),
        ];
        let resolved = resolve(def(keys::ENV_UPDATE), &layers);
        assert_eq!(resolved.value, Some(json!("manual")));
        assert_eq!(
            resolved.source,
            Source {
                scope: SourceScope::EnvironmentOwn,
                node: None
            }
        );
        let locked_above = [
            Layer::at(Scope::Environment, None, &own, false),
            locked(Scope::Project, Some(project), &for_project),
        ];
        let resolved = resolve(def(keys::ENV_UPDATE), &locked_above);
        assert_eq!(
            resolved.value,
            Some(json!("on_start")),
            "a lock above takes the choice from the environment"
        );
        let own_lock = Layer::<Id>::at(Scope::Provider, None, &own, true);
        assert!(!own_lock.locked, "an own value carries no lock");
    }

    #[test]
    fn a_seeded_default_stands_in_the_registrys_place() {
        let (org, seeded, hidden) = (
            new_id(),
            json!({ "OpenAI": { "upstreams": [] }, "Anthropic": { "upstreams": [] } }),
            json!({ "OpenAI": null }),
        );
        let presets = def(keys::PROVIDERS_PRESETS);
        let shipped = resolve::<Id>(presets, &[Layer::default(&seeded)]);
        assert_eq!(shipped.value, Some(seeded.clone()));
        assert_eq!(shipped.source, Source::registry());
        let resolved = resolve(
            presets,
            &[Layer::default(&seeded), at_node(Scope::Org, org, &hidden)],
        );
        assert_eq!(
            resolved.value,
            Some(json!({ "OpenAI": null, "Anthropic": { "upstreams": [] } }))
        );
        assert_eq!(resolved.source.scope, SourceScope::Org);
    }

    #[test]
    fn a_row_that_says_null_sets_the_key_to_nothing() {
        let (shown, taken_down) = (
            json!({ "text": "Maintenance at six", "level": "warning", "until": null }),
            Json::Null,
        );
        let banner = def(keys::SYSTEM_BANNER);
        assert_eq!(
            resolve::<Id>(banner, &[Layer::at(Scope::System, None, &shown, false)]).value,
            Some(shown.clone())
        );
        let resolved = resolve::<Id>(
            banner,
            &[Layer::at(Scope::System, None, &taken_down, false)],
        );
        assert_eq!(resolved.value, None);
        assert_eq!(resolved.source.scope, SourceScope::System);
    }

    #[test]
    fn a_setting_locked_at_the_root_org_decides_three_levels_down() {
        let path: Vec<Id> = (0..4).map(|_| new_id()).collect();
        let (at_root, deeper) = (json!(30), json!(0));
        let mut layers = vec![locked(Scope::Org, Some(path[0]), &at_root)];
        layers.extend(
            path[1..]
                .iter()
                .map(|org| at_node(Scope::Org, *org, &deeper)),
        );
        let resolved = resolve(def(keys::RETENTION_ARCHIVE_IDLE_DAYS), &layers);
        assert_eq!(resolved.value, Some(json!(30)));
        assert_eq!(resolved.locked_by.and_then(|lock| lock.node), Some(path[0]));
    }
}
