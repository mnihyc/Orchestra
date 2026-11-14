//! Exact retained text intervals shared by native adapters, with bounded incremental capture.
use crate::events::{BlobRef, OutputRef};
use orch_types::Hash;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Standalone adapter default, matching the shipped output capture setting.
pub const DEFAULT_CAPTURE_BYTES: u64 = 67_108_864;

/// Last valid UTF-8 bytes within the chosen allowance, without a synthetic gap marker.
#[must_use]
pub fn tail(text: &str, limit: u64) -> &str {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut cut = text.len().saturating_sub(limit);
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    &text[cut..]
}

/// Content-addressed native output cache, uploaded before its declared references are published.
#[derive(Debug, Clone)]
pub struct OutputStore {
    directory: PathBuf,
}
impl OutputStore {
    /// Stores immutable files below this native cache directory.
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
    /// Directory whose hash-named files the connector uploads.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    /// Atomically retains verified bytes; existing content must match its address.
    pub fn put(&self, bytes: &[u8]) -> std::io::Result<BlobRef> {
        use std::io::Write;
        let hash = Hash::of(bytes);
        let path = self.directory.join(hash.to_string());
        std::fs::create_dir_all(&self.directory)?;
        match std::fs::read(&path) {
            Ok(held) if held == bytes => {
                return Ok(BlobRef {
                    hash,
                    size: i64::try_from(bytes.len()).map_err(std::io::Error::other)?,
                });
            }
            Ok(_) => {
                return Err(std::io::Error::other(
                    "retained output does not match its hash",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::read(&path)? != bytes {
                    return Err(std::io::Error::other(
                        "retained output does not match its hash",
                    ));
                }
            }
            Err(error) => return Err(error.error),
        }
        Ok(BlobRef {
            hash,
            size: i64::try_from(bytes.len()).map_err(std::io::Error::other)?,
        })
    }
    /// Captures the selected tail while keeping the complete produced byte count and exact interval.
    pub fn retain(
        &self,
        text: &str,
        total: i64,
        limit: u64,
    ) -> std::io::Result<(OutputRef, Option<BlobRef>)> {
        let kept = tail(text, limit);
        let mut output = inline(kept, total);
        output.inline_limit = Some(limit.min(crate::events::OUTPUT_INLINE_MAX_BYTES as u64));
        let len = i64::try_from(kept.len()).map_err(std::io::Error::other)?;
        if total < len {
            return Err(std::io::Error::other(
                "retained output exceeds its produced byte count",
            ));
        }
        if kept.len() <= output.inline.len() {
            return Ok((output, None));
        }
        let blob = self.put(kept.as_bytes())?;
        output = output.with_retained_blob(blob.hash, total - len, len);
        Ok((output, Some(blob)))
    }
}

/// Inline fallback still records the original count, so unavailable capture never claims completeness.
#[must_use]
pub fn inline(text: &str, total: i64) -> OutputRef {
    let mut output = OutputRef::of(text);
    output.total_len = total;
    output.truncated = i64::try_from(output.inline.len()).unwrap_or(i64::MAX) < total;
    output
}

/// Incremental output owns at most the chosen tail, without copying it on every native fragment.
#[derive(Debug)]
pub struct OutputCapture {
    store: OutputStore,
    limit: u64,
    chunks: VecDeque<String>,
    kept: usize,
    total: i64,
}
impl OutputCapture {
    /// Starts a capture with the session's explicit allowance.
    #[must_use]
    pub fn new(store: OutputStore, limit: u64) -> Self {
        Self {
            store,
            limit,
            chunks: VecDeque::new(),
            kept: 0,
            total: 0,
        }
    }
    /// Retains each fragment once, evicting only bytes before the selected tail.
    pub fn push(&mut self, text: &str) {
        self.total = self
            .total
            .saturating_add(i64::try_from(text.len()).unwrap_or(i64::MAX));
        let limit = usize::try_from(self.limit).unwrap_or(usize::MAX);
        if text.len() >= limit {
            self.chunks.clear();
            self.kept = 0;
            let kept = tail(text, self.limit);
            if !kept.is_empty() {
                self.kept = kept.len();
                self.chunks.push_back(kept.to_owned());
            }
            return;
        }
        if !text.is_empty() {
            self.chunks.push_back(text.to_owned());
            self.kept += text.len();
        }
        while self.kept > limit {
            let Some(first) = self.chunks.front_mut() else {
                break;
            };
            let excess = self.kept - limit;
            if first.len() <= excess {
                self.kept -= first.len();
                self.chunks.pop_front();
            } else {
                let mut cut = excess;
                while !first.is_char_boundary(cut) {
                    cut += 1;
                }
                first.drain(..cut);
                self.kept -= cut;
            }
        }
    }
    /// Materializes the selected bytes once when a native item ends.
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::with_capacity(self.kept);
        for chunk in &self.chunks {
            text.push_str(chunk);
        }
        text
    }
    /// Original produced byte count, independent of retained bytes.
    #[must_use]
    pub const fn total(&self) -> i64 {
        self.total
    }
    /// Retains its exact tail as an immutable blob at completion.
    pub fn finish(&self) -> std::io::Result<(OutputRef, Option<BlobRef>)> {
        self.store.retain(&self.text(), self.total, self.limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunked_unicode_capture_and_whole_capture_retain_the_same_exact_original_interval()
    -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = OutputStore::new(directory.path().into());
        let source = "prefix🙂字 tail\n".repeat(8_000);
        let limit = 32_771;
        let mut capture = OutputCapture::new(store.clone(), limit);
        for chunk in source.split_inclusive('\n') {
            capture.push(chunk);
        }
        let (streamed, _) = capture.finish()?;
        let (whole, _) = store.retain(
            &source,
            i64::try_from(source.len()).map_err(std::io::Error::other)?,
            limit,
        )?;
        assert_eq!(streamed, whole);
        let range = whole
            .retained
            .ok_or_else(|| std::io::Error::other("retained interval"))?;
        assert_eq!(range.offset + range.len, whole.total_len);
        assert!(range.len <= i64::try_from(limit).map_err(std::io::Error::other)?);
        assert_eq!(
            std::fs::read(
                store.directory().join(
                    whole
                        .blob
                        .ok_or_else(|| std::io::Error::other("retained blob"))?
                        .to_string()
                )
            )?,
            tail(&source, limit).as_bytes()
        );
        Ok(())
    }
}
