//! Registry keys, schemas, classes, scopes and shipped defaults.
//!
//! Everything that people decide per scope is a key of this registry: which
//! model, which mode, how long, how many, what is allowed. A [`SettingDef`]
//! says of one key what a value of it looks like, where it can be set, how
//! the values of several scopes combine, what holds while nobody set it, and
//! which screen group it is shown in. The registry is closed: [`all`] lists
//! every key, [`get`] finds one, and [`validate`] says whether a value fits.
//!
//! A key is a **parameter** or a **policy**. That is its [`Class`], and it
//! decides who may write it at a scope ([`SettingDef::written_by`]): a
//! parameter by whoever manages the thing it is set on, a policy by whoever
//! makes the rules there, and anything at the system by its admins.
//!
//! The settings screens are drawn from what is written here, so each key
//! carries its title, a sentence about it, and a schema of its value.

use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::NodeKind;
use orch_types::Json;
use orch_types::fields::FieldError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;

// Grant choices are whole days; reject an overflow before a server turns one into a date.
const QUICK_GRANT_DAY_MILLIS: i64 = 24 * 60 * 60 * 1_000;

/// Declares an enum that is written as a fixed word, with its list, its word
/// and its reading.
macro_rules! words {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident ($what:literal) {
            $( $(#[$vmeta:meta])* $variant:ident = $word:literal, )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        $vis enum $name {
            $( $(#[$vmeta])* #[serde(rename = $word)] $variant, )*
        }

        impl $name {
            /// Every one, in order.
            pub const ALL: &'static [$name] = &[ $( $name::$variant, )* ];

            /// The word it is written with.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( $name::$variant => $word, )* }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = orch_access_contract::roles::UnknownName;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $( $word => Ok($name::$variant), )*
                    _ => Err(orch_access_contract::roles::UnknownName { what: $what, name: text.to_owned() }),
                }
            }
        }
    };
}
pub(crate) use words;

words! {
    /// Where a key can be set. The scopes above a thing say what such things
    /// start from; the thing's own value is what applies to it.
    pub enum Scope ("scope") {
        /// The system: one for an installation.
        System = "system",
        /// An org, at any level; each org on a path is a scope of its own.
        Org = "org",
        /// A project.
        Project = "project",
        /// A topic.
        Topic = "topic",
        /// One thread.
        Thread = "thread",
        /// One environment: its own values, which come after what its home
        /// node resolves.
        Environment = "environment",
        /// One provider's runtime: its own values, which come after
        /// what its home resolves.
        Provider = "provider",
        /// One host's own maintenance policy, after the system's values.
        Host = "host",
    }
}

impl Scope {
    /// Whether a value at this scope is a stored row of a node or of the
    /// system, which can be locked. The own values of an environment and of
    /// a provider's runtime are the innermost and carry no lock.
    #[must_use]
    pub const fn has_rows(self) -> bool {
        !matches!(self, Self::Environment | Self::Provider | Self::Host)
    }
}

impl From<NodeKind> for Scope {
    /// The scope at which a node of this kind holds its rows.
    fn from(kind: NodeKind) -> Self {
        match kind {
            NodeKind::Org => Self::Org,
            NodeKind::Project => Self::Project,
            NodeKind::Topic => Self::Topic,
            NodeKind::Thread => Self::Thread,
        }
    }
}

words! {
    /// What kind of decision a key is, which says who may write it.
    pub enum Class ("class") {
        /// How one thing behaves: written by whoever manages that thing.
        Parameter = "parameter",
        /// A rule for a place: written by whoever makes the rules there.
        Policy = "policy",
    }
}

words! {
    /// How the values that several scopes set for one key combine, from the
    /// system downward.
    pub enum Merge ("merge mode") {
        /// The nearest scope that sets the key wins.
        Replace = "replace",
        /// The lists of all scopes together, in order from the top, each
        /// item once.
        Union = "union",
        /// What every scope that sets the key names; unset means no
        /// restriction.
        Intersect = "intersect",
        /// Objects merged member by member; a lower scope overrides the same
        /// member.
        Map = "map",
        /// Texts joined from the top scope down, separated by one blank line.
        Append = "append",
        /// The smallest number among the scopes that set the key; unset means
        /// no limit.
        Min = "min",
    }
}

impl Merge {
    /// Whether a lock on a row changes what the key resolves to. It does
    /// where a lower scope could otherwise override or add; where lower
    /// scopes can only narrow, a lock changes nothing.
    #[must_use]
    pub const fn honours_locks(self) -> bool {
        !matches!(self, Self::Intersect | Self::Min)
    }
}

words! {
    /// The group of a settings screen that a key is shown in.
    pub enum Group ("group") {
        /// Models, instructions, tools and notes of agents.
        Agent = "agent",
        /// Approval, sandbox, arrangements and capabilities.
        Confinement = "confinement",
        /// Which models may be used.
        Models = "models",
        /// What a new environment starts with, and where threads run.
        Environment = "environment",
        /// What a switch, a move or an update starts from.
        Switching = "switching",
        /// How a workspace is indexed and cached.
        Workspace = "workspace",
        /// How people and agents reach a thread, announcements, side
        /// threads and sharing.
        Collaboration = "collaboration",
        /// Chats and documents.
        ChatDocuments = "chat_documents",
        /// Registration, invitations and personal orgs.
        Joining = "joining",
        /// How people sign in.
        SigningIn = "signing_in",
        /// Automatic titles, descriptions and tags.
        Automation = "automation",
        /// Interactive shells.
        Shells = "shells",
        /// Browsers in an environment: whether, how they start, how many.
        Browsers = "browsers",
        /// Displays served over VNC in an environment.
        Displays = "displays",
        /// Owned port access, audiences and their limits.
        Ports = "ports",
        /// Rates, estimates and limits.
        Limits = "limits",
        /// What is archived and deleted by itself, and how long records are
        /// kept.
        Retention = "retention",
        /// What is masked in stored text.
        Redaction = "redaction",
        /// Provider runtimes, with their services declared by the hosting machine.
        ProviderRuntimes = "provider_runtimes",
        /// Providers and what members may bring.
        Providers = "providers",
        /// Requests for access.
        Access = "access",
        /// The system itself: hosts, storage, mail and its banner.
        System = "system",
    }
}

impl Group {
    /// Its heading.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::Confinement => "Confinement and isolation",
            Self::Models => "Models",
            Self::Environment => "Environment",
            Self::Switching => "Switching, moving and updating",
            Self::Workspace => "Workspace",
            Self::Collaboration => "Collaboration and broadcast",
            Self::ChatDocuments => "Chat and documents",
            Self::Joining => "Joining",
            Self::SigningIn => "Signing in",
            Self::Automation => "Automation",
            Self::Shells => "Shells",
            Self::Browsers => "Browsers",
            Self::Displays => "Displays",
            Self::Ports => "Ports",
            Self::Limits => "Limits and caps",
            Self::Retention => "Retention",
            Self::Redaction => "Redaction",
            Self::ProviderRuntimes => "Provider runtimes",
            Self::Providers => "Providers and resources",
            Self::Access => "Access",
            Self::System => "System",
        }
    }
}

words! {
    /// What a whole number counts.
    pub enum Unit ("unit") {
        /// Things: nothing more is said.
        Count = "count",
        /// Milliseconds.
        Milliseconds = "milliseconds",
        /// Seconds.
        Seconds = "seconds",
        /// Minutes.
        Minutes = "minutes",
        /// Hours.
        Hours = "hours",
        /// Days.
        Days = "days",
        /// Bytes.
        Bytes = "bytes",
        /// Hundredths.
        Percent = "percent",
    }
}

/// What a text has to look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// Any text.
    Any,
    /// A relative path that never goes up: no part of it is `..`, and it
    /// does not begin at a root.
    RelativePath,
    /// The name of a secret: a capital letter or an underscore, then at most
    /// 63 capitals, digits and underscores.
    SecretName,
    /// An address that begins with `https://`.
    HttpsAddress,
    /// A decimal number written as text, zero or more: digits, and after a
    /// point more digits.
    Decimal,
    /// A decimal number written as text that is greater than zero.
    PositiveDecimal,
    /// The size of a window or a screen, written `<width>x<height>` in whole
    /// numbers: from 320 to 3840 wide and from 240 to 2160 high.
    Geometry,
    /// One host's name in its canonical form: lower case, IDNA written in
    /// letters, digits and hyphens, without a final dot; never an address, a
    /// wildcard, a port, a path or a range.
    HostName,
    /// An HTTPS or SSH Git address without a password, token, query or fragment.
    RepositoryAddress,
    /// A branch name accepted by Git's branch-name rules.
    GitBranch,
}

/// What the names of the members of a map have to look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Names {
    /// Any name.
    Any,
    /// An identifier.
    Id,
}

/// One member of an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member {
    /// Its name.
    pub name: &'static str,
    /// What its value looks like.
    pub schema: Schema,
    /// Whether it has to be given.
    pub required: bool,
}

/// What a value of a key looks like: enough to say whether one fits, and to
/// draw the control that edits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schema {
    /// True or false.
    Bool,
    /// A whole number within bounds.
    Int {
        /// The least it may be.
        min: Option<i64>,
        /// The most it may be.
        max: Option<i64>,
        /// What it counts.
        unit: Unit,
    },
    /// A text.
    Text {
        /// The most characters it may have.
        max_chars: Option<u32>,
        /// What it has to look like.
        form: Form,
    },
    /// One of a closed list of words.
    Choice {
        /// The words.
        values: &'static [&'static str],
    },
    /// An identifier.
    Id,
    /// A moment.
    Time,
    /// A list.
    List {
        /// What each item looks like.
        items: &'static Schema,
        /// Whether an item may stand in it once only.
        unique: bool,
    },
    /// An object whose members are named freely and all look alike.
    Map {
        /// What the names look like.
        names: Names,
        /// What each value looks like.
        values: &'static Schema,
    },
    /// An object with members of its own.
    Object {
        /// Its members.
        members: &'static [Member],
        /// Whether it may have further members.
        open: bool,
    },
    /// A value that fits one of several schemas.
    OneOf {
        /// The schemas.
        any: &'static [Schema],
        /// What to say of a value that fits none, as the words after its
        /// place: "must be …".
        says: &'static str,
    },
    /// A value, or `null`.
    Nullable {
        /// What a value looks like.
        of: &'static Schema,
    },
    /// Any value.
    Any,
}

/// The pattern of a decimal number written as text.
const DECIMAL_PATTERN: &str = "^[0-9]+(\\.[0-9]+)?$";
/// The pattern of the name of a secret.
const SECRET_NAME_PATTERN: &str = "^[A-Z_][A-Z0-9_]{0,63}$";
/// The most characters the name of a secret has.
const SECRET_NAME_MAX_CHARS: usize = 64;
/// The pattern of a size written `<width>x<height>`: the bounds are checked
/// by [`parse_geometry`], which a pattern cannot say.
const GEOMETRY_PATTERN: &str = "^[0-9]{3,4}x[0-9]{3,4}$";
/// The widths a browser's window or a display may have, in pixels: from the
/// narrowest screen a page is laid out for to an ultra high definition one.
const GEOMETRY_WIDTHS: std::ops::RangeInclusive<u32> = 320..=3840;
/// The heights a browser's window or a display may have, in pixels.
const GEOMETRY_HEIGHTS: std::ops::RangeInclusive<u32> = 240..=2160;

fn is_decimal(text: &str) -> bool {
    let (whole, fraction) = text
        .split_once('.')
        .map_or((text, None), |(whole, fraction)| (whole, Some(fraction)));
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    digits(whole) && fraction.is_none_or(digits)
}

/// Whether `text` can be the name of a secret: a capital letter or an
/// underscore, then at most 63 capitals, digits and underscores. The same rule
/// holds for the names a list of secret names holds and for the secrets
/// themselves.
#[must_use]
pub fn is_secret_name(text: &str) -> bool {
    let mut bytes = text.bytes();
    let first = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_uppercase() || byte == b'_');
    first
        && text.len() <= SECRET_NAME_MAX_CHARS
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// The width and the height of a size written `<width>x<height>` in whole
/// numbers, where both lie within what a browser's window or a display may
/// have (320 to 3840 wide, 240 to 2160 high); nothing for any other text.
#[must_use]
pub fn parse_geometry(text: &str) -> Option<(u32, u32)> {
    let (width, height) = text.split_once('x')?;
    let number = |part: &str| {
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    let (width, height) = (number(width)?, number(height)?);
    (GEOMETRY_WIDTHS.contains(&width) && GEOMETRY_HEIGHTS.contains(&height))
        .then_some((width, height))
}

/// Whether `text` is a relative path that never goes up, whichever of the two
/// separators a machine uses: it begins at no root (a separator, or a drive
/// with a colon), and no part of it is `..`.
fn is_relative_path(text: &str) -> bool {
    let separator = |character: char| character == '/' || character == '\\';
    let drive = text.as_bytes().get(1) == Some(&b':');
    !text.starts_with(separator)
        && !drive
        && !text.contains('\0')
        && text.split(separator).all(|part| part != "..")
}

/// The longest host name, and the longest label of one.
const HOST_NAME_MAX: usize = 253;
const HOST_LABEL_MAX: usize = 63;

/// Whether `text` is one host's name in its canonical form: labels of lower-case
/// letters, digits, hyphens and underscores, none empty, none longer than a label
/// is, none beginning or ending with a hyphen; no final dot; and not an address,
/// which a name whose labels are all digits would be read as.
fn is_host_name(text: &str) -> bool {
    let labels_fit = text.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= HOST_LABEL_MAX
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    });
    let numeric = text
        .split('.')
        .all(|label| label.bytes().all(|b| b.is_ascii_digit()));
    !text.is_empty()
        && text.len() <= HOST_NAME_MAX
        && labels_fit
        && !numeric
        && matches!(url::Host::parse(text), Ok(url::Host::Domain(_)))
}

impl Form {
    fn fault(self, text: &str) -> Option<&'static str> {
        let fits = match self {
            Self::Any => true,
            Self::HostName => is_host_name(text),
            Self::RepositoryAddress => is_repository_address(text),
            Self::GitBranch => is_git_branch(text),
            Self::RelativePath => is_relative_path(text),
            Self::SecretName => is_secret_name(text),
            Self::HttpsAddress => text
                .strip_prefix("https://")
                .is_some_and(|rest| !rest.is_empty()),
            Self::Decimal => is_decimal(text),
            Self::PositiveDecimal => {
                is_decimal(text) && text.bytes().any(|byte| (b'1'..=b'9').contains(&byte))
            }
            Self::Geometry => parse_geometry(text).is_some(),
        };
        if fits {
            return None;
        }
        Some(match self {
            Self::Any => "must be text",
            Self::RepositoryAddress => {
                "must be an HTTPS or SSH repository address without embedded credentials, query or fragment"
            }
            Self::GitBranch => {
                "must be a Git branch name without parent sequences, lock suffixes or special reference characters"
            }
            Self::RelativePath => "must be a relative path without `..`",
            Self::SecretName => {
                "must be a name of capitals, digits and underscores that begins with no digit"
            }
            Self::HttpsAddress => "must be an address that begins with https://",
            Self::Decimal => "must be a decimal number written as text, zero or more",
            Self::PositiveDecimal => "must be a decimal number written as text, greater than zero",
            Self::Geometry => {
                "must be a size written width x height, from 320x240 to 3840x2160, such as 1280x800"
            }
            Self::HostName => {
                "must be one host's name in lower case without a final dot, such as pypi.org: no address, wildcard, port or path"
            }
        })
    }

    fn pattern(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::RepositoryAddress | Self::GitBranch => None,
            Self::RelativePath => Some("^(?![/\\\\])(?!.:)(?!(.*[/\\\\])?\\.\\.([/\\\\]|$)).*$"),
            Self::SecretName => Some(SECRET_NAME_PATTERN),
            Self::HttpsAddress => Some("^https://.+"),
            Self::Decimal | Self::PositiveDecimal => Some(DECIMAL_PATTERN),
            Self::Geometry => Some(GEOMETRY_PATTERN),
            Self::HostName => Some(HOST_NAME_PATTERN),
        }
    }
}

/// Whether an address can be frozen as a source without putting credentials in metadata.
#[must_use]
pub fn is_repository_address(text: &str) -> bool {
    if text.is_empty()
        || text.starts_with('-')
        || text
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return false;
    }
    let address = if text.contains("://") {
        text.to_owned()
    } else {
        let Some((authority, path)) = text.split_once(':') else {
            return false;
        };
        if authority.contains(['/', '\\']) || path.is_empty() {
            return false;
        }
        format!("ssh://{authority}/{path}")
    };
    url::Url::parse(&address).is_ok_and(|url| {
        matches!(url.scheme(), "https" | "ssh")
            && url.host_str().is_some()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && (url.scheme() == "ssh" || url.username().is_empty())
            && !url.path().trim_matches('/').is_empty()
    })
}

/// Whether a name can be a branch without being an option or a special reference.
#[must_use]
pub fn is_git_branch(text: &str) -> bool {
    !text.is_empty()
        && text != "@"
        && text != "HEAD"
        && !text.starts_with('-')
        && !text.ends_with('.')
        && !text.contains("..")
        && !text.contains("@{")
        && !text.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
        && text.split('/').all(|component| {
            !component.is_empty() && !component.starts_with('.') && !component.ends_with(".lock")
        })
}

/// What a form shows of a host name's rule; the check above is the whole of it.
const HOST_NAME_PATTERN: &str = "^[a-z0-9_]([a-z0-9_.-]*[a-z0-9_])?$";

fn join(path: &str, member: &str) -> String {
    if path.is_empty() {
        member.to_owned()
    } else {
        format!("{path}.{member}")
    }
}

fn fault(faults: &mut Vec<FieldError>, path: &str, message: impl Into<String>) {
    faults.push(FieldError {
        path: path.to_owned(),
        message: message.into(),
    });
}

impl Schema {
    /// Checks `value`, adding to `faults` everything that does not fit, each
    /// with its place under `path`.
    fn check(&self, value: &Json, path: &str, faults: &mut Vec<FieldError>) {
        match (self, value) {
            (Self::Any, _) | (Self::Bool, Json::Bool(_)) | (Self::Nullable { .. }, Json::Null) => {}
            (Self::Nullable { of }, value) => of.check(value, path, faults),
            (Self::Int { min, max, .. }, Json::Number(number)) => {
                match (number.as_i64(), *min, *max) {
                    (None, _, _) => fault(faults, path, "must be a whole number"),
                    (Some(number), Some(min), _) if number < min => {
                        fault(faults, path, format!("must be at least {min}"))
                    }
                    (Some(number), _, Some(max)) if number > max => {
                        fault(faults, path, format!("must be at most {max}"))
                    }
                    (Some(_), _, _) => {}
                }
            }
            (Self::Text { max_chars, form }, Json::String(text)) => {
                if let Some(message) = form.fault(text) {
                    fault(faults, path, message);
                }
                if let Some(most) = max_chars.filter(|most| text.chars().count() > *most as usize) {
                    fault(faults, path, format!("must be at most {most} characters"));
                }
            }
            (Self::Choice { values }, Json::String(text)) if values.contains(&text.as_str()) => {}
            (Self::Id, Json::String(text)) if text.parse::<orch_types::Id>().is_ok() => {}
            (Self::Time, Json::String(text)) if text.parse::<orch_types::Time>().is_ok() => {}
            (Self::List { items, unique }, Json::Array(list)) => {
                for (index, item) in list.iter().enumerate() {
                    let at = format!("{path}[{index}]");
                    items.check(item, &at, faults);
                    if *unique && list[..index].contains(item) {
                        fault(faults, &at, "is in the list twice");
                    }
                }
            }
            (Self::Map { names, values }, Json::Object(map)) => {
                for (name, value) in map {
                    let at = join(path, name);
                    if *names == Names::Id && name.parse::<orch_types::Id>().is_err() {
                        fault(faults, &at, "must be named by an identifier");
                    }
                    values.check(value, &at, faults);
                }
            }
            (Self::Object { members, open }, Json::Object(map)) => {
                for member in *members {
                    match map.get(member.name) {
                        Some(value) => member.schema.check(value, &join(path, member.name), faults),
                        None if member.required => {
                            fault(faults, &join(path, member.name), "is required")
                        }
                        None => {}
                    }
                }
                let known = |name: &str| members.iter().any(|member| member.name == name);
                for name in map.keys().filter(|name| !*open && !known(name)) {
                    fault(faults, &join(path, name), "is no member of this value");
                }
            }
            (Self::OneOf { any, says }, value) => {
                if !any
                    .iter()
                    .any(|schema| schema.faults_at(value, path).is_empty())
                {
                    fault(faults, path, *says);
                }
            }
            (schema, _) => fault(faults, path, schema.expects()),
        }
    }

