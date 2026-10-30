//! A text with placeholders from a closed list, in two forms and by language: checked when it is saved, and escaped for where it goes.
//!
//! What a channel or a destination sends is a text with placeholders, in two forms: `single`, for one
//! notification, and `digest`, for several in one message. Orch ships both for every built-in notifier, in
//! each language, and a channel sends the shipped ones until its own say otherwise:
//!
//! ```json
//! { "*": { "single": { "subject": "[{{org.name}}] {{title}}", "body": "{{title}}\n{{body}}\n{{url}}" },
//!          "digest": { "subject": "[{{org.name}}] {{count}} unread",
//!                      "body": "{{#notifications}}{{time}}  {{title}}\n{{url}}\n{{/notifications}}" } } }
//! ```
//!
//! | Rule | Detail |
//! |------|--------|
//! | What a template may hold | Text, and these placeholders: `{{title}}`, `{{body}}`, `{{url}}`, `{{kind}}`, `{{where}}`, `{{actor}}`, `{{time}}`, `{{count}}`, `{{recipient.name}}`, `{{org.name}}`; and one section, `{{#notifications}}…{{/notifications}}`, repeated for each notification of a digest. Nothing else: no conditions, no includes, no code |
//! | Checked when saved | A placeholder outside that list, a section that is not closed, or a template over 16 KiB is refused, naming what is wrong. `subject` is used by mail and ignored elsewhere |
//! | Escaped for where it goes | Values are escaped for the notifier's medium: for the HTML part of a mail, which is made from the text; for a JSON body. A body that is given for a JSON medium must render to JSON |
//! | What a text can show | What the Inbox row shows, and no more: a title, a first line, a link |
//!
//! A text longer than the most a notifier takes is cut at the end of a line and ends with how many more there
//! are and the address of the Inbox ([`fit`]).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The longest a template may be: 16 KiB, which holds any message a person would want sent and keeps a
/// channel's row small.
pub const MAX_TEMPLATE_BYTES: u64 = 16 * 1024;
/// The name of the one section a template may hold.
pub const SECTION: &str = "notifications";
/// The language key of the texts that serve every language without its own.
pub const ANY_LANGUAGE: &str = "*";

/// The values a template may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Placeholder {
    /// The notification's title.
    Title,
    /// Its body: a first line.
    Body,
    /// The address to open.
    Url,
    /// The kind of notification.
    Kind,
    /// The path of the node it happened at.
    Where,
    /// Who did it.
    Actor,
    /// When, in the recipient's time zone.
    Time,
    /// How many notifications the message carries.
    Count,
    /// The recipient's name.
    RecipientName,
    /// The organization's name.
    OrgName,
}

impl Placeholder {
    /// Every placeholder, in their fixed order.
    pub const ALL: [Self; 10] = [
        Self::Title,
        Self::Body,
        Self::Url,
        Self::Kind,
        Self::Where,
        Self::Actor,
        Self::Time,
        Self::Count,
        Self::RecipientName,
        Self::OrgName,
    ];

    /// The name as it is written between the braces.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Body => "body",
            Self::Url => "url",
            Self::Kind => "kind",
            Self::Where => "where",
            Self::Actor => "actor",
            Self::Time => "time",
            Self::Count => "count",
            Self::RecipientName => "recipient.name",
            Self::OrgName => "org.name",
        }
    }

    /// The placeholder of that name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|placeholder| placeholder.name() == name)
    }
}

/// Which of the two forms a text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Form {
    /// One notification.
    Single,
    /// Several in one message.
    Digest,
}

/// A text with placeholders: a subject, which mail uses and the others ignore, and a body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateText {
    /// The subject, for mail.
    pub subject: Option<String>,
    /// The body.
    pub body: String,
}

/// The shipped texts of a notifier in one language: both forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplatePair {
    /// For one notification.
    pub single: TemplateText,
    /// For several.
    pub digest: TemplateText,
}

