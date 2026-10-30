//! Images and files of messages as pinned blobs.
//!
//! An image or a file that a message carries is a blob of the store, held by
//! the hash that names it and pinned for as long as the message exists, so
//! that the store keeps what a chat shows. A message may carry only blobs the
//! store knows, an attachment says the size the store has of it, and none is
//! larger than `chat.attachment_max_bytes`. Taking the bytes in
//! (`POST …/attachments`) and giving them out (`GET …/blobs/{hash}`) belong to
//! the store itself, which is not connected here yet.

use crate::Error;
use orch_api_contract::chats::ChatPart;
use orch_db::Conn;
use orch_db::queries::chats::{self as rows, ChatRow};
use orch_settings::Place;
use orch_types::Hash;

/// The longest name of a file or an image.
pub const NAME_MAX_CHARS: usize = 255;

/// The blobs the parts carry as images and files, and in the copies they hold,
/// each once.
#[must_use]
pub fn blobs_in(parts: &[ChatPart]) -> Vec<Hash> {
    let mut found: Vec<Hash> = Vec::new();
    let mut note = |blob: &Hash| {
        if !found.contains(blob) {
            found.push(*blob);
        }
    };
    for part in parts {
        match part {
            ChatPart::PastedText(paste) => note(&paste.blob),
            ChatPart::Image { blob, .. } | ChatPart::Attachment { blob, .. } => note(blob),
            ChatPart::Forwarded(copy) => {
                copy.images.iter().for_each(|image| note(&image.blob));
                copy.attachments.iter().for_each(|file| note(&file.blob));
            }
            ChatPart::Text { .. } | ChatPart::ItemRef { .. } | ChatPart::Mention { .. } => {}
        }
    }
    found
}

/// The largest file a chat takes: `chat.attachment_max_bytes` as it resolves at
/// the chat's node, at its root org for a direct or a group chat.
pub async fn largest(conn: &mut Conn, chat: &ChatRow) -> Result<i64, Error> {
    let place = Place::Node(chat.node_id.unwrap_or(chat.org_id));
    Ok(
        orch_settings::get::<i64>(conn, place, "chat.attachment_max_bytes")
            .await?
            .unwrap_or(i64::MAX),
    )
}

fn bytes(hash: &Hash) -> Vec<u8> {
    hash.as_bytes().to_vec()
}

/// Checks the images and files the parts carry: the names are fit, the store
/// knows each blob, an attachment says the size the store holds, and none is
/// larger than `largest`. Copies made by forwarding are not checked for their
/// size again, only for being known.
pub async fn check(conn: &mut Conn, parts: &[ChatPart], largest: i64) -> Result<(), Error> {
    for (at, part) in parts.iter().enumerate() {
        match part {
            ChatPart::PastedText(paste) => {
                check_name(at, &paste.name)?;
                if paste.size < 0
                    || paste.size > largest
                    || paste.characters < 0
                    || paste.characters > paste.size
                    || paste.ascii_characters < 0
                    || paste.ascii_characters > paste.characters
                {
                    return Err(Error::invalid(
                        format!("parts[{at}]"),
                        "pasted text has its stored size and a valid character count",
                    ));
                }
            }
            ChatPart::Image { name, .. } => check_name(at, name)?,
            ChatPart::Attachment {
                name,
                size,
                media_type,
                ..
            } => {
                check_name(at, name)?;
                if *size < 0 || media_type.trim().is_empty() {
                    return Err(Error::invalid(
                        format!("parts[{at}]"),
                        "a file says its size and its type",
                    ));
                }
                if *size > largest {
                    return Err(Error::invalid(
                        format!("parts[{at}].size"),
                        format!("a file is at most {largest} bytes here"),
                    ));
                }
            }
            _ => {}
        }
    }
    let wanted = blobs_in(parts);
    if wanted.is_empty() {
        return Ok(());
    }
    let known = rows::blobs_known(conn, &wanted.iter().map(bytes).collect::<Vec<_>>()).await?;
    for (at, part) in parts.iter().enumerate() {
        let blob = match part {
            ChatPart::Image { blob, .. } | ChatPart::Attachment { blob, .. } => blob,
            ChatPart::PastedText(paste) => &paste.blob,
            _ => continue,
        };
        let stored = known.iter().find(|(hash, _)| *hash == bytes(blob));
        let Some((_, stored_size)) = stored else {
            return Err(Error::invalid(
                format!("parts[{at}].blob"),
                "is not a file the store holds",
            ));
        };
        if *stored_size > largest {
            return Err(Error::invalid(
                format!("parts[{at}].blob"),
                format!("is larger than the {largest} bytes a file is here"),
            ));
        }
        if let ChatPart::Attachment { size, .. } = part
            && size != stored_size
        {
            return Err(Error::invalid(
                format!("parts[{at}].size"),
                "is not the size of the file the store holds",
            ));
        }
        if let ChatPart::PastedText(paste) = part
            && paste.size != *stored_size
        {
            return Err(Error::invalid(
                format!("parts[{at}].size"),
                "is not the size of the stored pasted text",
            ));
        }
    }
    for blob in &wanted {
        if !known.iter().any(|(hash, _)| *hash == bytes(blob)) {
            return Err(Error::invalid(
                "parts",
                "a copy carries a file the store does not hold",
            ));
        }
    }
    Ok(())
}

fn check_name(at: usize, name: &str) -> Result<(), Error> {
    if name.trim().is_empty() || name.chars().count() > NAME_MAX_CHARS {
        return Err(Error::invalid(
            format!("parts[{at}].name"),
            format!("is a name of 1 to {NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(())
}

/// Pins the blobs of `parts` once more: a message carries them.
pub async fn pin(conn: &mut Conn, parts: &[ChatPart]) -> Result<(), Error> {
    let blobs = blobs_in(parts);
    if !blobs.is_empty() {
        rows::pin_blobs(conn, &blobs.iter().map(bytes).collect::<Vec<_>>(), 1).await?;
    }
    Ok(())
}

/// Pins the blobs of `parts` once fewer: a message no longer carries them.
pub async fn unpin(conn: &mut Conn, parts: &[ChatPart]) -> Result<(), Error> {
    let blobs = blobs_in(parts);
    if !blobs.is_empty() {
        rows::pin_blobs(conn, &blobs.iter().map(bytes).collect::<Vec<_>>(), -1).await?;
    }
    Ok(())
}