    fn faults_at(&self, value: &Json, path: &str) -> Vec<FieldError> {
        let mut faults = Vec::new();
        self.check(value, path, &mut faults);
        faults
    }

    /// What to say of a value that is not of the kind the schema takes.
    fn expects(&self) -> String {
        match self {
            Self::Bool => "must be true or false".into(),
            Self::Int { .. } => "must be a whole number".into(),
            Self::Text { .. } => "must be text".into(),
            Self::Choice { values } => format!("must be one of {}", values.join(", ")),
            Self::Id => "must be an identifier".into(),
            Self::Time => "must be a moment, written as 2026-10-08T17:04:05.123Z".into(),
            Self::List { .. } => "must be a list".into(),
            Self::Map { .. } | Self::Object { .. } => "must be an object".into(),
            Self::OneOf { says, .. } => (*says).to_owned(),
            Self::Nullable { of } => of.expects(),
            Self::Any => "must be a value".into(),
        }
    }

    /// Everything about `value` that does not fit, each with its place in
    /// the value; nothing when it fits.
    #[must_use]
    pub fn faults(&self, value: &Json) -> Vec<FieldError> {
        self.faults_at(value, "")
    }

    /// The schema as JSON Schema, for whoever draws a control from it. What
    /// a whole number counts is written as the annotation `x-unit`.
    #[must_use]
    pub fn json_schema(&self) -> Json {
        match self {
            Self::Bool => json!({ "type": "boolean" }),
            Self::Int { min, max, unit } => {
                let mut schema = json!({ "type": "integer" });
                if let Some(min) = min {
                    schema["minimum"] = json!(min);
                }
                if let Some(max) = max {
                    schema["maximum"] = json!(max);
                }
                if *unit != Unit::Count {
                    schema["x-unit"] = json!(unit.as_str());
                }
                schema
            }
            Self::Text { max_chars, form } => {
                let mut schema = json!({ "type": "string" });
                if let Some(most) = max_chars {
                    schema["maxLength"] = json!(most);
                }
                if let Some(pattern) = form.pattern() {
                    schema["pattern"] = json!(pattern);
                }
                schema
            }
            Self::Choice { values } => json!({ "type": "string", "enum": values }),
            Self::Id => json!({ "type": "string", "format": "uuid" }),
            Self::Time => json!({ "type": "string", "format": "date-time" }),
            Self::List { items, unique } => {
                let mut schema = json!({ "type": "array", "items": items.json_schema() });
                if *unique {
                    schema["uniqueItems"] = json!(true);
                }
                schema
            }
            Self::Map { names, values } => {
                let mut schema =
                    json!({ "type": "object", "additionalProperties": values.json_schema() });
                if *names == Names::Id {
                    schema["propertyNames"] = json!({ "format": "uuid" });
                }
                schema
            }
            Self::Object { members, open } => {
                let properties: serde_json::Map<String, Json> = members
                    .iter()
                    .map(|member| (member.name.to_owned(), member.schema.json_schema()))
                    .collect();
                let required: Vec<&str> = members
                    .iter()
                    .filter(|member| member.required)
                    .map(|member| member.name)
                    .collect();
                json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": open })
            }
            Self::OneOf { any, .. } => {
                json!({ "anyOf": any.iter().map(Schema::json_schema).collect::<Vec<_>>() })
            }
            Self::Nullable { of } => json!({ "anyOf": [of.json_schema(), { "type": "null" }] }),
            Self::Any => json!({}),
        }
    }
}

/// What supplies the default of a key that has no fixed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Supplier {
    /// What the key is resolved for runs in: for a thread its environment,
    /// for an environment its template, or the machine it is on. Such a
    /// value sits between the registry and the system scope, and is reported
    /// as coming from the environment.
    Environment,
    /// The model the thread runs on.
    Model,
    /// What an installation is seeded with, or what its adapters bring: a
    /// template, the presets, the definitions of capabilities.
    Seeds,
    /// How the installation is set up, as whether it has a mail server.
    Installation,
}

/// What holds for a key while no scope sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefaultOf {
    /// This value, written as JSON.
    Value(&'static str),
    /// Nothing: the key is unset, and what that means is said with the key.
    Unset,
    /// What another key resolves to.
    SameAs(&'static str),
    /// Nothing fixed: it is supplied when the key is resolved.
    Supplied(Supplier),
}

/// One key of the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingDef {
    /// The key.
    pub key: &'static str,
    /// A few words that name it on a screen.
    pub title: &'static str,
    /// What it decides, in a sentence.
    pub description: &'static str,
    /// The group it is shown in.
    pub group: Group,
    /// What a value of it looks like.
    pub schema: Schema,
    /// Whether it is a parameter or a policy.
    pub class: Class,
    /// The scopes it can be set at, from the top.
    pub scopes: &'static [Scope],
    /// How the values of several scopes combine.
    pub merge: Merge,
    /// What holds while no scope sets it.
    pub default: DefaultOf,
    /// One authored value, written as JSON, for the setting's help entry. It is never applied as a default.
    pub example: &'static str,
}

/// Whom a write of a setting is asked of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Writer {
    /// A system admin.
    SystemAdmin,
    /// Whoever holds this permission on the node the value is written at.
    OnTheNode(Permission),
    /// Whoever manages the environment the thread runs in: the key is about
    /// where the thread runs, which is the environment's to decide. For a
    /// thread that is in no environment it is whoever manages the thread.
    OnTheThreadsEnvironment,
    /// Whoever manages the environment whose own value it is.
    OnTheEnvironment,
    /// Whoever manages the provider whose runtime's own value it is.
    OnTheProvider,
    /// Whoever manages the host whose maintenance policy it is.
    OnTheHost,
}

impl Writer {
    /// The permission it comes to, and nothing for a system admin.
    #[must_use]
    pub const fn permission(self) -> Option<Permission> {
        match self {
            Self::SystemAdmin => None,
            Self::OnTheNode(permission) => Some(permission),
            Self::OnTheThreadsEnvironment | Self::OnTheEnvironment => {
                Some(Permission::EnvironmentManage)
            }
            Self::OnTheProvider => Some(Permission::ProviderManage),
            Self::OnTheHost => Some(Permission::HostManage),
        }
    }
}

/// Who may set or clear the lock of a row at `scope`: whoever makes the
/// rules there, whatever the class of the key. Nothing for the own values of
/// an environment and of a provider's runtime, which carry no lock.
#[must_use]
pub const fn locked_by(scope: Scope) -> Option<Writer> {
    match scope {
        Scope::System => Some(Writer::SystemAdmin),
        Scope::Org | Scope::Project | Scope::Topic | Scope::Thread => {
            Some(Writer::OnTheNode(Permission::SettingsManage))
        }
        Scope::Environment | Scope::Provider | Scope::Host => None,
    }
}

impl SettingDef {
    /// Whether zero disables an activity rather than counting none of something or acting immediately.
    #[must_use]
    pub fn zero_disables(&self) -> bool {
        matches!(
            self.key,
            keys::AGENTS_HARNESS_CHANNEL_HOURS
                | keys::CALLS_REPEAT_LIMIT
                | keys::ENV_IDLE_STOP_MINUTES
                | keys::BROWSER_IDLE_MINUTES
                | keys::ENV_IDLE_WARNING_MINUTES
                | keys::ENV_UNUSED_REMOVE_DAYS
                | keys::RUNTIME_IDLE_STOP_MINUTES
                | keys::RUNTIME_RENEW_DAYS
                | keys::WORKSPACE_RECORD_DAYS
                | keys::USAGE_CATALOGUE_REFRESH_DAYS
                | keys::RETENTION_ARCHIVE_IDLE_DAYS
                | keys::RETENTION_AGENT_HISTORY_IDLE_DAYS
                | keys::RELATED_COUNT
                | keys::RETENTION_DELETE_ARCHIVED_DAYS
                | keys::RELEASE_CHECK_DAYS
                | keys::CLEANUP_RELEASE_KEEP_COUNT
                | keys::CLEANUP_UNUSED_KEEP_DAYS
                | keys::COMPOSER_PASTE_INLINE_CONTEXT_SHARE
                | keys::NOTIFICATIONS_PERSON_RATE_PER_HOUR
                | keys::NOTIFICATIONS_PERSON_NOTE_CHARS
                | keys::FILES_PREVIEW_CACHE_BYTES
        )
    }

    /// Whether the key can be set at `scope`.
    #[must_use]
    pub fn set_at(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }

    /// Whether the key is about where a thread runs: where its agent starts,
    /// and how far its commands are confined. At thread scope such a key is
    /// the environment's to decide.
    #[must_use]
    pub fn is_about_where_a_thread_runs(&self) -> bool {
        self.key == keys::THREAD_CWD || self.key == keys::SANDBOX_MODE
    }

    /// Who may write the key at `scope`; nothing where it cannot be set
    /// there. Setting or clearing a lock is asked of [`locked_by`] instead.
    #[must_use]
    pub fn written_by(&self, scope: Scope) -> Option<Writer> {
        if !self.set_at(scope) {
            return None;
        }
        if matches!(self.key, keys::ENV_AFTER_UP | keys::RUNTIME_AFTER_UP)
            && matches!(scope, Scope::Org | Scope::Project | Scope::Topic)
        {
            return Some(Writer::OnTheNode(Permission::SettingsManage));
        }
        Some(match (scope, self.class) {
            (Scope::System, _) => Writer::SystemAdmin,
            (Scope::Environment, _) => Writer::OnTheEnvironment,
            (Scope::Provider, _) => Writer::OnTheProvider,
            (Scope::Host, _) => Writer::OnTheHost,
            (_, Class::Policy) => Writer::OnTheNode(Permission::SettingsManage),
            (Scope::Org, Class::Parameter) => Writer::OnTheNode(Permission::OrgManage),
            (Scope::Project, Class::Parameter) => Writer::OnTheNode(Permission::ProjectManage),
            (Scope::Topic, Class::Parameter) => Writer::OnTheNode(Permission::TopicManage),
            (Scope::Thread, Class::Parameter) if self.is_about_where_a_thread_runs() => {
                Writer::OnTheThreadsEnvironment
            }
            (Scope::Thread, Class::Parameter) => Writer::OnTheNode(Permission::ThreadManage),
        })
    }

    /// Whether a row of the key at `scope` can usefully be locked: the row
    /// is one that carries a lock, the key can be set somewhere below it,
    /// and a lock changes what the key resolves to.
    #[must_use]
    pub fn lockable_at(&self, scope: Scope) -> bool {
        // Orgs lie inside orgs, so an org always has a scope below it.
        let below =
            self.scopes.iter().any(|other| rank(*other) > rank(scope)) || scope == Scope::Org;
        self.set_at(scope) && scope.has_rows() && below && self.merge.honours_locks()
    }

    /// Whether the key can be locked at any scope.
    #[must_use]
    pub fn lockable(&self) -> bool {
        self.scopes.iter().any(|scope| self.lockable_at(*scope))
    }

    /// Its fixed default; nothing for a key that is unset, takes another
    /// key's value, or is supplied when it is resolved.
    #[must_use]
    pub fn default_value(&self) -> Option<Json> {
        match self.default {
            DefaultOf::Value(text) => serde_json::from_str(text).ok(),
            DefaultOf::Unset | DefaultOf::SameAs(_) | DefaultOf::Supplied(_) => None,
        }
    }

    /// The authored help value; invalid shipped data is reported rather than replaced by an invented null.
    pub fn example_value(&self) -> Result<Json, serde_json::Error> {
        serde_json::from_str(self.example)
    }

    /// Everything about `value` that does not fit the key, each with its
    /// place in the value; nothing when it fits.
    #[must_use]
    pub fn faults(&self, value: &Json) -> Vec<FieldError> {
        let mut faults = self.schema.faults(value);
        if matches!(self.key, keys::RELATED_WEIGHTS | keys::RELATED_PLACE_SCORES) {
            for (name, value) in value.as_object().into_iter().flatten() {
                if value
                    .as_str()
                    .and_then(|text| text.parse::<f64>().ok())
                    .is_none_or(|number| !number.is_finite() || !(0.0..=1.0).contains(&number))
                {
                    fault(&mut faults, name, "must be a decimal from zero to one");
                }
            }
        }
        if self.key == keys::AGENTS_HARNESS_VERSIONS {
            for (kind, choice) in value.as_object().into_iter().flatten() {
                if choice.get("use").and_then(Json::as_str) == Some("exact")
                    && choice
                        .get("version")
                        .and_then(Json::as_str)
                        .is_none_or(|version| version.trim().is_empty())
                {
                    faults.push(FieldError {
                        path: format!("{kind}.version"),
                        message: "is required".to_owned(),
                    });
                }
            }
        }
        if self.key == keys::ACCESS_QUICK_GRANT_DURATIONS {
            for (index, duration) in value.as_array().into_iter().flatten().enumerate() {
                let valid = duration.as_str().is_some_and(|duration| {
                    duration == "always"
                        || duration
                            .strip_suffix('d')
                            .and_then(|days| days.parse::<i64>().ok())
                            .is_some_and(|days| {
                                days > 0 && days.checked_mul(QUICK_GRANT_DAY_MILLIS).is_some()
                            })
                });
                if !valid {
                    faults.push(FieldError {
                        path: format!("[{index}]"),
                        message: "Use a positive number of days followed by d, or always"
                            .to_owned(),
                    });
                }
            }
        }
        if faults.is_empty()
            && matches!(self.key, keys::ENV_AFTER_UP | keys::RUNTIME_AFTER_UP)
            && let Err(message) = crate::unit::setting_commands(value)
        {
            faults.push(FieldError {
                path: String::new(),
                message,
            });
        }
        if self.key == keys::RUNTIME_STORAGE
            && value.get("workspace").is_some()
            && let Some(reason) = crate::unit::runtime_unavailable("workspace")
        {
            faults.push(FieldError {
                path: "workspace".into(),
                message: reason.into(),
            });
        }
        faults
    }
}

/// How far down a scope lies.
const fn rank(scope: Scope) -> u8 {
    match scope {
        Scope::System => 0,
        Scope::Org => 1,
        Scope::Project => 2,
        Scope::Topic => 3,
        Scope::Thread | Scope::Environment | Scope::Provider | Scope::Host => 4,
    }
}

/// The key as the registry is served: its key, title, description, group,
/// the JSON Schema of its value, its class, scopes and merge mode, and its
/// fixed default or `null`.
#[derive(Serialize, JsonSchema)]
#[schemars(rename = "SettingDef")]
struct Served<'a> {
    key: &'a str,
    title: &'a str,
    description: &'a str,
    group: Group,
    schema: Json,
    class: Class,
    scopes: &'a [Scope],
    merge: Merge,
    default: Option<Json>,
    example: Json,
}

impl Serialize for SettingDef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Served {
            key: self.key,
            title: self.title,
            description: self.description,
            group: self.group,
            schema: self.schema.json_schema(),
            class: self.class,
            scopes: self.scopes,
            merge: self.merge,
            default: self.default_value(),
            example: self.example_value().map_err(serde::ser::Error::custom)?,
        }
        .serialize(serializer)
    }
}

impl JsonSchema for SettingDef {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        Served::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        Served::json_schema(generator)
    }
}

/// Why a value cannot be written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    /// There is no such key.
    #[error("{0:?} is no setting")]
    UnknownKey(String),
    /// The key is not set at that scope.
    #[error("{key} is not set at the scope {scope}")]
    WrongScope {
        /// The key.
        key: &'static str,
        /// The scope that was asked for.
        scope: Scope,
    },
    /// The value does not fit the key.
    #[error("the value does not fit {key}")]
    Value {
        /// The key.
        key: &'static str,
        /// Everything that does not fit, each with its place in the value.
        faults: Vec<FieldError>,
    },
}

/// Every key of the registry, in its order.
#[must_use]
pub fn all() -> &'static [SettingDef] {
    REGISTRY
}

/// The key `key`, where the registry has it.
#[must_use]
pub fn get(key: &str) -> Option<&'static SettingDef> {
    REGISTRY.iter().find(|def| def.key == key)
}

/// Canonicalize exact host names before validating and storing their list; invalid entries remain invalid.
#[must_use]
pub fn canonical_value(key: &str, value: &Json) -> Json {
    if key != keys::NETWORK_ALLOWED_HOSTS {
        return value.clone();
    }
    let Some(hosts) = value.as_array() else {
        return value.clone();
    };
    let mut normalized = Vec::new();
    for host in hosts {
        let canonical = host
            .as_str()
            .and_then(|name| {
                let name = name.strip_suffix('.').unwrap_or(name);
                if name.chars().any(|character| {
                    matches!(
                        character,
                        ':' | '/' | '?' | '#' | '@' | '[' | ']' | '\\' | '%' | '*'
                    ) || character.is_whitespace()
                }) {
                    return None;
                }
                match url::Host::parse(name) {
                    Ok(url::Host::Domain(name)) if is_host_name(&name) => Some(Json::String(name)),
                    _ => None,
                }
            })
            .unwrap_or_else(|| host.clone());
        if !normalized.contains(&canonical) {
            normalized.push(canonical);
        }
    }
    Json::Array(normalized)
}

/// Canonicalize own settings through the same rule used for scoped setting writes.
#[must_use]
pub fn canonical_own_values(values: &Json) -> Json {
    match values.as_object() {
        Some(values) => Json::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), canonical_value(key, value)))
                .collect(),
        ),
        None => values.clone(),
    }
}

/// Says whether `value` can be a value of `key`: the key exists, and the
/// value fits its schema. Validation happens when a value is written; one
/// that stops being usable later is kept, and whoever uses it reports that.
pub fn validate(key: &str, value: &Json) -> Result<&'static SettingDef, Invalid> {
    let def = get(key).ok_or_else(|| Invalid::UnknownKey(key.to_owned()))?;
    let mut faults = def.faults(value);
    if key == keys::CALLS_RULES
        && let Err(message) = orch_provider_contract::calls::validate(value)
    {
        faults.push(FieldError {
            path: String::new(),
            message,
        });
    }
    if faults.is_empty() {
        Ok(def)
    } else {
        Err(Invalid::Value {
            key: def.key,
            faults,
        })
    }
}

/// Says whether `value` can be written for `key` at `scope`: as
/// [`validate`], and the key can be set there.
pub fn validate_at(key: &str, scope: Scope, value: &Json) -> Result<&'static SettingDef, Invalid> {
    let def = get(key).ok_or_else(|| Invalid::UnknownKey(key.to_owned()))?;
    if !def.set_at(scope) {
        return Err(Invalid::WrongScope {
            key: def.key,
            scope,
        });
    }
    let def = validate(key, value)?;
    if def.key == keys::AGENT_MESSAGES {
        let faults = message_faults(scope, value);
        if !faults.is_empty() {
            return Err(Invalid::Value {
                key: def.key,
                faults,
            });
        }
    }
    Ok(def)
}

/// The placeholders a replacement of a message to agents keeps, by message: what Orch puts there is what the text
/// is for.
const MESSAGE_PLACEHOLDERS: &[(&str, &str)] = &[
    ("preamble", "{frames}"),
    ("turn_stopped", "[name]"),
    ("attribution_started", "[name]"),
    ("pending_notices", "<notices>"),
];

/// What the messages to agents may not be made to say. `frames`, the paragraphs on who is speaking and whose word
/// counts, is the system's alone, never empty, and keeps the two marks only Orch makes; a replaced `preamble` holds
/// the one place where that paragraph goes; a text with a placeholder keeps it, once for the preamble.
fn message_faults(scope: Scope, value: &Json) -> Vec<FieldError> {
    let mut faults = Vec::new();
    let Some(messages) = value.as_object() else {
        return faults;
    };
    for (message, text) in messages {
        let Some(text) = text.as_str() else {
            continue;
        };
        if message == "frames" {
            if scope != Scope::System {
                fault(&mut faults, message, "is set by the system alone");
            } else if text.trim().is_empty() {
                fault(&mut faults, message, "cannot be empty");
            } else if !(text.contains("<orch>") && text.contains("<relayed>")) {
                fault(
                    &mut faults,
                    message,
                    "must keep the marks <orch> and <relayed>",
                );
            }
            continue;
        }
        let Some((_, placeholder)) = MESSAGE_PLACEHOLDERS
            .iter()
            .find(|(name, _)| *name == message.as_str())
        else {
            continue;
        };
        let count = text.matches(placeholder).count();
        if count == 0 || (message == "preamble" && count > 1) {
            fault(
                &mut faults,
                message,
                format!("must hold {placeholder} once, where Orch puts what it stands for"),
            );
        }
    }
    faults
}

