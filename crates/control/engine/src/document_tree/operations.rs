//! Atomic document-tree acts, exact deletion cohorts and actor-scoped durable replay.

use super::{
    Actor, Command, Error, Item, MembershipStamp, Options, Receipt, Request, access, audience,
    copying, create, prepare,
};
use orch_access_contract::permissions::Permission;
use orch_db::{
    Conn, Tx,
    audit::{self, Actor as AuditActor, Entry as AuditEntry},
    queries::documents::{self as rows, batch, containers},
};
use orch_documents::versions::Writer;
use orch_types::{Id, Time, new_id};
use serde_json::json;
use std::collections::HashSet;

pub(super) mod checkpoint;
mod sources;
mod staged;
pub(super) use checkpoint::Checkpoint;
pub use sources::{OperationSources, prepare_operation_sources_in, require_operation_sources_in};
pub use staged::{Operation, apply_operation_in, complete_operation_in, require_operation_in};

fn validate_root_name(row: &containers::EntryRow, name: &str) -> Result<(), Error> {
    create::validate_name(name, row.kind == "container")?;
    if row.kind == "document" && !name.ends_with(".md") {
        return Err(Error::Invalid("a text document basename ends in .md"));
    }
    Ok(())
}

async fn namespace(
    conn: &mut Conn,
    request: &Request,
    prepared: &prepare::Prepared,
) -> Result<(), Error> {
    if request.command == Command::CreateFolder {
        let (node, _, parent) = prepared.destination.ok_or(Error::NotFound)?;
        if containers::name_taken(
            conn,
            node,
            parent,
            request.name.as_deref().unwrap_or_default(),
            None,
        )
        .await?
        {
            return Err(Error::Conflict);
        }
    }
    if matches!(request.command, Command::Copy | Command::Move) {
        let (node, _, parent) = prepared.destination.ok_or(Error::NotFound)?;
        let mut names = HashSet::new();
        for row in &prepared.roots {
            let name = request.name.as_deref().unwrap_or(&row.basename);
            validate_root_name(row, name)?;
            if request.command == Command::Move
                && row.node_id == node
                && row.parent_container_id == parent
                && row.basename == name
            {
                return Err(Error::Invalid(
                    "the document entry is already in this directory",
                ));
            }
            if !names.insert(name.to_ascii_lowercase()) {
                return Err(Error::Conflict);
            }
            let except = (request.command == Command::Move).then_some((row.kind.as_str(), row.id));
            if containers::name_taken(conn, node, parent, name, except).await? {
                return Err(Error::Conflict);
            }
        }
    }
    if request.command == Command::Rename {
        let row = prepared.roots.first().ok_or(Error::NotFound)?;
        let title = request.name.as_deref().unwrap_or_default().trim();
        if title.is_empty()
            || title.chars().count()
                > usize::try_from(orch_api_contract::documents::DOCUMENT_TITLE_MAX_CHARS)
                    .unwrap_or(usize::MAX)
        {
            return Err(Error::Invalid(
                "a document title has one to two hundred characters",
            ));
        }
        let name = if title == row.title {
            row.basename.clone()
        } else {
            create::basename(title)
        };
        if containers::name_taken(
            conn,
            row.node_id,
            row.parent_container_id,
            &name,
            Some((&row.kind, row.id)),
        )
        .await?
        {
            return Err(Error::Conflict);
        }
    }
    Ok(())
}

struct AuditChange<'a> {
    row: &'a containers::EntryRow,
    operation: Id,
    action: &'a str,
    source: Option<Item>,
    until: Option<chrono::DateTime<chrono::Utc>>,
    /// The place it was in before, where that is another: its line belongs to the trail of both.
    from_node: Option<Id>,
}

