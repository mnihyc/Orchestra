//! One native prepared dispatcher for all tree commands, with the existing writers and replay.

use super::*;
use crate::EngineError;
use crate::document_tree::{DeletionConfirmation, DeletionDigest, OperationSources};
use futures::future::BoxFuture;

/// Fully prepared native operands, private to their original actor and transaction.
/// Source admission belongs to the caller; this packet grants no permission.
pub struct Operation {
    packet: PreparedTree,
    material: Material,
    transaction: Option<String>,
    confirmation: Option<DeletionDigest>,
}
enum Material {
    Replay,
    Folder(Id),
    Copy(Box<copying::PreparedCopy>),
    Move,
    Delete,
    Restore {
        rows: Vec<containers::EntryRow>,
        until: Option<chrono::DateTime<chrono::Utc>>,
    },
    Rename {
        document: Id,
        title: String,
        writer: Writer,
    },
    Version {
        document: Id,
        prepared: batch::Restoration,
    },
}
impl Operation {
    /// The command selected by the native validated request.
    #[must_use]
    pub fn command(&self) -> Command {
        self.packet.request.command
    }
    /// Native existing inputs that contributed to the immutable audience/copy outcome.
    /// Durable replay uses the saved native receipt association, never a newly expanded tree.
    #[must_use]
    pub fn context_items(&self) -> Vec<Item> {
        let items = match &self.packet.state {
            TreeState::New { prepared, .. } => prepared
                .entries
                .iter()
                .map(access::item)
                .collect::<Vec<_>>(),
            TreeState::Replay { receipt, .. } => receipt
                .entries
                .iter()
                .map(|entry| entry.item)
                .chain(receipt.copied.iter().map(|copy| copy.source))
                .collect(),
        };
        items
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    /// True only when this exact actor/request already has its durable result.
    #[must_use]
    pub fn is_replay(&self) -> bool {
        matches!(&self.packet.state, TreeState::Replay { .. })
    }
}

fn writer(identity: &access::Identity) -> Writer {
    match (identity.thread, identity.agent.clone()) {
        (Some(thread), Some(agent)) => Writer::Agent { thread, agent },
        _ => Writer::User(identity.owner),
    }
}
async fn readme_footprint(conn: &mut Conn, prepared: &prepare::Prepared) -> Result<(), Error> {
    let Some((node, _, _)) = prepared.destination else {
        return Ok(());
    };
    let documents: Vec<_> = prepared
        .entries
        .iter()
        .filter(|row| row.kind == "document")
        .map(|row| row.id)
        .collect();
    if !documents.is_empty() {
        sqlx::query("SELECT id FROM nodes WHERE readme_document_id=ANY($1) AND id<>$2 ORDER BY id FOR UPDATE")
            .bind(&documents).bind(node).fetch_all(conn).await?;
    }
    Ok(())
}

/// Consume exactly the locked header stage after its caller admits every required source.
/// Completes original authority/confirmation/namespace/audience and native material once per preparation.
pub fn complete_operation_in<'a>(
    conn: &'a mut Conn,
    actor: Actor<'a>,
    sources: OperationSources,
    confirmation: Option<DeletionConfirmation<'a>>,
    now: Time,
    options: Options,
) -> BoxFuture<'a, Result<Operation, Error>> {
    Box::pin(async move {
        if (sources.command() == Command::Delete) != confirmation.is_some() {
            return Err(Error::Invalid(
                "only deletion requires its complete confirmation plan",
            ));
        }
        sources::check_association(conn, actor, &sources).await?;
        let digest = match confirmation {
            Some(DeletionConfirmation::Membership(stamp)) => Some(stamp.deletion_digest()?),
            Some(DeletionConfirmation::Digest(digest)) => Some(*digest),
            None => None,
        };
        let transaction = sources.transaction.clone();
        let packet = complete_locked_tree(conn, actor, sources.locked, confirmation, now).await?;
        let material = match &packet.state {
            TreeState::Replay { .. } => Material::Replay,
            TreeState::New { prepared, .. } => match packet.request.command {
                Command::CreateFolder => Material::Folder(new_id()),
                Command::Copy => {
                    let mut material =
                        copying::prepare_copy(conn, actor, &packet.request, prepared, options)
                            .await?;
                    material.prepare_first_header(conn, actor, now).await?;
                    Material::Copy(Box::new(material))
                }
                Command::Move => {
                    readme_footprint(conn, prepared).await?;
                    Material::Move
                }
                Command::Delete => Material::Delete,
                Command::Restore => {
                    let (rows, until) = restoring_selection(conn, prepared, now, options).await?;
                    if packet.request.to.is_some() {
                        readme_footprint(conn, prepared).await?;
                    }
                    Material::Restore { rows, until }
                }
                Command::Rename => {
                    let row = prepared.roots.first().ok_or(Error::NotFound)?;
                    Material::Rename {
                        document: row.id,
                        title: packet
                            .request
                            .name
                            .as_deref()
                            .unwrap_or_default()
                            .trim()
                            .to_owned(),
                        writer: writer(&packet.identity),
                    }
                }
                Command::RestoreVersion => {
                    let row = prepared.roots.first().ok_or(Error::NotFound)?;
                    let mut settings = crate::tools::edit::settings_at(conn, row.node_id).await?;
                    settings.rules.window_secs = options.version_window_secs;
                    let material = batch::prepare_restoration(
                        conn,
                        row.id,
                        packet
                            .request
                            .version
                            .ok_or(Error::Invalid("give a document version"))?,
                        &writer(&packet.identity),
                        settings.limits,
                        settings.rules,
                    )
                    .await?;
                    Material::Version {
                        document: row.id,
                        prepared: material,
                    }
                }
            },
        };
        Ok(Operation {
            packet,
            material,
            transaction,
            confirmation: digest,
        })
    })
}

