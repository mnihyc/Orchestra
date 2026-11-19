//! The script language of the scripted model server: directives, the segments they make, and the errors of a malformed script.
//!
//! A script is text. A line that is empty, or begins with `#`, is nothing; every other line is a directive,
//! a word and what follows it. The first line of a script written into a user message is the marker `#!mock`.
//! The directives are listed in the documentation of the parent module. A script makes one response for each
//! round of tool calls: the directive `then` ends the response of one round, and what follows it is the
//! response to the request that carries the tool results.

use std::time::Duration;

/// The line that opens a script written into a message.
pub const MARKER: &str = "#!mock";

/// The directive that ends the response of one round of tool calls.
const THEN: &str = "then";

/// How many characters of text one piece of a stream holds unless a script says otherwise: about two
/// tokens, so that a short answer is a handful of events and a longer one is a good many.
pub const DEFAULT_PIECE_CHARS: usize = 8;

/// How many characters a token is taken to hold when the figures of use are not given. The usual rule of
/// thumb for English text, and for a double any steady figure would do.
pub const CHARS_PER_TOKEN: usize = 4;

/// The seconds a rate limit or an overload asks the client to wait, unless the script says otherwise: short,
/// so that a test that honours it stays quick.
pub const DEFAULT_RETRY_AFTER_SECS: u64 = 2;

/// What the reasoning effort of the vendor's stream calls an answer cut short by its token limit.
pub const DEFAULT_INCOMPLETE_REASON: &str = "max_output_tokens";

/// The word a script writes for a retry-after that is to be left out of the answer.
const NO_RETRY_AFTER: &str = "none";

/// How long `stall` without a number holds an answer back: an hour, which no client waits for.
pub const STALL_UNTIL_GIVEN_UP_SECS: u64 = 3600;

/// The plan a limit is on unless the script names one.
pub const DEFAULT_PLAN: &str = "pro";

/// The kinds of error the real interfaces give.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// Too many requests: a rate limit.
    RateLimit,
    /// The service or the model is overloaded.
    Overloaded,
    /// The request is invalid.
    InvalidRequest,
    /// The credential is missing or wrong.
    Authentication,
    /// The credential may not do this.
    Permission,
    /// The model or the path is not there.
    NotFound,
    /// The request is too large.
    TooLarge,
    /// The service failed.
    Server,
    /// The service timed out.
    Timeout,
    /// The account has no credit or quota left.
    Billing,
    /// The prompt is longer than the model's context.
    ContextLength,
    /// A limit of the plan of a subscription: its session, its week, a family of models, or its spending.
    PlanLimit,
}

impl ErrorKind {
    /// The word a script uses for it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::RateLimit => "rate_limit",
            Self::Overloaded => "overloaded",
            Self::InvalidRequest => "invalid_request",
            Self::Authentication => "authentication",
            Self::Permission => "permission",
            Self::NotFound => "not_found",
            Self::TooLarge => "too_large",
            Self::Server => "server",
            Self::Timeout => "timeout",
            Self::Billing => "billing",
            Self::ContextLength => "context_length",
            Self::PlanLimit => "plan_limit",
        }
    }

    /// Every kind, in the order the manual lists them.
    pub const ALL: [Self; 12] = [
        Self::RateLimit,
        Self::Overloaded,
        Self::InvalidRequest,
        Self::Authentication,
        Self::Permission,
        Self::NotFound,
        Self::TooLarge,
        Self::Server,
        Self::Timeout,
        Self::Billing,
        Self::ContextLength,
        Self::PlanLimit,
    ];

    fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.word() == word)
    }
}

/// Which window of a plan a limit is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LimitWindow {
    /// The window of a few hours.
    Session,
    /// The window of a week, for all models.
    Weekly,
    /// The weekly window of the largest family of models alone.
    WeeklyOpus,
    /// The weekly window of the middle family of models alone.
    WeeklySonnet,
    /// The spending limit of a month.
    Spend,
}

impl LimitWindow {
    /// The word a script uses for it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Weekly => "weekly",
            Self::WeeklyOpus => "weekly_opus",
            Self::WeeklySonnet => "weekly_sonnet",
            Self::Spend => "spend",
        }
    }

    /// Every window, in the order the manual lists them.
    pub const ALL: [Self; 5] = [
        Self::Session,
        Self::Weekly,
        Self::WeeklyOpus,
        Self::WeeklySonnet,
        Self::Spend,
    ];

    fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|window| window.word() == word)
    }
}

