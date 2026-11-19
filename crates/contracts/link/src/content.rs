//! Command-bound blob transfers.
//!
//! Content of any size does not travel in the session: a machine moves
//! it over HTTPS beside it, to the same host. Content is named by its
//! hash, goes in pieces, and can go on where it stopped. Nothing is
//! moved for its own sake: every request names a command that is open
//! for that machine, an upload is taken only for a kind of command that
//! uploads, and a download is served only for content that the command
//! itself lists.
//!
//! A piece is sized to the link, so that a thin line loses a few seconds
//! of work to a cut and no more. A transfer is given up for standing
//! still, and never for being slow.

use crate::commands::{CommandKind, Transfer};
use orch_types::{Hash, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The path under which content is read and written, before its hash.
pub const BLOBS_PATH: &str = "/link/v1/blobs";
/// The path that answers which of some content the system lacks.
pub const MISSING_PATH: &str = "/link/v1/blobs/missing";
/// The header that names the open command a request belongs to.
pub const COMMAND_HEADER: &str = "x-orch-command";
/// The header that says how many bytes of some content have arrived.
pub const RECEIVED_HEADER: &str = "x-orch-received";
/// The header that says, as `0` or `1`, whether content is whole.
pub const COMPLETE_HEADER: &str = "x-orch-complete";
/// The scheme under which a machine presents its session's token where
/// the connection carries no client certificate.
pub const AUTHORIZATION_SCHEME: &str = "OrchLink";
/// The status of an upload whose piece does not begin where the last
/// one ended. The answer says how much has arrived.
pub const STATUS_OFFSET_CONFLICT: u16 = 409;
/// The status of an upload whose bytes do not have the hash or the size
/// they were announced with. What arrived is thrown away.
pub const STATUS_HASH_MISMATCH: u16 = 422;
/// The code of that failure.
pub const HASH_MISMATCH: &str = "hash_mismatch";

/// The most bytes one request may carry.
pub const MAX_CHUNK_BYTES: i64 = 4 * 1024 * 1024;
/// The size of the first piece of a transfer, before anything is known
/// about the link.
pub const FIRST_CHUNK_BYTES: i64 = 256 * 1024;
/// The smallest piece: below it the requests themselves cost more than
/// what they carry.
pub const MIN_CHUNK_BYTES: i64 = 64 * 1024;
/// A piece is sized to this many seconds of what the link achieved, so
/// that a cut loses about that much work and progress is seen to move.
pub const CHUNK_SECONDS: i64 = 5;
/// By how much a piece shrinks after a request stood still.
pub const STALL_SHRINK: i64 = 2;
/// How long a request may go without a byte moving before it is given
/// up and the transfer picked up again from what has arrived.
pub const STALL_SECS: u32 = 60;
/// The most hashes one question about missing content may list.
pub const MAX_MISSING_HASHES: usize = 1_000;
/// The most transfers one machine runs at once.
pub const MAX_TRANSFERS_PER_MACHINE: u32 = 4;

/// The path of the content with `hash`.
#[must_use]
pub fn blob_path(hash: &Hash) -> String {
    format!("{BLOBS_PATH}/{hash}")
}

/// The `Authorization` header a machine sends with `session_token`.
#[must_use]
pub fn authorization(session_token: &str) -> String {
    format!("{AUTHORIZATION_SCHEME} {session_token}")
}

/// The session token in an `Authorization` header, when the header is
/// of the link's own scheme.
#[must_use]
pub fn session_token(authorization: &str) -> Option<&str> {
    let (scheme, token) = authorization.split_once(' ')?;
    (scheme == AUTHORIZATION_SCHEME && !token.is_empty()).then_some(token)
}

/// The query of an upload request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PutQuery {
    /// Where in the content this piece begins.
    pub offset: i64,
    /// The size of the whole content.
    pub size: i64,
}

/// How much of some content has arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct BlobProgress {
    /// How many bytes have arrived, from the first.
    pub received: i64,
    /// Whether that is all of it, and its hash was found right.
    pub complete: bool,
}

impl BlobProgress {
    /// What [`COMPLETE_HEADER`] carries.
    #[must_use]
    pub const fn complete_header(&self) -> &'static str {
        if self.complete { "1" } else { "0" }
    }

    /// The progress two headers state, when they state one.
    #[must_use]
    pub fn from_headers(received: &str, complete: &str) -> Option<Self> {
        let complete = match complete {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        let received = received.parse().ok().filter(|received| *received >= 0)?;
        Some(Self { received, complete })
    }
}

/// The question which of some content the system lacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MissingRequest {
    /// The hashes asked about, at most [`MAX_MISSING_HASHES`].
    pub hashes: Vec<Hash>,
}

/// The answer to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MissingReply {
    /// Those of the hashes asked about that the system does not hold.
    pub missing: Vec<Hash>,
}

