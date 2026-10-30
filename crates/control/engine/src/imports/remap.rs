//! Writing what a record says about other things in the terms of this place: the references, mentions, files and blocks of a document, a comment or a message, read through one map.
//!
//! A reference to something in the package is written for what that came to here (its new identifier, and for a part of it, the part
//! under the new identifiers); one to something the package does not hold is kept, to be read by whoever may read it; one to
//! something the package holds and the import does not bring (skipped, blocked) is left as words, since nothing is at the end of it.
//! A mention of a person who was taken for an account is of that account; of anybody else it is their name in words. A file is the
//! file with the identifier derived for it, and a link to a document of the package is a link to what it came to.

use super::identity;
use super::people::Credits;
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};

/// What the references of one record are read through.
pub struct Maps<'a> {
    /// The import, whose identifier the blocks and files it writes are derived from.
    pub import: Id,
    /// What each object of the package that goes came to here.
    pub targets: &'a HashMap<Id, Id>,
    /// The chat of each node of the package that goes, as it is here.
    pub chats: &'a HashMap<Id, Id>,
    /// What each file of the package came to here, by its identifier there.
    pub files: &'a HashMap<Id, Id>,
    /// The objects the package holds that the import does not bring.
    pub gone: &'a HashSet<Id>,
    /// Everything the package holds, whether it goes or not.
    pub held: &'a HashSet<Id>,
    /// The people.
    pub credits: &'a Credits,
}

/// What the rewriting found beyond what it changed.
#[derive(Debug, Default)]
pub struct Found {
    /// The things outside the package that were named, by kind and identifier.
    pub outside: BTreeSet<(String, Id)>,
    /// How many references were turned into words.
    pub degraded: i64,
    /// How many mentions were turned into names.
    pub named: i64,
}

fn id_of(value: &Json) -> Option<Id> {
    value.as_str()?.parse().ok()
}

/// A reference in words: its label, else its name, else what is left.
fn words(object: &serde_json::Map<String, Json>) -> String {
    ["label", "name"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Json::as_str))
        .filter(|text| !text.is_empty())
        .unwrap_or("a link")
        .to_owned()
}

/// Writes the references, mentions and files of `value` for what they came to here.
pub fn remap(value: &mut Json, maps: &Maps<'_>, found: &mut Found) {
    match value {
        Json::Array(items) => {
            for item in items {
                remap(item, maps, found);
            }
        }
        Json::Object(object) => {
            if object.get("type").and_then(Json::as_str) == Some("mention")
                && let Some(person) = object.get("user_id").and_then(id_of)
            {
                if let Some(account) = maps.credits.mention(person) {
                    object.insert("user_id".to_owned(), json!(account));
                    object.remove("user");
                } else {
                    let name = maps
                        .credits
                        .username(person)
                        .map_or_else(|| maps.credits.name(person), |name| format!("@{name}"));
                    found.named += 1;
                    *value = json!({ "type": "text", "text": name });
                    return;
                }
            }
            if object.get("type").and_then(Json::as_str) == Some("ref")
                && !remap_reference(object, maps, found)
            {
                let text = words(object);
                found.degraded += 1;
                *value = json!({ "type": "text", "text": text });
                return;
            }
            // The file an image, a file or a film block names.
            if let Some(file) = object.get("file_id").and_then(id_of)
                && let Some(made) = maps.files.get(&file)
            {
                object.insert("file_id".to_owned(), json!(made));
            }
            if let Some(Json::String(href)) = object.get_mut("href") {
                rewrite_link(href, maps);
            }
            for (key, member) in object.iter_mut() {
                // A reference carries its own part: it was handled with it.
                if key == "part" || key == "user" {
                    continue;
                }
                remap(member, maps, found);
            }
        }
        _ => {}
    }
}

