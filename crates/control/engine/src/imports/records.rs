//! Reading a large record of a package without holding it: a JSON object whose big members are arrays, or a JSON array, read from a file by a thread of its own and handed on a page of elements at a time through a channel of a few places.
//!
//! A thread of a hundred million tokens is a record of hundreds of megabytes, and a chat of a million messages is nearly as many
//! lines. The importer copies the entry to a scratch file (`Entries::spool`, which also checks its hash) and reads the file with this:
//! memory is one page of elements and the few places of the channel, whatever the record's size. A page ends at a number of elements
//! or at a number of bytes of the file read, whichever comes first, so that a record of a few very large elements is held to the same
//! bounds as one of many small ones. The members of an object come in the order the record has them (the export writes the small
//! ones, `projection`, `thread`, `segments`, before the large arrays), so a reader that needs the small ones to understand the large
//! ones has them first.

use serde::Deserialize as _;
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value as Json;
use std::fmt;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

/// How many pages wait for whoever writes them: a few, so that a slow database slows the reading down and no more than this many pages of
/// memory are ever held for it.
const PAGES_WAITING: usize = 2;

/// A piece of a record that is an object.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// A member that is not streamed, whole.
    Member(String, Json),
    /// Elements of a member that is streamed, in order; the last page of a member may be shorter than the others.
    Page(String, Vec<Json>),
}

/// The pieces of a record as they are read; an `Err` is the last thing in it, and ends the reading.
pub type Pieces = mpsc::Receiver<Result<Piece, String>>;

/// The pages of a record that is an array.
pub type Pages = mpsc::Receiver<Result<Vec<Json>, String>>;

/// How a record is cut into pages: at most `elements` of them and, once that many bytes of the file have been read since the page began,
/// whatever it holds by then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    /// The most elements in a page.
    pub elements: usize,
    /// The bytes of the file read after which a page ends.
    pub bytes: u64,
}

/// A reader that counts what is read through it, so that a page can end at a size and not only at a number of elements.
struct Counted<R> {
    inner: R,
    read: Arc<AtomicU64>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.read.fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }
}

/// Where a page began and how large it may be, for the visitors that cut one.
#[derive(Clone)]
struct Meter {
    cut: Cut,
    read: Arc<AtomicU64>,
}

impl Meter {
    /// What has been read so far.
    fn now(&self) -> u64 {
        self.read.load(Ordering::Relaxed)
    }

    /// Whether a page that began when `began` bytes had been read, and holds `held` elements, is full.
    fn full(&self, began: u64, held: usize) -> bool {
        held >= self.cut.elements.max(1)
            || self.now().saturating_sub(began) >= self.cut.bytes.max(1)
    }
}

/// Reads the object in `reader`, handing the members named in `streamed` on a page at a time and every other member whole, in the
/// order the record has them. `reader` is read from a thread of its own.
#[must_use]
pub fn read_object<R: Read + Send + 'static>(
    reader: R,
    streamed: &'static [&'static str],
    cut: Cut,
) -> Pieces {
    let (sender, receiver) = mpsc::channel(PAGES_WAITING);
    tokio::task::spawn_blocking(move || {
        let read = Arc::new(AtomicU64::new(0));
        let counted = Counted {
            inner: reader,
            read: Arc::clone(&read),
        };
        let mut parser = serde_json::Deserializer::from_reader(std::io::BufReader::new(counted));
        let top = Top {
            sender: &sender,
            streamed,
            meter: Meter { cut, read },
        };
        let outcome = serde::Deserializer::deserialize_map(&mut parser, top)
            .map_err(|error| error.to_string())
            .and_then(|()| parser.end().map_err(|error| error.to_string()));
        if let Err(error) = outcome {
            let _ = sender.blocking_send(Err(error));
        }
    });
    receiver
}

/// Reads the array in `reader` a page at a time. `reader` is read from a thread of its own.
#[must_use]
pub fn read_array<R: Read + Send + 'static>(reader: R, cut: Cut) -> Pages {
    let (sender, receiver) = mpsc::channel(PAGES_WAITING);
    tokio::task::spawn_blocking(move || {
        let read = Arc::new(AtomicU64::new(0));
        let counted = Counted {
            inner: reader,
            read: Arc::clone(&read),
        };
        let mut parser = serde_json::Deserializer::from_reader(std::io::BufReader::new(counted));
        let elements = Elements {
            sender: &sender,
            meter: Meter { cut, read },
        };
        let outcome = serde::Deserializer::deserialize_seq(&mut parser, elements)
            .map_err(|error| error.to_string())
            .and_then(|()| parser.end().map_err(|error| error.to_string()));
        if let Err(error) = outcome {
            let _ = sender.blocking_send(Err(error));
        }
    });
    receiver
}

struct Top<'a> {
    sender: &'a mpsc::Sender<Result<Piece, String>>,
    streamed: &'static [&'static str],
    meter: Meter,
}

