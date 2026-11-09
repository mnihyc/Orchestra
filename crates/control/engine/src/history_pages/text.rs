//! Written turns spill at the transcript copy quantum; this is buffering, never a content cap.
use super::EngineError;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::sync::{Mutex, PoisonError};
const BUFFER_BYTES: usize = 64 * 1024;
#[derive(Default)]
pub(crate) struct Text {
    memory: String,
    disk: Option<Mutex<BufWriter<std::fs::File>>>,
    bytes: usize,
    newlines: usize,
    ends_newline: bool,
}
impl Text {
    pub(crate) fn len(&self) -> usize {
        self.bytes
    }
    pub(crate) fn lines(&self) -> usize {
        self.newlines + usize::from(self.bytes > 0 && !self.ends_newline)
    }
    pub(crate) fn push(&mut self, piece: &str) -> Result<(), EngineError> {
        if self.disk.is_none() && self.bytes.saturating_add(piece.len()) <= BUFFER_BYTES {
            self.memory.push_str(piece);
        } else {
            if self.disk.is_none() {
                let mut file = BufWriter::with_capacity(
                    BUFFER_BYTES,
                    tempfile::tempfile().map_err(EngineError::unreadable)?,
                );
                file.write_all(self.memory.as_bytes())
                    .map_err(EngineError::unreadable)?;
                self.memory = String::new();
                self.disk = Some(Mutex::new(file));
            }
            if let Some(file) = &self.disk {
                file.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .write_all(piece.as_bytes())
                    .map_err(EngineError::unreadable)?;
            }
        }
        self.bytes = self
            .bytes
            .checked_add(piece.len())
            .ok_or_else(|| EngineError::unreadable("written history size overflowed"))?;
        self.newlines += piece.bytes().filter(|byte| *byte == b'\n').count();
        if let Some(last) = piece.as_bytes().last() {
            self.ends_newline = *last == b'\n';
        }
        Ok(())
    }
    pub(crate) fn with_reader<T>(
        &self,
        read: impl FnOnce(&mut dyn Read) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        if let Some(file) = &self.disk {
            let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
            file.flush().map_err(EngineError::unreadable)?;
            file.get_mut()
                .seek(SeekFrom::Start(0))
                .map_err(EngineError::unreadable)?;
            let result = read(file.get_mut());
            file.get_mut()
                .seek(SeekFrom::End(0))
                .map_err(EngineError::unreadable)?;
            result
        } else {
            read(&mut std::io::Cursor::new(self.memory.as_bytes()))
        }
    }
    pub(crate) fn copy_to(&self, output: &mut impl Write) -> Result<(), EngineError> {
        self.with_reader(|reader| {
            std::io::copy(reader, output).map_err(EngineError::unreadable)?;
            Ok(())
        })
    }
    pub(crate) fn read(&self, limit: Option<usize>) -> Result<String, EngineError> {
        let limit = limit.unwrap_or(self.bytes).min(self.bytes);
        self.with_reader(|reader| {
            let mut bytes = Vec::with_capacity(limit.min(BUFFER_BYTES));
            reader
                .take(limit as u64)
                .read_to_end(&mut bytes)
                .map_err(EngineError::unreadable)?;
            match String::from_utf8(bytes) {
                Ok(text) => Ok(text),
                Err(error) => {
                    if error.utf8_error().error_len().is_some() {
                        return Err(EngineError::unreadable(error));
                    }
                    let end = error.utf8_error().valid_up_to();
                    let mut bytes = error.into_bytes();
                    bytes.truncate(end);
                    String::from_utf8(bytes).map_err(EngineError::unreadable)
                }
            }
        })
    }
    pub(crate) fn prepend(self, prefix: &str) -> Result<Self, EngineError> {
        let mut output = Self::default();
        output.push(prefix)?;
        self.with_reader(|reader| {
            let mut carry = Vec::new();
            let mut block = [0_u8; BUFFER_BYTES];
            loop {
                let n = reader.read(&mut block).map_err(EngineError::unreadable)?;
                if n == 0 {
                    break;
                }
                carry.extend_from_slice(&block[..n]);
                let end = match std::str::from_utf8(&carry) {
                    Ok(_) => carry.len(),
                    Err(error) if error.error_len().is_none() => error.valid_up_to(),
                    Err(error) => return Err(EngineError::unreadable(error)),
                };
                output
                    .push(std::str::from_utf8(&carry[..end]).map_err(EngineError::unreadable)?)?;
                carry.drain(..end);
            }
            if !carry.is_empty() {
                return Err(EngineError::unreadable(
                    "a written spool ends inside a character",
                ));
            }
            Ok(())
        })?;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spilled_text_keeps_unicode_prefixes_lines_and_append_after_read() -> Result<(), EngineError>
    {
        let source = "字🙂\n".repeat(BUFFER_BYTES);
        let mut text = Text::default();
        text.push(&source)?;
        assert!(text.disk.is_some());
        assert_eq!(text.lines(), source.lines().count());
        assert_eq!(text.read(Some(5))?, "字");
        text.push("after")?;
        assert_eq!(text.read(None)?, source.clone() + "after");
        let text = text.prepend("head\n")?;
        assert_eq!(text.read(None)?, "head\n".to_owned() + &source + "after");
        Ok(())
    }
    #[test]
    fn spooled_masking_keeps_whole_text_pattern_order_and_assignment_words()
    -> Result<(), EngineError> {
        let mut text = Text::default();
        let middle = "x字\n".repeat(BUFFER_BYTES);
        for piece in [
            "before ",
            middle.as_str(),
            " after API_TOKEN=private-value\nplain=kept secret-value",
        ] {
            text.push(piece)?;
        }
        let redactor = orch_chat::redaction::Redactor::new(
            &[orch_chat::redaction::Pattern {
                name: "multiline".into(),
                regex: "(?s)before.*after".into(),
            }],
            vec!["secret-value".into()],
        );
        let expected = crate::raw_events::mask_text(&redactor, text.read(None)?);
        let actual = crate::raw_events::mask_spooled(&redactor, &text)?;
        assert_eq!(actual.read(None)?, expected);
        assert!(!expected.contains("secret-value"));
        assert!(!expected.contains("private-value"));
        assert_eq!(actual.lines(), expected.lines().count());
        Ok(())
    }
}