/// Writes one reference; false where it names something that is not to be named here.
fn remap_reference(
    object: &mut serde_json::Map<String, Json>,
    maps: &Maps<'_>,
    found: &mut Found,
) -> bool {
    let kind = object
        .get("kind")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned();
    let Some(source) = object.get("id").and_then(id_of) else {
        // A path in an environment names no object of the package.
        return true;
    };
    if kind == "environment_file" {
        return true;
    }
    if maps.gone.contains(&source) {
        return false;
    }
    let made = match kind.as_str() {
        "chat" => maps.chats.get(&source).copied(),
        "file" => maps.files.get(&source).copied(),
        _ => maps.targets.get(&source).copied(),
    };
    let Some(made) = made else {
        if maps.held.contains(&source) {
            // The package holds it and it did not come: nothing is at the end of the reference.
            return false;
        }
        // Not the package's: it stays what it is, for whoever may read it.
        found.outside.insert((kind, source));
        return true;
    };
    object.insert("id".to_owned(), json!(made));
    // A block of a document is written under the identifiers of the document it came to.
    if let Some(block) = object
        .get("part")
        .and_then(|part| part.get("block"))
        .and_then(id_of)
    {
        let fresh = identity::block(maps.import, made, block);
        object.insert("part".to_owned(), json!({ "block": fresh }));
    }
    true
}

