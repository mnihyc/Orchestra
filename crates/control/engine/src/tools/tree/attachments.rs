//! Virtual attachment directories share their readable owner's authority and existing written paths.

use super::*;
use orch_api_contract::chats::ChatPart;
use orch_api_contract::threads::ImageMediaType;
use orch_types::Hash;

/// One carried file in exactly the order the chat's established renderer numbers it.
#[derive(Debug, Clone)]
pub(crate) struct CarriedFile {
    pub blob: Hash,
    pub name: String,
    pub media_type: String,
    pub size: Option<i64>,
}

/// Shared extraction for path resolution and raw download; forwarded delivery itself is unchanged.
pub(crate) fn carried_files(parts: &[ChatPart]) -> Vec<CarriedFile> {
    let mut files = Vec::new();
    for part in parts {
        match part {
            ChatPart::PastedText(paste) => files.push(CarriedFile {
                blob: paste.blob,
                name: paste.name.clone(),
                media_type: "text/plain".to_owned(),
                size: Some(paste.size),
            }),
            ChatPart::Image {
                blob,
                name,
                media_type,
            } => files.push(CarriedFile {
                blob: *blob,
                name: name.clone(),
                media_type: match media_type {
                    ImageMediaType::Png => "image/png",
                    ImageMediaType::Jpeg => "image/jpeg",
                    ImageMediaType::Webp => "image/webp",
                    ImageMediaType::Gif => "image/gif",
                }
                .to_owned(),
                size: None,
            }),
            ChatPart::Attachment {
                blob,
                name,
                media_type,
                size,
            } => files.push(CarriedFile {
                blob: *blob,
                name: name.clone(),
                media_type: media_type.clone(),
                size: Some(*size),
            }),
            ChatPart::Forwarded(copy) => {
                for image in &copy.images {
                    files.push(CarriedFile {
                        blob: image.blob,
                        name: image.name.clone(),
                        media_type: image.media_type.clone(),
                        size: None,
                    });
                }
                for file in &copy.attachments {
                    files.push(CarriedFile {
                        blob: file.blob,
                        name: file.name.clone(),
                        media_type: file.media_type.clone(),
                        size: Some(file.size),
                    });
                }
            }
            _ => {}
        }
    }
    files
}

pub(crate) fn chat_file_id(chat: Id, message: Id, carried: usize) -> Id {
    let slot = u64::try_from(carried).unwrap_or(u64::MAX).to_be_bytes();
    crate::ids::derived(
        "shared.chat.file",
        &[chat.as_bytes(), message.as_bytes(), &slot],
    )
}

