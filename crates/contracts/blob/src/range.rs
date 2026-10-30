//! Range reads and content metadata.
//!
//! A blob is read whole or by a range of its bytes. The range is stated the way an HTTP request states it
//! (`Range: bytes=0-99`, `bytes=500-`, `bytes=-500`: [`ByteRange`]) and is resolved against the blob's size
//! into the bytes that are read ([`ResolvedRange`]), with the arithmetic of RFC 9110: an end past the content
//! is cut to its last byte; a suffix longer than the content is the whole of it; a start at or past the end,
//! or a suffix of nothing, cannot be satisfied.
//!
//! A single range only. A header that is not `bytes`, that cannot be read, or that asks for several ranges
//! is ignored by the server, which answers with the whole content ([`ByteRange::from_header`]).

use orch_types::Hash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What is known of a stored blob without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct BlobMeta {
    /// The BLAKE3 hash of its content: its name.
    pub hash: Hash,
    /// Its size in bytes.
    pub size: i64,
}

/// A range of bytes as a request states it. Positions count from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ByteRange {
    /// From `start` to the end: `bytes=500-`.
    From {
        /// The first byte.
        start: i64,
    },
    /// From `start` to `end`, both included: `bytes=0-99`.
    Between {
        /// The first byte.
        start: i64,
        /// The last byte.
        end: i64,
    },
    /// The last `len` bytes: `bytes=-500`.
    Last {
        /// How many.
        len: i64,
    },
}

/// The bytes a range comes to against a blob of a known size: `len` bytes from `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedRange {
    /// The first byte.
    pub start: i64,
    /// How many bytes. At least 1 for a range of a blob that has bytes, and 0 only for the whole of an empty
    /// blob.
    pub len: i64,
}

/// Why a range cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RangeError {
    /// The header is not in the unit `bytes`.
    #[error("only ranges of bytes are served")]
    NotBytes,
    /// The header cannot be read.
    #[error("the range cannot be read")]
    Syntax,
    /// The header asks for several ranges; one is served.
    #[error("several ranges are asked for and one is served")]
    Several,
    /// The range holds no byte of a blob of this size.
    #[error("the range holds no byte of a blob of {size} bytes")]
    Unsatisfiable {
        /// The blob's size, which a `416` answer states.
        size: i64,
    },
}

impl RangeError {
    /// Whether the server ignores the header and answers with the whole content, as it does for a range it
    /// does not understand. Only an unsatisfiable range is refused.
    #[must_use]
    pub const fn is_ignored(self) -> bool {
        !matches!(self, Self::Unsatisfiable { .. })
    }
}

fn number(text: &str) -> Result<i64, RangeError> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(RangeError::Syntax);
    }
    text.parse().map_err(|_| RangeError::Syntax)
}

impl ByteRange {
    /// Reads the value of a `Range` header: `bytes=a-b`, `bytes=a-` or `bytes=-n`, a single range.
    pub fn parse(header: &str) -> Result<Self, RangeError> {
        let spec = header
            .trim()
            .strip_prefix("bytes=")
            .ok_or(RangeError::NotBytes)?;
        if spec.contains(',') {
            return Err(RangeError::Several);
        }
        let (first, last) = spec.trim().split_once('-').ok_or(RangeError::Syntax)?;
        match (first.trim(), last.trim()) {
            ("", "") => Err(RangeError::Syntax),
            ("", len) => Ok(Self::Last { len: number(len)? }),
            (start, "") => Ok(Self::From {
                start: number(start)?,
            }),
            (start, end) => {
                let (start, end) = (number(start)?, number(end)?);
                if start > end {
                    Err(RangeError::Syntax)
                } else {
                    Ok(Self::Between { start, end })
                }
            }
        }
    }

    /// The range of a `Range` header, or `None` when the server ignores the header and serves the whole.
    #[must_use]
    pub fn from_header(header: &str) -> Option<Self> {
        Self::parse(header).ok()
    }

    /// The bytes this comes to in a blob of `size` bytes.
    pub fn resolve(self, size: i64) -> Result<ResolvedRange, RangeError> {
        let none = RangeError::Unsatisfiable { size };
        let (start, end) = match self {
            Self::From { start } if start < size => (start, size - 1),
            Self::Between { start, end } if start <= end && start < size => {
                (start, end.min(size - 1))
            }
            Self::Last { len } if len > 0 && size > 0 => ((size - len).max(0), size - 1),
            _ => return Err(none),
        };
        Ok(ResolvedRange {
            start,
            len: end - start + 1,
        })
    }
}

impl ResolvedRange {
    /// The whole of a blob of `size` bytes.
    #[must_use]
    pub const fn whole(size: i64) -> Self {
        Self {
            start: 0,
            len: size,
        }
    }

    /// The last byte, included; one before the start for an empty range.
    #[must_use]
    pub const fn last(&self) -> i64 {
        self.start + self.len - 1
    }

    /// The position after the last byte.
    #[must_use]
    pub const fn end(&self) -> i64 {
        self.start + self.len
    }

    /// Whether it is the whole of a blob of `size` bytes.
    #[must_use]
    pub const fn is_whole(&self, size: i64) -> bool {
        self.start == 0 && self.len == size
    }

    /// The `Content-Range` header of the answer: `bytes 0-99/1234`.
    #[must_use]
    pub fn content_range(&self, size: i64) -> String {
        format!("bytes {}-{}/{size}", self.start, self.last())
    }
}

