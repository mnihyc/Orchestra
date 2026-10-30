//! The combination matrix of places and inheritance: every key of the registry that a node's path resolves,
//! written at chosen scopes of an eight-level path (the system, a root org, a department, a department inside it, a
//! project, a folder, a topic and a thread), with or without a lock, read at the deepest place the key lists or half
//! way down, and compared with the key's declared merge rule.
//!
//! Dimensions: the merge of the key (replace, union, intersect, map, append, min), how many scopes set it, where a lock
//! stands among them, where it is read, and whether the scopes set equal or different values. The key itself and the
//! scopes are drawn with the case's seed among those the registry allows. Expected values follow these merge rules:
//! replace takes the nearest scope, or the highest locked one; union joins the arrays from the top, each item once,
//! ignoring rows below a lock; intersect keeps what every scope that sets the key names, a lock changing nothing; map
//! merges members, a lower scope overriding, ignoring rows below a lock; append joins texts from the top with one
//! blank line, ignoring rows below a lock; min takes the smallest number among the scopes that set the key, a lock
//! changing nothing (a place may raise a shipped default because the default is not a configured scope).
//! With no scope setting the key, the registry's default holds. Rows that a fresh database already holds (seeds) are
//! read and counted like any other row.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "../../engine/tests/matrix_kit/mod.rs"]
mod matrix_kit;
mod support;

use matrix_kit::{Case, Matrix, Report, Seeded, dim, failure};
use orch_settings::{Place, Target};
use orch_settings_contract::registry::{self, DefaultOf, Merge, Schema, Scope, SettingDef};
use orch_types::{Id, Json};
use serde_json::json;
use support::World;

const SOURCE: &str = "Declared merge rules, inherited locks and defaults; configured minimum values may exceed the default";

/// One place of the path, from the top, with the scope its rows have.
#[derive(Debug, Clone, Copy)]
struct Place8 {
    name: &'static str,
    scope: Scope,
    node: Option<Id>,
}

/// The path of the matrix: the system, three orgs, two projects, a topic and a thread.
async fn path(world: &World) -> Vec<Place8> {
    let lab = world.node("org", world.department, "Lab").await;
    let site = world.node("project", lab, "Site").await;
    let pages = world.node("project", site, "Pages").await;
    let topic = world.node("topic", pages, "Login").await;
    let thread = world.node("thread", topic, "Fix").await;
    vec![
        Place8 {
            name: "system",
            scope: Scope::System,
            node: None,
        },
        Place8 {
            name: "root org",
            scope: Scope::Org,
            node: Some(world.root),
        },
        Place8 {
            name: "department",
            scope: Scope::Org,
            node: Some(world.department),
        },
        Place8 {
            name: "inner department",
            scope: Scope::Org,
            node: Some(lab),
        },
        Place8 {
            name: "project",
            scope: Scope::Project,
            node: Some(site),
        },
        Place8 {
            name: "folder",
            scope: Scope::Project,
            node: Some(pages),
        },
        Place8 {
            name: "topic",
            scope: Scope::Topic,
            node: Some(topic),
        },
        Place8 {
            name: "thread",
            scope: Scope::Thread,
            node: Some(thread),
        },
    ]
}

/// The keys a node's path resolves, by merge: those whose default is a value or nothing (a default supplied by an
/// environment or another key depends on more than the path).
fn keys_of(merge: Merge) -> Vec<&'static SettingDef> {
    registry::all()
        .iter()
        .filter(|def| def.merge == merge)
        .filter(|def| matches!(def.default, DefaultOf::Value(_) | DefaultOf::Unset))
        .filter(|def| {
            def.scopes.iter().any(|scope| {
                matches!(
                    scope,
                    Scope::System | Scope::Org | Scope::Project | Scope::Topic | Scope::Thread
                )
            })
        })
        .collect()
}

