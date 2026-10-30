//! Bringing the messages of a chat into the chat of a node: each message under an identifier derived from the chat it joins and its own identifier in the source, so that a later import adds only the messages that are not there yet; authors, replies, reactions, pins and files read through the map; and the taking back of the messages an import added.
//!
//! A message is written as the importer's own where its author is not an account the import may write as; the name of whoever
//! wrote it there is the first line of what it says ("originally by"). A message keeps the moment it was written at, and it
//! takes the next number of the chat's sequence, so it lies after what the chat held already. Nobody is told: no notification,
//! no mail, no mention counted and nothing unread; only the announcement that the chat changed, which an open chat shows.
//! A line the system wrote in the chat is not brought, and a deleted message has nothing to bring.
//!
//! The record is read a page of messages at a time from a copy of it in a scratch file, so that a chat of a million messages costs
//! the memory of a page: each page is placed (which of its messages are here already), written in groups, and let go of.

use super::{Batch, Made, MakeError, Record, Taken, note_origin, refuse};
use crate::error::EngineError;
use crate::imports::identity;
use crate::imports::model::Object;
use crate::imports::people::Credits;
use crate::imports::records::{self, Cut};
use chrono::{DateTime, Utc};
use orch_api_contract::chats::{
    ChatMentionName, ChatMentionTarget, ChatMessage, ChatMessageKind, ChatPart, Reaction,
};
use orch_api_contract::documents::Author;
use orch_api_contract::imports::{ImportAction, ImportOutcome, ImportWarning};
use orch_chat::attachments::blobs_in;
use orch_chat::messages::search_text_of;
use orch_chat::views::parts_of;
use orch_db::queries::chats::{self as rows, NewMessage};
use orch_db::queries::hierarchy;
use orch_db::queries::imports::{OriginRow, forget_objects, kind};
use orch_db::queries::imports_chats as extra;
use orch_db::{DbError, Tx};
use orch_types::{Hash, Id, Json, Seq, Time};
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// The most messages written between two looks at what is there: the identifiers of a group are asked for in one statement, its
/// numbers of the chat's sequence are taken in one, and its rows of origins are written in one. Five hundred keeps those arrays
/// small, and a chat of ten thousand messages comes to twenty rounds.
const GROUP: usize = 500;

/// How many messages of the record are read together: a group.
const READ_PAGE: usize = GROUP;

/// How many bytes of the record a page may take, once it holds that much, however few messages that is.
const READ_PAGE_BYTES: u64 = 4 * 1024 * 1024;

/// How many things a run remembers to spare asking again (the files known to be here, the messages it wrote, the files it carried): a
/// record that names more is asked about again where the memory was let go, which costs a statement and nothing else.
const REMEMBERED_MAX: usize = 100_000;

/// The word of a message in the identifiers derived from a chat.
const MESSAGE_WORD: &str = "message";

/// What a message says when nothing of it could come back.
const NOTHING_LEFT: &str = "_Nothing of this message could be brought._";

/// What a quoted turn of a thread is put as: the thread it was in is not here.
const QUOTED_TURN: &str = "(a quoted turn)";

/// The marks of Markdown that a name is written without, so that it stays one name inside the line that gives it.
const MARKDOWN_MARKS: &str = "\\`*_[]<>~|";

/// Where a message of the record is here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placed {
    /// The identifier it has here, or will have.
    id: Id,
    /// Whether it is there already.
    here: bool,
}

/// Whose a message is here.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Writer {
    /// An account.
    Account(Id),
    /// An agent, by its name: its thread is not here.
    Agent(String),
}

/// What a message of the source comes to here, before it is written.
#[derive(Debug, Clone, PartialEq)]
struct Brought {
    /// Whose it is.
    writer: Writer,
    /// What it says, the line that names whoever wrote it there first.
    parts: Vec<ChatPart>,
    /// The words a search finds it by.
    search: Option<String>,
    /// The accounts it names with `@`.
    mentions: Vec<Id>,
    /// How people reacted, as accounts here: `{emoji: [user ids]}`; none where nobody did.
    reactions: Option<Json>,
    /// How many files it carried that the package does not hold.
    missing_files: usize,
    /// How many turns it quoted, put as words.
    quoted: usize,
}

/// The parts of a message as they are brought.
#[derive(Debug, Default)]
struct Said {
    /// The parts.
    parts: Vec<ChatPart>,
    /// The accounts named with `@`, each once.
    mentions: Vec<Id>,
    /// Files left out.
    missing: usize,
    /// Turns put as words.
    quoted: usize,
}