/// The same original native gates, with the actual current principal/time, never a new material plan.
pub async fn require_operation_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    prepared: &Operation,
    now: Time,
) -> Result<(), Error> {
    match (
        &prepared.transaction,
        orch_db::queries::transaction_guard::in_transaction(conn),
    ) {
        (Some(original), true) if transaction_id(conn).await? == *original => {}
        (None, false) => {}
        _ => return Err(Error::Stale),
    }
    let current = access::identity(conn, actor).await?;
    let earlier = &prepared.packet.identity;
    if current.kind != earlier.kind
        || current.id != earlier.id
        || current.owner != earlier.owner
        || current.thread != earlier.thread
    {
        return Err(Error::Denied);
    }
    match &prepared.packet.state {
        TreeState::Replay { receipt, .. } => {
            require_replayed_entries(conn, actor, receipt, now).await?
        }
        TreeState::New {
            prepared: native, ..
        } => {
            prepare::require(conn, actor, &prepared.packet.request, native, now).await?;
            if let Some(digest) = &prepared.confirmation {
                let actual = prepare::stamp(conn, &native.entries, now).await?;
                if !DeletionConfirmation::Digest(digest).matches(&actual, now)? {
                    return Err(Error::Stale);
                }
            }
        }
    }
    if let Material::Restore {
        until: Some(until), ..
    } = &prepared.material
        && *until <= now.get()
    {
        return Err(Error::Conflict);
    }
    Ok(())
}

/// Apply on the real supplied transaction; consume the original check only at its first mutation.
/// The savepoint restores every partial native effect on ordinary refusal or caller closure.
pub async fn apply_operation_in<F>(
    tx: &mut Tx,
    actor: Actor<'_>,
    prepared: Operation,
    now: Time,
    options: Options,
    before: F,
) -> Result<Receipt, Error>
where
    F: FnOnce() -> Result<(), EngineError> + Send,
{
    sqlx::query("SAVEPOINT document_tree_prepared")
        .execute(&mut **tx)
        .await?;
    let mut checkpoint = Checkpoint::new(before);
    let result = apply_on(&mut *tx, actor, prepared, now, options, &mut checkpoint).await;
    if result.is_err() {
        sqlx::query("ROLLBACK TO SAVEPOINT document_tree_prepared")
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("RELEASE SAVEPOINT document_tree_prepared")
        .execute(&mut **tx)
        .await?;
    result
}

pub(super) fn apply_on<'a, 'check: 'a>(
    conn: &'a mut Conn,
    actor: Actor<'a>,
    prepared: Operation,
    now: Time,
    options: Options,
    checkpoint: &'a mut Checkpoint<'check>,
) -> BoxFuture<'a, Result<Receipt, Error>> {
    Box::pin(async move {
        if prepared.transaction.is_none()
            || !orch_db::queries::transaction_guard::in_transaction(conn)
        {
            return Err(Error::Stale);
        }
        require_operation_in(conn, actor, &prepared, now).await?;
        let Operation {
            packet, material, ..
        } = prepared;
        if matches!(&material, Material::Replay) {
            return finish_tree_checked(conn, actor, &packet, now, options, None, checkpoint).await;
        }
        let TreeState::New {
            prepared: native,
            operation,
            ..
        } = &packet.state
        else {
            return Err(Error::Stale);
        };
        let applied = match material {
            Material::Replay => return Err(Error::Stale),
            Material::Folder(id) => {
                insert_folder_checked(
                    conn,
                    &packet.request,
                    native,
                    &packet.identity,
                    id,
                    now,
                    checkpoint,
                )
                .await?
            }
            Material::Copy(material) => {
                copying::apply_copy(conn, actor, *material, now, checkpoint).await?
            }
            Material::Move => (
                move_entries_checked(
                    conn,
                    &packet.identity,
                    &packet.request,
                    native,
                    now,
                    checkpoint,
                )
                .await?,
                Vec::new(),
            ),
            Material::Delete => (
                delete_entries(conn, &packet.identity, native, *operation, now, checkpoint).await?,
                Vec::new(),
            ),
            Material::Restore { rows, .. } => {
                if packet.request.to.is_some() {
                    move_entries_checked(
                        conn,
                        &packet.identity,
                        &packet.request,
                        native,
                        now,
                        checkpoint,
                    )
                    .await?;
                }
                let restored = restore_selected_checked(conn, rows, now, checkpoint).await?;
                (
                    if packet.request.to.is_some() {
                        native.entries.iter().map(access::item).collect()
                    } else {
                        restored
                    },
                    Vec::new(),
                )
            }
            Material::Rename {
                document,
                title,
                writer,
            } => {
                rows::rename_checked(conn, document, &title, &writer, now.get(), || {
                    checkpoint.check_engine()
                })
                .await??
                .ok_or(Error::NotFound)?;
                (vec![Item::Document(document)], Vec::new())
            }
            Material::Version { document, prepared } => {
                match batch::apply_restoration(conn, prepared, now.get(), || {
                    checkpoint.check_engine()
                })
                .await??
                {
                    batch::Restored::Done(_) => {}
                    batch::Restored::Gone | batch::Restored::NoSuchVersion => {
                        return Err(Error::NotFound);
                    }
                    _ => return Err(Error::Limit),
                }
                (vec![Item::Document(document)], Vec::new())
            }
        };
        finish_tree_checked(
            conn,
            actor,
            &packet,
            now,
            options,
            Some(applied),
            checkpoint,
        )
        .await
    })
}
