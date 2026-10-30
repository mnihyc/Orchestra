//! File media types and inert text classification shared by stored attachments and workspaces.

/// How many bytes at the start of a file decide whether it is text.
pub const SNIFF_BYTES: usize = 8 * 1024;

/// The media type of something that is not known by name or by shape and is not text.
pub const UNKNOWN: &str = "application/octet-stream";

/// The media type given to text no extension names.
pub const PLAIN_TEXT: &str = "text/plain";

/// The media type of a directory.
pub const DIRECTORY: &str = "inode/directory";

const BY_EXTENSION: &[(&str, &str)] = &[
    ("txt", PLAIN_TEXT),
    ("log", PLAIN_TEXT),
    ("md", "text/markdown"),
    ("markdown", "text/markdown"),
    ("html", "text/html"),
    ("htm", "text/html"),
    ("css", "text/css"),
    ("csv", "text/csv"),
    ("tsv", "text/tab-separated-values"),
    ("js", "text/javascript"),
    ("mjs", "text/javascript"),
    ("cjs", "text/javascript"),
    ("json", "application/json"),
    ("jsonl", "application/x-ndjson"),
    ("ndjson", "application/x-ndjson"),
    ("ipynb", "application/x-ipynb+json"),
    ("xml", "application/xml"),
    ("yaml", "application/yaml"),
    ("yml", "application/yaml"),
    ("toml", "application/toml"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("ico", "image/x-icon"),
    ("avif", "image/avif"),
    ("heic", "image/heic"),
    ("heif", "image/heif"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("pdf", "application/pdf"),
    ("zip", "application/zip"),
    ("tar", "application/x-tar"),
    ("gz", "application/gzip"),
    ("tgz", "application/gzip"),
    ("zst", "application/zstd"),
    ("xz", "application/x-xz"),
    ("bz2", "application/x-bzip2"),
    ("7z", "application/x-7z-compressed"),
    ("rar", "application/vnd.rar"),
    ("jar", "application/java-archive"),
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("oga", "audio/ogg"),
    ("opus", "audio/ogg"),
    ("flac", "audio/flac"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("ogv", "video/ogg"),
    ("mov", "video/quicktime"),
    ("m4v", "video/mp4"),
    ("mkv", "video/x-matroska"),
    ("doc", "application/msword"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    ("xls", "application/vnd.ms-excel"),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    ("ppt", "application/vnd.ms-powerpoint"),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("odt", "application/vnd.oasis.opendocument.text"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
    ("wasm", "application/wasm"),
];

/// Extensions of source and configuration that read as text whatever their first bytes are.
const TEXT_EXTENSIONS: &[&str] = &[
    "rs", "py", "go", "java", "kt", "c", "h", "cc", "cpp", "hpp", "cs", "rb", "php", "swift", "ts",
    "tsx", "jsx", "vue", "sh", "bash", "zsh", "sql", "lua", "pl", "r", "ini", "cfg", "conf",
    "lock", "gradle", "proto", "tf",
];

/// Whether the first bytes of a file say it is text.
#[must_use]
pub fn is_text(head: &[u8]) -> bool {
    let head = &head[..head.len().min(SNIFF_BYTES)];
    if head.starts_with(&[0xff, 0xfe]) || head.starts_with(&[0xfe, 0xff]) {
        return true;
    }
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        // A character cut off by the end of what was read is not a fault of the file.
        Err(error) => error.error_len().is_none(),
    }
}

fn sniff(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if head.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else if head.starts_with(b"BM") && head.len() > 14 {
        Some("image/bmp")
    } else if head.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else {
        None
    }
}

fn extension_of(name: &str) -> Option<String> {
    name.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, extension)| extension.to_ascii_lowercase())
}

/// The media type of a file called `name` whose first bytes are `head`; an empty `head` is no
/// evidence, and the name decides alone.
#[must_use]
pub fn media_type(name: &str, head: &[u8]) -> &'static str {
    if let Some(extension) = extension_of(name) {
        if let Some((_, media)) = BY_EXTENSION.iter().find(|(known, _)| *known == extension) {
            return media;
        }
        if TEXT_EXTENSIONS.contains(&extension.as_str()) {
            return PLAIN_TEXT;
        }
    }
    if let Some(found) = sniff(head) {
        return found;
    }
    if !head.is_empty() && is_text(head) {
        return PLAIN_TEXT;
    }
    UNKNOWN
}

/// Whether the content of a file called `name` is text, as far as the name says it when nothing of
/// the content has been seen.
#[must_use]
pub fn is_text_by_name(name: &str) -> bool {
    let media = media_type(name, &[]);
    media.starts_with("text/")
        || matches!(
            media,
            "application/json"
                | "application/x-ndjson"
                | "application/xml"
                | "application/yaml"
                | "application/toml"
        )
        || media == "image/svg+xml"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_media_and_office_files_keep_their_kind_when_they_move_between_sources() {
        for (name, expected) in [
            ("track.OGA", "audio/ogg"),
            ("track.opus", "audio/ogg"),
            ("track.m4a", "audio/mp4"),
            ("track.aac", "audio/aac"),
            ("film.mov", "video/quicktime"),
            ("film.ogv", "video/ogg"),
            ("phone.heif", "image/heif"),
            ("phone.heic", "image/heic"),
            (
                "sheet.ods",
                "application/vnd.oasis.opendocument.spreadsheet",
            ),
            (
                "paper.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            ("bundle.rar", "application/vnd.rar"),
        ] {
            assert_eq!(media_type(name, &[]), expected, "{name}");
        }
    }

    #[test]
    fn absent_bytes_do_not_turn_an_unknown_file_into_a_claimed_text_format() {
        assert_eq!(media_type("unknown", &[]), UNKNOWN);
        assert!(is_text(b""));
        assert_eq!(media_type("source.rs", &[]), PLAIN_TEXT);
    }
}