/// When a limit ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reset {
    /// At this moment, in seconds since the epoch.
    At(i64),
    /// This many seconds after the moment the request is answered, by the server's clock.
    In(i64),
}

/// What a script says of a limit of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitSpec {
    /// Which window of the plan.
    pub window: LimitWindow,
    /// When it ends, if the vendor gives a time.
    pub resets: Option<Reset>,
    /// The plan, as the vendor names it.
    pub plan: String,
}

impl Default for LimitSpec {
    fn default() -> Self {
        Self {
            window: LimitWindow::Session,
            resets: None,
            plan: DEFAULT_PLAN.to_owned(),
        }
    }
}

/// An error to give: its kind, and what a script may say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorSpec {
    /// The kind.
    pub kind: ErrorKind,
    /// The message, if the script gives one; each kind has its own otherwise.
    pub message: Option<String>,
    /// The seconds of `Retry-After` for a rate limit or an overload: `None` is the default, and `Some(None)`
    /// leaves the header out.
    pub retry_after: Option<Option<u64>>,
    /// The status, if the script gives one; each kind has its own in each format otherwise.
    pub status: Option<u16>,
    /// What a limit of a plan is, for the kind that is one.
    pub limit: LimitSpec,
}

impl ErrorSpec {
    /// An error of a kind, with everything else left to what the kind has.
    #[must_use]
    pub fn of(kind: ErrorKind) -> Self {
        Self {
            kind,
            message: None,
            retry_after: None,
            status: None,
            limit: LimitSpec::default(),
        }
    }
}

/// How fast the text of a step is sent: its pieces and the wait before each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    /// Characters in one piece.
    pub chunk: usize,
    /// The wait before each piece.
    pub delay: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Self {
            chunk: DEFAULT_PIECE_CHARS,
            delay: Duration::ZERO,
        }
    }
}

/// One thing a response does, in the order the script says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A message of the assistant.
    Text {
        /// What it says.
        text: String,
        /// How it is sent.
        pace: Pace,
    },
    /// The model's reasoning.
    Reasoning {
        /// What it says.
        text: String,
        /// How it is sent.
        pace: Pace,
    },
    /// A call of a function tool.
    Tool {
        /// The tool's name.
        name: String,
        /// The arguments, a JSON object as compact text.
        arguments: String,
        /// How the arguments are sent.
        pace: Pace,
    },
    /// A call of a tool that takes free text.
    Custom {
        /// The tool's name.
        name: String,
        /// The text it is called with.
        input: String,
        /// How the text is sent.
        pace: Pace,
    },
    /// Waits, once.
    Sleep(Duration),
    /// Sends a comment line, the way a server keeps a quiet stream alive.
    Keepalive,
    /// An error event inside the stream; the stream ends after it.
    StreamError(ErrorSpec),
    /// Aborts the connection; nothing more is sent.
    Cut,
    /// Ends the body cleanly, without the events that would close the stream.
    Truncate,
    /// An event of a name no client knows, with the data given, as it stands.
    Event {
        /// The name after `event:`.
        name: String,
        /// The text after `data:`.
        data: String,
    },
    /// A line of data that is no JSON, where the stream has events.
    Garbage(String),
}

/// How a response ends, when nothing cuts it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Finish {
    /// The model is done.
    #[default]
    Complete,
    /// The model stopped for a reason of its own, such as its token limit.
    Incomplete(String),
    /// The response failed.
    Fail(ErrorSpec),
}

/// The figures of use a script states; the rest are estimated from the text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageSpec {
    /// Tokens of input, those read from a cache among them.
    pub input: Option<i64>,
    /// Tokens of output, the reasoning among them.
    pub output: Option<i64>,
    /// Of the input, those read from a cache.
    pub cached: Option<i64>,
    /// Of the output, the reasoning.
    pub reasoning: Option<i64>,
    /// Of the input, those written to a cache.
    pub cache_write: Option<i64>,
    /// What the upstream states it charged, as a decimal.
    pub cost: Option<String>,
}

