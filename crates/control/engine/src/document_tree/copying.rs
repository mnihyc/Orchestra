//! Fresh document-tree material through normal headers, content batches and independent file pins.

use super::operations::Checkpoint;
use super::{Actor, Copied, Directory, Error, Item, Options, Request, access, create, prepare};
use futures::future::BoxFuture;
use orch_api_contract::documents::{DOCUMENT_OPS_MAX, DocumentOp, NewBlock};
use orch_db::{
    Conn,
    queries::documents::{self as rows, batch, containers},
};
use orch_documents::versions::Writer;
use orch_types::{Id, Time, new_id};
use std::collections::{HashMap, HashSet};

pub(super) fn material_error(error: rows::copy::Error) -> Error {
    match error {
        rows::copy::Error::Db(error) => error.into(),
        rows::copy::Error::Content(error) => error.into(),
        rows::copy::Error::MissingFile => Error::Conflict,
        rows::copy::Error::MissingSource => Error::NotFound,
    }
}

pub(super) fn creation_ops(blocks: Vec<NewBlock>) -> Vec<DocumentOp> {
    let mut previous = None;
    blocks
        .into_iter()
        .map(|block| {
            let id = block.id;
            let op = DocumentOp::Insert {
                block,
                parent_id: None,
                after_id: previous,
            };
            previous = Some(id);
            op
        })
        .collect()
}

/// Native copy operands and freshly remapped material, before the first header is written.
pub(super) struct PreparedCopy {
    request: Request,
    parent: Directory,
    node: Id,
    root: Id,
    identity: access::Identity,
    ordered: Vec<containers::EntryRow>,
    root_items: HashSet<Item>,
    ids: HashMap<Item, Id>,
    material: HashMap<Id, rows::copy::Plan>,
    settings: crate::tools::edit::Settings,
    text_copy: Option<super::TextCopy>,
    first_header: Option<super::TextCreationPlan>,
}

pub(super) async fn prepare_copy(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    prepared: &prepare::Prepared,
    options: Options,
) -> Result<PreparedCopy, Error> {
    let parent = request
        .to
        .ok_or(Error::Invalid("copy needs an existing destination"))?;
    let (node, root, _parent_id) = prepared.destination.ok_or(Error::NotFound)?;
    let identity = access::identity(conn, actor).await?;
    let live = (request.command == super::Command::MoveOut)
        .then(|| prepare::live_entries(&prepared.entries));
    let ordered = prepare::topological(live.as_deref().unwrap_or(&prepared.entries))?;
    let root_items: HashSet<_> = prepared.roots.iter().map(access::item).collect();
    let ids: HashMap<_, _> = ordered
        .iter()
        .map(|row| {
            let item = access::item(row);
            let id = prepared
                .text_copy
                .as_ref()
                .filter(|_| root_items.contains(&item))
                .map_or_else(new_id, |copy| copy.id);
            (item, id)
        })
        .collect();
    let pairs: Vec<_> = ordered
        .iter()
        .filter(|row| row.kind == "document")
        .map(|row| (row.id, ids[&Item::Document(row.id)]))
        .collect();
    let material = rows::copy::plan_tree(conn, &pairs)
        .await
        .map_err(material_error)?;
    let mut settings = crate::tools::edit::settings_at(conn, node).await?;
    settings.rules.window_secs = options.version_window_secs;
    for plan in material.values() {
        let total = plan
            .files
            .iter()
            .fold(0_i64, |n, file| n.saturating_add(file.file.size));
        if plan
            .files
            .iter()
            .any(|file| file.file.size > settings.files.file_bytes)
            || total > settings.files.files_bytes
        {
            return Err(Error::Limit);
        }
    }
    Ok(PreparedCopy {
        request: request.clone(),
        parent,
        node,
        root,
        identity,
        ordered,
        root_items,
        ids,
        material,
        settings,
        text_copy: prepared.text_copy.clone(),
        first_header: None,
    })
}

type CopyResult = Result<(Vec<Item>, Vec<Copied>), Error>;

