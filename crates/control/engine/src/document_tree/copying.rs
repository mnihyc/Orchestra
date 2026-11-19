//! Fresh document-tree material through normal headers, content batches and independent file pins.

use super::{Actor, Copied, Directory, Error, Item, Options, Request, access, create, prepare};
use orch_api_contract::documents::{DOCUMENT_OPS_MAX, DocumentOp, NewBlock};
use orch_db::{
    Conn,
    queries::documents::{self as rows, batch, containers},
};
use orch_documents::versions::Writer;
use orch_types::{Time, new_id};
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

pub(super) async fn copy(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    prepared: &prepare::Prepared,
    now: Time,
    options: Options,
) -> Result<(Vec<Item>, Vec<Copied>), Error> {
    let parent = request
        .to
        .ok_or(Error::Invalid("copy needs an existing destination"))?;
    let (node, root, _parent_id) = prepared.destination.ok_or(Error::NotFound)?;
    let identity = access::identity(conn, actor).await?;
    let ordered = prepare::topological(&prepared.entries)?;
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
    let mut material = rows::copy::plan_tree(conn, &pairs)
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
            let title = if let Some(title) = prepared
                .text_copy
                .as_ref()
                .and_then(|copy| copy.title.as_deref())
            {
                title
            } else if selected && request.name.is_some() {
                new_name
                    .strip_suffix(".md")
                    .ok_or(Error::Invalid("a text document basename ends in .md"))?
            } else {
                &row.title
            };
            create::create_text(conn, actor, new_parent, fresh, title, Some(new_name), now).await?;
            // The copy says what the original says it is about, as it carries its title.
            rows::copy_description(conn, row.id, fresh).await?;
            if prepared.text_copy.as_ref().is_some_and(|copy| copy.private) {
                create::make_private(conn, actor, fresh, node).await?;
            }
            let plan = material.remove(&row.id).ok_or(Error::Conflict)?;
            rows::copy::install_files(
                conn,
                fresh,
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
                    fresh,
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
            Item::Document(fresh)
        };
        made.push(destination);
        copied.push(Copied {
            source,
            destination,
        });
    }
    // Parent/root were checked under the same scope lock; no guessed destination anchor is used.
    Ok((made, copied))
}