/// A value of `schema`, drawn with `seeded`; none where the schema is not one this matrix can draw.
fn draw(schema: &Schema, seeded: &mut Seeded, n: usize) -> Option<Json> {
    Some(match schema {
        Schema::Bool => json!(seeded.chance(50)),
        Schema::Int { min, max, .. } => {
            let low = min.unwrap_or(0);
            let high = max.unwrap_or(low + 1_000).min(low + 1_000);
            json!(
                low + i64::try_from(seeded.below(usize::try_from(high - low + 1).unwrap_or(1)))
                    .unwrap_or(0)
            )
        }
        Schema::Choice { values } => json!(seeded.pick(values)),
        Schema::Text {
            form: registry::Form::Any,
            max_chars,
        } => {
            let text = format!("Rule {n}: keep the build green");
            json!(
                text.chars()
                    .take(usize::try_from(max_chars.unwrap_or(200)).unwrap_or(200))
                    .collect::<String>()
            )
        }
        Schema::List { items, unique: _ } => {
            if let Schema::Choice { values } = items {
                let picked: Vec<&str> = values
                    .iter()
                    .copied()
                    .filter(|_| seeded.chance(60))
                    .collect();
                json!(picked)
            } else {
                let count = 1 + seeded.below(3);
                let mut list = Vec::new();
                for at in 0..count {
                    let item = draw(items, seeded, n * 10 + at)?;
                    if !list.contains(&item) {
                        list.push(item);
                    }
                }
                Json::Array(list)
            }
        }
        Schema::Map {
            names: registry::Names::Any,
            values,
        } => {
            let mut map = serde_json::Map::new();
            for name in ["alpha", "beta", "gamma"] {
                if seeded.chance(60) {
                    map.insert(name.to_owned(), draw(values, seeded, n)?);
                }
            }
            Json::Object(map)
        }
        Schema::Object { members, open: _ } => {
            let mut object = serde_json::Map::new();
            for member in *members {
                if member.required || seeded.chance(50) {
                    object.insert(member.name.to_owned(), draw(&member.schema, seeded, n)?);
                }
            }
            Json::Object(object)
        }
        Schema::Nullable { of } => draw(of, seeded, n)?,
        Schema::OneOf { any, .. } => any
            .iter()
            .find_map(|schema| draw(schema, &mut seeded.clone(), n))?,
        _ => return None,
    })
}

/// A value for the key that the key accepts: drawn from its schema, or else its example.
fn value_for(def: &SettingDef, seeded: &mut Seeded, n: usize) -> Option<Json> {
    for _ in 0..8 {
        if let Some(value) = draw(&def.schema, seeded, n)
            && def.faults(&value).is_empty()
        {
            return Some(value);
        }
    }
    def.example_value()
        .ok()
        .filter(|value| def.faults(value).is_empty())
}

/// A row on the path: where, what, and whether it is locked.
#[derive(Debug, Clone)]
struct Layer {
    at: usize,
    value: Json,
    locked: bool,
}

/// Merge the layers along the place's path, from the top, using the key's declared rule.
fn expected(def: &SettingDef, layers: &[Layer]) -> Option<Json> {
    if layers.is_empty() {
        return def.default_value().filter(|value| !value.is_null());
    }
    let locked = layers.iter().position(|layer| layer.locked);
    // For union, map and append the rows below a locked row are ignored.
    let honoured = |layers: &[Layer]| -> Vec<Layer> {
        match locked {
            Some(at) => layers[..=at].to_vec(),
            None => layers.to_vec(),
        }
    };
    let value = match def.merge {
        Merge::Replace => match locked {
            Some(at) => layers[at].value.clone(),
            None => layers.last().expect("a layer").value.clone(),
        },
        Merge::Union => {
            let mut all: Vec<Json> = Vec::new();
            for layer in honoured(layers) {
                for item in layer.value.as_array().cloned().unwrap_or_default() {
                    if !all.contains(&item) {
                        all.push(item);
                    }
                }
            }
            Json::Array(all)
        }
        Merge::Intersect => {
            let mut kept: Vec<Json> = layers[0].value.as_array().cloned().unwrap_or_default();
            for layer in &layers[1..] {
                let names = layer.value.as_array().cloned().unwrap_or_default();
                kept.retain(|item| names.contains(item));
            }
            Json::Array(kept)
        }
        Merge::Map => {
            let mut merged = serde_json::Map::new();
            for layer in honoured(layers) {
                for (name, value) in layer.value.as_object().cloned().unwrap_or_default() {
                    merged.insert(name, value);
                }
            }
            Json::Object(merged)
        }
        Merge::Append => {
            let texts: Vec<String> = honoured(layers)
                .iter()
                .filter_map(|layer| layer.value.as_str().map(str::to_owned))
                .collect();
            json!(texts.join("\n\n"))
        }
        Merge::Min => {
            let least = layers.iter().filter_map(|layer| layer.value.as_i64()).min();
            least.map_or(Json::Null, |least| json!(least))
        }
    };
    Some(value).filter(|value| !value.is_null())
}

