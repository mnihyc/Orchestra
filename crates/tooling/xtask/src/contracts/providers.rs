//! Provider implementer documentation derives its wire shapes and limits from the contracts.
//! The worked programs are the very files the conformance suite starts, never a second example.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use orch_provider_contract::{address, choice, limits, members, source, wire};
use schemars::{JsonSchema, SchemaGenerator, generate::SchemaSettings};
use serde_json::Value;

const DIRECTORY: &str = "docs/development/extensions/providers";
const PAGES: [&str; 5] = ["README", "facts", "models", "resets", "credential"];

struct Artifact {
    path: PathBuf,
    text: String,
}

fn schema<T: JsonSchema>() -> Value {
    // Describe input acceptance: optional members may be absent. Each schema carries its own definitions.
    let settings = SchemaSettings::draft2020_12();
    SchemaGenerator::new(settings)
        .into_root_schema_for::<T>()
        .into()
}

fn schemas() -> BTreeMap<&'static str, Value> {
    BTreeMap::from([
        ("describe-params", schema::<wire::DescribeParams>()),
        ("source-descriptor", schema::<source::SourceDescriptor>()),
        ("source-ask", schema::<source::SourceAsk>()),
        ("facts-report", schema::<source::FactsReport>()),
        ("models-report", schema::<source::ModelsReport>()),
        ("reset-spend-params", schema::<wire::ResetSpendParams>()),
        ("reset-report", schema::<source::ResetReport>()),
        ("present-ask", schema::<source::PresentAsk>()),
        ("presented", schema::<source::Presented>()),
        ("source-error", schema::<source::SourceError>()),
        ("address-request", schema::<address::AddressRequest>()),
        ("address-reply", schema::<address::AddressReply>()),
        ("source-choice", schema::<choice::SourceChoice>()),
        ("provider-sources", schema::<choice::ProviderSources>()),
        ("source-spec", schema::<choice::SourceSpec>()),
        ("balance", schema::<members::Balance>()),
        ("spend", schema::<members::Spend>()),
        ("fact-member", schema::<members::FactMember>()),
        ("member-state", schema::<members::MemberState>()),
    ])
}

fn cell(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

fn shape(value: &Value) -> String {
    if let Some(reference) = value.get("$ref").and_then(Value::as_str) {
        return reference.rsplit('/').next().unwrap_or(reference).to_owned();
    }
    if let Some(value) = value.get("const") {
        return value.to_string();
    }
    if let Some(values) = value.get("enum").and_then(Value::as_array) {
        return values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(" or ");
    }
    for alternative in ["anyOf", "oneOf"] {
        if let Some(values) = value.get(alternative).and_then(Value::as_array) {
            return values.iter().map(shape).collect::<Vec<_>>().join(" or ");
        }
    }
    match value.get("type") {
        Some(Value::String(kind)) if kind == "array" => {
            format!(
                "{}[]",
                value.get("items").map_or_else(|| "JSON".into(), shape)
            )
        }
        Some(Value::String(kind)) if kind == "object" => match value.get("additionalProperties") {
            Some(Value::Object(_)) => format!("map of {}", shape(&value["additionalProperties"])),
            _ => "object".into(),
        },
        Some(Value::String(kind)) => value
            .get("format")
            .and_then(Value::as_str)
            .map_or_else(|| kind.clone(), |format| format!("{kind} ({format})")),
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" or "),
        _ => "JSON; see schema".into(),
    }
}

/// Transparent wrappers may put the root's members behind a local schema reference.
fn root_value(schema: &Value) -> &Value {
    let mut value = schema;
    let mut visited = BTreeSet::new();
    while let Some(reference) = value.get("$ref").and_then(Value::as_str) {
        if !visited.insert(reference) {
            break;
        }
        let Some(path) = reference.strip_prefix('#') else {
            break;
        };
        let Some(next) = schema.pointer(path) else {
            break;
        };
        value = next;
    }
    value
}

