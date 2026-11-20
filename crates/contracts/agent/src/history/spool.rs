//! Temporary indexed files keep historical item and tool correlation off the heap.

use std::collections::hash_map::RandomState;
use std::fs::File;
use std::hash::BuildHasher;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;

// A half-full hash table bounds expected probes; resizing copies one fixed-size slot at a time.
// The first index uses 24 KiB, below one I/O page, and grows only when its occupancy reaches one half.
const FIRST_SLOTS: u64 = 1024;
const SLOT_BYTES: u64 = 24;
const PAGE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Default)]
struct Slot {
    hash: u64,
    offset: u64,
    length: u64,
}

fn read_slot(file: &mut File, at: u64) -> io::Result<Slot> {
    file.seek(SeekFrom::Start(at * SLOT_BYTES))?;
    let mut bytes = [0_u8; SLOT_BYTES as usize];
    file.read_exact(&mut bytes)?;
    let number = |start| {
        let mut number = [0; 8];
        number.copy_from_slice(&bytes[start..start + 8]);
        u64::from_le_bytes(number)
    };
    Ok(Slot {
        hash: number(0),
        offset: number(8),
        length: number(16),
    })
}

fn write_slot(file: &mut File, at: u64, slot: Slot) -> io::Result<()> {
    file.seek(SeekFrom::Start(at * SLOT_BYTES))?;
    file.write_all(&slot.hash.to_le_bytes())?;
    file.write_all(&slot.offset.to_le_bytes())?;
    file.write_all(&slot.length.to_le_bytes())
}

/// A temporary disk hash table with one value decoded for each operation.
#[derive(Debug)]
pub struct Map<T> {
    cancel: Arc<AtomicBool>,
    index: File,
    values: File,
    hashes: RandomState,
    slots: u64,
    keys: u64,
    end: u64,
    marker: std::marker::PhantomData<T>,
}

impl<T: Serialize + DeserializeOwned> Map<T> {
    /// Opens an empty index backed by temporary files that close with the map.
    pub fn with_cancel(cancel: Arc<AtomicBool>) -> io::Result<Self> {
        let index = tempfile::tempfile()?;
        index.set_len(FIRST_SLOTS * SLOT_BYTES)?;
        Ok(Self {
            cancel,
            index,
            values: tempfile::tempfile()?,
            hashes: RandomState::new(),
            slots: FIRST_SLOTS,
            keys: 0,
            end: 0,
            marker: std::marker::PhantomData,
        })
    }

    fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "history indexing was cancelled",
            ))
        } else {
            Ok(())
        }
    }

    fn key(&mut self, slot: Slot) -> io::Result<String> {
        self.values.seek(SeekFrom::Start(slot.offset))?;
        let mut length = [0; 8];
        self.values.read_exact(&mut length)?;
        let length = usize::try_from(u64::from_le_bytes(length)).map_err(io::Error::other)?;
        let mut bytes = vec![0; length];
        self.values.read_exact(&mut bytes)?;
        String::from_utf8(bytes).map_err(io::Error::other)
    }

    fn find(&mut self, key: &str, hash: u64) -> io::Result<(u64, Slot)> {
        let mut at = hash % self.slots;
        loop {
            self.check()?;
            let slot = read_slot(&mut self.index, at)?;
            if slot.length == 0 || (slot.hash == hash && self.key(slot)? == key) {
                return Ok((at, slot));
            }
            at = (at + 1) % self.slots;
        }
    }

    fn grow(&mut self) -> io::Result<()> {
        let slots = self
            .slots
            .checked_mul(2)
            .ok_or_else(|| io::Error::other("history index is too large"))?;
        let mut index = tempfile::tempfile()?;
        index.set_len(
            slots
                .checked_mul(SLOT_BYTES)
                .ok_or_else(|| io::Error::other("history index is too large"))?,
        )?;
        for previous in 0..self.slots {
            self.check()?;
            let slot = read_slot(&mut self.index, previous)?;
            if slot.length == 0 {
                continue;
            }
            let mut at = slot.hash % slots;
            while read_slot(&mut index, at)?.length != 0 {
                at = (at + 1) % slots;
            }
            write_slot(&mut index, at, slot)?;
        }
        self.index = index;
        self.slots = slots;
        Ok(())
    }

    /// Reads the current value of a key.
    pub fn get(&mut self, key: &str) -> io::Result<Option<T>> {
        let hash = self.hashes.hash_one(key);
        let (_, slot) = self.find(key, hash)?;
        if slot.length == 0 {
            return Ok(None);
        }
        // find has left the payload cursor immediately after this exact key.
        let used = 8 + u64::try_from(key.len()).map_err(io::Error::other)?;
        let input = (&mut self.values).take(slot.length - used);
        serde_json::from_reader(BufReader::with_capacity(PAGE_BYTES, input))
            .map_err(io::Error::other)
    }

    fn put(&mut self, key: &str, value: Option<&T>) -> io::Result<()> {
        let hash = self.hashes.hash_one(key);
        let (mut at, mut previous) = self.find(key, hash)?;
        if previous.length == 0 && self.keys >= self.slots / 2 {
            self.grow()?;
            (at, previous) = self.find(key, hash)?;
        }
        let offset = self.end;
        self.values.seek(SeekFrom::Start(offset))?;
        self.values.write_all(
            &u64::try_from(key.len())
                .map_err(io::Error::other)?
                .to_le_bytes(),
        )?;
        self.values.write_all(key.as_bytes())?;
        {
            let mut writer = BufWriter::with_capacity(PAGE_BYTES, &mut self.values);
            serde_json::to_writer(&mut writer, &value).map_err(io::Error::other)?;
            writer.flush()?;
        }
        self.end = self.values.stream_position()?;
        write_slot(
            &mut self.index,
            at,
            Slot {
                hash,
                offset,
                length: self.end - offset,
            },
        )?;
        if previous.length == 0 {
            self.keys += 1;
        }
        Ok(())
    }

    /// Stores a value, replacing any earlier value under the same key.
    pub fn insert(&mut self, key: &str, value: &T) -> io::Result<()> {
        self.put(key, Some(value))
    }

    /// Takes the current value out of the index.
    pub fn remove(&mut self, key: &str) -> io::Result<Option<T>> {
        let value = self.get(key)?;
        if value.is_some() {
            self.put(key, None)?;
        }
        Ok(value)
    }

    /// Reads the next live entry in slot order, starting with a cursor of zero.
    pub fn next(&mut self, cursor: &mut u64) -> io::Result<Option<(String, T)>> {
        while *cursor < self.slots {
            self.check()?;
            let slot = read_slot(&mut self.index, *cursor)?;
            *cursor += 1;
            if slot.length == 0 {
                continue;
            }
            let key = self.key(slot)?;
            let used = 8 + u64::try_from(key.len()).map_err(io::Error::other)?;
            let input = (&mut self.values).take(slot.length - used);
            let value: Option<T> =
                serde_json::from_reader(BufReader::with_capacity(PAGE_BYTES, input))
                    .map_err(io::Error::other)?;
            if let Some(value) = value {
                return Ok(Some((key, value)));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_growing_history_keeps_edits_skips_removed_items_and_stops_when_cancelled() -> io::Result<()>
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut map = Map::with_cancel(Arc::clone(&cancel))?;
        let boundary = FIRST_SLOTS / 2;
        for id in 0..boundary {
            map.insert(&id.to_string(), &id)?;
        }
        map.insert("0", &boundary)?;
        map.insert(&boundary.to_string(), &boundary)?;
        assert!(map.slots > FIRST_SLOTS);
        assert_eq!(map.get("0")?, Some(boundary));
        assert_eq!(map.remove("1")?, Some(1));
        assert_eq!(map.get("1")?, None);

        let mut cursor = 0;
        let mut remaining: std::collections::BTreeSet<_> = (0..=boundary)
            .filter(|id| *id != 1)
            .map(|id| (id.to_string(), if id == 0 { boundary } else { id }))
            .collect();
        while let Some(entry) = map.next(&mut cursor)? {
            assert!(remaining.remove(&entry));
        }
        assert!(remaining.is_empty());

        let mut cursor = 0;
        assert!(map.next(&mut cursor)?.is_some());
        cancel.store(true, Ordering::Relaxed);
        assert!(
            matches!(map.next(&mut cursor), Err(error) if error.kind() == io::ErrorKind::Interrupted)
        );
        Ok(())
    }
}