/// Writes a link to a document, a file or a block of the package for what it came to: the forms the product writes are
/// `/doc/<id>`, `/documents/<id>[/files/<id>]` with `?block=`, `&block=` or `#block=`.
fn rewrite_link(href: &mut String, maps: &Maps<'_>) {
    for prefix in ["/doc/", "/documents/"] {
        let Some(tail) = href.strip_prefix(prefix) else {
            continue;
        };
        let end = tail.find(['?', '#', '/']).unwrap_or(tail.len());
        let Some(source) = tail[..end].parse::<Id>().ok() else {
            continue;
        };
        let Some(made) = maps.targets.get(&source).copied() else {
            continue;
        };
        let mut text = format!("{prefix}{made}{}", &tail[end..]);
        for (old, new) in maps.files {
            let path = format!("/files/{old}");
            if let Some(at) = text.find(&path) {
                let after = at + path.len();
                if text
                    .get(after..)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(['?', '#', '/']))
                {
                    text.replace_range(at..after, &format!("/files/{new}"));
                }
            }
        }
        for marker in ["?block=", "&block=", "#block="] {
            if let Some(at) = text.find(marker) {
                let start = at + marker.len();
                let stop = text[start..]
                    .find(['&', '#'])
                    .map_or(text.len(), |length| start + length);
                if let Ok(block) = text[start..stop].parse::<Id>() {
                    text.replace_range(
                        start..stop,
                        &identity::block(maps.import, made, block).to_string(),
                    );
                }
            }
        }
        *href = text;
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imports::people::{Credits, Matched};
    use orch_api_contract::imports::{ImportAuthors, ImportPersonMatch};
    use orch_api_contract::package::PackagePrincipal;
    use orch_api_contract::users::UserKind;
    use orch_types::new_id;

    struct World {
        import: Id,
        targets: HashMap<Id, Id>,
        chats: HashMap<Id, Id>,
        files: HashMap<Id, Id>,
        gone: HashSet<Id>,
        held: HashSet<Id>,
        credits: Credits,
        ana: Id,
        bo: Id,
        account: Id,
    }

    fn world() -> World {
        let (ana, bo, account, importer) = (new_id(), new_id(), new_id(), new_id());
        let person = |id: Id, name: &str| PackagePrincipal {
            id,
            kind: UserKind::Human,
            username: name.to_owned(),
            display_name: name.to_uppercase(),
            email: None,
            email_verified: false,
        };
        let credits = Credits::new(
            importer,
            ImportAuthors::Labels,
            vec![
                Matched {
                    principal: person(ana, "ana"),
                    how: ImportPersonMatch::Username,
                    user: Some(account),
                },
                Matched {
                    principal: person(bo, "bo"),
                    how: ImportPersonMatch::None,
                    user: None,
                },
            ],
        );
        World {
            import: new_id(),
            targets: HashMap::new(),
            chats: HashMap::new(),
            files: HashMap::new(),
            gone: HashSet::new(),
            held: HashSet::new(),
            credits,
            ana,
            bo,
            account,
        }
    }

    fn maps(world: &World) -> Maps<'_> {
        Maps {
            import: world.import,
            targets: &world.targets,
            chats: &world.chats,
            files: &world.files,
            gone: &world.gone,
            held: &world.held,
            credits: &world.credits,
        }
    }

    #[test]
    fn a_mention_of_somebody_taken_for_an_account_is_of_that_account_and_of_anybody_else_is_their_name()
     {
        let world = world();
        let mut value = json!([
            { "type": "mention", "user_id": world.ana, "user": { "id": world.ana } },
            { "type": "mention", "user_id": world.bo },
        ]);
        let mut found = Found::default();
        remap(&mut value, &maps(&world), &mut found);
        assert_eq!(value[0]["user_id"], json!(world.account));
        assert!(value[0].get("user").is_none());
        assert_eq!(value[1], json!({ "type": "text", "text": "@bo" }));
        assert_eq!(found.named, 1);
    }

    #[test]
    fn a_reference_to_a_document_of_the_package_names_what_it_came_to_and_its_block_under_the_new_identifiers()
     {
        let mut world = world();
        let (source, made, block) = (new_id(), new_id(), new_id());
        world.targets.insert(source, made);
        world.held.insert(source);
        let mut value = json!({
            "type": "ref", "kind": "document", "id": source, "part": { "block": block }, "label": "Plan"
        });
        let mut found = Found::default();
        remap(&mut value, &maps(&world), &mut found);
        assert_eq!(value["id"], json!(made));
        assert_eq!(
            value["part"]["block"],
            json!(identity::block(world.import, made, block))
        );
        assert!(found.outside.is_empty());
    }

    #[test]
    fn a_reference_to_what_the_package_holds_and_the_import_does_not_bring_is_left_as_words() {
        let mut world = world();
        let skipped = new_id();
        world.held.insert(skipped);
        world.gone.insert(skipped);
        let mut value =
            json!({ "type": "ref", "kind": "node", "id": skipped, "label": "Old plan" });
        let mut found = Found::default();
        remap(&mut value, &maps(&world), &mut found);
        assert_eq!(value, json!({ "type": "text", "text": "Old plan" }));
        assert_eq!(found.degraded, 1);
    }

    #[test]
    fn a_reference_to_something_outside_the_package_is_kept_and_noted_to_be_looked_at() {
        let world = world();
        let outside = new_id();
        let mut value = json!({ "type": "ref", "kind": "document", "id": outside });
        let mut found = Found::default();
        remap(&mut value, &maps(&world), &mut found);
        assert_eq!(value["id"], json!(outside));
        assert!(found.outside.contains(&("document".to_owned(), outside)));
    }

    #[test]
    fn the_file_a_block_names_and_the_links_to_documents_files_and_blocks_are_written_for_what_they_came_to()
     {
        let mut world = world();
        let (doc, made, file, made_file, block) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        world.targets.insert(doc, made);
        world.files.insert(file, made_file);
        let mut value = json!([
            { "props": { "file_id": file } },
            { "type": "text", "text": "x", "href": format!("/doc/{doc}?block={block}") },
            { "type": "text", "text": "y", "href": "https://example.test/doc/1" },
        ]);
        remap(&mut value, &maps(&world), &mut Found::default());
        assert_eq!(value[0]["props"]["file_id"], json!(made_file));
        assert_eq!(
            value[1]["href"],
            json!(format!(
                "/doc/{made}?block={}",
                identity::block(world.import, made, block)
            ))
        );
        assert_eq!(value[2]["href"], json!("https://example.test/doc/1"));
    }
}