/// Why a piece of an upload is not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChunkRefusal {
    /// It does not begin where the last one ended.
    #[error("the piece begins at another place than the {received} bytes that have arrived")]
    Offset {
        /// How many bytes have arrived.
        received: i64,
    },
    /// It carries more than one request may.
    #[error("the piece is larger than a request may carry")]
    ChunkTooLarge,
    /// The whole is larger than may be stored.
    #[error("the content is larger than may be stored")]
    BlobTooLarge,
    /// It reaches past the size the content was announced with.
    #[error("the piece reaches past the end of the content")]
    PastEnd,
}

/// Takes one piece of an upload of which `received` bytes have arrived:
/// `len` bytes, announced by `query`. Returns how much has arrived with
/// it, and whether that is all: the hash of the whole is then still to
/// be checked.
pub fn accept_chunk(
    received: i64,
    query: PutQuery,
    len: i64,
    max_blob_bytes: i64,
) -> Result<BlobProgress, ChunkRefusal> {
    if query.size < 0 || query.size > max_blob_bytes {
        return Err(ChunkRefusal::BlobTooLarge);
    }
    if query.offset != received {
        return Err(ChunkRefusal::Offset { received });
    }
    if len > MAX_CHUNK_BYTES {
        return Err(ChunkRefusal::ChunkTooLarge);
    }
    let end = query
        .offset
        .checked_add(len)
        .filter(|end| len >= 0 && *end <= query.size);
    let end = end.ok_or(ChunkRefusal::PastEnd)?;
    Ok(BlobProgress {
        received: end,
        complete: end == query.size,
    })
}

/// How many bytes the next piece of a transfer has, when the pieces
/// before it moved at `bytes_per_second`; `None` before any did.
#[must_use]
pub fn next_chunk_bytes(bytes_per_second: Option<i64>) -> i64 {
    match bytes_per_second {
        Some(rate) => rate
            .saturating_mul(CHUNK_SECONDS)
            .clamp(MIN_CHUNK_BYTES, MAX_CHUNK_BYTES),
        None => FIRST_CHUNK_BYTES,
    }
}

/// How many bytes the next piece has after one of `last` bytes stood
/// still and was given up.
#[must_use]
pub fn chunk_bytes_after_stall(last: i64) -> i64 {
    (last / STALL_SHRINK).clamp(MIN_CHUNK_BYTES, MAX_CHUNK_BYTES)
}

/// Whether content may be uploaded for an open command of `kind`.
#[must_use]
pub fn takes_uploads(kind: CommandKind) -> bool {
    kind.transfer() == Transfer::Upload
}

/// Whether content may be downloaded for an open command of `kind`.
#[must_use]
pub fn serves_downloads(kind: CommandKind) -> bool {
    kind.transfer() == Transfer::Download
}

