//! The files of a copy given to an agent: a conversation, a chat or a document in its written form, or a place with everything in it that the person who shares may read, laid out as the shared tree is, and an index of them that counts what was left out.
//!
//! A copy is made once, as the thing is at that moment and as the person who shares it may read it: the text of a
//! conversation, a chat or a document is the one the tree gives an agent that reads it (`written`, and the
//! reading of a document), so a copy on disk and the live tree read the same. A place is laid out as it is in the
//! tree (`<place>/<thread>/conversation.md`, `chat.md`, `docs/<title>.md`); what the person may not read is not
//! written and is counted in `index.md`. The images and files that a chat or a document carries are not part of
//! what is made here: they are bytes of the blob store.

use crate::error::EngineError;
use crate::sharing::{Found, ShareError, Subject};
use crate::tools::read::document_written;
use crate::tools::tree::{assign_names, tree_name};
use crate::written::{self, Detail};
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::share::ShareParts;
use orch_authz::{Act, Policy, Principal, documents as document_access, nodes};
use orch_db::Conn;
use orch_db::queries::documents as document_rows;
use orch_db::queries::shares::{self as rows, PlaceRow};
use orch_types::{Id, Time};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// The most places below a place that a copy walks. A place with more is copied as far as this reaches, and the
/// index says so.
pub const PLACES_MAX: i64 = 2_000;

/// The name of the file that lists the others.
pub const INDEX: &str = "index.md";

/// One file of a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// Where it is written, relative to the directory of the copy.
    pub path: String,
    /// What it holds.
    pub bytes: Vec<u8>,
}

/// What a copy holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    /// The files, `index.md` among them.
    pub files: Vec<File>,
    /// How many threads, chats and documents there were that the person may not read.
    pub left_out: i64,
}

impl Files {
    /// Their size together, in bytes.
    #[must_use]
    pub fn bytes(&self) -> i64 {
        self.files
            .iter()
            .map(|file| i64::try_from(file.bytes.len()).unwrap_or(i64::MAX))
            .sum()
    }
}

/// A file with a line of the index that describes it.
struct Listed {
    path: String,
    what: String,
}

/// Everything read about the places of a copy before it is laid out.
struct Surroundings {
    places: Vec<PlaceRow>,
    readable_threads: HashSet<Id>,
    chats: HashMap<Id, (Id, i64)>,
    documents: HashMap<Id, Vec<(Id, String)>>,
    cut_short: bool,
}

/// Lays `bytes` out as a file and lists it.
fn put(out: &mut Files, listed: &mut Vec<Listed>, path: String, what: String, text: String) {
    listed.push(Listed {
        path: path.clone(),
        what,
    });
    out.files.push(File {
        path,
        bytes: text.into_bytes(),
    });
}

/// The text of the index: what the copy is, the files in it and what was left out.
fn index_text(
    name: &str,
    as_of: Time,
    listed: &[Listed],
    left_out: i64,
    cut_short: bool,
) -> String {
    let mut text = format!(
        "# {name}\n\nA copy as of {}. It holds {} files, listed here with what each is.\n\n",
        written::minute(as_of),
        listed.len()
    );
    for file in listed {
        let _ = writeln!(text, "- {} \u{2014} {}", file.path, file.what);
    }
    if left_out > 0 {
        let _ = write!(
            text,
            "\n{left_out} things in it were left out: the person who shared it may not read them or take them away as files.\n"
        );
    }
    if cut_short {
        let _ = write!(
            text,
            "\nThe place holds more than a copy reaches; what is further down is not in it.\n"
        );
    }
    text
}

