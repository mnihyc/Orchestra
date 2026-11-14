//! Masking of what must not be stored in a chat: the values of the secrets a node resolves, and the patterns an org asks to be masked.
//!
//! Text is masked when it is written, before it is stored and before anybody
//! is told of it, so that nothing unmasked is ever read back. The patterns are
//! those `redaction.patterns` resolves to at the chat's node (at its root org
//! for a direct or a group chat); in the chat of a topic or of a thread the
//! values of that topic's secrets are masked as well, unless
//! `redaction.mask_secrets` is off there.

use crate::Error;
use orch_api_contract::chats::ChatPart;
use orch_db::Conn;
use orch_db::queries::chats::{self as rows, ChatRow};
use orch_db::secrets::MasterKey;
use orch_settings::Place;
use orch_settings::secrets::{self, Owner};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// What a masked value is replaced by.
pub const MASK: &str = "[masked]";

/// The fewest characters a secret has to have to be masked wherever it
/// occurs: a shorter value would turn words that happen to contain it into
/// masks, and a value that short is no secret.
pub const MIN_SECRET_CHARS: usize = 4;

/// How a deployment hands over the key that opens secrets: asked for only when
/// the node has secrets, and failing, rather than leaving text unmasked, when
/// it cannot be read.
pub type KeyProvider<'a> = &'a (dyn Fn() -> Result<&'static MasterKey, String> + Sync);

/// A pattern to mask, as `redaction.patterns` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pattern {
    /// What it is called.
    pub name: String,
    /// The regular expression.
    pub regex: String,
}

/// What masks text.
#[derive(Clone, Default)]
pub struct Redactor {
    patterns: Vec<Regex>,
    secrets: Vec<String>,
    plan: std::sync::OnceLock<NativePlan>,
}

impl Redactor {
    /// A redactor that masks nothing.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// A redactor for these patterns and these secret values. A pattern that
    /// is no regular expression is left out and told of in the log, since a
    /// setting that cannot be used should not stop people from talking; a
    /// secret too short to mask is left out.
    #[must_use]
    pub fn new(patterns: &[Pattern], secrets: Vec<String>) -> Self {
        let patterns = patterns
            .iter()
            .filter_map(|pattern| match Regex::new(&pattern.regex) {
                Ok(regex) => Some(regex),
                Err(error) => {
                    tracing::warn!(name = %pattern.name, %error, "a redaction pattern is not a regular expression");
                    None
                }
            })
            .collect();
        let mut secrets: Vec<String> = secrets
            .into_iter()
            .filter(|secret| secret.chars().count() >= MIN_SECRET_CHARS)
            .collect();
        // The longest first, so that a value that holds another is masked whole.
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        secrets.dedup();
        Self {
            patterns,
            secrets,
            plan: std::sync::OnceLock::new(),
        }
    }

    /// The text with every secret value and every match of a pattern masked.
    #[must_use]
    pub fn mask(&self, text: &str) -> String {
        let mut masked = text.to_owned();
        for secret in &self.secrets {
            masked = masked.replace(secret.as_str(), MASK);
        }
        for pattern in &self.patterns {
            masked = pattern.replace_all(&masked, MASK).into_owned();
        }
        masked
    }

    /// The exact same ordered replacements over private file-backed input. A match may span any
    /// number of caller chunks; matching never depends on a guessed overlap or line boundary.
    pub fn mask_reader(
        &self,
        reader: impl std::io::Read,
    ) -> std::io::Result<orch_private_file::PrivateText> {
        fn replace(
            text: &str,
            ranges: impl Iterator<Item = (usize, usize)>,
        ) -> std::io::Result<orch_private_file::PrivateText> {
            orch_private_file::PrivateText::build(|output| {
                let mut after = 0;
                for (first, last) in ranges {
                    output.write_all(&text.as_bytes()[after..first])?;
                    output.write_all(MASK.as_bytes())?;
                    after = last;
                }
                output.write_all(&text.as_bytes()[after..])
            })
        }
        let mut current = orch_private_file::PrivateText::from_reader(reader)?;
        for secret in &self.secrets {
            let text = current.text();
            let next = replace(
                text,
                text.match_indices(secret.as_str())
                    .map(|(start, _)| (start, start + secret.len())),
            )?;
            current = next;
        }
        for pattern in &self.patterns {
            let text = current.text();
            let next = replace(
                text,
                pattern
                    .find_iter(text)
                    .map(|found| (found.start(), found.end())),
            )?;
            current = next;
        }
        Ok(current)
    }

