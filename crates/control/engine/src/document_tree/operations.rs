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

async fn move_entries(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    prepared: &prepare::Prepared,
    now: Time,
) -> Result<Vec<Item>, Error> {
    let (node, root, parent) = prepared.destination.ok_or(Error::NotFound)?;
    let selected: HashSet<_> = prepared.roots.iter().map(access::item).collect();
    let identity = access::identity(conn, actor).await?;
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
    actor: Actor<'_>,
    prepared: &prepare::Prepared,
    operation: Id,
    now: Time,
) -> Result<Vec<Item>, Error> {
    let identity = access::identity(conn, actor).await?;
    let by = identity.thread.is_none().then_some(identity.owner);
    let mut out = Vec::new();
    for row in &prepared.entries {
        if row.deleted_at.is_none() {
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

async fn restore_entries(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    prepared: &prepare::Prepared,
    now: Time,
    options: Options,
) -> Result<Vec<Item>, Error> {
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
    for row in &restoring {
        let until = super::restore_deadline_in(conn, &access::project(row), options)
            .await?
            .ok_or(Error::Conflict)?;
        if until <= now.get() {
            return Err(Error::Conflict);
        }
    }
    // Reparent the still-deleted cohort first. The old parent can be deleted when the
    // caller explicitly restores elsewhere; no intermediate live orphan is admitted.
    if request.to.is_some() {
        let moved = prepare::Prepared {
            roots: prepared.roots.clone(),
            entries: prepared.entries.clone(),
            destination: prepared.destination,
            text_copy: None,
        };
        move_entries(conn, actor, request, &moved, now).await?;
    }
    let mut out = Vec::new();
    for row in restoring {
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
    if request.to.is_some() {
        Ok(prepared.entries.iter().map(access::item).collect())
    } else {
        Ok(out)
    }
}

async fn apply(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    prepared: &prepare::Prepared,
    operation: Id,
    now: Time,
    options: Options,
) -> Result<(Vec<Item>, Vec<super::Copied>), Error> {
    match request.command {
        Command::CreateFolder => {
            let (node, root, parent) = prepared.destination.ok_or(Error::NotFound)?;
            let identity = access::identity(conn, actor).await?;
            let id = new_id();
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
            create::creation_grant(conn, id, true, &identity, now).await?;
            Ok((vec![Item::Container(id)], Vec::new()))
        }
        Command::Copy => copying::copy(conn, actor, request, prepared, now, options).await,
        Command::Move => Ok((
            move_entries(conn, actor, request, prepared, now).await?,
            Vec::new(),
        )),
        Command::Delete => Ok((
            delete_entries(conn, actor, prepared, operation, now).await?,
            Vec::new(),
        )),
        Command::Restore => Ok((
            restore_entries(conn, actor, request, prepared, now, options).await?,
            Vec::new(),
        )),
        Command::Rename => {
            let row = prepared.roots.first().ok_or(Error::NotFound)?;
            let identity = access::identity(conn, actor).await?;
            let writer = identity
                .thread
                .zip(identity.agent)
                .map_or(Writer::User(identity.owner), |(thread, agent)| {
                    Writer::Agent { thread, agent }
                });
            rows::rename(
                conn,
                row.id,
                request.name.as_deref().unwrap_or_default().trim(),
                &writer,
                now.get(),
            )
            .await?;
            Ok((vec![Item::Document(row.id)], Vec::new()))
        }
        Command::RestoreVersion => {
            let row = prepared.roots.first().ok_or(Error::NotFound)?;
            let identity = access::identity(conn, actor).await?;
            let writer = identity
                .thread
                .zip(identity.agent)
                .map_or(Writer::User(identity.owner), |(thread, agent)| {
                    Writer::Agent { thread, agent }
                });
            let mut settings = crate::tools::edit::settings_at(conn, row.node_id).await?;
            settings.rules.window_secs = options.version_window_secs;
            match batch::restore(
                conn,
                row.id,
                request
                    .version
                    .ok_or(Error::Invalid("give a document version"))?,
                &writer,
                settings.limits,
                settings.rules,
                now.get(),
            )
            .await?
            {
                batch::Restored::Done(_) => {}
                batch::Restored::Gone | batch::Restored::NoSuchVersion => {
                    return Err(Error::NotFound);
                }
                _ => return Err(Error::Limit),
            }
            Ok((vec![Item::Document(row.id)], Vec::new()))
        }
    }
}

async fn execute(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    deletion_plan: Option<&MembershipStamp>,
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
        let mut receipt: Receipt = serde_json::from_value(saved.result)?;
        let rows = reread(
            conn,
            &receipt
                .entries
                .iter()
                .map(|entry| entry.item)
                .collect::<Vec<_>>(),
        )
        .await?;
        access::require_entries(
            conn,
            actor,
            &rows,
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
        .await?;
        receipt.entries = rows.iter().map(access::project).collect();
        return Ok(receipt);
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
    let mut prepared = prepare::prepare(conn, actor, request, now).await?;
    prepared.text_copy = text_copy.cloned();
    if let Some(expected) = deletion_plan
        && (expected.authority_until.is_some_and(|at| at <= now.get())
            || *expected != prepare::stamp(conn, &prepared.entries, now).await?)
    {
        return Err(Error::Stale);
    }
    namespace(conn, request, &prepared).await?;
    let before = audience::read(conn, actor, &prepared.entries, now).await?;
    let operation = new_id();
    let (items, copied) = apply(conn, actor, request, &prepared, operation, now, options).await?;
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
        )
        .await?;
    }
    let after = audience::read(conn, actor, &changed, now).await?;
    let (gaining, losing) = audience::difference(&before, &after, &copied);
    let mut nodes: HashSet<_> = prepared.entries.iter().map(|row| row.node_id).collect();
    nodes.extend(changed.iter().map(|row| row.node_id));
    for node in nodes {
        orch_db::queries::hierarchy::stamp(conn, node).await?;
        orch_db::queries::hierarchy::touch_activity(conn, node, now.get()).await?;
    }
    if !matches!(request.command, Command::Rename | Command::RestoreVersion) {
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

/// Executes one complete operation inside the supplied transaction, rolling back its own work on any refusal.
pub async fn execute_in(
    tx: &mut Tx,
    actor: Actor<'_>,
    request: &Request,
    deletion_plan: Option<&MembershipStamp>,
    now: Time,
    options: Options,
) -> Result<Receipt, Error> {
    sqlx::query("SAVEPOINT document_tree_operation")
        .execute(&mut **tx)
        .await?;
    let result = execute(&mut *tx, actor, request, deletion_plan, now, options, None).await;
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
    let result = execute(&mut *tx, actor, request, None, now, options, Some(copy)).await;
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
