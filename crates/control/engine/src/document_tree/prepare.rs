//! Complete bounded membership, scope locks and authorization before document-tree mutation.

use super::{
    Actor, Command, Directory, Error, ITEMS_MAX, Item, MembershipStamp, Options, Plan, Request,
    access, create,
};
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::Permission;
use orch_db::{Conn, Tx, queries::documents::containers};
use orch_types::{Id, Time};
use std::collections::{HashMap, HashSet};

/// The same client identity bound as ordinary document creation and copy.
const REQUEST_REF_MAX_CHARS: usize = 200;

pub(super) struct Prepared {
    pub roots: Vec<containers::EntryRow>,
    pub entries: Vec<containers::EntryRow>,
    pub destination: Option<(Id, Id, Option<Id>)>,
    pub text_copy: Option<super::TextCopy>,
}

pub(super) fn kind(item: Item) -> &'static str {
    match item {
        Item::Document(_) => "document",
        Item::Container(_) => "container",
    }
}

pub(super) fn validate(request: &Request) -> Result<(), Error> {
    if request.request_ref.trim().is_empty()
        || request.request_ref.chars().count() > REQUEST_REF_MAX_CHARS
    {
        return Err(Error::Invalid(
            "give a nonempty request identity of at most two hundred characters",
        ));
    }
    if request.items.len() > ITEMS_MAX {
        return Err(Error::Limit);
    }
    if request.items.iter().copied().collect::<HashSet<_>>().len() != request.items.len() {
        return Err(Error::Invalid("select each document entry only once"));
    }
    match request.command {
        Command::CreateFolder => {
            if !request.items.is_empty() || request.to.is_none() || request.name.is_none() {
                return Err(Error::Invalid(
                    "folder creation needs one name and an existing parent",
                ));
            }
            create::validate_name(request.name.as_deref().unwrap_or_default(), true)?;
        }
        Command::Copy | Command::Move => {
            if request.items.is_empty()
                || request.to.is_none()
                || (request.name.is_some() && request.items.len() != 1)
            {
                return Err(Error::Invalid(
                    "copy and move need sources and an existing destination; a new name takes one source",
                ));
            }
        }
        Command::Rename => {
            if !matches!(request.items.as_slice(), [Item::Document(_)])
                || request.name.is_none()
                || request.to.is_some()
            {
                return Err(Error::Invalid("text rename needs one document and a title"));
            }
        }
        Command::Delete | Command::Restore => {
            if request.items.is_empty()
                || request.name.is_some()
                || (request.command == Command::Delete && request.to.is_some())
            {
                return Err(Error::Invalid(
                    "deletion and restoration name complete existing document entries",
                ));
            }
        }
        Command::RestoreVersion => {
            if !matches!(request.items.as_slice(), [Item::Document(_)])
                || request.version.is_none()
                || request.to.is_some()
                || request.name.is_some()
            {
                return Err(Error::Invalid(
                    "version restoration needs one document and one version",
                ));
            }
        }
    }
    if request.command != Command::RestoreVersion && request.version.is_some() {
        return Err(Error::Invalid("only version restoration takes a version"));
    }
    Ok(())
}

pub(super) async fn load_roots(
    conn: &mut Conn,
    request: &Request,
) -> Result<Vec<containers::EntryRow>, Error> {
    let mut rows = Vec::new();
    for item in &request.items {
        rows.push(
            containers::entry(conn, kind(*item), item.id())
                .await?
                .ok_or(Error::NotFound)?,
        );
    }
    Ok(rows)
}

pub(super) fn topological(
    entries: &[containers::EntryRow],
) -> Result<Vec<containers::EntryRow>, Error> {
    let folders: HashSet<_> = entries
        .iter()
        .filter(|row| row.kind == "container")
        .map(|row| row.id)
        .collect();
    let mut remaining = entries.to_vec();
    let mut out = Vec::new();
    let mut done = HashSet::new();
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut next = Vec::new();
        for row in remaining {
            if row
                .parent_container_id
                .is_none_or(|id| !folders.contains(&id) || done.contains(&id))
            {
                if row.kind == "container" {
                    done.insert(row.id);
                }
                out.push(row);
            } else {
                next.push(row)
            }
        }
        if before == next.len() {
            return Err(Error::Invalid("a document tree cannot contain a cycle"));
        }
        remaining = next;
    }
    Ok(out)
}

pub(super) fn live_entries(entries: &[containers::EntryRow]) -> Vec<containers::EntryRow> {
    let parents: HashMap<_, _> = entries
        .iter()
        .filter(|row| row.kind == "container")
        .map(|row| (row.id, row))
        .collect();
    entries
        .iter()
        .filter(|row| {
            if row.deleted_at.is_some() {
                return false;
            }
            let mut parent = row.parent_container_id;
            let mut seen = HashSet::new();
            while let Some(id) = parent {
                if !seen.insert(id) {
                    return false;
                }
                let Some(row) = parents.get(&id) else { break };
                if row.deleted_at.is_some() {
                    return false;
                }
                parent = row.parent_container_id;
            }
            true
        })
        .cloned()
        .collect()
}