    /// Private capture state must be encrypted by its owner, never sent to a client or logged.
    pub fn native_snapshot(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&NativeRules {
            patterns: self
                .patterns
                .iter()
                .map(|pattern| pattern.as_str().to_owned())
                .collect(),
            secrets: self.secrets.clone(),
        })
    }
    /// Reopens only this capture's explicit rule shape; failures disclose no rule or secret value.
    pub fn restore_native(bytes: &[u8]) -> Result<Self, String> {
        let rules: NativeRules = serde_json::from_slice(bytes)
            .map_err(|_| "native masking rules could not be read".to_owned())?;
        let patterns = rules
            .patterns
            .iter()
            .map(|pattern| Regex::new(pattern))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "native masking rules could not be compiled".to_owned())?;
        Ok(Self {
            patterns,
            secrets: rules.secrets,
            plan: std::sync::OnceLock::new(),
        })
    }
    /// Capture protection can grow during one item, without exposing values to the caller.
    pub fn extend_native(&mut self, other: &Self) {
        for pattern in &other.patterns {
            if !self
                .patterns
                .iter()
                .any(|old| old.as_str() == pattern.as_str())
            {
                self.patterns.push(pattern.clone());
            }
        }
        for secret in &other.secrets {
            if !self.secrets.contains(secret) {
                self.secrets.push(secret.clone());
            }
        }
        self.secrets
            .sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        let _ = self.plan.take();
    }
    /// Whether a previously identified open continuation must stay private until field completion.
    #[must_use]
    pub fn native_open_ended(&self) -> bool {
        !self.native_plan().open.is_empty()
    }

    /// Whether any rule can mask native content; empty rules need no private streaming suffix.
    #[must_use]
    pub fn has_rules(&self) -> bool {
        !self.patterns.is_empty() || !self.secrets.is_empty()
    }

    fn native_plan(&self) -> &NativePlan {
        self.plan.get_or_init(|| {
            let mut longest = self.secrets.iter().map(String::len).max().unwrap_or(0);
            let mut all = Vec::new();
            let mut open = Vec::new();
            for secret in &self.secrets {
                all.push(Some(vec![secret.as_bytes().to_vec()]));
            }
            for pattern in &self.patterns {
                let Ok(hir) = regex_syntax::parse(pattern.as_str()) else {
                    open.push(None);
                    all.push(None);
                    continue;
                };
                if hir.properties().minimum_len().is_none() {
                    continue;
                }
                let sequence = regex_syntax::hir::literal::Extractor::new().extract(&hir);
                let prefixes = sequence.literals().map(|literals| {
                    literals
                        .iter()
                        .map(|literal| literal.as_bytes().to_vec())
                        .collect::<Vec<_>>()
                });
                all.push(prefixes.clone());
                match hir.properties().maximum_len() {
                    Some(bytes) => longest = longest.max(bytes),
                    None => open.push(prefixes),
                }
            }
            NativePlan {
                margin: if longest == 0 {
                    0
                } else {
                    longest.saturating_add(char::MAX.len_utf8())
                },
                all,
                open,
            }
        })
    }

    /// A safe private suffix follows actual maximum match length. Unbounded expressions keep
    /// their possible literal-prefix continuation until completion, preserving unrelated early text.
    #[must_use]
    pub fn native_cut(&self, text: &str) -> (usize, bool) {
        let plan = self.native_plan();
        let candidate = prefix_candidate(text, &plan.open);
        let mut cut = text.len().saturating_sub(plan.margin);
        if let Some(candidate) = candidate {
            cut = cut.min(candidate.saturating_sub(char::MAX.len_utf8()));
        }
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        (cut, full_prefix_present(text, &plan.open))
    }

    /// In an initial growing snapshot, possible secret prefixes are masked while the original
    /// assertion context stays private for the next fragment. Identifiers remain unchanged.
    #[must_use]
    pub fn mask_native_start(&self, text: &str, byte_units: bool) -> String {
        let plan = self.native_plan();
        let mut after = text.len().saturating_sub(plan.margin);
        while !text.is_char_boundary(after) {
            after -= 1;
        }
        let bounded = prefix_candidate(&text[after..], &plan.all).map(|at| after + at);
        let open = prefix_candidate(text, &plan.open);
        let mut cut = bounded.into_iter().chain(open).min().unwrap_or(text.len());
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut out = self.native_prefix(text, cut, byte_units, &[]).0;
        out.extend(std::iter::repeat_n(
            '*',
            if byte_units {
                text.len() - cut
            } else {
                text[cut..].chars().count()
            },
        ));
        out
    }

    /// Mask native content without changing its offset unit: characters for prose, bytes for output.
    #[must_use]
    pub fn mask_native(&self, text: &str, byte_units: bool) -> String {
        self.native_prefix(text, text.len(), byte_units, &[]).0
    }

    /// Emits one safe prefix, retaining match decisions for the still-private suffix.
    /// Carrying those decisions prevents a match split at the cut from exposing its trailing bytes.
    #[must_use]
    pub fn native_prefix(
        &self,
        text: &str,
        cut: usize,
        byte_units: bool,
        carried: &[(usize, usize)],
    ) -> (String, Vec<(usize, usize)>) {
        let mut sources: Vec<Matches<'_>> = Vec::new();
        for secret in &self.secrets {
            let size = secret.len();
            let source: Box<dyn Iterator<Item = (usize, usize)> + '_> = Box::new(
                text.match_indices(secret.as_str())
                    .map(move |(first, _)| (first, first + size)),
            );
            sources.push(source.peekable());
        }
        for pattern in &self.patterns {
            let source: Box<dyn Iterator<Item = (usize, usize)> + '_> = Box::new(
                pattern
                    .find_iter(text)
                    .filter(|found| found.start() < found.end())
                    .map(|found| (found.start(), found.end())),
            );
            sources.push(source.peekable());
        }
        let source: Box<dyn Iterator<Item = (usize, usize)> + '_> =
            Box::new(carried.iter().copied());
        sources.push(source.peekable());
        let mut out = String::new();
        let mut pending = Vec::new();
        let mut after = 0;
        for (first, last) in (NativeRanges { sources }) {
            if first < cut {
                out.push_str(&text[after..first].replace('\0', "*"));
                let end = last.min(cut);
                let count = if byte_units {
                    end - first
                } else {
                    text[first..end].chars().count()
                };
                out.extend(std::iter::repeat_n('*', count));
                after = end;
            }
            if first < cut && last > cut {
                pending.push((first.max(cut) - cut, last - cut));
            }
        }
        pending.extend(
            carried
                .iter()
                .filter(|(_, last)| *last > cut)
                .map(|(first, last)| (first.saturating_sub(cut), last - cut)),
        );
        pending.sort_unstable();
        out.push_str(&text[after..cut].replace('\0', "*"));
        (out, pending)
    }

    /// Masks the text of each part that has some.
    pub fn mask_parts(&self, parts: &mut [ChatPart]) {
        for part in parts {
            if let ChatPart::Text { text } = part {
                *text = self.mask(text);
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeRules {
    patterns: Vec<String>,
    secrets: Vec<String>,
}

#[derive(Debug, Clone)]
struct NativePlan {
    margin: usize,
    all: Vec<Option<Vec<Vec<u8>>>>,
    open: Vec<Option<Vec<Vec<u8>>>>,
}
impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor")
            .field("pattern_count", &self.patterns.len())
            .field("secret_count", &self.secrets.len())
            .finish_non_exhaustive()
    }
}
fn full_prefix_present(text: &str, sets: &[Option<Vec<Vec<u8>>>]) -> bool {
    sets.iter().any(|prefixes| match prefixes {
        None => true,
        Some(prefixes) => prefixes.iter().any(|prefix| {
            prefix.is_empty()
                || text
                    .as_bytes()
                    .windows(prefix.len())
                    .any(|window| window == prefix)
        }),
    })
}

fn prefix_candidate(text: &str, sets: &[Option<Vec<Vec<u8>>>]) -> Option<usize> {
    let mut first = None;
    for prefixes in sets {
        let Some(prefixes) = prefixes else {
            return Some(0);
        };
        for prefix in prefixes {
            if prefix.is_empty() {
                return Some(0);
            }
            if let Some(at) = text
                .as_bytes()
                .windows(prefix.len())
                .position(|window| window == prefix)
            {
                first = Some(first.map_or(at, |old: usize| old.min(at)));
            }
            let most = prefix.len().saturating_sub(1).min(text.len());
            if let Some(len) = (1..=most)
                .rev()
                .find(|len| text.as_bytes().ends_with(&prefix[..*len]))
            {
                let at = text.len() - len;
                first = Some(first.map_or(at, |old| old.min(at)));
            }
        }
    }
    first
}

type Matches<'a> = std::iter::Peekable<Box<dyn Iterator<Item = (usize, usize)> + 'a>>;
struct NativeRanges<'a> {
    sources: Vec<Matches<'a>>,
}
impl NativeRanges<'_> {
    fn next_source(&mut self) -> Option<usize> {
        self.sources
            .iter_mut()
            .enumerate()
            .filter_map(|(index, source)| source.peek().copied().map(|range| (index, range)))
            .min_by_key(|(_, range)| *range)
            .map(|(index, _)| index)
    }
}
impl Iterator for NativeRanges<'_> {
    type Item = (usize, usize);
    fn next(&mut self) -> Option<Self::Item> {
        let index = self.next_source()?;
        let mut range = self.sources[index].next()?;
        loop {
            let Some(index) = self.next_source() else {
                return Some(range);
            };
            let Some(next) = self.sources[index].peek().copied() else {
                return Some(range);
            };
            if next.0 > range.1 {
                return Some(range);
            }
            let _ = self.sources[index].next();
            range.1 = range.1.max(next.1);
        }
    }
}