/// The response to one request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Segment {
    /// What it does, in order.
    pub steps: Vec<Step>,
    /// The figures of use.
    pub usage: UsageSpec,
    /// The model to report instead of the request's.
    pub model: Option<String>,
    /// The stop reason of the Messages format.
    pub stop: Option<String>,
    /// How it ends.
    pub finish: Finish,
    /// An error to answer with instead of anything else.
    pub error: Option<ErrorSpec>,
    /// Whether the stream is written the way a gateway in front of the vendors writes it.
    pub gateway: bool,
    /// Headers to add to the answer, whatever it is.
    pub headers: Vec<(String, String)>,
    /// How long to hold the answer back before a single byte of it, status line included, is sent.
    pub stall: Option<Duration>,
    /// A whole answer that is no document of either format: its status and its body, as they stand.
    pub raw_body: Option<RawBody>,
}

/// A whole answer written as it stands, which is what a gateway or a proxy in the way gives when it fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBody {
    /// The HTTP status.
    pub status: u16,
    /// The body.
    pub text: String,
}

impl Segment {
    /// The segment with `{{result}}` replaced by the text of the tool results the request carried.
    #[must_use]
    pub fn with_result(&self, result: &str) -> Self {
        let mut segment = self.clone();
        for step in &mut segment.steps {
            match step {
                Step::Text { text, .. } | Step::Reasoning { text, .. } => {
                    *text = text.replace("{{result}}", result);
                }
                Step::Custom { input, .. } => *input = input.replace("{{result}}", result),
                _ => {}
            }
        }
        segment
    }

    /// The plain answer a script gives once it has no segment left for a round.
    #[must_use]
    pub fn done() -> Self {
        Self {
            steps: vec![Step::Text {
                text: "Done.".to_owned(),
                pace: Pace::default(),
            }],
            ..Self::default()
        }
    }

    /// A segment that says `text`.
    #[must_use]
    pub fn saying(text: &str) -> Self {
        Self {
            steps: vec![Step::Text {
                text: text.to_owned(),
                pace: Pace::default(),
            }],
            ..Self::default()
        }
    }
}

/// A script: the responses to the requests of one conversation, one for each round of tool calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    /// The segments, in the order of the rounds.
    pub segments: Vec<Segment>,
}

/// What is wrong with a script.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct ScriptError {
    /// The line, counted from 1 in the text that was parsed.
    pub line: usize,
    /// What is wrong with it.
    pub message: String,
}

fn fault(line: usize, message: impl Into<String>) -> ScriptError {
    ScriptError {
        line,
        message: message.into(),
    }
}

/// `\n`, `\t` and `\\` in the text of a directive.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The text cut into pieces of `chunk` characters, never inside a character.
#[must_use]
pub fn pieces(text: &str, chunk: usize) -> Vec<String> {
    let chunk = chunk.max(1);
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(chunk)
        .map(|piece| piece.iter().collect())
        .collect()
}

/// A whole number of tokens or milliseconds.
fn number<T: std::str::FromStr>(line: usize, what: &str, text: &str) -> Result<T, ScriptError> {
    text.parse::<T>()
        .map_err(|_| fault(line, format!("{what} must be a whole number, not {text:?}")))
}

fn parse_usage(line: usize, rest: &str, stated: &mut UsageSpec) -> Result<(), ScriptError> {
    let mut positional = 0_usize;
    for token in rest.split_whitespace() {
        if let Some((key, value)) = token.split_once('=') {
            match key {
                "input" => stated.input = Some(number(line, "input", value)?),
                "output" => stated.output = Some(number(line, "output", value)?),
                "cached" => stated.cached = Some(number(line, "cached", value)?),
                "reasoning" => stated.reasoning = Some(number(line, "reasoning", value)?),
                "cache_write" => stated.cache_write = Some(number(line, "cache_write", value)?),
                "cost" => {
                    if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit() || c == '.') {
                        return Err(fault(
                            line,
                            format!("cost must be a decimal, not {value:?}"),
                        ));
                    }
                    stated.cost = Some(value.to_owned());
                }
                other => return Err(fault(line, format!("usage has no member {other:?}"))),
            }
        } else {
            match positional {
                0 => stated.input = Some(number(line, "input", token)?),
                1 => stated.output = Some(number(line, "output", token)?),
                _ => return Err(fault(line, format!("unexpected {token:?} in usage"))),
            }
            positional += 1;
        }
    }
    Ok(())
}

/// A moment: whole seconds since the epoch, or `+` and whole seconds from the moment of the answer.
fn parse_reset(line: usize, value: &str) -> Result<Reset, ScriptError> {
    match value.strip_prefix('+') {
        Some(seconds) => Ok(Reset::In(number(line, "resets", seconds)?)),
        None => Ok(Reset::At(number(line, "resets", value)?)),
    }
}

