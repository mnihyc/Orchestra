//! A turn owns at most one ordinary item page in memory; longer turns use a private reversible spool.
use super::{EngineError, ItemRow};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::sync::{Mutex, PoisonError};

#[derive(Default)]
pub(crate) struct Items {
    memory: Vec<ItemRow>,
    memory_bytes: usize,
    disk: Option<Mutex<std::fs::File>>,
    count: usize,
    bytes: u64,
    reversed: bool,
}
impl Items {
    pub(super) fn len(&self) -> usize {
        self.count
    }
    pub(super) fn held(&self) -> usize {
        if self.disk.is_none() { self.len() } else { 0 }
    }
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    pub(super) fn reverse(&mut self) {
        self.reversed = !self.reversed;
    }
    pub(super) fn push(&mut self, item: ItemRow) -> Result<(), EngineError> {
        struct Size(usize);
        impl Write for Size {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_add(bytes.len())
                    .ok_or_else(|| std::io::Error::other("turn size overflowed"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut size = Size(0);
        serde_json::to_writer(&mut size, &item)?;
        if self.disk.is_none()
            && self.memory.len() < super::cuts::PAGE as usize
            && self.memory_bytes.saturating_add(size.0)
                <= super::thread_history::PAGE_BYTES as usize
        {
            self.memory.push(item);
            self.memory_bytes += size.0;
            self.count += 1;
            return Ok(());
        }
        if self.disk.is_none() {
            let mut file = tempfile::tempfile().map_err(EngineError::unreadable)?;
            for old in self.memory.drain(..) {
                Self::write(&mut file, &old)?;
            }
            self.bytes = file.stream_position().map_err(EngineError::unreadable)?;
            self.disk = Some(Mutex::new(file));
            self.memory_bytes = 0;
        }
        if let Some(file) = &self.disk {
            let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
            Self::write(&mut file, &item)?;
            self.bytes = file.stream_position().map_err(EngineError::unreadable)?;
        }
        self.count += 1;
        Ok(())
    }
    fn write(file: &mut std::fs::File, item: &ItemRow) -> Result<(), EngineError> {
        let start = file
            .seek(SeekFrom::End(0))
            .map_err(EngineError::unreadable)?;
        file.write_all(&0_u64.to_le_bytes())
            .map_err(EngineError::unreadable)?;
        {
            let mut writer = BufWriter::new(&mut *file);
            serde_json::to_writer(&mut writer, item)?;
            writer.flush().map_err(EngineError::unreadable)?;
        }
        let end = file.stream_position().map_err(EngineError::unreadable)?;
        let length = end - start - 8;
        file.seek(SeekFrom::Start(start))
            .map_err(EngineError::unreadable)?;
        file.write_all(&length.to_le_bytes())
            .map_err(EngineError::unreadable)?;
        file.seek(SeekFrom::Start(end))
            .map_err(EngineError::unreadable)?;
        file.write_all(&length.to_le_bytes())
            .map_err(EngineError::unreadable)?;
        Ok(())
    }
    pub(super) fn append(&mut self, other: &mut Self) -> Result<(), EngineError> {
        let other = std::mem::take(other);
        for item in other.iter() {
            self.push(item?)?;
        }
        Ok(())
    }
    pub(crate) fn iter(&self) -> Iter<'_> {
        Iter {
            items: self,
            position: if self.reversed { self.bytes } else { 0 },
            read: 0,
        }
    }
    /// Explicit materialization is reserved for a caller whose own byte bound already admits it.
    pub(crate) fn into_vec(self) -> Result<Vec<ItemRow>, EngineError> {
        self.iter().collect()
    }
}
pub(crate) struct Iter<'a> {
    items: &'a Items,
    position: u64,
    read: usize,
}
impl Iterator for Iter<'_> {
    type Item = Result<ItemRow, EngineError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.read >= self.items.count {
            return None;
        }
        if let Some(disk) = &self.items.disk {
            let result = (|| {
                let mut file = disk.lock().unwrap_or_else(PoisonError::into_inner);
                let mut word = [0_u8; 8];
                let prefix = if self.items.reversed {
                    self.position.checked_sub(8).ok_or_else(|| {
                        EngineError::unreadable("the turn spool ended inside a record")
                    })?
                } else {
                    self.position
                };
                file.seek(SeekFrom::Start(prefix))
                    .map_err(EngineError::unreadable)?;
                file.read_exact(&mut word)
                    .map_err(EngineError::unreadable)?;
                let length = u64::from_le_bytes(word);
                let start = if self.items.reversed {
                    length
                        .checked_add(8)
                        .and_then(|bytes| prefix.checked_sub(bytes))
                        .ok_or_else(|| {
                            EngineError::unreadable("the turn spool record length is invalid")
                        })?
                } else {
                    prefix
                };
                if start
                    .checked_add(length)
                    .and_then(|end| end.checked_add(16))
                    .is_none_or(|end| end > self.items.bytes)
                {
                    return Err(EngineError::unreadable(
                        "the turn spool record exceeds its file",
                    ));
                }
                file.seek(SeekFrom::Start(start))
                    .map_err(EngineError::unreadable)?;
                file.read_exact(&mut word)
                    .map_err(EngineError::unreadable)?;
                if u64::from_le_bytes(word) != length {
                    return Err(EngineError::unreadable("the turn spool record changed"));
                }
                let item = serde_json::from_reader((&mut *file).take(length))?;
                file.read_exact(&mut word)
                    .map_err(EngineError::unreadable)?;
                if u64::from_le_bytes(word) != length {
                    return Err(EngineError::unreadable(
                        "the turn spool record is incomplete",
                    ));
                }
                self.position = if self.items.reversed {
                    start
                } else {
                    start + length + 16
                };
                Ok(item)
            })();
            self.read += 1;
            if result.is_err() {
                self.read = self.items.count;
            }
            Some(result)
        } else {
            let index = if self.items.reversed {
                self.items.count - self.read - 1
            } else {
                self.read
            };
            self.read += 1;
            Some(Ok(self.items.memory[index].clone()))
        }
    }
}
