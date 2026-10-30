//! The files a document holds beside its text, of any kind, and their limits.
//!
//! A document holds images, data sets and scripts beside its text, kept by
//! the server and served to whoever may read the document. This is the part
//! of that which needs no storage: what a file may be called, which of them
//! an export carries inside the page (the images), which it names and
//! leaves (the rest), whether its bytes are text, and whether one more file
//! fits within what the document may hold. Where the bytes are kept is the
//! blob store's; callers provide the bytes and keep their storage references.

/// The most characters of a file's name.
pub const FILE_NAME_MAX_CHARS: usize = 255;

/// A file of a document, as an export or a reader asks about it. The bytes
/// are there when the caller has them to give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// Its name, unique in its document.
    pub name: String,
    /// Its type.
    pub media_type: String,
    /// Its size in bytes.
    pub size: i64,
    /// Whether its bytes are text.
    pub is_text: bool,
    /// Its bytes, where they are at hand.
    pub bytes: Option<Vec<u8>>,
}

impl FileInfo {
    /// Whether an export carries it inside the page: an image.
    #[must_use]
    pub fn is_image(&self) -> bool {
        is_image(&self.media_type)
    }
}

/// Whether a type is that of an image.
#[must_use]
pub fn is_image(media_type: &str) -> bool {
    media_type.to_ascii_lowercase().starts_with("image/")
}

/// Whether a type is that of a film or a sound.
#[must_use]
pub fn is_film_or_sound(media_type: &str) -> bool {
    let lower = media_type.to_ascii_lowercase();
    lower.starts_with("video/") || lower.starts_with("audio/")
}

/// A name as a document keeps it for a file: without the space around it, and
/// without anything that could lead out of a folder when it is unpacked, or
/// that does not belong in a name.
pub fn clean_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("must not be empty");
    }
    if name.chars().count() > FILE_NAME_MAX_CHARS {
        return Err("is too long");
    }
    if name == "." || name == ".." {
        return Err("must not be a folder");
    }
    if name.contains(['/', '\\']) {
        return Err("must not contain a slash");
    }
    if name.chars().any(char::is_control) {
        return Err("must not contain control characters");
    }
    Ok(name.to_owned())
}

/// A name for a file that none of `taken` has: the wanted one, or with a number before its ending (`a (2).png`). The
/// name chosen is put among `taken`.
pub fn unique_name(taken: &mut std::collections::HashSet<String>, wanted: &str) -> String {
    let name = clean_name(wanted).unwrap_or_else(|_| "file".to_owned());
    if taken.insert(name.clone()) {
        return name;
    }
    let (stem, ending) = match name.rsplit_once('.') {
        Some((stem, ending)) if !stem.is_empty() => (stem.to_owned(), format!(".{ending}")),
        _ => (name.clone(), String::new()),
    };
    let mut number = 2_u32;
    loop {
        let candidate = format!("{stem} ({number}){ending}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        number += 1;
    }
}

/// The type a file is served as: what its sender said when that is a type,
/// else what its name's ending says of the common kinds, else bytes.
#[must_use]
pub fn media_type_of(name: &str, declared: Option<&str>) -> String {
    if let Some(declared) = declared {
        let declared = declared.trim();
        let well_formed = declared.split_once('/').is_some_and(|(kind, sub)| {
            !kind.is_empty() && !sub.is_empty() && !declared.contains(char::is_whitespace)
        });
        if well_formed && declared != "application/octet-stream" {
            return declared.to_ascii_lowercase();
        }
    }
    let ending = name
        .rsplit_once('.')
        .map(|(_, ending)| ending.to_ascii_lowercase());
    let known = match ending.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("pdf") => "application/pdf",
        Some("json") => "application/json",
        Some("csv") => "text/csv",
        Some("md") => "text/markdown",
        Some("txt" | "log") => "text/plain",
        Some("html" | "htm") => "text/html",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    };
    known.to_owned()
}

/// Whether bytes are text, so that the file can be read and searched where
/// it lies: they decode as UTF-8 and hold no control character but the
/// ones that end a line or step to a tab stop.
#[must_use]
pub fn looks_like_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| {
        text.chars()
            .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
    })
}

