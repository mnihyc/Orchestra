//! Schema-declared identities and private operands are kept separate from displayed text.
use crate::Json;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
    Exact,
    Form,
    Keys,
}

pub fn normalize(root: &Json, value: &mut Json) -> u64 {
    normalize_in(root, value, true)
}
pub fn normalize_public(root: &Json, value: &mut Json) -> u64 {
    normalize_in(root, value, false)
}
fn normalize_in(root: &Json, value: &mut Json, input: bool) -> u64 {
    let mut paths = Vec::new();
    collect(root, root, value, String::new(), input, &mut paths);
    paths.sort();
    paths.dedup();
    let mut count = 0;
    let held: Vec<_> = paths
        .into_iter()
        .filter_map(|(path, role)| {
            value.pointer_mut(&path).map(|slot| {
                count += match role {
                    Role::Exact => 0,
                    Role::Form => repair_form(slot),
                    Role::Keys => repair_values(slot),
                };
                (path, std::mem::take(slot))
            })
        })
        .collect();
    count += super::normalize_json(value);
    for (path, original) in held {
        if let Some(slot) = value.pointer_mut(&path) {
            *slot = original;
        }
    }
    count
}
fn repair_values(value: &mut Json) -> u64 {
    match value {
        Json::Object(values) => values.values_mut().map(repair_values).sum(),
        Json::Array(values) => values.iter_mut().map(repair_values).sum(),
        Json::String(text) => super::normalize_text(text),
        _ => 0,
    }
}
fn repair_form(value: &mut Json) -> u64 {
    let Some(object) = value.as_object_mut() else {
        return 0;
    };
    let mut count = 0;
    for (keyword, value) in object {
        match keyword.as_str() {
            "$ref" | "$id" | "required" | "dependentRequired" | "type" | "format" => {}
            "properties" | "$defs" | "definitions" | "patternProperties" | "dependentSchemas" => {
                if let Some(properties) = value.as_object_mut() {
                    for (name, schema) in properties {
                        if name.contains('\0')
                            && let Some(object) = schema.as_object_mut()
                            && !object.contains_key("title")
                        {
                            object.insert("title".to_owned(), Json::String(name.clone()));
                        }
                        count += repair_form(schema);
                    }
                }
            }
            "items"
            | "additionalProperties"
            | "contains"
            | "propertyNames"
            | "not"
            | "if"
            | "then"
            | "else" => count += repair_form(value),
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                if let Some(values) = value.as_array_mut() {
                    count += values.iter_mut().map(repair_form).sum::<u64>();
                }
            }
            "enum" => {
                if let Some(values) = value.as_array_mut() {
                    let mut used: Vec<_> = values
                        .iter()
                        .filter(|value| !super::needs_repair(value))
                        .cloned()
                        .collect();
                    for value in values {
                        let original = value.clone();
                        count += super::normalize_json(value);
                        if *value != original
                            && let Some(base) = value.as_str().map(str::to_owned)
                        {
                            let mut ordinal = 2usize;
                            while used.contains(value) {
                                *value = Json::String(format!("{base} ({ordinal})"));
                                ordinal += 1;
                            }
                        }
                        used.push(value.clone());
                    }
                }
            }
            _ => count += super::normalize_json(value),
        }
    }
    count
}

fn resolve<'a>(root: &'a Json, mut schema: &'a Json) -> &'a Json {
    while let Some(reference) = schema.get("$ref").and_then(Json::as_str) {
        let Some(next) = reference
            .strip_prefix('#')
            .and_then(|path| root.pointer(path))
        else {
            break;
        };
        if std::ptr::eq(next, schema) {
            break;
        }
        schema = next;
    }
    schema
}

fn accepts(root: &Json, schema: &Json, value: &Json) -> bool {
    let schema = resolve(root, schema);
    if let Some(expected) = schema.get("const") {
        return expected == value;
    }
    if let Some(choices) = schema.get("enum").and_then(Json::as_array) {
        return choices.contains(value);
    }
    if let Some(kind) = schema.get("type").and_then(Json::as_str) {
        let matches = match kind {
            "null" => value.is_null(),
            "string" => value.is_string(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "boolean" => value.is_boolean(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            _ => true,
        };
        if !matches {
            return false;
        }
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Json::as_object),
        value.as_object(),
    ) {
        if schema
            .get("required")
            .and_then(Json::as_array)
            .is_some_and(|required| {
                required
                    .iter()
                    .filter_map(Json::as_str)
                    .any(|name| !object.contains_key(name))
            })
        {
            return false;
        }
        for (name, property) in properties {
            let property = resolve(root, property);
            if (property.get("const").is_some() || property.get("enum").is_some())
                && object
                    .get(name)
                    .is_some_and(|value| !accepts(root, property, value))
            {
                return false;
            }
        }
    }
    true
}

fn nodes<'a>(root: &'a Json, schema: &'a Json, value: &Json, out: &mut Vec<&'a Json>) {
    out.push(schema);
    let resolved = resolve(root, schema);
    if !std::ptr::eq(resolved, schema) {
        out.push(resolved);
    }
    if let Some(all) = resolved.get("allOf").and_then(Json::as_array) {
        for child in all {
            nodes(root, child, value, out);
        }
    }
    for kind in ["oneOf", "anyOf"] {
        if let Some(child) = resolved
            .get(kind)
            .and_then(Json::as_array)
            .and_then(|choices| choices.iter().find(|schema| accepts(root, schema, value)))
        {
            nodes(root, child, value, out);
        }
    }
}

fn collect(
    root: &Json,
    schema: &Json,
    value: &Json,
    path: String,
    input: bool,
    out: &mut Vec<(String, Role)>,
) {
    let mut roles = Vec::new();
    nodes(root, schema, value, &mut roles);
    if roles.iter().any(|role| {
        role.get("x-native-units").and_then(Json::as_str) == Some("bytes")
            || role.get("x-native-reference").and_then(Json::as_bool) == Some(true)
            || input && role.get("x-storage-exact").and_then(Json::as_bool) == Some(true)
    }) {
        out.push((path, Role::Exact));
        return;
    }
    if roles
        .iter()
        .any(|role| role.get("x-native-schema").and_then(Json::as_bool) == Some(true))
    {
        out.push((path, Role::Form));
        return;
    }
    if roles
        .iter()
        .any(|role| role.get("x-native-keys").and_then(Json::as_bool) == Some(true))
    {
        out.push((path, Role::Keys));
        return;
    }
    match value {
        Json::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                for child in roles.iter().filter_map(|role| role.get("items")) {
                    collect(root, child, value, format!("{path}/{index}"), input, out);
                }
            }
        }
        Json::Object(values) => {
            for (name, value) in values {
                let children: Vec<_> = roles
                    .iter()
                    .filter_map(|role| {
                        role.get("properties")
                            .and_then(|properties| properties.get(name))
                    })
                    .collect();
                for child in children {
                    collect(
                        root,
                        child,
                        value,
                        format!("{path}/{}", name.replace('~', "~0").replace('/', "~1")),
                        input,
                        out,
                    );
                }
            }
        }
        _ => {}
    }
}
