//! Cutting what a source says into pieces of about one size, each the text of one vector.
//!
//! A source is its parts in order: the blocks of a document, the items of a conversation, the messages of a
//! chat. Parts are joined into a piece until the next would pass the target size; a heading always begins a
//! new piece, so that a section stays together; a part longer than the target is cut at the last end of a
//! sentence or line that keeps at least half a target, else where the target is reached. Cutting from the
//! start keeps the pieces before a change as they were, so adding to the end of a conversation or a document
//! changes only its last pieces, and only changed pieces are sent again.
//!
//! Sizes are estimated tokens ([`estimated_tokens`]): a quarter of a token for each byte of ASCII text and one
//! for each other character, which is close to what tokenizers make of English and of Chinese, Japanese and
//! Korean alike, and never under it by much for either.

use orch_types::Id;

/// What a cut is called where a generation records it: a generation's vectors are comparable only with the
/// same way of cutting, and another word here makes every source cut anew.
pub const CHUNKING: &str = "parts-v1";

/// ASCII bytes per estimated token: what tokenizers of English text average.
const ASCII_BYTES_PER_TOKEN: i64 = 4;

/// The smallest share of a target, as a divisor, that a cut inside one part leaves before the end of a
/// sentence it cuts at: half.
const LEAST_PIECE_DIVISOR: i64 = 2;

/// One part of a source, in its order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// The block, item or message.
    pub id: Id,
    /// Its text as search reads it.
    pub text: String,
    /// Whether it heads a section, which begins a new piece.
    pub heading: bool,
}

/// One piece of a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// The part it begins with.
    pub first_part: Id,
    /// Its text.
    pub text: String,
    /// Its estimated tokens.
    pub tokens: i64,
}

/// The estimated tokens of a text.
#[must_use]
pub fn estimated_tokens(text: &str) -> i64 {
    let (ascii, other) = text
        .chars()
        .fold((0_i64, 0_i64), |(ascii, other), character| {
            if character.is_ascii() {
                (ascii + 1, other)
            } else {
                (ascii, other + 1)
            }
        });
    (ascii + ASCII_BYTES_PER_TOKEN - 1) / ASCII_BYTES_PER_TOKEN + other
}

/// Whether a character ends a sentence or a line, where a long part is best cut.
fn ends_sentence(character: char) -> bool {
    matches!(
        character,
        '.' | '!' | '?' | '\n' | '。' | '！' | '？' | '；' | ';' | '…'
    )
}

/// Cuts a text longer than `target` into runs of at most about `target` estimated tokens: at the last end of a
/// sentence past half a target, else at the last space past it, else where the target is reached.
fn cut_long(text: &str, target: i64) -> Vec<String> {
    let mut runs = Vec::new();
    let mut run = String::new();
    let mut tokens = 0_i64;
    // Where the run could end at a sentence, or else at a space: its length in bytes and its tokens there.
    let mut sentence: Option<(usize, i64)> = None;
    let mut space: Option<(usize, i64)> = None;
    let mut ascii_pending = 0_i64;
    for character in text.chars() {
        run.push(character);
        if character.is_ascii() {
            ascii_pending += 1;
            if ascii_pending == ASCII_BYTES_PER_TOKEN {
                tokens += 1;
                ascii_pending = 0;
            }
        } else {
            tokens += 1;
        }
        if tokens * LEAST_PIECE_DIVISOR >= target {
            if ends_sentence(character) {
                sentence = Some((run.len(), tokens));
            } else if character.is_whitespace() {
                space = Some((run.len(), tokens));
            }
        }
        if tokens >= target {
            let (end, used) = sentence.or(space).unwrap_or((run.len(), tokens));
            let rest = run.split_off(end);
            runs.push(std::mem::take(&mut run).trim().to_owned());
            run = rest;
            tokens -= used;
            sentence = None;
            space = None;
        }
    }
    if !run.trim().is_empty() {
        runs.push(run.trim().to_owned());
    }
    runs.retain(|run| !run.is_empty());
    runs
}