async fn record(
    conn: &mut Conn,
    actor: Actor<'_>,
    change: AuditChange<'_>,
    now: Time,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<(), Error> {
    let identity = access::identity(conn, actor).await?;
    let who = identity
        .thread
        .map_or(AuditActor::User(identity.owner), AuditActor::Agent);
    let kind = if change.row.kind == "container" {
        "document_container"
    } else {
        "document"
    };
    let noun = if change.row.kind == "container" {
        "document folder"
    } else {
        "document"
    };
    let summary = format!(
        "{} {noun} \"{}\"",
        change.action.rsplit('.').next().unwrap_or("changed"),
        change.row.title
    );
    let mut entry = AuditEntry::new(who, change.action, kind, Some(change.row.id), &summary);
    entry.node_id = Some(change.row.node_id);
    entry.org_id = Some(change.row.org_id);
    entry.history = true;
    entry.details = json!({"title":change.row.title,"node":change.row.node_id,"operation":change.operation,
        "source":change.source,"copied_from":change.source.map(Item::id),"purge_at":change.until,"deletion_id":change.until.map(|_|change.operation)});
    if let Some(from) = change.from_node {
        // The trail of the place it left reads the line too.
        entry.details["from_node"] = json!(from);
    }
    checkpoint.check()?;
    audit::append(conn, &entry, &audit::current_origin(), now.get()).await?;
    Ok(())
}

async fn reread(conn: &mut Conn, items: &[Item]) -> Result<Vec<containers::EntryRow>, Error> {
    let mut out = Vec::new();
    for item in items {
        out.push(
            containers::entry(conn, prepare::kind(*item), item.id())
                .await?
                .ok_or(Error::NotFound)?,
        );
    }
    Ok(out)
}

async fn move_entries_with_identity(
    conn: &mut Conn,
    identity: &access::Identity,
    request: &Request,
    prepared: &prepare::Prepared,
    now: Time,
) -> Result<Vec<Item>, Error> {
    move_entries_checked(
        conn,
        identity,
        request,
        prepared,
        now,
        &mut Checkpoint::native(),
    )
    .await
}

async fn move_entries_checked(
    conn: &mut Conn,
    identity: &access::Identity,
    request: &Request,
    prepared: &prepare::Prepared,
    now: Time,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Vec<Item>, Error> {
    let (node, root, parent) = prepared.destination.ok_or(Error::NotFound)?;
    let selected: HashSet<_> = prepared.roots.iter().map(access::item).collect();
    let by = identity.thread.is_none().then_some(identity.owner);
    let mut out = Vec::new();
    for row in prepare::topological(&prepared.entries)? {
        let item = access::item(&row);
        let selected = selected.contains(&item);
        let parent = if selected {
            parent
        } else {
            row.parent_container_id
        };
        let name = if selected {
            request.name.as_deref().unwrap_or(&row.basename)
        } else {
            &row.basename
        };
        let title = if row.kind == "document" && name != row.basename {
            name.strip_suffix(".md")
                .ok_or(Error::Invalid("a text document basename ends in .md"))?
        } else {
            &row.title
        };
        if row.node_id != node || row.parent_container_id != parent || row.basename != name {
            checkpoint.check()?;
            if row.kind == "container" {
                sqlx::query("UPDATE document_containers SET node_id=$2,org_id=$3,parent_container_id=$4,name=$5,updated_at=$6 WHERE id=$1")
                    .bind(row.id).bind(node).bind(root).bind(parent).bind(name).bind(now.get()).execute(&mut *conn).await?;
            } else {
                let seq:i64=sqlx::query_scalar("UPDATE documents SET node_id=$2,org_id=$3,parent_container_id=$4,title=$5,basename=$6,
                    doc_seq=doc_seq+1,seq_updated=doc_seq+1,updated_at=$7,updated_by=$8 WHERE id=$1 RETURNING doc_seq")
                    .bind(row.id).bind(node).bind(root).bind(parent).bind(title).bind(name).bind(now.get()).bind(by).fetch_one(&mut *conn).await?;
                rows::notify(conn, row.id, seq).await?;
                sqlx::query("UPDATE nodes SET readme_document_id=NULL WHERE readme_document_id=$1 AND id<>$2")
                    .bind(row.id).bind(node).execute(&mut *conn).await?;
            }
        }
        out.push(item);
    }
    Ok(out)
}

async fn delete_entries(
    conn: &mut Conn,
    identity: &access::Identity,
    prepared: &prepare::Prepared,
    operation: Id,
    now: Time,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Vec<Item>, Error> {
    let by = identity.thread.is_none().then_some(identity.owner);
    let mut out = Vec::new();
    for row in &prepared.entries {
        if row.deleted_at.is_none() {
            checkpoint.check()?;
            if row.kind == "container" {
                sqlx::query("UPDATE document_containers SET deleted_at=$2,deleted_by=$3,deleted_in=$4,updated_at=$2 WHERE id=$1")
                    .bind(row.id).bind(now.get()).bind(by).bind(operation).execute(&mut *conn).await?;
            } else {
                rows::touch(conn, row.id).await?;
                sqlx::query(
                    "UPDATE documents SET deleted_at=$2,deleted_by=$3,deleted_in=$4 WHERE id=$1",
                )
                .bind(row.id)
                .bind(now.get())
                .bind(by)
                .bind(operation)
                .execute(&mut *conn)
                .await?;
            }
            out.push(access::item(row));
        }
    }
    Ok(out)
}

async fn restoring_selection(
    conn: &mut Conn,
    prepared: &prepare::Prepared,
    now: Time,
    options: Options,
) -> Result<
    (
        Vec<containers::EntryRow>,
        Option<chrono::DateTime<chrono::Utc>>,
    ),
    Error,
> {
    let mut selected = HashSet::new();
    let mut cohorts = HashSet::new();
    for row in &prepared.roots {
        selected.insert(access::item(row));
        cohorts.extend(row.deleted_in);
    }
    let restoring: Vec<_> = prepare::topological(&prepared.entries)?
        .into_iter()
        .filter(|row| {
            row.deleted_at.is_some()
                && (selected.contains(&access::item(row))
                    || row
                        .deleted_in
                        .is_some_and(|cohort| cohorts.contains(&cohort)))
        })
        .collect();
    // Existing deletion promises win over a changed deployment retention setting.
    let mut until = None;
    for row in &restoring {
        let deadline = super::restore_deadline_in(conn, &access::project(row), options)
            .await?
            .ok_or(Error::Conflict)?;
        if deadline <= now.get() {
            return Err(Error::Conflict);
        }
        until = Some(
            until.map_or(deadline, |current: chrono::DateTime<chrono::Utc>| {
                current.min(deadline)
            }),
        );
    }
    Ok((restoring, until))
}

async fn restore_selected(
    conn: &mut Conn,
    restoring: Vec<containers::EntryRow>,
    now: Time,
) -> Result<Vec<Item>, Error> {
    restore_selected_checked(conn, restoring, now, &mut Checkpoint::native()).await
}

async fn restore_selected_checked(
    conn: &mut Conn,
    restoring: Vec<containers::EntryRow>,
    now: Time,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Vec<Item>, Error> {
    let mut out = Vec::new();
    for row in restoring {
        checkpoint.check()?;
        if row.kind == "container" {
            sqlx::query("UPDATE document_containers SET deleted_at=NULL,deleted_by=NULL,deleted_in=NULL,updated_at=$2 WHERE id=$1")
                .bind(row.id).bind(now.get()).execute(&mut *conn).await?;
        } else {
            sqlx::query(
                "UPDATE documents SET deleted_at=NULL,deleted_by=NULL,deleted_in=NULL WHERE id=$1",
            )
            .bind(row.id)
            .execute(&mut *conn)
            .await?;
            rows::touch(conn, row.id).await?;
        }
        out.push(access::item(&row));
    }
    Ok(out)
}

async fn insert_folder(
    conn: &mut Conn,
    request: &Request,
    prepared: &prepare::Prepared,
    identity: &access::Identity,
    id: Id,
    now: Time,
) -> Result<(Vec<Item>, Vec<super::Copied>), Error> {
    insert_folder_checked(
        conn,
        request,
        prepared,
        identity,
        id,
        now,
        &mut Checkpoint::native(),
    )
    .await
}

async fn insert_folder_checked(
    conn: &mut Conn,
    request: &Request,
    prepared: &prepare::Prepared,
    identity: &access::Identity,
    id: Id,
    now: Time,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<(Vec<Item>, Vec<super::Copied>), Error> {
    let (node, root, parent) = prepared.destination.ok_or(Error::NotFound)?;
    checkpoint.check()?;
    containers::insert(
        conn,
        &containers::NewContainer {
            id,
            node,
            root,
            parent,
            owner: identity.owner,
            thread: identity.thread,
            agent: identity.agent.as_deref(),
            name: request.name.as_deref().unwrap_or_default(),
            now: now.get(),
        },
    )
    .await?;
    create::creation_grant(conn, id, true, identity, now).await?;
    Ok((vec![Item::Container(id)], Vec::new()))
}

enum TreeState {
    Replay {
        receipt: Receipt,
        items: Vec<Item>,
    },
    New {
        prepared: prepare::Prepared,
        before: std::collections::HashMap<Item, std::collections::BTreeSet<Id>>,
        operation: Id,
    },
}

struct PreparedTree {
    request: Request,
    identity: access::Identity,
    arguments: orch_types::Json,
    state: TreeState,
}

/// Native folder-creation preparation, consumed once on its original transaction.
/// It contains factual locked operands and replay data, never reusable authority.
pub struct FolderCreation {
    packet: PreparedTree,
    transaction: String,
}

impl FolderCreation {
    /// The native request's actual destination directory.
    #[must_use]
    pub fn destination(&self) -> Option<super::Directory> {
        self.packet.request.to
    }

    /// Whether the native request already has its durable completed outcome.
    #[must_use]
    pub fn is_replay(&self) -> bool {
        matches!(&self.packet.state, TreeState::Replay { .. })
    }
    /// Identities from the native durable outcome, even when a later purge removed its rows.
    #[must_use]
    pub fn replay_items(&self) -> &[Item] {
        match &self.packet.state {
            TreeState::Replay { items, .. } => items,
            TreeState::New { .. } => &[],
        }
    }
}

async fn transaction_id(conn: &mut Conn) -> Result<String, Error> {
    Ok(sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(conn)
        .await?)
}

/// The ordinary native locks, authority, namespace, audience and replay checks for
/// one folder creation. Entry preparation outside a transaction is provisional;
/// application refuses a packet from a different transaction.
pub async fn prepare_folder_creation_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    now: Time,
) -> Result<FolderCreation, Error> {
    if request.command != Command::CreateFolder {
        return Err(Error::Invalid("this preparation requires folder creation"));
    }
    let packet = prepare_tree(conn, actor, request, None, now, None).await?;
    Ok(FolderCreation {
        packet,
        transaction: transaction_id(conn).await?,
    })
}

/// Repeats only the prepared operation's native ordinary gate with the current
/// reader. Durable replay keeps its existing View/retained-Manage rule.
pub async fn require_folder_creation_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    prepared: &FolderCreation,
    now: Time,
) -> Result<(), Error> {
    match &prepared.packet.state {
        TreeState::New { .. } => {
            let destination = prepared.packet.request.to.ok_or(Error::NotFound)?;
            access::require_create(conn, actor, destination, now).await?;
        }
        TreeState::Replay { receipt, .. } => {
            require_replayed_entries(conn, actor, receipt, now).await?
        }
    }
    Ok(())
}

/// Applies the same native folder writer with its original savepoint and receipt.
/// The outer error is the opaque caller's check; the inner error is the unchanged
/// native operation error. Replays do not invoke a first-effect callback.
pub async fn apply_folder_creation_in<E, F>(
    tx: &mut Tx,
    actor: Actor<'_>,
    prepared: FolderCreation,
    now: Time,
    options: Options,
    before: F,
) -> Result<Result<Receipt, Error>, E>
where
    F: FnOnce() -> Result<(), E>,
{
    let checked: Result<(), Error> = async {
        if transaction_id(&mut *tx).await? != prepared.transaction {
            return Err(Error::Stale);
        }
        let current = access::identity(&mut *tx, actor).await?;
        let earlier = &prepared.packet.identity;
        if current.kind != earlier.kind
            || current.id != earlier.id
            || current.owner != earlier.owner
            || current.thread != earlier.thread
        {
            return Err(Error::Denied);
        }
        Ok(())
    }
    .await;
    if let Err(error) = checked {
        return Ok(Err(error));
    }
    let TreeState::New {
        prepared: native, ..
    } = &prepared.packet.state
    else {
        return Ok(finish_tree(&mut *tx, actor, &prepared.packet, now, options, None).await);
    };
    if let Err(error) = sqlx::query("SAVEPOINT document_folder_prepared")
        .execute(&mut **tx)
        .await
    {
        return Ok(Err(error.into()));
    }
    let id = new_id();
    if let Err(error) = before() {
        if let Err(native) = sqlx::query("RELEASE SAVEPOINT document_folder_prepared")
            .execute(&mut **tx)
            .await
        {
            return Ok(Err(native.into()));
        }
        return Err(error);
    }
    let result = async {
        let applied = insert_folder(
            &mut *tx,
            &prepared.packet.request,
            native,
            &prepared.packet.identity,
            id,
            now,
        )
        .await?;
        finish_tree(
            &mut *tx,
            actor,
            &prepared.packet,
            now,
            options,
            Some(applied),
        )
        .await
    }
    .await;
    if result.is_err()
        && let Err(error) = sqlx::query("ROLLBACK TO SAVEPOINT document_folder_prepared")
            .execute(&mut **tx)
            .await
    {
        return Ok(Err(error.into()));
    }
    if let Err(error) = sqlx::query("RELEASE SAVEPOINT document_folder_prepared")
        .execute(&mut **tx)
        .await
    {
        return Ok(Err(error.into()));
    }
    Ok(result)
}

/// Native single-document copy, including its exact current material and header footprint.
/// This is a consumed transaction-bound packet, never permission or a replay capability.
pub struct DocumentCopying {
    packet: PreparedTree,
    transaction: String,
    material: Option<copying::PreparedCopy>,
    header: Option<super::TextCreationPlan>,
    id: Id,
}

/// Complete native copy header subjects before any source block/file material is prepared.
/// The caller must admit these facts and current ordinary gates; this packet grants no access.
pub struct DocumentCopySources {
    locked: LockedTree,
    transaction: Option<String>,
    id: Id,
}

impl DocumentCopySources {
    /// The actual existing destination directory.
    #[must_use]
    pub fn destination(&self) -> Option<super::Directory> {
        self.locked.request.to
    }
    /// Actual current source members, or the genuine empty original-read set of a completed replay.
    #[must_use]
    pub fn sources(&self) -> &[containers::EntryRow] {
        match &self.locked.state {
            LockedTreeState::New(prepared) => &prepared.entries,
            LockedTreeState::Replay { .. } => &[],
        }
    }
    /// A completed actor/request reads its saved outcome rather than new original material.
    #[must_use]
    pub fn is_replay(&self) -> bool {
        matches!(&self.locked.state, LockedTreeState::Replay { .. })
    }
}

/// Lock the native actor/request and header membership without loading current block/file bytes.
pub async fn prepare_document_copy_sources_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    copy: &super::TextCopy,
) -> Result<DocumentCopySources, Error> {
    if request.command != Command::Copy || !matches!(request.items.as_slice(), [Item::Document(_)])
    {
        return Err(Error::Invalid("document copy requires one text source"));
    }
    let locked = lock_tree(conn, actor, request, Some(copy)).await?;
    let transaction = if orch_db::queries::transaction_guard::in_transaction(conn) {
        Some(transaction_id(conn).await?)
    } else {
        None
    };
    Ok(DocumentCopySources {
        locked,
        transaction,
        id: copy.id,
    })
}

impl DocumentCopying {
    /// The native generated copy identity, including its durable replay identity.
    #[must_use]
    pub fn id(&self) -> Id {
        self.id
    }

    /// The actual existing destination directory selected by the native request.
    #[must_use]
    pub fn destination(&self) -> Option<super::Directory> {
        self.packet.request.to
    }

    /// Only a new copy reads original material; completed replay reads its own stored outcome.
    #[must_use]
    pub fn sources(&self) -> &[containers::EntryRow] {
        match &self.packet.state {
            TreeState::New { prepared, .. } => &prepared.entries,
            TreeState::Replay { .. } => &[],
        }
    }

    /// Whether the same actor/request already has its native durable outcome.
    #[must_use]
    pub fn is_replay(&self) -> bool {
        matches!(&self.packet.state, TreeState::Replay { .. })
    }

    /// Original native outcome ids remain factual even when a later purge removed their rows.
    #[must_use]
    pub fn replay_items(&self) -> &[Item] {
        match &self.packet.state {
            TreeState::Replay { items, .. } => items,
            TreeState::New { .. } => &[],
        }
    }
}

/// Complete the native source, destination, namespace, current material and header planning.
pub async fn prepare_document_copy_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    copy: &super::TextCopy,
    now: Time,
    options: Options,
) -> Result<DocumentCopying, Error> {
    let sources = prepare_document_copy_sources_in(conn, actor, request, copy).await?;
    prepare_document_copy_material_in(conn, actor, sources, now, options).await
}

/// Consume only the same locked subjects, after their consuming operation has admitted them.
pub async fn prepare_document_copy_material_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    sources: DocumentCopySources,
    now: Time,
    options: Options,
) -> Result<DocumentCopying, Error> {
    match (
        &sources.transaction,
        orch_db::queries::transaction_guard::in_transaction(conn),
    ) {
        (Some(original), true) if transaction_id(conn).await? == *original => {}
        (None, false) => {}
        _ => return Err(Error::Stale),
    }
    let current = access::identity(conn, actor).await?;
    let earlier = &sources.locked.identity;
    if current.kind != earlier.kind
        || current.id != earlier.id
        || current.owner != earlier.owner
        || current.thread != earlier.thread
    {
        return Err(Error::Denied);
    }
    let packet = complete_locked_tree(conn, actor, sources.locked, None, now).await?;
    let (material, header) = match &packet.state {
        TreeState::New { prepared, .. } => {
            let material =
                copying::prepare_copy(conn, actor, &packet.request, prepared, options).await?;
            let header = material.prepare_text_header(conn, actor, now).await?;
            (Some(material), Some(header))
        }
        TreeState::Replay { .. } => (None, None),
    };
    Ok(DocumentCopying {
        packet,
        transaction: transaction_id(conn).await?,
        material,
        header,
        id: sources.id,
    })
}

/// Repeat ordinary native copy/replay gates after the completed footprint, without replanning.
pub async fn require_document_copy_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    prepared: &DocumentCopying,
    now: Time,
) -> Result<(), Error> {
    let parent = prepared.packet.request.to.ok_or(Error::NotFound)?;
    let destination = access::require_create(conn, actor, parent, now).await?;
    match &prepared.packet.state {
        TreeState::New {
            prepared: native, ..
        } => {
            if Some(destination) != native.destination {
                return Err(Error::Stale);
            }
            access::require_entries(
                conn,
                actor,
                &native.entries,
                Permission::DocView,
                now,
                false,
                orch_authz::Act::Use,
            )
            .await?;
            create::require_text_creation_in(
                conn,
                actor,
                prepared.header.as_ref().ok_or(Error::Stale)?,
                now,
            )
            .await?;
        }
        TreeState::Replay { receipt, .. } => {
            require_replayed_entries(conn, actor, receipt, now).await?
        }
    }
    Ok(())
}

