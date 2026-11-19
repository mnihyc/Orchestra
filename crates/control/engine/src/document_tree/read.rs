//! Batched typed document entries and redacted ancestry for person and agent navigation.

use super::{Actor, Ancestor, Directory, Error, Item, Listed, TreeRead, access};
use orch_db::{Conn, queries::documents::containers};
use orch_types::{Id, Time};

/// Reads scoped node/item candidates in a batch, with readable folders and redacted structural ancestors.
pub async fn load_entries_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    nodes: &[Id],
    items: &[Item],
    now: Time,
) -> Result<TreeRead, Error> {
    let documents: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            Item::Document(id) => Some(*id),
            _ => None,
        })
        .collect();
    let folders: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            Item::Container(id) => Some(*id),
            _ => None,
        })
        .collect();
    let rows = containers::candidates(conn, nodes, &documents, &folders).await?;
    project_read(conn, actor, rows, now, true).await
}

/// Native readable entry operands. A consumer may discard entries for its own
/// existing output window, but cannot change their native content or standing.
#[derive(Debug)]
pub struct EntrySelection {
    entries: Vec<Listed>,
    rows: Vec<containers::EntryRow>,
}
impl EntrySelection {
    /// The actual rows and native standings selected on this read connection.
    #[must_use]
    pub fn entries(&self) -> &[Listed] {
        &self.entries
    }

    /// Restricts this native selection without manufacturing an entry or authority flag.
    pub fn retain(&mut self, keep: impl FnMut(&Listed) -> bool) {
        self.entries.retain(keep);
        let items: std::collections::HashSet<_> =
            self.entries.iter().map(|row| row.entry.item).collect();
        self.rows.retain(|row| items.contains(&access::item(row)));
    }
}

/// The native row/standing selector without ancestry or any response projection.
/// Callers may apply their existing output window before completing selected ancestry.
pub async fn select_entries_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    rows: Vec<containers::EntryRow>,
    now: Time,
    retained: bool,
) -> Result<EntrySelection, Error> {
    let held = access::standings(conn, actor, &rows, now, retained).await?;
    let entries: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            let standing = held.get(&access::item(row)).copied().unwrap_or_default();
            (standing.read && (row.deleted_at.is_none() || standing.manage)).then(|| Listed {
                entry: access::project(row),
                standing,
            })
        })
        .collect();
    let items: std::collections::HashSet<_> = entries.iter().map(|row| row.entry.item).collect();
    let rows = rows
        .into_iter()
        .filter(|row| items.contains(&access::item(row)))
        .collect();
    Ok(EntrySelection { entries, rows })
}

fn project_read(
    conn: &mut Conn,
    actor: Actor<'_>,
    rows: Vec<containers::EntryRow>,
    now: Time,
    retained: bool,
) -> impl std::future::Future<Output = Result<TreeRead, Error>> {
    // All document-reader facades retain only a handle to this projection.
    Box::pin(prepare_projection(conn, actor, rows, now, retained))
}

async fn prepare_projection(
    conn: &mut Conn,
    actor: Actor<'_>,
    rows: Vec<containers::EntryRow>,
    now: Time,
    retained: bool,
) -> Result<TreeRead, Error> {
    let entries = select_entries_in(conn, actor, rows, now, retained).await?;
    project_selected(conn, actor, entries.entries, now, retained).await
}

/// Completes redacted structural ancestry for a native readable selection after its window is chosen.
pub async fn project_selected_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    entries: EntrySelection,
    now: Time,
    retained: bool,
) -> Result<TreeRead, Error> {
    // A retained selection is factual data, never a loan of its producer's authority.
    let current = select_entries_in(conn, actor, entries.rows, now, retained).await?;
    let mut read = project_selected(conn, actor, current.entries, now, retained).await?;
    redact_retained_ancestors(conn, actor, &mut read, retained).await?;
    Ok(read)
}

