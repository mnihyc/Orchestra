//! A document checked out as a file and committed back: the front matter that says which document the file came from and at which sequence.
//!
//! An agent copies a document into its workspace (`download_shared`), edits the file with its own tools, and commits it
//! (`edit_document` `rewrite` with `file`). The file begins with a front matter, between two lines of `---`, that
//! names the document, the sequence it was written at and its title. The commit reads it: the sequence is where the
//! change was written (the same rule that keeps two people from overwriting each other), a file that came from
//! another document is refused, and the front matter is no part of the text. A file without one is a text like any
//! other.

use orch_types::Id;

/// The line that opens and closes the front matter.
const FENCE: &str = "---";

/// What a checked-out file says of where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    /// The document it was written from.
    pub document: Id,
    /// The document's sequence when it was written.
    pub seq: i64,
    /// The title it had then.
    pub title: Option<String>,
}

/// A text with the front matter in front of it.
#[must_use]
pub fn write(checkout: &Checkout, body: &str) -> String {
    let mut out = format!(
        "{FENCE}\norch_document: {}\norch_seq: {}\n",
        checkout.document, checkout.seq
    );
    if let Some(title) = &checkout.title {
        out.push_str("orch_title: ");
        out.push_str(&serde_json::to_string(title).unwrap_or_else(|_| "\"\"".to_owned()));
        out.push('\n');
    }
    out.push_str(FENCE);
    out.push_str("\n\n");
    out.push_str(body);
    out
}

/// What a file says of where it came from, and the text after the front matter: the whole text, and nothing, where
/// there is no front matter of this kind.
#[must_use]
pub fn read(text: &str) -> (Option<Checkout>, &str) {
    let Some(rest) = text.strip_prefix(FENCE).and_then(|rest| {
        rest.strip_prefix("\r\n")
            .or_else(|| rest.strip_prefix('\n'))
    }) else {
        return (None, text);
    };
    let mut document: Option<Id> = None;
    let mut seq: Option<i64> = None;
    let mut title: Option<String> = None;
    let mut taken = 0;
    let mut closed = false;
    for line in rest.split_inclusive('\n') {
        taken += line.len();
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim_end() == FENCE {
            closed = true;
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "orch_document" => document = value.parse().ok(),
            "orch_seq" => seq = value.parse().ok(),
            "orch_title" => {
                title = serde_json::from_str::<String>(value)
                    .ok()
                    .or_else(|| Some(value.to_owned()));
            }
            _ => {}
        }
    }
    if !closed {
        return (None, text);
    }
    let body = &rest[taken..];
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
        .unwrap_or(body);
    match (document, seq) {
        (Some(document), Some(seq)) => (
            Some(Checkout {
                document,
                seq,
                title,
            }),
            body,
        ),
        // A front matter of somebody else's keeps its place in the text.
        _ => (None, text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn a_checked_out_text_says_where_it_came_from_and_the_body_is_what_follows() {
        let checkout = Checkout {
            document: new_id(),
            seq: 41,
            title: Some("Rollout \"plan\"".to_owned()),
        };
        let file = write(&checkout, "# Plan\n\ntext\n");
        let (read_back, body) = read(&file);
        assert_eq!(read_back, Some(checkout));
        assert_eq!(body, "# Plan\n\ntext\n");
    }

    #[test]
    fn a_text_without_the_front_matter_of_a_checkout_is_all_body() {
        for text in [
            "# Plan\n",
            "---\ntitle: other tool\n---\n\nbody\n",
            "---\norch_document: not-an-id\norch_seq: 3\n---\n\nbody\n",
            "---\norch_seq: 3\nnever closed\n",
        ] {
            let (found, body) = read(text);
            assert_eq!(found, None, "{text:?}");
            assert_eq!(body, text);
        }
    }
}