/// Apply the original header/material/audit/replay producer after its actual first-write cut.
/// A completed native replay never calls the opaque first-effect callback.
pub async fn apply_document_copy_in<E, F>(
    tx: &mut Tx,
    actor: Actor<'_>,
    prepared: DocumentCopying,
    now: Time,
    options: Options,
    before: F,
) -> Result<Result<Receipt, Error>, E>
where
    F: FnOnce() -> Result<(), E>,
{
    let checked: Result<(), Error> = async {
        if transaction_id(&mut *tx).await? != prepared.transaction {
            return Err(Error::Stale);
        }
        let current = access::identity(&mut *tx, actor).await?;
        let earlier = &prepared.packet.identity;
        if current.kind != earlier.kind
            || current.id != earlier.id
            || current.owner != earlier.owner
            || current.thread != earlier.thread
        {
            return Err(Error::Denied);
        }
        Ok(())
    }
    .await;
    if let Err(error) = checked {
        return Ok(Err(error));
    }
    if prepared.is_replay() {
        return Ok(finish_tree(&mut *tx, actor, &prepared.packet, now, options, None).await);
    }
    let (Some(material), Some(header)) = (prepared.material, prepared.header) else {
        return Ok(Err(Error::Stale));
    };
    if let Err(error) = sqlx::query("SAVEPOINT document_copy_prepared")
        .execute(&mut **tx)
        .await
    {
        return Ok(Err(error.into()));
    }
    let header = match create::apply_text_creation_in(tx, actor, header, now, before).await {
        Err(error) => {
            if let Err(native) = sqlx::query("RELEASE SAVEPOINT document_copy_prepared")
                .execute(&mut **tx)
                .await
            {
                return Ok(Err(native.into()));
            }
            return Err(error);
        }
        Ok(value) => value,
    };
    let result = async {
        let header = header?;
        if !header.created {
            return Err(Error::Conflict);
        }
        let applied =
            copying::apply_text_material(&mut *tx, actor, material, &header.document, now).await?;
        finish_tree(
            &mut *tx,
            actor,
            &prepared.packet,
            now,
            options,
            Some(applied),
        )
        .await
    }
    .await;
    if result.is_err()
        && let Err(error) = sqlx::query("ROLLBACK TO SAVEPOINT document_copy_prepared")
            .execute(&mut **tx)
            .await
    {
        return Ok(Err(error.into()));
    }
    if let Err(error) = sqlx::query("RELEASE SAVEPOINT document_copy_prepared")
        .execute(&mut **tx)
        .await
    {
        return Ok(Err(error.into()));
    }
    Ok(result)
}

