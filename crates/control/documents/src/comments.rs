//! Discussions on a block, or on words of it: opened, answered, reacted to and resolved beside the text; no part of the document or of a version, and in an export or in what an agent reads only where that was asked.
//!
//! A discussion is about one block, or about some words of it. It is a list
//! of comments with reactions, and nothing of it is a block: the text of the
//! document, its versions and the stamps of its blocks are left alone. The
//! words a discussion was made on are kept as they read then; where they are
//! still in the block they are marked there, where they are gone the
//! discussion stays beside the block, and where the block is gone beside the
//! document.

use orch_api_contract::chats::Reaction;
use orch_api_contract::documents::{DISCUSSION_QUOTE_MAX_CHARS, Inline};
use orch_types::Id;

use crate::blocks::{self, Fault};

/// The most bytes a comment may take, as compact JSON: a quarter of what a
/// block may, since a comment is a remark and a longer text is a document.
pub const COMMENT_MAX_BYTES: usize = 16 * 1024;
/// The most bytes of an emoji, with the joiners and modifiers that make one
/// picture of several: a family of four is 25.
pub const EMOJI_MAX_BYTES: usize = 32;

/// The words a discussion is made on, as they are kept: cut of the space
/// around them, and at most the number of characters a quote may be.
pub fn clean_quote(quote: Option<&str>) -> Result<String, Fault> {
    let quote = quote.unwrap_or("").trim();
    let limit = usize::try_from(DISCUSSION_QUOTE_MAX_CHARS).unwrap_or(usize::MAX);
    if quote.chars().count() > limit {
        return Err(Fault::at(
            "quote",
            format!("must be at most {DISCUSSION_QUOTE_MAX_CHARS} characters"),
        ));
    }
    Ok(quote.to_owned())
}

/// Whether the words of a discussion are still in its block, and so are
/// marked there. A discussion on a whole block (no words) is anchored for
/// as long as the block is live.
#[must_use]
pub fn anchored(quote: &str, block_text: Option<&str>) -> bool {
    match block_text {
        None => false,
        Some(_) if quote.is_empty() => true,
        Some(text) => text.contains(quote),
    }
}

/// What a comment says, checked as the inline content of a block is, and
/// stored as such. It has to say something.
pub fn check_content(content: &[Inline]) -> Result<Vec<Inline>, Vec<Fault>> {
    let mut faults = Vec::new();
    let checked = blocks::check_inline(content, "content", &mut faults);
    let says_something = checked.iter().any(|run| match run {
        Inline::Text { text, .. } => !text.trim().is_empty(),
        Inline::Break => false,
        Inline::Mention { .. } | Inline::Ref { .. } | Inline::Math { .. } => true,
    });
    if !says_something {
        faults.push(Fault::at("content", "must say something"));
    }
    let bytes = serde_json::to_string(&checked).map_or(0, |text| text.len());
    if bytes > COMMENT_MAX_BYTES {
        faults.push(Fault::at(
            "content",
            format!("must be at most {COMMENT_MAX_BYTES} bytes"),
        ));
    }
    if faults.is_empty() {
        Ok(checked)
    } else {
        Err(faults)
    }
}

/// An emoji as it is kept: not empty, not long, and not blank or control characters.
pub fn clean_emoji(emoji: &str) -> Result<String, Fault> {
    let emoji = emoji.trim();
    if emoji.is_empty()
        || emoji.len() > EMOJI_MAX_BYTES
        || emoji.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(Fault::at("emoji", "must be one emoji"));
    }
    Ok(emoji.to_owned())
}

/// The reactions to a comment: each emoji with the people who put it,
/// in the order they did.
pub type Reactions = Vec<Reaction>;

