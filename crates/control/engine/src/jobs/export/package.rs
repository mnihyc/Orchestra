//! The manifest of an Orch package: every thing an archive holds as an object with its kind, identifier, parent, record, name, times, maker and hash, the places and document folders that lie between them, the people the records name, and what was left out.
//!
//! The things are written by the export job as it goes; what it remembers of each (its path, its reading copy, its files,
//! its versions) is an [`Entry`]. When the last thing is written, [`Exporting::manifest`] reads what the records do not
//! hold (who made each thing and when it last changed, where a document lies, what a place is called and about) and
//! puts it all in the manifest, with the SHA-256 that the zip kept of every entry it was given.

use super::{Archive, Exporting, Tally};
use crate::export_layout::Layout;
use crate::exports::{Selected, node_kind};
use crate::jobs::runner::{Failure, INTERNAL, Stop};
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::Permission;
use orch_api_contract::exports::{ExportCounts, ExportLeftOut, ExportRequest};
use orch_api_contract::package::{
    PACKAGE_FORMAT, PACKAGE_VERSION, PackageAccess, PackageFile, PackageHeader, PackageKind,
    PackageLeftOut, PackageLeftOutWhat, PackageManifest, PackageObject, PackagePrincipal,
    PackageRendering, PackageScope, PackageSource, PackageVersion,
};
use orch_api_contract::users::UserKind;
use orch_authz::Policy;
use orch_db::queries::package as meta;
use orch_db::queries::shares::PlaceRow;
use orch_types::{Id, Json, Time};
use std::collections::{BTreeSet, HashMap, HashSet};

/// A file a thing carries, as the archive holds it.
#[derive(Debug, Clone)]
pub(super) struct Carried {
    /// The identifier it had, where it had one a block can name it by.
    pub id: Option<Id>,
    /// Its name in the thing that carries it.
    pub name: String,
    /// Where it lies in the archive.
    pub path: String,
}

/// An earlier version of a document, as the archive holds it.
#[derive(Debug, Clone)]
pub(super) struct Cut {
    /// Its number.
    pub number: i64,
    /// When it was cut.
    pub cut_at: DateTime<Utc>,
    /// Where its record lies in the archive.
    pub path: String,
}

/// What the export job remembers of a thing it wrote, for the manifest.
#[derive(Debug, Clone)]
pub(super) struct Entry {
    /// What it is.
    pub kind: PackageKind,
    /// Its identifier.
    pub id: Id,
    /// The node it lies beside or belongs to.
    pub parent: Id,
    /// Where its record lies.
    pub path: Option<String>,
    /// Where its reading copy lies.
    pub reading: Option<String>,
    /// Where a document's HTML reading copy lies.
    pub html: Option<String>,
    /// Its name or title.
    pub name: String,
    /// How many turns a thread has.
    pub turns: Option<i64>,
    /// How many messages a chat has.
    pub messages: Option<i64>,
    /// The files it carries.
    pub files: Vec<Carried>,
    /// The earlier versions of a document.
    pub versions: Vec<Cut>,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            kind: PackageKind::Document,
            id: Id::nil(),
            parent: Id::nil(),
            path: None,
            reading: None,
            html: None,
            name: String::new(),
            turns: None,
            messages: None,
            files: Vec::new(),
            versions: Vec::new(),
        }
    }
}

/// Collects the people a record names: a reference to a person has their identifier beside the names they are shown
/// by, a mention in text names them by `user_id`, and the people who reacted to a message or a comment are a list of
/// identifiers under `users`.
pub(super) fn people_in(value: &Json, into: &mut BTreeSet<Id>) {
    match value {
        Json::Array(items) => {
            for item in items {
                people_in(item, into);
            }
        }
        Json::Object(map) => {
            if map.contains_key("username")
                && map.contains_key("display_name")
                && let Some(id) = map.get("id").and_then(identifier)
            {
                into.insert(id);
            }
            for (key, member) in map {
                match key.as_str() {
                    "user_id" => {
                        if let Some(id) = identifier(member) {
                            into.insert(id);
                        }
                    }
                    "users" => {
                        for each in member.as_array().into_iter().flatten() {
                            if let Some(id) = identifier(each) {
                                into.insert(id);
                            }
                        }
                    }
                    _ => {}
                }
                people_in(member, into);
            }
        }
        _ => {}
    }
}

