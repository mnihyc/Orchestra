//! A sequential disk log and cancellation for native history traversal.

pub(super) use orch_agent_contract::history::spool::Map;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;

const PAGE_BYTES: usize = 64 * 1024;

/// A private sequential disk log; cursor reads never retain earlier records.
#[derive(Debug)]
pub(super) struct Log<T> {
    file: File,
    end: u64,
    cancel: Arc<AtomicBool>,
    marker: std::marker::PhantomData<T>,
}

impl<T: Serialize + DeserializeOwned> Log<T> {
    pub(super) fn new(cancel: Arc<AtomicBool>) -> io::Result<Self> {
        Ok(Self {
            file: tempfile::tempfile()?,
            end: 0,
            cancel,
            marker: std::marker::PhantomData,
        })
    }
    pub(super) fn append(&mut self, value: &T) -> io::Result<u64> {
        check(&self.cancel)?;
        let offset = self.end;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(&0_u64.to_le_bytes())?;
        {
            let mut writer = BufWriter::with_capacity(PAGE_BYTES, &mut self.file);
            serde_json::to_writer(&mut writer, value).map_err(io::Error::other)?;
            writer.flush()?;
        }
        self.end = self.file.stream_position()?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file
            .write_all(&(self.end - offset - 8).to_le_bytes())?;
        Ok(offset)
    }
    pub(super) fn next(&mut self, cursor: &mut u64) -> io::Result<Option<T>> {
        check(&self.cancel)?;
        if *cursor == self.end {
            return Ok(None);
        }
        if *cursor > self.end {
            return Err(io::Error::other(
                "native history log cursor is outside its snapshot",
            ));
        }
        self.file.seek(SeekFrom::Start(*cursor))?;
        let mut length = [0; 8];
        self.file.read_exact(&mut length)?;
        let length = u64::from_le_bytes(length);
        let value = serde_json::from_reader(BufReader::with_capacity(
            PAGE_BYTES,
            (&mut self.file).take(length),
        ))
        .map_err(io::Error::other)?;
        *cursor += 8 + length;
        Ok(Some(value))
    }
    pub(super) fn at(&mut self, offset: u64) -> io::Result<T> {
        let mut cursor = offset;
        self.next(&mut cursor)?
            .ok_or_else(|| io::Error::other("native history log entry is missing"))
    }
}

pub(super) fn check(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "native history work was cancelled",
        ))
    } else {
        Ok(())
    }
}

pub(crate) struct CancelOnDrop(pub(crate) Option<Arc<AtomicBool>>);
impl CancelOnDrop {
    pub(crate) fn disarm(&mut self) {
        self.0 = None;
    }
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(cancel) = &self.0 {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}
