//! A document written out as Typst source with its images, from which a browser typesets the PDF: formulas converted where they can be, diagrams marked for the drawing the page puts there.
//!
//! The server compiles nothing: it writes the document as Typst source and
//! names the images that source uses, and the reader's browser typesets the
//! PDF, on the paper it chooses (the source sets none). Colours, alignment,
//! columns, page breaks and cell spans are kept, which Markdown cannot.
//!
//! A formula becomes an equation where [`latex_to_typst`] can write one, and
//! is otherwise its LaTeX source in a raw block, marked `// orch:formula`. A
//! diagram is its Mermaid source in a figure, preceded by one line,
//! `// orch:diagram <block id>`: whoever compiles the source can replace that
//! line and the single line after it by `#image("diagrams/<block id>.svg")`
//! once the drawing is made. Every piece of text is escaped as Typst reads it,
//! so that no block writes markup into the source.

use std::fmt::Write as _;

use orch_api_contract::documents::{
    BlockContent, BlockType, DocumentBlock, DocumentDiscussion, Inline, InlineRefKind, Mark,
    TableCell,
};
use orch_types::{Id, Json};

use crate::blocks::{self, Lookup, Outline};
use crate::files;

/// What the source is asked to hold beside the text and the images.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options<'a> {
    /// The discussions beside the document, to be written after the text.
    pub discussions: Option<&'a [DocumentDiscussion]>,
    /// Whether the files that are no images go with the source in an archive,
    /// under `files/`: they are then linked, and otherwise only named.
    pub attachments_beside: bool,
}

/// An image the source uses, to be put in the archive at `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageUse {
    /// Where the source finds it, from the source's own directory.
    pub path: String,
    /// The file of the document that holds it.
    pub file_id: Id,
}

/// The source of a document and what it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The Typst source.
    pub source: String,
    /// The images it uses.
    pub images: Vec<ImageUse>,
    /// The diagrams it marks, by the block they are.
    pub diagrams: Vec<Id>,
}

const TINT_TEXT: [&str; 12] = [
    "#59636e", "#8a5a2b", "#bc4c00", "#7d6b00", "#1a7f37", "#0f7b7b", "#0969da", "#8250df",
    "#bf3989", "#cf222e", "#44546a", "#6e7781",
];
const TINT_FILL: [&str; 12] = [
    "#eef0f2", "#f3e6d8", "#ffe8d1", "#fff4c2", "#dcf5e1", "#d5f3f3", "#ddeeff", "#ece4ff",
    "#ffe3f1", "#ffe0e2", "#e1e6ee", "#e9ebee",
];

fn tint(list: &[&'static str; 12], index: u32) -> Option<&'static str> {
    list.get(usize::try_from(index).ok()?).copied()
}

/// A string as Typst reads one between quotes.
#[must_use]
pub fn string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Text as Typst markup reads it, so that it comes out as the same text.
#[must_use]
pub fn escape(text: &str, at_line_start: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, c) in chars.iter().copied().enumerate() {
        let next = chars.get(index + 1).copied();
        let before = index.checked_sub(1).map(|at| chars[at]);
        let line_start = index == 0 && at_line_start;
        match c {
            '\\' | '#' | '$' | '*' | '_' | '`' | '<' | '>' | '@' | '[' | ']' | '~' => {
                out.push('\\');
                out.push(c);
            }
            // A comment starts with two slashes; a slash at the start of a line is a term.
            '/' if line_start || matches!(next, Some('/' | '*')) => out.push_str("\\/"),
            // Two hyphens are a dash; one at the start of a line is a list.
            '-' if line_start || matches!(next, Some('-' | '?')) || before == Some('-') => {
                out.push_str("\\-");
            }
            '+' | '=' if line_start => {
                out.push('\\');
                out.push(c);
            }
            // "12." at the start of a line is a list.
            '.' if at_line_start
                && index > 0
                && chars[..index].iter().all(char::is_ascii_digit) =>
            {
                out.push_str("\\.");
            }
            c => out.push(c),
        }
    }
    out
}

// ------------------------------------------------------------------ formulas

fn symbol_for(command: &str) -> Option<&'static str> {
    Some(match command {
        "alpha" => "alpha",
        "beta" => "beta",
        "gamma" => "gamma",
        "delta" => "delta",
        "epsilon" => "epsilon",
        "varepsilon" => "epsilon.alt",
        "zeta" => "zeta",
        "eta" => "eta",
        "theta" => "theta",
        "vartheta" => "theta.alt",
        "iota" => "iota",
        "kappa" => "kappa",
        "lambda" => "lambda",
        "mu" => "mu",
        "nu" => "nu",
        "xi" => "xi",
        "pi" => "pi",
        "varpi" => "pi.alt",
        "rho" => "rho",
        "sigma" => "sigma",
        "tau" => "tau",
        "upsilon" => "upsilon",
        "phi" => "phi.alt",
        "varphi" => "phi",
        "chi" => "chi",
        "psi" => "psi",
        "omega" => "omega",
        "Gamma" => "Gamma",
        "Delta" => "Delta",
        "Theta" => "Theta",
        "Lambda" => "Lambda",
        "Xi" => "Xi",
        "Pi" => "Pi",
        "Sigma" => "Sigma",
        "Upsilon" => "Upsilon",
        "Phi" => "Phi",
        "Psi" => "Psi",
        "Omega" => "Omega",
        "cdot" => "dot.op",
        "times" => "times",
        "div" => "div",
        "pm" => "plus.minus",
        "mp" => "minus.plus",
        "leq" | "le" => "lt.eq",
        "geq" | "ge" => "gt.eq",
        "neq" | "ne" => "eq.not",
        "ll" => "lt.double",
        "gg" => "gt.double",
        "approx" => "approx",
        "equiv" => "equiv",
        "sim" => "tilde.op",
        "propto" => "prop",
        "infty" => "infinity",
        "partial" => "partial",
        "nabla" => "nabla",
        "forall" => "forall",
        "exists" => "exists",
        "in" => "in",
        "notin" => "in.not",
        "subset" => "subset",
        "subseteq" => "subset.eq",
        "supset" => "supset",
        "supseteq" => "supset.eq",
        "cup" => "union",
        "cap" => "inter",
        "emptyset" => "emptyset",
        "to" | "rightarrow" => "arrow.r",
        "leftarrow" => "arrow.l",
        "Rightarrow" => "arrow.r.double",
        "Leftarrow" => "arrow.l.double",
        "leftrightarrow" => "arrow.l.r",
        "Leftrightarrow" => "arrow.l.r.double",
        "mapsto" => "arrow.r.bar",
        "ldots" | "dots" => "dots.h",
        "cdots" => "dots.h.c",
        "vdots" => "dots.v",
        "ddots" => "dots.down",
        "sum" => "sum",
        "prod" => "product",
        "int" => "integral",
        "oint" => "integral.cont",
        "angle" => "angle",
        "perp" => "perp",
        "parallel" => "parallel",
        "langle" => "angle.l",
        "rangle" => "angle.r",
        "neg" | "lnot" => "not",
        "land" => "and",
        "lor" => "or",
        "circ" => "compose",
        "ast" => "ast.op",
        "star" => "star",
        "prime" => "prime",
        "hbar" => "planck.reduce",
        "ell" => "ell",
        "Re" => "Re",
        "Im" => "Im",
        _ => return None,
    })
}