/// `<kind> [retry-after=<seconds|none>] [status=<code>] [window=<w>] [resets=<moment>] [plan=<name>] [message]`
fn parse_error(line: usize, rest: &str, with_options: bool) -> Result<ErrorSpec, ScriptError> {
    let mut words = rest.trim().splitn(2, char::is_whitespace);
    let word = words.next().unwrap_or_default();
    let kind = ErrorKind::from_word(word).ok_or_else(|| {
        let known: Vec<_> = ErrorKind::ALL.iter().map(|kind| kind.word()).collect();
        fault(
            line,
            format!(
                "unknown kind of error {word:?}; the kinds are {}",
                known.join(", ")
            ),
        )
    })?;
    let mut wanted = ErrorSpec {
        kind,
        message: None,
        retry_after: None,
        status: None,
        limit: LimitSpec::default(),
    };
    let mut tail = words.next().unwrap_or_default().trim();
    // The options come first and the message is what is left; `stream_error` and `fail` take no options.
    loop {
        if !with_options {
            break;
        }
        let (option, after) = tail
            .split_once(char::is_whitespace)
            .map_or((tail, ""), |(option, after)| (option, after.trim_start()));
        if let Some(value) = option.strip_prefix("retry-after=") {
            wanted.retry_after = Some(if value == NO_RETRY_AFTER {
                None
            } else {
                Some(number(line, "retry-after", value)?)
            });
        } else if let Some(value) = option.strip_prefix("status=") {
            wanted.status = Some(number(line, "status", value)?);
        } else if let Some(value) = option.strip_prefix("window=") {
            only_for_a_plan_limit(line, kind, "window")?;
            wanted.limit.window = LimitWindow::from_word(value).ok_or_else(|| {
                let known: Vec<_> = LimitWindow::ALL
                    .iter()
                    .map(|window| window.word())
                    .collect();
                fault(
                    line,
                    format!(
                        "unknown window {value:?}; the windows are {}",
                        known.join(", ")
                    ),
                )
            })?;
        } else if let Some(value) = option.strip_prefix("resets=") {
            only_for_a_plan_limit(line, kind, "resets")?;
            wanted.limit.resets = Some(parse_reset(line, value)?);
        } else if let Some(value) = option.strip_prefix("plan=") {
            only_for_a_plan_limit(line, kind, "plan")?;
            value.clone_into(&mut wanted.limit.plan);
        } else {
            break;
        }
        tail = after;
    }
    if !tail.is_empty() {
        wanted.message = Some(unescape(tail));
    }
    Ok(wanted)
}

/// Refuses an option of a limit on an error that is not one.
fn only_for_a_plan_limit(line: usize, kind: ErrorKind, option: &str) -> Result<(), ScriptError> {
    if kind == ErrorKind::PlanLimit {
        Ok(())
    } else {
        Err(fault(
            line,
            format!("{option} belongs to plan_limit, not to {}", kind.word()),
        ))
    }
}

