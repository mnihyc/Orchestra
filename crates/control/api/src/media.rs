//! What a file is, from its name and its first bytes: its media type, whether its content is text, and a name made safe to put in a path.
//!
//! A viewer is chosen by what a file is, and what it is comes from its extension; only for images
//! and PDF, whose extension is often missing or wrong, are the first bytes looked at as well. A
//! file is text when its first 8 KiB hold no NUL byte and read as UTF-8, or begin with a UTF-16
//! byte-order mark. The environment's own machine makes the same judgement for a file it lists; this
//! is for what the API judges itself: a listing made from a list of the tree, an upload, and what
//! it serves.

pub use orch_types::media::{
    DIRECTORY, PLAIN_TEXT, SNIFF_BYTES, UNKNOWN, is_text, is_text_by_name, media_type,
};

/// The longest name a file keeps when it is stored under a name of its own: room remains for its prefix.
pub const NAME_MAX_CHARS: usize = 200;

/// Whether a file of this type may be shown inline, in the page that asked for it, without being
/// able to act in the page: active content (a page, a vector image with scripts) is served as
/// plain text or as a download, never as itself.
#[must_use]
pub fn shows_inline(media_type: &str) -> bool {
    let essence = media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    !matches!(
        essence.as_str(),
        "text/html"
            | "application/xhtml+xml"
            | "image/svg+xml"
            | "application/xml"
            | "text/xml"
            | "message/rfc822"
    ) && !essence.ends_with("+xml")
        && !essence.starts_with("multipart/")
}

/// A name that is safe to put in a path as one component: no separators, no control characters, no
/// leading dot, at most [`NAME_MAX_CHARS`] characters, and never empty.
#[must_use]
pub fn safe_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.');
    let mut name: String = trimmed.chars().take(NAME_MAX_CHARS).collect();
    if name.is_empty() {
        name = "file".to_owned();
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_decides_the_type_and_the_first_bytes_decide_where_the_name_does_not() {
        assert_eq!(media_type("photo.JPG", b""), "image/jpeg");
        assert_eq!(media_type("main.rs", b""), PLAIN_TEXT);
        assert_eq!(media_type("Makefile", b"all:\n\tcc"), PLAIN_TEXT);
        assert_eq!(
            media_type("scan", &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
            "image/png"
        );
        assert_eq!(media_type("blob.bin", &[0, 1, 2, 3]), UNKNOWN);
        assert_eq!(media_type(".gitignore", b"target/\n"), PLAIN_TEXT);
    }

    #[test]
    fn content_is_text_when_it_holds_no_nul_and_reads_as_utf8() {
        assert!(is_text("héllo".as_bytes()));
        assert!(!is_text(&[0x68, 0x00, 0x69]));
        assert!(!is_text(&[0xc3, 0x28]));
        assert!(is_text(&[0xff, 0xfe, 0x68, 0x00]));
    }

    #[test]
    fn a_name_for_a_path_has_no_separator_and_no_leading_dot() {
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name("  .hidden "), "hidden");
        assert_eq!(safe_name(""), "file");
        assert_eq!(safe_name("a\nb"), "a_b");
        assert_eq!(safe_name(&"x".repeat(500)).chars().count(), NAME_MAX_CHARS);
    }

    #[test]
    fn active_content_is_not_shown_inline() {
        assert!(!shows_inline("text/html"));
        assert!(!shows_inline("image/svg+xml"));
        assert!(shows_inline("image/png"));
        assert!(shows_inline("text/plain"));
        assert!(!shows_inline("Text/HTML; charset=utf-8"));
        assert!(!shows_inline("image/SVG+XML; charset=utf-8"));
        assert!(!shows_inline("multipart/x-mixed-replace; boundary=sample"));
    }
}