fn operator_for(command: &str) -> Option<&'static str> {
    Some(match command {
        "lim" => "lim",
        "log" => "log",
        "ln" => "ln",
        "lg" => "lg",
        "exp" => "exp",
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "cot" => "cot",
        "sec" => "sec",
        "csc" => "csc",
        "arcsin" => "arcsin",
        "arccos" => "arccos",
        "arctan" => "arctan",
        "sinh" => "sinh",
        "cosh" => "cosh",
        "tanh" => "tanh",
        "min" => "min",
        "max" => "max",
        "sup" => "sup",
        "inf" => "inf",
        "det" => "det",
        "dim" => "dim",
        "gcd" => "gcd",
        "ker" => "ker",
        "deg" => "deg",
        "arg" => "arg",
        "mod" => "mod",
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Command(String),
    Open,
    Close,
    Sup,
    Sub,
    Amp,
    Row,
    Space,
    Char(char),
}

fn tokens(source: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        at += 1;
        match c {
            '\\' => {
                let first = *chars.get(at)?;
                if first.is_ascii_alphabetic() {
                    let mut name = String::new();
                    while chars.get(at).is_some_and(char::is_ascii_alphabetic) {
                        name.push(chars[at]);
                        at += 1;
                    }
                    out.push(Tok::Command(name));
                } else {
                    at += 1;
                    out.push(if first == '\\' {
                        Tok::Row
                    } else {
                        Tok::Command(first.to_string())
                    });
                }
            }
            '{' => out.push(Tok::Open),
            '}' => out.push(Tok::Close),
            '^' => out.push(Tok::Sup),
            '_' => out.push(Tok::Sub),
            '&' => out.push(Tok::Amp),
            '%' | '$' | '#' | '~' => return None,
            c if c.is_whitespace() => {
                if out.last() != Some(&Tok::Space) {
                    out.push(Tok::Space);
                }
            }
            c => out.push(Tok::Char(c)),
        }
    }
    Some(out)
}

struct Converter {
    toks: Vec<Tok>,
    at: usize,
}

impl Converter {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn skip_spaces(&mut self) {
        while self.peek() == Some(&Tok::Space) {
            self.at += 1;
        }
    }

    fn take(&mut self) -> Option<Tok> {
        let tok = self.toks.get(self.at).cloned();
        if tok.is_some() {
            self.at += 1;
        }
        tok
    }

    /// The text of a `{...}` group as written in LaTeX, for `\text`, `\mathrm` and environments.
    fn raw_group(&mut self) -> Option<String> {
        self.skip_spaces();
        if self.take()? != Tok::Open {
            return None;
        }
        let mut out = String::new();
        let mut depth = 1;
        while let Some(tok) = self.take() {
            match tok {
                Tok::Open => {
                    depth += 1;
                    out.push('{');
                }
                Tok::Close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(out);
                    }
                    out.push('}');
                }
                Tok::Char(c) => out.push(c),
                Tok::Space => out.push(' '),
                Tok::Command(name) if name == " " => out.push(' '),
                _ => return None,
            }
        }
        None
    }

    /// A group `{...}` or one token, converted, and whether it is a single unit.
    fn argument(&mut self) -> Option<(String, bool)> {
        self.skip_spaces();
        if self.peek() == Some(&Tok::Open) {
            self.take();
            let inner = self.sequence(Some(&Tok::Close))?;
            let single = !inner.contains(' ') && !inner.contains('(');
            return Some((inner, single));
        }
        let text = self.atom()?;
        let single = !text.contains(' ');
        Some((text, single))
    }

    fn grouped(text: &str, single: bool) -> String {
        if single {
            text.to_owned()
        } else {
            format!("({text})")
        }
    }

    /// One thing: a letter, a number, a symbol, a command with its arguments.
    fn atom(&mut self) -> Option<String> {
        self.skip_spaces();
        match self.take()? {
            Tok::Char(c) if c.is_ascii_alphabetic() => Some(c.to_string()),
            Tok::Char(c) if c.is_ascii_digit() => {
                let mut number = c.to_string();
                while let Some(Tok::Char(d)) = self.peek() {
                    if d.is_ascii_digit() || *d == '.' {
                        number.push(*d);
                        self.at += 1;
                    } else {
                        break;
                    }
                }
                Some(number)
            }
            Tok::Char(c) => Some(match c {
                ',' | ';' => format!("\\{c}"),
                '"' => return None,
                '{' | '}' => return None,
                other => other.to_string(),
            }),
            Tok::Open => {
                let inner = self.sequence(Some(&Tok::Close))?;
                Some(inner)
            }
            Tok::Command(name) => self.command(&name),
            Tok::Sup | Tok::Sub | Tok::Amp | Tok::Row | Tok::Close | Tok::Space => None,
        }
    }

    fn command(&mut self, name: &str) -> Option<String> {
        if let Some(symbol) = symbol_for(name) {
            return Some(symbol.to_owned());
        }
        if let Some(operator) = operator_for(name) {
            return Some(operator.to_owned());
        }
        match name {
            "frac" | "dfrac" | "tfrac" => {
                let (top, _) = self.argument()?;
                let (bottom, _) = self.argument()?;
                Some(format!("({top}) / ({bottom})"))
            }
            "sqrt" => {
                if self.peek() == Some(&Tok::Char('[')) {
                    self.take();
                    let mut index = String::new();
                    while let Some(tok) = self.take() {
                        match tok {
                            Tok::Char(']') => break,
                            Tok::Char(c) if c.is_ascii_alphanumeric() => index.push(c),
                            _ => return None,
                        }
                    }
                    let (radicand, _) = self.argument()?;
                    return Some(format!("root({index}, {radicand})"));
                }
                let (radicand, _) = self.argument()?;
                Some(format!("sqrt({radicand})"))
            }
            "text" | "textrm" | "mbox" => Some(string(&self.raw_group()?)),
            "mathrm" | "operatorname" | "mathit" | "textit" => {
                let text = self.raw_group()?;
                Some(format!("upright({})", string(&text)))
            }
            "mathbf" | "boldsymbol" | "bm" | "textbf" => {
                let (inner, _) = self.argument()?;
                Some(format!("bold({inner})"))
            }
            "mathbb" => {
                let text = self.raw_group()?;
                Some(match text.as_str() {
                    "R" => "RR".to_owned(),
                    "N" => "NN".to_owned(),
                    "Z" => "ZZ".to_owned(),
                    "Q" => "QQ".to_owned(),
                    "C" => "CC".to_owned(),
                    "H" => "HH".to_owned(),
                    "P" => "PP".to_owned(),
                    "E" => "EE".to_owned(),
                    other if other.chars().count() == 1 => format!("bb({other})"),
                    _ => return None,
                })
            }
            "mathcal" | "mathscr" => {
                let (inner, _) = self.argument()?;
                Some(format!("cal({inner})"))
            }
            "hat" | "widehat" => self.accent("hat"),
            "bar" | "overline" => self.accent("overline"),
            "tilde" | "widetilde" => self.accent("tilde"),
            "vec" => self.accent("arrow"),
            "dot" => self.accent("dot"),
            "ddot" => self.accent("dot.double"),
            "underline" => self.accent("underline"),
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "displaystyle" => {
                // The delimiters stay; the sizing does not matter to Typst.
                match self.peek() {
                    Some(Tok::Char('.')) => {
                        self.take();
                        Some(String::new())
                    }
                    _ => Some(String::new()),
                }
            }
            "," => Some("thin".to_owned()),
            ";" | ":" | ">" => Some("med".to_owned()),
            " " => Some("space".to_owned()),
            "quad" => Some("quad".to_owned()),
            "qquad" => Some("wide".to_owned()),
            "!" => Some(String::new()),
            "{" => Some("{".to_owned()),
            "}" => Some("}".to_owned()),
            "|" => Some("parallel".to_owned()),
            "%" | "&" | "#" | "_" | "$" => Some(format!("\\{name}")),
            "begin" => self.environment(),
            _ => None,
        }
    }

    fn accent(&mut self, function: &str) -> Option<String> {
        let (inner, _) = self.argument()?;
        Some(format!("{function}({inner})"))
    }

    fn environment(&mut self) -> Option<String> {
        let name = self.raw_group()?;
        let delimiter = match name.as_str() {
            "matrix" => None,
            "pmatrix" => Some("\"(\""),
            "bmatrix" => Some("\"[\""),
            "Bmatrix" => Some("\"{\""),
            "vmatrix" => Some("\"|\""),
            _ => return None,
        };
        let mut rows: Vec<Vec<String>> = vec![Vec::new()];
        loop {
            let cell = self.sequence_until_cell()?;
            let (text, ended) = cell;
            if let Some(row) = rows.last_mut() {
                row.push(text);
            }
            match ended {
                CellEnd::Cell => {}
                CellEnd::Row => rows.push(Vec::new()),
                CellEnd::Environment => break,
            }
        }
        if rows
            .last()
            .is_some_and(|row| row.iter().all(String::is_empty))
        {
            rows.pop();
        }
        let body: Vec<String> = rows.iter().map(|row| row.join(", ")).collect();
        let delim = delimiter.map_or(String::new(), |delim| format!("delim: {delim}, "));
        Some(format!("mat({delim}{})", body.join("; ")))
    }

    fn sequence_until_cell(&mut self) -> Option<(String, CellEnd)> {
        let mut out: Vec<String> = Vec::new();
        loop {
            self.skip_spaces();
            match self.peek()? {
                Tok::Amp => {
                    self.take();
                    return Some((out.join(" "), CellEnd::Cell));
                }
                Tok::Row => {
                    self.take();
                    return Some((out.join(" "), CellEnd::Row));
                }
                Tok::Command(name) if name == "end" => {
                    self.take();
                    self.raw_group()?;
                    return Some((out.join(" "), CellEnd::Environment));
                }
                _ => out.push(self.term()?),
            }
        }
    }

    /// An atom with its scripts.
    fn term(&mut self) -> Option<String> {
        let mut base = self.atom()?;
        loop {
            self.skip_spaces();
            match self.peek() {
                Some(Tok::Sup) => {
                    self.take();
                    let (script, single) = self.argument()?;
                    base = format!("{base}^{}", Self::grouped(&script, single));
                }
                Some(Tok::Sub) => {
                    self.take();
                    let (script, single) = self.argument()?;
                    base = format!("{base}_{}", Self::grouped(&script, single));
                }
                _ => return Some(base),
            }
        }
    }

    fn sequence(&mut self, until: Option<&Tok>) -> Option<String> {
        let mut out: Vec<String> = Vec::new();
        loop {
            self.skip_spaces();
            match self.peek() {
                None => {
                    return if until.is_none() {
                        Some(out.join(" "))
                    } else {
                        None
                    };
                }
                Some(tok) if Some(tok) == until => {
                    self.take();
                    return Some(out.join(" "));
                }
                Some(_) => {
                    let term = self.term()?;
                    if !term.is_empty() {
                        out.push(term);
                    }
                }
            }
        }
    }
}