impl<'de> Visitor<'de> for Top<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            if self.streamed.contains(&key.as_str()) {
                let name = key.clone();
                map.next_value_seed(Streamed {
                    sender: self.sender,
                    meter: self.meter.clone(),
                    name,
                })?;
            } else {
                let value: Json = map.next_value()?;
                // The reader has gone away: nothing more is wanted.
                if self
                    .sender
                    .blocking_send(Ok(Piece::Member(key, value)))
                    .is_err()
                {
                    return Err(serde::de::Error::custom("the reader stopped"));
                }
            }
        }
        Ok(())
    }
}

struct Streamed<'a> {
    sender: &'a mpsc::Sender<Result<Piece, String>>,
    meter: Meter,
    name: String,
}

impl<'de> DeserializeSeed<'de> for Streamed<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        let name = self.name;
        deserializer.deserialize_any(MemberOrArray {
            sender: self.sender,
            meter: self.meter,
            name,
        })
    }
}

/// A member named to be streamed that is an array is streamed; anything else (null, a number, an object) is a member like the others.
struct MemberOrArray<'a> {
    sender: &'a mpsc::Sender<Result<Piece, String>>,
    meter: Meter,
    name: String,
}

impl<'de> Visitor<'de> for MemberOrArray<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let mut buffer: Vec<Json> = Vec::with_capacity(self.meter.cut.elements.clamp(1, 1024));
        let mut began = self.meter.now();
        while let Some(element) = seq.next_element::<Json>()? {
            buffer.push(element);
            if self.meter.full(began, buffer.len()) {
                let full = std::mem::take(&mut buffer);
                if self
                    .sender
                    .blocking_send(Ok(Piece::Page(self.name.clone(), full)))
                    .is_err()
                {
                    return Err(serde::de::Error::custom("the reader stopped"));
                }
                began = self.meter.now();
            }
        }
        if !buffer.is_empty()
            && self
                .sender
                .blocking_send(Ok(Piece::Page(self.name.clone(), buffer)))
                .is_err()
        {
            return Err(serde::de::Error::custom("the reader stopped"));
        }
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<(), A::Error> {
        let value = Json::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
        self.member(value)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        self.member(Json::Null)
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<(), E> {
        self.member(Json::Bool(value))
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<(), E> {
        self.member(Json::from(value))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<(), E> {
        self.member(Json::from(value))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<(), E> {
        self.member(Json::from(value))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<(), E> {
        self.member(Json::String(value.to_owned()))
    }
}

impl MemberOrArray<'_> {
    fn member<E: serde::de::Error>(&self, value: Json) -> Result<(), E> {
        self.sender
            .blocking_send(Ok(Piece::Member(self.name.clone(), value)))
            .map_err(|_| E::custom("the reader stopped"))
    }
}

struct Elements<'a> {
    sender: &'a mpsc::Sender<Result<Vec<Json>, String>>,
    meter: Meter,
}

impl<'de> Visitor<'de> for Elements<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON array")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let mut buffer: Vec<Json> = Vec::with_capacity(self.meter.cut.elements.clamp(1, 1024));
        let mut began = self.meter.now();
        while let Some(element) = seq.next_element::<Json>()? {
            buffer.push(element);
            if self.meter.full(began, buffer.len()) {
                let full = std::mem::take(&mut buffer);
                if self.sender.blocking_send(Ok(full)).is_err() {
                    return Err(serde::de::Error::custom("the reader stopped"));
                }
                began = self.meter.now();
            }
        }
        if !buffer.is_empty() && self.sender.blocking_send(Ok(buffer)).is_err() {
            return Err(serde::de::Error::custom("the reader stopped"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    /// Pages of at most `elements` elements, and of any size in bytes that a test of elements does not reach.
    const fn by(elements: usize) -> Cut {
        Cut {
            elements,
            bytes: u64::MAX,
        }
    }

    async fn all(mut pieces: Pieces) -> Result<Vec<Piece>, String> {
        let mut out = Vec::new();
        while let Some(piece) = pieces.recv().await {
            out.push(piece?);
        }
        Ok(out)
    }

    #[tokio::test]
    async fn the_big_arrays_of_an_object_come_a_page_at_a_time_and_the_other_members_whole_in_the_order_of_the_record()
     {
        let record = json!({
            "projection": { "revision": 3 },
            "thread": { "id": "t" },
            "items": [1, 2, 3, 4, 5],
            "note": "after",
        });
        let pieces = all(read_object(
            Cursor::new(serde_json::to_vec(&record).unwrap()),
            &["items"],
            by(2),
        ))
        .await
        .unwrap();
        assert_eq!(
            pieces,
            vec![
                Piece::Member("projection".into(), json!({ "revision": 3 })),
                Piece::Member("thread".into(), json!({ "id": "t" })),
                Piece::Page("items".into(), vec![json!(1), json!(2)]),
                Piece::Page("items".into(), vec![json!(3), json!(4)]),
                Piece::Page("items".into(), vec![json!(5)]),
                Piece::Member("note".into(), json!("after")),
            ]
        );
    }

    #[tokio::test]
    async fn a_streamed_member_that_is_no_array_is_a_member_like_the_others_and_an_empty_array_sends_nothing()
     {
        let record =
            br#"{"items": null, "turns": [], "segments": {"a": 1}, "n": 7, "ok": true, "s": "x"}"#;
        let pieces = all(read_object(
            Cursor::new(record.to_vec()),
            &["items", "turns", "segments"],
            by(10),
        ))
        .await
        .unwrap();
        assert_eq!(
            pieces,
            vec![
                Piece::Member("items".into(), Json::Null),
                Piece::Member("segments".into(), json!({ "a": 1 })),
                Piece::Member("n".into(), json!(7)),
                Piece::Member("ok".into(), json!(true)),
                Piece::Member("s".into(), json!("x")),
            ]
        );
    }

    #[tokio::test]
    async fn a_record_that_is_cut_short_or_has_more_after_it_is_an_error_at_the_end_of_what_was_read()
     {
        let cut = all(read_object(
            Cursor::new(br#"{"items": [1, 2"#.to_vec()),
            &["items"],
            by(10),
        ))
        .await;
        assert!(cut.is_err());
        let more = all(read_object(
            Cursor::new(br#"{"items": [1]} {"x": 1}"#.to_vec()),
            &["items"],
            by(10),
        ))
        .await;
        assert!(
            more.is_err(),
            "a second value after the record is not part of it"
        );
        let not_an_object = all(read_object(
            Cursor::new(b"[1, 2]".to_vec()),
            &["items"],
            by(10),
        ))
        .await;
        assert!(not_an_object.is_err());
    }

    #[tokio::test]
    async fn an_array_comes_a_page_at_a_time() {
        let record: Vec<Json> = (0..10).map(|n| json!({ "n": n })).collect();
        let mut pages = read_array(Cursor::new(serde_json::to_vec(&record).unwrap()), by(4));
        let mut sizes = Vec::new();
        let mut seen = 0_i64;
        while let Some(page) = pages.recv().await {
            let page = page.unwrap();
            sizes.push(page.len());
            for element in page {
                assert_eq!(element["n"], seen);
                seen += 1;
            }
        }
        assert_eq!(sizes, vec![4, 4, 2]);
        assert_eq!(seen, 10);
    }

    #[tokio::test]
    async fn a_page_ends_at_a_size_too_so_that_a_few_very_large_elements_are_held_to_the_same_bounds()
     {
        // Ten elements of about twenty thousand bytes each: the buffer of the reader is 8 KiB, so a page that may take thirty thousand
        // bytes ends after two elements, not after the hundred the count would allow.
        let record: Vec<Json> = (0..10)
            .map(|n| json!({ "n": n, "text": "x".repeat(20_000) }))
            .collect();
        let mut pages = read_array(
            Cursor::new(serde_json::to_vec(&record).unwrap()),
            Cut {
                elements: 100,
                bytes: 30_000,
            },
        );
        let (mut sizes, mut seen) = (Vec::new(), 0_i64);
        while let Some(page) = pages.recv().await {
            let page = page.unwrap();
            sizes.push(page.len());
            for element in page {
                assert_eq!(element["n"], seen);
                seen += 1;
            }
        }
        assert_eq!(seen, 10);
        assert!(
            sizes.len() >= 4 && sizes.iter().all(|size| *size <= 3),
            "a page holds a few of them and not all: {sizes:?}"
        );
    }

    #[tokio::test]
    async fn a_large_record_is_never_held_whole_no_more_than_a_few_pages_wait_for_the_reader() {
        // A hundred thousand elements of a few hundred bytes: if the reader is slow the thread of the parser waits at the channel, it does
        // not run ahead and hold them.
        let mut text = String::from(r#"{"items":["#);
        for n in 0..100_000_u32 {
            if n > 0 {
                text.push(',');
            }
            text.push_str(&format!(r#"{{"n":{n},"text":"{}"}}"#, "x".repeat(200)));
        }
        text.push_str("]}");
        let mut pieces = read_object(Cursor::new(text.into_bytes()), &["items"], by(500));
        let mut pages = 0_usize;
        let mut elements = 0_usize;
        while let Some(piece) = pieces.recv().await {
            let Piece::Page(_, page) = piece.unwrap() else {
                panic!("only pages here");
            };
            assert!(page.len() <= 500, "a page holds what a page holds");
            pages += 1;
            elements += page.len();
            tokio::task::yield_now().await;
        }
        assert_eq!((pages, elements), (200, 100_000));
    }
}