fn table(title: &str, value: &Value) -> String {
    let Some(properties) = value.get("properties").and_then(Value::as_object) else {
        if let Some(choices) = value.get("oneOf").and_then(Value::as_array)
            && choices
                .iter()
                .all(|choice| choice.get("const").is_some() || choice.get("enum").is_some())
        {
            let mut text = format!("**{title}**\n\n| Value | Meaning |\n|-------|---------|\n");
            for choice in choices {
                text.push_str(&format!(
                    "| `{}` | {} |\n",
                    cell(&shape(choice)),
                    cell(
                        choice
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                    )
                ));
            }
            return text;
        }
        return format!(
            "**{title}**: `{}`. See the linked schema for alternatives.\n",
            cell(&shape(value))
        );
    };
    let required = value.get("required").and_then(Value::as_array);
    let mut text = format!(
        "**{title}**\n\n| Member | JSON shape | Presence | Meaning |\n|--------|------------|----------|---------|\n"
    );
    for (name, property) in properties {
        let presence =
            if required.is_some_and(|names| names.iter().any(|item| item.as_str() == Some(name))) {
                "required"
            } else {
                "may be absent"
            };
        let description = property
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        text.push_str(&format!(
            "| `{}` | `{}` | {presence} | {} |\n",
            cell(name),
            cell(&shape(property)),
            cell(description)
        ));
    }
    text
}

fn tables(title: &str, schema: &Value, definitions: bool) -> String {
    let mut text = table(title, root_value(schema));
    if definitions {
        for key in ["$defs", "definitions"] {
            if let Some(definitions) = schema.get(key).and_then(Value::as_object) {
                for (name, value) in definitions {
                    if value.get("properties").is_some()
                        || value.get("enum").is_some()
                        || value.get("oneOf").is_some()
                    {
                        text.push('\n');
                        text.push_str(&table(name, value));
                    }
                }
            }
        }
    }
    text
}

fn methods() -> Result<String, String> {
    let rows = [
        (
            wire::DESCRIBE,
            "describe-params",
            "source-descriptor",
            "required",
        ),
        (wire::FACTS_READ, "source-ask", "facts-report", "facts"),
        (wire::MODELS_LIST, "source-ask", "models-report", "models"),
        (
            wire::RESETS_SPEND,
            "reset-spend-params",
            "reset-report",
            "facts.spends_resets",
        ),
        (
            wire::CREDENTIAL_PRESENT,
            "present-ask",
            "presented",
            "presents; keys, program only",
        ),
    ];
    if rows.iter().map(|row| row.0).collect::<Vec<_>>().as_slice() != wire::METHODS.as_slice() {
        return Err("provider method added without its documentation schema mapping".into());
    }
    let mut text = format!(
        "Contract **{}**.\n\n| Method | Params schema | Result schema | Declaration | Deadline |\n|--------|---------------|---------------|-------------|----------|\n",
        wire::CONTRACT
    );
    for (method, params, result, declaration) in rows {
        text.push_str(&format!("| `{method}` | [{params}](schema/{params}.json) | [{result}](schema/{result}.json) | {declaration} | {} s |\n", wire::deadline(method).as_secs()));
    }
    Ok(text)
}

fn errors() -> String {
    let mut text = "| Error code | Retryable |\n|------------|-----------|\n".to_owned();
    for code in source::SourceErrorCode::ALL {
        text.push_str(&format!("| `{}` | {} |\n", code.as_str(), code.retryable()));
    }
    text.push_str("\nThe error object is [SourceError](schema/source-error.json). HTTP 3xx is redirected (never followed), 401/403 unauthorized, 429/5xx unavailable; another non-200 status or a wrong envelope is malformed. A safe error is kept beside this element, retaining the last facts/time.\n");
    text.push('\n');
    text.push_str(&tables(
        "SourceError",
        &schema::<source::SourceError>(),
        true,
    ));
    text
}