const SYSTEM: &[Scope] = &[Scope::System];
const SYSTEM_HOST: &[Scope] = &[Scope::System, Scope::Host];
const SYSTEM_ORG: &[Scope] = &[Scope::System, Scope::Org];
// Program builds are chosen for a project's environments, then for an individual environment.
const HARNESS_SCOPES: &[Scope] = &[
    Scope::System,
    Scope::Org,
    Scope::Project,
    Scope::Environment,
];
const SYSTEM_TO_PROJECT: &[Scope] = &[Scope::System, Scope::Org, Scope::Project];
const SYSTEM_TO_TOPIC: &[Scope] = &[Scope::System, Scope::Org, Scope::Project, Scope::Topic];
const SYSTEM_TO_THREAD: &[Scope] = &[
    Scope::System,
    Scope::Org,
    Scope::Project,
    Scope::Topic,
    Scope::Thread,
];
const SYSTEM_TO_TOPIC_ENV: &[Scope] = &[
    Scope::System,
    Scope::Org,
    Scope::Project,
    Scope::Topic,
    Scope::Environment,
];
const SYSTEM_TO_THREAD_ENV: &[Scope] = &[
    Scope::System,
    Scope::Org,
    Scope::Project,
    Scope::Topic,
    Scope::Thread,
    Scope::Environment,
];
// Operation defaults apply in both the hierarchy and a resource's own runtime context.
const SYSTEM_TO_THREAD_ENV_PROVIDER: &[Scope] = &[
    Scope::System,
    Scope::Org,
    Scope::Project,
    Scope::Topic,
    Scope::Thread,
    Scope::Environment,
    Scope::Provider,
];
const SYSTEM_TO_TOPIC_ENV_PROVIDER: &[Scope] = &[
    Scope::System,
    Scope::Org,
    Scope::Project,
    Scope::Topic,
    Scope::Environment,
    Scope::Provider,
];
const ORG_TO_TOPIC: &[Scope] = &[Scope::Org, Scope::Project, Scope::Topic];
// New icon colours are defaults of places that hold further places.
const ORG_TO_PROJECT: &[Scope] = &[Scope::Org, Scope::Project];
const ORG_TO_THREAD: &[Scope] = &[Scope::Org, Scope::Project, Scope::Topic, Scope::Thread];
const ORG_TO_TOPIC_ENV: &[Scope] = &[Scope::Org, Scope::Project, Scope::Topic, Scope::Environment];
// A unit inherits from the system through its home, then overrides on the resource itself.
const fn unit_scopes(resource: Scope) -> &'static [Scope] {
    match resource {
        Scope::Provider => &[
            Scope::System,
            Scope::Org,
            Scope::Project,
            Scope::Topic,
            Scope::Provider,
        ],
        _ => &[
            Scope::System,
            Scope::Org,
            Scope::Project,
            Scope::Topic,
            Scope::Environment,
        ],
    }
}
const UNIT_ENV_SCOPES: &[Scope] = unit_scopes(Scope::Environment);
const UNIT_RUNTIME_SCOPES: &[Scope] = unit_scopes(Scope::Provider);
const ENVIRONMENT: &[Scope] = &[Scope::Environment];
const THREAD: &[Scope] = &[Scope::Thread];

const UNSET: DefaultOf = DefaultOf::Unset;
const FROM_ENVIRONMENT: DefaultOf = DefaultOf::Supplied(Supplier::Environment);
const FROM_MODEL: DefaultOf = DefaultOf::Supplied(Supplier::Model);
const FROM_SEEDS: DefaultOf = DefaultOf::Supplied(Supplier::Seeds);
const FROM_INSTALLATION: DefaultOf = DefaultOf::Supplied(Supplier::Installation);
const TRUE: DefaultOf = DefaultOf::Value("true");
const FALSE: DefaultOf = DefaultOf::Value("false");
const EMPTY_MAP: DefaultOf = DefaultOf::Value("{}");
const EMPTY_LIST: DefaultOf = DefaultOf::Value("[]");
const NULL: DefaultOf = DefaultOf::Value("null");

const fn value(json: &'static str) -> DefaultOf {
    DefaultOf::Value(json)
}

const fn int(min: i64, unit: Unit) -> Schema {
    Schema::Int {
        min: Some(min),
        max: None,
        unit,
    }
}

const fn int_between(min: i64, max: i64, unit: Unit) -> Schema {
    Schema::Int {
        min: Some(min),
        max: Some(max),
        unit,
    }
}

const fn choice(values: &'static [&'static str]) -> Schema {
    Schema::Choice { values }
}

/// A list in which an item may stand more than once.
const fn list(items: &'static Schema) -> Schema {
    Schema::List {
        items,
        unique: false,
    }
}

/// A list in which each item stands once; its order is kept.
const fn set(items: &'static Schema) -> Schema {
    Schema::List {
        items,
        unique: true,
    }
}

const fn map(values: &'static Schema) -> Schema {
    Schema::Map {
        names: Names::Any,
        values,
    }
}

const fn by_id(values: &'static Schema) -> Schema {
    Schema::Map {
        names: Names::Id,
        values,
    }
}

const fn required(name: &'static str, schema: Schema) -> Member {
    Member {
        name,
        schema,
        required: true,
    }
}

const fn optional(name: &'static str, schema: Schema) -> Member {
    Member {
        name,
        schema,
        required: false,
    }
}

/// What a place's policy says of one runtime: environments may be made under it, may not, or must be.
const RUNTIME_RULE: Schema = choice(&["allow", "forbid", "require"]);
const BOOL: Schema = Schema::Bool;
const TEXT: Schema = Schema::Text {
    max_chars: None,
    form: Form::Any,
};
const ID: Schema = Schema::Id;
const ANY: Schema = Schema::Any;
const DECIMAL: Schema = Schema::Text {
    max_chars: None,
    form: Form::Decimal,
};
const POSITIVE_DECIMAL: Schema = Schema::Text {
    max_chars: None,
    form: Form::PositiveDecimal,
};
const RELATIVE_PATH: Schema = Schema::Text {
    max_chars: None,
    form: Form::RelativePath,
};
const SECRET_NAME: Schema = Schema::Text {
    max_chars: None,
    form: Form::SecretName,
};
const HOST_NAME: Schema = Schema::Text {
    max_chars: Some(HOST_NAME_MAX as u32),
    form: Form::HostName,
};
const HTTPS_ADDRESS: Schema = Schema::Text {
    max_chars: None,
    form: Form::HttpsAddress,
};
const REPOSITORY_ADDRESS: Schema = Schema::Text {
    max_chars: None,
    form: Form::RepositoryAddress,
};
const GIT_BRANCH: Schema = Schema::Text {
    max_chars: None,
    form: Form::GitBranch,
};
const GEOMETRY: Schema = Schema::Text {
    max_chars: None,
    form: Form::Geometry,
};

/// A size in bytes. No size is less than nothing.
const BYTES: Schema = int(0, Unit::Bytes);
/// A limit on what a unit is given. A limit of nothing would give nothing,
/// and to some drivers zero means no limit at all, so each is at least one.
const LIMIT: Schema = int(1, Unit::Count);
/// What an environment declares it needs: a statement, which may be nothing.
const NEED: Schema = int(0, Unit::Count);

/// The most characters of the standing instructions of one scope: about five
/// thousand words, which is more than anybody writes to an agent by hand and
/// still a small part of a model's context once every scope has added its own.
const INSTRUCTIONS: Schema = Schema::Text {
    max_chars: Some(20_000),
    form: Form::Any,
};

const APPROVAL_MODE: Schema = choice(&["ask", "on_request", "auto", "bypass"]);
const ARRANGEMENT: Schema = choice(&[
    "separated",
    "key_separated",
    "credential_shared",
    "handed_over",
    "together",
]);
const ORCH_TOOL: Schema = choice(&[
    "list_shared",
    "read_shared",
    "search_shared",
    "download_shared",
    "edit_document",
    "manage_shared",
    "send_message",
    "announce",
    "request_capability",
    "use_browser",
    "use_display",
    "list_ports",
    "open_port",
    "close_port",
    "list_port_connections",
]);
const RESOURCE_KIND: Schema = choice(&[
    "provider",
    "host",
    "template",
    "mcp_server",
    "mount",
    "skill",
]);
/// What a place gives its threads of one skill: nothing, its latest version, or one version by its
/// identifier.
const SKILL_CHOICE: Schema = Schema::OneOf {
    any: &[
        Schema::Choice {
            values: &["off", "latest"],
        },
        Schema::Id,
    ],
    says: "must be \"off\", \"latest\" or the identifier of one of the skill's versions",
};
const ANNOUNCEMENT_OPTION: Schema = choice(&["interrupt", "steer", "next_turn", "wake_idle"]);
const INVITATION_FORM: Schema = choice(&["link", "email"]);
const EMAIL_CODE_USE: Schema = choice(&["sign_in", "second_step"]);
const TRIGGER: Schema = choice(&["turn_completed", "milestone", "interval"]);
const UPDATE: Schema = choice(&["manual", "on_start"]);
const OFF_OR_SUMMARIZER: Schema = choice(&["off", "summarizer"]);

const MOUNT_MODE: Schema = Schema::OneOf {
    any: &[
        Schema::Bool,
        Schema::Choice {
            values: &["read_only", "private"],
        },
    ],
    says: "must be true, false, \"read_only\" or \"private\"",
};
const STORAGE: Schema = Schema::Object {
    members: &[
        optional("workspace", TEXT),
        optional("home", TEXT),
        optional("state", TEXT),
    ],
    open: false,
};
const DATA_SOURCE: Schema = Schema::OneOf {
    any: &[
        Schema::Object {
            members: &[required("kind", choice(&["made_by_orch"]))],
            open: false,
        },
        Schema::Object {
            members: &[
                required("kind", choice(&["existing_directory"])),
                required("path", TEXT),
            ],
            open: false,
        },
    ],
    says: "must select made_by_orch or an existing_directory with its absolute path",
};
const DATA_SOURCES: Schema = Schema::Object {
    members: &[
        optional("workspace", DATA_SOURCE),
        optional("home", DATA_SOURCE),
    ],
    open: false,
};
const AFTER_UP_COMMAND: Schema = Schema::Object {
    members: &[
        required("command", list(&TEXT)),
        optional(
            "timeout_secs",
            Schema::Int {
                min: Some(1),
                max: Some(3600),
                unit: Unit::Seconds,
            },
        ),
    ],
    open: false,
};
const AFTER_UP: Schema = Schema::Map {
    names: Names::Any,
    values: &Schema::Nullable {
        of: &AFTER_UP_COMMAND,
    },
};
const ENVIRONMENT_LIMITS: Schema = Schema::Object {
    members: &[
        optional("cpu_millis", LIMIT),
        optional("memory_mb", LIMIT),
        optional("memory_swap_mb", LIMIT),
        optional(
            "blkio_weight",
            Schema::Int {
                min: Some(10),
                max: Some(1000),
                unit: Unit::Count,
            },
        ),
        optional("pids", LIMIT),
        optional("disk_gb", LIMIT),
    ],
    open: false,
};
const ENVIRONMENT_NEEDS: Schema = Schema::Object {
    members: &[
        optional("memory_mb", NEED),
        optional("cpu_millis", NEED),
        optional("disk_gb", NEED),
    ],
    open: false,
};
const UPSTREAM: Schema = Schema::Object {
    members: &[
        required("protocol", TEXT),
        required("base_url", TEXT),
        optional("options", ANY),
    ],
    open: false,
};
const PRESET: Schema = Schema::Object {
    members: &[
        optional("note", TEXT),
        optional("docs_url", TEXT),
        required(
            "upstreams",
            Schema::List {
                items: &UPSTREAM,
                unique: false,
            },
        ),
        optional("template", ANY),
        optional("model", ANY),
        optional("dimensions", int(1, Unit::Count)),
    ],
    open: false,
};
/// A preset, or `null` to hide one that a scope above offers.
const PRESET_OR_HIDDEN: Schema = Schema::Nullable { of: &PRESET };
/// Which build of one agent program new units get: the one Orch ships, the newest whose check passed, the newest
/// (untested), or one exact version.
const HARNESS_VERSION: Schema = Schema::Object {
    members: &[
        required(
            "use",
            choice(&["shipped", "newest_checked", "newest", "exact"]),
        ),
        optional("version", TEXT),
    ],
    open: false,
};
const SUMMARIZER: Schema = Schema::Object {
    members: &[required("kind", TEXT)],
    open: true,
};
/// The API provider and model that make the vectors of search by meaning: the vectors' length where the model
/// can shorten them, and what is put before a query and before a document (`{title}` stands for the title of
/// what is indexed, or `none`).
const EMBEDDINGS: Schema = Schema::Object {
    members: &[
        required("provider_id", ID),
        required("model_name", TEXT),
        optional("dimensions", int(1, Unit::Count)),
        optional("query_prefix", TEXT),
        optional("document_prefix", TEXT),
    ],
    open: false,
};
/// The tabs of a document's Links section, in the order the section shows them.
const LINKS_TAB: Schema = choice(&["referenced_by", "references", "related", "graph"]);
// Fractions use the same decimal text fields as rates, so their precision is preserved by settings editors.
const RELATED_WEIGHTS: Schema = Schema::Object {
    members: &[
        required("meaning", DECIMAL),
        required("place", DECIMAL),
        required("references", DECIMAL),
    ],
    open: false,
};
const RELATED_PLACE_SCORES: Schema = Schema::Object {
    members: &[
        required("folder", DECIMAL),
        required("topic", DECIMAL),
        required("project", DECIMAL),
        required("org", DECIMAL),
    ],
    open: false,
};
/// Native scope routing and per-tier candidate overrides for word search.
const WORD_TUNING: Schema = Schema::Object {
    members: &[
        required("scope_candidates", int(1, Unit::Count)),
        optional("request_windows", int(1, Unit::Count)),
        required(
            "tiers",
            Schema::Object {
                members: &[
                    optional("title", int(1, Unit::Count)),
                    optional("description", int(1, Unit::Count)),
                    optional("heading", int(1, Unit::Count)),
                    optional("body", int(1, Unit::Count)),
                ],
                open: false,
            },
        ),
    ],
    open: false,
};
/// How search by meaning cuts, sends, finds and joins; each member is a default an admin may change.
const SEMANTIC_TUNING: Schema = Schema::Object {
    members: &[
        // The texts of one vector, in estimated tokens.
        required("chunk_tokens", int(16, Unit::Count)),
        // The texts of one request to the endpoint, in estimated tokens.
        required("batch_tokens", int(16, Unit::Count)),
        // How many results by meaning are joined with those by words.
        required("candidates", int(1, Unit::Count)),
        // How many candidates the index looks at for each result kept.
        required("oversample", int(1, Unit::Count)),
        // The constant of reciprocal-rank fusion.
        required("fusion_k", int(1, Unit::Count)),
        // The least likeness, in thousandths, at which another endpoint serves the same model.
        required("same_model_cosine_milli", int_between(0, 1000, Unit::Count)),
        // How often the indexer looks for work, in seconds.
        required("interval_secs", int(1, Unit::Seconds)),
        optional("exact_filter_candidates", int(0, Unit::Count)),
        optional("source_batch", int(1, Unit::Count)),
        optional("max_scan_tuples", int(1, Unit::Count)),
        optional("scan_mem_multiplier", int(1, Unit::Count)),
        optional("ef_search", int(1, Unit::Count)),
        optional(
            "iterative_scan",
            choice(&["strict_order", "relaxed_order", "off"]),
        ),
        // How long a search waits for its query's vector before it answers by words alone, in seconds.
        required("query_wait_secs", int(1, Unit::Seconds)),
    ],
    open: false,
};
const TRIGGERS: Schema = Schema::Object {
    members: &[
        required(
            "on",
            Schema::List {
                items: &TRIGGER,
                unique: true,
            },
        ),
        required("min_interval_secs", int(0, Unit::Seconds)),
        // An interval of nothing would never let a turn go on.
        optional("interval_secs", int(1, Unit::Seconds)),
    ],
    open: false,
};
const CAPABILITY_DECISION: Schema = Schema::Object {
    members: &[
        required("decision", choice(&["allow", "ask", "deny"])),
        required(
            "approver_scope",
            choice(&["environment", "host", "org", "system"]),
        ),
    ],
    open: false,
};
const RATE_ABOVE: Schema = Schema::Object {
    members: &[
        required("context_tokens", int(0, Unit::Count)),
        required("input_per_mtok", DECIMAL),
        optional("cached_input_per_mtok", DECIMAL),
        required("output_per_mtok", DECIMAL),
        optional("cache_write_per_mtok", DECIMAL),
    ],
    open: false,
};
const RATE: Schema = Schema::Object {
    members: &[
        required("input_per_mtok", DECIMAL),
        required("cached_input_per_mtok", DECIMAL),
        required("output_per_mtok", DECIMAL),
        optional("cache_write_per_mtok", DECIMAL),
        optional(
            "tiers",
            Schema::Map {
                names: Names::Any,
                values: &DECIMAL,
            },
        ),
        optional(
            "above",
            Schema::List {
                items: &RATE_ABOVE,
                unique: false,
            },
        ),
    ],
    open: false,
};
const REDACTION_PATTERN: Schema = Schema::Object {
    members: &[required("name", TEXT), required("regex", TEXT)],
    open: false,
};
const BANNER: Schema = Schema::Object {
    members: &[
        required("text", TEXT),
        required("level", choice(&["info", "warning"])),
        required("until", Schema::Nullable { of: &Schema::Time }),
    ],
    open: false,
};
const BANNER_OR_NONE: Schema = Schema::Nullable { of: &BANNER };
const HTTPS_ADDRESS_OR_NONE: Schema = Schema::Nullable { of: &HTTPS_ADDRESS };
/// The places of one parent, in the order somebody put them.
const PLACES_IN_ORDER: Schema = set(&ID);
/// The order of an org's tree: how it is ordered, and for an order of one's own, the places of each parent as
/// they were put.
const TREE_ORDER: Schema = Schema::Object {
    members: &[
        required("by", choice(&["custom", "newest", "changed", "name"])),
        optional("custom", by_id(&PLACES_IN_ORDER)),
    ],
    open: false,
};

