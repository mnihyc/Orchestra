//! The plan of an import: what each object of the source would become here, which of them meet something that is there already and how they were found, who the people are, what is blocked, what does not come back, and what it all comes to against the limits.
//!
//! The plan is made from the opened source, the request and what is there now; nothing is written. Objects are ordered so that
//! what a thing lies in comes before it and the structure (places, threads, document folders) before the content (chats, documents).
//! An object is found to meet one that is here in two ways, in this order: an earlier import of the same thing made it or took it
//! over (`import_origins`), or a thing of the same name lies beside the same things. What is done with it is the default of the
//! way it was found, or what the person chose for it; a thing under one that is skipped is skipped with it.

use super::model::{
    Destination, Existing, Object, Opened, Plan, Wants, depth, made_as, may_hold, node_word,
};
use super::people::{Credits, match_people};
use super::report::kind_word;
use super::{ImportError, identity};
use crate::engine::Engine;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_access_contract::roles::Role;
use orch_api_contract::imports::{
    ImportAccessPlan, ImportAction, ImportAuthors, ImportBlock, ImportLimits, ImportLoss,
    ImportMatch, ImportRequest, ImportRoot, ImportSourceMode,
};
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::package::{PackageKind, PackageLeftOutWhat, PackageObject};
use orch_authz::{Act, Policy, Principal, nodes};
use orch_db::Conn;
use orch_db::queries::imports::{self as rows, Table};
use orch_db::queries::shares as places;
use orch_types::Id;
use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::sync::Arc;

impl Wants {
    /// Reads a request.
    pub fn read(request: &ImportRequest, max_items: i64) -> Result<Self, ImportError> {
        let parts = request.options.parts.unwrap_or_default();
        // As many choices as the place lets an import hold things, and as many people as there are things for them to have made.
        let most = usize::try_from(max_items).unwrap_or(0).max(1);
        if request.choices.objects.len() > most || request.choices.people.len() > most {
            return Err(ImportError::Invalid {
                field: "choices".to_owned(),
                message: "name more than the import may hold".to_owned(),
            });
        }
        Ok(Self {
            conversations: parts.conversations.unwrap_or(true),
            chats: parts.chats.unwrap_or(true),
            documents: parts.documents.unwrap_or(true),
            versions: request.options.versions.unwrap_or(true),
            comments: request.options.comments.unwrap_or(true),
            access: request.options.access.unwrap_or(false),
            authors: request.options.authors,
            same_source: request.options.same_source.unwrap_or(ImportAction::Replace),
            same_name: request.options.same_name.unwrap_or(ImportAction::KeepBoth),
            objects: request
                .choices
                .objects
                .iter()
                .map(|choice| (choice.source, choice.action))
                .collect(),
        })
    }
}

/// The nodes the person holds a permission on, asked once for each.
struct Rights<'a> {
    principal: &'a Principal,
    held: HashMap<(Id, Permission), bool>,
}

impl<'a> Rights<'a> {
    fn new(principal: &'a Principal) -> Self {
        Self {
            principal,
            held: HashMap::new(),
        }
    }

    async fn holds(
        &mut self,
        conn: &mut Conn,
        node: Id,
        permission: Permission,
    ) -> Result<bool, ImportError> {
        if let Some(held) = self.held.get(&(node, permission)) {
            return Ok(*held);
        }
        let answer = match nodes::check(
            conn,
            self.principal,
            node,
            permission,
            Act::Use,
            Policy::default(),
        )
        .await
        {
            Ok(orch_access_contract::decisions::Decision::Allowed { .. }) => true,
            Ok(orch_access_contract::decisions::Decision::Refused(_))
            | Err(orch_authz::Error::NotFound(_) | orch_authz::Error::Archived { .. }) => false,
            Err(error) => return Err(error.into()),
        };
        self.held.insert((node, permission), answer);
        Ok(answer)
    }

    /// Whether the person holds the permission on the document folder `folder` at this moment: a folder they cannot see is not one they
    /// hold anything on, and the answer is the same for one that is not there.
    async fn holds_in_folder(
        &mut self,
        conn: &mut Conn,
        folder: Id,
        permission: Permission,
    ) -> Result<bool, ImportError> {
        if let Some(held) = self.held.get(&(folder, permission)) {
            return Ok(*held);
        }
        let answer = match orch_authz::documents::check(
            conn,
            self.principal,
            folder,
            permission,
            Policy::default(),
        )
        .await
        {
            Ok(orch_access_contract::decisions::Decision::Allowed { .. }) => true,
            Ok(orch_access_contract::decisions::Decision::Refused(_))
            | Err(orch_authz::Error::NotFound(_) | orch_authz::Error::Archived { .. }) => false,
            Err(error) => return Err(error.into()),
        };
        self.held.insert((folder, permission), answer);
        Ok(answer)
    }
}