/// A channel's or a destination's own texts in one language: either form may be left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateOverride {
    /// For one notification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub single: Option<TemplateText>,
    /// For several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<TemplateText>,
}

impl TemplateOverride {
    /// The text of one form, if this language has it.
    #[must_use]
    pub fn get(&self, form: Form) -> Option<&TemplateText> {
        match form {
            Form::Single => self.single.as_ref(),
            Form::Digest => self.digest.as_ref(),
        }
    }
}

impl TemplatePair {
    /// The text of one form.
    #[must_use]
    pub fn get(&self, form: Form) -> &TemplateText {
        match form {
            Form::Single => &self.single,
            Form::Digest => &self.digest,
        }
    }
}

/// The text a notification goes out with: the channel's own for the language, or for every language
/// ([`ANY_LANGUAGE`]); and otherwise the one the notifier ships, for the language or for every language. A
/// language that is not listed sends the shipped texts. Each form is looked up by itself.
#[must_use]
pub fn resolve<'a>(
    own: &'a BTreeMap<String, TemplateOverride>,
    shipped: &'a BTreeMap<String, TemplatePair>,
    language: &str,
    form: Form,
) -> Option<&'a TemplateText> {
    let by_language = |map: &'a BTreeMap<String, TemplateOverride>, key: &str| {
        map.iter()
            .find(|(tag, _)| tag.eq_ignore_ascii_case(key))
            .and_then(|(_, texts)| texts.get(form))
    };
    by_language(own, language)
        .or_else(|| by_language(own, ANY_LANGUAGE))
        .or_else(|| {
            let shipped_for = |key: &str| {
                shipped
                    .iter()
                    .find(|(tag, _)| tag.eq_ignore_ascii_case(key))
                    .map(|(_, pair)| pair.get(form))
            };
            shipped_for(language).or_else(|| shipped_for(ANY_LANGUAGE))
        })
}

/// Why a template is refused when it is saved. Each names what is wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    /// The text is longer than [`MAX_TEMPLATE_BYTES`].
    #[error("the template is {0} bytes, and a template is at most 16 KiB")]
    TooLong(u64),
    /// A placeholder is not on the list.
    #[error(
        "{{{{{0}}}}} is not a placeholder; the placeholders are title, body, url, kind, where, actor, time, count, recipient.name and org.name"
    )]
    UnknownPlaceholder(String),
    /// `{{` is not closed by `}}`.
    #[error("a {{{{ at byte {0} is not closed by }}}}")]
    Unclosed(u64),
    /// Nothing is between the braces.
    #[error("a placeholder at byte {0} is empty")]
    Empty(u64),
    /// A section other than `notifications` is opened.
    #[error("{{{{#{0}}}}} is not a section; the one section is notifications")]
    UnknownSection(String),
    /// A section is opened and never closed.
    #[error("the section {{{{#notifications}}}} is not closed by {{{{/notifications}}}}")]
    SectionNotClosed,
    /// A section is closed that was not opened.
    #[error("{{{{/notifications}}}} at byte {0} closes a section that was not opened")]
    SectionNotOpened(u64),
    /// A section is opened inside the section.
    #[error("the section {{{{#notifications}}}} at byte {0} is inside another")]
    NestedSection(u64),
    /// A text for a JSON medium does not render to JSON.
    #[error("the text does not render to JSON: {0}")]
    NotJson(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Value(Placeholder),
    Section(Vec<Part>),
}

/// A text with placeholders that has been checked and cut into parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    parts: Vec<Part>,
}

fn to_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