enum CellEnd {
    Cell,
    Row,
    Environment,
}

/// Writes a LaTeX formula as Typst math, where it can: the common commands,
/// fractions, roots, scripts, accents, the usual alphabets and matrices.
/// Whatever else a formula uses, it returns none, and the formula is kept as
/// its source.
#[must_use]
pub fn latex_to_typst(source: &str) -> Option<String> {
    let mut converter = Converter {
        toks: tokens(source)?,
        at: 0,
    };
    if converter.toks.is_empty() {
        return None;
    }
    let math = converter.sequence(None)?;
    (!math.trim().is_empty()).then_some(math)
}

// ------------------------------------------------------------------ the document

fn prop(block: &DocumentBlock, member: &str) -> Option<Json> {
    serde_json::to_value(&block.props)
        .ok()
        .and_then(|props| props.get(member).cloned())
}

fn runs_of(block: &DocumentBlock) -> &[Inline] {
    match &block.content {
        BlockContent::Inline(runs) => runs,
        BlockContent::Rows(_) => &[],
    }
}

fn fill_of(block: &DocumentBlock) -> (Option<&'static str>, Option<&'static str>) {
    let number = |member: &str| {
        prop(block, member)
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
    };
    (
        number("color").and_then(|n| tint(&TINT_TEXT, n)),
        number("background").and_then(|n| tint(&TINT_FILL, n)),
    )
}

/// What a piece of the inline text is, so that two pieces can be kept from
/// running into each other: a call (`#strong[..]`) must not be followed by a
/// text that Typst would read as more of the call.
enum Piece {
    Call(String),
    Text(String),
}

fn join_pieces(pieces: Vec<Piece>) -> String {
    let mut out = String::new();
    let mut after_call = false;
    for piece in pieces {
        match piece {
            Piece::Call(text) => {
                out.push_str(&text);
                after_call = true;
            }
            Piece::Text(text) => {
                let runs_on = text.starts_with('(')
                    || text.starts_with('[')
                    || (text.starts_with('.')
                        && text
                            .chars()
                            .nth(1)
                            .is_some_and(|c| c.is_alphabetic() || c == '_'));
                if after_call && runs_on {
                    // An empty comment ends the call.
                    out.push_str("/**/");
                }
                out.push_str(&text);
                after_call = false;
            }
        }
    }
    out
}

struct Source<'a, 'b> {
    lookup: &'a dyn Lookup,
    outline: &'b Outline<'b>,
    options: Options<'a>,
    images: Vec<ImageUse>,
    diagrams: Vec<Id>,
}

fn piece_text(piece: &Piece) -> String {
    match piece {
        Piece::Call(text) | Piece::Text(text) => text.clone(),
    }
}

fn indent_lines(text: &str, indent: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .rsplit('@')
        .next()
        .unwrap_or(rest)
        .to_owned()
}

fn size_text(size: i64) -> String {
    const KIB: i64 = 1024;
    if size >= KIB * KIB {
        format!("{:.1} MB", size as f64 / (KIB * KIB) as f64)
    } else if size >= KIB {
        format!("{:.1} KB", size as f64 / KIB as f64)
    } else {
        format!("{size} bytes")
    }
}

/// The image types Typst can set.
fn typst_image(media_type: &str) -> bool {
    matches!(
        media_type.to_ascii_lowercase().as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/svg+xml" | "image/webp"
    )
}

fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl<'b> Source<'_, 'b> {
    fn inline(&self, runs: &[Inline], start: bool) -> String {
        let runs = crate::markdown::merged(runs);
        let mut pieces: Vec<Piece> = Vec::new();
        let mut at_start = start;
        let mut index = 0;
        while index < runs.len() {
            let href = match &runs[index] {
                Inline::Text {
                    href: Some(href), ..
                } if blocks::allowed_href(href) => Some(href.as_str()),
                _ => None,
            };
            let mut end = index + 1;
            if href.is_some() {
                while end < runs.len()
                    && matches!(&runs[end], Inline::Text { href: other, .. } if other.as_deref() == href)
                {
                    end += 1;
                }
            }
            let mut inner: Vec<Piece> = Vec::new();
            for run in &runs[index..end] {
                inner.push(self.run(run, &mut at_start));
            }
            match href {
                Some(href) => pieces.push(Piece::Call(format!(
                    "#link({})[{}]",
                    string(href),
                    join_pieces(inner)
                ))),
                None => pieces.extend(inner),
            }
            index = end;
        }
        join_pieces(pieces)
    }

    fn run(&self, run: &Inline, at_start: &mut bool) -> Piece {
        match run {
            Inline::Text {
                text,
                marks,
                color,
                background,
                ..
            } => {
                let marks = marks.clone().unwrap_or_default();
                let mut body = if marks.contains(&Mark::Code) {
                    Piece::Call(format!("#raw({})", string(text)))
                } else {
                    let lines: Vec<String> = text
                        .split('\n')
                        .enumerate()
                        .map(|(index, line)| escape(line, index > 0 || *at_start))
                        .collect();
                    Piece::Text(lines.join("\\\n"))
                };
                *at_start = false;
                for (mark, call) in [
                    (Mark::Bold, "strong"),
                    (Mark::Italic, "emph"),
                    (Mark::Underline, "underline"),
                    (Mark::Strike, "strike"),
                ] {
                    if marks.contains(&mark) {
                        body = Piece::Call(format!("#{call}[{}]", piece_text(&body)));
                    }
                }
                if let Some(color) = color.and_then(|n| tint(&TINT_TEXT, n)) {
                    body = Piece::Call(format!(
                        "#text(fill: rgb(\"{color}\"))[{}]",
                        piece_text(&body)
                    ));
                }
                if let Some(fill) = background.and_then(|n| tint(&TINT_FILL, n)) {
                    body = Piece::Call(format!(
                        "#highlight(fill: rgb(\"{fill}\"))[{}]",
                        piece_text(&body)
                    ));
                }
                body
            }
            Inline::Mention { user_id, .. } => {
                *at_start = false;
                let name = self
                    .lookup
                    .username(*user_id)
                    .unwrap_or_else(|| "unknown".to_owned());
                Piece::Text(format!("\\@{}", escape(&name, false)))
            }
            Inline::Ref { kind, id, .. } => {
                *at_start = false;
                let what = match kind {
                    InlineRefKind::Node => "a node",
                    InlineRefKind::Document => "a document",
                    InlineRefKind::Chat => "a chat",
                    InlineRefKind::File | InlineRefKind::EnvironmentFile => "a file",
                };
                let name = self
                    .lookup
                    .name_of(*kind, *id)
                    .unwrap_or_else(|| what.to_owned());
                Piece::Call(format!(
                    "#box(fill: luma(238), inset: (x: 2pt), radius: 2pt)[{}]",
                    escape(&name, false)
                ))
            }
            Inline::Math { source } => {
                *at_start = false;
                match latex_to_typst(source) {
                    Some(math) => Piece::Text(format!("${math}$")),
                    None => Piece::Call(format!("#raw({})", string(source))),
                }
            }
            Inline::Break => {
                *at_start = true;
                Piece::Text("\\\n".to_owned())
            }
        }
    }

    fn sequence(&mut self, siblings: &[&'b DocumentBlock]) -> String {
        let mut chunks: Vec<String> = Vec::new();
        let mut list: Vec<String> = Vec::new();
        let mut ordered = false;
        let mut number = 0_u64;
        for block in siblings {
            match block.block_type {
                BlockType::Bullet | BlockType::Todo | BlockType::Toggle => {
                    if ordered && !list.is_empty() {
                        chunks.push(list.join("\n"));
                        list.clear();
                    }
                    ordered = false;
                    list.push(self.item(block, None));
                }
                BlockType::Numbered => {
                    if !ordered && !list.is_empty() {
                        chunks.push(list.join("\n"));
                        list.clear();
                    }
                    if list.is_empty() {
                        number = prop(block, "start").and_then(|v| v.as_u64()).unwrap_or(1);
                    } else {
                        number += 1;
                    }
                    ordered = true;
                    list.push(self.item(block, Some(number)));
                }
                _ => {
                    if !list.is_empty() {
                        chunks.push(list.join("\n"));
                        list.clear();
                    }
                    let text = self.block(block);
                    if !text.is_empty() {
                        chunks.push(text);
                    }
                }
            }
        }
        if !list.is_empty() {
            chunks.push(list.join("\n"));
        }
        chunks.join("\n\n")
    }

    fn item(&mut self, block: &'b DocumentBlock, number: Option<u64>) -> String {
        let (color, _) = fill_of(block);
        let mut text = self.inline(runs_of(block), true).replace("\\\n", " \\\n");
        if let Some(color) = color {
            text = format!("#text(fill: rgb(\"{color}\"))[{text}]");
        }
        let marker = match (block.block_type, number) {
            (BlockType::Todo, _) => {
                if prop(block, "checked").and_then(|v| v.as_bool()) == Some(true) {
                    "- ☑ ".to_owned()
                } else {
                    "- ☐ ".to_owned()
                }
            }
            (BlockType::Numbered, Some(n)) => format!("{n}. "),
            _ => "- ".to_owned(),
        };
        let indent = " ".repeat(marker.chars().count());
        let mut out = String::new();
        for (index, line) in text.split('\n').enumerate() {
            if index == 0 {
                out.push_str(&marker);
            } else {
                out.push('\n');
                out.push_str(&indent);
            }
            out.push_str(line);
        }
        let outline = self.outline;
        let children = outline.children_of(block.id);
        if !children.is_empty() {
            let inside = self.sequence(&children);
            out.push('\n');
            out.push_str(&indent_lines(&inside, &indent));
        }
        out
    }

    /// Wraps the markup of a block in what its look asks for.
    fn dressed(&self, block: &DocumentBlock, inner: String) -> String {
        let (color, fill) = fill_of(block);
        let mut out = inner;
        if let Some(color) = color {
            out = format!("#text(fill: rgb(\"{color}\"))[{out}]");
        }
        if let Some(fill) = fill {
            out = format!(
                "#block(width: 100%, fill: rgb(\"{fill}\"), inset: 4pt, radius: 2pt)[{out}]"
            );
        }
        match prop(block, "align")
            .and_then(|v| v.as_str().map(str::to_owned))
            .as_deref()
        {
            Some("center") => format!("#align(center)[{out}]"),
            Some("right") => format!("#align(right)[{out}]"),
            Some("justify") => format!("#par(justify: true)[{out}]"),
            _ => out,
        }
    }

    fn block(&mut self, block: &'b DocumentBlock) -> String {
        let outline = self.outline;
        match block.block_type {
            BlockType::Paragraph => {
                let text = self.inline(runs_of(block), true);
                let children = outline.children_of(block.id);
                let mut out = if text.trim().is_empty() {
                    String::new()
                } else {
                    self.dressed(block, text)
                };
                if !children.is_empty() {
                    let inside = self.sequence(&children);
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(&inside);
                }
                out
            }
            BlockType::Heading => {
                let level = prop(block, "level")
                    .and_then(|v| v.as_u64())
                    .map_or(1, |level| level.clamp(1, 6));
                let text = self.inline(runs_of(block), false).replace("\\\n", " ");
                let marks = "=".repeat(usize::try_from(level).unwrap_or(1));
                let mut out = self.dressed(block, format!("{marks} {text}"));
                if out.starts_with('#') {
                    // A decorated heading is a heading inside a call.
                    out = format!("{marks} {text}");
                }
                let children = outline.children_of(block.id);
                if !children.is_empty() {
                    out.push_str("\n\n");
                    out.push_str(&self.sequence(&children));
                }
                out
            }
            BlockType::Quote | BlockType::Callout => {
                let text = self.inline(runs_of(block), true);
                let children = outline.children_of(block.id);
                let mut inside = text;
                if !children.is_empty() {
                    let more = self.sequence(&children);
                    if !inside.is_empty() {
                        inside.push_str("\n\n");
                    }
                    inside.push_str(&more);
                }
                let inside = indent_lines(&inside, "  ");
                if block.block_type == BlockType::Quote {
                    let quote = format!("#quote(block: true)[\n{inside}\n]");
                    return self.dressed(block, quote);
                }
                let tone = prop(block, "tone")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "info".to_owned());
                let (edge, fill) = match tone.as_str() {
                    "success" => ("#1a7f37", "#e6f6ea"),
                    "warning" => ("#bf8700", "#fff6d6"),
                    "danger" => ("#cf222e", "#ffe9ea"),
                    _ => ("#0969da", "#e6f0fb"),
                };
                let callout = format!(
                    "#block(width: 100%, fill: rgb(\"{fill}\"), stroke: (left: 2pt + rgb(\"{edge}\")), inset: 8pt, radius: 3pt)[\n{inside}\n]"
                );
                self.dressed(block, callout)
            }
            BlockType::Code => {
                let language = prop(block, "language")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let body = string(&blocks::inline_text(runs_of(block)));
                if language.is_empty() {
                    format!("#raw(block: true, {body})")
                } else {
                    format!("#raw(block: true, lang: {}, {body})", string(&language))
                }
            }
            BlockType::Math => {
                let source = prop(block, "source")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                match latex_to_typst(&source) {
                    Some(math) => format!("$ {math} $"),
                    None => format!(
                        "// orch:formula\n#raw(block: true, lang: \"latex\", {})",
                        string(&source)
                    ),
                }
            }
            BlockType::Diagram => {
                let source = prop(block, "source")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                self.diagrams.push(block.id);
                format!(
                    "// orch:diagram {}\n#figure(raw(block: true, lang: \"mermaid\", {}), caption: [Diagram, as its Mermaid source])",
                    block.id,
                    string(&source)
                )
            }
            BlockType::Divider => "#line(length: 100%)".to_owned(),
            BlockType::PageBreak => "#pagebreak()".to_owned(),
            BlockType::Image => self.image(block),
            BlockType::Video | BlockType::Audio | BlockType::File => self.attachment(block),
            BlockType::Table => self.table(block),
            BlockType::Columns => {
                let columns = outline.children_of(block.id);
                if columns.is_empty() {
                    return String::new();
                }
                let shares: Vec<String> = columns
                    .iter()
                    .map(|column| {
                        let share = prop(column, "share")
                            .and_then(|v| v.as_f64())
                            .filter(|share| share.is_finite() && *share > 0.0)
                            .unwrap_or(1.0);
                        format!("{share}fr")
                    })
                    .collect();
                let mut cells = Vec::new();
                for column in columns {
                    let inside = outline.children_of(column.id);
                    let text = self.sequence(&inside);
                    cells.push(format!("[\n{}\n]", indent_lines(&text, "  ")));
                }
                format!(
                    "#grid(columns: ({},), column-gutter: 12pt,\n{}\n)",
                    shares.join(", "),
                    indent_lines(&cells.join(",\n"), "  ")
                )
            }
            BlockType::Column => {
                let inside = outline.children_of(block.id);
                self.sequence(&inside)
            }
            BlockType::Bullet | BlockType::Numbered | BlockType::Todo | BlockType::Toggle => {
                self.item(block, None)
            }
        }
    }

    fn image(&mut self, block: &DocumentBlock) -> String {
        let caption = self.inline(runs_of(block), false);
        let width = prop(block, "width")
            .and_then(|v| v.as_f64())
            .filter(|w| w.is_finite() && *w > 0.0)
            .map_or_else(String::new, |w| format!(", width: {w}pt"));
        let file = prop(block, "file_id")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<Id>().ok()))
            .and_then(|id| self.lookup.file(id).map(|file| (id, file)));
        let Some((id, file)) = file.filter(|(_, file)| typst_image(&file.media_type)) else {
            return "#text(fill: luma(100))[An image that is not in this document.]".to_owned();
        };
        let path = format!("images/{}-{}", self.images.len() + 1, safe_name(&file.name));
        self.images.push(ImageUse {
            path: path.clone(),
            file_id: id,
        });
        let figure = if caption.is_empty() {
            format!("#figure(image({}{width}))", string(&path))
        } else {
            format!(
                "#figure(image({}{width}), caption: [{caption}])",
                string(&path)
            )
        };
        match prop(block, "align")
            .and_then(|v| v.as_str().map(str::to_owned))
            .as_deref()
        {
            Some("right") => format!("#align(right)[{figure}]"),
            Some("left") => format!("#align(left)[{figure}]"),
            _ => figure,
        }
    }

    fn attachment(&mut self, block: &DocumentBlock) -> String {
        let kind = match block.block_type {
            BlockType::Video => "Film",
            BlockType::Audio => "Sound",
            _ => "File",
        };
        let caption = self.inline(runs_of(block), false);
        if let Some(url) = prop(block, "url").and_then(|v| v.as_str().map(str::to_owned))
            && blocks::allowed_media_url(&url)
        {
            return format!(
                "#text(fill: luma(100))[{kind}: #link({})[Play from {}]]",
                string(&url),
                escape(&host_of(&url), false)
            );
        }
        let file = prop(block, "file_id")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<Id>().ok()))
            .and_then(|id| self.lookup.file(id));
        let Some(file) = file else {
            return format!("#text(fill: luma(100))[{kind}: not in this document]");
        };
        let label = if caption.is_empty() {
            escape(&file.name, false)
        } else {
            caption
        };
        let what = escape(
            &format!("{}, {}", file.media_type, size_text(file.size)),
            false,
        );
        if self.options.attachments_beside && !files::is_image(&file.media_type) {
            format!(
                "#text(fill: luma(100))[{kind}: #link({})[{label}] ({what})]",
                string(&format!("files/{}", file.name))
            )
        } else {
            format!(
                "#text(fill: luma(100))[{kind}: {label} ({}, {what}; not in this document)]",
                escape(&file.name, false)
            )
        }
    }

    fn cell(&self, cell: &TableCell, head: bool, strong_column: bool) -> String {
        let mut inner = self.inline(&cell.content, false).replace("\\\n", " \\\n");
        if head || strong_column {
            inner = format!("#strong[{inner}]");
        }
        let mut args = Vec::new();
        if let Some(span) = cell.colspan.filter(|n| *n > 1) {
            args.push(format!("colspan: {span}"));
        }
        if let Some(span) = cell.rowspan.filter(|n| *n > 1) {
            args.push(format!("rowspan: {span}"));
        }
        if let Some(fill) = cell.background.and_then(|n| tint(&TINT_FILL, n)) {
            args.push(format!("fill: rgb(\"{fill}\")"));
        }
        if let Some(color) = cell.color.and_then(|n| tint(&TINT_TEXT, n)) {
            inner = format!("#text(fill: rgb(\"{color}\"))[{inner}]");
        }
        if args.is_empty() {
            format!("[{inner}]")
        } else {
            format!("table.cell({})[{inner}]", args.join(", "))
        }
    }

    fn table(&self, block: &DocumentBlock) -> String {
        let BlockContent::Rows(rows) = &block.content else {
            return String::new();
        };
        let Some(first) = rows.first() else {
            return String::new();
        };
        let span = |value: Option<u32>| usize::try_from(value.unwrap_or(1)).unwrap_or(1).max(1);
        let width: usize = first.cells.iter().map(|cell| span(cell.colspan)).sum();
        if width == 0 {
            return String::new();
        }
        let header_row = prop(block, "header_row").and_then(|v| v.as_bool()) == Some(true);
        let header_column = prop(block, "header_column").and_then(|v| v.as_bool()) == Some(true);
        let widths: Vec<f64> = prop(block, "widths")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let columns = if widths.len() == width {
            widths.iter().map(|w| format!("{w}fr")).collect::<Vec<_>>()
        } else {
            vec!["1fr".to_owned(); width]
        };
        let mut out = format!("#table(\n  columns: ({},),\n", columns.join(", "));
        for (r, row) in rows.iter().enumerate() {
            let cells: Vec<String> = row
                .cells
                .iter()
                .enumerate()
                .map(|(c, cell)| self.cell(cell, r == 0 && header_row, c == 0 && header_column))
                .collect();
            if r == 0 && header_row {
                let _ = writeln!(out, "  table.header({}),", cells.join(", "));
            } else {
                let _ = writeln!(out, "  {},", cells.join(", "));
            }
        }
        out.push(')');
        out
    }

    fn comments(
        &mut self,
        blocks_list: &[DocumentBlock],
        discussions: &[DocumentDiscussion],
    ) -> String {
        let order: Vec<Id> = Outline::new(blocks_list)
            .walk()
            .into_iter()
            .map(|(block, _)| block.id)
            .collect();
        let mut sorted: Vec<&DocumentDiscussion> = discussions.iter().collect();
        sorted.sort_by_key(|discussion| {
            (
                discussion
                    .block_id
                    .and_then(|id| order.iter().position(|each| *each == id))
                    .unwrap_or(usize::MAX),
                discussion.created_at,
            )
        });
        let mut out = vec!["= Comments".to_owned()];
        for discussion in sorted {
            let about = match discussion
                .block_id
                .and_then(|id| blocks_list.iter().find(|block| block.id == id))
            {
                Some(block) => {
                    let text = blocks::plain_text(
                        block.block_type,
                        &serde_json::to_value(&block.props).unwrap_or(Json::Null),
                        &block.content,
                    );
                    format!(
                        "On “{}”",
                        escape(text.lines().next().unwrap_or("").trim(), false)
                    )
                }
                None => "On the document as a whole".to_owned(),
            };
            let mut lines = vec![format!("#text(weight: \"bold\")[{about}]")];
            if !discussion.quote.is_empty() {
                lines.push(format!(
                    "#quote(block: true)[{}]",
                    escape(&discussion.quote, false)
                ));
            }
            if let Some(resolved) = &discussion.resolved {
                lines.push(format!(
                    "#text(fill: luma(100))[Resolved by {} at {}]",
                    escape(&resolved.by.display_name, false),
                    resolved.at.get().format("%Y-%m-%d %H:%M UTC")
                ));
            }
            for comment in &discussion.comments {
                let said = if comment.deleted {
                    "#emph[(deleted)]".to_owned()
                } else {
                    self.inline(&comment.content, false).replace("\\\n", " ")
                };
                lines.push(format!(
                    "#strong[{}] #text(fill: luma(100), size: 9pt)[{}] \\\n{said}",
                    escape(&comment.author.display_name, false),
                    comment.created_at.get().format("%Y-%m-%d %H:%M UTC")
                ));
            }
            let body = indent_lines(&lines.join("\n\n"), "  ");
            out.push(format!(
                "#block(inset: (left: 8pt), stroke: (left: 1pt + luma(190)))[\n{body}\n]"
            ));
        }
        out.join("\n\n")
    }
}