/// What a document may hold in files, as resolved at its node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most bytes of one file.
    pub file_bytes: i64,
    /// The most bytes of all its files together.
    pub files_bytes: i64,
}

/// Why one more file does not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TooLarge {
    /// The file alone is over what one file may be.
    #[error("a file here is at most {limit} bytes")]
    File {
        /// The limit.
        limit: i64,
    },
    /// The files together would be over what they may come to.
    #[error("the files of a document here come to at most {limit} bytes")]
    Files {
        /// The limit.
        limit: i64,
    },
}

/// Whether a file of `size` bytes fits in a document whose files come to
/// `held` bytes now, `replacing` of them being a file of the same name that
/// it takes the place of.
pub fn check_fits(size: i64, held: i64, replacing: i64, limits: Limits) -> Result<(), TooLarge> {
    if size > limits.file_bytes {
        return Err(TooLarge::File {
            limit: limits.file_bytes,
        });
    }
    if held - replacing + size > limits.files_bytes {
        return Err(TooLarge::Files {
            limit: limits.files_bytes,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_could_lead_out_of_a_folder_is_not_a_name() {
        assert_eq!(clean_name("  plan.pdf ").unwrap(), "plan.pdf");
        for bad in ["", "   ", "..", ".", "a/b", "..\\x", "tab\tname", "nul\0"] {
            assert!(clean_name(bad).is_err(), "{bad:?}");
        }
        assert!(clean_name(&"x".repeat(FILE_NAME_MAX_CHARS + 1)).is_err());
        assert!(clean_name(&"x".repeat(FILE_NAME_MAX_CHARS)).is_ok());
    }

    #[test]
    fn a_name_that_is_taken_gets_a_number_before_its_ending() {
        let mut taken = std::collections::HashSet::new();
        assert_eq!(unique_name(&mut taken, "a.png"), "a.png");
        assert_eq!(unique_name(&mut taken, "a.png"), "a (2).png");
        assert_eq!(unique_name(&mut taken, "a.png"), "a (3).png");
        assert_eq!(unique_name(&mut taken, "notes"), "notes");
        assert_eq!(unique_name(&mut taken, "notes"), "notes (2)");
    }

    #[test]
    fn a_file_is_served_as_the_type_it_was_sent_with_or_the_one_its_name_ends_with() {
        assert_eq!(media_type_of("a.png", Some("Image/PNG")), "image/png");
        assert_eq!(media_type_of("a.png", None), "image/png");
        assert_eq!(
            media_type_of("a.png", Some("application/octet-stream")),
            "image/png"
        );
        assert_eq!(media_type_of("data", None), "application/octet-stream");
        assert_eq!(media_type_of("a.csv", Some("not a type")), "text/csv");
        assert!(is_image("image/svg+xml") && !is_image("video/mp4"));
        assert!(is_film_or_sound("Audio/mpeg") && is_film_or_sound("video/webm"));
    }

    #[test]
    fn bytes_are_text_when_they_decode_and_hold_nothing_that_is_no_text() {
        assert!(looks_like_text("line one\r\nline\ttwo é".as_bytes()));
        assert!(looks_like_text(b""));
        assert!(!looks_like_text(&[0x89, b'P', b'N', b'G']));
        assert!(!looks_like_text(b"a\0b"));
        assert!(!looks_like_text(b"\x1b[31mred"));
    }

    #[test]
    fn one_more_file_fits_when_it_and_the_rest_are_within_the_limits() {
        let limits = Limits {
            file_bytes: 100,
            files_bytes: 250,
        };
        assert_eq!(check_fits(100, 100, 0, limits), Ok(()));
        assert_eq!(
            check_fits(101, 0, 0, limits),
            Err(TooLarge::File { limit: 100 })
        );
        assert_eq!(
            check_fits(100, 151, 0, limits),
            Err(TooLarge::Files { limit: 250 })
        );
        assert_eq!(
            check_fits(100, 151, 50, limits),
            Ok(()),
            "a file that takes the place of one of the same name gives its room back"
        );
    }
}
