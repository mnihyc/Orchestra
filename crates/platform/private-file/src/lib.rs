//! Immutable temporary text for consumers which need a whole string view without a heap-sized copy.
#![deny(unsafe_code)]
use std::io::{self, Read, Write};
#[allow(
    unsafe_code,
    reason = "only this platform boundary maps a uniquely owned immutable private file"
)]
mod platform;

/// A UTF-8 view whose backing file and map are private, immutable and owned until this value is dropped.
/// No path, writable file handle or mutable map is exposed.
pub struct PrivateText {
    map: Option<memmap2::Mmap>,
    _file: std::fs::File,
}
impl PrivateText {
    /// Writes through an opaque sink, flushes it, then maps the private backing file read-only.
    pub fn build(write: impl FnOnce(&mut dyn Write) -> io::Result<()>) -> io::Result<Self> {
        let mut file = tempfile::tempfile()?;
        {
            let mut sink = io::BufWriter::new(&mut file);
            write(&mut sink)?;
            sink.flush()?;
        }
        let map = if file.metadata()?.len() == 0 {
            None
        } else {
            let map = platform::map(&file)?;
            std::str::from_utf8(&map)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            Some(map)
        };
        Ok(Self { map, _file: file })
    }
    /// Copies a reader once into the same sealed private storage.
    pub fn from_reader(mut reader: impl Read) -> io::Result<Self> {
        Self::build(|sink| {
            io::copy(&mut reader, sink)?;
            Ok(())
        })
    }
    /// The complete immutable text. The operating system pages its backing file as needed.
    #[expect(
        clippy::expect_used,
        reason = "the constructor validates this private immutable map as UTF-8 and exposes no mutation handle"
    )]
    pub fn text(&self) -> &str {
        self.map.as_ref().map_or("", |map| {
            std::str::from_utf8(map).expect("the private text was validated")
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_text_preserves_empty_unicode_and_large_input_and_refuses_invalid_utf8()
    -> io::Result<()> {
        assert_eq!(PrivateText::from_reader(&b""[..])?.text(), "");
        let source = "a字🙂\n".repeat(20000);
        let kept = PrivateText::from_reader(source.as_bytes())?;
        assert_eq!(kept.text(), source);
        assert!(PrivateText::from_reader(&[255_u8][..]).is_err());
        assert!(
            PrivateText::build(|sink| {
                sink.write_all(b"partial")?;
                Err(io::Error::other("interrupted"))
            })
            .is_err()
        );
        assert_eq!(kept.text(), source);
        Ok(())
    }
}