/// The `Content-Range` header of a `416` answer for a blob of `size` bytes: `bytes */1234`.
#[must_use]
pub fn unsatisfied_content_range(size: i64) -> String {
    format!("bytes */{size}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_range_header_is_read_in_its_three_forms() {
        assert_eq!(
            ByteRange::parse("bytes=0-99"),
            Ok(ByteRange::Between { start: 0, end: 99 })
        );
        assert_eq!(
            ByteRange::parse("bytes=500-"),
            Ok(ByteRange::From { start: 500 })
        );
        assert_eq!(
            ByteRange::parse("bytes=-500"),
            Ok(ByteRange::Last { len: 500 })
        );
        assert_eq!(
            ByteRange::parse(" bytes= 5 - 9 "),
            Ok(ByteRange::Between { start: 5, end: 9 })
        );
        assert_eq!(
            ByteRange::parse("bytes=7-7"),
            Ok(ByteRange::Between { start: 7, end: 7 })
        );
    }

    #[test]
    fn a_header_that_cannot_be_served_is_ignored_and_the_whole_is_answered() {
        for (header, error) in [
            ("items=0-9", RangeError::NotBytes),
            ("bytes=0-9,20-29", RangeError::Several),
            ("bytes=9-0", RangeError::Syntax),
            ("bytes=-", RangeError::Syntax),
            ("bytes=a-b", RangeError::Syntax),
            ("bytes=5", RangeError::Syntax),
            ("bytes=-5-", RangeError::Syntax),
            ("bytes=99999999999999999999-", RangeError::Syntax),
        ] {
            assert_eq!(ByteRange::parse(header), Err(error), "{header}");
            assert!(error.is_ignored());
            assert_eq!(ByteRange::from_header(header), None);
        }
        assert!(!RangeError::Unsatisfiable { size: 1 }.is_ignored());
    }

    #[test]
    fn a_range_comes_to_the_bytes_of_the_blob_it_is_asked_of() {
        let at = |range: &str, size| ByteRange::parse(range).unwrap().resolve(size);
        assert_eq!(
            at("bytes=0-99", 1234),
            Ok(ResolvedRange { start: 0, len: 100 })
        );
        assert_eq!(
            at("bytes=1200-", 1234),
            Ok(ResolvedRange {
                start: 1200,
                len: 34
            })
        );
        assert_eq!(
            at("bytes=-500", 1234),
            Ok(ResolvedRange {
                start: 734,
                len: 500
            })
        );
        assert_eq!(
            at("bytes=1000-9999", 1234),
            Ok(ResolvedRange {
                start: 1000,
                len: 234
            }),
            "an end past the content is cut"
        );
        assert_eq!(
            at("bytes=-5000", 1234),
            Ok(ResolvedRange::whole(1234)),
            "a suffix longer than the content is all of it"
        );
        assert_eq!(at("bytes=0-", 1), Ok(ResolvedRange { start: 0, len: 1 }));
        assert_eq!(
            at("bytes=1233-1233", 1234),
            Ok(ResolvedRange {
                start: 1233,
                len: 1
            })
        );
    }

    #[test]
    fn a_range_that_holds_no_byte_is_refused_and_says_how_large_the_blob_is() {
        let size = 1234;
        for header in [
            "bytes=1234-",
            "bytes=1234-1300",
            "bytes=5000-6000",
            "bytes=-0",
        ] {
            assert_eq!(
                ByteRange::parse(header).unwrap().resolve(size),
                Err(RangeError::Unsatisfiable { size }),
                "{header}"
            );
        }
        assert_eq!(
            ByteRange::From { start: 0 }.resolve(0),
            Err(RangeError::Unsatisfiable { size: 0 }),
            "nothing to serve in an empty blob"
        );
        assert_eq!(
            ByteRange::Last { len: 5 }.resolve(0),
            Err(RangeError::Unsatisfiable { size: 0 })
        );
        assert_eq!(unsatisfied_content_range(size), "bytes */1234");
    }

    #[test]
    fn what_is_read_is_the_bytes_between_the_start_and_the_end_and_is_told_in_content_range() {
        let range = ByteRange::parse("bytes=0-99")
            .unwrap()
            .resolve(1234)
            .unwrap();
        assert_eq!((range.last(), range.end()), (99, 100));
        assert_eq!(range.content_range(1234), "bytes 0-99/1234");
        assert!(!range.is_whole(1234) && ResolvedRange::whole(1234).is_whole(1234));
        assert!(ResolvedRange::whole(0).is_whole(0));
        // The ranges of a blob, read one after another, make the blob.
        let size = 10;
        let parts = [
            ByteRange::Between { start: 0, end: 3 },
            ByteRange::Between { start: 4, end: 7 },
            ByteRange::From { start: 8 },
        ];
        let total: i64 = parts
            .iter()
            .map(|part| part.resolve(size).unwrap().len)
            .sum();
        assert_eq!(total, size);
    }

    #[test]
    fn a_range_and_a_blobs_meta_are_written_as_data() {
        let range: ByteRange =
            serde_json::from_value(json!({ "kind": "between", "start": 0, "end": 9 })).unwrap();
        assert_eq!(range, ByteRange::Between { start: 0, end: 9 });
        let meta: BlobMeta =
            serde_json::from_value(json!({ "hash": "0".repeat(64), "size": 37309 })).unwrap();
        assert_eq!(meta.size, 37309);
        assert_eq!(serde_json::to_value(meta).unwrap()["hash"], "0".repeat(64));
    }
}