impl Template {
    /// Checks a text and cuts it into parts: at most [`MAX_TEMPLATE_BYTES`]; placeholders from the list
    /// only; `{{#notifications}}` closed by `{{/notifications}}` and not inside itself. Whitespace inside
    /// the braces is ignored.
    pub fn parse(text: &str) -> Result<Self, TemplateError> {
        if to_u64(text.len()) > MAX_TEMPLATE_BYTES {
            return Err(TemplateError::TooLong(to_u64(text.len())));
        }
        let mut top: Vec<Part> = Vec::new();
        let mut section: Option<Vec<Part>> = None;
        let mut rest = text;
        let mut offset = 0usize;
        while let Some(open) = rest.find("{{") {
            if open > 0 {
                current(&mut top, &mut section).push(Part::Text(rest[..open].to_owned()));
            }
            let after = &rest[open + 2..];
            let Some(close) = after.find("}}") else {
                return Err(TemplateError::Unclosed(to_u64(offset + open)));
            };
            let tag = after[..close].trim();
            let at = to_u64(offset + open);
            if tag.is_empty() {
                return Err(TemplateError::Empty(at));
            }
            if let Some(name) = tag.strip_prefix('#') {
                let name = name.trim();
                if name != SECTION {
                    return Err(TemplateError::UnknownSection(name.to_owned()));
                }
                if section.is_some() {
                    return Err(TemplateError::NestedSection(at));
                }
                section = Some(Vec::new());
            } else if let Some(name) = tag.strip_prefix('/') {
                let name = name.trim();
                if name != SECTION {
                    return Err(TemplateError::UnknownSection(name.to_owned()));
                }
                let Some(inner) = section.take() else {
                    return Err(TemplateError::SectionNotOpened(at));
                };
                top.push(Part::Section(inner));
            } else {
                let placeholder = Placeholder::parse(tag)
                    .ok_or_else(|| TemplateError::UnknownPlaceholder(tag.to_owned()))?;
                current(&mut top, &mut section).push(Part::Value(placeholder));
            }
            let consumed = open + 2 + close + 2;
            offset += consumed;
            rest = &rest[consumed..];
        }
        if !rest.is_empty() {
            current(&mut top, &mut section).push(Part::Text(rest.to_owned()));
        }
        if section.is_some() {
            return Err(TemplateError::SectionNotClosed);
        }
        Ok(Self { parts: top })
    }

    /// Whether the template has the section.
    #[must_use]
    pub fn has_section(&self) -> bool {
        self.parts
            .iter()
            .any(|part| matches!(part, Part::Section(_)))
    }

    /// The placeholders the template names, each once, in the order they first appear.
    #[must_use]
    pub fn placeholders(&self) -> Vec<Placeholder> {
        fn walk(parts: &[Part], found: &mut Vec<Placeholder>) {
            for part in parts {
                match part {
                    Part::Value(placeholder) if !found.contains(placeholder) => {
                        found.push(*placeholder)
                    }
                    Part::Section(inner) => walk(inner, found),
                    _ => {}
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.parts, &mut found);
        found
    }

    /// The text with the values filled in, each escaped for where it goes. The section is repeated for each
    /// notification; a placeholder outside it names the newest notification's value (the last), and inside it
    /// the notification of that turn.
    #[must_use]
    pub fn render(&self, values: &Values, escape: Escape) -> String {
        let mut out = String::new();
        render_parts(&self.parts, values, values.items.last(), escape, &mut out);
        out
    }
}

fn current<'a>(top: &'a mut Vec<Part>, section: &'a mut Option<Vec<Part>>) -> &'a mut Vec<Part> {
    section.as_mut().unwrap_or(top)
}

fn render_parts(
    parts: &[Part],
    values: &Values,
    item: Option<&Item>,
    escape: Escape,
    out: &mut String,
) {
    for part in parts {
        match part {
            Part::Text(text) => out.push_str(text),
            Part::Value(placeholder) => {
                out.push_str(&escape.apply(&values.value(*placeholder, item)))
            }
            Part::Section(inner) => {
                for each in &values.items {
                    render_parts(inner, values, Some(each), escape, out);
                }
            }
        }
    }
}

/// What one notification shows, as text already made for the recipient.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Item {
    /// Its title.
    pub title: String,
    /// Its first line.
    pub body: String,
    /// The address to open.
    pub url: String,
    /// Its kind.
    pub kind: String,
    /// The path of the node it happened at.
    pub location: String,
    /// Who did it.
    pub actor: String,
    /// When, in the recipient's time zone.
    pub time: String,
}

/// The values a message is rendered with: its notifications, oldest first, the recipient and the
/// organization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values {
    /// The notifications of the message, oldest first; one for the form `single`.
    pub items: Vec<Item>,
    /// The recipient's name.
    pub recipient_name: String,
    /// The organization's name.
    pub org_name: String,
}