pub(super) fn apply_copy<'a, 'check: 'a>(
    conn: &'a mut Conn,
    actor: Actor<'a>,
    prepared: PreparedCopy,
    now: Time,
    checkpoint: &'a mut Checkpoint<'check>,
) -> BoxFuture<'a, CopyResult> {
    Box::pin(async move {
        let PreparedCopy {
            request,
            parent,
            node,
            root,
            identity,
            ordered,
            root_items,
            ids,
            mut material,
            settings,
            text_copy,
            mut first_header,
        } = prepared;
        let mut made = Vec::new();
        let mut copied = Vec::new();
        for row in ordered {
            let source = access::item(&row);
            let fresh = ids[&source];
            let selected = root_items.contains(&source);
            let new_parent = if selected {
                parent
            } else {
                Directory::Container(
                    *ids.get(&Item::Container(
                        row.parent_container_id.ok_or(Error::Conflict)?,
                    ))
                    .ok_or(Error::Conflict)?,
                )
            };
            let new_name = if selected {
                request.name.as_deref().unwrap_or(&row.basename)
            } else {
                &row.basename
            };
            create::validate_name(new_name, row.kind == "container")?;
            let destination = if row.kind == "container" {
                let container_parent = match new_parent {
                    Directory::NodeDocs(_) => None,
                    Directory::Container(id) => Some(id),
                };
                checkpoint.check()?;
                containers::insert(
                    conn,
                    &containers::NewContainer {
                        id: fresh,
                        node,
                        root,
                        parent: container_parent,
                        owner: identity.owner,
                        thread: identity.thread,
                        agent: identity.agent.as_deref(),
                        name: new_name,
                        now: now.get(),
                    },
                )
                .await?;
                create::creation_grant(conn, fresh, true, &identity, now).await?;
                Item::Container(fresh)
            } else {
                let title = text_title(
                    text_copy.as_ref(),
                    &row,
                    new_name,
                    selected && request.name.is_some(),
                )?;
                let header = match first_header.take() {
                    Some(header) if header.id() == fresh => header,
                    Some(_) => return Err(Error::Conflict),
                    None => {
                        create::prepare_text_creation_in(
                            conn,
                            actor,
                            new_parent,
                            fresh,
                            title,
                            Some(new_name),
                            now,
                        )
                        .await?
                    }
                };
                if header.is_replay() {
                    return Err(Error::Conflict);
                }
                create::apply_text_creation_on(conn, actor, header, now, || {
                    checkpoint.check_engine()
                })
                .await??;
                let plan = material.remove(&row.id).ok_or(Error::Conflict)?;
                install_document(
                    conn,
                    actor,
                    DocumentInstall {
                        source: &row,
                        destination: fresh,
                        node,
                        identity: &identity,
                        settings: &settings,
                        plan,
                        private: text_copy.as_ref().is_some_and(|copy| copy.private),
                    },
                    now,
                )
                .await?;
                Item::Document(fresh)
            };
            made.push(destination);
            if selected {
                record_copy_out(conn, actor, &row, node, fresh, now).await?;
                checkpoint.check()?;
            }
            copied.push(Copied {
                source,
                destination,
            });
        }
        // Parent/root were checked under the same scope lock; no guessed destination anchor is used.
        Ok((made, copied))
    })
}

fn text_title<'a>(
    copy: Option<&'a super::TextCopy>,
    row: &'a containers::EntryRow,
    name: &'a str,
    renamed: bool,
) -> Result<&'a str, Error> {
    if let Some(title) = copy.and_then(|copy| copy.title.as_deref()) {
        Ok(title)
    } else if renamed {
        name.strip_suffix(".md")
            .ok_or(Error::Invalid("a text document basename ends in .md"))
    } else {
        Ok(&row.title)
    }
}

impl PreparedCopy {
    /// Complete an existing first text destination before returning the original source footprint.
    /// Child headers inside freshly created folders remain intrinsic to that same native act.
    pub(super) async fn prepare_first_header(
        &mut self,
        conn: &mut Conn,
        actor: Actor<'_>,
        now: Time,
    ) -> Result<(), Error> {
        let Some(row) = self.ordered.first() else {
            return Err(Error::NotFound);
        };
        if row.kind != "document" {
            return Ok(());
        }
        let source = access::item(row);
        if !self.root_items.contains(&source) {
            return Err(Error::Conflict);
        }
        let id = *self.ids.get(&source).ok_or(Error::Conflict)?;
        let name = self.request.name.as_deref().unwrap_or(&row.basename);
        let title = text_title(
            self.text_copy.as_ref(),
            row,
            name,
            self.request.name.is_some(),
        )?;
        let header =
            create::prepare_text_creation_in(conn, actor, self.parent, id, title, Some(name), now)
                .await?;
        if header.is_replay() {
            return Err(Error::Conflict);
        }
        self.first_header = Some(header);
        Ok(())
    }
    /// The ordinary single-text header takes its final native directory/namespace footprint.
    pub(super) async fn prepare_text_header(
        &self,
        conn: &mut Conn,
        actor: Actor<'_>,
        now: Time,
    ) -> Result<super::TextCreationPlan, Error> {
        let [row] = self.ordered.as_slice() else {
            return Err(Error::Invalid(
                "one text header requires one copied document",
            ));
        };
        let item = access::item(row);
        if !matches!(item, Item::Document(_)) || !self.root_items.contains(&item) {
            return Err(Error::Invalid(
                "one text header requires the actual root document",
            ));
        }
        let id = *self.ids.get(&item).ok_or(Error::Conflict)?;
        let name = self.request.name.as_deref().unwrap_or(&row.basename);
        let title = text_title(
            self.text_copy.as_ref(),
            row,
            name,
            self.request.name.is_some(),
        )?;
        let header =
            create::prepare_text_creation_in(conn, actor, self.parent, id, title, Some(name), now)
                .await?;
        // A completed tree replay was selected before material preparation. A late occupied
        // destination cannot become a new copy into an unrelated existing document.
        if header.is_replay() {
            return Err(Error::Conflict);
        }
        Ok(header)
    }
}