async fn require_replayed_entries(
    conn: &mut Conn,
    actor: Actor<'_>,
    receipt: &Receipt,
    now: Time,
) -> Result<(), Error> {
    let rows = reread(
        conn,
        &receipt
            .entries
            .iter()
            .map(|entry| entry.item)
            .collect::<Vec<_>>(),
    )
    .await?;
    require_replayed_rows(conn, actor, &rows, now).await
}

async fn require_replayed_rows(
    conn: &mut Conn,
    actor: Actor<'_>,
    rows: &[containers::EntryRow],
    now: Time,
) -> Result<(), Error> {
    access::require_entries(
        conn,
        actor,
        rows,
        Permission::DocView,
        now,
        true,
        orch_authz::Act::Use,
    )
    .await?;
    let deleted: Vec<_> = rows
        .iter()
        .filter(|row| row.deleted_at.is_some())
        .cloned()
        .collect();
    access::require_entries(
        conn,
        actor,
        &deleted,
        Permission::DocManage,
        now,
        true,
        orch_authz::Act::Delete,
    )
    .await
}

/// Checked native movement of one stored text into a hierarchy node's docs root.
/// The packet is factual locked input, consumed on its exact original transaction.
pub struct DocumentMovement {
    packet: PreparedTree,
    transaction: String,
}
impl DocumentMovement {
    /// The actual single source selected by the native request.
    #[must_use]
    pub fn document(&self) -> Option<Id> {
        match self.packet.request.items.as_slice() {
            [Item::Document(id)] => Some(*id),
            _ => None,
        }
    }
    /// The actual native requested destination directory.
    #[must_use]
    pub fn destination(&self) -> Option<super::Directory> {
        self.packet.request.to
    }
    /// Whether this exact native request already completed and will not write again.
    #[must_use]
    pub fn is_replay(&self) -> bool {
        matches!(&self.packet.state, TreeState::Replay { .. })
    }
}