impl crate::engine::Engine {
    /// The files of a copy of `found`, for the areas in `include`, as `principal` may read them.
    pub(crate) async fn files_of_copy(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        found: &Found,
        include: &ShareParts,
        as_of: Time,
    ) -> Result<Files, ShareError> {
        let mut out = Files::default();
        let mut listed: Vec<Listed> = Vec::new();
        let mut cut_short = false;
        match found.subject {
            Subject::Document(id) => {
                let row = document_rows::get(conn, id)
                    .await
                    .map_err(EngineError::from)?
                    .ok_or(ShareError::NotFound)?;
                // A copy is the thing taken away as a file: it takes the Export row of whoever makes it.
                if let Decision::Refused(refusal) = document_access::check(
                    conn,
                    principal,
                    id,
                    Permission::DocExport,
                    Policy::default(),
                )
                .await?
                {
                    return Err(ShareError::Denied(Box::new(refusal)));
                }
                let name = format!("{}.md", tree_name(&row.title));
                let text = document_written(conn, &row, &written::files_beside(&name)).await?;
                put(&mut out, &mut listed, name, "a document".to_owned(), text);
            }
            Subject::Chat(chat) => {
                if let (false, Some(node)) = (found.people_chat, found.node)
                    && let Decision::Refused(refusal) = nodes::check(
                        conn,
                        principal,
                        node,
                        Permission::ChatExport,
                        Act::Use,
                        Policy::default(),
                    )
                    .await?
                {
                    return Err(ShareError::Denied(Box::new(refusal)));
                }
                let title = found.name.clone();
                let name = if found.people_chat {
                    format!("{}.md", tree_name(&found.name))
                } else {
                    "chat.md".to_owned()
                };
                let read = written::read_chat_in(
                    conn,
                    chat,
                    &title,
                    (None, None),
                    Detail::Normal,
                    None,
                    Some(&written::files_beside(&name)),
                )
                .await?;
                put(
                    &mut out,
                    &mut listed,
                    name,
                    format!("a chat, {} messages", read.last),
                    read.text,
                );
            }
            Subject::Node(node) => {
                let around = self.surroundings(conn, principal, node, include).await?;
                cut_short = around.cut_short;
                let root = around
                    .places
                    .iter()
                    .find(|place| place.id == node)
                    .cloned()
                    .ok_or(ShareError::NotFound)?;
                let mut children: HashMap<Id, Vec<&PlaceRow>> = HashMap::new();
                for place in &around.places {
                    if let Some(parent) = place.parent_id.filter(|_| place.id != node) {
                        children.entry(parent).or_default().push(place);
                    }
                }
                self.lay_out(
                    conn,
                    &around,
                    &children,
                    &root,
                    &tree_name(root.slug.as_deref().unwrap_or(&root.name)),
                    &mut out,
                    &mut listed,
                )
                .await?;
                out.left_out = around_left_out(&around, include, conn).await?;
                // Of a thing that was asked for by itself and cannot be taken away there is nothing to copy, and
                // the refusal says what the giver lacks; of a place, what they may read and not export is left
                // out and counted.
                if out.files.is_empty()
                    && out.left_out > 0
                    && let Some(refusal) =
                        self.export_lacking(conn, principal, &root, include).await?
                {
                    return Err(ShareError::Denied(Box::new(refusal)));
                }
            }
        }
        let index = index_text(&found.name, as_of, &listed, out.left_out, cut_short);
        out.files.push(File {
            path: INDEX.to_owned(),
            bytes: index.into_bytes(),
        });
        Ok(out)
    }