fn identifier(value: &Json) -> Option<Id> {
    value.as_str()?.parse().ok()
}

fn time(at: DateTime<Utc>) -> Time {
    Time::new(at)
}

/// What no export carries, said in every package whatever its things are: the plan and the report of an import
/// name these as what does not come back.
const NEVER_CARRIED: [PackageLeftOutWhat; 6] = [
    PackageLeftOutWhat::NativeSessions,
    PackageLeftOutWhat::Workspaces,
    PackageLeftOutWhat::Secrets,
    PackageLeftOutWhat::UsageAndAudit,
    PackageLeftOutWhat::PersonalState,
    PackageLeftOutWhat::WebImages,
];

impl Exporting {
    /// The manifest of the package whose things have been written.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "what was written, what the places are, the options, the counts and the time: one reading, in the order the manifest is made"
    )]
    pub(super) async fn manifest(
        &self,
        selected: &Selected,
        request: &ExportRequest,
        layout: &Layout,
        archive: &Archive,
        tally: &Tally,
        counts: ExportCounts,
        left_out: &[ExportLeftOut],
        now: DateTime<Utc>,
    ) -> Result<PackageManifest, Stop> {
        let mut conn = self.engine.inner.db.acquire().await?;
        let principal = self.principal(&mut conn).await?;
        let digest = |path: &str| archive.digest_of(path).unwrap_or_default();
        let by_id: HashMap<Id, &PlaceRow> =
            selected.scope.iter().map(|node| (node.id, node)).collect();
        let root = selected.place.id;

        // The places: those with a directory, those that were asked for and may be named, and the way from each of
        // them and from each thing that was written to the place the package is of.
        let mut nodes: BTreeSet<Id> = BTreeSet::new();
        nodes.insert(root);
        nodes.extend(layout.dirs.keys().copied());
        for node in &selected.scope {
            if node.kind != "thread"
                && selected.included.contains(&node.id)
                && selected.viewable.contains_key(&node.id)
            {
                nodes.insert(node.id);
            }
        }
        let written_threads: HashSet<Id> = tally
            .entries
            .iter()
            .filter(|entry| entry.kind == PackageKind::Thread)
            .map(|entry| entry.id)
            .collect();
        for entry in &tally.entries {
            // A chat or a document beside a thread whose conversation is not in the package keeps the thread as a
            // place with no record.
            if by_id.contains_key(&entry.parent) {
                nodes.insert(entry.parent);
            }
        }
        nodes.extend(written_threads.iter().copied());
        let mut needed: Vec<Id> = nodes.iter().copied().collect();
        while let Some(id) = needed.pop() {
            let mut at = by_id.get(&id).and_then(|node| node.parent_id);
            while let Some(parent) = at {
                if !by_id.contains_key(&parent) || !nodes.insert(parent) {
                    break;
                }
                at = by_id.get(&parent).and_then(|node| node.parent_id);
            }
        }
        let node_ids: Vec<Id> = nodes.iter().copied().collect();
        let node_meta: HashMap<Id, meta::NodeMeta> = meta::node_meta(&mut conn, &node_ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row))
            .collect();

        // What the written things say of themselves beyond their records.
        let document_ids: Vec<Id> = tally
            .entries
            .iter()
            .filter(|entry| entry.kind == PackageKind::Document)
            .map(|entry| entry.id)
            .collect();
        let chat_ids: Vec<Id> = tally
            .entries
            .iter()
            .filter(|entry| entry.kind == PackageKind::Chat)
            .map(|entry| entry.id)
            .collect();
        let documents: HashMap<Id, meta::DocumentMeta> =
            meta::document_meta(&mut conn, &document_ids)
                .await?
                .into_iter()
                .map(|row| (row.id, row))
                .collect();
        let chats: HashMap<Id, meta::ChatMeta> = meta::chat_meta(&mut conn, &chat_ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row))
            .collect();
        let container_ids: Vec<Id> = documents
            .values()
            .filter_map(|document| document.folder_id)
            .collect();
        let containers = meta::folders_with_ancestors(&mut conn, &container_ids).await?;
        let readable: HashSet<Id> = {
            use crate::document_tree::{Actor, Item};
            let items: Vec<Item> = containers
                .iter()
                .map(|container| Item::Container(container.id))
                .collect();
            if items.is_empty() {
                HashSet::new()
            } else {
                crate::document_tree::load_entries_in(
                    &mut conn,
                    Actor::Person(&principal),
                    &[],
                    &items,
                    Time::new(now),
                )
                .await
                .map_err(|error| Failure::new(INTERNAL, error.to_string()))?
                .entries
                .into_iter()
                .filter_map(|listed| match listed.entry.item {
                    Item::Container(id) => Some(id),
                    Item::Document(_) => None,
                })
                .collect()
            }
        };

        // The objects, a place before what lies in it.
        let mut people: BTreeSet<Id> = archive.people.clone().unwrap_or_default();
        people.insert(self.user);
        let mut objects: Vec<PackageObject> = Vec::new();
        let mut places: Vec<&PlaceRow> = nodes
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .filter(|node| node.kind != "thread")
            .collect();
        places.sort_by_key(|node| (node.path.len(), node.created_at, node.id));
        for node in places {
            let row = node_meta.get(&node.id);
            let hidden = !selected.viewable.contains_key(&node.id);
            if let Some(row) = row {
                people.insert(row.created_by);
            }
            objects.push(PackageObject {
                kind: PackageKind::of_node(node_kind(&node.kind)),
                id: node.id,
                parent: (node.id != root).then_some(node.parent_id).flatten(),
                path: None,
                name: if hidden {
                    String::new()
                } else {
                    node.name.clone()
                },
                description: if hidden {
                    String::new()
                } else {
                    node.description.clone()
                },
                hidden,
                created_at: Some(time(node.created_at)),
                updated_at: Some(time(node.updated_at)),
                creator: row.map(|row| row.created_by),
                sha256: None,
                bytes: 0,
                messages: None,
                turns: None,
                comments: None,
                renderings: Vec::new(),
                files: Vec::new(),
                versions: Vec::new(),
            });
        }
        // Threads: those whose conversation was written carry it; those that only hold a chat or a document do not.
        let mut threads: Vec<&PlaceRow> = nodes
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .filter(|node| node.kind == "thread")
            .collect();
        threads.sort_by_key(|node| (node.path.len(), node.created_at, node.id));
        for node in threads {
            let entry = tally
                .entries
                .iter()
                .find(|entry| entry.kind == PackageKind::Thread && entry.id == node.id);
            let row = node_meta.get(&node.id);
            let hidden = !selected.viewable.contains_key(&node.id);
            if let Some(row) = row {
                people.insert(row.created_by);
            }
            let (sha256, bytes) = entry
                .and_then(|entry| entry.path.as_deref())
                .map(&digest)
                .map_or((None, 0), |(sha, size)| (Some(sha), size));
            objects.push(PackageObject {
                kind: PackageKind::Thread,
                id: node.id,
                parent: (node.id != root).then_some(node.parent_id).flatten(),
                path: entry.and_then(|entry| entry.path.clone()),
                name: if hidden {
                    String::new()
                } else {
                    node.name.clone()
                },
                description: if hidden {
                    String::new()
                } else {
                    node.description.clone()
                },
                hidden,
                created_at: Some(time(node.created_at)),
                updated_at: Some(time(node.updated_at)),
                creator: row.map(|row| row.created_by),
                sha256,
                bytes,
                messages: None,
                turns: entry.and_then(|entry| entry.turns),
                comments: None,
                renderings: entry
                    .map(|entry| renderings(entry, &digest))
                    .unwrap_or_default(),
                files: entry.map(|entry| files(entry, &digest)).unwrap_or_default(),
                versions: Vec::new(),
            });
        }
        // Document folders, an outer folder before the one in it.
        let depth_of: HashMap<Id, Option<Id>> = containers
            .iter()
            .map(|container| (container.id, container.parent_id))
            .collect();
        let depth = |mut id: Id| {
            let mut depth = 0_usize;
            while let Some(Some(parent)) = depth_of.get(&id) {
                depth += 1;
                id = *parent;
                if depth > depth_of.len() {
                    break;
                }
            }
            depth
        };
        let mut folders: Vec<&meta::FolderMeta> = containers.iter().collect();
        folders.sort_by_key(|folder| (depth(folder.id), folder.created_at, folder.id));
        for folder in folders {
            let named = readable.contains(&folder.id);
            if let Some(maker) = folder.created_by {
                people.insert(maker);
            }
            objects.push(PackageObject {
                kind: PackageKind::Folder,
                id: folder.id,
                parent: Some(folder.parent_id.unwrap_or(folder.node_id)),
                path: None,
                name: if named {
                    folder.name.clone()
                } else {
                    String::new()
                },
                description: String::new(),
                hidden: !named,
                created_at: Some(time(folder.created_at)),
                updated_at: Some(time(folder.updated_at)),
                creator: folder.created_by,
                sha256: None,
                bytes: 0,
                messages: None,
                turns: None,
                comments: None,
                renderings: Vec::new(),
                files: Vec::new(),
                versions: Vec::new(),
            });
        }
        // Chats and documents, as they were written.
        for entry in &tally.entries {
            match entry.kind {
                PackageKind::Chat => {
                    let row = chats.get(&entry.id);
                    if let Some(maker) = row.and_then(|row| row.created_by) {
                        people.insert(maker);
                    }
                    let (sha256, bytes) = entry
                        .path
                        .as_deref()
                        .map(&digest)
                        .map_or((None, 0), |(sha, size)| (Some(sha), size));
                    objects.push(PackageObject {
                        kind: PackageKind::Chat,
                        id: entry.id,
                        parent: Some(entry.parent),
                        path: entry.path.clone(),
                        name: entry.name.clone(),
                        description: String::new(),
                        hidden: false,
                        created_at: row.map(|row| time(row.created_at)),
                        updated_at: row.and_then(|row| row.last_message_at.map(time)),
                        creator: row.and_then(|row| row.created_by),
                        sha256,
                        bytes,
                        messages: row.map(|row| row.messages).or(entry.messages),
                        turns: None,
                        comments: None,
                        renderings: renderings(entry, &digest),
                        files: files(entry, &digest),
                        versions: Vec::new(),
                    });
                }
                PackageKind::Document => {
                    let row = documents.get(&entry.id);
                    if let Some(maker) = row.and_then(|row| row.created_by) {
                        people.insert(maker);
                    }
                    let (sha256, bytes) = entry
                        .path
                        .as_deref()
                        .map(&digest)
                        .map_or((None, 0), |(sha, size)| (Some(sha), size));
                    objects.push(PackageObject {
                        kind: PackageKind::Document,
                        id: entry.id,
                        parent: Some(row.and_then(|row| row.folder_id).unwrap_or(entry.parent)),
                        path: entry.path.clone(),
                        name: entry.name.clone(),
                        description: row.map(|row| row.description.clone()).unwrap_or_default(),
                        hidden: false,
                        created_at: row.map(|row| time(row.created_at)),
                        updated_at: row.map(|row| time(row.updated_at)),
                        creator: row.and_then(|row| row.created_by),
                        sha256,
                        bytes,
                        messages: None,
                        turns: None,
                        comments: row.map(|row| row.comments),
                        renderings: renderings(entry, &digest),
                        files: files(entry, &digest),
                        versions: entry
                            .versions
                            .iter()
                            .map(|cut| {
                                let (sha256, _) = digest(&cut.path);
                                PackageVersion {
                                    number: cut.number,
                                    cut_at: time(cut.cut_at),
                                    path: cut.path.clone(),
                                    sha256,
                                }
                            })
                            .collect(),
                    });
                }
                _ => {}
            }
        }

        // The sharing: carried only by the one who manages the access to the place the package is of, and then for each place and
        // document of which they manage the access, who was named in its page by name and with what.
        let manages = |held: &[orch_access_contract::decisions::Held]| {
            held.iter()
                .any(|each| each.permission == Permission::AccessManage)
        };
        let scope_manages =
            orch_authz::nodes::effective(&mut conn, &principal, root, Policy::default())
                .await
                .map(|effective| manages(&effective.held))
                .unwrap_or(false);
        let access: Option<Vec<PackageAccess>> = if scope_manages {
            let mut shared: Vec<PackageAccess> = Vec::new();
            let mut places: Vec<Id> = Vec::new();
            for object in objects.iter().filter(|object| {
                !object.hidden
                    && matches!(
                        object.kind,
                        PackageKind::Org
                            | PackageKind::Project
                            | PackageKind::Topic
                            | PackageKind::Thread
                    )
            }) {
                let held = orch_authz::nodes::effective(
                    &mut conn,
                    &principal,
                    object.id,
                    Policy::default(),
                )
                .await
                .map(|effective| manages(&effective.held))
                .unwrap_or(false);
                if held {
                    places.push(object.id);
                }
            }
            let documents: Vec<Id> = objects
                .iter()
                .filter(|object| object.kind == PackageKind::Document)
                .map(|object| object.id)
                .collect();
            // What the person holds on each document is worked out a thousand documents at a time, and the
            // process's other work goes on between them: at once, a place of 100,000 documents kept a worker of the
            // runtime busy for minutes, and the server answered nothing else meanwhile.
            let mut managed_documents: Vec<Id> = Vec::new();
            for chunk in documents.chunks(1_000) {
                managed_documents.extend(
                    orch_authz::documents::listed(&mut conn, &principal, chunk, Policy::default())
                        .await
                        .map_err(|error| Failure::new(INTERNAL, error.to_string()))?
                        .iter()
                        .filter(|listed| manages(&listed.held))
                        .map(|listed| listed.document.id),
                );
                tokio::task::yield_now().await;
            }
            for (page, ids) in [("node", &places), ("document", &managed_documents)] {
                if ids.is_empty() {
                    continue;
                }
                for row in meta::shared_with(&mut conn, page, ids, now).await? {
                    people.insert(row.holder_id);
                    shared.push(PackageAccess {
                        object: row.object_id,
                        principal: row.holder_id,
                        permission: row.permission,
                        expires_at: row.expires_at.map(time),
                        note: row.note,
                    });
                }
            }
            Some(shared)
        } else {
            None
        };

        // The people.
        let ids: Vec<Id> = people.iter().copied().collect();
        let principals: Vec<PackagePrincipal> = meta::people_meta(&mut conn, &ids)
            .await?
            .into_iter()
            .map(|person| {
                let mayread = self.emails || person.id == self.user;
                PackagePrincipal {
                    id: person.id,
                    kind: if person.kind == "service" {
                        UserKind::Service
                    } else {
                        UserKind::Human
                    },
                    username: person.username,
                    display_name: person.display_name,
                    email: person.email.filter(|_| mayread),
                    email_verified: mayread && person.email_verified,
                }
            })
            .collect();

        let mut left: Vec<PackageLeftOut> = left_out.iter().map(PackageLeftOut::from).collect();
        left.extend(NEVER_CARRIED.iter().map(|what| PackageLeftOut {
            what: *what,
            count: 0,
            place: None,
        }));
        let deployment = &self.engine.inner.config.deployment;
        Ok(PackageManifest {
            format: PACKAGE_FORMAT.to_owned(),
            version: PACKAGE_VERSION,
            header: PackageHeader {
                package_id: self.export,
                source: PackageSource {
                    name: deployment.name.clone(),
                    url: deployment.url.clone(),
                    version: deployment.version.clone(),
                },
                made_by: Some(self.user),
                made_at: time(now),
                scope: PackageScope {
                    id: root,
                    kind: node_kind(&selected.place.kind),
                    name: selected.place.name.clone(),
                },
                options: request.clone(),
                counts,
            },
            objects,
            principals,
            access,
            left_out: left,
        })
    }
}