/// Cuts the parts of one source into pieces of about `target` estimated tokens each.
#[must_use]
pub fn cut(parts: &[Part], target: i64) -> Vec<Piece> {
    fn close(current: &mut Option<Piece>, pieces: &mut Vec<Piece>) {
        if let Some(piece) = current.take().filter(|piece| !piece.text.trim().is_empty()) {
            pieces.push(piece);
        }
    }
    let target = target.max(1);
    let mut pieces: Vec<Piece> = Vec::new();
    let mut current: Option<Piece> = None;
    for part in parts {
        let text = part.text.trim();
        if text.is_empty() {
            continue;
        }
        let tokens = estimated_tokens(text);
        if part.heading {
            close(&mut current, &mut pieces);
        }
        if tokens > target {
            close(&mut current, &mut pieces);
            for run in cut_long(text, target) {
                pieces.push(Piece {
                    first_part: part.id,
                    tokens: estimated_tokens(&run),
                    text: run,
                });
            }
            continue;
        }
        match current.as_mut() {
            Some(piece) if piece.tokens + tokens <= target => {
                piece.text.push('\n');
                piece.text.push_str(text);
                piece.tokens += tokens;
            }
            _ => {
                close(&mut current, &mut pieces);
                current = Some(Piece {
                    first_part: part.id,
                    text: text.to_owned(),
                    tokens,
                });
            }
        }
    }
    close(&mut current, &mut pieces);
    pieces
}

/// A piece's prompt identity, including the title that its document prefix may use.
#[must_use]
pub fn text_hash(title: &str, text: &str) -> orch_types::Hash {
    let mut hashed = Vec::new();
    hashed.extend_from_slice(&(title.len() as u64).to_le_bytes());
    hashed.extend_from_slice(title.as_bytes());
    hashed.extend_from_slice(text.as_bytes());
    orch_types::Hash::of(&hashed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(n: u128, text: &str, heading: bool) -> Part {
        Part {
            id: Id::from_u128(n),
            text: text.to_owned(),
            heading,
        }
    }

    #[test]
    fn english_counts_a_quarter_token_a_letter_and_chinese_one_a_character() {
        assert_eq!(estimated_tokens("abcd"), 1);
        assert_eq!(estimated_tokens("abcde"), 2);
        assert_eq!(estimated_tokens("发布流程"), 4);
        assert_eq!(estimated_tokens("rollout 发布"), 2 + 2);
        assert_eq!(estimated_tokens(""), 0);
    }

    #[test]
    fn parts_are_joined_up_to_the_target_and_a_heading_begins_a_piece() {
        let parts = [
            part(1, "aaaa aaaa", false),
            part(2, "bbbb bbbb", false),
            part(3, "Heading", true),
            part(4, "cccc", false),
            part(5, "   ", false),
        ];
        let pieces = cut(&parts, 6);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0].first_part, Id::from_u128(1));
        assert_eq!(pieces[0].text, "aaaa aaaa\nbbbb bbbb");
        assert_eq!(pieces[1].first_part, Id::from_u128(3));
        assert_eq!(pieces[1].text, "Heading\ncccc");
        let small = cut(&parts, 3);
        assert_eq!(
            small
                .iter()
                .map(|piece| piece.first_part.as_u128())
                .collect::<Vec<_>>(),
            vec![1, 2, 3],
            "a part that fills a piece stands alone, and a heading takes what fits after it"
        );
    }

    #[test]
    fn a_long_part_is_cut_at_the_end_of_a_sentence_in_any_script() {
        let chinese = "第一句话说完了。".repeat(10);
        let pieces = cut(&[part(7, &chinese, false)], 20);
        assert!(pieces.len() > 1);
        for piece in &pieces {
            assert!(piece.tokens <= 20, "{piece:?}");
            assert!(piece.text.ends_with('。'), "{piece:?}");
            assert_eq!(piece.first_part, Id::from_u128(7));
        }
        assert_eq!(
            pieces
                .iter()
                .map(|piece| piece.text.as_str())
                .collect::<String>(),
            chinese,
            "nothing is lost or repeated"
        );
        let english = "words ".repeat(100);
        let pieces = cut(&[part(8, &english, false)], 10);
        assert!(pieces.iter().all(|piece| piece.tokens <= 10));
        assert!(
            pieces
                .iter()
                .all(|piece| piece.text.split_whitespace().all(|word| word == "words")),
            "a long part without sentences is cut between words"
        );
        assert_eq!(
            pieces
                .iter()
                .map(|piece| piece.text.split_whitespace().count())
                .sum::<usize>(),
            100
        );
    }

    #[test]
    fn adding_to_the_end_keeps_every_earlier_piece() {
        let mut parts: Vec<Part> = (0..20)
            .map(|n| part(n, &format!("message number {n} about the rollout"), false))
            .collect();
        let before = cut(&parts, 30);
        parts.push(part(99, "a new message at the end", false));
        let after = cut(&parts, 30);
        assert_eq!(&after[..before.len() - 1], &before[..before.len() - 1]);
    }
}