/// Which of the things `ids` of the kind `kind` the person may see, asked for all at once: the access engine answers for a list of
/// documents and folders together, and for a list of places. What they may not see is not told of: not by its name, its identifier or its
/// time.
async fn visible(
    conn: &mut Conn,
    principal: &Principal,
    kind: PackageKind,
    ids: &[Id],
) -> Result<HashSet<Id>, ImportError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(match kind {
        PackageKind::Document | PackageKind::Folder => {
            // A thousand at a time, letting the process's other work go on between them (as an export does).
            let mut seen = HashSet::new();
            for chunk in ids.chunks(1_000) {
                seen.extend(
                    orch_authz::documents::listed(conn, principal, chunk, Policy::default())
                        .await?
                        .into_iter()
                        .filter(|listed| listed.holds(Permission::DocView))
                        .map(|listed| listed.document.id),
                );
                tokio::task::yield_now().await;
            }
            seen
        }
        _ => nodes::holding(
            conn,
            principal,
            ids,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await?
        .into_iter()
        .map(|listed| listed.id)
        .collect(),
    })
}

/// The permission that makes a thing of `kind` in the node `node`, which is of the kind `parent`.
pub(super) fn creation_right(parent: NodeKind, kind: PackageKind) -> Option<Permission> {
    use PackageKind::{Chat, Document, Folder, Org, Project, Thread, Topic};
    match (kind, parent) {
        (Org | Project, NodeKind::Org) => Some(Permission::OrgInteract),
        (Project | Topic, NodeKind::Project) => Some(Permission::ProjectInteract),
        (Thread, NodeKind::Topic) => Some(Permission::TopicInteract),
        (Folder | Document, _) => Some(Permission::DocCreate),
        (Chat, _) => Some(Permission::ChatInteract),
        _ => None,
    }
}

fn block(code: &str, message: impl Into<String>) -> ImportBlock {
    ImportBlock {
        code: code.to_owned(),
        message: message.into(),
        params: None,
    }
}

fn block_with(code: &str, message: impl Into<String>, params: serde_json::Value) -> ImportBlock {
    ImportBlock {
        code: code.to_owned(),
        message: message.into(),
        params: Some(params),
    }
}

/// What a thing found beside the same things is, by the table it lives in.
fn table_of(kind: PackageKind) -> Table {
    match kind {
        PackageKind::Folder => Table::Containers,
        PackageKind::Document => Table::Documents,
        PackageKind::Chat => Table::Chats,
        _ => Table::Nodes,
    }
}

/// The kind of an origin, as the table of origins writes it.
#[must_use]
pub fn origin_kind(kind: PackageKind) -> &'static str {
    match kind {
        PackageKind::Org => "org",
        PackageKind::Project => "project",
        PackageKind::Topic => "topic",
        PackageKind::Thread => "thread",
        PackageKind::Chat => "chat",
        PackageKind::Folder => "folder",
        PackageKind::Document => "document",
    }
}

/// The names of a thing's places, from the top of the source.
fn path_of(by_id: &HashMap<Id, &PackageObject>, object: &PackageObject) -> Vec<String> {
    let mut path = Vec::new();
    let mut at = object.parent;
    let mut steps = 0;
    while let Some(parent) = at {
        let Some(found) = by_id.get(&parent) else {
            break;
        };
        path.push(if found.hidden || found.name.is_empty() {
            String::new()
        } else {
            found.name.clone()
        });
        at = found.parent;
        steps += 1;
        if steps > by_id.len() {
            break;
        }
    }
    path.reverse();
    path
}

/// A name for something that takes the place of a name that is taken: `Name (2)`, `Name (3)`, the first that is free.
#[must_use]
pub fn another_name(name: &str, taken: &HashSet<String>, longest: usize) -> String {
    for number in 2..10_000 {
        let tail = format!(" ({number})");
        let room = longest.saturating_sub(tail.chars().count());
        let stem: String = name.chars().take(room).collect();
        let candidate = format!("{}{tail}", stem.trim_end());
        if !taken.contains(&candidate.to_lowercase()) {
            return candidate;
        }
    }
    format!("{name} {}", orch_types::new_id().simple())
}

