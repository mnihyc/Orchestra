//! Exact provider performance ranges read disjoint timestamp prefixes without a ledger scan or write lock.

use super::performance::Sums;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

// Four bits per level trade sixteen folds per observation for a small range decomposition.
// The height follows the signed microsecond representation, not a time or request limit.
const PREFIX_BITS: u32 = 4;
const LEVELS: u32 = i64::BITS / PREFIX_BITS;

#[derive(Debug, serde::Serialize)]
struct Prefix {
    level: i16,
    prefix: i64,
}

fn invalid(error: impl std::fmt::Display) -> DbError {
    DbError::Other(sqlx::Error::Protocol(error.to_string()))
}

fn cover(from: i64, until: i64) -> Result<Vec<Prefix>, DbError> {
    let mut start = i128::from(from);
    let end = i128::from(until);
    let mut result = Vec::new();
    while start < end {
        let level = (0..LEVELS)
            .rev()
            .find(|level| {
                let span = 1_i128 << (level * PREFIX_BITS);
                start.rem_euclid(span) == 0 && start + span <= end
            })
            .ok_or_else(|| invalid("the timestamp range has no representable prefix"))?;
        let span = 1_i128 << (level * PREFIX_BITS);
        result.push(Prefix {
            level: i16::try_from(level).map_err(invalid)?,
            prefix: i64::try_from(start.div_euclid(span)).map_err(invalid)?,
        });
        start += span;
    }
    Ok(result)
}

pub(super) async fn read(
    conn: &mut Conn,
    provider: Id,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Sums, DbError> {
    // Use the database's own timestamp encoding, including sub-microsecond caller values.
    let (from,until):(i64,i64)=sqlx::query_as("SELECT (extract(epoch FROM $1::timestamptz)*1000000)::bigint,(extract(epoch FROM $2::timestamptz)*1000000)::bigint")
        .bind(from).bind(until).fetch_one(&mut *conn).await?;
    let ranges = cover(from, until)?;
    let mut total = Sums {
        errors: serde_json::json!({}),
        speed_buckets: serde_json::json!({}),
        first_token_buckets: serde_json::json!({}),
        ..Sums::default()
    };
    if ranges.is_empty() {
        return Ok(total);
    }
    let rows: Vec<Json> = sqlx::query_scalar("SELECT n.totals FROM jsonb_to_recordset($2) AS wanted(level smallint,prefix bigint) JOIN usage_health_tree n ON n.level=wanted.level AND n.prefix=wanted.prefix WHERE n.provider_id=$1")
        .bind(provider).bind(serde_json::to_value(ranges).map_err(invalid)?).fetch_all(conn).await?;
    for value in rows {
        let mut part: Sums = serde_json::from_value(value).map_err(invalid)?;
        if let Some(outcomes) = part.errors.as_object_mut() {
            outcomes.retain(|code, _| orch_api_contract::usage::provider_error(code));
        }
        total.merge_checked(&part).ok_or_else(|| invalid("a provider performance range is inconsistent or exceeds its integer representation"))?;
    }
    Ok(total)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;

    #[test]
    fn timestamp_prefixes_cover_signed_ranges_once_including_both_representation_edges() {
        for (from, until) in [
            (0, 1),
            (-1, 1),
            (-1234567, 7654321),
            (i64::MIN, i64::MIN + 1000000),
            (i64::MAX - 1000000, i64::MAX),
            (i64::MIN, i64::MAX),
        ] {
            let ranges = cover(from, until).expect("a signed timestamp range is representable");
            let mut next = i128::from(from);
            for range in ranges {
                let span = 1_i128
                    << (u32::try_from(range.level).expect("a nonnegative level") * PREFIX_BITS);
                assert_eq!(
                    i128::from(range.prefix) * span,
                    next,
                    "no gap or overlapping prefix"
                );
                next += span;
            }
            assert_eq!(
                next,
                i128::from(until),
                "the exclusive endpoint stays exact"
            );
        }
        assert!(cover(1, 1).expect("an empty range").is_empty());
        assert!(cover(2, 1).expect("a reversed range").is_empty());
    }
}