/// What writing the messages of a record came to.
#[derive(Debug, Default)]
struct Totals {
    /// Messages added.
    added: usize,
    /// The files of the messages added, remembered so that each is counted once (the memory is let go past a bound).
    files: HashSet<Hash>,
    /// How many different files the messages added carried.
    file_count: usize,
    /// Replies and quoted turns that point at something that is not here.
    outside: usize,
    /// Files left out.
    missing: usize,
    /// The number of the newest message added.
    last_seq: Option<Seq>,
    /// The latest moment a message added was written.
    last_at: Option<DateTime<Utc>>,
}

/// Writing the messages of one chat, a page at a time.
struct Writing<'a, 'b> {
    /// What making needs.
    batch: &'a Batch<'b>,
    /// The chat.
    chat: Id,
    /// The files the batch carried for this chat.
    staged: HashSet<Hash>,
    /// The blobs the messages may carry: files from this chat's package or live destination messages.
    known: HashSet<Hash>,
    /// Claims that were not brought and are not already in the destination chat.
    absent: HashSet<Hash>,
    /// What each message of the source is here, as far as it is remembered: what was there, and what was written so far.
    written: HashMap<Id, Id>,
    /// How many lines the system wrote in the chat there, which are not brought.
    system: usize,
    /// What came of it.
    totals: Totals,
}

/// Brings the messages of the chat the plan says.
pub(super) async fn make_chat(
    tx: &mut Tx,
    batch: &Batch<'_>,
    object: &Object,
) -> Result<Made, MakeError> {
    if object.action == ImportAction::Skip {
        return Ok(Made::left(ImportOutcome::Skipped, None));
    }
    let Some(chat) = rows::chat_of_node(tx, object.lies_in).await? else {
        return refuse("the chat of the place it belongs to is not there any more");
    };
    // The record is copied and checked against its hash before anything is written: one that is damaged makes nothing.
    let record = Record::open(batch, object, "chat").await?;
    rows::lock(tx, chat.id).await?;
    let staged: HashSet<Hash> = object
        .object
        .files
        .iter()
        .filter_map(|file| batch.staged.get(&file.path))
        .map(|staged| staged.blob)
        .collect();
    let mut writing = Writing {
        batch,
        chat: chat.id,
        known: staged.clone(),
        absent: HashSet::new(),
        staged,
        written: HashMap::new(),
        system: 0,
        totals: Totals::default(),
    };
    let mut pages = records::read_array(
        record.file()?,
        Cut {
            elements: READ_PAGE,
            bytes: READ_PAGE_BYTES,
        },
    );
    while let Some(page) = pages.recv().await {
        let unreadable =
            |error: String| MakeError::Refused(format!("its messages cannot be read: {error}"));
        let page = page.map_err(unreadable)?;
        let messages: Vec<ChatMessage> = page
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<ChatMessage>, _>>()
            .map_err(|error| unreadable(error.to_string()))?;
        writing.page(tx, &messages).await?;
    }
    let (system, totals) = (writing.system, writing.totals);
    if let (Some(seq), Some(at)) = (totals.last_seq, totals.last_at) {
        extra::touch_last(tx, chat.id, seq, at).await?;
        rows::notify(tx, chat.id, seq).await?;
        if let Some(node) = chat.node_id {
            hierarchy::stamp(tx, node).await?;
        }
    }
    note_origin(tx, batch, object, chat.id, Some(object.lies_in), false).await?;
    let mut made = Made::done(object, chat.id);
    if totals.added == 0 {
        made.outcome = ImportOutcome::Unchanged;
    }
    made.carried.messages = count_of(totals.added);
    made.carried.files = count_of(totals.file_count);
    made.warnings
        .extend(warnings_of(system, totals.missing, totals.outside));
    Ok(made)
}