/// Continue the same material writer only after its exact native text header was created.
pub(super) async fn apply_text_material(
    conn: &mut Conn,
    actor: Actor<'_>,
    prepared: PreparedCopy,
    header: &rows::DocumentRow,
    now: Time,
) -> Result<(Vec<Item>, Vec<Copied>), Error> {
    let [row] = prepared.ordered.as_slice() else {
        return Err(Error::Invalid(
            "one text material requires one copied document",
        ));
    };
    let source = access::item(row);
    let fresh = *prepared.ids.get(&source).ok_or(Error::Conflict)?;
    if !matches!(source, Item::Document(_))
        || !prepared.root_items.contains(&source)
        || header.id != fresh
        || header.node_id != prepared.node
        || header.org_id != prepared.root
    {
        return Err(Error::Stale);
    }
    let plan = prepared
        .material
        .into_iter()
        .find_map(|(id, plan)| (id == row.id).then_some(plan))
        .ok_or(Error::Conflict)?;
    install_document(
        conn,
        actor,
        DocumentInstall {
            source: row,
            destination: fresh,
            node: prepared.node,
            identity: &prepared.identity,
            settings: &prepared.settings,
            plan,
            private: prepared.text_copy.as_ref().is_some_and(|copy| copy.private),
        },
        now,
    )
    .await?;
    record_copy_out(conn, actor, row, prepared.node, fresh, now).await?;
    Ok((
        vec![Item::Document(fresh)],
        vec![Copied {
            source,
            destination: Item::Document(fresh),
        }],
    ))
}

struct DocumentInstall<'a> {
    source: &'a containers::EntryRow,
    destination: Id,
    node: Id,
    identity: &'a access::Identity,
    settings: &'a crate::tools::edit::Settings,
    plan: rows::copy::Plan,
    private: bool,
}

async fn install_document(
    conn: &mut Conn,
    actor: Actor<'_>,
    install: DocumentInstall<'_>,
    now: Time,
) -> Result<(), Error> {
    let DocumentInstall {
        source,
        destination,
        node,
        identity,
        settings,
        plan,
        private,
    } = install;
    // The copy says what the original says it is about, as it carries its title.
    rows::copy_description(conn, source.id, destination).await?;
    if private {
        create::make_private(conn, actor, destination, node).await?;
    }
    rows::copy::install_files(
        conn,
        destination,
        &plan.files,
        identity.thread.is_none().then_some(identity.owner),
        identity.thread,
        now.get(),
    )
    .await
    .map_err(material_error)?;
    let writer = identity.agent.as_ref().zip(identity.thread).map_or(
        Writer::User(identity.owner),
        |(agent, thread)| Writer::Agent {
            thread,
            agent: agent.clone(),
        },
    );
    let operations = creation_ops(plan.blocks);
    let limit = usize::try_from(DOCUMENT_OPS_MAX)
        .unwrap_or(usize::MAX)
        .max(1);
    for operations in operations.chunks(limit) {
        match batch::apply(
            conn,
            destination,
            &writer,
            None,
            operations,
            settings.limits,
            settings.rules,
            now.get(),
        )
        .await?
        {
            batch::Outcome::Applied(_) | batch::Outcome::Repeated { .. } => {}
            _ => return Err(Error::Limit),
        }
    }
    Ok(())
}

async fn record_copy_out(
    conn: &mut Conn,
    actor: Actor<'_>,
    row: &containers::EntryRow,
    node: Id,
    fresh: Id,
    now: Time,
) -> Result<(), Error> {
    let kind = if row.kind == "container" {
        orch_access_contract::permissions::ObjectKind::DocumentContainer
    } else {
        orch_access_contract::permissions::ObjectKind::Document
    };
    let boundary = crate::copy_out::inspect(
        conn,
        crate::copy_out::Subject::Object(orch_access_contract::decisions::ObjectRef {
            kind,
            id: row.id,
        }),
        Some(node),
    )
    .await
    .map_err(|error| match error {
        crate::copy_out::Error::Native(error) => Error::Engine(error),
        _ => Error::Denied,
    })?;
    if let Some(boundary) = boundary {
        let by = match actor {
            Actor::Person(principal) => orch_db::audit::Actor::User(principal.user()),
            Actor::Agent { thread } => orch_db::audit::Actor::Agent(thread),
        };
        crate::copy_out::record(
            conn,
            boundary,
            by,
            &orch_db::audit::Origin::default(),
            fresh,
            now.get(),
        )
        .await?;
    }
    Ok(())
}