    /// The places of a copy and what the person may read in them.
    async fn surroundings(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        node: Id,
        include: &ShareParts,
    ) -> Result<Surroundings, ShareError> {
        let root = rows::places(conn, &[node])
            .await
            .map_err(EngineError::from)?
            .into_iter()
            .next()
            .ok_or(ShareError::NotFound)?;
        let below = rows::below(conn, node, PLACES_MAX)
            .await
            .map_err(EngineError::from)?;
        let cut_short = i64::try_from(below.len()).unwrap_or(i64::MAX) >= PLACES_MAX;
        let mut places = vec![root];
        places.extend(below);
        let ids: Vec<Id> = places.iter().map(|place| place.id).collect();
        let mut readable_threads: HashSet<Id> = HashSet::new();
        if include.timeline.is_some() {
            let threads: Vec<Id> = places
                .iter()
                .filter(|place| place.kind == "thread")
                .map(|place| place.id)
                .collect();
            readable_threads = nodes::holding(
                conn,
                principal,
                &threads,
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                Policy::default(),
            )
            .await?
            .into_iter()
            .map(|listed| listed.id)
            .collect();
            // The export of a place: a copy is the conversation taken away as a file.
            let exportable: HashSet<Id> = nodes::holding(
                conn,
                principal,
                &threads,
                PermissionName::Exact(Permission::ThreadExport),
                Act::Use,
                Policy::default(),
            )
            .await?
            .into_iter()
            .map(|listed| listed.id)
            .collect();
            readable_threads.retain(|thread| exportable.contains(thread));
        }
        let mut chats: HashMap<Id, (Id, i64)> = HashMap::new();
        if include.chat.is_some() {
            let readable: HashSet<Id> = nodes::holding(
                conn,
                principal,
                &ids,
                PermissionName::Exact(Permission::ChatView),
                Act::Use,
                Policy::default(),
            )
            .await?
            .into_iter()
            .map(|listed| listed.id)
            .collect();
            let exportable: HashSet<Id> = nodes::holding(
                conn,
                principal,
                &ids,
                PermissionName::Exact(Permission::ChatExport),
                Act::Use,
                Policy::default(),
            )
            .await?
            .into_iter()
            .map(|listed| listed.id)
            .collect();
            let readable: HashSet<Id> = readable.intersection(&exportable).copied().collect();
            for chat in rows::chats_of_nodes(conn, &ids)
                .await
                .map_err(EngineError::from)?
            {
                if chat.messages > 0 && readable.contains(&chat.node_id) {
                    chats.insert(chat.node_id, (chat.chat_id, chat.messages));
                }
            }
        }
        let mut documents: HashMap<Id, Vec<(Id, String)>> = HashMap::new();
        if include.documents.is_some() {
            let beside = rows::documents_beside(conn, &ids)
                .await
                .map_err(EngineError::from)?;
            let listed =
                document_access::listed(conn, principal, &beside, Policy::default()).await?;
            for each in listed
                .into_iter()
                .filter(|each| each.holds(Permission::DocView) && each.holds(Permission::DocExport))
            {
                if let Some(row) = document_rows::get(conn, each.document.id)
                    .await
                    .map_err(EngineError::from)?
                {
                    documents
                        .entry(row.node_id)
                        .or_default()
                        .push((row.id, row.title));
                }
            }
        }
        Ok(Surroundings {
            places,
            readable_threads,
            chats,
            documents,
            cut_short,
        })
    }

    /// Writes the files of a place and of what lies in it, depth first; a directory with nothing in it is not made.
    #[allow(
        clippy::too_many_arguments,
        reason = "a place is laid out from what was read about it, under a path, into the files and the index"
    )]
    fn lay_out<'a>(
        &'a self,
        conn: &'a mut Conn,
        around: &'a Surroundings,
        children: &'a HashMap<Id, Vec<&'a PlaceRow>>,
        place: &'a PlaceRow,
        directory: &'a str,
        out: &'a mut Files,
        listed: &'a mut Vec<Listed>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ShareError>> + Send + 'a>>
    {
        Box::pin(async move {
            if place.kind == "thread" && around.readable_threads.contains(&place.id) {
                let read =
                    written::read_conversation(conn, place.id, (None, None), Detail::Normal, None)
                        .await?
                        .ok_or(ShareError::NotFound)?;
                put(
                    out,
                    listed,
                    format!("{directory}/conversation.md"),
                    format!("the conversation of {}, {} turns", place.name, read.last),
                    read.text,
                );
            }
            if let Some((chat, _)) = around.chats.get(&place.id) {
                let read = written::read_chat(
                    conn,
                    *chat,
                    &place.name,
                    (None, None),
                    Detail::Normal,
                    None,
                )
                .await?;
                put(
                    out,
                    listed,
                    format!("{directory}/chat.md"),
                    format!("the chat of {}, {} messages", place.name, read.last),
                    read.text,
                );
            }
            if let Some(documents) = around.documents.get(&place.id) {
                let names = assign_names(
                    &documents
                        .iter()
                        .map(|(id, title)| (*id, title.clone()))
                        .collect::<Vec<_>>(),
                    ".md",
                    false,
                );
                for ((id, _), name) in documents.iter().zip(names) {
                    let Some(row) = document_rows::get(conn, *id)
                        .await
                        .map_err(EngineError::from)?
                    else {
                        continue;
                    };
                    let text = document_written(conn, &row, &written::files_beside(&name)).await?;
                    put(
                        out,
                        listed,
                        format!("{directory}/docs/{name}"),
                        format!("a document of {}", place.name),
                        text,
                    );
                }
            }
            if let Some(kids) = children.get(&place.id) {
                let names = assign_names(
                    &kids
                        .iter()
                        .map(|kid| (kid.id, kid.name.clone()))
                        .collect::<Vec<_>>(),
                    "",
                    true,
                );
                for (kid, name) in kids.iter().zip(names) {
                    self.lay_out(
                        conn,
                        around,
                        children,
                        kid,
                        &format!("{directory}/{name}"),
                        out,
                        listed,
                    )
                    .await?;
                }
            }
            Ok(())
        })
    }
}

