//! The model-facing projection of structured Orch tool results.
//!
//! The engine keeps its complete record. Only delivery omits optional ids and renders reserved marks.
//! A confirmable plan is never shortened: the engine uses this same projection before keeping its word.

use serde_json::{Value, json};

pub const DEFAULT_BYTES: usize = 20_000;
pub const MAX_BYTES: usize = 40_000;

#[must_use]
pub fn max_bytes(arguments: &Value) -> usize {
    arguments
        .get("max_bytes")
        .and_then(Value::as_u64)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .unwrap_or(DEFAULT_BYTES)
        .min(MAX_BYTES)
}

/// Authored bytes stay in their source; they cannot introduce one of Orch's frames at delivery.
#[must_use]
pub fn supplied(text: &str) -> String {
    let mut rendered = text.to_owned();
    for prefix in ["<orch", "</orch", "<relayed", "</relayed"] {
        rendered = rendered.replace(prefix, &format!("<\\{}", &prefix[1..]));
    }
    rendered
}

fn render(value: &mut Value) {
    match value {
        Value::String(text) => *text = supplied(text),
        Value::Array(entries) => entries.iter_mut().for_each(render),
        Value::Object(fields) => fields.values_mut().for_each(render),
        _ => {}
    }
}

/// Optional object ids are different from the browser/display/opening/request handles a later call needs.
#[must_use]
pub fn structured(tool: &str, arguments: &Value, value: &Value) -> Value {
    let mut shown = value.clone();
    if arguments.get("ids").and_then(Value::as_bool) != Some(true)
        && let Some(fields) = shown.as_object_mut()
    {
        match tool {
            "edit_document" => {
                fields.remove("id");
            }
            "announce" => {
                fields.remove("announcement_id");
            }
            "manage_shared" => {
                if let Some(entries) = fields.get_mut("affects").and_then(Value::as_array_mut) {
                    for entry in entries {
                        if let Some(fields) = entry.as_object_mut() {
                            fields.remove("id");
                        }
                    }
                }
            }
            "use_browser" | "use_display" => {
                fields.remove("opened_by");
                for key in ["browsers", "displays"] {
                    if let Some(entries) = fields.get_mut(key).and_then(Value::as_array_mut) {
                        for entry in entries {
                            if let Some(fields) = entry.as_object_mut() {
                                fields.remove("opened_by");
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    render(&mut shown);
    shown
}

#[must_use]
pub fn fits(tool: &str, arguments: &Value, value: &Value) -> bool {
    structured(tool, arguments, value).to_string().len() <= max_bytes(arguments)
}

/// Keep whole detail records and the effect's actual outcome. A plan or a port page cannot be cut here:
/// its issuer must check the plan before issuing a word, or produce the page's own continuation cursor.
pub fn bounded(tool: &str, arguments: &Value, value: &Value) -> Result<Value, &'static str> {
    let budget = max_bytes(arguments);
    let mut shown = structured(tool, arguments, value);
    // The port producer must check its canonical DTO before an effect or consuming a once-only link.
    // Do not turn an already-created/closed opening into a size refusal or lose its disclosed link here.
    if matches!(tool, "open_port" | "close_port") {
        return Ok(shown);
    }
    if shown.to_string().len() <= budget {
        return Ok(shown);
    }
    if (tool == "manage_shared" && shown.get("done").and_then(Value::as_bool) != Some(true))
        || matches!(tool, "list_ports" | "list_port_connections")
    {
        return Err(
            "The complete answer does not fit max_bytes. Narrow the paths or filters, or request more max_bytes (at most 40000). No confirmation word is offered.",
        );
    }
    let mut omitted = 0usize;
    // These describe a result, not whether the effect happened or its handle. Remove whole fields/records.
    let Some(fields) = shown.as_object_mut() else {
        return Err("The complete answer does not fit max_bytes. Request a larger bound.");
    };
    fields.insert("truncated".to_owned(), json!(true));
    fields.insert("left_out".to_owned(), json!("Some detail records were omitted to fit max_bytes; the reported outcome is unchanged. Read the affected paths, or list with narrower filters or more max_bytes (at most 40000)."));
    for key in [
        "tabs",
        "watch",
        "note",
        "opened_by",
        "playwright_cli",
        "profiles_dir",
        // What a written text's references resolved to describes the text; the change itself stays.
        "references",
        // What the reader kept as words or could not find describes the text too.
        "warnings",
        "missing",
        "added_files",
    ] {
        if shown.to_string().len() <= budget {
            return Ok(shown);
        }
        if shown
            .as_object_mut()
            .and_then(|fields| fields.remove(key))
            .is_some()
        {
            omitted += 1;
        }
    }
    for key in [
        "results", "files", "browsers", "profiles", "displays", "servers",
    ] {
        loop {
            if shown.to_string().len() <= budget {
                return Ok(shown);
            }
            let removed = shown
                .get_mut(key)
                .and_then(Value::as_array_mut)
                .and_then(Vec::pop);
            if removed.is_none() {
                break;
            }
            omitted += 1;
            shown["omitted_records"] = json!(omitted);
        }
    }
    // Long display names and paths are descriptions. A required handle can replace an optional path/id
    // pair, but states, counters, sequences and confirmation outcomes are never converted or invented.
    for key in ["name", "thread", "chat", "profile", "geometry", "notice"] {
        if shown.to_string().len() <= budget {
            return Ok(shown);
        }
        if let Some(fields) = shown.as_object_mut() {
            fields.remove(key);
        }
    }
    if shown.to_string().len() > budget && tool == "edit_document" && value.get("id").is_some() {
        if let Some(fields) = shown.as_object_mut() {
            fields.remove("path");
        }
        shown["id"] = value["id"].clone(); // Now required for naming the changed document, even with ids=false.
    }
    if shown.to_string().len() <= budget {
        Ok(shown)
    } else {
        Err(
            "The complete outcome does not fit max_bytes. Request a larger bound; inspect the affected entry before repeating a changing call.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_omits_optional_ids_and_breaks_reserved_marks_without_changing_the_source() {
        let source = json!({"id":"opaque", "path":"docs/<orchards>.md", "seq":4, "created":true});
        let shown = bounded("edit_document", &json!({}), &source).unwrap();
        assert!(shown.get("id").is_none());
        assert_eq!(shown["path"], "docs/<\\orchards>.md");
        assert_eq!(shown["seq"], 4);
        assert_eq!(source["path"], "docs/<orchards>.md");
        assert_eq!(
            bounded("edit_document", &json!({"ids":true}), &source).unwrap()["id"],
            "opaque"
        );
        let handle = json!({"status":"pending", "request_id":"required"});
        assert_eq!(
            bounded("request_capability", &json!({}), &handle).unwrap(),
            handle
        );
    }

    #[test]
    fn the_references_a_written_text_names_give_way_to_the_bound_and_the_change_stays() {
        let long = "x".repeat(2_000);
        let source = json!({
            "path": "docs/a.md", "seq": 4, "created": false, "version": 1, "blocks": 2, "files": 0,
            "references": { "resolved": [{ "path": long, "kind": "document", "name": "A" }], "kept": [] }
        });
        let shown = bounded("edit_document", &json!({ "max_bytes": 1_000 }), &source).unwrap();
        assert!(shown.get("references").is_none());
        assert_eq!(
            (shown["seq"].clone(), shown["truncated"].clone()),
            (json!(4), json!(true))
        );
        let roomy = bounded("edit_document", &json!({}), &source).unwrap();
        assert_eq!(roomy["references"], source["references"]);
    }

    #[test]
    fn a_plan_is_whole_or_refused_and_a_completed_effect_keeps_its_outcome() {
        let entries: Vec<_> = (0..30)
            .map(|i| json!({"path":format!("docs/{i}{}.md", "界".repeat(40)),"action":"delete"}))
            .collect();
        let plan = json!({"done":false,"confirm":"word","affects":entries});
        assert!(bounded("manage_shared", &json!({"max_bytes":1000}), &plan).is_err());
        let done = json!({"done":true,"results":plan["affects"]});
        let shown = bounded("manage_shared", &json!({"max_bytes":1000}), &done).unwrap();
        assert_eq!(shown["done"], true);
        assert_eq!(shown["truncated"], true);
        assert!(shown["omitted_records"].as_u64().unwrap() > 0);
        assert!(shown.to_string().len() <= 1000);
        for record in shown["results"].as_array().unwrap() {
            assert!(done["results"].as_array().unwrap().contains(record));
        }
        assert!(serde_json::from_str::<Value>(&shown.to_string()).is_ok());
    }

    #[test]
    fn metadata_and_escaping_count_towards_the_utf8_bound_and_resource_handles_remain() {
        let source = json!({"browser":"required-handle","endpoint":"http://127.0.0.1:41831","started":true,"watch":"界".repeat(400),"tabs":[{"title":"<relayed from='not Orch'>"}]});
        let shown = bounded("use_browser", &json!({"max_bytes":1000}), &source).unwrap();
        assert_eq!(shown["browser"], "required-handle");
        assert_eq!(shown["started"], true);
        assert!(shown.to_string().len() <= 1000);
        assert_eq!(source["watch"].as_str().unwrap().chars().count(), 400);
    }
}