/// Preserves the original current tree scope/row/namespace/audience preparation.
/// A new no-op is handled by the API's original no-op branch, not a false first-effect check.
pub async fn prepare_document_movement_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    now: Time,
) -> Result<DocumentMovement, Error> {
    if request.command != Command::Move
        || !matches!(request.items.as_slice(), [Item::Document(_)])
        || !matches!(request.to, Some(super::Directory::NodeDocs(_)))
        || request.name.is_some()
        || request.version.is_some()
    {
        return Err(Error::Invalid(
            "this preparation requires one text move into a node directory",
        ));
    }
    let packet = prepare_tree(conn, actor, request, None, now, None).await?;
    if let TreeState::New { prepared, .. } = &packet.state {
        let (node, _, parent) = prepared.destination.ok_or(Error::NotFound)?;
        let changes = prepared
            .entries
            .iter()
            .any(|row| row.node_id != node || row.parent_container_id != parent);
        if !changes {
            return Err(Error::Invalid(
                "this preparation requires an actual document movement",
            ));
        }
        // The same writer clears existing readme references after its document UPDATE.
        // Its source document lock also fences new foreign-key references to that id.
        let document = request.items[0].id();
        sqlx::query(
            "SELECT id FROM nodes WHERE readme_document_id=$1 AND id<>$2 ORDER BY id FOR UPDATE",
        )
        .bind(document)
        .bind(node)
        .fetch_all(&mut *conn)
        .await?;
    }
    Ok(DocumentMovement {
        packet,
        transaction: transaction_id(conn).await?,
    })
}