/// Compare resolved values; an intersection is a set with no defined order.
fn agree(def: &SettingDef, expected: &Option<Json>, got: &Option<Json>) -> bool {
    match (def.merge, expected, got) {
        (Merge::Intersect, Some(Json::Array(a)), Some(Json::Array(b))) => {
            let mut a: Vec<String> = a.iter().map(Json::to_string).collect();
            let mut b: Vec<String> = b.iter().map(Json::to_string).collect();
            a.sort();
            b.sort();
            a == b
        }
        _ => expected == got,
    }
}

fn merge_word(merge: Merge) -> &'static str {
    match merge {
        Merge::Replace => "replace",
        Merge::Union => "union",
        Merge::Intersect => "intersect",
        Merge::Map => "map",
        Merge::Append => "append",
        Merge::Min => "min",
    }
}

fn merge_of(word: &str) -> Merge {
    match word {
        "replace" => Merge::Replace,
        "union" => Merge::Union,
        "intersect" => Merge::Intersect,
        "map" => Merge::Map,
        "append" => Merge::Append,
        _ => Merge::Min,
    }
}

/// The rows of the key that the database already holds on the path (a seed), as layers.
async fn rows_already(world: &World, def: &SettingDef, places: &[Place8]) -> Vec<Layer> {
    let rows: Vec<(Option<Id>, Json, bool)> =
        sqlx::query_as("SELECT node_id, value, locked FROM settings WHERE key = $1")
            .bind(def.key)
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    let mut layers: Vec<Layer> = rows
        .into_iter()
        .filter_map(|(node, value, locked)| {
            places
                .iter()
                .position(|place| place.node == node)
                .map(|at| Layer { at, value, locked })
        })
        .collect();
    layers.sort_by_key(|layer| layer.at);
    layers
}