pub(super) async fn prepare(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    now: Time,
) -> Result<Prepared, Error> {
    validate(request)?;
    let original = load_roots(conn, request).await?;
    let destination = match request.to {
        Some(parent) => Some(access::directory(conn, parent).await?),
        None => None,
    };
    let mut scopes: Vec<_> = original.iter().map(|row| row.node_id).collect();
    scopes.extend(destination.map(|place| place.0));
    containers::lock_scopes(conn, &scopes).await?;
    let roots = load_roots(conn, request).await?;
    if roots
        .iter()
        .zip(&original)
        .any(|(a, b)| a.node_id != b.node_id)
    {
        return Err(Error::Stale);
    }
    if let Some(parent) = request.to
        && Some(access::directory(conn, parent).await?) != destination
    {
        return Err(Error::Stale);
    }
    for root in &roots {
        if (request.command == Command::Restore) != root.deleted_at.is_some() {
            return Err(Error::NotFound);
        }
    }
    let first = containers::subtree(conn, &roots, ITEMS_MAX).await?;
    if first.len() > ITEMS_MAX {
        return Err(Error::Limit);
    }
    containers::lock_entries(conn, &first).await?;
    // Content/files may have changed while their row lock was being acquired.
    let roots = load_roots(conn, request).await?;
    let mut entries = containers::subtree(conn, &roots, ITEMS_MAX).await?;
    if entries.len() > ITEMS_MAX {
        return Err(Error::Limit);
    }
    topological(&entries)?;
    if matches!(request.command, Command::Copy) {
        entries = live_entries(&entries);
    }
    let permission = match request.command {
        Command::Copy => Permission::DocView,
        Command::Rename => Permission::DocEdit,
        _ => Permission::DocManage,
    };
    let act = if request.command == Command::Restore {
        orch_authz::Act::Delete
    } else {
        orch_authz::Act::Use
    };
    access::require_entries(
        conn,
        actor,
        &entries,
        permission,
        now,
        matches!(
            request.command,
            Command::Delete | Command::Move | Command::Restore
        ),
        act,
    )
    .await?;
    if let Some(parent) = request.to {
        access::require_create(conn, actor, parent, now).await?;
        if let Directory::Container(id) = parent
            && entries
                .iter()
                .any(|row| row.kind == "container" && row.id == id)
        {
            return Err(Error::Invalid(
                "a document folder cannot be put inside itself or its descendants",
            ));
        }
    }
    if request.command == Command::Restore {
        for root in &roots {
            let parent = root
                .parent_container_id
                .map_or(Directory::NodeDocs(root.node_id), Directory::Container);
            access::require_create(conn, actor, request.to.unwrap_or(parent), now).await?;
        }
    }
    if matches!(request.command, Command::Move | Command::Restore)
        && let Some((_, root, _)) = destination
        && entries.iter().any(|row| row.org_id != root)
    {
        return Err(Error::Invalid(
            "a moved document tree stays in its original root organization",
        ));
    }
    Ok(Prepared {
        roots,
        entries,
        destination,
        text_copy: None,
    })
}

pub(super) fn deadline(now: Time, options: Options) -> Result<DateTime<Utc>, Error> {
    let duration = chrono::Duration::from_std(options.retention)
        .map_err(|_| Error::Invalid("the retention interval is outside its supported range"))?;
    now.get().checked_add_signed(duration).ok_or(Error::Invalid(
        "the restoration deadline is outside its supported range",
    ))
}

pub(super) async fn stamp(
    conn: &mut Conn,
    entries: &[containers::EntryRow],
    now: Time,
) -> Result<MembershipStamp, Error> {
    let mut entries: Vec<_> = entries.iter().map(access::project).collect();
    entries.sort_by_key(|row| row.item);
    let state = containers::authority_state(
        conn,
        &entries.iter().map(|row| row.item.id()).collect::<Vec<_>>(),
        &entries.iter().map(|row| row.node).collect::<Vec<_>>(),
    )
    .await?;
    let mut hash = orch_types::Hash::hasher();
    hash.update(&serde_json::to_vec(&state)?);
    let authority_fingerprint = orch_types::Hash::finish(&hash).to_string();
    Ok(MembershipStamp {
        entries,
        authority_epoch: orch_authz::epoch::current(conn).await?,
        authority_fingerprint,
        authority_until: containers::authority_until(conn, now.get()).await?,
    })
}

/// Computes the complete authorized deletion preview without persisting a plan or changing an item.
pub async fn plan_delete_in(
    tx: &mut Tx,
    actor: Actor<'_>,
    request: &Request,
    now: Time,
    options: Options,
) -> Result<Plan, Error> {
    if request.command != Command::Delete {
        return Err(Error::Invalid("only deletion has a confirmation preview"));
    }
    let prepared = prepare(&mut *tx, actor, request, now).await?;
    let stamp = stamp(&mut *tx, &prepared.entries, now).await?;
    let audience = super::audience::read(&mut *tx, actor, &prepared.entries, now).await?;
    let (_, losing) = super::audience::difference(&audience, &HashMap::new(), &[]);
    Ok(Plan {
        stamp,
        restore_until: deadline(now, options)?,
        gaining: 0,
        losing,
    })
}
