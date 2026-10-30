//! What a document is said to be in one line: the first line of its description, or while it has none, of its first paragraph.
//!
//! A document carries a description as a place and a thread do, written under its title by a person, set by an
//! agent, or kept by automation. Where documents are listed, for people and for agents alike, one line of it stands
//! beside the title, so that a reader chooses what to open from that line. A document that nobody described is not
//! left without one: its first paragraph says what it is about well enough, and it is taken as it stands, by rule
//! and with no model, so that the same document always gives the same line.

/// The first line of `text` that is not blank, without the space around it.
#[must_use]
pub fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

/// The line a list shows for a document: the first line of `description`, or where that has none, the first line
/// of `first_paragraph`, the text of its first paragraph; white space inside it collapsed, and cut at `chars`
/// characters with an ellipsis where it was longer. Empty where both are.
#[must_use]
pub fn line(description: &str, first_paragraph: Option<&str>, chars: usize) -> String {
    let chosen = match first_line(description) {
        "" => first_paragraph.map(first_line).unwrap_or(""),
        own => own,
    };
    let collapsed = chosen.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= chars {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(chars).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_gives_its_first_line_that_is_not_blank() {
        assert_eq!(
            line("\n  How we ship  \nand roll back", Some("Ignored"), 200),
            "How we ship"
        );
    }

    #[test]
    fn without_a_description_the_first_paragraph_gives_the_line() {
        assert_eq!(
            line(" \n ", Some("Ship the fix\nbehind the flag"), 200),
            "Ship the fix"
        );
        assert_eq!(line("", None, 200), "", "nothing is made up");
    }

    #[test]
    fn a_long_line_is_cut_at_a_character_and_says_so() {
        assert_eq!(line("界界界界", None, 3), "界界界…");
        assert_eq!(line("a   b\tc", None, 200), "a b c");
        assert_eq!(line("abc", None, 3), "abc", "a line that fits is whole");
    }
}