/// Only presence is loaded for a broad tree: filenames/messages are read when their directory is opened.
pub(super) async fn directories(
    conn: &mut Conn,
    documents: &[Id],
    chats: &[Id],
) -> Result<HashSet<Where>, EngineError> {
    let docs: Vec<Id> = if documents.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar(
            "SELECT DISTINCT document_id FROM document_files WHERE document_id=ANY($1)",
        )
        .bind(documents)
        .fetch_all(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?
    };
    let chats: Vec<Id> = if chats.is_empty() {
        Vec::new()
    } else {
        // The matching partial index keeps file discovery independent of the chat's text history.
        sqlx::query_scalar("SELECT DISTINCT m.chat_id FROM chat_messages m WHERE m.chat_id=ANY($1) AND m.deleted_at IS NULL
            AND jsonb_typeof(m.content)='array'
            AND m.content @? '$[*] ? (@.type == \"image\" || @.type == \"attachment\" || @.type == \"pasted_text\" || (@.type == \"forwarded\" && (exists(@.images[0]) || exists(@.attachments[0]))))'")
            .bind(chats).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
    };
    Ok(docs
        .into_iter()
        .map(Where::DocumentFiles)
        .chain(chats.into_iter().map(Where::ChatFiles))
        .collect())
}

impl Tree {
    async fn remember_files(conn: &mut Conn, entries: &[Entry]) -> Result<(), EngineError> {
        for entry in entries {
            if let Where::ChatFile {
                chat,
                message,
                carried,
            } = entry.at
            {
                sqlx::query("INSERT INTO chat_file_references (id,chat_id,message_id,carried) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
                    .bind(entry.id).bind(chat).bind(message).bind(i32::try_from(carried).unwrap_or(i32::MAX))
                    .execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
            }
        }
        Ok(())
    }
    /// Some(node) is a node chat; None is a direct/group chat; outer None means it is unreadable.
    #[must_use]
    pub fn chat_origin(&self, chat: Id) -> Option<Option<Id>> {
        if let Some(node) = self.dirs.iter().find(|dir| {
            dir.chat
                .as_ref()
                .is_some_and(|entry| entry.id == chat && entry.standing.reads())
        }) {
            return Some(Some(node.place.id));
        }
        self.people_chat_of(chat)
            .filter(|entry| entry.standing.reads())
            .map(|_| None)
    }

    pub(super) fn attachment_directory(
        &self,
        prefix: &str,
        name: &str,
        at: Where,
        standing: &Standing,
    ) -> Entry {
        let (kind, owner) = match at {
            Where::DocumentFiles(id) => ("document.files", id),
            Where::ChatFiles(id) => ("chat.files", id),
            _ => unreachable!("only virtual attachment directories use this constructor"),
        };
        let mut entry = Entry::bare(
            path_of(prefix, name, true),
            name.to_owned(),
            SharedEntryKind::Folder,
            ids_of(kind, owner),
            at,
        );
        entry.via = entry_via(standing);
        entry.shared_by = standing.shared_by;
        entry
    }

    fn chat_file_entry(
        prefix: &str,
        chat: Id,
        message: &rows::NumberedMessage,
        carried: usize,
        file: &CarriedFile,
    ) -> Entry {
        let name = orch_chat::written::file_name(message.number, carried, &file.name);
        let id = chat_file_id(chat, message.row.id, carried);
        let mut entry = Entry::bare(
            path_of(prefix, &name, false),
            name,
            SharedEntryKind::File,
            id,
            Where::ChatFile {
                chat,
                message: message.row.id,
                carried,
            },
        );
        entry.size = file.size;
        entry.description = Some(file.media_type.clone());
        entry.updated_at = Some(Time::new(
            message.row.edited_at.unwrap_or(message.row.created_at),
        ));
        entry
    }

    pub(super) async fn attachment_entries(
        &self,
        conn: &mut Conn,
        at: Where,
        prefix: &str,
    ) -> Result<Vec<Entry>, EngineError> {
        if !self.directory_at(at) {
            return Ok(Vec::new());
        }
        match at {
            Where::DocumentFiles(document) => {
                let mut entries = Vec::new();
                for file in document_rows::files(conn, document).await? {
                    let mut entry = Entry::bare(
                        path_of(prefix, &file.name, false),
                        file.name,
                        SharedEntryKind::File,
                        file.id,
                        Where::DocumentFile {
                            document,
                            file: file.id,
                        },
                    );
                    entry.size = Some(file.size);
                    entry.description = Some(file.media_type);
                    entry.updated_at = Some(Time::new(file.added_at));
                    entries.push(entry);
                }
                Ok(entries)
            }
            Where::ChatFiles(chat) => {
                let total = rows::chat_total(conn, chat).await?;
                let messages = rows::chat_messages_numbered(conn, chat, 1, total.max(1)).await?;
                let mut entries = Vec::new();
                for message in messages {
                    if message.row.is_deleted() || message.row.kind != "message" {
                        continue;
                    }
                    let parts: Vec<ChatPart> = serde_json::from_value(message.row.content.clone())?;
                    entries.extend(carried_files(&parts).iter().enumerate().map(|(at, file)| {
                        Self::chat_file_entry(prefix, chat, &message, at + 1, file)
                    }));
                }
                Self::remember_files(conn, &entries).await?;
                Ok(entries)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// The final file component needs a DB read; ordinary paths remain synchronous and unchanged.
    pub(crate) async fn find_attachment(
        &self,
        conn: &mut Conn,
        named: &Named,
        consumed: usize,
    ) -> Result<Vec<Found>, EngineError> {
        let Named::Path { absolute, parts } = named else {
            if let Named::Id(file) = named {
                let stored: Option<(Id, String)> =
                    sqlx::query_as("SELECT document_id,name FROM document_files WHERE id=$1")
                        .bind(file)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(orch_db::DbError::from)?;
                if let Some((document, name)) = stored.filter(|(document, _)| {
                    self.document(*document)
                        .is_some_and(|doc| doc.standing.reads())
                }) && let Some(owner) = self.find_id(document)
                {
                    let parent = owner.path.rsplit_once('/').map_or("", |(parent, _)| parent);
                    let directory = crate::written::files_beside(&owner.path);
                    return Ok(vec![Found {
                        entry: FoundEntry::File(Where::DocumentFile {
                            document,
                            file: *file,
                        }),
                        path: format!("{parent}/{directory}/{name}"),
                    }]);
                }
                let saved: Option<(Id, Id, i32)> = sqlx::query_as(
                    "SELECT chat_id,message_id,carried FROM chat_file_references WHERE id=$1",
                )
                .bind(file)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
                if let Some((chat, message, carried)) = saved
                    && let Some(node) = self.chat_origin(chat)
                {
                    let prefix = match node {
                        Some(node) => self
                            .path_of_node(node)
                            .map(|path| format!("{path}chat.files/")),
                        None => self.find_id(chat).map(|found| {
                            format!("{}chat.files/", found.path.trim_end_matches("chat.md"))
                        }),
                    };
                    if let Some(prefix) = prefix {
                        let number = rows::chat_numbers(conn, chat, &[message])
                            .await?
                            .get(&message)
                            .copied();
                        if let Some(number) = number {
                            for message in
                                rows::chat_messages_numbered(conn, chat, number, number).await?
                            {
                                if message.row.is_deleted() {
                                    continue;
                                }
                                let parts: Vec<ChatPart> =
                                    serde_json::from_value(message.row.content.clone())?;
                                let carried = usize::try_from(carried).unwrap_or_default();
                                if let Some(file) =
                                    carried_files(&parts).get(carried.saturating_sub(1))
                                {
                                    let entry = Self::chat_file_entry(
                                        &prefix, chat, &message, carried, file,
                                    );
                                    return Ok(vec![Found {
                                        entry: FoundEntry::File(entry.at),
                                        path: entry.path,
                                    }]);
                                }
                            }
                        }
                    }
                }
            }
            return Ok(Vec::new());
        };
        let Some((filename, parent_parts)) = parts.split_last() else {
            return Ok(Vec::new());
        };
        let parent = Named::Path {
            absolute: *absolute,
            parts: parent_parts.to_vec(),
        };
        let mut found = Vec::new();
        for directory in self.find_all(&parent, consumed) {
            let FoundEntry::Directory(at @ (Where::DocumentFiles(_) | Where::ChatFiles(_))) =
                directory.entry
            else {
                continue;
            };
            if !self.directory_at(at) {
                continue;
            }
            let entries = if let Where::ChatFiles(chat) = at {
                let Some(number) = filename
                    .split_once('-')
                    .and_then(|(number, _)| number.parse::<i64>().ok())
                    .filter(|number| *number > 0)
                else {
                    continue;
                };
                let mut entries = Vec::new();
                for message in rows::chat_messages_numbered(conn, chat, number, number).await? {
                    if message.row.is_deleted() || message.row.kind != "message" {
                        continue;
                    }
                    let parts: Vec<ChatPart> = serde_json::from_value(message.row.content.clone())?;
                    entries.extend(carried_files(&parts).iter().enumerate().map(|(at, file)| {
                        Self::chat_file_entry(&directory.path, chat, &message, at + 1, file)
                    }));
                }
                entries
            } else {
                self.attachment_entries(conn, at, &directory.path).await?
            };
            Self::remember_files(conn, &entries).await?;
            found.extend(
                entries
                    .into_iter()
                    .filter(|entry| entry.name == *filename)
                    .map(|entry| Found {
                        entry: FoundEntry::File(entry.at),
                        path: entry.path,
                    }),
            );
        }
        Ok(found)
    }
}