/// One case: a key drawn for its merge, rows written as the case says, the value read and compared.
async fn run(case: &Case) -> Result<String, matrix_kit::Failure> {
    let merge = merge_of(case.get("merge"));
    let keys = keys_of(merge);
    if keys.is_empty() {
        return Ok(format!("no key merges by {}", case.get("merge")));
    }
    let mut seeded = Seeded::new(case.seed ^ (case.index as u64).wrapping_mul(0x9E37_79B9));
    let def = *seeded.pick(&keys);
    let world = World::new().await;
    let places = path(&world).await;
    // The places the key may be written at, from the top.
    let open: Vec<usize> = (0..places.len())
        .filter(|&at| def.scopes.contains(&places[at].scope))
        .collect();
    if open.is_empty() {
        return Ok(format!("{} lists no scope of a node's path", def.key));
    }
    // Where it is read: the deepest place it lists, or one half way down the places it lists.
    let read_at = match case.get("read") {
        "middle" if open.len() > 2 => open[open.len() / 2],
        _ => *open.last().expect("a place"),
    };
    let reachable: Vec<usize> = open.iter().copied().filter(|&at| at <= read_at).collect();
    let wanted = match case.get("levels") {
        "0" => 0,
        "1" => 1,
        "2" => 2,
        "3" => 3,
        _ => reachable.len(),
    }
    .min(reachable.len());
    // The scopes that set it, drawn, kept in order from the top.
    let mut chosen = reachable.clone();
    while chosen.len() > wanted {
        let drop = seeded.below(chosen.len());
        chosen.remove(drop);
    }
    let lock_at = match (case.get("lock"), chosen.len()) {
        (_, 0) | ("none", _) => None,
        ("top", _) => Some(chosen[0]),
        ("bottom", n) => Some(chosen[n - 1]),
        (_, n) => Some(chosen[n / 2]),
    };
    let first = value_for(def, &mut seeded, 0);
    let Some(first) = first else {
        return Ok(format!(
            "{}: no value of its schema could be drawn",
            def.key
        ));
    };
    let mut written: Vec<Layer> = Vec::new();
    for (n, &at) in chosen.iter().enumerate() {
        let value = if case.is("spread", "equal") {
            first.clone()
        } else {
            value_for(def, &mut seeded, n + 1).unwrap_or_else(|| first.clone())
        };
        written.push(Layer {
            at,
            value,
            locked: lock_at == Some(at),
        });
    }
    // Unlocked rows first, from the top, then the locked one: a row under a lock may be refused, and the case is
    // about what is read, not about the order of writing.
    let target = |at: usize| places[at].node.map_or(Target::System, Target::Node);
    for layer in written.iter().filter(|layer| !layer.locked) {
        if let Err(error) = world
            .try_set(
                target(layer.at),
                def.key,
                layer.value.clone(),
                None,
                None,
                true,
            )
            .await
        {
            return Ok(format!(
                "{}: the write at the {} was refused ({error}); not counted",
                def.key, places[layer.at].name
            ));
        }
    }
    for layer in &mut written {
        if !layer.locked {
            continue;
        }
        if let Err(error) = world
            .try_set(
                target(layer.at),
                def.key,
                layer.value.clone(),
                Some(true),
                None,
                true,
            )
            .await
        {
            // A lock that would change nothing (a min or an intersect key, the innermost scope) is refused; the row is
            // written without it and the case goes on as one without a lock.
            if !error.to_string().contains("would change nothing") {
                return Ok(format!(
                    "{}: the locked write at the {} was refused ({error}); not counted",
                    def.key, places[layer.at].name
                ));
            }
            if let Err(error) = world
                .try_set(
                    target(layer.at),
                    def.key,
                    layer.value.clone(),
                    None,
                    None,
                    true,
                )
                .await
            {
                return Ok(format!(
                    "{}: the write at the {} was refused ({error}); not counted",
                    def.key, places[layer.at].name
                ));
            }
            layer.locked = false;
        }
    }
    let layers: Vec<Layer> = rows_already(&world, def, &places)
        .await
        .into_iter()
        .filter(|layer| layer.at <= read_at)
        .collect();
    let want = expected(def, &layers);
    let read = places[read_at].node.map_or(Place::System, Place::Node);
    let got = {
        let mut conn = world.conn().await;
        match orch_settings::resolve(&mut conn, read, def.key).await {
            Ok(view) => view.resolved.value,
            Err(error) => {
                return Err(failure(
                    format!(
                        "{want:?} (the key is resolved at the {})",
                        places[read_at].name
                    ),
                    SOURCE,
                    format!("an error: {error}"),
                ));
            }
        }
    };
    let story = format!(
        "key {} ({}), read at the {}, rows [{}], default {:?}",
        def.key,
        merge_word(def.merge),
        places[read_at].name,
        layers
            .iter()
            .map(|layer| format!(
                "{}{}={}",
                places[layer.at].name,
                if layer.locked { " (locked)" } else { "" },
                layer.value
            ))
            .collect::<Vec<_>>()
            .join("; "),
        def.default_value()
    );
    if agree(def, &want, &got) {
        Ok(story)
    } else {
        Err(failure(
            format!("{want:?}; {story}"),
            SOURCE,
            format!("{got:?}"),
        ))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_merge_resolves_along_an_eight_level_path_as_the_rules_say() {
    let matrix = Matrix::new(
        "inheritance",
        &[
            dim(
                "merge",
                &["replace", "union", "intersect", "map", "append", "min"],
            ),
            dim("levels", &["0", "1", "2", "3", "all"]),
            dim("lock", &["none", "top", "middle", "bottom"]),
            dim("read", &["deepest", "middle"]),
            dim("spread", &["equal", "distinct"]),
        ],
        |pick| {
            let levels = pick.get("levels");
            // Nothing to lock or to tell apart without rows; one row has one place for a lock and one value.
            !(levels == "0" && (!pick.is("lock", "none") || !pick.is("spread", "distinct")))
                && !(levels == "1"
                    && (pick.is("lock", "middle")
                        || pick.is("lock", "bottom")
                        || pick.is("spread", "equal")))
                && !(levels == "2" && pick.is("lock", "middle"))
        },
    );
    let cases = matrix.cases(11, &[&["merge", "levels", "lock"]], 20_261_015, 40);
    let mut report = Report::default();
    let mut skipped = Vec::new();
    // One case after another: each has a database of its own and takes a fraction of a second.
    for case in &cases {
        match run(case).await {
            Ok(story)
                if story.contains("not counted")
                    || story.starts_with("no key")
                    || story.contains("could be drawn")
                    || story.contains("lists no scope") =>
            {
                skipped.push(format!("{}: {story}", case.id()));
            }
            Ok(_) => report.passed(),
            Err(failure) => report.record(case, Err(failure)),
        }
    }
    if !skipped.is_empty() {
        eprintln!(
            "matrix inheritance: {} cases not counted:\n  {}",
            skipped.len(),
            skipped.join("\n  ")
        );
    }
    report.finish("inheritance");
}