/// Writes the live blocks of a document, under its title, as Typst source.
#[must_use]
pub fn render(
    title: &str,
    blocks_list: &[DocumentBlock],
    lookup: &dyn Lookup,
    options: &Options<'_>,
) -> Written {
    let outline = Outline::new(blocks_list);
    let mut source = Source {
        lookup,
        outline: &outline,
        options: *options,
        images: Vec::new(),
        diagrams: Vec::new(),
    };
    let roots = outline.roots();
    let body = source.sequence(&roots);
    let mut text = String::new();
    let _ = write!(
        text,
        "// Written by Orch. The paper is left to whoever compiles it.\n\
// A diagram is marked `orch:diagram <block id>`: that line and the line after it may be replaced\n\
// by an image of the drawing. A formula that could not be converted is marked `orch:formula`.\n\
#set document(title: {})\n\
#set text(size: 11pt)\n\
#set par(leading: 0.65em)\n\
#show raw.where(block: true): it => block(width: 100%, fill: luma(246), inset: 8pt, radius: 3pt, it)\n\
#align(center)[#text(size: 22pt, weight: \"bold\")[{}]]\n\n",
        string(title),
        escape(title, false)
    );
    text.push_str(&body);
    if let Some(discussions) = options.discussions
        && !discussions.is_empty()
    {
        text.push_str("\n\n");
        text.push_str(&source.comments(blocks_list, discussions));
    }
    text.push('\n');
    Written {
        source: text,
        images: source.images,
        diagrams: source.diagrams,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::FileInfo;
    use orch_api_contract::documents::{BlockProps, TableRow};
    use orch_types::new_id;
    use serde_json::json;
    use std::collections::HashMap;

    struct World {
        files: HashMap<Id, FileInfo>,
    }

    impl Lookup for World {
        fn username(&self, _user: Id) -> Option<String> {
            Some("ada".into())
        }

        fn name_of(&self, _kind: InlineRefKind, _id: Id) -> Option<String> {
            Some("Plans".into())
        }

        fn file(&self, id: Id) -> Option<FileInfo> {
            self.files.get(&id).cloned()
        }
    }

    fn nothing() -> World {
        World {
            files: HashMap::new(),
        }
    }

    fn plain(text: &str) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            marks: None,
            href: None,
            color: None,
            background: None,
        }
    }

    fn make(
        block_type: BlockType,
        props: Json,
        content: BlockContent,
        parent: Option<Id>,
        position: u32,
    ) -> DocumentBlock {
        let checked = blocks::check(
            block_type,
            serde_json::from_value::<BlockProps>(props).ok().as_ref(),
            Some(&content),
        )
        .unwrap();
        DocumentBlock {
            id: new_id(),
            document_id: Id::nil(),
            parent_id: parent,
            position: position.to_string(),
            block_type,
            props: blocks::props_from_json(&checked.props),
            content: checked.content,
            removed: false,
            seq_created: 1,
            seq_updated: 1,
            updated_by: None,
            updated_at: "2026-10-08T17:04:05.123Z".parse().unwrap(),
        }
    }

    fn text_block(block_type: BlockType, props: Json, words: &str, position: u32) -> DocumentBlock {
        make(
            block_type,
            props,
            BlockContent::Inline(vec![plain(words)]),
            None,
            position,
        )
    }

    fn written(blocks_list: &[DocumentBlock], world: &World) -> Written {
        render("Handbook", blocks_list, world, &Options::default())
    }

    #[test]
    fn text_is_escaped_as_typst_reads_it() {
        assert_eq!(
            escape("a # b $ c * d _ e", false),
            "a \\# b \\$ c \\* d \\_ e"
        );
        assert_eq!(
            escape("[x] <y> @z ~ `c` \\", false),
            "\\[x\\] \\<y\\> \\@z \\~ \\`c\\` \\\\"
        );
        assert_eq!(
            escape("see https://example.org", false),
            "see https:\\//example.org"
        );
        assert_eq!(escape("a -- b --- c", false), "a \\-\\- b \\-\\-\\- c");
        assert_eq!(escape("- not a list", true), "\\- not a list");
        assert_eq!(escape("+ not", true), "\\+ not");
        assert_eq!(escape("= not a heading", true), "\\= not a heading");
        assert_eq!(escape("12. not a list", true), "12\\. not a list");
        assert_eq!(escape("12. fine here", false), "12. fine here");
        assert_eq!(
            string("a \"quoted\" \\ line\nnext"),
            "\"a \\\"quoted\\\" \\\\ line\\nnext\""
        );
    }

    #[test]
    fn nothing_written_in_a_block_becomes_markup_of_the_source() {
        let nasty = "#import \"x\": *; #eval(\"1\") $ [ ] \\ </a>";
        let para = text_block(BlockType::Paragraph, json!({}), nasty, 1);
        let out = written(&[para], &nothing());
        assert!(
            out.source.contains(
                "\\#import \"x\": \\*; \\#eval(\"1\") \\$ \\[ \\] \\\\ \\<\\/a\\>"
                    .replace("\\<\\/a\\>", "\\</a\\>")
                    .as_str()
            ) || out.source.contains("\\#import")
        );
        assert!(!out.source.contains("\n#import"), "{}", out.source);
        assert!(!out.source.contains(" #eval"), "{}", out.source);
    }

    #[test]
    fn formulas_become_equations_where_they_can_and_stay_source_where_they_cannot() {
        let cases = [
            ("x^2", "x^2"),
            ("x_{ij}", "x_(i j)"),
            ("\\frac{a+b}{c}", "(a + b) / (c)"),
            ("\\sqrt{x}", "sqrt(x)"),
            ("\\sqrt[3]{x}", "root(3, x)"),
            ("\\alpha + \\beta \\leq \\gamma", "alpha + beta lt.eq gamma"),
            ("\\sum_{i=0}^{n} i", "sum_(i = 0)^n i"),
            ("\\int_0^1 f(x)\\,dx", "integral_0^1 f ( x ) thin d x"),
            ("\\mathbb{R} \\to \\mathbb{N}", "RR arrow.r NN"),
            ("\\text{if } x \\neq 0", "\"if \" x eq.not 0"),
            (
                "\\hat{x} + \\vec{v} + \\bar{y}",
                "hat(x) + arrow(v) + overline(y)",
            ),
            ("ab", "a b"),
            ("\\mathrm{d}x", "upright(\"d\") x"),
            ("f(a, b)", "f ( a \\, b )"),
        ];
        for (latex, typst) in cases {
            assert_eq!(latex_to_typst(latex).as_deref(), Some(typst), "{latex}");
        }
        assert_eq!(
            latex_to_typst("\\begin{pmatrix} 1 & 2 \\\\ 3 & 4 \\end{pmatrix}").as_deref(),
            Some("mat(delim: \"(\", 1, 2; 3, 4)")
        );
        for unknown in [
            "\\begin{align} a &= b \\end{align}",
            "\\unknowncommand{x}",
            "a % comment",
            "",
            "\\color{red} x",
        ] {
            assert_eq!(latex_to_typst(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn a_formula_block_is_an_equation_or_marked_source_and_a_diagram_is_marked_for_the_drawing() {
        let converted = make(
            BlockType::Math,
            json!({ "source": "E = mc^2" }),
            BlockContent::Inline(vec![]),
            None,
            1,
        );
        let kept = make(
            BlockType::Math,
            json!({ "source": "\\begin{align} a &= b \\end{align}" }),
            BlockContent::Inline(vec![]),
            None,
            2,
        );
        let diagram = make(
            BlockType::Diagram,
            json!({ "source": "graph TD\n  A-->B" }),
            BlockContent::Inline(vec![]),
            None,
            3,
        );
        let out = written(&[converted, kept, diagram.clone()], &nothing());
        assert!(out.source.contains("$ E = m c^2 $"), "{}", out.source);
        assert!(out.source.contains("// orch:formula\n#raw(block: true, lang: \"latex\", \"\\\\begin{align} a &= b \\\\end{align}\")"), "{}", out.source);
        let marker = format!(
            "// orch:diagram {}\n#figure(raw(block: true, lang: \"mermaid\", \"graph TD\\n  A-->B\"), caption: [Diagram, as its Mermaid source])",
            diagram.id
        );
        assert!(out.source.contains(&marker), "{}", out.source);
        assert_eq!(out.diagrams, [diagram.id]);
    }

    #[test]
    fn every_block_type_has_its_form() {
        let mut all: Vec<DocumentBlock> = Vec::new();
        let mut position = 0;
        let mut add = |block_type, props: Json, words: &str| {
            position += 1;
            all.push(text_block(block_type, props, words, position));
        };
        add(BlockType::Heading, json!({ "level": 2 }), "Head");
        add(
            BlockType::Paragraph,
            json!({ "align": "center", "color": 9, "background": 3 }),
            "centred",
        );
        add(BlockType::Bullet, json!({}), "bullet");
        add(BlockType::Numbered, json!({ "start": 4 }), "numbered");
        add(BlockType::Numbered, json!({}), "numbered two");
        add(BlockType::Todo, json!({ "checked": true }), "done");
        add(BlockType::Todo, json!({ "checked": false }), "to do");
        add(BlockType::Toggle, json!({}), "more");
        add(BlockType::Quote, json!({}), "quoted");
        add(BlockType::Callout, json!({ "tone": "warning" }), "careful");
        add(BlockType::Code, json!({ "language": "rust" }), "let x = 1;");
        let mut next = |block_type, props: Json| {
            position += 1;
            all.push(make(
                block_type,
                props,
                BlockContent::Inline(vec![]),
                None,
                position,
            ));
        };
        next(BlockType::Divider, json!({}));
        next(BlockType::PageBreak, json!({}));
        let out = written(&all, &nothing());
        for expected in [
            "== Head",
            "#align(center)[#block(width: 100%, fill: rgb(\"#fff4c2\"), inset: 4pt, radius: 2pt)[#text(fill: rgb(\"#cf222e\"))[centred]]]",
            "- bullet",
            "4. numbered\n5. numbered two",
            "- ☑ done\n- ☐ to do",
            "- more",
            "#quote(block: true)[\n  quoted\n]",
            "#block(width: 100%, fill: rgb(\"#fff6d6\"), stroke: (left: 2pt + rgb(\"#bf8700\")), inset: 8pt, radius: 3pt)[\n  careful\n]",
            "#raw(block: true, lang: \"rust\", \"let x = 1;\")",
            "#line(length: 100%)",
            "#pagebreak()",
            "#set document(title: \"Handbook\")",
        ] {
            assert!(
                out.source.contains(expected),
                "{expected:?} is not in\n{}",
                out.source
            );
        }
        assert!(
            !out.source.contains("#set page"),
            "the paper is left to whoever compiles it"
        );
    }

    #[test]
    fn marks_colours_links_mentions_and_chips_are_calls_kept_from_running_into_the_text_after_them()
    {
        let id = new_id();
        let para = make(
            BlockType::Paragraph,
            json!({}),
            BlockContent::Inline(vec![
                Inline::Text {
                    text: "all".into(),
                    marks: Some(vec![
                        Mark::Bold,
                        Mark::Italic,
                        Mark::Underline,
                        Mark::Strike,
                    ]),
                    href: None,
                    color: Some(9),
                    background: Some(2),
                },
                plain(".field and (more) and [brackets]"),
                Inline::Text {
                    text: "link".into(),
                    marks: None,
                    href: Some("https://example.org/a".into()),
                    color: None,
                    background: None,
                },
                Inline::Mention {
                    user_id: id,
                    user: None,
                },
                Inline::Ref {
                    display: None,
                    path: None,
                    kind: InlineRefKind::Node,
                    id,
                    part: None,
                    name: None,
                    label: None,
                },
                Inline::Math {
                    source: "x^2".into(),
                },
                Inline::Text {
                    text: "code".into(),
                    marks: Some(vec![Mark::Code]),
                    href: None,
                    color: None,
                    background: None,
                },
                plain("(after)"),
            ]),
            None,
            1,
        );
        let out = written(&[para], &nothing());
        for expected in [
            "#highlight(fill: rgb(\"#ffe8d1\"))[#text(fill: rgb(\"#cf222e\"))[#strike[#underline[#emph[#strong[all]]]]]]/**/.field and (more) and \\[brackets\\]",
            "#link(\"https://example.org/a\")[link]",
            "\\@ada",
            "#box(fill: luma(238), inset: (x: 2pt), radius: 2pt)[Plans]",
            "$x^2$",
            "#raw(\"code\")/**/(after)",
        ] {
            assert!(
                out.source.contains(expected),
                "{expected:?} is not in\n{}",
                out.source
            );
        }
    }

    #[test]
    fn a_table_keeps_its_header_spans_widths_and_colours_and_columns_stand_side_by_side() {
        let cell = |text: &str,
                    colspan: Option<u32>,
                    rowspan: Option<u32>,
                    background: Option<u32>| TableCell {
            content: vec![plain(text)],
            colspan,
            rowspan,
            color: None,
            background,
        };
        let table = make(
            BlockType::Table,
            json!({ "header_row": true, "header_column": true, "widths": [1.0, 3.0] }),
            BlockContent::Rows(vec![
                TableRow {
                    cells: vec![cell("A", None, None, None), cell("B", None, None, Some(5))],
                },
                TableRow {
                    cells: vec![
                        cell("tall", None, Some(2), None),
                        cell("x", None, None, None),
                    ],
                },
                TableRow {
                    cells: vec![cell("y", None, None, None)],
                },
            ]),
            None,
            1,
        );
        let columns = make(
            BlockType::Columns,
            json!({}),
            BlockContent::Inline(vec![]),
            None,
            2,
        );
        let left = make(
            BlockType::Column,
            json!({ "share": 2.0 }),
            BlockContent::Inline(vec![]),
            Some(columns.id),
            1,
        );
        let right = make(
            BlockType::Column,
            json!({}),
            BlockContent::Inline(vec![]),
            Some(columns.id),
            2,
        );
        let mut in_left = text_block(BlockType::Paragraph, json!({}), "left words", 1);
        in_left.parent_id = Some(left.id);
        let mut in_right = text_block(BlockType::Paragraph, json!({}), "right words", 1);
        in_right.parent_id = Some(right.id);
        let out = written(
            &[table, columns, left, right, in_left, in_right],
            &nothing(),
        );
        for expected in [
            "columns: (1fr, 3fr,),",
            "table.header([#strong[A]], table.cell(fill: rgb(\"#d5f3f3\"))[#strong[B]]),",
            "table.cell(rowspan: 2)[#strong[tall]], [x],",
            "[#strong[y]],",
            "#grid(columns: (2fr, 1fr,), column-gutter: 12pt,",
            "left words",
            "right words",
        ] {
            assert!(
                out.source.contains(expected),
                "{expected:?} is not in\n{}",
                out.source
            );
        }
    }

    #[test]
    fn an_image_the_source_uses_is_named_for_the_archive_and_a_film_or_a_file_is_named_where_it_stood()
     {
        let (png, bmp, mp4, zip) = (new_id(), new_id(), new_id(), new_id());
        let info = |name: &str, media_type: &str, size| FileInfo {
            name: name.to_owned(),
            media_type: media_type.to_owned(),
            size,
            is_text: false,
            bytes: None,
        };
        let world = World {
            files: HashMap::from([
                (png, info("pic one.png", "image/png", 10)),
                (bmp, info("old.bmp", "image/bmp", 10)),
                (mp4, info("clip.mp4", "video/mp4", 5_000_000)),
                (zip, info("data.zip", "application/zip", 900)),
            ]),
        };
        let image = make(
            BlockType::Image,
            json!({ "file_id": png, "width": 320.0 }),
            BlockContent::Inline(vec![plain("The picture")]),
            None,
            1,
        );
        let old = make(
            BlockType::Image,
            json!({ "file_id": bmp }),
            BlockContent::Inline(vec![]),
            None,
            2,
        );
        let film = make(
            BlockType::Video,
            json!({ "file_id": mp4 }),
            BlockContent::Inline(vec![plain("A clip")]),
            None,
            3,
        );
        let file = make(
            BlockType::File,
            json!({ "file_id": zip }),
            BlockContent::Inline(vec![]),
            None,
            4,
        );
        let url = make(
            BlockType::Audio,
            json!({ "url": "https://media.example.org/s.mp3" }),
            BlockContent::Inline(vec![]),
            None,
            5,
        );
        let all = [image, old, film, file, url];
        let alone = render("T", &all, &world, &Options::default());
        assert_eq!(
            alone.images,
            [ImageUse {
                path: "images/1-pic_one.png".into(),
                file_id: png
            }]
        );
        for expected in [
            "#figure(image(\"images/1-pic_one.png\", width: 320pt), caption: [The picture])",
            "An image that is not in this document.",
            "Film: A clip (clip.mp4, video/mp4, 4.8 MB; not in this document)",
            "File: data.zip (data.zip, application/zip, 900 bytes; not in this document)",
            "Sound: #link(\"https://media.example.org/s.mp3\")[Play from media.example.org]",
        ] {
            assert!(
                alone.source.contains(expected),
                "{expected:?} is not in\n{}",
                alone.source
            );
        }
        let beside = render(
            "T",
            &all,
            &world,
            &Options {
                attachments_beside: true,
                ..Options::default()
            },
        );
        assert!(
            beside
                .source
                .contains("#link(\"files/data.zip\")[data.zip]"),
            "{}",
            beside.source
        );
    }

    #[test]
    fn what_is_under_a_list_item_is_indented_under_it() {
        let parent = text_block(BlockType::Bullet, json!({}), "parent", 1);
        let mut child = text_block(BlockType::Numbered, json!({}), "child", 1);
        child.parent_id = Some(parent.id);
        let mut deeper = text_block(BlockType::Paragraph, json!({}), "a paragraph under it", 1);
        deeper.parent_id = Some(child.id);
        let out = written(&[parent, child, deeper], &nothing());
        assert!(
            out.source
                .contains("- parent\n  1. child\n     a paragraph under it"),
            "{}",
            out.source
        );
    }

    #[test]
    fn the_discussions_are_written_after_the_text() {
        let para = text_block(BlockType::Paragraph, json!({}), "The second paragraph", 1);
        let author = |name: &str| -> orch_api_contract::users::UserRef {
            serde_json::from_value(json!({
                "id": new_id(), "kind": "human", "username": name.to_lowercase(), "display_name": name,
                "avatar": { "text": "X", "color": 1 }
            }))
            .unwrap()
        };
        let discussion = DocumentDiscussion {
            id: new_id(),
            document_id: Id::nil(),
            block_id: Some(para.id),
            quote: "second".into(),
            anchored: true,
            resolved: None,
            created_by: author("Ada"),
            created_at: "2026-10-08T17:10:00.000Z".parse().unwrap(),
            comments: vec![orch_api_contract::documents::DocumentComment {
                id: new_id(),
                discussion_id: Id::nil(),
                author: author("Ada"),
                content: vec![plain("Why?")],
                reactions: vec![],
                edited_at: None,
                deleted: false,
                created_at: "2026-10-08T17:10:00.000Z".parse().unwrap(),
                seq_updated: 2,
            }],
            seq_updated: 2,
        };
        let without = written(std::slice::from_ref(&para), &nothing());
        assert!(!without.source.contains("Comments"));
        let with = render(
            "T",
            &[para],
            &nothing(),
            &Options {
                discussions: Some(std::slice::from_ref(&discussion)),
                ..Options::default()
            },
        );
        for expected in [
            "= Comments",
            "#text(weight: \"bold\")[On “The second paragraph”]",
            "#quote(block: true)[second]",
            "#strong[Ada] #text(fill: luma(100), size: 9pt)[2026-10-08 17:10 UTC] \\\n  Why?",
        ] {
            assert!(
                with.source.contains(expected),
                "{expected:?} is not in\n{}",
                with.source
            );
        }
    }
}