/// How many threads, chats and documents of a copy's places the person may not read.
async fn around_left_out(
    around: &Surroundings,
    include: &ShareParts,
    conn: &mut Conn,
) -> Result<i64, ShareError> {
    let ids: Vec<Id> = around.places.iter().map(|place| place.id).collect();
    let mut total = 0_i64;
    let mut readable = 0_i64;
    if include.timeline.is_some() {
        total += i64::try_from(
            around
                .places
                .iter()
                .filter(|place| place.kind == "thread")
                .count(),
        )
        .unwrap_or(i64::MAX);
        readable += i64::try_from(around.readable_threads.len()).unwrap_or(i64::MAX);
    }
    if include.chat.is_some() {
        let with_messages = rows::chats_of_nodes(conn, &ids)
            .await
            .map_err(EngineError::from)?
            .into_iter()
            .filter(|chat| chat.messages > 0)
            .count();
        total += i64::try_from(with_messages).unwrap_or(i64::MAX);
        readable += i64::try_from(around.chats.len()).unwrap_or(i64::MAX);
    }
    if include.documents.is_some() {
        total += i64::try_from(
            rows::documents_beside(conn, &ids)
                .await
                .map_err(EngineError::from)?
                .len(),
        )
        .unwrap_or(i64::MAX);
        readable += i64::try_from(around.documents.values().map(Vec::len).sum::<usize>())
            .unwrap_or(i64::MAX);
    }
    Ok((total - readable).max(0))
}

// ---------------------------------------------------------------- what the giver lacks to take it away (the export of a place)

impl crate::engine::Engine {
    /// The refusal for a copy of `place` of which nothing could be written for want of an Export row, naming the
    /// first area asked for that the giver lacks there; nothing where they hold it.
    async fn export_lacking(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        place: &PlaceRow,
        include: &ShareParts,
    ) -> Result<Option<orch_access_contract::decisions::Refusal>, ShareError> {
        let mut asked: Vec<Permission> = Vec::new();
        if include.timeline.is_some() && place.kind == "thread" {
            asked.push(Permission::ThreadExport);
        }
        if include.chat.is_some() {
            asked.push(Permission::ChatExport);
        }
        if include.documents.is_some() {
            asked.push(Permission::DocExport);
        }
        for permission in asked {
            if let Decision::Refused(refusal) = nodes::check(
                conn,
                principal,
                place.id,
                permission,
                Act::Use,
                Policy::default(),
            )
            .await?
            {
                return Ok(Some(refusal));
            }
        }
        Ok(None)
    }
}

impl Files {
    /// Writes one recipient's form without retaining another full snapshot for every destination.
    pub(crate) fn for_reader(
        &self,
        writings: &HashMap<orch_documents::references::Typed, orch_documents::references::Writing>,
    ) -> Self {
        let mut copy = self.clone();
        for file in &mut copy.files {
            if file.path.ends_with(".md")
                && let Ok(text) = std::str::from_utf8(&file.bytes)
            {
                let mut numbered = 0;
                file.bytes = orch_documents::references::for_reader(
                    text,
                    &|typed| writings.get(typed).cloned(),
                    &mut numbered,
                )
                .into_bytes();
            }
        }
        copy
    }
}

impl crate::engine::Engine {
    /// Resolves a snapshot's reference union once under this recipient's current reach.
    pub(crate) async fn copy_writings(
        &self,
        conn: &mut Conn,
        thread: Id,
        original: &Files,
    ) -> Result<
        HashMap<orch_documents::references::Typed, orch_documents::references::Writing>,
        ShareError,
    > {
        let wanted: Vec<_> = original
            .files
            .iter()
            .filter(|file| file.path.ends_with(".md"))
            .filter_map(|file| std::str::from_utf8(&file.bytes).ok())
            .flat_map(orch_documents::references::typed_in)
            .collect();
        Ok(self.writings_for(conn, thread, &wanted).await?)
    }

    /// A snapshot measured or read in one recipient's namespace.
    pub(crate) async fn copy_for_reader(
        &self,
        conn: &mut Conn,
        thread: Id,
        original: &Files,
    ) -> Result<Files, ShareError> {
        let writings = self.copy_writings(conn, thread, original).await?;
        Ok(original.for_reader(&writings))
    }
}