impl Engine {
    /// What the place allows of one import: the settings `import.max_bytes` and `import.max_items`, which follow the
    /// limits of an export of what lies there where nobody set them.
    pub async fn import_limits(
        &self,
        conn: &mut Conn,
        place: Id,
    ) -> Result<ImportLimits, ImportError> {
        let exports = self.export_limits(conn, place).await?;
        let max_bytes = orch_settings::get::<Option<i64>>(
            conn,
            orch_settings::Place::Node(place),
            orch_settings_contract::registry::keys::IMPORT_MAX_BYTES,
        )
        .await
        .map_err(crate::error::EngineError::from)?
        .flatten()
        .unwrap_or(exports.max_bytes);
        let max_items = orch_settings::get::<Option<i64>>(
            conn,
            orch_settings::Place::Node(place),
            orch_settings_contract::registry::keys::IMPORT_MAX_ITEMS,
        )
        .await
        .map_err(crate::error::EngineError::from)?
        .flatten()
        .unwrap_or(exports.max_items);
        Ok(ImportLimits {
            max_bytes,
            max_items,
        })
    }
}

/// Where a source object ends up here, as what lies in it needs to know.
#[derive(Debug, Clone, Copy)]
struct Where {
    /// Its identifier here.
    target: Id,
    /// The node it is, or for a document folder the node it lies beside.
    node: Id,
    /// What kind of node that is.
    node_kind: NodeKind,
    /// The kind it is made as.
    kind: PackageKind,
    /// Whether this import makes it.
    made: bool,
    /// Whether the import does anything with it.
    goes: bool,
}