/// The first `count` characters of a text.
fn first_characters(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

impl Script {
    /// Parses the text of a script. The marker line `#!mock` is allowed and not needed.
    pub fn parse(text: &str) -> Result<Self, ScriptError> {
        let mut segments = Vec::new();
        let mut segment = Segment::default();
        let mut pace = Pace::default();
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let directive = raw.trim();
            if directive.is_empty() || directive.starts_with('#') {
                continue;
            }
            let (word, rest) = directive
                .split_once(char::is_whitespace)
                .map_or((directive, ""), |(word, rest)| (word, rest.trim()));
            match word {
                "say" => segment.steps.push(Step::Text {
                    text: unescape(rest),
                    pace,
                }),
                "think" => segment.steps.push(Step::Reasoning {
                    text: unescape(rest),
                    pace,
                }),
                "tool" => {
                    let (name, json) = rest
                        .split_once(char::is_whitespace)
                        .map_or((rest, ""), |(name, json)| (name, json.trim()));
                    if name.is_empty() {
                        return Err(fault(line, "tool needs a name"));
                    }
                    let json = if json.is_empty() { "{}" } else { json };
                    let arguments: serde_json::Value =
                        serde_json::from_str(json).map_err(|error| {
                            fault(line, format!("the arguments are no JSON: {error}"))
                        })?;
                    if !arguments.is_object() {
                        return Err(fault(line, "the arguments of a tool are a JSON object"));
                    }
                    segment.steps.push(Step::Tool {
                        name: name.to_owned(),
                        arguments: arguments.to_string(),
                        pace,
                    });
                }
                "custom" => {
                    let (name, input) = rest
                        .split_once(char::is_whitespace)
                        .map_or((rest, ""), |(name, input)| (name, input.trim()));
                    if name.is_empty() {
                        return Err(fault(line, "custom needs a name"));
                    }
                    segment.steps.push(Step::Custom {
                        name: name.to_owned(),
                        input: unescape(input),
                        pace,
                    });
                }
                "usage" => parse_usage(line, rest, &mut segment.usage)?,
                "chunk" => {
                    let chunk: usize = number(line, "chunk", rest)?;
                    if chunk == 0 {
                        return Err(fault(line, "chunk must be at least 1"));
                    }
                    pace.chunk = chunk;
                }
                "delay" => pace.delay = Duration::from_millis(number(line, "delay", rest)?),
                "sleep" => segment.steps.push(Step::Sleep(Duration::from_millis(number(
                    line, "sleep", rest,
                )?))),
                "error" => segment.error = Some(parse_error(line, rest, true)?),
                "stream_error" => segment
                    .steps
                    .push(Step::StreamError(parse_error(line, rest, false)?)),
                "fail" => segment.finish = Finish::Fail(parse_error(line, rest, false)?),
                "incomplete" => {
                    segment.finish = Finish::Incomplete(if rest.is_empty() {
                        DEFAULT_INCOMPLETE_REASON.to_owned()
                    } else {
                        rest.to_owned()
                    });
                }
                "stop" => {
                    if rest.is_empty() {
                        return Err(fault(line, "stop needs a reason"));
                    }
                    segment.stop = Some(rest.to_owned());
                }
                "model" => {
                    if rest.is_empty() {
                        return Err(fault(line, "model needs a name"));
                    }
                    segment.model = Some(rest.to_owned());
                }
                "gateway" => segment.gateway = true,
                "keepalive" => segment.steps.push(Step::Keepalive),
                "cut" => segment.steps.push(Step::Cut),
                "truncate" => segment.steps.push(Step::Truncate),
                "cut_after" => {
                    let kept: usize = number(line, "cut_after", rest)?;
                    let shortened = segment.steps.iter_mut().rev().find_map(|step| match step {
                        Step::Text { text, .. } | Step::Reasoning { text, .. } => Some(text),
                        Step::Tool {
                            arguments: text, ..
                        }
                        | Step::Custom { input: text, .. } => Some(text),
                        _ => None,
                    });
                    let Some(shortened) = shortened else {
                        return Err(fault(
                            line,
                            "cut_after needs a say, think, tool or custom before it",
                        ));
                    };
                    *shortened = first_characters(shortened, kept);
                    segment.steps.push(Step::Cut);
                }
                "event" => {
                    let (name, data) = rest
                        .split_once(char::is_whitespace)
                        .map_or((rest, ""), |(name, data)| (name, data.trim()));
                    if name.is_empty() {
                        return Err(fault(line, "event needs a name"));
                    }
                    segment.steps.push(Step::Event {
                        name: name.to_owned(),
                        data: if data.is_empty() { "{}" } else { data }.to_owned(),
                    });
                }
                "garbage" => segment.steps.push(Step::Garbage(unescape(rest))),
                "header" => {
                    let (name, value) = rest
                        .split_once(char::is_whitespace)
                        .map_or((rest, ""), |(name, value)| (name, value.trim()));
                    if name.is_empty() {
                        return Err(fault(line, "header needs a name"));
                    }
                    segment.headers.push((name.to_owned(), value.to_owned()));
                }
                "stall" => {
                    segment.stall = Some(if rest.is_empty() {
                        Duration::from_secs(STALL_UNTIL_GIVEN_UP_SECS)
                    } else {
                        Duration::from_millis(number(line, "stall", rest)?)
                    });
                }
                "body" => {
                    let (status, body) = rest
                        .split_once(char::is_whitespace)
                        .map_or((rest, ""), |(status, body)| (status, body.trim()));
                    segment.raw_body = Some(RawBody {
                        status: number(line, "the status of body", status)?,
                        text: unescape(body),
                    });
                }
                THEN => {
                    segments.push(std::mem::take(&mut segment));
                    pace = Pace::default();
                }
                other => return Err(fault(line, format!("unknown directive {other:?}"))),
            }
        }
        segments.push(segment);
        Ok(Self { segments })
    }

    /// The script written in the text blocks of a message, if one is: the block holding a line that is the
    /// marker, from that line to its end.
    #[must_use]
    pub fn find(blocks: &[String]) -> Option<Result<Self, ScriptError>> {
        blocks.iter().find_map(|block| {
            let at = block.lines().position(|line| line.trim_end() == MARKER)?;
            let script: String = block.lines().skip(at).collect::<Vec<_>>().join("\n");
            Some(Self::parse(&script))
        })
    }

    /// The segment for the round `round` of tool calls, counted from 0: the plain answer once the script has
    /// no more.
    #[must_use]
    pub fn segment(&self, round: usize) -> Segment {
        self.segments
            .get(round)
            .cloned()
            .unwrap_or_else(Segment::done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Script {
        Script::parse(text).unwrap()
    }

    #[test]
    fn every_directive_makes_what_the_manual_says() {
        let script = parse(
            "#!mock\n\
             # a comment\n\
             \n\
             chunk 4\n\
             delay 25\n\
             think Let me look.\n\
             say Hello\\nworld\n\
             tool read_file {\"path\": \"a.txt\"}\n\
             custom apply_patch *** Begin Patch\n\
             sleep 100\n\
             keepalive\n\
             usage 120 30 cached=100 reasoning=10 cache_write=5 cost=0.0021\n\
             model mock-large\n\
             stop end_turn\n\
             incomplete\n\
             gateway\n",
        );
        assert_eq!(script.segments.len(), 1);
        let segment = &script.segments[0];
        let pace = Pace {
            chunk: 4,
            delay: Duration::from_millis(25),
        };
        assert_eq!(
            segment.steps,
            [
                Step::Reasoning {
                    text: "Let me look.".into(),
                    pace
                },
                Step::Text {
                    text: "Hello\nworld".into(),
                    pace
                },
                Step::Tool {
                    name: "read_file".into(),
                    arguments: "{\"path\":\"a.txt\"}".into(),
                    pace
                },
                Step::Custom {
                    name: "apply_patch".into(),
                    input: "*** Begin Patch".into(),
                    pace
                },
                Step::Sleep(Duration::from_millis(100)),
                Step::Keepalive,
            ]
        );
        assert_eq!(
            segment.usage,
            UsageSpec {
                input: Some(120),
                output: Some(30),
                cached: Some(100),
                reasoning: Some(10),
                cache_write: Some(5),
                cost: Some("0.0021".into()),
            }
        );
        assert_eq!(segment.model.as_deref(), Some("mock-large"));
        assert_eq!(segment.stop.as_deref(), Some("end_turn"));
        assert_eq!(
            segment.finish,
            Finish::Incomplete("max_output_tokens".into())
        );
        assert!(segment.gateway);
    }

    #[test]
    fn the_marker_is_allowed_and_not_needed() {
        assert_eq!(parse("say hi"), parse("#!mock\nsay hi"));
        assert_eq!(parse("").segments, [Segment::default()]);
    }

    #[test]
    fn then_ends_the_response_of_a_round_and_the_pace_starts_again() {
        let script = parse("chunk 2\nsay ab\nthen\nsay cd\nthen\nsay ef");
        assert_eq!(script.segments.len(), 3);
        let chunk = |segment: &Segment| match &segment.steps[0] {
            Step::Text { pace, .. } => pace.chunk,
            _ => 0,
        };
        assert_eq!(chunk(&script.segments[0]), 2);
        assert_eq!(chunk(&script.segments[1]), DEFAULT_PIECE_CHARS);
    }

    #[test]
    fn a_script_that_has_no_segment_left_for_a_round_gives_the_plain_answer() {
        let script = parse("tool t\nthen\nsay finished");
        assert!(matches!(script.segment(0).steps[0], Step::Tool { .. }));
        assert_eq!(script.segment(1), Segment::saying("finished"));
        assert_eq!(script.segment(2), Segment::done());
    }

    #[test]
    fn a_tool_without_arguments_is_called_with_an_empty_object() {
        let script = parse("tool list_files");
        assert!(matches!(
            &script.segments[0].steps[0],
            Step::Tool { arguments, .. } if arguments == "{}"
        ));
    }

    #[test]
    fn errors_carry_the_kind_the_options_and_the_message() {
        let script = parse("error rate_limit retry-after=7 status=429 Slow down, please");
        let error = script.segments[0].error.clone().unwrap();
        assert_eq!(error.kind, ErrorKind::RateLimit);
        assert_eq!(error.retry_after, Some(Some(7)));
        assert_eq!(error.status, Some(429));
        assert_eq!(error.message.as_deref(), Some("Slow down, please"));
        let bare = parse("error overloaded").segments[0].error.clone().unwrap();
        assert_eq!(
            (bare.retry_after, bare.status, bare.message),
            (None, None, None)
        );
        let none = parse("error overloaded retry-after=none").segments[0]
            .error
            .clone()
            .unwrap();
        assert_eq!(none.retry_after, Some(None));
    }

    #[test]
    fn an_error_inside_the_stream_and_a_failed_response_take_a_kind_and_a_message() {
        let script = parse("stream_error overloaded Busy now\nfail rate_limit");
        assert!(matches!(
            &script.segments[0].steps[0],
            Step::StreamError(ErrorSpec { kind: ErrorKind::Overloaded, message: Some(m), .. }) if m == "Busy now"
        ));
        assert!(matches!(
            &script.segments[0].finish,
            Finish::Fail(ErrorSpec {
                kind: ErrorKind::RateLimit,
                ..
            })
        ));
    }

    #[test]
    fn a_mistake_is_named_with_its_line() {
        for (text, line, part) in [
            ("say hi\nsya hello", 2, "unknown directive"),
            ("tool", 1, "needs a name"),
            ("tool t [1]", 1, "JSON object"),
            ("tool t {oops}", 1, "no JSON"),
            ("usage 1 2 3", 1, "unexpected"),
            ("usage cached=many", 1, "whole number"),
            ("usage color=red", 1, "no member"),
            ("usage 1 2 cost=1e5", 1, "decimal"),
            ("chunk 0", 1, "at least 1"),
            ("delay slow", 1, "whole number"),
            ("error weird", 1, "unknown kind"),
            ("\n\nerror rate_limit retry-after=soon", 3, "whole number"),
            ("stop", 1, "needs a reason"),
            ("model", 1, "needs a name"),
            ("event", 1, "needs a name"),
            ("header", 1, "needs a name"),
            ("cut_after 3", 1, "needs a say"),
            ("stall soon", 1, "whole number"),
        ] {
            let error = Script::parse(text).unwrap_err();
            assert_eq!(error.line, line, "{text:?}");
            assert!(error.message.contains(part), "{text:?}: {error}");
        }
    }

    #[test]
    fn a_script_is_found_in_the_block_that_holds_the_marker_and_only_from_the_marker_on() {
        let blocks = vec![
            "<system-reminder>be nice</system-reminder>".to_owned(),
            "Please do this:\n#!mock\nsay found\n".to_owned(),
            "say not part of it".to_owned(),
        ];
        let script = Script::find(&blocks).unwrap().unwrap();
        assert_eq!(script.segments, [Segment::saying("found")]);
        assert!(Script::find(&["say hi".to_owned()]).is_none());
        assert!(
            Script::find(&["#!mock".to_owned(), "#!mock\nbad".to_owned()])
                .unwrap()
                .unwrap()
                .segments
                == [Segment::default()],
            "the first block with a marker is the script"
        );
        assert!(Script::find(&["#!mock\nbad".to_owned()]).unwrap().is_err());
    }

    #[test]
    fn the_result_of_the_tools_fills_its_place_in_the_text_of_the_next_round() {
        let segment = parse("say It says: {{result}}\nthink got {{result}}")
            .segment(0)
            .with_result("hello");
        assert_eq!(
            segment.steps,
            [
                Step::Text {
                    text: "It says: hello".into(),
                    pace: Pace::default()
                },
                Step::Reasoning {
                    text: "got hello".into(),
                    pace: Pace::default()
                },
            ]
        );
    }

    #[test]
    fn text_is_cut_into_pieces_by_characters_and_never_inside_one() {
        assert_eq!(pieces("abcdefg", 3), ["abc", "def", "g"]);
        assert_eq!(pieces("größe", 2), ["gr", "öß", "e"]);
        assert!(pieces("", 3).is_empty());
        assert_eq!(
            pieces("abc", 0),
            ["a", "b", "c"],
            "a piece holds at least one character"
        );
    }

    #[test]
    fn a_limit_of_a_plan_says_which_window_until_when_and_on_which_plan() {
        let error = parse("error plan_limit window=weekly resets=1792000000 plan=team You are out")
            .segments[0]
            .error
            .clone()
            .unwrap();
        assert_eq!(error.kind, ErrorKind::PlanLimit);
        assert_eq!(error.limit.window, LimitWindow::Weekly);
        assert_eq!(error.limit.resets, Some(Reset::At(1_792_000_000)));
        assert_eq!(error.limit.plan, "team");
        assert_eq!(error.message.as_deref(), Some("You are out"));
        let relative = parse("error plan_limit resets=+3600").segments[0]
            .error
            .clone()
            .unwrap();
        assert_eq!(relative.limit.resets, Some(Reset::In(3600)));
        assert_eq!(relative.limit.window, LimitWindow::Session);
        assert_eq!(relative.limit.plan, DEFAULT_PLAN);
        let bare = parse("error plan_limit").segments[0].error.clone().unwrap();
        assert_eq!(bare.limit.resets, None, "a vendor may give no time");
        for window in LimitWindow::ALL {
            let script = format!("error plan_limit window={}", window.word());
            assert_eq!(
                parse(&script).segments[0]
                    .error
                    .clone()
                    .unwrap()
                    .limit
                    .window,
                window
            );
        }
    }

    #[test]
    fn what_belongs_to_a_limit_is_refused_on_any_other_error() {
        for text in [
            "error rate_limit window=weekly",
            "error server resets=5",
            "error overloaded plan=pro",
        ] {
            let error = Script::parse(text).unwrap_err();
            assert!(
                error.message.contains("belongs to plan_limit"),
                "{text}: {error}"
            );
        }
        assert!(Script::parse("error plan_limit window=yearly").is_err());
        assert!(Script::parse("error plan_limit resets=soon").is_err());
    }

    #[test]
    fn a_stream_can_be_cut_inside_a_sentence_and_inside_the_arguments_of_a_call() {
        let script = parse("chunk 4\nsay The answer is that the file\ncut_after 18");
        assert_eq!(
            script.segments[0].steps,
            [
                Step::Text {
                    text: "The answer is that".into(),
                    pace: Pace {
                        chunk: 4,
                        delay: Duration::ZERO
                    }
                },
                Step::Cut,
            ]
        );
        let script = parse("tool Bash {\"command\": \"ls -la /tmp\"}\ncut_after 14");
        assert!(matches!(
            &script.segments[0].steps[0],
            Step::Tool { arguments, .. } if arguments == "{\"command\":\"ls"
        ));
        assert!(
            Script::parse("cut_after 3")
                .unwrap_err()
                .message
                .contains("needs a say")
        );
    }

    #[test]
    fn an_event_nobody_knows_garbage_a_header_a_stall_and_a_body_that_is_no_document_are_directives()
     {
        let script = parse(
            "header x-gateway proxy-7\nevent mystery {\"n\": 1}\ngarbage this is {not json\nstall 250\nbody 502 <html>Bad Gateway</html>",
        );
        let segment = &script.segments[0];
        assert_eq!(
            segment.headers,
            [("x-gateway".to_owned(), "proxy-7".to_owned())]
        );
        assert_eq!(
            segment.steps,
            [
                Step::Event {
                    name: "mystery".into(),
                    data: "{\"n\": 1}".into()
                },
                Step::Garbage("this is {not json".into()),
            ]
        );
        assert_eq!(segment.stall, Some(Duration::from_millis(250)));
        assert_eq!(
            segment.raw_body,
            Some(RawBody {
                status: 502,
                text: "<html>Bad Gateway</html>".into()
            })
        );
        assert_eq!(
            parse("stall").segments[0].stall,
            Some(Duration::from_secs(STALL_UNTIL_GIVEN_UP_SECS)),
            "without a number it is held until the client gives up"
        );
        assert!(Script::parse("event").is_err());
        assert!(Script::parse("header").is_err());
        assert!(Script::parse("body teapot").is_err());
    }

    #[test]
    fn a_backslash_before_anything_but_n_t_and_a_backslash_stays() {
        let script = parse("say C:\\dir\\new \\q end\\");
        assert!(matches!(
            &script.segments[0].steps[0],
            Step::Text { text, .. } if text == "C:\\dir\new \\q end\\"
        ));
    }
}