/// Applies the same native move/audience/audit/remember algorithm once. The opaque
/// caller check runs immediately before its first UPDATE; a durable replay skips it.
pub async fn apply_document_movement_in<E, F>(
    tx: &mut Tx,
    actor: Actor<'_>,
    prepared: DocumentMovement,
    now: Time,
    options: Options,
    before: F,
) -> Result<Result<Receipt, Error>, E>
where
    F: FnOnce() -> Result<(), E>,
{
    let checked: Result<(), Error> = async {
        if transaction_id(&mut *tx).await? != prepared.transaction {
            return Err(Error::Stale);
        }
        let current = access::identity(&mut *tx, actor).await?;
        let earlier = &prepared.packet.identity;
        if current.kind != earlier.kind
            || current.id != earlier.id
            || current.owner != earlier.owner
            || current.thread != earlier.thread
        {
            return Err(Error::Denied);
        }
        Ok(())
    }
    .await;
    if let Err(error) = checked {
        return Ok(Err(error));
    }
    let TreeState::New {
        prepared: native, ..
    } = &prepared.packet.state
    else {
        return Ok(finish_tree(&mut *tx, actor, &prepared.packet, now, options, None).await);
    };
    if let Err(error) = sqlx::query("SAVEPOINT document_move_prepared")
        .execute(&mut **tx)
        .await
    {
        return Ok(Err(error.into()));
    }
    if let Err(error) = before() {
        if let Err(native) = sqlx::query("RELEASE SAVEPOINT document_move_prepared")
            .execute(&mut **tx)
            .await
        {
            return Ok(Err(native.into()));
        }
        return Err(error);
    }
    let result = async {
        let moved = move_entries_with_identity(
            &mut *tx,
            &prepared.packet.identity,
            &prepared.packet.request,
            native,
            now,
        )
        .await?;
        finish_tree(
            &mut *tx,
            actor,
            &prepared.packet,
            now,
            options,
            Some((moved, Vec::new())),
        )
        .await
    }
    .await;
    if result.is_err()
        && let Err(error) = sqlx::query("ROLLBACK TO SAVEPOINT document_move_prepared")
            .execute(&mut **tx)
            .await
    {
        return Ok(Err(error.into()));
    }
    if let Err(error) = sqlx::query("RELEASE SAVEPOINT document_move_prepared")
        .execute(&mut **tx)
        .await
    {
        return Ok(Err(error.into()));
    }
    Ok(result)
}

