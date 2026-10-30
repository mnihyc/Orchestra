//! Media types from a file's name and its first bytes, and whether its content is text.
//!
//! A viewer is chosen by what a file is, and what it is comes from its extension; only for images
//! and PDF, whose extension is often missing or wrong, are the first bytes looked at as well. A
//! file is text when its first 8 KiB hold no NUL byte and read as UTF-8, or when they begin with a
//! UTF-16 byte-order mark.

pub use orch_types::media::{DIRECTORY, PLAIN_TEXT, SNIFF_BYTES, UNKNOWN, is_text, media_type};

/// The media type of a symbolic link whose target has no known type.
pub const SYMLINK: &str = "inode/symlink";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_what_reads_as_utf8_without_a_nul_or_starts_as_utf16() {
        assert!(is_text(b""));
        assert!(is_text("héllo wörld".as_bytes()));
        assert!(!is_text(b"abc\0def"));
        assert!(!is_text(&[0xff, 0x00, 0x80, 0x81, 0xfe]));
        assert!(
            is_text(&[0xff, 0xfe, b'h', 0, b'i', 0]),
            "a UTF-16 byte-order mark makes it text"
        );
        let cut = &"héllo".as_bytes()[..2];
        assert!(
            is_text(cut),
            "a character cut off by the end of the sample is not a fault of the file"
        );
    }

    #[test]
    fn the_extension_decides_and_only_images_and_pdf_are_known_by_their_shape() {
        assert_eq!(media_type("notes.MD", b"# hi"), "text/markdown");
        assert_eq!(media_type("data.json", b"{}"), "application/json");
        assert_eq!(media_type("Makefile", b"all:\n"), PLAIN_TEXT);
        assert_eq!(media_type("blob.bin", &[0, 1, 2, 3]), UNKNOWN);
        assert_eq!(
            media_type(
                "picture",
                &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0]
            ),
            "image/png"
        );
        assert_eq!(media_type("paper", b"%PDF-1.7\n%"), "application/pdf");
        assert_eq!(
            media_type("scan.png", b"%PDF-1.7"),
            "image/png",
            "the extension is believed over the bytes"
        );
        assert_eq!(media_type("photo", &[0xff, 0xd8, 0xff, 0xe0]), "image/jpeg");
    }
}