impl Values {
    fn value(&self, placeholder: Placeholder, item: Option<&Item>) -> String {
        let of =
            |pick: fn(&Item) -> &String| item.map(|item| pick(item).clone()).unwrap_or_default();
        match placeholder {
            Placeholder::Title => of(|item| &item.title),
            Placeholder::Body => of(|item| &item.body),
            Placeholder::Url => of(|item| &item.url),
            Placeholder::Kind => of(|item| &item.kind),
            Placeholder::Where => of(|item| &item.location),
            Placeholder::Actor => of(|item| &item.actor),
            Placeholder::Time => of(|item| &item.time),
            Placeholder::Count => self.items.len().to_string(),
            Placeholder::RecipientName => self.recipient_name.clone(),
            Placeholder::OrgName => self.org_name.clone(),
        }
    }

    /// Values for checking a template by rendering it: two notifications whose texts hold every character
    /// that needs escaping.
    #[must_use]
    pub fn sample() -> Self {
        let item = |n: u32| Item {
            title: format!("Turn {n} \"finished\""),
            body: "line one\nline <two> & \\ three".to_owned(),
            url: format!("https://orch.example/inbox/{n}"),
            kind: "turn.completed".to_owned(),
            location: "acme/web/fix-login".to_owned(),
            actor: "Dana".to_owned(),
            time: "2026-10-11 10:00".to_owned(),
        };
        Self {
            items: vec![item(1), item(2)],
            recipient_name: "Bo".to_owned(),
            org_name: "Acme".to_owned(),
        }
    }
}

/// How a value is made safe for the place it goes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Escape {
    /// A text body: control characters other than a newline and a tab are dropped, so that a value cannot
    /// carry terminal codes.
    Text,
    /// One line, for a subject: every control character, a newline too, becomes a space, so that a value
    /// cannot start a new header of a mail.
    Line,
    /// The inside of a JSON string.
    Json,
    /// Text in HTML.
    Html,
}

impl Escape {
    /// The escaping for a notifier whose body is of `kind`.
    #[must_use]
    pub const fn for_body(kind: BodyKind) -> Self {
        match kind {
            BodyKind::Text => Self::Text,
            BodyKind::Json => Self::Json,
        }
    }

    /// `value`, made safe.
    #[must_use]
    pub fn apply(self, value: &str) -> String {
        match self {
            Self::Text => value
                .chars()
                .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
                .collect(),
            Self::Line => value
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect::<String>()
                .trim()
                .to_owned(),
            Self::Json => escape_json(value),
            Self::Html => escape_html(value),
        }
    }
}

/// What a notifier's body is, and so how its values are escaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    /// Text.
    Text,
    /// JSON.
    Json,
}

/// `value` as the inside of a JSON string: `"` and `\` escaped, control characters as `\n`, `\t`, `\r` or
/// `\u00XX`.
#[must_use]
pub fn escape_json(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

/// `value` as text in HTML: `&`, `<`, `>`, `"` and `'` escaped.
#[must_use]
pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A text as it goes out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The subject, one line, when the text has one.
    pub subject: Option<String>,
    /// The body, escaped for the notifier's medium.
    pub body: String,
}