/// One checked restoration into a deleted text's actual recorded directory.
pub struct DocumentRestoration {
    packet: PreparedTree,
    transaction: String,
    restoring: Vec<containers::EntryRow>,
    until: Option<chrono::DateTime<chrono::Utc>>,
}
impl DocumentRestoration {
    /// The actual single deleted text selected by this native request.
    #[must_use]
    pub fn document(&self) -> Option<Id> {
        match self.packet.request.items.as_slice() {
            [Item::Document(id)] => Some(*id),
            _ => None,
        }
    }
    /// The original retained promise, checked before its first restore update.
    #[must_use]
    pub fn until(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.until
    }
    /// Whether the original native request already completed and must not write again.
    #[must_use]
    pub fn is_replay(&self) -> bool {
        matches!(&self.packet.state, TreeState::Replay { .. })
    }
}
/// The original retained authority/parent/namespace/deadline preparation for one text restoration.
pub async fn prepare_document_restoration_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    now: Time,
    options: Options,
) -> Result<DocumentRestoration, Error> {
    if request.command != Command::Restore
        || !matches!(request.items.as_slice(), [Item::Document(_)])
        || request.to.is_some()
        || request.name.is_some()
        || request.version.is_some()
    {
        return Err(Error::Invalid(
            "this preparation requires one text restoration in its recorded directory",
        ));
    }
    let packet = prepare_tree(conn, actor, request, None, now, None).await?;
    let (restoring, until) = match &packet.state {
        TreeState::New { prepared, .. } => {
            restoring_selection(conn, prepared, now, options).await?
        }
        TreeState::Replay { .. } => (Vec::new(), None),
    };
    if matches!(&packet.state, TreeState::New { .. }) && restoring.is_empty() {
        return Err(Error::NotFound);
    }
    Ok(DocumentRestoration {
        packet,
        transaction: transaction_id(conn).await?,
        restoring,
        until,
    })
}
/// The same native restore/audience/audit/remember writer. Its opaque caller check
/// runs immediately before clearing deletion, and replay skips a second update.
pub async fn apply_document_restoration_in<E, F>(
    tx: &mut Tx,
    actor: Actor<'_>,
    prepared: DocumentRestoration,
    now: Time,
    options: Options,
    before: F,
) -> Result<Result<Receipt, Error>, E>
where
    F: FnOnce() -> Result<(), E>,
{
    let checked: Result<(), Error> = async {
        if transaction_id(&mut *tx).await? != prepared.transaction {
            return Err(Error::Stale);
        }
        let current = access::identity(&mut *tx, actor).await?;
        let earlier = &prepared.packet.identity;
        if current.kind != earlier.kind
            || current.id != earlier.id
            || current.owner != earlier.owner
            || current.thread != earlier.thread
        {
            return Err(Error::Denied);
        }
        if prepared.until.is_some_and(|until| until <= now.get()) {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    .await;
    if let Err(error) = checked {
        return Ok(Err(error));
    }
    if matches!(&prepared.packet.state, TreeState::Replay { .. }) {
        return Ok(finish_tree(&mut *tx, actor, &prepared.packet, now, options, None).await);
    }
    if let Err(error) = sqlx::query("SAVEPOINT document_restore_prepared")
        .execute(&mut **tx)
        .await
    {
        return Ok(Err(error.into()));
    }
    if let Err(error) = before() {
        if let Err(native) = sqlx::query("RELEASE SAVEPOINT document_restore_prepared")
            .execute(&mut **tx)
            .await
        {
            return Ok(Err(native.into()));
        }
        return Err(error);
    }
    let result = async {
        let restored = restore_selected(&mut *tx, prepared.restoring, now).await?;
        finish_tree(
            &mut *tx,
            actor,
            &prepared.packet,
            now,
            options,
            Some((restored, Vec::new())),
        )
        .await
    }
    .await;
    if result.is_err()
        && let Err(error) = sqlx::query("ROLLBACK TO SAVEPOINT document_restore_prepared")
            .execute(&mut **tx)
            .await
    {
        return Ok(Err(error.into()));
    }
    if let Err(error) = sqlx::query("RELEASE SAVEPOINT document_restore_prepared")
        .execute(&mut **tx)
        .await
    {
        return Ok(Err(error.into()));
    }
    Ok(result)
}

enum LockedTreeState {
    Replay { receipt: Receipt, items: Vec<Item> },
    New(prepare::Prepared),
}

struct LockedTree {
    request: Request,
    identity: access::Identity,
    arguments: orch_types::Json,
    state: LockedTreeState,
}

async fn prepare_tree(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    deletion_plan: Option<super::DeletionConfirmation<'_>>,
    now: Time,
    text_copy: Option<&super::TextCopy>,
) -> Result<PreparedTree, Error> {
    prepare::validate(request)?;
    if (request.command == Command::Delete) != deletion_plan.is_some() {
        return Err(Error::Invalid(
            "only deletion requires its complete confirmation plan",
        ));
    }
    let sources = sources::prepare_with_copy(conn, actor, request, text_copy).await?;
    sources::check_association(conn, actor, &sources).await?;
    complete_locked_tree(conn, actor, sources.locked, deletion_plan, now).await
}

/// The native actor/request and complete header footprint, before private material or audience.
async fn lock_tree(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    text_copy: Option<&super::TextCopy>,
) -> Result<LockedTree, Error> {
    prepare::validate(request)?;
    let identity = access::identity(conn, actor).await?;
    let arguments = if let Some(copy) = text_copy {
        json!({"request":request,"text_copy":copy})
    } else {
        serde_json::to_value(request)?
    };
    orch_db::queries::chats::lock_named(
        conn,
        &format!(
            "document.tree:{}:{}:{}",
            identity.kind, identity.id, request.request_ref
        ),
    )
    .await?;
    if let Some(saved) =
        containers::operation(conn, identity.kind, identity.id, &request.request_ref).await?
    {
        if saved.arguments != arguments {
            return Err(Error::Conflict);
        }
        let receipt: Receipt = serde_json::from_value(saved.result)?;
        let items = receipt.entries.iter().map(|entry| entry.item).collect();
        return Ok(LockedTree {
            request: request.clone(),
            identity,
            arguments,
            state: LockedTreeState::Replay { receipt, items },
        });
    }
    let mut effective = request.clone();
    if let Some(copy) = text_copy {
        if request.command != Command::Copy
            || !matches!(request.items.as_slice(), [Item::Document(_)])
            || request.name.is_some()
            || !matches!(actor, Actor::Person(_))
        {
            return Err(Error::Invalid(
                "ordinary text-copy options take one document and a person",
            ));
        }
        if rows::get(conn, copy.id).await?.is_some() {
            return Err(Error::Conflict);
        }
        if let Some(title) = copy.title.as_deref() {
            effective.name = Some(create::basename(title.trim()));
        }
    }
    let request = &effective;
    let mut prepared = prepare::lock(conn, request).await?;
    prepared.text_copy = text_copy.cloned();
    Ok(LockedTree {
        request: request.clone(),
        identity,
        arguments,
        state: LockedTreeState::New(prepared),
    })
}

async fn complete_locked_tree(
    conn: &mut Conn,
    actor: Actor<'_>,
    locked: LockedTree,
    deletion_plan: Option<super::DeletionConfirmation<'_>>,
    now: Time,
) -> Result<PreparedTree, Error> {
    let LockedTree {
        request,
        identity,
        arguments,
        state,
    } = locked;
    let state = match state {
        LockedTreeState::Replay { mut receipt, items } => {
            let rows = reread(
                conn,
                &receipt
                    .entries
                    .iter()
                    .map(|entry| entry.item)
                    .collect::<Vec<_>>(),
            )
            .await?;
            require_replayed_rows(conn, actor, &rows, now).await?;
            receipt.entries = rows.iter().map(access::project).collect();
            TreeState::Replay { receipt, items }
        }
        LockedTreeState::New(prepared) => {
            prepare::require(conn, actor, &request, &prepared, now).await?;
            if let Some(expected) = deletion_plan {
                let actual = prepare::stamp(conn, &prepared.entries, now).await?;
                if !expected.matches(&actual, now)? {
                    return Err(Error::Stale);
                }
            }
            namespace(conn, &request, &prepared).await?;
            let before = audience::read(conn, actor, &prepared.entries, now).await?;
            TreeState::New {
                prepared,
                before,
                operation: new_id(),
            }
        }
    };
    Ok(PreparedTree {
        request,
        identity,
        arguments,
        state,
    })
}

async fn finish_tree(
    conn: &mut Conn,
    actor: Actor<'_>,
    packet: &PreparedTree,
    now: Time,
    options: Options,
    applied: Option<(Vec<Item>, Vec<super::Copied>)>,
) -> Result<Receipt, Error> {
    finish_tree_checked(
        conn,
        actor,
        packet,
        now,
        options,
        applied,
        &mut Checkpoint::native(),
    )
    .await
}

async fn finish_tree_checked(
    conn: &mut Conn,
    actor: Actor<'_>,
    packet: &PreparedTree,
    now: Time,
    options: Options,
    applied: Option<(Vec<Item>, Vec<super::Copied>)>,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Receipt, Error> {
    let (prepared, before, operation) = match &packet.state {
        TreeState::Replay { receipt, .. } => return Ok(receipt.clone()),
        TreeState::New {
            prepared,
            before,
            operation,
        } => (prepared, before, operation),
    };
    let request = &packet.request;
    let identity = &packet.identity;
    let arguments = packet.arguments.clone();
    let operation = *operation;
    let (items, copied) = applied.ok_or(Error::Invalid(
        "new tree effects require their prepared native operands",
    ))?;
    let changed = reread(conn, &items).await?;
    let until = if request.command == Command::Delete {
        Some(prepare::deadline(now, options)?)
    } else {
        None
    };
    // Each name is registered as written here, so that the catalogue's check finds this writer.
    let action = match request.command {
        Command::CreateFolder | Command::Copy => {
            if identity.thread.is_some() {
                orch_db::audit::name!("document.created_by_agent")
            } else {
                orch_db::audit::name!("document.created")
            }
        }
        Command::Move => orch_db::audit::name!("document.moved"),
        Command::Delete => orch_db::audit::name!("document.deleted"),
        Command::Restore => orch_db::audit::name!("document.restored"),
        Command::Rename => orch_db::audit::name!("document.renamed"),
        Command::RestoreVersion => orch_db::audit::name!("document.version_restored"),
    };
    for row in &changed {
        let source = copied
            .iter()
            .find(|copy| copy.destination == access::item(row))
            .map(|copy| copy.source);
        let action = if request.command == Command::Restore && row.deleted_at.is_some() {
            orch_db::audit::name!("document.moved")
        } else {
            action
        };
        let from_node = prepared
            .entries
            .iter()
            .find(|before| before.id == row.id && before.node_id != row.node_id)
            .map(|before| before.node_id);
        record(
            conn,
            actor,
            AuditChange {
                row,
                operation,
                action,
                source,
                until,
                from_node,
            },
            now,
            checkpoint,
        )
        .await?;
    }
    let after = audience::read(conn, actor, &changed, now).await?;
    let (gaining, losing) = audience::difference(before, &after, &copied);
    let mut nodes: HashSet<_> = prepared.entries.iter().map(|row| row.node_id).collect();
    nodes.extend(changed.iter().map(|row| row.node_id));
    for node in nodes {
        checkpoint.check()?;
        orch_db::queries::hierarchy::stamp(conn, node).await?;
        orch_db::queries::hierarchy::touch_activity(conn, node, now.get()).await?;
    }
    if !matches!(request.command, Command::Rename | Command::RestoreVersion) {
        checkpoint.check()?;
        orch_authz::epoch::bump(conn).await?;
    }
    let receipt = Receipt {
        operation,
        entries: changed.iter().map(access::project).collect(),
        copied,
        gaining,
        losing,
        restore_until: until,
    };
    checkpoint.check()?;
    containers::remember(
        conn,
        identity.kind,
        identity.id,
        &request.request_ref,
        &containers::Operation {
            arguments,
            operation_id: operation,
            result: serde_json::to_value(&receipt)?,
        },
        now.get(),
    )
    .await?;
    Ok(receipt)
}

fn execute(
    tx: &mut Tx,
    actor: Actor<'_>,
    request: &Request,
    deletion_plan: Option<super::DeletionConfirmation<'_>>,
    now: Time,
    options: Options,
    text_copy: Option<&super::TextCopy>,
) -> impl std::future::Future<Output = Result<Receipt, Error>> {
    // The operation lives on the heap beneath both savepoint facades.
    Box::pin(execute_inner(
        tx,
        actor,
        request,
        deletion_plan,
        now,
        options,
        text_copy,
    ))
}

async fn execute_inner(
    tx: &mut Tx,
    actor: Actor<'_>,
    request: &Request,
    deletion_plan: Option<super::DeletionConfirmation<'_>>,
    now: Time,
    options: Options,
    text_copy: Option<&super::TextCopy>,
) -> Result<Receipt, Error> {
    prepare::validate(request)?;
    if (request.command == Command::Delete) != deletion_plan.is_some() {
        return Err(Error::Invalid(
            "only deletion requires its complete confirmation plan",
        ));
    }
    let sources = sources::prepare_with_copy(&mut *tx, actor, request, text_copy).await?;
    let prepared =
        complete_operation_in(&mut *tx, actor, sources, deletion_plan, now, options).await?;
    staged::apply_on(
        &mut *tx,
        actor,
        prepared,
        now,
        options,
        &mut Checkpoint::native(),
    )
    .await
}

/// Executes one complete operation inside the supplied transaction, rolling back its own work on any refusal.
pub async fn execute_in(
    tx: &mut Tx,
    actor: Actor<'_>,
    request: &Request,
    deletion_plan: Option<&MembershipStamp>,
    now: Time,
    options: Options,
) -> Result<Receipt, Error> {
    execute_confirmed_in(
        tx,
        actor,
        request,
        deletion_plan.map(super::DeletionConfirmation::Membership),
        now,
        options,
    )
    .await
}

/// Execute with either representation of the exact native confirmation, through the same writer.
pub async fn execute_confirmed_in(
    tx: &mut Tx,
    actor: Actor<'_>,
    request: &Request,
    deletion_plan: Option<super::DeletionConfirmation<'_>>,
    now: Time,
    options: Options,
) -> Result<Receipt, Error> {
    sqlx::query("SAVEPOINT document_tree_operation")
        .execute(&mut **tx)
        .await?;
    let result = execute(tx, actor, request, deletion_plan, now, options, None).await;
    if result.is_err() {
        sqlx::query("ROLLBACK TO SAVEPOINT document_tree_operation")
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("RELEASE SAVEPOINT document_tree_operation")
        .execute(&mut **tx)
        .await?;
    result
}

/// The existing single-text API copy, through exactly the recursive copy/authority/replay producer.
pub async fn copy_text_in(
    tx: &mut Tx,
    actor: Actor<'_>,
    request: &Request,
    copy: &super::TextCopy,
    now: Time,
    options: Options,
) -> Result<Receipt, Error> {
    sqlx::query("SAVEPOINT document_tree_text_copy")
        .execute(&mut **tx)
        .await?;
    let result = async {
        let prepared =
            prepare_document_copy_in(&mut *tx, actor, request, copy, now, options).await?;
        match apply_document_copy_in(tx, actor, prepared, now, options, || {
            Ok::<(), std::convert::Infallible>(())
        })
        .await
        {
            Ok(result) => result,
            Err(never) => match never {},
        }
    }
    .await;
    if result.is_err() {
        sqlx::query("ROLLBACK TO SAVEPOINT document_tree_text_copy")
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("RELEASE SAVEPOINT document_tree_text_copy")
        .execute(&mut **tx)
        .await?;
    result
}