fn renderings(entry: &Entry, digest: &impl Fn(&str) -> (String, i64)) -> Vec<PackageRendering> {
    [("markdown", &entry.reading), ("html", &entry.html)]
        .into_iter()
        .flat_map(|(format, path)| path.iter().map(move |path| (format, path)))
        .map(|(format, path)| PackageRendering {
            format: format.to_owned(),
            path: path.clone(),
            sha256: digest(path).0,
        })
        .collect()
}

fn files(entry: &Entry, digest: &impl Fn(&str) -> (String, i64)) -> Vec<PackageFile> {
    entry
        .files
        .iter()
        .map(|file| {
            let (sha256, bytes) = digest(&file.path);
            PackageFile {
                id: file.id,
                name: file.name.clone(),
                path: file.path.clone(),
                bytes,
                sha256,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_people_a_record_names_are_found_wherever_they_are_named() {
        let (author, mentioned, reacted, plain) = (
            orch_types::new_id(),
            orch_types::new_id(),
            orch_types::new_id(),
            orch_types::new_id(),
        );
        let record = json!([{
            "author": { "id": author, "username": "ana", "display_name": "Ana" },
            "content": [{ "type": "mention", "user_id": mentioned }],
            "reactions": [{ "emoji": "+1", "users": [reacted] }],
            "id": plain,
        }]);
        let mut found = BTreeSet::new();
        people_in(&record, &mut found);
        assert_eq!(found, BTreeSet::from([author, mentioned, reacted]));
    }
}