impl TemplateText {
    /// Checks the text as it is saved: the subject and the body are templates, together at most 16 KiB; and
    /// for a JSON medium the body renders to JSON, checked with sample values.
    pub fn check(&self, kind: BodyKind) -> Result<(), TemplateError> {
        let size = to_u64(self.subject.as_deref().map_or(0, str::len) + self.body.len());
        if size > MAX_TEMPLATE_BYTES {
            return Err(TemplateError::TooLong(size));
        }
        if let Some(subject) = &self.subject {
            Template::parse(subject)?;
        }
        let body = Template::parse(&self.body)?;
        if kind == BodyKind::Json {
            let rendered = body.render(&Values::sample(), Escape::Json);
            serde_json::from_str::<serde_json::Value>(&rendered)
                .map_err(|error| TemplateError::NotJson(error.to_string()))?;
        }
        Ok(())
    }

    /// Renders the text for a message: the subject as one line, the body escaped for `kind`. A body for a
    /// JSON medium that does not come out as JSON is an error.
    pub fn render(&self, values: &Values, kind: BodyKind) -> Result<Rendered, TemplateError> {
        let subject = self
            .subject
            .as_deref()
            .map(Template::parse)
            .transpose()?
            .map(|template| template.render(values, Escape::Line));
        let body = Template::parse(&self.body)?.render(values, Escape::for_body(kind));
        if kind == BodyKind::Json {
            serde_json::from_str::<serde_json::Value>(&body)
                .map_err(|error| TemplateError::NotJson(error.to_string()))?;
        }
        Ok(Rendered { subject, body })
    }
}

/// What is wrong with a map of a channel's own texts, and where.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the {form:?} text for {language:?}: {error}")]
pub struct TextFault {
    /// The language key.
    pub language: String,
    /// The form.
    pub form: Form,
    /// What is wrong.
    pub error: TemplateError,
}

/// Checks all the texts a channel or a destination sets, as they are saved. Every fault is returned.
pub fn check_texts(
    texts: &BTreeMap<String, TemplateOverride>,
    kind: BodyKind,
) -> Result<(), Vec<TextFault>> {
    let mut faults = Vec::new();
    for (language, own) in texts {
        for form in [Form::Single, Form::Digest] {
            if let Some(Err(error)) = own.get(form).map(|text| text.check(kind)) {
                faults.push(TextFault {
                    language: language.clone(),
                    form,
                    error,
                });
            }
        }
    }
    if faults.is_empty() {
        Ok(())
    } else {
        Err(faults)
    }
}

/// A text that was cut to fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fitted {
    /// The text that goes out, with its ending where it was cut.
    pub text: String,
    /// How many lines were left out; 0 when nothing was cut.
    pub left_out: i64,
}