fn bounds() -> String {
    let mut text = "| Bound | Value |\n|-------|-------|\n".to_owned();
    for (name, value, unit) in [
        ("CONTRACT", wire::CONTRACT.to_string(), "contract version"),
        (
            "DESCRIBE_DEADLINE",
            wire::DESCRIBE_DEADLINE.as_secs().to_string(),
            "seconds for describe",
        ),
        (
            "READ_DEADLINE",
            wire::READ_DEADLINE.as_secs().to_string(),
            "seconds for facts.read / credential.present",
        ),
        (
            "LIST_DEADLINE",
            wire::LIST_DEADLINE.as_secs().to_string(),
            "seconds for models.list / resets.spend",
        ),
        (
            "REPORT_MAX_BYTES",
            limits::REPORT_MAX_BYTES.to_string(),
            "bytes per reply",
        ),
        (
            "TEXT_MAX_CHARS",
            limits::TEXT_MAX_CHARS.to_string(),
            "characters",
        ),
        (
            "DECIMAL_MAX_CHARS",
            limits::DECIMAL_MAX_CHARS.to_string(),
            "characters of decimal text",
        ),
        (
            "ERROR_MESSAGE_MAX_CHARS",
            limits::ERROR_MESSAGE_MAX_CHARS.to_string(),
            "characters after redaction",
        ),
        ("WINDOWS_MAX", limits::WINDOWS_MAX.to_string(), "windows"),
        ("BALANCES_MAX", limits::BALANCES_MAX.to_string(), "balances"),
        ("SPEND_MAX", limits::SPEND_MAX.to_string(), "spend rows"),
        (
            "RESET_CREDITS_MAX",
            limits::RESET_CREDITS_MAX.to_string(),
            "reset credits",
        ),
        ("MODELS_MAX", limits::MODELS_MAX.to_string(), "models"),
        (
            "MIN_INTERVAL_LEAST_SECS",
            limits::MIN_INTERVAL_LEAST_SECS.to_string(),
            "seconds",
        ),
        (
            "DESCRIBE_EVERY",
            limits::DESCRIBE_EVERY.as_secs().to_string(),
            "seconds",
        ),
        (
            "PRESENTED_RENEW_BEFORE",
            limits::PRESENTED_RENEW_BEFORE.as_secs().to_string(),
            "seconds before expiry",
        ),
        (
            "PRESENTED_REUSE_WITHOUT_EXPIRY",
            limits::PRESENTED_REUSE_WITHOUT_EXPIRY.as_secs().to_string(),
            "seconds",
        ),
    ] {
        text.push_str(&format!("| `{name}` | {value} {unit} |\n"));
    }
    text.push_str(&format!("\nSecret replacement text: `{}`. Amount syntax: `^-?[0-9]+(\\.[0-9]+)?$`; UTC times use RFC 3339.\n", limits::REDACTED));
    text
}

/// Replaces exactly one whole marked block, preserving the authored words around it.
fn replace(text: &str, marker: &str, content: &str) -> Result<String, String> {
    let begin = format!("<!-- BEGIN {marker} -->");
    let end = format!("<!-- END {marker} -->");
    if text.matches(&begin).count() != 1 || text.matches(&end).count() != 1 {
        return Err(format!(
            "provider page needs exactly one {marker} begin/end marker"
        ));
    }
    let start = text
        .find(&begin)
        .ok_or_else(|| format!("missing {begin}"))?;
    let finish = text.find(&end).ok_or_else(|| format!("missing {end}"))?;
    let body = start + begin.len();
    if finish < body || (start != 0 && !text[..start].ends_with('\n')) {
        return Err(format!("provider page has misplaced {marker} markers"));
    }
    Ok(format!(
        "{}\n\n{}\n{}",
        &text[..body],
        content.trim_end_matches('\n'),
        &text[finish..]
    ))
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn example(root: &Path, name: &str) -> Result<String, String> {
    let path = root.join("examples/plugins/provider").join(name);
    let text = read(&path)?;
    if !text.ends_with('\n') || text.contains("\n```\n") {
        return Err(format!(
            "{} needs a final newline and no closing Markdown fence",
            path.display()
        ));
    }
    Ok(format!("```python\n{text}```\n"))
}

fn artifacts(root: &Path) -> Result<Vec<Artifact>, String> {
    let schemas = schemas();
    let mut result = Vec::new();
    for (name, schema) in &schemas {
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(schema).map_err(|error| error.to_string())?
        );
        result.push(Artifact {
            path: PathBuf::from(DIRECTORY)
                .join("schema")
                .join(format!("{name}.json")),
            text,
        });
    }
    let program = example(root, "plan.py")?;
    let address = example(root, "address.py")?;
    for page in PAGES {
        let path = PathBuf::from(DIRECTORY).join(format!("{page}.md"));
        let mut text = read(&root.join(&path))?;
        text = replace(&text, "provider.errors", &errors())?;
        text = replace(&text, "provider.bounds", &bounds())?;
        match page {
            "README" => {
                text = replace(&text, "provider.methods", &methods()?)?;
                text = replace(
                    &text,
                    "provider.descriptor",
                    &tables("SourceDescriptor", &schemas["source-descriptor"], true),
                )?;
            }
            "facts" => {
                let members = root_value(&schemas["facts-report"])
                    .get("properties")
                    .and_then(Value::as_object)
                    .ok_or("FactsReport schema has no member properties")?;
                if members::FactMember::ALL
                    .iter()
                    .any(|member| !members.contains_key(member.as_str()))
                    || members.len() != members::FactMember::ALL.len()
                {
                    return Err("FactsReport schema and FactMember differ".into());
                }
                let fields = format!(
                    "{}\n{}",
                    tables("SourceAsk", &schemas["source-ask"], true),
                    tables("FactsReport", &schemas["facts-report"], false)
                );
                text = replace(&text, "provider.facts", &fields)?;
                text = replace(
                    &text,
                    "provider.facts-shapes",
                    &tables("FactsReport item shapes", &schemas["facts-report"], true),
                )?;
                text = replace(&text, "example.program", &program)?;
                text = replace(&text, "example.address", &address)?;
            }
            "models" | "resets" | "credential" => {
                let (ask, reply) = match page {
                    "models" => ("source-ask", "models-report"),
                    "resets" => ("reset-spend-params", "reset-report"),
                    _ => ("present-ask", "presented"),
                };
                let fields = format!(
                    "{}\n{}",
                    tables(ask, &schemas[ask], true),
                    tables(reply, &schemas[reply], true)
                );
                text = replace(&text, &format!("provider.{page}"), &fields)?;
                text = replace(&text, "example.program", &program)?;
            }
            _ => return Err(format!("unknown provider page {page}")),
        }
        result.push(Artifact { path, text });
    }
    Ok(result)
}