/// Makes the plan of an import into the place `node`.
#[allow(
    clippy::too_many_lines,
    reason = "one reading in the order it is made: the place, the people, the matches, the rights, the totals"
)]
pub async fn build(
    engine: &Engine,
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    request: &ImportRequest,
    opened: Arc<Opened>,
) -> Result<Plan, ImportError> {
    let limits = engine.import_limits(conn, node).await?;
    let wants = Wants::read(request, limits.max_items)?;
    let importer = principal.user();
    let place = places::places(conn, &[node])
        .await?
        .into_iter()
        .next()
        .ok_or(ImportError::NotFound)?;
    let kind = crate::exports::node_kind(&place.kind);
    let destination = Destination {
        id: node,
        kind,
        name: place.name.clone(),
        org: place.org_id,
        archived: place.archived,
        folder: request.destination.folder,
        root: request.destination.root.unwrap_or_default(),
    };
    let mut rights = Rights::new(principal);
    let effective = nodes::effective(conn, principal, node, Policy::default()).await?;
    let admin =
        principal.is_system_admin() || effective.role.is_some_and(|held| held.role == Role::Admin);
    let manages_access = effective
        .held
        .iter()
        .any(|held| held.permission == Permission::AccessManage);
    let authors = match wants.authors {
        Some(ImportAuthors::Accounts) | None if admin => ImportAuthors::Accounts,
        _ => ImportAuthors::Labels,
    };
    let access = ImportAccessPlan {
        carried: opened.access.is_some(),
        allowed: manages_access,
        restored: wants.access && opened.access.is_some() && manages_access,
    };
    // The folder the documents go into must be one of this place's.
    if let Some(folder) = destination.folder {
        let found = orch_db::queries::documents::containers::get(conn, folder)
            .await?
            .filter(|row| row.deleted_at.is_none());
        // A folder the person may not add to is no folder of this place to them: the same answer as for one that is not there.
        let fits = found.is_some_and(|row| row.node_id == node)
            && rights
                .holds_in_folder(conn, folder, Permission::DocCreate)
                .await?;
        if !fits {
            return Err(ImportError::Invalid {
                field: "destination.folder".to_owned(),
                message: "is not a folder of this place".to_owned(),
            });
        }
    }

    // The people.
    let people = match_people(conn, &opened.principals, &request.choices.people).await?;
    let credits = Credits::new(importer, authors, people.clone());

    // Which objects are asked for, and in which order: structure before content, and a parent before what lies in it.
    let by_id: HashMap<Id, &PackageObject> = opened
        .objects
        .iter()
        .map(|object| (object.id, object))
        .collect();
    let kept = |object: &PackageObject| match object.kind {
        PackageKind::Document | PackageKind::Folder => wants.documents,
        PackageKind::Chat => wants.chats,
        _ => true,
    };
    let mut order: Vec<&PackageObject> = opened
        .objects
        .iter()
        .filter(|object| kept(object))
        .collect();
    order.sort_by_key(|object| {
        (
            u8::from(matches!(
                object.kind,
                PackageKind::Chat | PackageKind::Document
            )),
            depth(&by_id, object.id),
        )
    });

    // What an earlier import made of them, where it is still there.
    let source_ids: Vec<Id> = order.iter().map(|object| object.id).collect();
    let origin_rows = rows::origins_of(conn, node, &source_ids).await?;
    let mut live_origins: HashMap<Id, rows::OriginRow> = HashMap::new();
    let mut removed_origins: HashMap<Id, Id> = HashMap::new();
    for table in [
        Table::Nodes,
        Table::Documents,
        Table::Containers,
        Table::Chats,
    ] {
        let ids: Vec<Id> = origin_rows
            .iter()
            .filter(|row| table_of(origin_package_kind(&row.kind)) == table)
            .map(|row| row.object_id)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let alive: HashSet<Id> = rows::alive(conn, table, &ids).await?.into_iter().collect();
        for row in &origin_rows {
            if table_of(origin_package_kind(&row.kind)) == table {
                if alive.contains(&row.object_id) {
                    live_origins.insert(row.source_id, row.clone());
                } else {
                    removed_origins.insert(row.source_id, row.object_id);
                }
            }
        }
    }

    // What an earlier import made is found again only by one who may see it now.
    {
        let mut by_kind: HashMap<PackageKind, Vec<Id>> = HashMap::new();
        for row in live_origins.values() {
            let kind = match origin_package_kind(&row.kind) {
                PackageKind::Org
                | PackageKind::Project
                | PackageKind::Topic
                | PackageKind::Thread => PackageKind::Project,
                other => other,
            };
            if kind != PackageKind::Chat {
                by_kind.entry(kind).or_default().push(row.object_id);
            }
        }
        let mut hidden: HashSet<Id> = HashSet::new();
        for (kind, ids) in &by_kind {
            let seen = visible(conn, principal, *kind, ids).await?;
            hidden.extend(ids.iter().filter(|id| !seen.contains(id)));
        }
        if !hidden.is_empty() {
            live_origins.retain(|_, row| !hidden.contains(&row.object_id));
        }
    }
    let documents_here = crate::tools::edit::settings_at(conn, node).await?.enabled;
    let root_id = opened
        .objects
        .iter()
        .find(|object| object.parent.is_none())
        .map(|object| object.id);
    let here = destination.root == ImportRoot::Here && opened.mode == ImportSourceMode::Package;
    let at_destination = Where {
        target: node,
        node,
        node_kind: destination.kind,
        kind: destination_kind(&destination),
        made: false,
        goes: true,
    };
    let mut names_by_parent: HashMap<(Option<Id>, PackageKind), Vec<String>> = HashMap::new();
    for object in &order {
        names_by_parent
            .entry((object.parent, made_as(object.kind)))
            .or_default()
            .push(object.name.clone());
    }
    type Key = (Option<Id>, PackageKind);
    let mut siblings: HashMap<Key, HashMap<String, rows::Sibling>> = HashMap::new();
    let mut resolved: HashMap<Id, Where> = HashMap::new();
    let mut planned: Vec<Object> = Vec::with_capacity(order.len());
    let mut index: HashMap<Id, usize> = HashMap::new();

    for object in order {
        let made = made_as(object.kind);
        let top = object.parent.is_none();
        let above = match object.parent {
            None => at_destination,
            Some(parent) => resolved.get(&parent).copied().unwrap_or(Where {
                goes: false,
                ..at_destination
            }),
        };
        let path = path_of(&by_id, object);
        // What the package is of, into the place itself: nothing is made, and what lay in it lies here.
        if here && Some(object.id) == root_id {
            let fits = matches!(
                (made, destination.kind),
                (PackageKind::Project, NodeKind::Org | NodeKind::Project)
                    | (PackageKind::Topic, NodeKind::Topic)
                    | (PackageKind::Thread, NodeKind::Thread)
            );
            let blocked = (!fits).then(|| {
                block(
                    "import.unsupported",
                    "what the package is of cannot go into this place itself; choose to make it inside",
                )
            });
            resolved.insert(
                object.id,
                Where {
                    goes: fits,
                    ..at_destination
                },
            );
            index.insert(object.id, planned.len());
            planned.push(Object {
                object: object.clone(),
                action: if fits {
                    ImportAction::Replace
                } else {
                    ImportAction::Skip
                },
                allowed: Vec::new(),
                existing: Some(Existing {
                    id: node,
                    name: destination.name.clone(),
                    found_by: ImportMatch::Name,
                    updated_at: None,
                    origin: None,
                }),
                blocked,
                lies_in: node,
                container: None,
                parent_made: false,
                skipped_above: false,
                target: node,
                made_as: made,
                bare: false,
                path,
            });
            continue;
        }
        let skipped_above = !above.goes;
        let lies_in = above.node;
        let container = match made {
            PackageKind::Folder | PackageKind::Document => {
                if above.kind == PackageKind::Folder {
                    Some(above.target)
                } else if top || above.target == node {
                    destination.folder.filter(|_| above.target == node)
                } else {
                    None
                }
            }
            _ => None,
        };
        let parent_made = above.made;
        // Whether it may lie where it goes.
        let mut blocked: Option<ImportBlock> = None;
        if !skipped_above {
            if !may_hold(above.kind, made) {
                blocked = Some(block_with(
                    "import.unsupported",
                    format!(
                        "a {} cannot lie in a {} here",
                        kind_name(made),
                        kind_name(above.kind)
                    ),
                    serde_json::json!({ "kind": kind_word(made), "parent_kind": kind_word(above.kind) }),
                ));
            } else if matches!(made, PackageKind::Document | PackageKind::Folder) && !documents_here
            {
                blocked = Some(block("import.disabled", "documents are switched off here"));
            } else if destination.archived {
                blocked = Some(block("import.forbidden", "this place is archived"));
            } else if !parent_made
                && let Some(permission) = creation_right(above.node_kind, made)
                && !rights.holds(conn, lies_in, permission).await?
            {
                blocked = Some(block("import.forbidden", "you may not make this here"));
            } else if !parent_made
                && matches!(made, PackageKind::Document | PackageKind::Folder)
                && let Some(folder) = container
                && !rights
                    .holds_in_folder(conn, folder, Permission::DocCreate)
                    .await?
            {
                blocked = Some(block("import.forbidden", "you may not make this here"));
            }
        }

        // What it meets that is here: what an earlier import made of it, or what has its name beside the same things.
        let mut existing: Option<Existing> = live_origins.get(&object.id).map(|row| Existing {
            id: row.object_id,
            name: row
                .source_name
                .clone()
                .unwrap_or_else(|| object.name.clone()),
            found_by: ImportMatch::Source,
            updated_at: None,
            origin: Some(row.clone()),
        });
        if existing.is_none()
            && !parent_made
            && !skipped_above
            && blocked.is_none()
            && made != PackageKind::Chat
        {
            let key: Key = (object.parent, made);
            if let Entry::Vacant(entry) = siblings.entry(key) {
                let names = names_by_parent.get(&key).cloned().unwrap_or_default();
                let found = match made {
                    PackageKind::Folder => {
                        rows::sibling_folders(conn, lies_in, container, &names).await?
                    }
                    PackageKind::Document => {
                        rows::sibling_documents(conn, lies_in, container, &names).await?
                    }
                    _ => {
                        rows::sibling_nodes(
                            conn,
                            lies_in,
                            node_word(made).unwrap_or("project"),
                            &names,
                        )
                        .await?
                    }
                };
                let ids: Vec<Id> = found.iter().map(|each| each.id).collect();
                let seen = visible(conn, principal, made, &ids).await?;
                entry.insert(
                    found
                        .into_iter()
                        .filter(|each| seen.contains(&each.id))
                        .map(|each| (each.asked.to_lowercase(), each))
                        .collect(),
                );
            }
            if let Some(found) = siblings
                .get(&key)
                .and_then(|group| group.get(&object.name.to_lowercase()))
            {
                existing = Some(Existing {
                    id: found.id,
                    name: found.name.clone(),
                    found_by: ImportMatch::Name,
                    updated_at: Some(found.updated_at),
                    origin: None,
                });
            }
        }
        if made == PackageKind::Chat {
            // A chat is always the chat of the node it belongs to: what is brought is added to it.
            existing = None;
        }

        // What may be done, and what is.
        let (allowed, default) = match (&existing, made) {
            (_, PackageKind::Chat) | (None, _) => (
                vec![ImportAction::Create, ImportAction::Skip],
                ImportAction::Create,
            ),
            (Some(found), _) => (
                vec![
                    ImportAction::Replace,
                    ImportAction::KeepBoth,
                    ImportAction::Skip,
                ],
                match found.found_by {
                    ImportMatch::Source => wants.same_source,
                    ImportMatch::Name => wants.same_name,
                },
            ),
        };
        let default = if allowed.contains(&default) {
            default
        } else if default == ImportAction::Create {
            ImportAction::KeepBoth
        } else {
            ImportAction::Skip
        };
        let chosen = wants
            .objects
            .get(&object.id)
            .copied()
            .filter(|each| allowed.contains(each))
            .unwrap_or(default);
        let action = if skipped_above || blocked.is_some() {
            ImportAction::Skip
        } else {
            chosen
        };
        let target = match (&existing, action) {
            (Some(found), ImportAction::Replace) => found.id,
            _ => {
                let first = identity::object(importer, node, opened.package, object.id);
                // Undo retains removed rows for restoration. Their identity is
                // not available to a new create, even though the origin is not live.
                removed_origins
                    .get(&object.id)
                    .map_or(first, |removed| identity::recreated(first, *removed))
            }
        };
        let goes = !skipped_above && blocked.is_none() && action != ImportAction::Skip;
        let is_node = matches!(
            made,
            PackageKind::Project | PackageKind::Topic | PackageKind::Thread
        );
        resolved.insert(
            object.id,
            Where {
                target,
                node: if is_node { target } else { lies_in },
                node_kind: match made {
                    PackageKind::Topic => NodeKind::Topic,
                    PackageKind::Thread => NodeKind::Thread,
                    PackageKind::Project => NodeKind::Project,
                    _ => above.node_kind,
                },
                kind: made,
                made: goes && action != ImportAction::Replace,
                goes,
            },
        );
        index.insert(object.id, planned.len());
        planned.push(Object {
            bare: made == PackageKind::Thread && (object.path.is_none() || !wants.conversations),
            object: object.clone(),
            action,
            allowed,
            existing,
            blocked,
            lies_in,
            container,
            parent_made,
            skipped_above,
            target,
            made_as: made,
            path,
        });
    }

    // What it comes to.
    let items = i64::try_from(
        planned
            .iter()
            .filter(|each| each.goes())
            .filter(|each| each.action != ImportAction::Replace || !each.made_as.is_place())
            .count(),
    )
    .unwrap_or(i64::MAX);
    let bytes = planned
        .iter()
        .filter(|each| each.goes())
        .map(|each| {
            each.object.bytes + each.object.files.iter().map(|file| file.bytes).sum::<i64>()
        })
        .sum::<i64>();

    // What does not come back.
    let mut losses: Vec<ImportLoss> = opened.notes.clone();
    if opened.mode == ImportSourceMode::Package {
        for left in &opened.left_out {
            losses.push(ImportLoss {
                code: left_out_code(left.what).to_owned(),
                count: left.count,
                message: left_out_text(left.what, left.count),
                object: None,
                params: None,
            });
        }
        let orgs = i64::try_from(
            opened
                .objects
                .iter()
                .filter(|object| object.kind == PackageKind::Org)
                .count(),
        )
        .unwrap_or(i64::MAX);
        if orgs > 0 {
            losses.push(ImportLoss {
                code: "org_as_project".to_owned(),
                count: orgs,
                message: "orgs of the package come back as projects".to_owned(),
                object: None,
                params: None,
            });
        }
        if wants.access && !access.restored {
            losses.push(ImportLoss {
                code: "sharing_not_restored".to_owned(),
                count: 0,
                message: if opened.access.is_some() {
                    "the sharing of the package is not restored: it needs Manage access here"
                        .to_owned()
                } else {
                    "the package carries no sharing".to_owned()
                },
                object: None,
                params: Some(serde_json::json!({
                    "reason": if opened.access.is_some() { "needs_manage" } else { "not_carried" }
                })),
            });
        }
        let unnamed: HashSet<Id> = planned
            .iter()
            .filter(|each| each.goes())
            .filter_map(|each| each.object.creator)
            .filter(|who| {
                !opened
                    .principals
                    .iter()
                    .any(|principal| principal.id == *who)
            })
            .collect();
        if !unnamed.is_empty() {
            losses.push(ImportLoss {
                code: "unknown_person".to_owned(),
                count: i64::try_from(unnamed.len()).unwrap_or(i64::MAX),
                message: "some things are by people the package does not name".to_owned(),
                object: None,
                params: None,
            });
        }
    }

    Ok(Plan {
        destination,
        opened,
        objects: planned,
        index,
        people,
        credits,
        authors,
        access,
        losses,
        limits,
        admin,
        items,
        bytes,
        wants,
    })
}