macro_rules! registry {
    ( $( $name:ident = $key:literal, $group:ident, $class:ident, $scopes:ident, $merge:ident, $default:expr, $schema:expr,
         $title:literal, $about:literal; )* ) => {
        /// The keys of the registry, by name, for code that reads one.
        pub mod keys {
            $( #[doc = $about] pub const $name: &str = $key; )*
        }

        static REGISTRY: &[SettingDef] = &[
            $( SettingDef {
                key: $key,
                title: $title,
                description: $about,
                group: Group::$group,
                schema: $schema,
                class: Class::$class,
                scopes: $scopes,
                merge: Merge::$merge,
                default: $default,
                example: crate::examples::$name,
            }, )*
        ];
    };
}

registry! {
    CALLS_RULES = "calls.rules", Automation, Policy, SYSTEM_TO_TOPIC, Replace, value(orch_provider_contract::calls::DEFAULT_RULES), Schema::List { items: &Schema::Any, unique: false },
        "Call recovery", "Ordered rules for retrying, continuing, failing over or waiting after a model call fails.";
    CALLS_REPEAT_LIMIT = "calls.repeat_limit", Automation, Policy, SYSTEM_TO_TOPIC, Replace, value("3"), int(0, Unit::Count),
        "Repeat limit", "Automatic recoveries of one failure kind allowed within the repeat window; zero stops automatic recovery.";
    CALLS_REPEAT_WINDOW = "calls.repeat_window", Automation, Policy, SYSTEM_TO_TOPIC, Replace, value("300"), int(1, Unit::Seconds),
        "Repeat window", "The rolling window in seconds used to stop repeated model-call recovery.";
    CALLS_NOTIFY_ON_STOP = "calls.notify_on_stop", Automation, Policy, SYSTEM_TO_TOPIC, Replace, FALSE, BOOL,
        "Notify on stop", "Whether a repeated failure sends the thread owner one Inbox item when automatic recovery stops.";
    CALLS_RETURN = "calls.return", Automation, Policy, SYSTEM_TO_TOPIC, Replace, value("\"next_turn\""), choice(&["next_turn", "stay"]),
        "After failover", "Return to the thread's own choice at the next turn when it is ready, or stay on the candidate.";
    AGENT_MODEL = "agent.model", Agent, Parameter, ORG_TO_THREAD, Replace, UNSET, TEXT,
        "Model", "The model that threads here run on, by its routing name.";
    AGENT_EFFORT = "agent.effort", Agent, Parameter, ORG_TO_THREAD, Replace, FROM_MODEL, TEXT,
        "Effort", "The reasoning effort asked of the model, one of those the model lists.";
    AGENT_PARAMS = "agent.params", Agent, Parameter, ORG_TO_THREAD, Map, EMPTY_MAP, map(&ANY),
        "Agent parameters", "Values for the parameters that the agent's adapter declares, by parameter.";
    AGENT_FEATURES = "agent.features", Agent, Policy, SYSTEM_TO_TOPIC, Map, EMPTY_MAP, map(&BOOL),
        "Agent features", "Which optional features of agents are switched on, by feature.";
    AGENT_INSTRUCTIONS = "agent.instructions", Agent, Parameter, ORG_TO_THREAD, Append, value("\"\""), INSTRUCTIONS,
        "Instructions", "Standing instructions given to the agent with every session; each scope adds its own under those above it.";
    AGENT_MEMORY = "agent.memory", Agent, Parameter, ORG_TO_THREAD, Replace, value("\"on\""), choice(&["on", "harness", "off"]),
        "Agent notes", "Whether an agent keeps notes for itself from one session to the next: by whoever can, by its harness alone, or not at all.";
    AGENTS_HARNESS_VERSIONS = "agents.harness_versions", Agent, Policy, HARNESS_SCOPES, Map, EMPTY_MAP, map(&HARNESS_VERSION),
        "Agent program versions", "Which build of each agent program new and updated environments get: the one Orch ships, the newest whose check passed, the newest (untested), or an exact version. An agent profile that names its version keeps it.";
    AGENTS_HARNESS_CHANNEL_HOURS = "agents.harness_channel_hours", Agent, Parameter, SYSTEM, Replace, value("24"), Schema::Int { min: Some(0), max: None, unit: Unit::Hours },
        "Release check interval", "How often to read release channels when a place requests newer builds; zero keeps scheduled reads off.";
    AGENTS_HARNESS_INTERFACE_TIMEOUT_SECS = "agents.harness_interface_timeout_secs", Agent, Parameter, SYSTEM, Replace, value("120"), Schema::Int { min: Some(1), max: None, unit: Unit::Seconds },
        "Interface check timeout", "How long a build may take to print its interface.";
    AGENTS_HARNESS_CASE_TIMEOUT_SECS = "agents.harness_case_timeout_secs", Agent, Parameter, SYSTEM, Replace, value("90"), Schema::Int { min: Some(1), max: None, unit: Unit::Seconds },
        "Conformance case timeout", "How long each native harness case may take against the scripted model.";
    // Seven native cases at the default case deadline, plus one case's budget for unit startup and evidence.
    AGENTS_HARNESS_CONFORMANCE_TIMEOUT_SECS = "agents.harness_conformance_timeout_secs", Agent, Parameter, SYSTEM, Replace, value("720"), Schema::Int { min: Some(1), max: None, unit: Unit::Seconds },
        "Conformance check timeout", "How long the complete native harness check may take in its isolated unit.";
    AGENTS_HARNESS_TRUST_MIRROR = "agents.harness_trust_mirror", Agent, Parameter, SYSTEM, Replace, FALSE, BOOL,
        "Trust mirror checksums", "Allow the release mirror to publish executable checksums of its own. These builds are labelled as mirror checksums, not vendor checksums.";
    AGENTS_HARNESS_MIRROR = "agents.harness_mirror", Agent, Parameter, SYSTEM, Replace, NULL, Schema::Nullable { of: &TEXT },
        "Release mirror", "A URL that serves the vendors' release files where this server cannot reach them; empty reads the vendors directly.";
    AGENTS_SUBAGENTS_AT_ONCE = "agents.subagents_at_once", Agent, Parameter, SYSTEM_TO_THREAD, Min, value("8"), int_between(1, 64, Unit::Count),
        "Sub-agents at once", "How many sub-agents the harness may run at once; the shortest value along the hierarchy wins. A harness without this control says it does not apply.";
    AGENTS_SUBAGENT_DEPTH = "agents.subagent_depth", Agent, Parameter, SYSTEM_TO_THREAD, Min, value("1"), int_between(1, 5, Unit::Count),
        "Sub-agent depth", "How many layers of sub-agents the harness may spawn; 1 allows no nesting. The shortest value along the hierarchy wins; a harness without this control says it does not apply.";
    AGENTS_SKILLS = "agents.skills", Agent, Parameter, ORG_TO_TOPIC, Map, EMPTY_MAP, by_id(&SKILL_CHOICE),
        "Skills", "Which skills the agents of threads here are given, by skill: off, its latest version, or a pinned version; a lower scope may change or switch off an inherited one.";
    AGENTS_PERSONAL_SKILLS = "agents.personal_skills", Agent, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Personal skills", "Whether the agent of a thread is also given the own skills of the person who started it.";
    THREAD_CWD = "thread.cwd", Environment, Parameter, THREAD, Replace, value("\"\""), RELATIVE_PATH,
        "Start directory", "Where the agent starts inside the thread's directory, as a relative path.";
    THREAD_SIDE_THREADS = "thread.side_threads", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Side threads", "Whether side threads may be started from the threads here.";
    THREAD_SIDE_MAX = "thread.side_max", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("4"), int(1, Unit::Count),
        "Side threads at once", "How many side threads of one thread may be open at once.";
    THREAD_SIDE_IDLE_HOURS = "thread.side_idle_hours", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("24"), int(1, Unit::Hours),
        "Side thread idle time", "After how many hours without a turn an open side thread is closed.";
    THREAD_INPUTS_WITHOUT_USE = "thread.inputs_without_use", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("\"held\""), choice(&["held", "refused", "allowed"]),
        "Writers without use", "What happens to a message for the agent from somebody who may write here but may not use the thread's model or the environment it runs in: it waits for somebody who may run it on their own access, it is refused before it is sent, or it runs on the thread owner's access.";
    THREAD_SWITCH_HISTORY = "thread.switch_history", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("\"carry\""), choice(&["carry", "summary", "none"]),
        "History on a switch", "What a switch or a fork carries of the conversation unless its plan says otherwise: as natively as the new agent program allows, with the recent turns in its context where it imports nothing; a summary with the transcript as a file; or nothing.";
    THREAD_SWITCH_CARRY_SHARE = "thread.switch_carry_share", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("80"), int_between(10, 95, Unit::Percent),
        "Carried context", "How much of the new model's context window a switch may fill with the conversation it carries, keeping the rest for work; older turns stay in the transcript file.";
    THREAD_NATIVE_FORK_CHECK_BYTES = "thread.native_fork_check_bytes", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("33554432"), int(1, Unit::Bytes),
        "Native fork check size", "The most a conversation may take, as the database keeps its words, for a fork at one of its turns to be checked against the agent program's own record of it so that the program can make the fork itself; a longer conversation is carried as the newest turns that fit, with the whole of it as a file. The program keeps a record of 32 MiB at most, so a larger value only starts checks that cannot finish.";
    THREAD_READ_TURN_BYTES = "thread.read_turn_bytes", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("33554432"), int(1, Unit::Bytes),
        "Turn read size", "The most the messages of one turn may take, written out in full, for Orch to read that turn into a summary of the conversation or a goal check; a larger turn is named and left out, and nothing of it is read.";
    THREAD_PLAN_FIRST = "thread.plan_first", Collaboration, Parameter, SYSTEM_TO_TOPIC, Replace, FALSE, BOOL,
        "Plan first", "Whether the composer's Plan switch starts on for a thread's first message: the agent proposes a plan and changes nothing until somebody proceeds. The approval mode stays as it is.";
    THREAD_PLAN_ANSWERED_BY = "thread.plan_answered_by", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("\"writers\""), choice(&["writers", "managers"]),
        "Who answers plans", "Who may proceed with a plan an agent proposes, or keep it planning: whoever may write to the agent here, or only those who manage the thread.";
    THREAD_GOAL_MAX_TURNS = "thread.goal.max_turns", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("0"), int(0, Unit::Count),
        "Goal turns", "The most turns a goal may use here; 0 is no limit. A goal may set its own limit unless a place locks this one. Reaching a limit stops the goal and says which.";
    THREAD_GOAL_MAX_MINUTES = "thread.goal.max_minutes", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("0"), int(0, Unit::Minutes),
        "Goal minutes", "The most minutes a goal may be active here; 0 is no limit. It is looked at between turns: a turn that runs is not cut short.";
    THREAD_GOAL_MAX_CREDITS = "thread.goal.max_credits", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("\"0\""), DECIMAL,
        "Goal credits", "The most credits a goal may use here, written as a decimal number; 0 is no limit. Usage caps hold as always.";
    APPROVAL_MODE = "approval.mode", Confinement, Parameter, ORG_TO_THREAD, Replace, FROM_ENVIRONMENT, APPROVAL_MODE,
        "Approval mode", "When an agent has to ask before it acts: always, when it asks by itself, or never.";
    APPROVAL_ALLOWED_MODES = "approval.allowed_modes", Confinement, Policy, SYSTEM_TO_TOPIC, Intersect, value("[\"ask\",\"on_request\",\"auto\",\"bypass\"]"), set(&APPROVAL_MODE),
        "Allowed approval modes", "The approval modes that may be chosen here.";
    SANDBOX_MODE = "sandbox.mode", Confinement, Parameter, ORG_TO_THREAD, Replace, FROM_ENVIRONMENT, choice(&["read_only", "workspace", "open"]),
        "Sandbox", "What the harness's own sandbox lets an agent's commands do: read, write in the workspace, or anything.";
    AGENT_MESSAGES = "agent.messages", Agent, Parameter, SYSTEM_TO_TOPIC, Map, EMPTY_MAP, map(&TEXT),
        "Messages to agents", "Texts that replace what Orch itself says to agents, by message.";
    MODELS_ALLOWED = "models.allowed", Models, Policy, SYSTEM_TO_TOPIC, Intersect, UNSET, list(&TEXT),
        "Allowed models", "Patterns naming the models that may be used here at all; a model has to be named by every scope that sets a list.";
    PROVIDERS_ALLOWED = "providers.allowed", Models, Policy, SYSTEM_TO_TOPIC, Intersect, UNSET, list(&TEXT),
        "Allowed providers", "Patterns over providers' Orch addresses; every scope that sets a list must admit the provider used for work here.";
    COLLAB_ATTRIBUTION = "collab.attribution", Collaboration, Parameter, ORG_TO_THREAD, Replace, value("\"auto\""), choice(&["auto", "always", "never"]),
        "Attribution", "Whether Orch's composed input, stop, denial and forwarded text names people: auto once a second person's words are delivered, always, or never (a person/an approver, with source, time and notes kept). Authored chats and documents retain their writers.";
    COLLAB_DEFAULT_DELIVERY = "collab.default_delivery", Collaboration, Parameter, ORG_TO_THREAD, Replace, value("\"queue\""), choice(&["queue", "steer"]),
        "Default delivery", "What an input does when a turn is running, unless its sender says otherwise: wait for the next turn, or steer the running one.";
    COMPOSER_PASTE_INLINE_CONTEXT_SHARE = "composer.paste_inline_context_share", Collaboration, Parameter, SYSTEM_TO_THREAD, Replace, value("25"), int_between(0, 100, Unit::Percent),
        "Pasted text in the message", "The default share of remaining context for pasted text. Zero always sends it as a file; 100 always sends it in the message. The sender can choose for each paste. Token counts are estimated where the model reports none.";
    ENV_THREAD_DEFAULT = "env.thread_default", Environment, Parameter, SYSTEM_TO_TOPIC, Replace, value("\"shared\""), choice(&["shared", "fresh"]),
        "New threads", "Whether a new thread joins the resolved default environment or is given one of its own.";
    ENV_DEFAULT_ID = "env.default_id", Environment, Parameter, SYSTEM_TO_TOPIC, Replace, UNSET, ID,
        "Default environment", "The environment new threads join when they share one and ask for no other. The creator must manage it. Clear the choice to inherit the place above.";
    ENV_UPDATE = "env.update", Environment, Parameter, UNIT_ENV_SCOPES, Replace, value("\"manual\""), UPDATE,
        "Updates", "Whether an environment that is behind waits for somebody to update it, or is updated whenever it starts.";
    ENV_TEMPLATE_ID = "env.template_id", Environment, Parameter, UNIT_ENV_SCOPES, Replace, FROM_SEEDS, ID,
        "Template", "The template that a managed environment is made from.";
    ENV_HOST_ID = "env.host_id", Environment, Parameter, UNIT_ENV_SCOPES, Replace, UNSET, ID,
        "Host", "The host that a managed environment is made on; unset, the one with the most room.";
    ENV_RUNTIME = "env.runtime", Environment, Parameter, UNIT_ENV_SCOPES, Replace, UNSET, TEXT,
        "Runtime", "The container runtime an environment is made under, among those its host offers; unset, its template's own.";
    ENVIRONMENTS_RUNTIMES = "environments.runtimes", Environment, Policy, SYSTEM_TO_TOPIC, Map, EMPTY_MAP, map(&RUNTIME_RULE),
        "Runtimes", "Which runtimes environments here may be made under, by runtime name: allow, forbid or require; the name * stands for every runtime not named.";
    ENV_STORAGE = "env.storage", Environment, Parameter, UNIT_ENV_SCOPES, Map, EMPTY_MAP, STORAGE,
        "Storage", "Named machine storages for new workspace, home and connector state data; existing data keeps its recorded location.";
    ENV_DATA_SOURCES = "env.data_sources", Environment, Parameter, UNIT_ENV_SCOPES, Replace, EMPTY_MAP, DATA_SOURCES,
        "Workspace and home", "Where each data root lives: made by Orch or an existing directory allowed by this machine's environment.roots. Existing directories keep their owner and mode, have no volume size limit and are never removed by a purge.";
    ENV_AFTER_UP = "env.after_up", Environment, Parameter, UNIT_ENV_SCOPES, Map, EMPTY_MAP, AFTER_UP,
        "Commands after start", "Named argv commands run after each start or attach, where the machine permits environment.after_up; null removes an inherited name and a failure keeps the environment unavailable.";
    ENV_MOUNTS = "env.mounts", Environment, Parameter, UNIT_ENV_SCOPES, Map, EMPTY_MAP, by_id(&MOUNT_MODE),
        "Mounts", "The mounts attached beside the workspace, each as the mount says, off, read-only, or read-only with the environment's own changes kept apart.";
    ENV_RESOURCES = "env.resources", Environment, Parameter, UNIT_ENV_SCOPES, Map, EMPTY_MAP, ENVIRONMENT_LIMITS,
        "Limits", "The limits of an environment's unit in place of its template's: processor, memory, processes and disk.";
    ENV_NEEDS = "env.needs", Environment, Parameter, UNIT_ENV_SCOPES, Map, EMPTY_MAP, ENVIRONMENT_NEEDS,
        "Needs", "What an environment declares it needs at least, for choosing where it runs: memory, processor and disk.";
    ENV_IDLE_STOP_MINUTES = "env.idle_stop_minutes", Environment, Parameter, UNIT_ENV_SCOPES, Replace, value("1440"), int(0, Unit::Minutes),
        "Idle stop", "Stop a managed environment after this many minutes without activity; 0 never stops it.";
    ENV_VARIABLES = "env.variables", Environment, Parameter, UNIT_ENV_SCOPES, Map, EMPTY_MAP, map(&TEXT),
        "Variables", "Variables given to the environment, by name.";
    ENV_SECRET_NAMES = "env.secret_names", Environment, Parameter, UNIT_ENV_SCOPES, Union, EMPTY_LIST, set(&SECRET_NAME),
        "Secrets", "The names of the secrets that an environment is given.";
    NETWORK_ALLOWED_HOSTS = "network.allowed_hosts", Confinement, Policy, SYSTEM_TO_TOPIC_ENV_PROVIDER, Intersect, UNSET, set(&HOST_NAME),
        "Allowed hosts", "Exact other host names a restricted unit may reach; lists intersect and an empty list allows only Orch's own transport.";
    MCP_ENABLED = "mcp.enabled", Environment, Parameter, ORG_TO_TOPIC_ENV, Map, EMPTY_MAP, by_id(&BOOL),
        "MCP servers", "Which MCP servers are switched on, by server; a lower scope may switch an inherited one off.";
    EXEC_OFFERS = "exec.offers", Environment, Parameter, ENVIRONMENT, Replace, FROM_ENVIRONMENT, list(&TEXT),
        "Exec offers", "What an environment's exec node offers a harness that runs with its provider, in the order in which it is tried.";
    EXEC_ORCH_TOOLS = "exec.orch_tools", Environment, Parameter, ENVIRONMENT, Replace, FROM_ENVIRONMENT, BOOL,
        "Orch's own tools", "Whether the agents that work in this environment are offered Orch's own tools.";
    ORCH_TOOLS_ENABLED = "orch_tools.enabled", Agent, Parameter, ORG_TO_THREAD, Replace,
        value("[\"list_shared\",\"read_shared\",\"search_shared\",\"download_shared\",\"edit_document\",\"manage_shared\",\"send_message\",\"announce\",\"request_capability\",\"use_browser\",\"use_display\",\"list_ports\",\"open_port\",\"close_port\",\"list_port_connections\"]"), set(&ORCH_TOOL),
        "Orch tools", "Which of Orch's own tools an agent is offered.";
    ORCH_TOOLS_CALLS_PER_MINUTE = "orch_tools.calls_per_minute", Agent, Policy, SYSTEM_TO_TOPIC, Replace, value("120"), int(1, Unit::Count),
        "Tool calls a minute", "How many calls of Orch's tools one thread's agent may make in a minute.";
    SHARING_AGENT_REACH = "sharing.agent_reach", Agent, Policy, SYSTEM_TO_TOPIC, Replace, value("\"none\""), choice(&["none", "topic", "project"]),
        "Agents' reach", "Which other threads the agents here may read without anybody sharing them: none, those of their topic, or those of their project.";
    SHARING_AGENT_DOCUMENTS = "sharing.agent_documents", Agent, Policy, SYSTEM_TO_TOPIC, Replace, value("\"write\""), choice(&["none", "read", "write"]),
        "Topic documents", "Whether agents may read or also edit and create public documents beside their topic and its main threads; private documents, conversations, chats, management and Export keep their own rules.";
    BROADCAST_CHAT_MESSAGES_PER_HOUR = "broadcast.chat_messages_per_hour", Collaboration, Policy, SYSTEM_TO_THREAD, Replace, value("60"), int(0, Unit::Count),
        "Agent chat messages per hour", "Successful posts across all chats from one origin in a rolling hour; zero forbids posting.";
    SHARING_SNAPSHOT_MAX_BYTES = "sharing.snapshot_max_bytes", Collaboration, Policy, SYSTEM_TO_PROJECT, Replace, value("67108864"), BYTES,
        "Largest copy", "The largest copy that may be shared with an agent in one request, in bytes.";
    EXPORT_MAX_BYTES = "export.max_bytes", Collaboration, Policy, SYSTEM_TO_PROJECT, Replace, value("2147483648"), BYTES,
        "Largest archive", "The largest archive one export of what lies here may come to, in bytes; one that would pass it is not begun, and says so with its size.";
    EXPORT_MAX_ITEMS = "export.max_items", Collaboration, Policy, SYSTEM_TO_PROJECT, Replace, value("10000"), int(1, Unit::Count),
        "Most things in an archive", "How many conversations, chats and documents one export of what lies here may hold.";
    EXPORT_KEEP_HOURS = "export.keep_hours", Collaboration, Policy, SYSTEM, Replace, value("24"), int(1, Unit::Hours),
        "Archives are kept", "For how many hours a finished archive is kept for its person to fetch before it is removed.";
    IMPORT_MAX_BYTES = "import.max_bytes", Collaboration, Policy, SYSTEM_TO_PROJECT, Replace, NULL, Schema::Nullable { of: &BYTES },
        "Largest import", "The most bytes of source one import into what lies here may hold once expanded, in bytes; none follows the largest archive an export may come to, so that whatever exports can be imported. One that would pass it is not begun, and says so with its size.";
    IMPORT_MAX_ITEMS = "import.max_items", Collaboration, Policy, SYSTEM_TO_PROJECT, Replace, NULL, Schema::Nullable { of: &int(1, Unit::Count) },
        "Most things in an import", "How many places, document folders, documents, chats and threads one import into what lies here may hold; none follows the most things an export may hold.";
    WORKSPACE_IGNORE = "workspace.ignore", Workspace, Parameter, ORG_TO_TOPIC_ENV, Union, EMPTY_LIST, list(&TEXT),
        "Ignored paths", "Patterns, written as in a gitignore file, for what the index of a workspace leaves out.";
    WORKSPACE_INDEX = "workspace.index", Workspace, Parameter, ORG_TO_TOPIC_ENV, Replace, value("\"full\""), choice(&["full", "live"]),
        "Index", "Whether the whole tree of a workspace is indexed for use while it is offline, or only listed while its environment is online.";
    SOURCE_REPOSITORY_URL = "source.repository_url", Workspace, Parameter, ORG_TO_TOPIC, Replace, NULL, Schema::Nullable { of: &REPOSITORY_ADDRESS },
        "Repository", "The repository prepared only when the thread's directory is missing; an existing directory keeps its own files.";
    SOURCE_BRANCH = "source.branch", Workspace, Parameter, ORG_TO_TOPIC, Replace, NULL, Schema::Nullable { of: &GIT_BRANCH },
        "Source branch", "The branch to prepare; unset, use the repository's advertised default branch.";
    SOURCE_SECRET_NAME = "source.secret_name", Workspace, Parameter, ORG_TO_TOPIC, Replace, NULL, Schema::Nullable { of: &SECRET_NAME },
        "Source secret", "The name of an existing HTTPS Git reading secret; its value is sealed only to the preparation command.";
    WORKSPACE_RECORD_DAYS = "workspace.record_days", Workspace, Parameter, ORG_TO_TOPIC_ENV, Replace, value("7"), int(0, Unit::Days),
        "Keep turn files", "Days to keep complete turn and restore undo files in the environment; zero disables recording.";
    WORKSPACE_RECORD_MAX_BYTES = "workspace.record_max_bytes", Workspace, Parameter, ORG_TO_TOPIC_ENV, Replace, value("1073741824"), int(1, Unit::Bytes),
        "Largest recorded directory", "The included file and link bytes eligible for whole-state recording; larger directories use per-file undo.";
    BROADCAST_DELIVERY = "broadcast.delivery", Collaboration, Parameter, ORG_TO_TOPIC, Replace, value("\"steer\""), choice(&["interrupt", "steer", "next_turn"]),
        "Announcement delivery", "How an announcement reaches a thread whose turn is running: it interrupts, steers, or waits for the next turn.";
    BROADCAST_FALLBACK = "broadcast.fallback", Collaboration, Parameter, ORG_TO_TOPIC, Replace, value("\"next_turn\""), choice(&["next_turn", "interrupt"]),
        "When steering is impossible", "What is done instead when an announcement is to steer an agent that cannot be steered.";
    BROADCAST_WAKE_IDLE = "broadcast.wake_idle", Collaboration, Parameter, ORG_TO_TOPIC, Replace, FALSE, BOOL,
        "Wake idle threads", "Whether an announcement starts a turn on a thread that is idle.";
    BROADCAST_AGENT_OPTIONS = "broadcast.agent_options", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, EMPTY_LIST, set(&ANNOUNCEMENT_OPTION),
        "Agents' choices", "What an agent may choose for one announcement in place of the values set here.";
    TERMINAL_ENABLED = "terminal.enabled", Shells, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Shells", "Whether interactive shells may be opened.";
    TERMINAL_RECORD = "terminal.record", Shells, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Keep the last output", "Whether the last of what a shell printed is kept in its environment's history when the shell closes.";
    TERMINAL_DETACHED_MINUTES = "terminal.detached_minutes", Shells, Policy, SYSTEM_TO_TOPIC, Replace, value("480"), int(1, Unit::Minutes),
        "Detached time", "For how many minutes a shell keeps running with nobody attached before it is closed.";
    TERMINAL_MAX_PER_PERSON = "terminal.max_per_person", Shells, Policy, SYSTEM_TO_TOPIC, Replace, value("4"), int(1, Unit::Count),
        "Shells per person", "How many shells one person may have open at once in one environment.";
    TERMINAL_MAX_PER_ENVIRONMENT = "terminal.max_per_environment", Shells, Policy, SYSTEM_TO_TOPIC, Replace, value("16"), int(1, Unit::Count),
        "Shells per environment", "How many shells may be open at once in one environment, everybody's together.";
    BROWSER_ENABLED = "browser.enabled", Browsers, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Browsers", "Whether the environments here show their browsers, start one when asked, and offer their agents the tool for them. Off, an agent may still run a browser as it runs any program.";
    PORTS_ENABLED = "ports.enabled", Ports, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Ports", "Whether current door holders may use owned TCP and UDP ports through Orch.";
    PORTS_MAX_OPEN = "ports.max_open", Ports, Policy, SYSTEM_TO_TOPIC, Min, value("16"), int_between(0, 65535, Unit::Count),
        "Wider ports", "The distinct port numbers with wider or pending audiences; zero leaves own use available.";
    PORTS_LINK_MAX_SECS = "ports.link_max_secs", Ports, Policy, SYSTEM_TO_TOPIC, Min, value("86400"), int(1, Unit::Seconds),
        "Link lifetime", "The longest link lifetime from creation, including later expiry changes.";
    REQUESTS_EXPIRY_HOURS = "requests.expiry_hours", Confinement, Policy, SYSTEM_TO_TOPIC, Min, value("24"), int_between(1, 168, Unit::Hours),
        "Pending agent requests", "The longest pending boundary decision an agent requests; an earlier requested port-link lapse also ends it.";
    BROWSER_PRESET = "browser.preset", Browsers, Parameter, ORG_TO_TOPIC_ENV, Replace, value("\"headless\""), choice(&["headless", "window"]),
        "Browser preset", "How a browser is started where nobody says: headless, with no window and no display, or with a window on a display that Orch opens for it.";
    BROWSER_ARGS = "browser.args", Browsers, Parameter, ORG_TO_TOPIC_ENV, Replace, EMPTY_LIST, list(&TEXT),
        "Browser arguments", "Arguments added to those a browser is started with; the ones Orch gives itself, the profile's directory and the address it is driven at, cannot be set here.";
    BROWSER_VIEWPORT = "browser.viewport", Browsers, Parameter, ORG_TO_TOPIC_ENV, Replace, value("\"1280x800\""), GEOMETRY,
        "Browser window size", "The size of a new browser's window, written width x height, from 320x240 to 3840x2160.";
    BROWSER_IDLE_MINUTES = "browser.idle_minutes", Browsers, Parameter, SYSTEM_TO_TOPIC_ENV, Replace, value("0"), int(0, Unit::Minutes),
        "Browser idle close", "Close a browser that Orch started once it has been idle this many minutes: nobody watched or drove it and no agent's tool named it; 0 never does.";
    BROWSER_MAX_PER_ENVIRONMENT = "browser.max_per_environment", Browsers, Policy, SYSTEM_TO_TOPIC, Replace, value("4"), int(1, Unit::Count),
        "Browsers per environment", "How many browsers that Orch started may run at once in one environment.";
    VNC_ENABLED = "vnc.enabled", Displays, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Displays", "Whether the environments here show their displays, open one when asked, and offer their agents the tool for them.";
    VNC_GEOMETRY = "vnc.geometry", Displays, Parameter, ORG_TO_TOPIC_ENV, Replace, value("\"1280x800\""), GEOMETRY,
        "Display size", "The size of a display that Orch opens, written width x height, from 320x240 to 3840x2160.";
    VNC_MAX_PER_ENVIRONMENT = "vnc.max_per_environment", Displays, Policy, SYSTEM_TO_TOPIC, Replace, value("4"), int(1, Unit::Count),
        "Displays per environment", "How many displays that Orch opened may run at once in one environment.";
    ENV_IDLE_WARNING_MINUTES = "env.idle_warning_minutes", Environment, Parameter, SYSTEM_TO_TOPIC_ENV, Replace, value("60"), int(0, Unit::Minutes),
        "Idle warning", "How many minutes before an idle stop its warning is sent; 0 sends none.";
    ENV_REMOVED_KEEP_DAYS = "env.removed_keep_days", Environment, Policy, SYSTEM_TO_TOPIC_ENV, Replace, value("7"), int(0, Unit::Days),
        "Removed files", "Days to keep owned files after unit removal, bounded by the system retention window; 0 makes them eligible for immediate removal.";
    ENV_UNUSED_REMOVE_DAYS = "env.unused_remove_days", Environment, Parameter, SYSTEM_TO_TOPIC_ENV, Replace, value("0"), int(0, Unit::Days),
        "Remove when unused", "Schedule the removal of an environment made from a template once no thread has been in it for this many days; 0 never does.";
    ENV_LIFETIME = "env.lifetime", Environment, Parameter, ENVIRONMENT, Replace, value("\"lasting\""), choice(&["lasting", "temporary", "thread"]),
        "Lifetime", "Keep this environment, remove it after idle time, or also remove it when its creating thread is archived. Temporary environments include their owned files; existing directories are never deleted.";
    ENV_TEMPORARY_IDLE_HOURS = "env.temporary_idle_hours", Environment, Parameter, SYSTEM_TO_TOPIC_ENV, Replace, value("24"), int(1, Unit::Hours),
        "Temporary idle time", "Hours without activity before a temporary environment and its owned files are removed. Its owner gets an hour to export or keep it.";
    ENV_RECYCLE_IDLE_MINUTES = "env.recycle_idle_minutes", Environment, Parameter, SYSTEM_TO_PROJECT, Replace, value("60"), int(0, Unit::Minutes),
        "Recycle idle time", "The idle time, in minutes, that the Recycle dialog starts from; 0 applies no minimum idle time.";
    HOSTS_FULL_PERCENT = "hosts.full_percent", System, Policy, SYSTEM, Replace, value("90"), int_between(1, 100, Unit::Percent),
        "Full at", "The share of a host's memory, declared as needed by what runs there, at which the host counts as full.";
    HOSTS_WHEN_FULL = "hosts.when_full", System, Policy, SYSTEM, Replace, value("\"warn\""), choice(&["warn", "block"]),
        "When a host is full", "Whether making or moving an environment onto a full host only warns, or is refused beforehand.";
    RUNTIME_IDLE_STOP_MINUTES = "runtime.idle_stop_minutes", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Replace, value("1440"), int(0, Unit::Minutes),
        "Idle stop", "Stop a managed provider's runtime after this many minutes without a session or a lease; 0 never stops it.";
    RUNTIME_TEMPLATE_ID = "runtime.template_id", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Replace, FROM_SEEDS, ID,
        "Template", "The provider template that a managed provider's runtime is made from.";
    RUNTIME_HOST_ID = "runtime.host_id", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Replace, UNSET, ID,
        "Host", "The host that a managed provider's runtime is made on; unset, the one with the most room.";
    RUNTIME_RESOURCES = "runtime.resources", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Map, EMPTY_MAP, ENVIRONMENT_LIMITS,
        "Limits", "Limits over the template's values: processor, memory, processes and disk; the driver declares which limits it enforces.";
    RUNTIME_STORAGE = "runtime.storage", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Map, EMPTY_MAP, STORAGE,
        "Storage", "Named machine storages for new provider home and connector state data; provider runtimes have no workspace.";
    RUNTIME_HOME = "runtime.home", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Replace, UNSET, DATA_SOURCE,
        "Harness home", "A home made by Orch or an existing directory allowed by the runtime's machine. Unset uses the profile's choice or the harness's own default, preserving an existing sign-in there; a separate credential keeper retains its own home.";
    RUNTIME_NEEDS = "runtime.needs", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Map, EMPTY_MAP, ENVIRONMENT_NEEDS,
        "Needs", "The processor, memory and disk a managed provider runtime declares it needs for placement.";
    RUNTIME_AFTER_UP = "runtime.after_up", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Map, EMPTY_MAP, AFTER_UP,
        "Commands after start", "Named argv commands run in the provider runtime after each start or attach, where the machine permits them; only its owners may set them on the provider itself.";
    RUNTIME_VARIABLES = "runtime.variables", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Map, EMPTY_MAP, map(&TEXT),
        "Variables", "Environment variables supplied to the provider runtime over its template and agent profile; changes apply when that runtime starts or attaches.";
    RUNTIME_SECRET_NAMES = "runtime.secret_names", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Union, EMPTY_LIST, set(&SECRET_NAME),
        "Secrets", "Names of sealed provider secrets supplied as runtime variables over the agent profile; values are sealed to the runtime's machine, written on the provider and never returned.";
    RUNTIME_UPDATE = "runtime.update", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Replace, value("\"manual\""), UPDATE,
        "Updates", "Whether a runtime that is behind waits for somebody to update it, or is updated whenever it starts.";
    RUNTIME_RENEW_DAYS = "runtime.renew_days", ProviderRuntimes, Parameter, UNIT_RUNTIME_SCOPES, Replace, value("5"), int(0, Unit::Days),
        "Renewal", "Renew a sign-in when fewer than this many days remain, or this many days after its last renewal when its expiry is unknown; 0 switches renewal off.";
    PROVIDERS_PRESETS = "providers.presets", Providers, Parameter, SYSTEM_ORG, Map, FROM_SEEDS, map(&PRESET_OR_HIDDEN),
        "Provider presets", "Ready-made API upstreams offered when a provider is added, by name; a lower scope adds one, replaces one, or hides one.";
    PROVIDERS_ALLOW_HTTP_UPSTREAMS = "providers.allow_http_upstreams", Providers, Policy, SYSTEM_ORG, Replace, FALSE, BOOL,
        "Upstreams without TLS", "Whether an API endpoint may be reached without TLS, as for a model server on a private network.";
    PROVIDERS_EXPIRY_NOTICE_DAYS = "providers.expiry_notice_days", Providers, Parameter, SYSTEM_ORG, Replace, value("14"), int(0, Unit::Days),
        "Expiry notice", "How many days before a token that a provider runs on lapses its managers are told; 0 tells them only once it has lapsed.";
    PROVIDERS_SUBSCRIPTION_HARNESSES = "providers.subscription_harnesses", Providers, Policy, SYSTEM_ORG, Intersect, UNSET, set(&TEXT),
        "Subscriptions allowed", "The kinds of harness whose subscription may be added as a provider here.";
    PROVIDERS_SUBSCRIPTION_SHARING = "providers.subscription_sharing", Providers, Policy, SYSTEM_ORG, Intersect, UNSET, set(&TEXT),
        "Subscriptions shared", "The kinds of harness whose subscription may be used by people other than its owner.";
    RESOURCES_CREATED_BY_MEMBERS = "resources.created_by_members", Providers, Policy, SYSTEM_ORG, Intersect, value("[\"provider\",\"host\",\"template\",\"mcp_server\",\"mount\",\"skill\"]"), set(&RESOURCE_KIND),
        "Resources members may add", "The kinds of resource that any member may make in an org as their own.";
    SKILLS_MAX_BYTES = "skills.max_bytes", Providers, Policy, SYSTEM_ORG, Min, value("33554432"), int(1, Unit::Bytes),
        "Skill size", "The most bytes the files of one version of a skill hold together; a skill is instructions, scripts and small assets, and each version is read whole when it is made.";
    SKILLS_MAX_FILES = "skills.max_files", Providers, Policy, SYSTEM_ORG, Min, value("10000"), int(1, Unit::Count),
        "Skill files", "The most files one version of a skill holds; an agent reads a skill file by file from its SKILL.md, and each version's manifest lists every file.";
    NOTIFY_SUBAGENTS = "notify.subagents", Collaboration, Parameter, SYSTEM_TO_THREAD, Replace, FALSE, BOOL,
        "Sub-agent notices", "Whether the people who follow a thread are told when one of its sub-agents finishes, fails or is stopped, as they are told when a turn ends; the agent that started it is told by its program either way.";
    NOTIFY_OPERATIONS = "notify.operations", Collaboration, Parameter, SYSTEM_TO_THREAD_ENV_PROVIDER, Replace, value("\"when_blocking\""), choice(&["when_blocking", "always", "never"]),
        "Operation notices", "Whether Notify starts ticked in an operation: when work is blocked, always, or never. The person confirms the choice; recipients keep their own notification choices.";
    BROADCAST_RATE_PER_HOUR = "broadcast.rate_per_hour", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("6"), int(0, Unit::Count),
        "Announcements an hour", "How many announcements one thread's agent may send in a rolling hour; 0 allows none.";
    BROADCAST_AGENT_SCOPE = "broadcast.agent_scope", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("\"topic\""), choice(&["topic", "project", "org"]),
        "Announcement reach", "How far an agent's announcement may reach: its topic, its project or its nearest org.";
    CAPABILITY_RECREATE_GRACE_SECS = "capability.recreate_grace_secs", Confinement, Parameter, SYSTEM_TO_TOPIC_ENV, Replace, value("120"), int(0, Unit::Seconds),
        "Recreate grace", "How many seconds running turns may finish before a recreate interrupts them; 0 interrupts them immediately.";
    ACCESS_REQUEST_EXPIRY_DAYS = "access.request_expiry_days", Access, Policy, SYSTEM, Replace, value("14"), int(1, Unit::Days),
        "Access request expiry", "After how many days a request for access that nobody decided lapses.";
    ACCESS_MAX_PENDING_REQUESTS = "access.max_pending_requests", Access, Policy, SYSTEM, Replace, value("10"), int(1, Unit::Count),
        "Pending requests", "How many requests for access one person may have waiting.";
    ACCESS_QUICK_GRANT_DURATIONS = "access.quick_grant_durations", Access, Policy, SYSTEM_ORG, Replace, value("[\"1d\",\"7d\",\"30d\",\"always\"]"), set(&TEXT),
        "Quick access durations", "Durations offered for giving access in one click: a positive number of days followed by d, or always.";
    NODES_NEW_COLOR = "nodes.new_color", Automation, Parameter, ORG_TO_PROJECT, Replace, value("\"kind\""), choice(&["kind", "random"]), "New node colour", "Whether a new place starts with its kind's icon colour or one of the twelve picked at random.";
    TAGS_MAX_PER_PLACE = "tags.max_per_place", Limits, Policy, SYSTEM_TO_THREAD, Replace, value("5"), int(0, Unit::Count),
        "Tags on a place", "How many tags one place carries at most; 0 allows none.";
    AGENT_DEFAULT_PROVIDER_ID = "agent.default_provider_id", Agent, Parameter, ORG_TO_TOPIC, Replace, UNSET, ID,
        "Default provider", "The provider that new threads here are put on.";
    AGENT_DEFAULT_AGENT_ID = "agent.default_agent_id", Agent, Parameter, ORG_TO_TOPIC, Replace, UNSET, ID,
        "Default agent profile", "The agent profile that new threads here use.";
    TEMPLATES_RESTRICTED_DRIVERS = "templates.restricted_drivers", Providers, Policy, SYSTEM, Replace, value("[\"host\"]"), set(&TEXT),
        "Restricted drivers", "The drivers whose templates only system admins may define.";
    SWITCH_BATCH_PARALLEL = "switch.batch_parallel", Switching, Policy, SYSTEM, Replace, value("4"), int(1, Unit::Count),
        "Batch size", "How many switches, moves or updates of one batch run at the same time.";
    SWITCH_BATCH_THREADS = "switch.batch_threads", Switching, Policy, SYSTEM, Replace, value("200"), int(1, Unit::Count),
        "Threads per batch", "How many threads one batch may plan; raise this default for a larger reviewed switch.";
    MEASUREMENT_KEEPALIVE_SECS = "jobs.measurement_keepalive_secs", Switching, Policy, SYSTEM, Replace, value("300"), int(1, Unit::Seconds),
        "Keep counting", "How long a count continues after its last open plan stops watching.";
    MEASUREMENT_VALID_SECS = "jobs.measurement_valid_secs", Switching, Policy, SYSTEM, Replace, value("900"), int(1, Unit::Seconds),
        "Count freshness", "How long a completed count can be reused in a plan.";
    MEASUREMENT_TIMEOUT_SECS = "jobs.measurement_timeout_secs", Switching, Policy, SYSTEM, Replace, value("86400"), int(1, Unit::Seconds),
        "Counting time", "How long a background measurement may count before it is stopped.";
    JOB_STALL_SECS = "jobs.stall_secs", Switching, Policy, SYSTEM, Replace, value("30"), int(1, Unit::Seconds),
        "Waiting indication", "How long without a changed count before a job says it is waiting.";
    JOB_STOP_WAIT_SECS = "jobs.stop_wait_secs", Switching, Policy, SYSTEM, Replace, value("30"), int(1, Unit::Seconds),
        "Stopping indication", "How long before an unconfirmed cancellation names the machine that is still stopping.";
    JOB_IO_TIMEOUT_SECS = "jobs.io_timeout_secs", Switching, Policy, SYSTEM, Replace, value("21600"), int(1, Unit::Seconds),
        "File work allowance", "How long a copy, archive, upload or restore command may run before its work is stopped.";
    SWITCH_ACTIVE_TURN = "switch.active_turn", Switching, Parameter, SYSTEM_TO_THREAD_ENV, Replace, value("\"wait\""), choice(&["wait", "interrupt"]),
        "Running turn", "What a switch, a move, a recreate or an update does with a turn that is running: wait for it, or interrupt it.";
    SWITCH_WAIT_MINUTES = "switch.wait_minutes", Switching, Parameter, SYSTEM_TO_THREAD_ENV, Replace, value("10"), int(0, Unit::Minutes),
        "Wait", "For how many minutes it waits for running turns to end before it gives up and leaves everything as it was; 0 does not wait.";
    SWITCH_VERIFY_TIMEOUT_SECS = "switch.verify_timeout_secs", Switching, Parameter, SYSTEM_TO_THREAD_ENV, Replace, value("120"), int(10, Unit::Seconds),
        "Health check time", "How many seconds the checks of the new side may take before everything is rolled back.";
    SWITCH_KEEP_PREVIOUS = "switch.keep_previous", Switching, Parameter, SYSTEM_TO_THREAD_ENV, Replace, FALSE, BOOL,
        "Keep the old side", "Whether the side that a switch or a move leaves behind is kept instead of scheduled for removal.";
    BINDING_PREFER = "binding.prefer", Confinement, Parameter, SYSTEM_TO_TOPIC, Replace, UNSET, set(&ARRANGEMENT),
        "Preferred arrangements", "The arrangement types that are tried first, in this order; the rest follow in each adapter's own.";
    BINDING_ALLOWED = "binding.allowed", Confinement, Policy, SYSTEM_TO_TOPIC, Intersect, UNSET, set(&ARRANGEMENT),
        "Allowed arrangements", "The arrangement types that may be used here.";
    CREDENTIAL_REFRESH_MARGIN_SECS = "credential.refresh_margin_secs", Confinement, Policy, SYSTEM, Replace, value("300"), int(30, Unit::Seconds),
        "Credential refresh margin", "A shared copy of a credential that expires is renewed when fewer than this many seconds remain.";
    AUTO_TITLE = "auto.title", Automation, Parameter, SYSTEM_TO_TOPIC, Replace, value("\"harness\""), choice(&["off", "first_input", "harness", "summarizer"]),
        "Titles", "Who writes automatic titles: nobody, the first input, the harness, or the summarizer.";
    AUTO_DESCRIPTION = "auto.description", Automation, Parameter, SYSTEM_TO_TOPIC, Replace, value("\"off\""), OFF_OR_SUMMARIZER,
        "Descriptions", "Whether the summarizer keeps a description as live progress.";
    AUTO_TAGS = "auto.tags", Automation, Parameter, SYSTEM_TO_THREAD, Replace, value("\"off\""), OFF_OR_SUMMARIZER,
        "Tags", "Whether the summarizer sets a place's tags.";
    AUTO_TOPIC_ROLLUP = "auto.topic_rollup", Automation, Parameter, SYSTEM_TO_TOPIC, Replace, value("\"off\""), choice(&["off", "rollup"]),
        "Topic description", "Whether a topic's description is summed up from those of its threads.";
    SUMMARY_HANDOFF_INPUT_SHARE = "summary.handoff_input_share", Automation, Policy, SYSTEM_TO_TOPIC, Replace, value("50"), int_between(1, 100, Unit::Percent),
        "Summary input share", "How much of the summary model's context window a hand-off, a fork or a goal check may fill with the conversation it sends; the newest turns first, older ones as their one-line index.";
    AUTO_SUMMARIZER = "auto.summarizer", Automation, Parameter, SYSTEM_TO_TOPIC, Replace, UNSET, SUMMARIZER,
        "Summarizer", "Which summarizer is used, and its configuration.";
    AUTO_TRIGGERS = "auto.triggers", Automation, Parameter, SYSTEM_TO_TOPIC, Replace, value("{\"on\":[\"turn_completed\",\"milestone\"],\"min_interval_secs\":120}"), TRIGGERS,
        "Triggers", "When automatic titles and descriptions are written again: after a turn, at a milestone or at intervals, and how often at most.";
    CAPABILITY_POLICY = "capability.policy", Confinement, Policy, SYSTEM_TO_TOPIC, Map, FROM_SEEDS, map(&CAPABILITY_DECISION),
        "Capability policy", "For each capability, whether a request for it is allowed, asked about or denied, and who decides.";
    SHARING_ALLOW_EXTERNAL = "sharing.allow_external", Collaboration, Policy, SYSTEM_TO_PROJECT, Replace, TRUE, BOOL,
        "Owners share outside", "Whether the owners of a thing here may themselves name somebody from outside its root org on its page.";
    SHARING_EXTERNAL_MAX_DAYS = "sharing.external_max_days", Collaboration, Policy, SYSTEM_TO_TOPIC, Min, UNSET, int(1, Unit::Days),
        "Outside entries end", "The longest an entry that names somebody from outside may last after it is written or renewed.";
    SHARING_ALLOW_HOLDERS = "sharing.allow_holders", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Holders share", "Whether somebody who holds a thing here without owning it or managing its page may share it, at most at their own level: name others on its page, give a role in a place, share it with agents. When off, sharing is with a thing's owners and whoever manages its page.";
    USAGE_RATES = "usage.rates", Limits, Policy, SYSTEM_ORG, Map, EMPTY_MAP, map(&RATE),
        "Rate card", "What models cost in credits, by a pattern of their routing names.";
    USAGE_CATALOGUE = "usage.catalogue", Limits, Policy, SYSTEM_ORG, Replace, TRUE, BOOL,
        "Estimates", "Whether use that has no rate is estimated from the rate catalogue that ships with Orch.";
    USAGE_CATALOGUE_SCALE = "usage.catalogue_scale", Limits, Policy, SYSTEM_ORG, Replace, value("\"1\""), POSITIVE_DECIMAL,
        "Catalogue scale", "What one unit of the rate catalogue is worth in this place's credits.";
    USAGE_CATALOGUE_URL = "usage.catalogue_url", Limits, Policy, SYSTEM, Replace, NULL, HTTPS_ADDRESS_OR_NONE,
        "Catalogue address", "An https address from which the system may fetch a newer rate catalogue.";
    USAGE_CATALOGUE_REFRESH_DAYS = "usage.catalogue_refresh_days", Limits, Policy, SYSTEM, Replace, value("0"), int(0, Unit::Days),
        "Catalogue refresh", "Every how many days the system fetches the rate catalogue by itself; 0 only when an admin asks.";
    CHAT_ENABLED = "chat.enabled", ChatDocuments, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Chats", "Whether people may post in the chats of the nodes here.";
    CHAT_DIRECT = "chat.direct", ChatDocuments, Policy, SYSTEM_ORG, Replace, value("\"members\""), choice(&["members", "off"]),
        "Direct chats", "Whether the members of a root org may start direct and group chats with each other.";
    CHAT_GROUP_MAX_MEMBERS = "chat.group_max_members", ChatDocuments, Policy, SYSTEM, Replace, value("10"), int(2, Unit::Count),
        "Group size", "How many people one group chat may have.";
    CHAT_GROUP_MENTIONS = "chat.group_mentions", ChatDocuments, Policy, SYSTEM_TO_THREAD, Replace, value("\"writers\""), choice(&["writers", "managers"]),
        "Role and group mentions", "Who may name role and group audiences among current readers: chat writers, or chat managers; the creator manages a group chat.";
    NOTIFICATIONS_PERSON_RATE_PER_HOUR = "notifications.person_rate_per_hour", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("120"), int(0, Unit::Count),
        "Person notifications per hour", "How many people one person may explicitly notify in one root org in a rolling hour; 0 disables the limit.";
    NOTIFICATIONS_PERSON_NOTE_CHARS = "notifications.person_note_chars", Collaboration, Policy, SYSTEM_TO_TOPIC, Replace, value("200"), int(0, Unit::Count),
        "Person notification note length", "How many characters an optional notification note may have; 0 allows no note.";
    REFERENCES_PREVIEW_BYTES = "references.preview_bytes", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("262144"), int(1, Unit::Bytes),
        "File preview size", "Bytes a reference card reads for a text excerpt.";
    REFERENCES_PREVIEW_LINES = "references.preview_lines", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("2"), int(0, Unit::Count),
        "File preview context", "Lines shown before and after a referenced line; 0 shows only the referenced line.";
    FILES_PREVIEW_SOURCE_BYTES = "files.preview_source_bytes", ChatDocuments, Parameter, SYSTEM, Replace, value("16777216"), int(1, Unit::Bytes),
        "Preview source size", "The largest original decoded for a static file preview.";
    FILES_PREVIEW_PIXELS = "files.preview_pixels", ChatDocuments, Parameter, SYSTEM, Replace, value("40000000"), int(1, Unit::Count),
        "Preview image pixels", "The largest image decoded for a static preview, checked before allocating pixels.";
    FILES_PREVIEW_EDGE = "files.preview_edge", ChatDocuments, Parameter, SYSTEM, Replace, value("512"), int(1, Unit::Count),
        "Preview dimensions", "The longest edge of a static file thumbnail, in pixels.";
    FILES_PREVIEW_BYTES = "files.preview_bytes", ChatDocuments, Parameter, SYSTEM, Replace, value("262144"), int(1, Unit::Bytes),
        "Preview size", "The most encoded bytes in one static thumbnail or text excerpt.";
    FILES_PREVIEW_CACHE_BYTES = "files.preview_cache_bytes", ChatDocuments, Parameter, SYSTEM, Replace, value("67108864"), int(0, Unit::Bytes),
        "Preview cache", "Bytes kept for derived previews; older previews are discarded first, and zero disables caching.";
    FILES_VIEWER_BYTES = "files.viewer_bytes", ChatDocuments, Parameter, SYSTEM, Replace, value("67108864"), int(1, Unit::Bytes),
        "Media viewer size", "The largest original image, PDF, audio or video file read into a browser viewer.";
    FILES_NATIVE_IMAGE_BYTES = "files.native_image_bytes", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("20971520"), int(0, Unit::Bytes),
        "Native image size", "The largest image sent natively to a model; larger images are delivered by path, and zero always uses paths.";
    DOCS_DESCRIPTION_CHARS = "docs.description_chars", Automation, Parameter, SYSTEM_TO_TOPIC, Replace, value("200"), int_between(1, 4000, Unit::Count),
        "Document description length", "Characters the summarizer may write for a document's description.";
    DOCS_ENABLED = "docs.enabled", ChatDocuments, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Documents", "Whether new documents may be made beside the nodes here.";
    DOCS_TEMPLATES = "docs.templates", ChatDocuments, Parameter, SYSTEM_TO_THREAD, Union, EMPTY_LIST, set(&ID),
        "Starting documents", "Current readable ordinary documents offered when creating one here; a template grants no source access or creation permission.";
    DOCS_MAX_BYTES = "docs.max_bytes", ChatDocuments, Policy, SYSTEM_TO_TOPIC, Replace, value("2097152"), int(65_536, Unit::Bytes),
        "Document size", "The most text one document here may hold, in bytes.";
    DOCS_MAX_BLOCKS = "docs.max_blocks", ChatDocuments, Policy, SYSTEM_TO_TOPIC, Replace, value("5000"), int(100, Unit::Count),
        "Document blocks", "The most blocks one document here may have.";
    DOCS_VERSIONS_KEPT = "docs.versions_kept", ChatDocuments, Policy, SYSTEM_TO_TOPIC, Replace, value("200"), int(1, Unit::Count),
        "Versions kept", "How many versions of one document here are kept at most.";
    DOCS_HISTORY_MAX_BYTES = "docs.history_max_bytes", ChatDocuments, Policy, SYSTEM_TO_TOPIC, Replace, value("16777216"), BYTES,
        "History size", "How much the kept versions of one document here may take together, in bytes.";
    DOCS_FILE_MAX_BYTES = "docs.file_max_bytes", ChatDocuments, Policy, SYSTEM_TO_TOPIC, Replace, DefaultOf::SameAs("storage.max_blob_bytes"), BYTES,
        "File size", "The largest one file a document here may hold, in bytes.";
    DOCS_FILES_MAX_BYTES = "docs.files_max_bytes", ChatDocuments, Policy, SYSTEM_TO_TOPIC, Replace, value("1073741824"), BYTES,
        "Files together", "The most the files of one document here may come to together, in bytes.";
    DOCUMENTS_LINKS_SHOWN = "documents.links.shown", ChatDocuments, Policy, SYSTEM_TO_TOPIC, Replace,
        value("[\"referenced_by\",\"references\",\"related\",\"graph\"]"), set(&LINKS_TAB),
        "Links tabs", "The tabs of a document's Links section that its readers see; the document's owner can switch tabs off for one document.";
    DOCUMENTS_LINKS_PAGE_SIZE = "documents.links.page_size", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("20"), int(1, Unit::Count),
        "Links page size", "Rows shown in a Links list before Show more.";
    DOCUMENTS_LINKS_OCCURRENCES = "documents.links.occurrences", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("5"), int(1, Unit::Count),
        "Mentions per source", "Mentions listed under one source in Referenced by before Show more.";
    DOCUMENTS_LINKS_LINE_CHARS = "documents.links.line_chars", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("140"), int(20, Unit::Count),
        "Mention line length", "Characters of the line shown around each mention in Referenced by.";
    DOCUMENTS_LINKS_MENTIONS_COUNTED = "documents.links.mentions_counted", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("100"), int(1, Unit::Count),
        "Mentions counted", "Mentions of a document counted in one source before its count reads \"or more\".";
    DOCUMENTS_LINKS_ACCESS_CHECKS = "documents.links.access_checks", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("500"), int(1, Unit::Count),
        "Links access checks", "Sources whose reading access is checked for one page of Referenced by; a very popular document continues with a cursor.";
    DOCUMENTS_LINKS_GRAPH_NODES = "documents.links.graph_nodes", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("150"), int(1, Unit::Count),
        "Graph nodes", "The most nodes the Links graph draws.";
    CHAT_ATTACHMENT_MAX_BYTES = "chat.attachment_max_bytes", ChatDocuments, Policy, SYSTEM_ORG, Replace, DefaultOf::SameAs("storage.max_blob_bytes"), BYTES,
        "Attachment size", "The largest file attached to a chat message, in bytes.";
    // Fifty candidates oversample the twenty-row screen; a fourfold recent sample detects dense terms.
    SEARCH_WORD_RECENT_CANDIDATES = "search.word_recent_candidates", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("200"), int(1, Unit::Count),
        "Recent word sample", "Recent rows sampled before probing the text index.";
    SEARCH_WORD_CANDIDATES = "search.word_candidates", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("50"), int(1, Unit::Count),
        "Word candidates", "Candidates per indexed field before exact access and ranking.";
    SEARCH_WORD_PROBE_CANDIDATES = "search.word_probe_candidates", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("20000"), int(1, Unit::Count),
        "Word index probe", "Indexed matches sampled before switching to a recency scan.";
    // The million-document fixture has about 24,000 header/body rows inside its one-percent scope.
    // Thirty thousand leaves estimation room while keeping larger scopes on their global indexes.
    SEARCH_WORD_TUNING = "search.word_tuning", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("{\"scope_candidates\":30000,\"tiers\":{}}"), WORD_TUNING,
        "Word retrieval", "Estimated rows below which word search uses native scoped reads. Missing field limits use Word candidates. Request windows defaults to one; further results use Show more.";
    SEARCH_PAGE_SIZE = "search.page_size", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("20"), int(1, Unit::Count),
        "Page size", "Results shown before Show more.";
    SEARCH_REORDER_WINDOW_SECS = "search.reorder_window_secs", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("5"), int(0, Unit::Seconds),
        "Reorder window", "How long new results may reorder the first screen.";
    SEARCH_MEANING_PAUSE_MS = "search.meaning_pause_ms", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("800"), int(0, Unit::Milliseconds),
        "Typing pause", "How long typing pauses before an embeddings request, in milliseconds.";
    SEARCH_CURSOR_ENTRIES = "search.cursor_entries", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("100"), int(1, Unit::Count),
        "Saved searches", "Continuations kept for each reader.";
    SEARCH_CURSOR_TTL_SECS = "search.cursor_ttl_secs", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("900"), int(1, Unit::Seconds),
        "Search lifetime", "How long a search's continuation stays available.";
    SEARCH_VECTOR_CACHE_ENTRIES = "search.vector_cache_entries", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("1000"), int(0, Unit::Count),
        "Query cache size", "Query vectors cached per org; zero disables caching.";
    SEARCH_VECTOR_CACHE_SECS = "search.vector_cache_secs", ChatDocuments, Parameter, SYSTEM_ORG, Replace, value("300"), int(0, Unit::Seconds),
        "Vector cache", "How long an org reuses a query vector for the same model.";
    SEARCH_SEMANTIC_ENABLED = "search.semantic.enabled", ChatDocuments, Policy, SYSTEM_ORG, Replace, FALSE, BOOL,
        "Semantic search", "Also find results with a similar meaning, using the embeddings below.";
    RELATED_WEIGHTS = "related.weights", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace,
        value("{\"meaning\":\"0.6\",\"place\":\"0.25\",\"references\":\"0.15\"}"), RELATED_WEIGHTS,
        "Related weights", "Relative weights of meaning, place and references, as decimal text; zero leaves a signal out.";
    RELATED_PLACE_SCORES = "related.place_scores", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace,
        value("{\"folder\":\"1.0\",\"topic\":\"0.7\",\"project\":\"0.4\",\"org\":\"0.1\"}"), RELATED_PLACE_SCORES,
        "Related places", "Scores from zero to one for items in the same folder, topic, project or org.";
    RELATED_COUNT = "related.count", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("8"), int(0, Unit::Count),
        "Related items", "Items shown in Related; zero switches it off.";
    // A candidate pool oversamples the eight visible rows, leaving room for exact permission checks.
    RELATED_CANDIDATES = "related.candidates", ChatDocuments, Parameter, SYSTEM_TO_TOPIC, Replace, value("200"), int(1, Unit::Count),
        "Related candidates", "Candidates per signal and reference neighborhood before exact access checks and ranking.";
    SEARCH_SEMANTIC_EMBEDDINGS = "search.semantic.embeddings", ChatDocuments, Parameter, SYSTEM_ORG, Replace, UNSET, EMBEDDINGS,
        "Embeddings", "Provider and model that compute embeddings; their use is counted to whoever names them.";
    SEARCH_SEMANTIC_MAX_DISK_SHARE = "search.semantic.max_disk_share", ChatDocuments, Policy, SYSTEM_ORG, Replace, value("100"), Schema::Nullable { of: &int(0, Unit::Percent) },
        "Disk limit", "Index size limit as a share of the indexed content's size; indexing pauses at the limit. Empty: no limit.";
    SEARCH_SEMANTIC_TUNING = "search.semantic.tuning", ChatDocuments, Parameter, SYSTEM_ORG, Replace,
        value("{\"chunk_tokens\":400,\"batch_tokens\":4096,\"candidates\":50,\"oversample\":4,\"fusion_k\":60,\"same_model_cosine_milli\":990,\"interval_secs\":10,\"query_wait_secs\":3,\"source_batch\":64,\"exact_filter_candidates\":20000,\"max_scan_tuples\":20000,\"scan_mem_multiplier\":2,\"iterative_scan\":\"strict_order\"}"), SEMANTIC_TUNING,
        "Indexing and ranking", "How text is split, embedded and ranked.";
    RETENTION_ARCHIVE_IDLE_DAYS = "retention.archive_idle_days", Retention, Policy, SYSTEM_TO_TOPIC, Replace, value("0"), int(0, Unit::Days),
        "Archive idle topics", "Archive a topic after this many days without activity; 0 never does.";
    RETENTION_DELETE_ARCHIVED_DAYS = "retention.delete_archived_days", Retention, Policy, SYSTEM_TO_THREAD, Replace, value("0"), int(0, Unit::Days),
        "Delete archived nodes", "Delete a node this many days after it was archived; 0 never does.";
    RETENTION_AGENT_HISTORY_IDLE_DAYS = "retention.agent_history_idle_days", Retention, Policy, SYSTEM_TO_THREAD, Replace, value("7"), int(0, Unit::Days),
        "Agent history kept", "Remove a thread's agent history after this many days without a turn; 0 never does.";
    RETENTION_INBOX_DAYS = "retention.inbox_days", Retention, Policy, SYSTEM, Replace, value("90"), int(1, Unit::Days),
        "Inbox kept", "For how many days a notification stays in a person's Inbox, read or not, before it is removed.";
    REDACTION_MASK_SECRETS = "redaction.mask_secrets", Redaction, Policy, SYSTEM_TO_TOPIC, Replace, TRUE, BOOL,
        "Mask secrets", "Whether the values of the secrets a thread was given are masked in stored text.";
    REDACTION_PATTERNS = "redaction.patterns", Redaction, Policy, SYSTEM_TO_TOPIC, Union, EMPTY_LIST, list(&REDACTION_PATTERN),
        "Patterns", "Further patterns to mask in stored text, each with a name.";
    DIAGNOSTICS_RETENTION_DAYS = "diagnostics.retention_days", Retention, Policy, SYSTEM, Replace, value("14"), int_between(1, 365, Unit::Days),
        "Diagnostics kept", "For how many days diagnostics are kept.";
    ORGS_PERSONAL_ENABLED = "orgs.personal_enabled", Joining, Policy, SYSTEM, Replace, TRUE, BOOL,
        "Personal organizations", "Whether a new human account gets a personal organization. Existing organizations keep their data and access when this is switched off.";
    ORGS_PERSONAL_CLOSED = "orgs.personal_closed", Joining, Policy, SYSTEM, Replace, TRUE, BOOL,
        "Personal orgs closed", "Whether a person's own org is created taking no member but its person.";
    ORGS_PERSONAL_FLAT = "orgs.personal_flat", Joining, Policy, SYSTEM, Replace, TRUE, BOOL,
        "Personal orgs flat", "Whether a person's own org is created holding no org inside it.";
    ORGS_CREATED_BY = "orgs.created_by", Joining, Policy, SYSTEM, Replace, value("\"invited\""), choice(&["anyone", "admins", "invited"]),
        "Who may make an org", "Who may make a team org: anybody signed in, system admins only, or somebody a system admin invited to make one, each invitation used once. Personal organizations follow their separate creation policy.";
    NAVIGATION_ORDER = "navigation.order", Collaboration, Parameter, SYSTEM_ORG, Replace, value("{\"by\":\"changed\"}"), TREE_ORDER,
        "Order of the tree", "The order in which the org's tree is shown to whoever follows the org's own: as somebody arranged it, the newest first, the most recently changed first, or by name. An owner or admin of the org saves it from the order they see.";
    REGISTRATION_MODE ="registration.mode", Joining, Policy, SYSTEM, Replace, value("\"invite\""), choice(&["closed", "invite", "open"]),
        "Registration", "Who may register: nobody, only somebody with an invitation, or anybody who reaches the system.";
    INVITATIONS_FORMS = "invitations.forms", Joining, Policy, SYSTEM_ORG, Replace, value("[\"link\",\"email\"]"), set(&INVITATION_FORM),
        "Invitation forms", "The forms of invitation that may be made for an org of this tree: a link to share, a personal link for one address, or both.";
    INVITATIONS_DEFAULT_HOURS = "invitations.default_hours", Joining, Policy, SYSTEM_ORG, Replace, value("168"), int(1, Unit::Hours),
        "Invitation life", "For how many hours an invitation is good when its inviter says nothing.";
    INVITATIONS_MAX_HOURS = "invitations.max_hours", Joining, Policy, SYSTEM_ORG, Replace, value("720"), int(1, Unit::Hours),
        "Longest invitation", "The most hours an invitation may be good for.";
    INVITATIONS_MAX_USES = "invitations.max_uses", Joining, Policy, SYSTEM_ORG, Replace, value("100"), int_between(1, 1000, Unit::Count),
        "Uses of a shared link", "How many people one shared link may bring in at most.";
    MAIL_SHARED = "mail.shared", System, Policy, SYSTEM, Replace, TRUE, BOOL,
        "Shared mail server", "Whether a root org that has no mail server of its own sends its mail through the system's.";
    RELEASE_CHECK_DAYS = "release.check_days", System, Policy, SYSTEM, Replace, value("0"), int(0, Unit::Days),
        "Look for a release", "Days between read-only checks of the operator's configured release source; 0 is off. A check sends no installation statistics and installs nothing.";
    CLEANUP_RELEASE_KEEP_COUNT = "cleanup.release_keep_count", Retention, Policy, SYSTEM, Replace, value("3"), int(0, Unit::Count),
        "Keep unused releases", "Unused releases kept per payload after an update; 0 keeps all. Current and previous releases are always kept.";
    CLEANUP_UNUSED_KEEP_DAYS = "cleanup.unused_keep_days", Retention, Policy, SYSTEM_HOST, Replace, value("7"), int(0, Unit::Days),
        "Clean unused caches", "Days to keep images and tool versions that nothing uses; 0 switches automatic cleanup off. Data follows its own retention dates.";
    BACKUP_WARN_DAYS = "backup.warn_days", System, Policy, SYSTEM, Replace, value("2"), int(1, Unit::Days),
        "Database backup warning", "Warn when the last successful database-only backup is older than this many days. A failed attempt does not make it newer.";
    REGISTRATION_EMAIL_VERIFICATION = "registration.email_verification", SigningIn, Policy, SYSTEM, Replace, FROM_INSTALLATION, choice(&["off", "required"]),
        "Email verification", "Whether a person has to confirm their address before they sign in.";
    AUTH_LOCAL_LOGIN = "auth.local_login", SigningIn, Policy, SYSTEM, Replace, TRUE, BOOL,
        "Password sign-in", "Whether people other than system admins may sign in with a password or a mailed code.";
    AUTH_EMAIL_CODE = "auth.email_code", SigningIn, Policy, SYSTEM_ORG, Replace, EMPTY_LIST, set(&EMAIL_CODE_USE),
        "Code by mail", "What a code sent to a person's confirmed address may be when they sign in: a way in by itself, a second step after the password, both or neither.";
    AUTH_SECOND_STEP = "auth.second_step", SigningIn, Policy, SYSTEM_ORG, Replace, value("\"optional\""), choice(&["optional", "required"]),
        "Second step", "Whether the people of this org need a second proof set up and given before they work here.";
    TOKENS_MAX_DAYS = "tokens.max_days", SigningIn, Policy, SYSTEM_ORG, Min, UNSET, int(1, Unit::Days),
        "Longest token life", "The longest total life of an API token that is admitted here: from when it was made to when it ends.";
    SYSTEM_BANNER = "system.banner", System, Policy, SYSTEM, Replace, NULL, BANNER_OR_NONE,
        "Banner", "A message shown to everybody on the system, with its level and when it ends.";
    OUTBOUND_ALLOWED_PRIVATE_HOSTS = "outbound.allowed_private_hosts", System, Policy, SYSTEM, Replace, EMPTY_LIST, list(&TEXT),
        "Private hosts", "The hosts on private addresses that a channel or a destination of an org or a person may reach.";
    STORAGE_MAX_BLOB_BYTES = "storage.max_blob_bytes", System, Policy, SYSTEM, Replace, value("8589934592"), BYTES,
        "Largest file", "The largest single file that Orch stores, in bytes.";
    STORAGE_ENV_CACHE_QUOTA_BYTES = "storage.env_cache_quota_bytes", Workspace, Policy, SYSTEM_TO_PROJECT, Replace, value("21474836480"), BYTES,
        "File cache", "How many bytes of cached file contents are kept for one environment.";
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_exact_program_policy_requires_its_version_instead_of_an_implicit_default() {
        for value in [
            serde_json::json!({"codex":{"use":"exact"}}),
            serde_json::json!({"codex":{"use":"exact","version":" "}}),
        ] {
            let error = super::validate(super::keys::AGENTS_HARNESS_VERSIONS, &value).unwrap_err();
            let super::Invalid::Value { faults, .. } = error else {
                panic!("the version must be refused as an invalid value");
            };
            assert!(faults.iter().any(|fault| fault.path == "codex.version"));
        }
        assert!(
            super::validate(
                super::keys::AGENTS_HARNESS_VERSIONS,
                &serde_json::json!({"codex":{"use":"exact","version":"0.162.1"}})
            )
            .is_ok()
        );
    }
    #[test]
    fn temporary_removal_is_an_environment_opt_in_and_idle_time_can_come_from_its_place() {
        use super::*;
        let lifetime = get("env.lifetime").unwrap();
        assert_eq!(lifetime.default_value(), Some(serde_json::json!("lasting")));
        for value in ["temporary", "thread", "lasting"] {
            assert!(
                validate_at(
                    "env.lifetime",
                    Scope::Environment,
                    &serde_json::json!(value)
                )
                .is_ok()
            );
        }
        assert!(
            validate_at(
                "env.lifetime",
                Scope::Topic,
                &serde_json::json!("temporary")
            )
            .is_err(),
            "a place cannot opt all of its environments into deletion"
        );
        let idle = get("env.temporary_idle_hours").unwrap();
        assert_eq!(idle.default_value(), Some(serde_json::json!(24)));
        for scope in [
            Scope::System,
            Scope::Org,
            Scope::Project,
            Scope::Topic,
            Scope::Environment,
        ] {
            assert!(validate_at(idle.key, scope, &serde_json::json!(6)).is_ok());
        }
        assert!(!idle.faults(&serde_json::json!(0)).is_empty());
        assert!(
            lifetime
                .faults(&lifetime.example_value().unwrap())
                .is_empty()
        );
        assert!(idle.faults(&idle.example_value().unwrap()).is_empty());
    }
    #[test]
    fn environments_and_provider_runtimes_accept_the_same_common_values_from_system_through_home() {
        use super::*;
        for member in [
            "update",
            "template_id",
            "host_id",
            "resources",
            "storage",
            "needs",
            "after_up",
            "variables",
            "secret_names",
            "idle_stop_minutes",
        ] {
            let env = get(&format!("env.{member}")).unwrap();
            let runtime = get(&format!("runtime.{member}")).unwrap();
            assert_eq!(
                env.schema.json_schema(),
                runtime.schema.json_schema(),
                "{member}"
            );
            assert_eq!(env.merge, runtime.merge, "{member}");
            for scope in [Scope::System, Scope::Org, Scope::Project, Scope::Topic] {
                assert!(
                    env.set_at(scope) && runtime.set_at(scope),
                    "{member} at {scope}"
                );
            }
            assert!(env.set_at(Scope::Environment));
            assert!(runtime.set_at(Scope::Provider));
        }
        assert!(
            validate_at(
                "runtime.resources",
                Scope::System,
                &serde_json::json!({"disk_gb":20})
            )
            .is_ok()
        );
        assert!(
            validate_at(
                "env.storage",
                Scope::System,
                &serde_json::json!({"workspace":"fast","home":"fast","state":"fast"})
            )
            .is_ok()
        );
        assert!(
            validate_at(
                "runtime.storage",
                Scope::Project,
                &serde_json::json!({"state":"fast","home":"fast"})
            )
            .is_ok()
        );
        let faults = get("runtime.storage")
            .unwrap()
            .faults(&serde_json::json!({"workspace":"fast"}));
        assert!(
            faults.iter().any(
                |fault| fault.message == crate::unit::runtime_unavailable("workspace").unwrap()
            )
        );
    }
    #[test]
    fn an_exact_host_list_canonicalizes_names_but_refuses_addresses_wildcards_and_urls() {
        use super::*;
        let names = canonical_value(
            keys::NETWORK_ALLOWED_HOSTS,
            &json!(["PYPI.ORG.", "pypi.org", "BÜCHER.example"]),
        );
        assert_eq!(names, json!(["pypi.org", "xn--bcher-kva.example"]));
        assert!(validate_at(keys::NETWORK_ALLOWED_HOSTS, Scope::Environment, &names).is_ok());
        for host in [
            "127.0.0.1",
            "0x7f.1",
            "2130706433",
            "*.example",
            "example:443",
            "https://example",
            "name/path",
            "name..",
        ] {
            assert!(
                validate(
                    keys::NETWORK_ALLOWED_HOSTS,
                    &canonical_value(keys::NETWORK_ALLOWED_HOSTS, &json!([host]))
                )
                .is_err(),
                "{host}"
            );
        }
    }
    use super::*;
    use orch_types::new_id;

    fn paths(result: Result<&'static SettingDef, Invalid>) -> Vec<String> {
        match result {
            Err(Invalid::Value { faults, .. }) => {
                faults.into_iter().map(|fault| fault.path).collect()
            }
            other => panic!("expected a value that does not fit, got {other:?}"),
        }
    }

    #[test]
    fn every_keys_default_validates_against_its_own_schema() {
        for def in all() {
            match def.default {
                DefaultOf::Value(text) => {
                    let value: Json = serde_json::from_str(text)
                        .unwrap_or_else(|_| panic!("{}: {text}", def.key));
                    assert_eq!(def.faults(&value), [], "{}", def.key);
                    assert_eq!(def.default_value(), Some(value), "{}", def.key);
                }
                DefaultOf::SameAs(other) => {
                    let other = get(other)
                        .unwrap_or_else(|| panic!("{} takes the value of no key", def.key));
                    assert_eq!(
                        other.schema, def.schema,
                        "{} and {} take the same values",
                        def.key, other.key
                    );
                }
                DefaultOf::Unset | DefaultOf::Supplied(_) => {
                    assert_eq!(def.default_value(), None, "{}", def.key)
                }
            }
        }
    }

    #[test]
    fn every_setting_help_example_is_a_valid_value_and_is_served_beside_its_default() {
        for def in all() {
            let example = def
                .example_value()
                .unwrap_or_else(|error| panic!("{}: {error}", def.key));
            let faults = def.faults(&example);
            assert!(
                faults.is_empty(),
                "{} help shows a value its control refuses: {faults:?}",
                def.key
            );
            let served =
                serde_json::to_value(def).unwrap_or_else(|error| panic!("{}: {error}", def.key));
            assert_eq!(served["example"], example, "{}", def.key);
            assert_eq!(
                served["default"],
                json!(def.default_value()),
                "an example must not replace {}'s default",
                def.key
            );
        }
    }

    #[test]
    fn no_key_is_in_the_registry_twice_and_each_is_found_by_its_key() {
        let mut seen = std::collections::HashSet::new();
        for def in all() {
            assert!(seen.insert(def.key), "{} twice", def.key);
            assert_eq!(get(def.key).map(|found| found.key), Some(def.key));
            assert!(
                !def.scopes.is_empty() && !def.title.is_empty() && def.description.ends_with('.'),
                "{}",
                def.key
            );
        }
        assert_eq!(get(keys::AGENT_MODEL).map(|def| def.title), Some("Model"));
    }

    #[test]
    fn native_subagent_limits_have_bounded_values_at_every_place_and_explicit_shipped_defaults() {
        for (key, maximum, shipped) in [
            (keys::AGENTS_SUBAGENTS_AT_ONCE, 64, 8),
            (keys::AGENTS_SUBAGENT_DEPTH, 5, 1),
        ] {
            let definition = get(key).unwrap();
            assert_eq!(definition.scopes, SYSTEM_TO_THREAD);
            assert_eq!(definition.merge, Merge::Min);
            assert!(validate(key, &json!(1)).is_ok());
            assert!(validate(key, &json!(maximum)).is_ok());
            assert!(validate(key, &json!(0)).is_err());
            assert!(validate(key, &json!(maximum + 1)).is_err());
            assert_eq!(
                serde_json::to_value(definition).unwrap()["default"],
                json!(shipped),
                "the served default is independent of the help example"
            );
            assert!(
                matches!(definition.default, DefaultOf::Value(value) if serde_json::from_str::<Json>(value).unwrap() == json!(shipped))
            );
        }
    }

    #[test]
    fn an_unknown_key_is_refused() {
        assert_eq!(
            validate("agent.mood", &json!("calm")),
            Err(Invalid::UnknownKey("agent.mood".into()))
        );
        assert_eq!(get("agent.mood"), None);
        assert!(matches!(
            validate_at("agent.mood", Scope::Org, &json!(1)),
            Err(Invalid::UnknownKey(_))
        ));
    }

    #[test]
    fn a_value_outside_its_bounds_is_refused() {
        assert!(validate(keys::THREAD_SIDE_MAX, &json!(4)).is_ok());
        assert_eq!(paths(validate(keys::THREAD_SIDE_MAX, &json!(0))), [""]);
        assert!(validate(keys::HOSTS_FULL_PERCENT, &json!(100)).is_ok());
        assert_eq!(paths(validate(keys::HOSTS_FULL_PERCENT, &json!(101))), [""]);
        assert_eq!(paths(validate(keys::HOSTS_FULL_PERCENT, &json!(0))), [""]);
        assert_eq!(paths(validate(keys::DOCS_MAX_BYTES, &json!(65_535))), [""]);
        assert_eq!(
            paths(validate(keys::INVITATIONS_MAX_USES, &json!(1001))),
            [""]
        );
        assert_eq!(
            paths(validate(keys::SWITCH_VERIFY_TIMEOUT_SECS, &json!(9))),
            [""]
        );
        assert!(
            validate(keys::ENV_IDLE_STOP_MINUTES, &json!(0)).is_ok(),
            "0 means never"
        );
        assert_eq!(
            paths(validate(keys::ENV_IDLE_STOP_MINUTES, &json!(-1))),
            [""]
        );
        assert_eq!(
            paths(validate(keys::STORAGE_MAX_BLOB_BYTES, &json!(-1))),
            [""],
            "no size is less than nothing"
        );
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_refused() {
        assert_eq!(paths(validate(keys::TERMINAL_ENABLED, &json!("yes"))), [""]);
        assert_eq!(paths(validate(keys::THREAD_SIDE_MAX, &json!(2.5))), [""]);
        assert_eq!(paths(validate(keys::THREAD_SIDE_MAX, &json!("4"))), [""]);
        assert_eq!(paths(validate(keys::AGENT_MODEL, &json!(7))), [""]);
        assert_eq!(paths(validate(keys::APPROVAL_MODE, &json!("always"))), [""]);
        assert_eq!(paths(validate(keys::ENV_HOST_ID, &json!("host-1"))), [""]);
        assert!(validate(keys::ENV_HOST_ID, &json!(new_id())).is_ok());
        assert_eq!(
            paths(validate(keys::AGENT_MODEL, &Json::Null)),
            [""],
            "a key is unset by taking its row away"
        );
    }

    #[test]
    fn source_settings_refuse_embedded_credentials_and_invalid_branches_before_preparation() {
        for address in [
            "https://example.invalid/team/repo.git",
            "ssh://git@example.invalid/team/repo.git",
            "git@example.invalid:team/repo.git",
        ] {
            assert!(
                validate(keys::SOURCE_REPOSITORY_URL, &json!(address)).is_ok(),
                "{address}"
            );
        }
        for address in [
            "https://token@example.invalid/repo",
            "https://user:password@example.invalid/repo",
            "https://example.invalid/repo?token=x",
            "/local/repo",
            "http://example.invalid/repo",
        ] {
            assert!(
                validate(keys::SOURCE_REPOSITORY_URL, &json!(address)).is_err(),
                "{address}"
            );
        }
        assert!(validate(keys::SOURCE_BRANCH, &json!("fix/second")).is_ok());
        for branch in ["../main", "-option", "main.lock", "a//b", "a@{b", "HEAD"] {
            assert!(
                validate(keys::SOURCE_BRANCH, &json!(branch)).is_err(),
                "{branch}"
            );
        }
        assert!(validate(keys::WORKSPACE_RECORD_DAYS, &json!(0)).is_ok());
        assert!(validate(keys::WORKSPACE_RECORD_MAX_BYTES, &json!(0)).is_err());
    }

    #[test]
    fn a_fault_inside_a_value_is_reported_with_its_place() {
        let triggers = json!({ "on": ["turn_completed", "sometimes", "turn_completed"], "interval_secs": 0, "often": true });
        assert_eq!(
            paths(validate(keys::AUTO_TRIGGERS, &triggers)),
            [
                "on[1]",
                "on[2]",
                "min_interval_secs",
                "interval_secs",
                "often"
            ]
        );
        let mounts =
            json!({ new_id().to_string(): "private", "scratch": true, new_id().to_string(): "rw" });
        let faults = paths(validate(keys::ENV_MOUNTS, &mounts));
        assert_eq!(faults.len(), 2);
        assert_eq!(faults[0], "scratch");
        let rates = json!({ "gpt-*": { "input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": 15,
                                       "tiers": { "fast": "2x" }, "above": [{ "context_tokens": 272_000, "input_per_mtok": "5" }] } });
        assert_eq!(
            paths(validate(keys::USAGE_RATES, &rates)),
            [
                "gpt-*.output_per_mtok",
                "gpt-*.tiers.fast",
                "gpt-*.above[0].output_per_mtok"
            ]
        );
    }

    #[test]
    fn instructions_are_bounded_scope_by_scope() {
        assert!(validate(keys::AGENT_INSTRUCTIONS, &json!("x".repeat(20_000))).is_ok());
        assert_eq!(
            paths(validate(
                keys::AGENT_INSTRUCTIONS,
                &json!("x".repeat(20_001))
            )),
            [""]
        );
    }

    #[test]
    fn where_a_thread_starts_is_a_path_that_never_goes_up() {
        for good in ["", "src", "services/api", "a.b/..c", "./tools"] {
            assert!(validate(keys::THREAD_CWD, &json!(good)).is_ok(), "{good}");
        }
        for bad in [
            "..",
            "../other",
            "src/../../etc",
            "/etc",
            "a\\..\\b",
            "\\\\server\\share",
            "C:\\Users",
        ] {
            assert_eq!(
                paths(validate(keys::THREAD_CWD, &json!(bad))),
                [""],
                "{bad}"
            );
        }
    }

    #[test]
    fn the_size_of_a_window_or_a_screen_is_width_by_height_within_what_a_screen_can_be() {
        for key in [keys::BROWSER_VIEWPORT, keys::VNC_GEOMETRY] {
            for good in ["1280x800", "320x240", "3840x2160", "1920x1080"] {
                assert!(validate(key, &json!(good)).is_ok(), "{key}: {good}");
            }
            for bad in [
                "1280*800",
                "1280",
                "x800",
                "1280x",
                "319x240",
                "320x239",
                "3841x2160",
                "3840x2161",
                "1280 x 800",
                "1280x800x2",
                "-1280x800",
                "1280.5x800",
                "0x0",
                "1280X800",
                "",
            ] {
                assert_eq!(paths(validate(key, &json!(bad))), [""], "{key}: {bad:?}");
            }
            assert_eq!(
                paths(validate(key, &json!(1280))),
                [""],
                "a size is text, not a number"
            );
        }
        assert_eq!(parse_geometry("1280x800"), Some((1280, 800)));
        assert_eq!(parse_geometry("1280x"), None);
    }

    #[test]
    fn browsers_and_displays_have_a_switch_of_policy_and_values_of_the_environments_own() {
        for key in [keys::BROWSER_ENABLED, keys::VNC_ENABLED] {
            let switch = get(key).unwrap();
            assert_eq!(switch.class, Class::Policy, "{key}");
            assert_eq!(
                switch.written_by(Scope::Topic),
                Some(Writer::OnTheNode(Permission::SettingsManage))
            );
            assert!(!switch.set_at(Scope::Environment), "{key}");
            assert!(validate(key, &json!(true)).is_ok());
            assert!(validate(key, &json!("yes")).is_err());
        }
        for key in [
            keys::BROWSER_PRESET,
            keys::BROWSER_ARGS,
            keys::BROWSER_VIEWPORT,
            keys::BROWSER_IDLE_MINUTES,
            keys::VNC_GEOMETRY,
        ] {
            let own = get(key).unwrap();
            assert_eq!(own.class, Class::Parameter, "{key}");
            assert_eq!(
                own.written_by(Scope::Environment),
                Some(Writer::OnTheEnvironment),
                "{key}: whoever manages the environment sets it for that environment"
            );
        }
        for key in [
            keys::BROWSER_MAX_PER_ENVIRONMENT,
            keys::VNC_MAX_PER_ENVIRONMENT,
        ] {
            assert!(validate(key, &json!(1)).is_ok(), "{key}");
            assert!(
                validate(key, &json!(0)).is_err(),
                "{key}: nothing could be opened at none"
            );
        }
        assert!(validate(keys::BROWSER_PRESET, &json!("window")).is_ok());
        assert!(validate(keys::BROWSER_PRESET, &json!("visible")).is_err());
        assert!(validate(keys::BROWSER_ARGS, &json!(["--lang=de", "--mute-audio"])).is_ok());
        assert!(validate(keys::BROWSER_ARGS, &json!("--lang=de")).is_err());
        assert!(validate(keys::BROWSER_IDLE_MINUTES, &json!(0)).is_ok());
        assert!(validate(keys::BROWSER_IDLE_MINUTES, &json!(-1)).is_err());
    }

    #[test]
    fn the_name_of_a_secret_is_capitals_digits_and_underscores() {
        assert!(validate(keys::ENV_SECRET_NAMES, &json!(["API_KEY", "_TOKEN2"])).is_ok());
        assert_eq!(
            paths(validate(
                keys::ENV_SECRET_NAMES,
                &json!(["api_key", "2FA", "A-B", ""])
            )),
            ["[0]", "[1]", "[2]", "[3]"]
        );
        assert_eq!(
            paths(validate(keys::ENV_SECRET_NAMES, &json!(["A", "A"]))),
            ["[1]"],
            "each name once"
        );
        assert_eq!(
            paths(validate(keys::ENV_SECRET_NAMES, &json!(["A".repeat(65)]))),
            ["[0]"]
        );
    }

    #[test]
    fn rates_and_the_scale_are_decimals_written_as_text() {
        assert!(validate(keys::USAGE_CATALOGUE_SCALE, &json!("0.92")).is_ok());
        for bad in [
            json!("0"),
            json!("0.00"),
            json!("-1"),
            json!("1e3"),
            json!(".5"),
            json!("1."),
            json!(1),
        ] {
            assert_eq!(
                paths(validate(keys::USAGE_CATALOGUE_SCALE, &bad)),
                [""],
                "{bad}"
            );
        }
        assert!(validate(keys::USAGE_CATALOGUE_URL, &Json::Null).is_ok());
        assert!(
            validate(
                keys::USAGE_CATALOGUE_URL,
                &json!("https://rates.example/catalogue.json")
            )
            .is_ok()
        );
        assert_eq!(
            paths(validate(
                keys::USAGE_CATALOGUE_URL,
                &json!("http://rates.example/")
            )),
            [""]
        );
    }

    #[test]
    fn the_message_setting_has_one_active_name_and_rejects_new_uses_of_history_names() {
        assert!(
            validate_at(
                keys::ORCH_TOOLS_ENABLED,
                Scope::Topic,
                &json!(["send_message"])
            )
            .is_ok()
        );
        for old in ["send_chat_message", "send_thread_message"] {
            assert!(validate_at(keys::ORCH_TOOLS_ENABLED, Scope::Topic, &json!([old])).is_err());
        }
        let shipped = get(keys::ORCH_TOOLS_ENABLED)
            .unwrap()
            .default_value()
            .unwrap();
        assert!(shipped.as_array().unwrap().contains(&json!("send_message")));
        for old in ["send_chat_message", "send_thread_message"] {
            assert!(!shipped.as_array().unwrap().contains(&json!(old)));
        }
    }

    #[test]
    fn the_banner_is_a_message_with_its_level_or_nothing() {
        assert!(validate(keys::SYSTEM_BANNER, &Json::Null).is_ok());
        let until = json!({ "text": "Maintenance at six", "level": "warning", "until": "2026-10-08T18:00:00.000Z" });
        assert!(validate(keys::SYSTEM_BANNER, &until).is_ok());
        assert!(
            validate(
                keys::SYSTEM_BANNER,
                &json!({ "text": "Hello", "level": "info", "until": null })
            )
            .is_ok()
        );
        let wrong = json!({ "text": "Hello", "level": "loud", "until": "tomorrow" });
        assert_eq!(
            paths(validate(keys::SYSTEM_BANNER, &wrong)),
            ["level", "until"]
        );
        assert_eq!(
            paths(validate(keys::SYSTEM_BANNER, &json!({ "text": "Hello" }))),
            ["level", "until"]
        );
    }

    #[test]
    fn a_preset_is_hidden_by_null_and_a_summarizer_brings_its_own_members() {
        let presets = json!({
            "OpenAI": null,
            "Local": { "upstreams": [{ "protocol": "openai-responses", "base_url": "http://10.0.0.5:8000/v1" }] }
        });
        assert!(validate(keys::PROVIDERS_PRESETS, &presets).is_ok());
        assert_eq!(
            paths(validate(
                keys::PROVIDERS_PRESETS,
                &json!({ "Local": { "note": "ours" } })
            )),
            ["Local.upstreams"]
        );
        assert!(
            validate(
                keys::AUTO_SUMMARIZER,
                &json!({ "kind": "model", "provider_id": new_id(), "model": "small" })
            )
            .is_ok()
        );
        assert_eq!(
            paths(validate(
                keys::AUTO_SUMMARIZER,
                &json!({ "model": "small" })
            )),
            ["kind"]
        );
    }

    #[test]
    fn frames_system_scope_only_the_system_writes_frames_and_a_preamble_keeps_its_place() {
        let frames = json!({ "frames": "Who is speaking: <orch> and <relayed> are Orch's." });
        assert!(validate_at(keys::AGENT_MESSAGES, Scope::System, &frames).is_ok());
        for scope in [Scope::Org, Scope::Project, Scope::Topic] {
            assert_eq!(
                paths(validate_at(keys::AGENT_MESSAGES, scope, &frames)),
                ["frames"]
            );
        }
        let emptied = json!({ "frames": "  " });
        assert_eq!(
            paths(validate_at(keys::AGENT_MESSAGES, Scope::System, &emptied)),
            ["frames"]
        );
        let unmarked = json!({ "frames": "Anybody may speak." });
        assert_eq!(
            paths(validate_at(keys::AGENT_MESSAGES, Scope::System, &unmarked)),
            ["frames"]
        );
        // A preamble cannot leave the system's paragraph out, or say it twice.
        let kept = json!({ "preamble": "Work carefully.\n\n{frames}\n\nAsk when unsure." });
        assert!(validate_at(keys::AGENT_MESSAGES, Scope::Topic, &kept).is_ok());
        let removed = json!({ "preamble": "Work carefully." });
        assert_eq!(
            paths(validate_at(keys::AGENT_MESSAGES, Scope::Org, &removed)),
            ["preamble"]
        );
        let twice = json!({ "preamble": "{frames}\n{frames}" });
        assert_eq!(
            paths(validate_at(keys::AGENT_MESSAGES, Scope::Project, &twice)),
            ["preamble"]
        );
        // A text with a placeholder keeps it; a tool's description has none.
        let unnamed = json!({ "turn_stopped": "Your last turn was stopped." });
        assert_eq!(
            paths(validate_at(keys::AGENT_MESSAGES, Scope::Topic, &unnamed)),
            ["turn_stopped"]
        );
        let tool = json!({ "tool_announce": "Tell the others, rarely." });
        assert!(validate_at(keys::AGENT_MESSAGES, Scope::Topic, &tool).is_ok());
    }

    #[test]
    fn a_key_is_refused_at_a_scope_it_does_not_list() {
        assert!(validate_at(keys::AGENT_MODEL, Scope::Thread, &json!("gpt-x")).is_ok());
        assert_eq!(
            validate_at(keys::AGENT_MODEL, Scope::System, &json!("gpt-x")),
            Err(Invalid::WrongScope {
                key: "agent.model",
                scope: Scope::System
            })
        );
        assert!(validate_at(keys::HOSTS_FULL_PERCENT, Scope::Org, &json!(50)).is_err());
        assert!(validate_at(keys::EXEC_OFFERS, Scope::Topic, &json!([])).is_err());
        assert!(
            validate_at(
                keys::EXEC_OFFERS,
                Scope::Environment,
                &json!(["codex-exec-server", "plain"])
            )
            .is_ok()
        );
    }

    #[test]
    fn a_parameter_is_written_by_whoever_manages_the_thing_it_is_set_on() {
        let model = get(keys::AGENT_MODEL).unwrap();
        assert_eq!(
            model.written_by(Scope::Thread),
            Some(Writer::OnTheNode(Permission::ThreadManage))
        );
        assert_eq!(
            model.written_by(Scope::Topic),
            Some(Writer::OnTheNode(Permission::TopicManage))
        );
        assert_eq!(
            model.written_by(Scope::Project),
            Some(Writer::OnTheNode(Permission::ProjectManage))
        );
        assert_eq!(
            model.written_by(Scope::Org),
            Some(Writer::OnTheNode(Permission::OrgManage))
        );
        assert_eq!(
            model.written_by(Scope::System),
            None,
            "it is not set at the system"
        );
        let update = get(keys::ENV_UPDATE).unwrap();
        assert_eq!(
            update.written_by(Scope::Environment),
            Some(Writer::OnTheEnvironment)
        );
        assert_eq!(
            Writer::OnTheEnvironment.permission(),
            Some(Permission::EnvironmentManage)
        );
        let runtime = get(keys::RUNTIME_UPDATE).unwrap();
        assert_eq!(
            runtime
                .written_by(Scope::Provider)
                .and_then(Writer::permission),
            Some(Permission::ProviderManage)
        );
        assert_eq!(runtime.written_by(Scope::System), Some(Writer::SystemAdmin));
    }

    #[test]
    fn commands_after_start_need_settings_authority_at_scopes_and_validate_literal_argv_and_deadlines()
     {
        for key in [keys::ENV_AFTER_UP, keys::RUNTIME_AFTER_UP] {
            let def = get(key).unwrap();
            assert_eq!(
                def.written_by(Scope::Org),
                Some(Writer::OnTheNode(Permission::SettingsManage))
            );
            assert_eq!(def.written_by(Scope::System), Some(Writer::SystemAdmin));
            assert!(
                validate_at(
                    key,
                    Scope::Org,
                    &json!({"10-tool":{"command":["tool","literal a b"]},"20-inherited":null})
                )
                .is_ok()
            );
            for bad in [
                json!({"x":{"command":[]}}),
                json!({"x":{"command":["tool"],"timeout_secs":0}}),
                json!({"x":{"command":["tool"],"timeout_secs":3601}}),
                json!({"x":{"command":["tool"],"tolerate":true}}),
            ] {
                assert!(validate_at(key, Scope::Org, &bad).is_err());
            }
        }
        for key in [keys::RUNTIME_STORAGE, keys::RUNTIME_NEEDS] {
            assert!(get(key).unwrap().set_at(Scope::Provider));
            assert!(!get(key).unwrap().set_at(Scope::Environment));
        }
    }

    #[test]
    fn where_a_thread_runs_is_written_by_whoever_manages_its_environment() {
        for key in [keys::THREAD_CWD, keys::SANDBOX_MODE] {
            let def = get(key).unwrap();
            assert_eq!(
                def.written_by(Scope::Thread),
                Some(Writer::OnTheThreadsEnvironment),
                "{key}"
            );
            assert_eq!(
                Writer::OnTheThreadsEnvironment.permission(),
                Some(Permission::EnvironmentManage)
            );
        }
        let sandbox = get(keys::SANDBOX_MODE).unwrap();
        assert_eq!(
            sandbox.written_by(Scope::Topic),
            Some(Writer::OnTheNode(Permission::TopicManage))
        );
        let about_it: Vec<_> = all()
            .iter()
            .filter(|def| def.is_about_where_a_thread_runs())
            .map(|def| def.key)
            .collect();
        assert_eq!(about_it, ["thread.cwd", "sandbox.mode"]);
    }

    #[test]
    fn a_policy_is_written_by_whoever_makes_the_rules_and_the_system_by_its_admins() {
        let shells = get(keys::TERMINAL_ENABLED).unwrap();
        for scope in [Scope::Org, Scope::Project, Scope::Topic] {
            assert_eq!(
                shells.written_by(scope),
                Some(Writer::OnTheNode(Permission::SettingsManage))
            );
        }
        assert_eq!(shells.written_by(Scope::System), Some(Writer::SystemAdmin));
        let retention = get(keys::RETENTION_DELETE_ARCHIVED_DAYS).unwrap();
        assert_eq!(
            retention.written_by(Scope::Thread),
            Some(Writer::OnTheNode(Permission::SettingsManage))
        );
        assert_eq!(
            get(keys::ENV_REMOVED_KEEP_DAYS)
                .unwrap()
                .written_by(Scope::Environment),
            Some(Writer::OnTheEnvironment)
        );
        for def in all().iter().filter(|def| {
            def.class == Class::Policy
                && ![
                    keys::ENV_REMOVED_KEEP_DAYS,
                    keys::NETWORK_ALLOWED_HOSTS,
                    keys::AGENTS_HARNESS_VERSIONS,
                ]
                .contains(&def.key)
        }) {
            assert!(
                !def.set_at(Scope::Environment) && !def.set_at(Scope::Provider),
                "{}",
                def.key
            );
        }
        let versions = get(keys::AGENTS_HARNESS_VERSIONS).unwrap();
        assert_eq!(
            versions.written_by(Scope::Environment),
            Some(Writer::OnTheEnvironment)
        );
        assert!(!versions.set_at(Scope::Provider));
        for scope in [Scope::Org, Scope::Project] {
            assert_eq!(
                versions.written_by(scope),
                Some(Writer::OnTheNode(Permission::SettingsManage))
            );
        }
        assert_eq!(
            versions.written_by(Scope::System),
            Some(Writer::SystemAdmin)
        );
        let kept = get(keys::ENV_REMOVED_KEEP_DAYS).unwrap();
        let network = get(keys::NETWORK_ALLOWED_HOSTS).unwrap();
        assert_eq!(
            network.written_by(Scope::Environment),
            Some(Writer::OnTheEnvironment)
        );
        assert_eq!(
            network.written_by(Scope::Provider),
            Some(Writer::OnTheProvider)
        );
        assert_eq!(
            kept.written_by(Scope::Environment),
            Some(Writer::OnTheEnvironment)
        );
        assert_eq!(
            kept.written_by(Scope::Org),
            Some(Writer::OnTheNode(Permission::SettingsManage))
        );
    }

    #[test]
    fn a_lock_is_set_by_whoever_makes_the_rules_whatever_the_class_of_the_key() {
        assert_eq!(locked_by(Scope::System), Some(Writer::SystemAdmin));
        assert_eq!(
            locked_by(Scope::Topic),
            Some(Writer::OnTheNode(Permission::SettingsManage))
        );
        assert_eq!(locked_by(Scope::Environment), None);
        let model = get(keys::AGENT_MODEL).unwrap();
        assert!(model.lockable_at(Scope::Org) && model.lockable_at(Scope::Topic));
        assert!(
            !model.lockable_at(Scope::Thread),
            "nothing lies below a thread"
        );
        assert!(!model.lockable_at(Scope::System), "it is not set there");
        assert!(
            get(keys::ENV_UPDATE).unwrap().lockable_at(Scope::Topic),
            "an environment's own value lies below"
        );
        assert!(
            !get(keys::MODELS_ALLOWED).unwrap().lockable(),
            "lower scopes can only narrow it"
        );
        assert!(
            !get(keys::HOSTS_FULL_PERCENT).unwrap().lockable(),
            "it is set at the system alone"
        );
        assert!(!get(keys::EXEC_OFFERS).unwrap().lockable());
        assert!(
            get(keys::CHAT_DIRECT).unwrap().lockable_at(Scope::Org),
            "orgs lie inside orgs"
        );
    }

    #[test]
    fn the_registry_is_served_as_data_for_the_screens() {
        let written = serde_json::to_value(get(keys::THREAD_SIDE_IDLE_HOURS).unwrap()).unwrap();
        assert_eq!(
            written,
            json!({
                "key": "thread.side_idle_hours",
                "example": 24,
                "title": "Side thread idle time",
                "description": "After how many hours without a turn an open side thread is closed.",
                "group": "collaboration",
                "schema": { "type": "integer", "minimum": 1, "x-unit": "hours" },
                "class": "policy",
                "scopes": ["system", "org", "project", "topic"],
                "merge": "replace",
                "default": 24
            })
        );
        let model = serde_json::to_value(get(keys::AGENT_MODEL).unwrap()).unwrap();
        assert_eq!(model["default"], Json::Null);
        assert_eq!(model["schema"], json!({ "type": "string" }));
        let allowed = serde_json::to_value(get(keys::APPROVAL_ALLOWED_MODES).unwrap()).unwrap();
        assert_eq!(
            allowed["schema"],
            json!({ "type": "array", "uniqueItems": true, "items": { "type": "string", "enum": ["ask", "on_request", "auto", "bypass"] } })
        );
        assert_eq!(
            serde_json::to_value(all())
                .unwrap()
                .as_array()
                .map(Vec::len),
            Some(all().len())
        );
    }
}