/// Writes every schema/page, or compares every expected byte without changing them.
pub fn run(root: &Path, check: bool) -> Result<(), String> {
    // Resolve all inputs and markers before writing any artifact.
    for artifact in artifacts(root)? {
        let path = root.join(&artifact.path);
        if check {
            if read(&path)? != artifact.text {
                return Err(format!(
                    "{} is out of date: run cargo xtask contracts",
                    artifact.path.display()
                ));
            }
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
            std::fs::write(&path, artifact.text)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_provider_pages_schemas_and_worked_examples_match_the_contracts() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        run(&root, true).unwrap_or_else(|error| panic!("{error}"));
        let facts = read(&root.join(DIRECTORY).join("facts.md")).unwrap();
        let code = read(&root.join("examples/plugins/provider/plan.py")).unwrap();
        assert!(facts.contains(&format!("```python\n{code}```")));
        assert!(facts.contains("`balances`"));
        assert!(facts.contains("`spend`"));
        assert!(facts.contains("Not read yet"));
        assert!(facts.contains("no recovery proof"));
    }

    #[test]
    fn a_missing_duplicate_or_reversed_marker_refuses_a_page_instead_of_erasing_its_words() {
        let good =
            "Before\n<!-- BEGIN provider.facts -->\nold\n<!-- END provider.facts -->\nAfter\n";
        let changed = replace(good, "provider.facts", "fresh\n").unwrap();
        assert!(changed.starts_with("Before\n"));
        assert!(changed.ends_with("After\n"));
        assert!(!changed.contains("old"));
        assert!(replace("Before\nAfter\n", "provider.facts", "fresh").is_err());
        assert!(replace(&format!("{good}{good}"), "provider.facts", "fresh").is_err());
        assert!(
            replace(
                "<!-- END provider.facts -->\n<!-- BEGIN provider.facts -->",
                "provider.facts",
                "fresh"
            )
            .is_err()
        );
    }

    #[test]
    fn a_transparent_report_still_lists_its_members_for_the_reader() {
        let schema = serde_json::json!({ "$ref": "#/$defs/Report", "$defs": { "Report": {
            "type": "object", "properties": { "balances": { "type": "array", "items": { "type": "string" } } }
        } } });
        let text = tables("Report", &schema, false);
        assert!(text.contains("`balances`"));
        assert!(text.contains("`string[]`"));
    }
}