/// The content a command's body lists: every hash that stands anywhere
/// in it. A download for that command is served for these and for no
/// other.
#[must_use]
pub fn listed_hashes(body: &Json) -> BTreeSet<Hash> {
    fn collect(value: &Json, found: &mut BTreeSet<Hash>) {
        match value {
            Json::String(text) => found.extend(text.parse::<Hash>()),
            Json::Array(entries) => entries.iter().for_each(|entry| collect(entry, found)),
            Json::Object(members) => members.values().for_each(|member| collect(member, found)),
            _ => {}
        }
    }
    let mut found = BTreeSet::new();
    collect(body, &mut found);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BLOB_LIMIT: i64 = 8 * 1024 * 1024 * 1024;

    #[test]
    fn requests_and_answers_read_and_write_as_the_protocol_has_them() {
        let (a, b) = (Hash::of(b"a"), Hash::of(b"b"));
        let asked: MissingRequest =
            serde_json::from_value(json!({ "hashes": [a.to_string(), b.to_string()] })).unwrap();
        assert_eq!(asked.hashes, [a, b]);
        let answered = MissingReply { missing: vec![b] };
        assert_eq!(
            serde_json::to_value(&answered).unwrap(),
            json!({ "missing": [b.to_string()] })
        );
        let progress: BlobProgress =
            serde_json::from_value(json!({ "received": 262144, "complete": false })).unwrap();
        assert_eq!(
            serde_json::to_value(progress).unwrap(),
            json!({ "received": 262144, "complete": false })
        );
        assert!(
            serde_json::from_value::<MissingRequest>(json!({ "hashes": [], "all": true })).is_err()
        );
        assert_eq!(blob_path(&a), format!("/link/v1/blobs/{a}"));
    }

    #[test]
    fn progress_is_stated_in_two_headers() {
        let progress = BlobProgress {
            received: 1_048_576,
            complete: true,
        };
        assert_eq!(progress.complete_header(), "1");
        assert_eq!(BlobProgress::from_headers("1048576", "1"), Some(progress));
        assert_eq!(
            BlobProgress::from_headers("0", "0"),
            Some(BlobProgress {
                received: 0,
                complete: false
            })
        );
        assert_eq!(BlobProgress::from_headers("many", "1"), None);
        assert_eq!(BlobProgress::from_headers("12", "yes"), None);
        assert_eq!(BlobProgress::from_headers("-1", "0"), None);
    }

    #[test]
    fn a_machine_without_a_client_certificate_presents_its_sessions_token() {
        assert_eq!(authorization("t0k3n"), "OrchLink t0k3n");
        assert_eq!(session_token("OrchLink t0k3n"), Some("t0k3n"));
        assert_eq!(session_token("Bearer t0k3n"), None);
        assert_eq!(session_token("OrchLink "), None);
    }

    #[test]
    fn an_upload_goes_on_exactly_where_it_stopped() {
        let size = 600_000;
        let first = accept_chunk(0, PutQuery { offset: 0, size }, 262_144, BLOB_LIMIT).unwrap();
        assert_eq!(
            first,
            BlobProgress {
                received: 262_144,
                complete: false
            }
        );
        let again = accept_chunk(
            first.received,
            PutQuery { offset: 0, size },
            262_144,
            BLOB_LIMIT,
        );
        assert_eq!(again, Err(ChunkRefusal::Offset { received: 262_144 }));
        let ahead = accept_chunk(
            first.received,
            PutQuery {
                offset: 300_000,
                size,
            },
            1_000,
            BLOB_LIMIT,
        );
        assert_eq!(ahead, Err(ChunkRefusal::Offset { received: 262_144 }));
        let rest = accept_chunk(
            first.received,
            PutQuery {
                offset: 262_144,
                size,
            },
            337_856,
            BLOB_LIMIT,
        )
        .unwrap();
        assert_eq!(
            rest,
            BlobProgress {
                received: size,
                complete: true
            }
        );
    }

    #[test]
    fn a_piece_is_refused_when_it_is_too_large_or_reaches_past_the_end() {
        let query = PutQuery {
            offset: 0,
            size: 10_000_000,
        };
        assert_eq!(
            accept_chunk(0, query, MAX_CHUNK_BYTES + 1, BLOB_LIMIT),
            Err(ChunkRefusal::ChunkTooLarge)
        );
        assert!(accept_chunk(0, query, MAX_CHUNK_BYTES, BLOB_LIMIT).is_ok());
        let small = PutQuery {
            offset: 0,
            size: 100,
        };
        assert_eq!(
            accept_chunk(0, small, 101, BLOB_LIMIT),
            Err(ChunkRefusal::PastEnd)
        );
    }

    #[test]
    fn content_larger_than_may_be_stored_is_refused_before_a_byte_is_taken() {
        let query = PutQuery {
            offset: 0,
            size: BLOB_LIMIT + 1,
        };
        assert_eq!(
            accept_chunk(0, query, 1_000, BLOB_LIMIT),
            Err(ChunkRefusal::BlobTooLarge)
        );
    }

    #[test]
    fn empty_content_is_whole_with_its_first_request() {
        let done = accept_chunk(0, PutQuery { offset: 0, size: 0 }, 0, BLOB_LIMIT).unwrap();
        assert_eq!(
            done,
            BlobProgress {
                received: 0,
                complete: true
            }
        );
    }

    #[test]
    fn a_piece_is_five_seconds_of_the_link_within_its_bounds() {
        assert_eq!(next_chunk_bytes(None), FIRST_CHUNK_BYTES);
        assert_eq!(next_chunk_bytes(Some(93_750)), 468_750, "750 kbit/s");
        assert_eq!(
            next_chunk_bytes(Some(1_000)),
            MIN_CHUNK_BYTES,
            "a thin line"
        );
        assert_eq!(
            next_chunk_bytes(Some(50_000_000)),
            MAX_CHUNK_BYTES,
            "a fast one"
        );
    }

    #[test]
    fn a_piece_that_stood_still_is_followed_by_a_smaller_one() {
        assert_eq!(
            chunk_bytes_after_stall(MAX_CHUNK_BYTES),
            MAX_CHUNK_BYTES / 2
        );
        assert_eq!(chunk_bytes_after_stall(MIN_CHUNK_BYTES), MIN_CHUNK_BYTES);
    }

    #[test]
    fn content_moves_only_for_the_kinds_of_command_that_move_it() {
        assert!(takes_uploads(CommandKind::FsFetch));
        assert!(takes_uploads(CommandKind::FsSnapshot));
        assert!(!takes_uploads(CommandKind::FsWrite));
        assert!(serves_downloads(CommandKind::FsWrite));
        assert!(serves_downloads(CommandKind::TurnStart));
        assert!(!serves_downloads(CommandKind::FsList));
        assert!(!takes_uploads(CommandKind::FsList));
    }

    #[test]
    fn a_download_is_for_content_the_command_itself_lists() {
        let (image, other) = (Hash::of(b"image"), Hash::of(b"other"));
        let body = json!({
            "thread_id": "0199c1a0-0000-7000-8000-000000000001", "text": "look at this",
            "images": [ { "blob": image.to_string(), "media_type": "image/png", "name": "shot.png" } ]
        });
        let listed = listed_hashes(&body);
        assert!(listed.contains(&image));
        assert!(!listed.contains(&other));
        assert_eq!(listed.len(), 1);
    }
}