/// The redactor for what is written in `chat`: the patterns that
/// `redaction.patterns` resolves to, and the values of the topic's secrets in
/// the chat of a topic or of a thread.
pub async fn for_chat(
    conn: &mut Conn,
    chat: &ChatRow,
    keys: KeyProvider<'_>,
) -> Result<Redactor, Error> {
    let place = Place::Node(chat.home());
    let patterns: Vec<Pattern> = orch_settings::get(conn, place, "redaction.patterns")
        .await?
        .unwrap_or_default();
    let mask_secrets: bool = orch_settings::get(conn, place, "redaction.mask_secrets")
        .await?
        .unwrap_or(true);
    let mut values = Vec::new();
    if mask_secrets
        && let Some(node) = chat.node_id
        && let Some(topic) = rows::topic_of(conn, node).await?
    {
        let named = secrets::list(conn, Owner::Node(topic))
            .await
            .map_err(|error| Error::Unmasked(error.to_string()))?;
        if !named.is_empty() {
            let key = keys().map_err(Error::Unmasked)?;
            values = secrets::open_resolved(conn, key, Owner::Node(topic))
                .await
                .map_err(|error| Error::Unmasked(error.to_string()))?
                .into_iter()
                .map(|opened| opened.value)
                .collect();
        }
    }
    Ok(Redactor::new(&patterns, values))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(regex: &str) -> Pattern {
        Pattern {
            name: "token".to_owned(),
            regex: regex.to_owned(),
        }
    }

    #[test]
    fn native_masks_preserve_character_and_byte_offsets_and_cross_fragment_matches() {
        let masker = Redactor::new(&[pattern("TOKEN-[0-9]{4}")], vec!["秘密值🙂".into()]);
        let original = "before 秘密值🙂 TOKEN-1234 after";
        for bytes in [false, true] {
            let masked = masker.mask_native(original, bytes);
            assert!(!masked.contains("秘密值🙂"));
            assert!(!masked.contains("TOKEN-1234"));
            if bytes {
                assert_eq!(masked.len(), original.len());
            } else {
                assert_eq!(masked.chars().count(), original.chars().count());
            }
            let mut pending = String::new();
            let mut decisions = Vec::new();
            let mut shown = String::new();
            for fragment in [
                "before 秘",
                "密值🙂 TO",
                "KEN-12",
                "34 after",
                " and more ordinary progress",
            ] {
                pending.push_str(fragment);
                let (cut, _) = masker.native_cut(&pending);
                let (safe, carry) = masker.native_prefix(&pending, cut, bytes, &decisions);
                shown.push_str(&safe);
                pending = pending[cut..].to_owned();
                decisions = carry;
            }
            shown.push_str(
                &masker
                    .native_prefix(&pending, pending.len(), bytes, &decisions)
                    .0,
            );
            assert_eq!(
                shown,
                masker.mask_native(&format!("{original} and more ordinary progress"), bytes)
            );
        }
    }

    #[test]
    fn an_open_ended_native_match_keeps_its_possible_continuation_private_until_completion() {
        let masker = Redactor::new(&[pattern("(?s)BEGIN.*END")], Vec::new());
        let text = "ordinary progress BEGIN private";
        let (cut, whole) = masker.native_cut(text);
        assert!(whole);
        assert!(cut <= text.find("BEGIN").unwrap_or(0));
        assert_eq!(
            masker.native_cut("ordinary unrelated progress"),
            (27, false)
        );
        assert!(
            !masker
                .mask_native("BEGIN private END", false)
                .contains("private")
        );
    }

    #[test]
    fn a_secret_is_masked_wherever_it_occurs_and_the_longer_value_first() {
        let redactor = Redactor::new(&[], vec!["hunter2".to_owned(), "hunter2-extra".to_owned()]);
        assert_eq!(
            redactor.mask("the password is hunter2-extra, or hunter2"),
            format!("the password is {MASK}, or {MASK}")
        );
    }

    #[test]
    fn a_pattern_masks_what_it_matches_and_a_broken_pattern_masks_nothing() {
        let redactor = Redactor::new(&[pattern(r"sk-[a-z0-9]{8,}"), pattern("(")], Vec::new());
        assert_eq!(
            redactor.mask("use sk-abcdef123456 now"),
            format!("use {MASK} now")
        );
        assert_eq!(redactor.mask("nothing here"), "nothing here");
    }

    #[test]
    fn a_value_too_short_to_be_a_secret_is_not_masked() {
        let redactor = Redactor::new(&[], vec!["ab".to_owned()]);
        assert_eq!(redactor.mask("a table of abacus"), "a table of abacus");
    }

    #[test]
    fn only_text_parts_are_masked() {
        let redactor = Redactor::new(&[], vec!["hunter2".to_owned()]);
        let mut parts = vec![
            ChatPart::Text {
                text: "hunter2".to_owned(),
            },
            ChatPart::ItemRef {
                thread_id: orch_types::new_id(),
                item_id: orch_types::new_id(),
            },
        ];
        redactor.mask_parts(&mut parts);
        assert_eq!(
            parts[0],
            ChatPart::Text {
                text: MASK.to_owned()
            }
        );
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;
    #[test]
    fn file_masking_is_identical_across_large_multiline_unicode_and_zero_width_matches()
    -> std::io::Result<()> {
        let source = format!(
            "prefix {} secret-value before{}after\n尾部 secret-value\n",
            "字🙂".repeat(20000),
            "a\n".repeat(40000)
        );
        for patterns in [
            vec![],
            vec![Pattern {
                name: "multiline".into(),
                regex: "(?s)before.*after".into(),
            }],
            vec![
                Pattern {
                    name: "line starts".into(),
                    regex: "(?m)^".into(),
                },
                Pattern {
                    name: "mask itself".into(),
                    regex: r"\[masked\]\[masked\]".into(),
                },
            ],
        ] {
            let redactor = Redactor::new(&patterns, vec!["secret-value".into(), "secret".into()]);
            let expected = redactor.mask(&source);
            let actual = redactor.mask_reader(source.as_bytes())?;
            assert_eq!(actual.text(), expected);
            assert_eq!(redactor.mask_reader(&b""[..])?.text(), redactor.mask(""));
        }
        Ok(())
    }
}