/// Puts a person's reaction on, or takes it off. Whether anything changed:
/// putting on what is on, or taking off what is not, changes nothing. An
/// emoji nobody uses any more is gone from the list.
pub fn react(reactions: &mut Reactions, emoji: &str, user: Id, on: bool) -> bool {
    let at = reactions.iter().position(|each| each.emoji == emoji);
    match (at, on) {
        (Some(at), true) => {
            if reactions[at].users.contains(&user) {
                false
            } else {
                reactions[at].users.push(user);
                true
            }
        }
        (None, true) => {
            reactions.push(Reaction {
                emoji: emoji.to_owned(),
                users: vec![user],
            });
            true
        }
        (Some(at), false) => {
            let before = reactions[at].users.len();
            reactions[at].users.retain(|each| *each != user);
            let changed = reactions[at].users.len() != before;
            if reactions[at].users.is_empty() {
                reactions.remove(at);
            }
            changed
        }
        (None, false) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn text(words: &str) -> Inline {
        Inline::Text {
            text: words.to_owned(),
            marks: None,
            href: None,
            color: None,
            background: None,
        }
    }

    #[test]
    fn the_words_a_discussion_is_on_are_at_most_four_hundred_characters() {
        assert_eq!(clean_quote(None).unwrap(), "");
        assert_eq!(clean_quote(Some("  the words ")).unwrap(), "the words");
        assert!(clean_quote(Some(&"é".repeat(400))).is_ok());
        assert!(clean_quote(Some(&"é".repeat(401))).is_err());
    }

    #[test]
    fn a_discussion_stays_anchored_while_its_words_are_in_the_block() {
        assert!(anchored("second", Some("the second paragraph")));
        assert!(!anchored("second", Some("the 2nd paragraph")));
        assert!(anchored("", Some("anything")), "a whole block");
        assert!(!anchored("", None), "the block is gone");
        assert!(!anchored("second", None));
    }

    #[test]
    fn a_comment_has_to_say_something_and_is_checked_as_a_block_is() {
        assert!(check_content(&[text("Why?")]).is_ok());
        assert!(
            check_content(&[Inline::Mention {
                user_id: new_id(),
                user: None
            }])
            .is_ok()
        );
        let nothing = check_content(&[text("   "), Inline::Break]).unwrap_err();
        assert_eq!(nothing[0].path, "content");
        assert!(check_content(&[]).is_err());
        let linked = Inline::Text {
            text: "x".into(),
            marks: None,
            href: Some("javascript:alert(1)".into()),
            color: None,
            background: None,
        };
        assert_eq!(
            check_content(&[linked]).unwrap_err()[0].path,
            "content[0].href"
        );
        let huge = text(&"x".repeat(COMMENT_MAX_BYTES));
        assert!(check_content(&[huge]).is_err());
    }

    #[test]
    fn an_emoji_is_one_picture_and_not_a_sentence() {
        assert_eq!(clean_emoji("👍").unwrap(), "👍");
        assert!(clean_emoji("👨‍👩‍👧‍👦").is_ok());
        assert!(clean_emoji("").is_err());
        assert!(clean_emoji("a b").is_err());
        assert!(clean_emoji(&"x".repeat(EMOJI_MAX_BYTES + 1)).is_err());
    }

    #[test]
    fn reactions_keep_the_order_people_reacted_in_and_forget_an_emoji_nobody_uses() {
        let (ada, bo) = (new_id(), new_id());
        let mut reactions = Reactions::new();
        assert!(react(&mut reactions, "👍", ada, true));
        assert!(react(&mut reactions, "👍", bo, true));
        assert!(!react(&mut reactions, "👍", ada, true), "already on");
        assert_eq!(reactions[0].users, [ada, bo]);
        assert!(react(&mut reactions, "🎉", bo, true));
        assert!(react(&mut reactions, "👍", ada, false));
        assert!(!react(&mut reactions, "👍", ada, false), "already off");
        assert_eq!(reactions[0].users, [bo]);
        assert!(react(&mut reactions, "👍", bo, false));
        assert_eq!(
            reactions
                .iter()
                .map(|r| r.emoji.as_str())
                .collect::<Vec<_>>(),
            ["🎉"]
        );
        assert!(!react(&mut reactions, "😀", ada, false));
    }
}
