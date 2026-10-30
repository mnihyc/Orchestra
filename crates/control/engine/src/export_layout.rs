//! How an archive is laid out: a directory for each place, by its name, and in it `conversations/<title>` for each thread, `chat` and `docs/<title>`, with the files and the versions of a thing beside it; every name its own among its neighbours, and a place that its reader may not see, named by nothing.
//!
//! The layout is worked out from the places and the things that are to be written alone, so that the plan of an
//! export can say how many places an archive would have and the job can say where each thing goes. A directory is
//! made for a place that holds something that is written, and for every place on the way to it from the place
//! the archive is of. A place its reader may not see comes without its name: its directory is called `hidden`, and
//! where several are, each is told from the others by the start of its identifier, as every name that two
//! neighbours would share is.

use orch_types::Id;
use std::collections::{HashMap, HashSet};

/// The most characters of a name that go into a path: what a title is cut to, so that a path never passes what a
/// file system takes of one name.
pub const NAME_MAX_CHARS: usize = 100;

/// How many characters of an identifier tell two names apart, at the least.
pub const SUFFIX_CHARS: usize = 4;

/// What a place without a name is called: its reader may not see it.
pub const HIDDEN: &str = "hidden";

/// The name of the file that lists everything in the archive, at its root.
pub const INDEX: &str = "index.md";

/// The name of the file that says who made the archive, of what, and what it holds, at its root.
pub const MANIFEST: &str = "export.json";

/// The directory of the conversations of a place's threads, in the place's directory.
pub const CONVERSATIONS: &str = "conversations";

/// The directory of the documents written beside a place, in the place's directory.
pub const DOCS: &str = "docs";

/// The names a directory of a place keeps for its own: nothing of the same name lies beside them.
const RESERVED_IN_A_PLACE: [&str; 5] = [CONVERSATIONS, DOCS, "chat.md", "chat.json", "chat.files"];

/// The name of the manifest of an Orch package, at its root.
pub const PACKAGE_MANIFEST: &str = orch_api_contract::package::PACKAGE_MANIFEST;

/// The names the root of the archive keeps for its own.
const RESERVED_AT_THE_ROOT: [&str; 3] = [INDEX, MANIFEST, PACKAGE_MANIFEST];

/// A node of the tree the archive is cut from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Its identifier.
    pub id: Id,
    /// The node it lies in.
    pub parent: Option<Id>,
    /// Its name; none where the reader may not see it.
    pub name: Option<String>,
}

/// A thing that is to be written, and the node it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanting {
    /// The conversations of threads: the thread, the node whose directory holds the file (the topic the thread
    /// lies in), and the thread's name.
    pub conversations: Vec<(Id, Id, String)>,
    /// The chats of nodes: the chat and its node.
    pub chats: Vec<(Id, Id)>,
    /// The documents: the document, the node it is written beside, and its title.
    pub documents: Vec<(Id, Id, String)>,
}

/// Where a thing goes: its file, and the path its files and its versions go beside, without an extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// The path of its file in the archive.
    pub path: String,
    /// The path without the extension: `<stem>.files/` and `<stem>.history/` lie beside the file.
    pub stem: String,
}

/// Where everything goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    /// The directory of each place that has one.
    pub dirs: HashMap<Id, String>,
    /// Each conversation, by its thread.
    pub conversations: HashMap<Id, Placed>,
    /// Each chat.
    pub chats: HashMap<Id, Placed>,
    /// Each document.
    pub documents: HashMap<Id, Placed>,
}

impl Layout {
    /// How many places have a directory.
    #[must_use]
    pub fn places(&self) -> i64 {
        i64::try_from(self.dirs.len()).unwrap_or(i64::MAX)
    }
}

/// A name that is safe in a path: no slash, no back slash, no control character, not empty, not `.` or `..`, no
/// longer than [`NAME_MAX_CHARS`], and ending in neither a dot nor a space.
#[must_use]
pub fn clean(name: &str) -> String {
    let replaced: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '/' || c == '\\' { '-' } else { c })
        .take(NAME_MAX_CHARS)
        .collect();
    let trimmed = replaced.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        "_".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The names of entries of one directory, in the order they are given: each its own, and where an entry before it