/// Cuts a text to at most `max_chars` characters, at the end of a line. Where it is cut, it ends with the
/// line that `ending` makes of the number of lines left out, which says how many more there are and where
/// the Inbox is. A first line that alone is too long is cut inside, since no line end is left to cut at.
#[must_use]
pub fn fit(text: &str, max_chars: u32, ending: impl Fn(i64) -> String) -> Fitted {
    let max = usize::try_from(max_chars).unwrap_or(usize::MAX);
    if text.chars().count() <= max {
        return Fitted {
            text: text.to_owned(),
            left_out: 0,
        };
    }
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    for kept in (1..total).rev() {
        let left = i64::try_from(total - kept).unwrap_or(i64::MAX);
        let candidate = format!("{}\n{}", lines[..kept].join("\n"), ending(left));
        if candidate.chars().count() <= max {
            return Fitted {
                text: candidate,
                left_out: left,
            };
        }
    }
    let left = i64::try_from(total.saturating_sub(1)).unwrap_or(i64::MAX);
    let tail = format!("\n{}", ending(left));
    let room = max.saturating_sub(tail.chars().count());
    let head: String = lines
        .first()
        .copied()
        .unwrap_or_default()
        .chars()
        .take(room)
        .collect();
    let mut cut = format!("{head}{tail}");
    if cut.chars().count() > max {
        cut = cut.chars().take(max).collect();
    }
    Fitted {
        text: cut,
        left_out: left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn single() -> TemplateText {
        TemplateText {
            subject: Some("[{{org.name}}] {{title}}".into()),
            body: "{{title}}\n{{body}}\n{{url}}".into(),
        }
    }

    fn digest() -> TemplateText {
        TemplateText {
            subject: Some("[{{org.name}}] {{count}} unread".into()),
            body: "{{#notifications}}{{time}}  {{title}}\n{{url}}\n{{/notifications}}".into(),
        }
    }

    fn item(title: &str, body: &str, url: &str) -> Item {
        Item {
            title: title.into(),
            body: body.into(),
            url: url.into(),
            time: "10:00".into(),
            ..Item::default()
        }
    }

    fn values(items: Vec<Item>) -> Values {
        Values {
            items,
            recipient_name: "Bo".into(),
            org_name: "Acme".into(),
        }
    }

    #[test]
    fn the_shipped_texts_read_as_the_spec_writes_them_and_pass_the_check() {
        let pair: BTreeMap<String, TemplatePair> = serde_json::from_value(json!({
            "*": { "single": { "subject": "[{{org.name}}] {{title}}", "body": "{{title}}\n{{body}}\n{{url}}" },
                   "digest": { "subject": "[{{org.name}}] {{count}} unread",
                               "body": "{{#notifications}}{{time}}  {{title}}\n{{url}}\n{{/notifications}}" } } }))
        .unwrap();
        for form in [Form::Single, Form::Digest] {
            assert_eq!(pair["*"].get(form).check(BodyKind::Text), Ok(()));
        }
        assert_eq!(single(), pair["*"].single);
        assert_eq!(digest(), pair["*"].digest);
    }

    #[test]
    fn a_template_may_hold_text_ten_placeholders_and_one_section_and_nothing_else() {
        for placeholder in Placeholder::ALL {
            let template = Template::parse(&format!("a {{{{{}}}}} b", placeholder.name())).unwrap();
            assert_eq!(template.placeholders(), [placeholder]);
        }
        assert_eq!(
            Template::parse("{{ title }} and {{  recipient.name}}")
                .unwrap()
                .placeholders(),
            [Placeholder::Title, Placeholder::RecipientName]
        );
        assert_eq!(
            Template::parse("no placeholders, { one brace } is text")
                .unwrap()
                .placeholders(),
            []
        );
        assert_eq!(Template::parse("{{title}}"), Template::parse("{{ title }}"));
        assert!(Template::parse(&digest().body).unwrap().has_section());
    }

    #[test]
    fn a_template_is_refused_for_what_is_not_on_the_list_naming_it() {
        assert_eq!(
            Template::parse("{{password}}"),
            Err(TemplateError::UnknownPlaceholder("password".into()))
        );
        assert_eq!(
            Template::parse("{{#if title}}x{{/if}}"),
            Err(TemplateError::UnknownSection("if title".into()))
        );
        assert_eq!(
            Template::parse("{{#notifications}}{{title}}"),
            Err(TemplateError::SectionNotClosed)
        );
        assert_eq!(
            Template::parse("{{title}}{{/notifications}}"),
            Err(TemplateError::SectionNotOpened(9))
        );
        assert_eq!(
            Template::parse(
                "{{#notifications}}{{#notifications}}x{{/notifications}}{{/notifications}}"
            ),
            Err(TemplateError::NestedSection(18))
        );
        assert_eq!(
            Template::parse("hello {{title"),
            Err(TemplateError::Unclosed(6))
        );
        assert_eq!(Template::parse("{{}}"), Err(TemplateError::Empty(0)));
        let long = "x".repeat(16 * 1024 + 1);
        assert_eq!(
            Template::parse(&long),
            Err(TemplateError::TooLong(16 * 1024 + 1))
        );
        assert!(Template::parse(&"x".repeat(16 * 1024)).is_ok());
        let message = TemplateError::UnknownPlaceholder("password".into()).to_string();
        assert!(
            message.contains("{{password}}") && message.contains("is not a placeholder"),
            "{message}"
        );
    }

    #[test]
    fn a_single_notification_and_a_digest_are_rendered_with_their_values() {
        let one = values(vec![item(
            "Turn finished",
            "all green",
            "https://orch.example/i/1",
        )]);
        let rendered = single().render(&one, BodyKind::Text).unwrap();
        assert_eq!(rendered.subject.as_deref(), Some("[Acme] Turn finished"));
        assert_eq!(
            rendered.body,
            "Turn finished\nall green\nhttps://orch.example/i/1"
        );
        let several = values(vec![
            item("A", "", "u1"),
            item("B", "", "u2"),
            item("C", "", "u3"),
        ]);
        let rendered = digest().render(&several, BodyKind::Text).unwrap();
        assert_eq!(rendered.subject.as_deref(), Some("[Acme] 3 unread"));
        assert_eq!(rendered.body, "10:00  A\nu1\n10:00  B\nu2\n10:00  C\nu3\n");
        // Outside the section a per-notification placeholder is the newest notification's.
        let outside = TemplateText {
            subject: None,
            body: "{{count}}: latest {{title}} for {{recipient.name}}".into(),
        };
        assert_eq!(
            outside.render(&several, BodyKind::Text).unwrap().body,
            "3: latest C for Bo"
        );
    }

    #[test]
    fn values_are_escaped_for_a_text_body_a_json_body_a_subject_and_html() {
        let hostile = item(
            "Evil\r\nBcc: x@y.z \u{1b}[31m\"quote\" <b>&",
            "line\nbreak",
            "u",
        );
        let v = values(vec![hostile]);
        let text = TemplateText {
            subject: Some("{{title}}".into()),
            body: "{{title}}|{{body}}".into(),
        };
        let out = text.render(&v, BodyKind::Text).unwrap();
        assert_eq!(
            out.subject.as_deref(),
            Some("Evil  Bcc: x@y.z  [31m\"quote\" <b>&"),
            "a value cannot start a new header"
        );
        assert_eq!(
            out.body, "Evil\nBcc: x@y.z [31m\"quote\" <b>&|line\nbreak",
            "terminal codes are dropped, a newline stays"
        );
        let json_body = TemplateText {
            subject: None,
            body: "{\"text\": \"{{title}}\", \"more\": \"{{body}}\"}".into(),
        };
        let out = json_body.render(&v, BodyKind::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(
            parsed["text"], "Evil\r\nBcc: x@y.z \u{1b}[31m\"quote\" <b>&",
            "the JSON carries the value exactly"
        );
        assert_eq!(parsed["more"], "line\nbreak");
        assert_eq!(
            escape_html("<a href=\"x\">T&C's</a>"),
            "&lt;a href=&quot;x&quot;&gt;T&amp;C&#39;s&lt;/a&gt;"
        );
        assert_eq!(Escape::Html.apply("a<b"), "a&lt;b");
    }

    #[test]
    fn a_body_for_a_json_medium_must_render_to_json() {
        let not_json = TemplateText {
            subject: None,
            body: "{{title}} finished".into(),
        };
        assert!(matches!(
            not_json.check(BodyKind::Json),
            Err(TemplateError::NotJson(_))
        ));
        assert_eq!(not_json.check(BodyKind::Text), Ok(()));
        let fitted_to_chat = TemplateText {
            subject: None,
            body: "{\"blocks\": [{\"text\": \"{{title}} {{url}}\"}], \"n\": {{count}}}".into(),
        };
        assert_eq!(fitted_to_chat.check(BodyKind::Json), Ok(()));
        assert!(matches!(
            not_json.render(&values(vec![item("x", "", "")]), BodyKind::Json),
            Err(TemplateError::NotJson(_))
        ));
    }

    #[test]
    fn all_the_texts_a_channel_sets_are_checked_naming_the_language_and_the_form() {
        let texts: BTreeMap<String, TemplateOverride> = serde_json::from_value(json!({
            "*": { "single": { "subject": null, "body": "{{title}}" } },
            "zh-CN": { "digest": { "subject": null, "body": "{{#notifications}}{{nope}}{{/notifications}}" } }
        }))
        .unwrap();
        let faults = check_texts(&texts, BodyKind::Text).unwrap_err();
        assert_eq!(faults.len(), 1);
        assert_eq!(
            (faults[0].language.as_str(), faults[0].form),
            ("zh-CN", Form::Digest)
        );
        assert!(faults[0].to_string().contains("nope"));
    }

    #[test]
    fn a_channel_sends_its_own_text_then_the_shipped_one_form_by_form() {
        let shipped: BTreeMap<String, TemplatePair> = BTreeMap::from([
            (
                "*".to_owned(),
                TemplatePair {
                    single: TemplateText {
                        subject: None,
                        body: "shipped any single".into(),
                    },
                    digest: TemplateText {
                        subject: None,
                        body: "shipped any digest".into(),
                    },
                },
            ),
            (
                "zh-CN".to_owned(),
                TemplatePair {
                    single: TemplateText {
                        subject: None,
                        body: "shipped zh single".into(),
                    },
                    digest: TemplateText {
                        subject: None,
                        body: "shipped zh digest".into(),
                    },
                },
            ),
        ]);
        let own: BTreeMap<String, TemplateOverride> = BTreeMap::from([(
            "*".to_owned(),
            TemplateOverride {
                single: Some(TemplateText {
                    subject: None,
                    body: "own single".into(),
                }),
                digest: None,
            },
        )]);
        let body =
            |language: &str, form| resolve(&own, &shipped, language, form).map(|t| t.body.as_str());
        assert_eq!(
            body("en", Form::Single),
            Some("own single"),
            "its own text serves every language without one"
        );
        assert_eq!(body("zh-CN", Form::Single), Some("own single"));
        assert_eq!(
            body("zh-CN", Form::Digest),
            Some("shipped zh digest"),
            "the other form is the shipped one, in the language"
        );
        assert_eq!(body("en", Form::Digest), Some("shipped any digest"));
        assert_eq!(
            body("ZH-cn", Form::Digest),
            Some("shipped zh digest"),
            "language tags are compared without case"
        );
        let none = BTreeMap::new();
        assert_eq!(resolve(&none, &BTreeMap::new(), "en", Form::Single), None);
    }

    #[test]
    fn a_text_over_the_most_a_notifier_takes_is_cut_at_a_line_and_the_rest_is_counted() {
        let ending = |more: i64| format!("... and {more} more: https://orch.example/inbox");
        let text = (1..=10)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            fit(&text, 1000, ending),
            Fitted {
                text: text.clone(),
                left_out: 0
            }
        );
        let cut = fit(&text, 60, ending);
        assert!(cut.text.chars().count() <= 60, "{cut:?}");
        assert!(cut.text.ends_with("https://orch.example/inbox"));
        assert!(cut.left_out > 0 && cut.left_out < 10);
        let kept = cut.text.lines().count() - 1;
        assert_eq!(
            i64::try_from(kept).unwrap() + cut.left_out,
            10,
            "what is kept and what is counted make all the lines"
        );
        assert!(
            cut.text.starts_with("line 1\n"),
            "it is cut at a line, so what stays is whole lines"
        );
        assert!(cut.text.contains(&format!("and {} more", cut.left_out)));
    }

    #[test]
    fn a_first_line_that_is_too_long_alone_is_cut_inside_and_a_limit_counts_characters_not_bytes() {
        let ending = |more: i64| format!("+{more}");
        let cut = fit(&format!("{}\nsecond", "x".repeat(100)), 20, ending);
        assert!(cut.text.chars().count() <= 20);
        assert!(cut.text.ends_with("+1"));
        assert_eq!(cut.left_out, 1);
        let wide = "字".repeat(30);
        assert_eq!(
            fit(&wide, 30, ending),
            Fitted {
                text: wide.clone(),
                left_out: 0
            },
            "thirty characters of three bytes each fit in 30"
        );
        assert!(fit(&wide, 29, ending).text.chars().count() <= 29);
    }
}
