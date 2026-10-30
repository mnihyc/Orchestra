//! What a reader says of what it kept as words or left out: a stable code, its parameters, and the English sentence for whoever does not know the code.
//!
//! A screen shows its own words for a code it knows, in the reader's language, and the sentence here only for a code
//! it does not. The codes are the contract; the sentences are what the catalogues say in English, kept together with
//! the codes in one table so that they cannot drift. A parameter is written in a sentence as `{name}`.

use std::collections::BTreeMap;

/// One thing a reader kept as words or left out.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Warning {
    /// Which, as the screens know it (`reader.image_missing`).
    pub code: String,
    /// What it is about: a path, the name of a tag, an address.
    pub params: BTreeMap<String, String>,
    /// The English sentence, with the parameters in it.
    pub message: String,
}

/// The codes, and what each says in English.
const WORDS: &[(&str, &str)] = &[
    ("reader.tag_kept_as_text", "Kept as text: <{tag}>"),
    (
        "reader.link_unresolved",
        "Link not found, kept as words: {target}",
    ),
    (
        "reader.link_is_file",
        "The link leads to a file, kept as words: {target}",
    ),
    (
        "reader.link_not_followed",
        "A link of a kind that is not followed was kept as words",
    ),
    ("reader.image_missing", "Picture not found: {path}"),
    (
        "reader.image_remote",
        "Pictures on the web are not fetched; they stay links",
    ),
    (
        "reader.image_inline",
        "A picture inside a line of text was kept as its description",
    ),
    (
        "reader.image_kind",
        "A picture of a kind that cannot be kept was left as its description",
    ),
    ("reader.film_missing", "Film not found: {path}"),
    ("reader.sound_missing", "Sound not found: {path}"),
    (
        "reader.media_address",
        "Cannot be played, kept as its caption: {address}",
    ),
    (
        "reader.columns_in_column",
        "Columns inside a column were written one after another",
    ),
    (
        "reader.column_alone",
        "A column outside columns was written as plain blocks",
    ),
    (
        "reader.columns_count",
        "Columns of fewer than two or more than six were written one after another",
    ),
    (
        "reader.pictures_left_out",
        "Pictures that the text carries inside it were left out",
    ),
    (
        "reader.unsafe_content",
        "Unsafe embedded content was removed",
    ),
    ("reader.unsafe_link", "Unsafe links were left as text"),
    (
        "reader.character_reference",
        "An unsupported character reference was kept as text",
    ),
    ("reader.html_incomplete", "Incomplete HTML was kept as text"),
    ("reader.table_span", "An invalid table span was removed"),
    (
        "reader.media_not_address",
        "A film or a sound that is not an address on the web was left as words",
    ),
    (
        "reader.word_styles",
        "Some of the Word document's styles are not known and were read as plain text",
    ),
    ("reader.more", "There are more of these than are listed"),
];

/// The code of a sentence that the shared HTML converter writes, which it keeps in English.
const SCRIPT: &[(&str, &str)] = &[
    (
        "An unsupported character reference was kept as text.",
        "reader.character_reference",
    ),
    (
        "Incomplete HTML was kept as text.",
        "reader.html_incomplete",
    ),
    (
        "Unsafe embedded content was removed.",
        "reader.unsafe_content",
    ),
    ("Unsafe links were left as text.", "reader.unsafe_link"),
    ("An invalid table span was removed.", "reader.table_span"),
    (
        "A picture inside a line of text was kept as its description.",
        "reader.image_inline",
    ),
    (
        "A film or a sound that is not an address on the web was left as words.",
        "reader.media_not_address",
    ),
    (
        "Columns of fewer than two or more than six were written one after another.",
        "reader.columns_count",
    ),
    (
        "Some of the Word document's styles are not known and were read as plain text.",
        "reader.word_styles",
    ),
];

/// The code for a thing a reader found that no screen has words for.
pub const OTHER: &str = "reader.other";

impl Warning {
    /// A warning of a known code, with the sentence for it in English.
    #[must_use]
    pub fn new(code: &str, params: &[(&str, &str)]) -> Self {
        let params: BTreeMap<String, String> = params
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        let mut message = WORDS
            .iter()
            .find(|(each, _)| *each == code)
            .map_or_else(|| code.to_owned(), |(_, words)| (*words).to_owned());
        for (name, value) in &params {
            message = message.replace(&format!("{{{name}}}"), value);
        }
        Self {
            code: code.to_owned(),
            params,
            message,
        }
    }

    /// What the shared HTML converter said, which is a sentence: with the code that stands for it where there is one.
    #[must_use]
    pub fn from_script(sentence: &str) -> Self {
        match SCRIPT.iter().find(|(words, _)| *words == sentence) {
            Some((_, code)) => Self::new(code, &[]),
            None => Self {
                code: OTHER.to_owned(),
                params: BTreeMap::new(),
                message: sentence.to_owned(),
            },
        }
    }
}

/// The most warnings (and missing paths) a reading lists; the rest are said by one more warning, so that a text with
/// hundreds of thousands of broken links is read in the time of its length and answered in a page.
pub const LISTED_MAX: usize = 200;

/// Adds a warning once.
pub fn note(warnings: &mut Vec<Warning>, warning: Warning) {
    if warnings.contains(&warning) {
        return;
    }
    if warnings.len() >= LISTED_MAX {
        let more = Warning::new("reader.more", &[]);
        if !warnings.contains(&more) {
            warnings.push(more);
        }
        return;
    }
    warnings.push(warning);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_warning_has_a_code_its_parameters_and_the_english_sentence_with_them_in_it() {
        let warning = Warning::new("reader.image_missing", &[("path", "images/a.png")]);
        assert_eq!(warning.code, "reader.image_missing");
        assert_eq!(warning.message, "Picture not found: images/a.png");
        assert_eq!(warning.params["path"], "images/a.png");
    }

    #[test]
    fn what_the_html_converter_says_has_a_code_and_what_it_does_not_know_is_other() {
        assert_eq!(
            Warning::from_script("Unsafe links were left as text.").code,
            "reader.unsafe_link"
        );
        let other = Warning::from_script("Something new.");
        assert_eq!(
            (other.code.as_str(), other.message.as_str()),
            (OTHER, "Something new.")
        );
    }

    #[test]
    fn every_code_has_words() {
        for (code, _) in WORDS {
            assert!(!Warning::new(code, &[]).message.is_empty());
        }
        for (_, code) in SCRIPT {
            assert!(WORDS.iter().any(|(each, _)| each == code), "{code}");
        }
    }
}