/// (or the layout, or a name that a case-blind file system would take for another) has the name already, `~` and
/// as many characters of its identifier as it takes to differ from all before it, at least [`SUFFIX_CHARS`].
/// `extension` goes after the name, and after the suffix.
#[must_use]
pub fn assign(entries: &[(Id, String)], extension: &str, reserved: &[&str]) -> Vec<String> {
    let mut taken: HashSet<String> = reserved.iter().map(|word| word.to_lowercase()).collect();
    let mut out = Vec::with_capacity(entries.len());
    for (id, name) in entries {
        let bare = clean(name);
        let mut chosen = format!("{bare}{extension}");
        if taken.contains(&chosen.to_lowercase()) {
            let digits = id.simple().to_string();
            chosen = (SUFFIX_CHARS..=digits.len())
                .map(|length| format!("{bare}~{}{extension}", &digits[..length]))
                .find(|candidate| !taken.contains(&candidate.to_lowercase()))
                .unwrap_or_else(|| format!("{bare}~{digits}{extension}"));
        }
        taken.insert(chosen.to_lowercase());
        out.push(chosen);
    }
    out
}

fn shown(node: &Node) -> String {
    node.name.clone().unwrap_or_else(|| HIDDEN.to_owned())
}

/// Lays out what `wanting` holds, for an archive of the place `root`, whose files end in `extension` (`.md` or
/// `.json`). Nodes are given in the order they were made; a thing whose node does not lie in the tree below `root`
/// is not placed.
#[must_use]
pub fn lay_out(root: Id, nodes: &[Node], wanting: &Wanting, extension: &str) -> Layout {
    let by_id: HashMap<Id, &Node> = nodes.iter().map(|node| (node.id, node)).collect();
    let order: HashMap<Id, usize> = nodes
        .iter()
        .enumerate()
        .map(|(at, node)| (node.id, at))
        .collect();
    let mut used: HashSet<Id> = HashSet::new();
    let mut note = |home: Id| {
        let mut at = Some(home);
        let mut way: Vec<Id> = Vec::new();
        while let Some(node) = at {
            if !by_id.contains_key(&node) {
                return false;
            }
            way.push(node);
            if node == root {
                used.extend(way);
                return true;
            }
            at = by_id.get(&node).and_then(|each| each.parent);
        }
        false
    };
    let homes: Vec<Id> = wanting
        .conversations
        .iter()
        .map(|each| each.1)
        .chain(wanting.chats.iter().map(|each| each.1))
        .chain(wanting.documents.iter().map(|each| each.1))
        .collect();
    let reachable: HashSet<Id> = homes.into_iter().filter(|home| note(*home)).collect();
    let mut children: HashMap<Id, Vec<Id>> = HashMap::new();
    for node in nodes {
        if node.id != root
            && used.contains(&node.id)
            && let Some(parent) = node.parent
        {
            children.entry(parent).or_default().push(node.id);
        }
    }
    let mut layout = Layout::default();
    let Some(top) = by_id.get(&root) else {
        return layout;
    };
    let top_name = assign(&[(root, shown(top))], "", &RESERVED_AT_THE_ROOT).remove(0);
    layout.dirs.insert(root, top_name);
    let mut queue: Vec<Id> = vec![root];
    while let Some(parent) = queue.pop() {
        let Some(kids) = children.get(&parent) else {
            continue;
        };
        let mut kids = kids.clone();
        kids.sort_by_key(|kid| order.get(kid).copied().unwrap_or(usize::MAX));
        let entries: Vec<(Id, String)> = kids
            .iter()
            .filter_map(|kid| by_id.get(kid).map(|node| (*kid, shown(node))))
            .collect();
        let names = assign(&entries, "", &RESERVED_IN_A_PLACE);
        let above = layout.dirs.get(&parent).cloned().unwrap_or_default();
        for ((kid, _), name) in entries.iter().zip(names) {
            layout.dirs.insert(*kid, format!("{above}/{name}"));
            queue.push(*kid);
        }
    }
    // The conversations, the chats and the documents of each place.
    let mut conversations: HashMap<Id, Vec<(Id, String)>> = HashMap::new();
    for (thread, home, title) in &wanting.conversations {
        if reachable.contains(home) {
            conversations
                .entry(*home)
                .or_default()
                .push((*thread, title.clone()));
        }
    }
    for (home, entries) in conversations {
        let Some(dir) = layout.dirs.get(&home).cloned() else {
            continue;
        };
        let names = assign(&entries, extension, &[]);
        for ((thread, _), name) in entries.iter().zip(names) {
            let path = format!("{dir}/{CONVERSATIONS}/{name}");
            let stem = path.strip_suffix(extension).unwrap_or(&path).to_owned();
            layout.conversations.insert(*thread, Placed { path, stem });
        }
    }
    for (chat, home) in &wanting.chats {
        if let Some(dir) = layout.dirs.get(home) {
            layout.chats.insert(
                *chat,
                Placed {
                    path: format!("{dir}/chat{extension}"),
                    stem: format!("{dir}/chat"),
                },
            );
        }
    }
    let mut documents: HashMap<Id, Vec<(Id, String)>> = HashMap::new();
    for (document, home, title) in &wanting.documents {
        if reachable.contains(home) {
            documents
                .entry(*home)
                .or_default()
                .push((*document, title.clone()));
        }
    }
    for (home, entries) in documents {
        let Some(dir) = layout.dirs.get(&home).cloned() else {
            continue;
        };
        let names = assign(&entries, extension, &[]);
        for ((document, _), name) in entries.iter().zip(names) {
            let path = format!("{dir}/{DOCS}/{name}");
            let stem = path.strip_suffix(extension).unwrap_or(&path).to_owned();
            layout.documents.insert(*document, Placed { path, stem });
        }
    }
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn node(id: Id, parent: Option<Id>, name: &str) -> Node {
        Node {
            id,
            parent,
            name: Some(name.to_owned()),
        }
    }

    #[test]
    fn a_name_is_its_own_among_its_neighbours_and_tells_its_twin_by_the_start_of_an_identifier() {
        let (first, second, third) = (new_id(), new_id(), new_id());
        let names = assign(
            &[
                (first, "Plan".to_owned()),
                (second, "plan".to_owned()),
                (third, "Notes".to_owned()),
            ],
            ".md",
            &[],
        );
        assert_eq!(names[0], "Plan.md", "the oldest keeps the name");
        assert_eq!(
            names[1],
            format!("plan~{}.md", &second.simple().to_string()[..SUFFIX_CHARS]),
            "a name a case-blind file system would take for another is told apart"
        );
        assert_eq!(names[2], "Notes.md");
    }

    #[test]
    fn a_name_the_layout_keeps_for_itself_gets_a_suffix_and_no_name_can_go_up() {
        let id = new_id();
        let kept = assign(&[(id, "docs".to_owned())], "", &RESERVED_IN_A_PLACE);
        assert!(kept[0].starts_with("docs~"));
        for name in ["..", ".", "", "a/b", "a\\b", "x\u{0}y"] {
            let cleaned = clean(name);
            assert!(!cleaned.contains('/') && !cleaned.contains('\\') && cleaned != "..");
            assert!(
                !cleaned.is_empty() && cleaned != "." && !cleaned.chars().any(char::is_control)
            );
        }
        assert_eq!(clean("Q3 plan. "), "Q3 plan");
        assert_eq!(clean(&"x".repeat(500)).chars().count(), NAME_MAX_CHARS);
    }

    #[test]
    fn a_place_has_a_directory_when_it_holds_something_or_lies_on_the_way_to_it() {
        let (org, project, empty, topic, thread) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        let nodes = vec![
            node(org, None, "Acme"),
            node(project, Some(org), "Web"),
            node(empty, Some(org), "Nothing"),
            node(topic, Some(project), "Fix login"),
            node(thread, Some(topic), "Repro"),
        ];
        let (chat, document) = (new_id(), new_id());
        let wanting = Wanting {
            conversations: vec![(thread, topic, "Repro".to_owned())],
            chats: vec![(chat, project)],
            documents: vec![(document, topic, "Rollout plan".to_owned())],
        };
        let layout = lay_out(org, &nodes, &wanting, ".md");
        assert_eq!(layout.dirs[&org], "Acme");
        assert_eq!(layout.dirs[&project], "Acme/Web");
        assert_eq!(layout.dirs[&topic], "Acme/Web/Fix login");
        assert!(
            !layout.dirs.contains_key(&empty),
            "a place with nothing in it has no directory"
        );
        assert!(
            !layout.dirs.contains_key(&thread),
            "a thread is a file of its topic"
        );
        assert_eq!(layout.places(), 3);
        assert_eq!(
            layout.conversations[&thread].path,
            "Acme/Web/Fix login/conversations/Repro.md"
        );
        assert_eq!(
            layout.conversations[&thread].stem,
            "Acme/Web/Fix login/conversations/Repro"
        );
        assert_eq!(layout.chats[&chat].path, "Acme/Web/chat.md");
        assert_eq!(layout.chats[&chat].stem, "Acme/Web/chat");
        assert_eq!(
            layout.documents[&document].path,
            "Acme/Web/Fix login/docs/Rollout plan.md"
        );
        let records = lay_out(org, &nodes, &wanting, ".json");
        assert_eq!(records.chats[&chat].path, "Acme/Web/chat.json");
        assert_eq!(
            records.documents[&document].stem,
            layout.documents[&document].stem
        );
    }

    #[test]
    fn a_place_its_reader_may_not_see_is_named_by_nothing_and_told_from_its_twins() {
        let (org, first, second, topic) = (new_id(), new_id(), new_id(), new_id());
        let hidden = |id, parent| Node {
            id,
            parent,
            name: None,
        };
        let nodes = vec![
            node(org, None, "Acme"),
            hidden(first, Some(org)),
            hidden(second, Some(org)),
            node(topic, Some(first), "Secret plans"),
        ];
        let (one, other) = (new_id(), new_id());
        let wanting = Wanting {
            conversations: vec![],
            chats: vec![(new_id(), first), (new_id(), second)],
            documents: vec![
                (one, topic, "Plan".to_owned()),
                (other, second, "Plan".to_owned()),
            ],
        };
        let layout = lay_out(org, &nodes, &wanting, ".md");
        assert_eq!(layout.dirs[&first], "Acme/hidden");
        assert!(
            layout.dirs[&second].starts_with("Acme/hidden~"),
            "{}",
            layout.dirs[&second]
        );
        assert!(
            layout.documents[&other].path.contains("hidden"),
            "{}",
            layout.documents[&other].path
        );
        for placed in layout.documents.values() {
            assert!(!placed.path.contains("Secret") || placed.path.contains("Acme/hidden/"));
        }
    }

    #[test]
    fn what_lies_outside_the_tree_is_not_placed_and_a_thread_asked_alone_is_its_own_directory() {
        let (org, topic, thread, stranger) = (new_id(), new_id(), new_id(), new_id());
        let nodes = vec![
            node(org, None, "Acme"),
            node(topic, Some(org), "Fix login"),
            node(thread, Some(topic), "Repro"),
        ];
        let wanting = Wanting {
            conversations: vec![(thread, thread, "Repro".to_owned())],
            chats: vec![(new_id(), stranger)],
            documents: vec![(new_id(), stranger, "Lost".to_owned())],
        };
        let layout = lay_out(thread, &nodes, &wanting, ".md");
        assert_eq!(
            layout.conversations[&thread].path,
            "Repro/conversations/Repro.md"
        );
        assert!(layout.chats.is_empty() && layout.documents.is_empty());
    }
}
