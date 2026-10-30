//! Bringing the discussions of a document: each opened on the block it was on, with its comments in their order, written by the people they are taken for, with the words of those who are not an account here under "originally by".

use super::{Batch, Made};
use crate::imports::identity;
use crate::imports::model::Object;
use crate::imports::remap::{Found, Maps, remap};
use orch_db::Tx;
use orch_db::queries::documents::discussions::{self, Answered, NewComment, Opened};
use orch_types::{Id, Json};
use serde_json::json;

use super::MakeError;

/// What is said before the words of somebody whose name is not an account here, in the person's own language where it is
/// known (the importer is the author, and their words come after the name of who wrote them there).
fn originally_by(name: &str) -> Json {
    json!({ "type": "text", "text": format!("{name}: "), "marks": ["italic"] })
}

fn parse_time(value: &Json) -> Option<chrono::DateTime<chrono::Utc>> {
    value.as_str()?.parse().ok()
}

/// Brings the discussions of a record, as many as can be.
pub(super) async fn bring(
    tx: &mut Tx,
    batch: &Batch<'_>,
    object: &Object,
    document: Id,
    discussions: &[Json],
    maps: &Maps<'_>,
    made: &mut Made,
) -> Result<(), MakeError> {
    let _ = object;
    let credits = &batch.plan.credits;
    for discussion in discussions {
        let Some(comments) = discussion.get("comments").and_then(Json::as_array) else {
            continue;
        };
        // What a discussion is about: the block it was on, as it is here, and the words it was made on.
        let block = discussion
            .get("block_id")
            .and_then(Json::as_str)
            .and_then(|text| text.parse::<Id>().ok())
            .map(|source| identity::block(batch.import, document, source));
        let quote = discussion
            .get("quote")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let mut opened: Option<Id> = None;
        for comment in comments {
            let Some(source) = comment
                .get("id")
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<Id>().ok())
            else {
                continue;
            };
            let derived = identity::inside(document, "comment", source);
            if discussions::comment(tx, document, derived).await?.is_some() {
                // Brought before.
                if opened.is_none() {
                    opened = discussions::comment(tx, document, derived)
                        .await?
                        .map(|row| row.discussion_id);
                }
                continue;
            }
            let writer = comment
                .get("author")
                .and_then(|author| author.get("id"))
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<Id>().ok());
            let credit = credits.of(writer);
            let mut content = comment
                .get("content")
                .cloned()
                .unwrap_or(Json::Array(Vec::new()));
            let mut found = Found::default();
            remap(&mut content, maps, &mut found);
            if let (Some(name), Some(parts)) = (credit.original.as_deref(), content.as_array_mut())
            {
                parts.insert(0, originally_by(name));
            }
            let at = comment
                .get("created_at")
                .and_then(parse_time)
                .unwrap_or(batch.now);
            let new = NewComment {
                id: derived,
                content: &content,
            };
            match opened {
                None => {
                    let Some(anchor) = block else {
                        made.warn(
                            "missing_file",
                            "a discussion that was about the whole document is not brought",
                        );
                        break;
                    };
                    match discussions::open(tx, document, anchor, quote, credit.user, &new, at)
                        .await?
                    {
                        Opened::Created(full) | Opened::Repeated(full) => {
                            opened = Some(full.discussion.id);
                            made.carried.comments += 1;
                        }
                        Opened::NoSuchBlock => {
                            made.warn(
                                "unsupported_block",
                                "a discussion was about a block that is not there and is not brought",
                            );
                            break;
                        }
                        Opened::Taken => break,
                    }
                }
                Some(thread) => {
                    match discussions::answer(tx, document, thread, credit.user, &new, at).await? {
                        Answered::Created { .. } | Answered::Repeated(_) => {
                            made.carried.comments += 1
                        }
                        Answered::NoSuchDiscussion | Answered::Taken => break,
                    }
                }
            }
        }
        // Whether it was resolved, and by whom, as far as that person is taken for an account.
        if let (Some(thread), Some(resolved)) = (
            opened,
            discussion.get("resolved").filter(|value| !value.is_null()),
        ) {
            let by = resolved
                .get("by")
                .and_then(|by| by.get("id"))
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<Id>().ok());
            let at = resolved.get("at").and_then(parse_time).unwrap_or(batch.now);
            let user = credits.of(by).user;
            discussions::resolve(tx, document, thread, true, user, at).await?;
        }
    }
    Ok(())
}