fn destination_kind(destination: &Destination) -> PackageKind {
    match destination.kind {
        NodeKind::Org => PackageKind::Org,
        NodeKind::Project => PackageKind::Project,
        NodeKind::Topic => PackageKind::Topic,
        NodeKind::Thread => PackageKind::Thread,
    }
}

fn origin_package_kind(word: &str) -> PackageKind {
    match word {
        "org" => PackageKind::Org,
        "project" => PackageKind::Project,
        "topic" => PackageKind::Topic,
        "thread" => PackageKind::Thread,
        "chat" => PackageKind::Chat,
        "folder" => PackageKind::Folder,
        _ => PackageKind::Document,
    }
}

fn kind_name(kind: PackageKind) -> &'static str {
    match kind {
        PackageKind::Org => "org",
        PackageKind::Project => "project",
        PackageKind::Topic => "topic",
        PackageKind::Thread => "thread",
        PackageKind::Chat => "chat",
        PackageKind::Folder => "document folder",
        PackageKind::Document => "document",
    }
}

fn left_out_code(what: PackageLeftOutWhat) -> &'static str {
    match what {
        PackageLeftOutWhat::NotExportable => "not_exportable",
        PackageLeftOutWhat::TooLarge => "too_large",
        PackageLeftOutWhat::NativeSessions => "native_sessions",
        PackageLeftOutWhat::Workspaces => "workspaces",
        PackageLeftOutWhat::Secrets => "secrets",
        PackageLeftOutWhat::UsageAndAudit => "usage_and_audit",
        PackageLeftOutWhat::PersonalState => "personal_state",
        PackageLeftOutWhat::WebImages => "web_images",
    }
}

