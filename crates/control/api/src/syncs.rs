//! The state behind a durable feed, put together from one list of what that state consists of.
//!
//! A client that lost its place on a feed reads the state behind it over HTTP: everything the feed
//! carries, or what changed after the number the client holds. What a feed's state consists of is
//! a list of members, each of which says how it is read for a caller from a number on; the answer
//! is made by going down that list and is the same list for the first load and for the rest. A
//! part that gives a feed one more thing to carry adds a member to the list of that feed and a
//! member to the reply in the contract, and touches nothing else.
//!
//! The whole is read in one transaction that sees the database as it was when the feed's number
//! was read, so that no member hands over a row stamped above the number the answer ends with, and
//! none is missing that was stamped at or below it. The answer says it is `complete` only when
//! every member gave everything that changed: a member that finds more changed rows than a
//! request holds hands over none and says so, and the client reads again from the start. A number
//! the client holds above the feed's own (a database that was restored) is no number to go on
//! from: nothing is said to have changed and the answer is not complete.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use async_trait::async_trait;
use orch_authz::Principal;
use orch_db::{Conn, Tx};
use orch_types::{Id, Json, Seq};
use serde::de::DeserializeOwned;

/// What a member reads from, for one request.
pub struct Reading<'a> {
    /// The request.
    pub ctx: &'a Ctx,
    /// The connection of the transaction that reads the whole.
    pub conn: &'a mut Conn,
    /// The caller.
    pub principal: &'a Principal,
    /// Whether the environment is reached only through a thread: jobs then carry current progress.
    pub progress_only: bool,
    /// What the feed is of: a topic, an environment.
    pub subject: Id,
    /// Where the feed stands: no row stamped above it is handed over.
    pub seq: Seq,
    /// The number the client holds, if it holds one and it is no higher than the feed's; a member
    /// reads what was stamped after it, or everything where there is none.
    pub since: Option<Seq>,
}

/// What one member handed over.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// The member, as the reply writes it.
    pub value: Json,
    /// Whether it holds everything the client lacks. False where more changed than a request
    /// holds: the value is then empty and the client reads again from the start.
    pub complete: bool,
}

impl Piece {
    /// A member that holds everything the client lacks.
    #[must_use]
    pub const fn whole(value: Json) -> Self {
        Self {
            value,
            complete: true,
        }
    }

    /// A member that found too much changed to hand it over.
    #[must_use]
    pub const fn cut(empty: Json) -> Self {
        Self {
            value: empty,
            complete: false,
        }
    }
}

/// One thing the state behind a feed consists of.
#[async_trait]
pub trait Member: Sync {
    /// The name it has in the reply.
    fn name(&self) -> &'static str;

    /// Reads it for the caller from the number the client holds.
    async fn read(&self, reading: &mut Reading<'_>) -> ApiResult<Piece>;
}

/// Begins the transaction that reads a feed's state: one that sees the database as it was when it
/// began.
pub async fn begin(ctx: &Ctx) -> ApiResult<Tx> {
    Ok(orch_db::queries::feeds::begin_reading(&ctx.app.db).await?)
}

/// Makes the reply of `T` from the pieces of the members, the feed's number, the number the client
/// held and whether all is there.
/// Assembles members read by a feed-specific projection using the common cursor semantics.
pub(crate) fn assemble<T: DeserializeOwned>(
    pieces: Vec<(&'static str, Piece)>,
    seq: Seq,
    since: Option<Seq>,
    complete: bool,
) -> ApiResult<T> {
    let mut reply = serde_json::Map::new();
    let mut complete = complete;
    for (name, piece) in pieces {
        complete &= piece.complete;
        if reply.insert(name.to_owned(), piece.value).is_some() {
            return Err(ApiError::internal(format!(
                "two members of a feed's state are called {name}"
            )));
        }
    }
    reply.insert("seq".to_owned(), Json::from(seq));
    reply.insert("since".to_owned(), since.map_or(Json::Null, Json::from));
    reply.insert("complete".to_owned(), Json::Bool(complete));
    serde_json::from_value(Json::Object(reply)).map_err(ApiError::internal)
}

/// Puts the reply together by going down the list of members.
pub async fn put_together<T: DeserializeOwned>(
    members: &[&dyn Member],
    reading: &mut Reading<'_>,
) -> ApiResult<T> {
    let asked = reading.since;
    // A number above the feed's own names nothing to go on from.
    let ahead = asked.is_some_and(|since| since > reading.seq);
    if ahead {
        reading.since = Some(reading.seq);
    }
    let mut pieces = Vec::with_capacity(members.len());
    for member in members {
        pieces.push((member.name(), member.read(reading).await?));
    }
    assemble(pieces, reading.seq, asked, !ahead)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Reply {
        row: String,
        rows: Vec<i64>,
        seq: Seq,
        since: Option<Seq>,
        complete: bool,
    }

    fn pieces(rows: Piece) -> Vec<(&'static str, Piece)> {
        vec![("row", Piece::whole(json!("the row"))), ("rows", rows)]
    }

    #[test]
    fn the_reply_is_made_of_the_members_and_the_numbers_it_ends_with() {
        let made: Reply =
            assemble(pieces(Piece::whole(json!([1, 2]))), 17, Some(12), true).unwrap();
        assert_eq!(
            made,
            Reply {
                row: "the row".to_owned(),
                rows: vec![1, 2],
                seq: 17,
                since: Some(12),
                complete: true
            }
        );
    }

    #[test]
    fn one_member_that_found_too_much_makes_the_whole_incomplete() {
        let made: Reply = assemble(pieces(Piece::cut(json!([]))), 17, Some(3), true).unwrap();
        assert!(!made.complete);
        assert!(made.rows.is_empty());
        let ahead: Reply = assemble(pieces(Piece::whole(json!([]))), 17, Some(99), false).unwrap();
        assert!(
            !ahead.complete,
            "a number above the feed's is no place to go on from"
        );
        assert_eq!(
            ahead.since,
            Some(99),
            "the number that was asked is told back"
        );
    }

    #[test]
    fn two_members_of_one_name_are_a_defect_and_not_an_answer() {
        let twice = vec![
            ("row", Piece::whole(json!("a"))),
            ("row", Piece::whole(json!("b"))),
        ];
        assert!(assemble::<Reply>(twice, 1, None, true).is_err());
    }
}