/// Takes back a batch of the messages that the import `import` added to the chat `chat`, as the person `principal` may now: how many
/// rows of the import it looked at, each a message deleted the way a chat deletes one (the row stays, emptied, at a number of the chat's
/// sequence of its own) with the files it carried let go, and forgotten as the import's. None are left when `Taken::Nothing` is the
/// answer; `Taken::Refused` says the person may not write in the chat now, and what the import added to it stays.
pub(super) async fn take_back(
    tx: &mut Tx,
    import: Id,
    chat: Id,
    principal: &orch_authz::Principal,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Taken, MakeError> {
    let Some(row) = rows::lock(tx, chat).await? else {
        return Ok(Taken::Nothing);
    };
    // What lies in a place that was deleted goes and comes back with it; a chat of people is not one an import writes in.
    let Some(node) = row.node_id else {
        return Ok(Taken::Nothing);
    };
    if !extra::place_is_live(tx, node).await? {
        return Ok(Taken::Nothing);
    }
    // The person has to be let write in the chat now, as they were when they brought the messages.
    let allowed = matches!(
        orch_authz::nodes::check(
            tx,
            principal,
            node,
            orch_access_contract::permissions::Permission::ChatInteract,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await,
        Ok(orch_access_contract::decisions::Decision::Allowed { .. })
    );
    if !allowed {
        return Ok(Taken::Refused);
    }
    let added = extra::added_to_chat(tx, import, chat, limit).await?;
    if added.is_empty() {
        return Ok(Taken::Nothing);
    }
    let looked: Vec<Id> = added.iter().map(|(message, _)| *message).collect();
    let live: Vec<Id> = added
        .iter()
        .filter(|(_, there)| *there)
        .map(|(message, _)| *message)
        .collect();
    if !live.is_empty() {
        let count = count_of(live.len());
        let Some(first) = extra::take_seqs(tx, chat, count).await? else {
            return Err(no_sequence());
        };
        let seqs: Vec<Seq> = (first..first + count).collect();
        let said = extra::delete_messages(tx, chat, &live, &seqs, principal.user(), now).await?;
        // A file is pinned once for each message that carries it, and let go the same number of times.
        let mut blobs: Vec<Vec<u8>> = Vec::new();
        for content in &said {
            for blob in blobs_in(&parts_of(content)) {
                blobs.push(blob.as_bytes().to_vec());
            }
        }
        extra::unpin_blobs(tx, &blobs).await?;
        hierarchy::touch_activity(tx, node, now).await?;
        rows::notify(tx, chat, first + count - 1).await?;
        hierarchy::stamp(tx, node).await?;
    }
    forget_objects(tx, import, &looked).await?;
    Ok(Taken::Some(count_of(looked.len())))
}

impl Writing<'_, '_> {
    /// Writes a page of the record: the messages of it that somebody wrote and has not deleted, those not here yet, in the order of the
    /// record.
    async fn page(&mut self, tx: &mut Tx, messages: &[ChatMessage]) -> Result<(), MakeError> {
        self.system += messages
            .iter()
            .filter(|message| message.kind == ChatMessageKind::System && !message.deleted)
            .count();
        let live = live_messages(messages);
        if live.is_empty() {
            return Ok(());
        }
        let sources: Vec<Id> = live.iter().map(|message| message.id).collect();
        let placed = place_messages(tx, self.chat, &sources).await?;
        let fresh: Vec<&ChatMessage> = live
            .iter()
            .copied()
            .filter(|message| placed.get(&message.id).is_some_and(|each| !each.here))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        self.know_blobs(tx, &fresh).await?;
        self.know_replies(tx, &fresh, &sources, &placed).await?;
        for group in fresh.chunks(GROUP) {
            self.group(tx, group, &placed).await?;
        }
        self.batch
            .say(|| format!("a chat: {} messages brought", self.totals.added));
        Ok(())
    }

    /// Finds files the destination chat already carries, besides those this package brought.
    async fn know_blobs(&mut self, tx: &mut Tx, fresh: &[&ChatMessage]) -> Result<(), MakeError> {
        if self.known.len().saturating_add(self.absent.len()) > REMEMBERED_MAX {
            self.known.clone_from(&self.staged);
            self.absent.clear();
        }
        let wanted: HashSet<Hash> = fresh
            .iter()
            .flat_map(|message| blobs_in(&message.parts))
            .filter(|blob| !self.known.contains(blob) && !self.absent.contains(blob))
            .collect();
        if wanted.is_empty() {
            return Ok(());
        }
        let held = extra::held_blobs(tx, self.chat, &wanted).await?;
        self.absent.extend(wanted.difference(&held).copied());
        self.known.extend(held);
        Ok(())
    }

    /// Finds what the messages replied to are here: what was there already, and what an earlier page of this run wrote, which a reply
    /// of this page may name however far back it lies.
    async fn know_replies(
        &mut self,
        tx: &mut Tx,
        fresh: &[&ChatMessage],
        sources: &[Id],
        placed: &HashMap<Id, Placed>,
    ) -> Result<(), MakeError> {
        if self.written.len() > REMEMBERED_MAX {
            self.written.clear();
        }
        for (source, there) in placed {
            if there.here {
                self.written.insert(*source, there.id);
            }
        }
        let in_page: HashSet<Id> = sources.iter().copied().collect();
        let mut asked: Vec<Id> = fresh
            .iter()
            .filter_map(|message| message.reply_to.as_ref())
            .map(|reply| reply.id)
            .filter(|id| !in_page.contains(id) && !self.written.contains_key(id))
            .collect();
        asked.sort_unstable();
        asked.dedup();
        if asked.is_empty() {
            return Ok(());
        }
        for (source, there) in place_messages(tx, self.chat, &asked).await? {
            if there.here {
                self.written.insert(source, there.id);
            }
        }
        Ok(())
    }

    /// Writes a group of messages that are not there, in the order of the record, and the rows that say what they are here.
    async fn group(
        &mut self,
        tx: &mut Tx,
        group: &[&ChatMessage],
        placed: &HashMap<Id, Placed>,
    ) -> Result<(), MakeError> {
        let (batch, chat) = (self.batch, self.chat);
        let Some(first) = extra::take_seqs(tx, chat, count_of(group.len())).await? else {
            return Err(no_sequence());
        };
        let importer = batch.principal.user();
        let mut origins = Vec::with_capacity(group.len());
        for (message, seq) in group.iter().zip(first..) {
            let Some(here) = placed.get(&message.id) else {
                continue;
            };
            let id = here.id;
            let brought = bring(message, &batch.plan.credits, &self.known);
            let reply_to_id = message
                .reply_to
                .as_ref()
                .and_then(|reply| self.written.get(&reply.id).copied());
            if message.reply_to.is_some() && reply_to_id.is_none() {
                self.totals.outside += 1;
            }
            self.totals.outside += brought.quoted;
            self.totals.missing += brought.missing_files;
            let content = serde_json::to_value(&brought.parts)
                .map_err(|error| MakeError::from(EngineError::unreadable(error)))?;
            let (author_id, author_agent) = match &brought.writer {
                Writer::Account(user) => (Some(*user), None),
                Writer::Agent(name) => (None, Some(name.as_str())),
            };
            let client = client_id_of(batch.import, message.id);
            let at = message.created_at.get();
            rows::insert_message(
                tx,
                &NewMessage {
                    id,
                    chat_id: chat,
                    kind: rows::MESSAGE,
                    author_id,
                    author_thread_id: None,
                    author_agent,
                    content: &content,
                    reply_to_id,
                    mentions: &brought.mentions,
                    mentions_all: message.mentions_all,
                    client_id: Some(&client),
                    search_text: brought.search.as_deref(),
                    seq,
                    at,
                },
            )
            .await?;
            let pinned = message.pinned.as_ref().map(|pin| (importer, pin.at.get()));
            let edited = message.edited_at.map(Time::get);
            if brought.reactions.is_some() || pinned.is_some() || edited.is_some() {
                extra::set_marks(tx, id, brought.reactions.as_ref(), pinned, edited).await?;
            }
            pin_parts(tx, &brought.parts, 1).await?;
            if self.totals.files.len() > REMEMBERED_MAX {
                self.totals.files.clear();
            }
            for blob in blobs_in(&brought.parts) {
                if self.totals.files.insert(blob) {
                    self.totals.file_count += 1;
                }
            }
            self.written.insert(message.id, id);
            origins.push(origin_of_message(batch, chat, message.id, id));
            self.totals.added += 1;
            self.totals.last_seq = Some(seq);
            self.totals.last_at = Some(self.totals.last_at.map_or(at, |before| before.max(at)));
        }
        extra::put_origins(tx, &origins).await?;
        Ok(())
    }
}

/// The messages of the record that come: what somebody wrote and has not deleted, each identifier once, in the record's order.
fn live_messages(record: &[ChatMessage]) -> Vec<&ChatMessage> {
    let mut seen = HashSet::new();
    record
        .iter()
        .filter(|message| {
            message.kind == ChatMessageKind::Message && !message.deleted && seen.insert(message.id)
        })
        .collect()
}

/// The identifier a message of a source has in `chat`: derived from the chat and the message, so that it is the same whichever
/// package it came in. `again` is how many deleted messages lie under the identifiers before it (what an undo took back
/// stays as the row of a deletion, which cannot be written again).
fn message_id(chat: Id, source: Id, again: usize) -> Id {
    if again == 0 {
        identity::inside(chat, MESSAGE_WORD, source)
    } else {
        identity::inside(chat, &format!("{MESSAGE_WORD}~{again}"), source)
    }
}

/// The value of `client_id` of a message an import writes: the import and the message of the source, which no other import
/// uses.
fn client_id_of(import: Id, source: Id) -> String {
    format!("import:{import}:{source}")
}

/// Where each of the messages `sources` is here: under the identifier derived from the chat and the message if nothing there
/// is a deleted message, under the first later identifier that is free or holds the message otherwise.
async fn place_messages(
    tx: &mut Tx,
    chat: Id,
    sources: &[Id],
) -> Result<HashMap<Id, Placed>, DbError> {
    let mut placed = HashMap::with_capacity(sources.len());
    let mut open: Vec<Id> = sources.to_vec();
    let mut again = 0_usize;
    while !open.is_empty() {
        let mut next = Vec::new();
        for group in open.chunks(GROUP) {
            let ids: Vec<Id> = group
                .iter()
                .map(|source| message_id(chat, *source, again))
                .collect();
            let there: HashMap<Id, bool> = extra::present_messages(tx, chat, &ids)
                .await?
                .into_iter()
                .collect();
            for (source, id) in group.iter().zip(ids) {
                match there.get(&id) {
                    None => {
                        placed.insert(*source, Placed { id, here: false });
                    }
                    Some(false) => {
                        placed.insert(*source, Placed { id, here: true });
                    }
                    Some(true) => next.push(*source),
                }
            }
        }
        open = next;
        again += 1;
    }
    Ok(placed)
}

/// Pins the blobs of `parts` once more (`delta` 1) or once fewer (`-1`): a message carries them for as long as it is there.
async fn pin_parts(tx: &mut Tx, parts: &[ChatPart], delta: i32) -> Result<(), DbError> {
    let blobs: Vec<Vec<u8>> = blobs_in(parts)
        .iter()
        .map(|blob| blob.as_bytes().to_vec())
        .collect();
    if !blobs.is_empty() {
        rows::pin_blobs(tx, &blobs, delta).await?;
    }
    Ok(())
}

/// The row of `import_origins` for a message that was added.
fn origin_of_message(batch: &Batch<'_>, chat: Id, source: Id, id: Id) -> OriginRow {
    OriginRow {
        scope_id: batch.plan.destination.id,
        source_id: source,
        kind: kind::MESSAGE.to_owned(),
        object_id: id,
        parent_id: Some(chat),
        created_by_import: Some(batch.import),
        last_import: batch.import,
        source_name: None,
        source_path: None,
        source_sha256: None,
        source_updated_at: None,
        source_creator: None,
        imported_by: Some(batch.principal.user()),
        imported_at: batch.now,
    }
}

/// What a message of the source comes to here.
fn bring(message: &ChatMessage, credits: &Credits, known: &HashSet<Hash>) -> Brought {
    let (writer, original) = match &message.author {
        Some(Author::Agent(agent)) => (Writer::Agent(agent.agent.clone()), None),
        Some(Author::User(user)) => {
            let credit = credits.of(Some(user.id));
            (Writer::Account(credit.user), credit.original)
        }
        None => (Writer::Account(credits.importer()), None),
    };
    let mut said = say(&message.parts, &message.mention_names, credits, known);
    let search = search_text_of(&said.parts);
    if said.parts.is_empty() {
        said.parts.push(text(NOTHING_LEFT));
    }
    if let Some(name) = original {
        said.parts.insert(0, text(&label_of(&name)));
    }
    Brought {
        writer,
        parts: said.parts,
        search,
        mentions: said.mentions,
        reactions: reactions_of(&message.reactions, credits),
        missing_files: said.missing,
        quoted: said.quoted,
    }
}

/// The parts of a message read through what is here: a mention of somebody who is an account here stays a mention of that
/// account, any other becomes the words; a file the store does not hold is left out; a turn of a thread, which is not here, is
/// put as words.
fn say(
    parts: &[ChatPart],
    names: &[ChatMentionName],
    credits: &Credits,
    known: &HashSet<Hash>,
) -> Said {
    let mut said = Said {
        parts: Vec::with_capacity(parts.len()),
        ..Said::default()
    };
    for part in parts {
        match part {
            ChatPart::Text { .. } => said.parts.push(part.clone()),
            ChatPart::Mention { target } => match target {
                ChatMentionTarget::User { id } => match credits.mention(*id) {
                    Some(user) => {
                        if !said.mentions.contains(&user) {
                            said.mentions.push(user);
                        }
                        said.parts.push(ChatPart::Mention {
                            target: ChatMentionTarget::User { id: user },
                        });
                    }
                    None => said.parts.push(text(&words_of_person(*id, names, credits))),
                },
                other => {
                    if let Some(name) = names.iter().find(|each| each.target == *other) {
                        said.parts.push(text(&format!("@{}", name.name)));
                    }
                }
            },
            ChatPart::Image { blob, .. } | ChatPart::Attachment { blob, .. } => {
                if known.contains(blob) {
                    said.parts.push(part.clone());
                } else {
                    said.missing += 1;
                }
            }
            ChatPart::PastedText(paste) => {
                if known.contains(&paste.blob) {
                    said.parts.push(part.clone());
                } else {
                    said.missing += 1;
                }
            }
            ChatPart::Forwarded(copy) => {
                let mut copy = copy.clone();
                let before = copy.images.len() + copy.attachments.len();
                copy.images.retain(|image| known.contains(&image.blob));
                copy.attachments.retain(|file| known.contains(&file.blob));
                said.missing += before - copy.images.len() - copy.attachments.len();
                said.parts.push(ChatPart::Forwarded(copy));
            }
            ChatPart::ItemRef { .. } => {
                said.parts.push(text(QUOTED_TURN));
                said.quoted += 1;
            }
        }
    }
    said
}

/// A part that is words.
fn text(words: &str) -> ChatPart {
    ChatPart::Text {
        text: words.to_owned(),
    }
}

/// How a person without an account here is named where they were mentioned: by their username, else by the name the record
/// gives the mention, else as the import calls them.
fn words_of_person(person: Id, names: &[ChatMentionName], credits: &Credits) -> String {
    let target = ChatMentionTarget::User { id: person };
    let name = credits
        .username(person)
        .map(str::to_owned)
        .or_else(|| {
            names
                .iter()
                .find(|each| each.target == target)
                .map(|each| each.name.clone())
        })
        .unwrap_or_else(|| credits.name(person));
    format!("@{name}")
}

/// The line that says who wrote a message there: the first of what it says.
fn label_of(name: &str) -> String {
    format!("_Originally by {}_\n\n", plain_name(name))
}

/// A name without the marks of Markdown and without line breaks.
fn plain_name(name: &str) -> String {
    let mut plain = String::with_capacity(name.len());
    for each in name.trim().chars() {
        if each.is_control() {
            plain.push(' ');
            continue;
        }
        if MARKDOWN_MARKS.contains(each) {
            plain.push('\\');
        }
        plain.push(each);
    }
    plain
}

/// How people reacted, as the accounts they are taken for: what reactions hold as `{emoji: [user ids]}`, each account once and
/// in the order they reacted. A reaction nobody of whom is an account here is left out, and none is none.
fn reactions_of(reactions: &[Reaction], credits: &Credits) -> Option<Json> {
    let mut found = serde_json::Map::new();
    for reaction in reactions {
        let mut people: Vec<Id> = Vec::new();
        for user in &reaction.users {
            if let Some(account) = credits.account(*user)
                && !people.contains(&account)
            {
                people.push(account);
            }
        }
        if !people.is_empty() {
            found.insert(
                reaction.emoji.clone(),
                Json::Array(
                    people
                        .iter()
                        .map(|id| Json::String(id.to_string()))
                        .collect(),
                ),
            );
        }
    }
    (!found.is_empty()).then_some(Json::Object(found))
}

/// What came back changed in the messages: the lines of the system that were not brought, the files that were not in the
/// package, and the replies and quoted turns whose other end is not here.
fn warnings_of(system: usize, missing: usize, outside: usize) -> Vec<ImportWarning> {
    let mut warnings = Vec::new();
    if system > 0 {
        warnings.push(warning(
            "unsupported_block",
            system,
            format!(
                "Not brought: {}",
                things(
                    system,
                    "line the system wrote in the chat",
                    "lines the system wrote in the chat"
                )
            ),
        ));
    }
    if missing > 0 {
        warnings.push(warning(
            "missing_file",
            missing,
            format!(
                "Left out because the package does not hold them: {}",
                things(missing, "file", "files")
            ),
        ));
    }
    if outside > 0 {
        warnings.push(warning(
            "references_outside",
            outside,
            format!(
                "Put as words because what they point at is not here: {}",
                things(outside, "reply or quoted turn", "replies or quoted turns")
            ),
        ));
    }
    warnings
}

/// A thing that came back changed, with its number.
fn warning(code: &str, count: usize, message: String) -> ImportWarning {
    ImportWarning {
        code: code.to_owned(),
        message,
        params: Some(json!({ "count": count })),
    }
}

/// A number of things, in words.
fn things(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// A count, as the report holds it.
fn count_of(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// The chat has no sequence: it was deleted from under the import.
fn no_sequence() -> MakeError {
    MakeError::from(EngineError::unreadable("the chat has no sequence"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imports::people::Matched;
    use chrono::Utc;
    use orch_api_contract::access::Role;
    use orch_api_contract::imports::{ImportAuthors, ImportPersonMatch};
    use orch_api_contract::package::PackagePrincipal;
    use orch_api_contract::threads::ImageMediaType;
    use orch_api_contract::users::UserKind;
    use orch_types::{Time, new_id};

    fn principal(name: &str) -> PackagePrincipal {
        PackagePrincipal {
            id: new_id(),
            kind: UserKind::Human,
            username: name.to_owned(),
            display_name: name.to_uppercase(),
            email: None,
            email_verified: false,
        }
    }

    fn credits(importer: Id, people: Vec<(PackagePrincipal, Option<Id>)>) -> Credits {
        Credits::new(
            importer,
            ImportAuthors::Accounts,
            people
                .into_iter()
                .map(|(principal, user)| Matched {
                    principal,
                    how: ImportPersonMatch::Username,
                    user,
                })
                .collect(),
        )
    }

    fn message(parts: Vec<ChatPart>) -> ChatMessage {
        ChatMessage {
            mention_names: Vec::new(),
            mention_targets: Vec::new(),
            asked_agents: Vec::new(),
            id: new_id(),
            chat_id: new_id(),
            kind: ChatMessageKind::Message,
            author: None,
            parts,
            system: None,
            reply_to: None,
            mentions: Vec::new(),
            mentions_all: false,
            reactions: Vec::new(),
            pinned: None,
            edited_at: None,
            deleted: false,
            seq_created: 1,
            seq_updated: 1,
            created_at: Time::new(Utc::now()),
        }
    }

    fn image(blob: Hash) -> ChatPart {
        ChatPart::Image {
            blob,
            media_type: ImageMediaType::Png,
            name: "a.png".to_owned(),
        }
    }

    fn file(blob: Hash) -> ChatPart {
        ChatPart::Attachment {
            blob,
            name: "a.txt".to_owned(),
            size: 1,
            media_type: "text/plain".to_owned(),
        }
    }

    #[test]
    fn the_first_identifier_of_a_message_is_derived_from_its_chat_and_itself() {
        let (chat, source) = (new_id(), new_id());
        assert_eq!(
            message_id(chat, source, 0),
            identity::inside(chat, "message", source)
        );
        assert_ne!(message_id(chat, source, 0), message_id(new_id(), source, 0));
        assert_ne!(message_id(chat, source, 0), message_id(chat, new_id(), 0));
    }

    #[test]
    fn a_message_whose_identifier_holds_a_deletion_gets_another_that_is_always_the_same() {
        let (chat, source) = (new_id(), new_id());
        let later = message_id(chat, source, 1);
        assert_ne!(later, message_id(chat, source, 0));
        assert_ne!(later, message_id(chat, source, 2));
        assert_eq!(later, message_id(chat, source, 1));
    }

    #[test]
    fn the_identity_that_makes_sending_again_harmless_names_the_import_and_the_message_and_fits() {
        let (import, source) = (new_id(), new_id());
        let client = client_id_of(import, source);
        assert_eq!(client, format!("import:{import}:{source}"));
        assert!(
            client.len() <= 200,
            "a chat takes a value of at most 200 characters"
        );
    }

    #[test]
    fn a_record_holds_each_message_somebody_wrote_once_in_its_order() {
        let (first, second) = (message(vec![text("one")]), message(vec![text("two")]));
        let mut line = message(Vec::new());
        line.kind = ChatMessageKind::System;
        let mut gone = message(Vec::new());
        gone.deleted = true;
        let again = ChatMessage {
            parts: vec![text("one, again")],
            ..first.clone()
        };
        let record = vec![first.clone(), line, again, gone, second.clone()];
        let ids: Vec<Id> = live_messages(&record).iter().map(|each| each.id).collect();
        assert_eq!(ids, vec![first.id, second.id]);
    }

    #[test]
    fn the_name_of_whoever_wrote_it_there_leads_without_the_marks_of_markdown() {
        assert_eq!(label_of("Ana"), "_Originally by Ana_\n\n");
        assert_eq!(
            label_of("  Ana_*x*\nBo "),
            "_Originally by Ana\\_\\*x\\* Bo_\n\n"
        );
    }

    #[test]
    fn a_mention_of_a_person_with_an_account_stays_a_mention_of_that_account_named_once() {
        let (importer, account) = (new_id(), new_id());
        let ana = principal("ana");
        let credits = credits(importer, vec![(ana.clone(), Some(account))]);
        let mention = ChatPart::Mention {
            target: ChatMentionTarget::User { id: ana.id },
        };
        let said = say(
            &[mention.clone(), text(" hello "), mention],
            &[],
            &credits,
            &HashSet::new(),
        );
        let mine = ChatPart::Mention {
            target: ChatMentionTarget::User { id: account },
        };
        assert_eq!(said.mentions, vec![account]);
        assert_eq!(said.parts, vec![mine.clone(), text(" hello "), mine]);
    }

    #[test]
    fn a_mention_of_a_person_without_an_account_becomes_their_username_as_words() {
        let importer = new_id();
        let (bo, stranger) = (principal("bo"), new_id());
        let credits = credits(importer, vec![(bo.clone(), None)]);
        let said = say(
            &[
                ChatPart::Mention {
                    target: ChatMentionTarget::User { id: bo.id },
                },
                ChatPart::Mention {
                    target: ChatMentionTarget::User { id: stranger },
                },
            ],
            &[ChatMentionName {
                target: ChatMentionTarget::User { id: stranger },
                name: "somebody".to_owned(),
            }],
            &credits,
            &HashSet::new(),
        );
        assert!(said.mentions.is_empty());
        assert_eq!(said.parts, vec![text("@bo"), text("@somebody")]);
    }

    #[test]
    fn a_mention_of_a_role_is_words_where_the_record_names_it_and_left_out_where_it_does_not() {
        let credits = credits(new_id(), Vec::new());
        let named = ChatMentionTarget::Role {
            node_id: new_id(),
            role: Role::Admin,
        };
        let unnamed = ChatMentionTarget::Group { id: new_id() };
        let said = say(
            &[
                ChatPart::Mention {
                    target: named.clone(),
                },
                ChatPart::Mention { target: unnamed },
            ],
            &[ChatMentionName {
                target: named,
                name: "admins".to_owned(),
            }],
            &credits,
            &HashSet::new(),
        );
        assert_eq!(said.parts, vec![text("@admins")]);
    }

    #[test]
    fn a_file_the_store_does_not_hold_is_left_out_and_counted() {
        let credits = credits(new_id(), Vec::new());
        let (held, lost) = (Hash::of(b"held"), Hash::of(b"lost"));
        let said = say(
            &[image(held), file(lost), text("words")],
            &[],
            &credits,
            &HashSet::from([held]),
        );
        assert_eq!(said.parts, vec![image(held), text("words")]);
        assert_eq!(said.missing, 1);
    }

    #[test]
    fn a_turn_of_a_thread_is_put_as_words_and_counted() {
        let credits = credits(new_id(), Vec::new());
        let said = say(
            &[ChatPart::ItemRef {
                thread_id: new_id(),
                item_id: new_id(),
            }],
            &[],
            &credits,
            &HashSet::new(),
        );
        assert_eq!(said.parts, vec![text(QUOTED_TURN)]);
        assert_eq!(said.quoted, 1);
    }

    #[test]
    fn a_message_of_nobody_is_the_importers_without_a_line_that_names_anyone() {
        let importer = new_id();
        let credits = credits(importer, Vec::new());
        let brought = bring(&message(vec![text("hello")]), &credits, &HashSet::new());
        assert_eq!(brought.writer, Writer::Account(importer));
        assert_eq!(brought.parts, vec![text("hello")]);
        assert_eq!(brought.search.as_deref(), Some("hello"));
    }

    #[test]
    fn a_message_with_nothing_left_of_it_says_so_and_is_not_searched() {
        let credits = credits(new_id(), Vec::new());
        let brought = bring(
            &message(vec![file(Hash::of(b"lost"))]),
            &credits,
            &HashSet::new(),
        );
        assert_eq!(brought.parts, vec![text(NOTHING_LEFT)]);
        assert_eq!(brought.search, None);
        assert_eq!(brought.missing_files, 1);
    }

    #[test]
    fn reactions_follow_the_accounts_the_people_are_taken_for_and_nobody_else() {
        let (importer, account) = (new_id(), new_id());
        let (ana, bo) = (principal("ana"), principal("bo"));
        let credits = credits(
            importer,
            vec![(ana.clone(), Some(account)), (bo.clone(), None)],
        );
        let reactions = vec![
            Reaction {
                emoji: "👍".to_owned(),
                users: vec![ana.id, bo.id, ana.id],
            },
            Reaction {
                emoji: "🎉".to_owned(),
                users: vec![bo.id],
            },
        ];
        assert_eq!(
            reactions_of(&reactions, &credits),
            Some(json!({ "👍": [account.to_string()] }))
        );
        let only_bo = vec![Reaction {
            emoji: "🎉".to_owned(),
            users: vec![bo.id],
        }];
        assert_eq!(reactions_of(&only_bo, &credits), None);
        assert_eq!(reactions_of(&[], &credits), None);
    }

    #[test]
    fn what_came_back_changed_is_said_once_for_each_kind_with_its_number() {
        assert!(warnings_of(0, 0, 0).is_empty());
        let warnings = warnings_of(1, 2, 3);
        let codes: Vec<&str> = warnings.iter().map(|each| each.code.as_str()).collect();
        assert_eq!(
            codes,
            vec!["unsupported_block", "missing_file", "references_outside"]
        );
        let of = |code: &str| warnings.iter().find(|each| each.code == code);
        assert_eq!(
            of("missing_file").and_then(|each| each.params.clone()),
            Some(json!({ "count": 2 }))
        );
        assert!(
            of("unsupported_block")
                .is_some_and(|each| each.message.contains("1 line the system wrote"))
        );
        assert!(of("missing_file").is_some_and(|each| each.message.contains("2 files")));
    }
}