fn left_out_text(what: PackageLeftOutWhat, count: i64) -> String {
    match what {
        PackageLeftOutWhat::NotExportable => format!(
            "{count} things were left out of the package: their maker could read them and not take them away"
        ),
        PackageLeftOutWhat::TooLarge => format!("{count} files were too large for the package"),
        PackageLeftOutWhat::NativeSessions => {
            "native sessions of agent programs do not come back: threads are imported as history"
                .to_owned()
        }
        PackageLeftOutWhat::Workspaces => "workspaces do not come back".to_owned(),
        PackageLeftOutWhat::Secrets => "secrets do not come back".to_owned(),
        PackageLeftOutWhat::UsageAndAudit => "usage and audit do not come back".to_owned(),
        PackageLeftOutWhat::PersonalState => {
            "what people keep for themselves, and approvals, do not come back".to_owned()
        }
        PackageLeftOutWhat::WebImages => "images on the web stay links".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_is_taken_gets_a_number_that_is_free_and_fits() {
        let taken: HashSet<String> = ["plan", "plan (2)"]
            .iter()
            .map(|each| (*each).to_owned())
            .collect();
        assert_eq!(another_name("Plan", &taken, 200), "Plan (3)");
        let long = "x".repeat(200);
        let name = another_name(&long, &HashSet::new(), 200);
        assert_eq!(name.chars().count(), 200);
        assert!(name.ends_with(" (2)"));
    }

    #[test]
    fn what_makes_a_thing_is_the_right_of_the_place_it_lies_in() {
        assert_eq!(
            creation_right(NodeKind::Org, PackageKind::Project),
            Some(Permission::OrgInteract)
        );
        assert_eq!(
            creation_right(NodeKind::Project, PackageKind::Topic),
            Some(Permission::ProjectInteract)
        );
        assert_eq!(
            creation_right(NodeKind::Topic, PackageKind::Thread),
            Some(Permission::TopicInteract)
        );
        assert_eq!(
            creation_right(NodeKind::Topic, PackageKind::Document),
            Some(Permission::DocCreate)
        );
        assert_eq!(creation_right(NodeKind::Thread, PackageKind::Topic), None);
    }
}