async fn project_selected(
    conn: &mut Conn,
    actor: Actor<'_>,
    entries: Vec<Listed>,
    now: Time,
    retained: bool,
) -> Result<TreeRead, Error> {
    let ids: Vec<_> = entries.iter().map(|entry| entry.entry.item.id()).collect();
    let ancestors = containers::ancestors_many(conn, &ids).await?;
    let ancestor_rows: Vec<_> = ancestors
        .iter()
        .map(|row| containers::EntryRow {
            kind: "container".into(),
            id: row.id,
            node_id: row.node_id,
            org_id: row.org_id,
            parent_container_id: row.parent_container_id,
            basename: row.name.clone(),
            title: row.name.clone(),
            revision: row.revision,
            owner_id: row.owner_id,
            deleted_at: row.deleted_at,
            deleted_in: row.deleted_in,
            files: 0,
            versions: 0,
        })
        .collect();
    let held = access::standings(conn, actor, &ancestor_rows, now, retained).await?;
    let read = TreeRead {
        entries,
        ancestors: ancestor_rows
            .into_iter()
            .map(|row| {
                let standing = held
                    .get(&Item::Container(row.id))
                    .copied()
                    .unwrap_or_default();
                Ancestor {
                    id: row.id,
                    parent: row
                        .parent_container_id
                        .map_or(Directory::NodeDocs(row.node_id), Directory::Container),
                    node: row.node_id,
                    root: row.org_id,
                    basename: standing.read.then_some(row.basename),
                    title: standing.read.then_some(row.title),
                    standing,
                }
            })
            .collect(),
    };
    Ok(read)
}

/// Projects a bounded caller-selected metadata batch using the existing authority rules.
/// The person UI may inspect retained rows; no content or mutation authority is added.
pub async fn project_entries_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    rows: Vec<containers::EntryRow>,
    now: Time,
    retained: bool,
) -> Result<TreeRead, Error> {
    let mut read = project_read(conn, actor, rows, now, retained).await?;
    redact_retained_ancestors(conn, actor, &mut read, retained).await?;
    Ok(read)
}

async fn redact_retained_ancestors(
    conn: &mut Conn,
    actor: Actor<'_>,
    read: &mut TreeRead,
    retained: bool,
) -> Result<(), Error> {
    if retained && matches!(actor, Actor::Person(_)) {
        let ids: Vec<_> = read.ancestors.iter().map(|row| row.id).collect();
        let deleted: std::collections::HashSet<_> = containers::get_many(conn, &ids)
            .await?
            .into_iter()
            .filter(|row| row.deleted_at.is_some())
            .map(|row| row.id)
            .collect();
        for ancestor in &mut read.ancestors {
            if deleted.contains(&ancestor.id) && !ancestor.standing.manage {
                ancestor.basename = None;
                ancestor.title = None;
                ancestor.standing.read = false;
            }
        }
    }
    Ok(())
}

/// Exact typed metadata, with an explicit retained mode separate from normal navigation.
pub async fn metadata_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    item: Item,
    now: Time,
    retained: bool,
) -> Result<TreeRead, Error> {
    let kind = match item {
        Item::Document(_) => "document",
        Item::Container(_) => "container",
    };
    let row = containers::entry(conn, kind, item.id())
        .await?
        .filter(|row| retained || row.deleted_at.is_none())
        .ok_or(Error::NotFound)?;
    let read = project_entries_in(conn, actor, vec![row], now, retained).await?;
    if read.entries.is_empty() {
        return Err(Error::NotFound);
    }
    Ok(read)
}

/// The same native Create decision used by mutation, without acquiring a mutation lock.
pub async fn can_create_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    parent: Directory,
    now: Time,
) -> Result<bool, Error> {
    match access::require_create(conn, actor, parent, now).await {
        Ok(_) => Ok(true),
        Err(Error::Denied | Error::NotFound) => Ok(false),
        Err(Error::Authorization(
            orch_authz::Error::Archived { .. }
            | orch_authz::Error::Suspended { .. }
            | orch_authz::Error::NotFound(_),
        )) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Reads one current item only when the actual caller may read it.
pub async fn entry_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    item: Item,
    now: Time,
) -> Result<Listed, Error> {
    let read = read_in(conn, actor, item, now).await?;
    read.entries
        .into_iter()
        .find(|entry| entry.entry.item == item)
        .ok_or(Error::NotFound)
}

/// Reads exactly one entry and its required ancestry, without traversing that folder's children.
pub async fn read_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    item: Item,
    now: Time,
) -> Result<TreeRead, Error> {
    let kind = match item {
        Item::Document(_) => "document",
        Item::Container(_) => "container",
    };
    let row = containers::entry(conn, kind, item.id())
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or(Error::NotFound)?;
    let read = project_read(conn, actor, vec![row], now, false).await?;
    if read.entries.is_empty() {
        return Err(Error::NotFound);
    }
    Ok(read)
}

/// Lists the readable current children of an existing directory without fetching content.
pub async fn list_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    parent: Directory,
    now: Time,
) -> Result<Vec<Listed>, Error> {
    let (node, _, container) = access::directory(conn, parent).await?;
    if let Directory::Container(id) = parent {
        entry_in(conn, actor, Item::Container(id), now).await?;
    }
    let rows = containers::entries_in(conn, node, container, false).await?;
    Ok(project_read(conn, actor, rows, now, false).await?.entries)
}
