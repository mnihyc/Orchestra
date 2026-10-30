//! Role command catalogue, serial keys, results and errors.
//!
//! A command is something the system asks one machine to do. It has an
//! identifier, a kind, a body, a moment after which it is not to be
//! started, and usually a serial key: commands that share a key run one
//! at a time in the order they arrived, and all others run side by side.
//!
//! A machine writes a command down before it says that it has it, and
//! writes the result down before it answers. A command that arrives a
//! second time, after a reconnect or after the system restarted, is
//! therefore answered from what was written and never runs twice.
//!
//! The envelope ([`Cmd`], [`CmdRes`]) can be read without knowing the
//! kind, so that a machine answers a kind it does not know with an error
//! and does not lose the frame. The catalogue ([`CommandKind`],
//! [`Command`], [`CommandResult`]) gives each kind its body, its result,
//! the role it is for, its key, its deadline and whether it moves content
//! beside the session.
//!
//! No stored command holds a secret. Where a body needs one it holds a
//! reference ([`Secret::Ref`], [`TicketSlot::Asked`]), which is replaced
//! at the moment the frame is written: by the value sealed to the machine
//! it goes to, or by a freshly signed ticket.

pub use crate::browser::{
    BrowserAdopt, BrowserAdoptResult, BrowserClose, BrowserOpen, BrowserOpenResult,
    BrowserProfileCommand, BrowserProfileResult, ScreenLook, ScreenLookResult,
};
use crate::events::{EnvironmentFacts, NodeCapacity, NodeStorage};
use crate::foreign;
use crate::policy::LocalPolicy;
use crate::session::{MachineFacts, Role};
use crate::tunnels::TunnelKind;
use orch_types::{Hash, Id, Json, Time};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, VecDeque};

/// Names with values that only whoever they are meant for understands:
/// an adapter's parameters, a driver's spec.
pub type Object = serde_json::Map<String, Json>;

/// How long a machine keeps what it wrote down about a command, to
/// answer the same command again. A day: longer than any command may
/// run, and longer than a system stays down for an update.
pub const JOURNAL_KEEP_HOURS: i64 = 24;
/// How long the system waits for a machine to say that it has a command.
/// Thirty seconds is a hundred round trips of a slow link.
pub const ACK_TIMEOUT_SECS: u32 = 30;
/// How often the system sends a command again when a machine did not say
/// that it has it, before it gives the command up as failed for now.
pub const ACK_RESENDS: u32 = 1;

// ---------------------------------------------------------------- vocabulary

/// What a harness's own sandbox lets the agent's commands do, from the
/// strictest to the widest.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    /// Read; nothing is written and nothing leaves the machine.
    ReadOnly,
    /// Write inside the thread's directory and the mounts it may write.
    Workspace,
    /// Whatever the environment allows: the harness applies no sandbox.
    Open,
}

impl SandboxMode {
    /// The mode as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Workspace => "workspace",
            Self::Open => "open",
        }
    }
}

/// When a harness asks a person before it acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Before every action it does not already trust.
    Ask,
    /// When it judges it necessary, or when its sandbox stops an action.
    OnRequest,
    /// The harness's own reviewer handles eligible actions.
    Auto,
    /// The harness bypasses its permission prompts.
    Bypass,
}

/// One way to place a harness, its tools and its credential for a
/// thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArrangementType {
    /// The harness runs with its provider and reaches its tools in the
    /// environment.
    Separated,
    /// The harness runs in the environment and calls the model through
    /// the forwarder, which adds the key where it is kept.
    KeySeparated,
    /// The harness runs in the environment on a copy of the credential.
    CredentialShared,
    /// The harness runs in the environment on the credential itself,
    /// until it is returned.
    HandedOver,
    /// One machine is both places.
    Together,
}

impl ArrangementType {
    /// The type as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Separated => "separated",
            Self::KeySeparated => "key_separated",
            Self::CredentialShared => "credential_shared",
            Self::HandedOver => "handed_over",
            Self::Together => "together",
        }
    }

    /// Whether whoever keeps the credential has to allow this type. Only
    /// the one in which nothing leaves a machine needs no consent.
    #[must_use]
    pub const fn needs_consent(self) -> bool {
        !matches!(self, Self::Together)
    }
}

/// The value `true`, where a member says by being there that something
/// holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct True;

impl Serialize for True {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("expected true"))
        }
    }
}

impl JsonSchema for True {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "True".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "const": true })
    }
}

/// Some stored content, by its hash and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct BlobRef {
    /// The hash of the content.
    pub hash: Hash,
    /// Its size in bytes.
    pub size: i64,
}

// ---------------------------------------------------------------- secrets

/// A secret value in a command.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum Secret {
    /// As the system stores the command: the identifier of the secret.
    #[serde(rename = "$secret")]
    Ref(Id),
    /// As the command is sent: the value, sealed to the key of the
    /// machine it is addressed to and readable by nobody else.
    #[serde(rename = "$sealed")]
    Sealed(String),
}

/// A value sealed to one machine's key. Whoever relays it passes it on
/// unchanged and cannot read it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Sealed {
    /// The sealed value.
    #[serde(rename = "$sealed")]
    pub value: String,
}

/// What a person gave for one field of a form: plain text, or a secret.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SecretOrText {
    /// A value that is no secret.
    Text(String),
    /// A secret.
    Secret(Secret),
}

/// Where a body holds a ticket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TicketSlot {
    /// As the command is sent: the signed ticket.
    Signed(String),
    /// As the system stores the command: what the ticket is to allow. A
    /// fresh one is signed each time the frame is written.
    Asked(TicketAsk),
}

/// The request for a ticket, in a stored command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TicketAsk {
    /// What the ticket is to allow.
    #[serde(rename = "$ticket")]
    pub ticket: TicketWanted,
}

/// What a ticket that is still to be signed is to allow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TicketWanted {
    /// The kind of tunnel.
    pub kind: TunnelKind,
    /// Whatever else the signer needs to fill in the ticket.
    #[serde(flatten)]
    pub more: Object,
}

impl TicketSlot {
    /// A slot that asks for a ticket of `kind`.
    #[must_use]
    pub fn ask(kind: TunnelKind) -> Self {
        Self::Asked(TicketAsk {
            ticket: TicketWanted {
                kind,
                more: Object::new(),
            },
        })
    }
}

/// A reference found in a stored body, to be replaced when the frame is
/// written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reference<'a> {
    /// A secret, by its identifier: replaced by its value, sealed to the
    /// machine the command goes to.
    Secret(Id),
    /// A ticket that is to be signed: replaced by the ticket.
    Ticket(&'a Object),
}

const SECRET_MEMBER: &str = "$secret";
const SEALED_MEMBER: &str = "$sealed";
const TICKET_MEMBER: &str = "$ticket";

fn reference(value: &Json) -> Option<Reference<'_>> {
    let Json::Object(members) = value else {
        return None;
    };
    if members.len() != 1 {
        return None;
    }
    if let Some(Json::String(id)) = members.get(SECRET_MEMBER) {
        return id.parse().ok().map(Reference::Secret);
    }
    if let Some(Json::Object(wanted)) = members.get(TICKET_MEMBER) {
        return Some(Reference::Ticket(wanted));
    }
    None
}

/// Whether a body still holds a reference, and so is in its stored form
/// and not fit to be sent.
#[must_use]
pub fn has_references(body: &Json) -> bool {
    match body {
        Json::Object(members) => reference(body).is_some() || members.values().any(has_references),
        Json::Array(entries) => entries.iter().any(has_references),
        _ => false,
    }
}

/// Turns a stored body into the body that is sent: every reference in it
/// is replaced by what `resolve` returns for it, which is the sealed
/// value for a secret and the signed ticket for a ticket. The first
/// failure stops the work, and the body is then not to be sent.
pub fn resolve_references<E>(
    body: &mut Json,
    resolve: &mut dyn FnMut(Reference<'_>) -> Result<String, E>,
) -> Result<(), E> {
    let replacement = match reference(body) {
        Some(found @ Reference::Secret(_)) => {
            let mut sealed = Object::new();
            sealed.insert(SEALED_MEMBER.to_owned(), Json::String(resolve(found)?));
            Some(Json::Object(sealed))
        }
        Some(found @ Reference::Ticket(_)) => Some(Json::String(resolve(found)?)),
        None => None,
    };
    if let Some(replacement) = replacement {
        *body = replacement;
        return Ok(());
    }
    match body {
        Json::Object(members) => members
            .values_mut()
            .try_for_each(|value| resolve_references(value, resolve)),
        Json::Array(entries) => entries
            .iter_mut()
            .try_for_each(|value| resolve_references(value, resolve)),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------- envelope

/// A command, as it is sent to a machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Cmd {
    /// The command's identifier: the same every time it is sent.
    pub id: Id,
    /// Its kind.
    #[serde(rename = "k")]
    pub kind: String,
    /// Its serial key; `null` for a command that waits for no other.
    #[serde(default)]
    pub key: Option<String>,
    /// The moment after which it is not to be started.
    pub deadline: Time,
    /// Its body, as its kind defines it.
    #[serde(rename = "b")]
    pub body: Json,
}

/// A machine's word that it wrote a command down and will run it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct CmdAck {
    /// The command.
    pub id: Id,
}

/// The system's wish that a command not be carried out after all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct CmdCancel {
    /// The command.
    pub id: Id,
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "CmdResWire", into = "CmdResWire")]
pub struct CmdRes {
    /// The command.
    pub id: Id,
    /// Its result as its kind defines it, or why it failed.
    pub outcome: Result<Json, CommandError>,
}

/// [`CmdRes`] as it is written: `ok` says which of `b` and `err` is
/// there.
#[derive(Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "CmdRes")]
struct CmdResWire {
    id: Id,
    ok: bool,
    #[serde(rename = "b", default, skip_serializing_if = "Option::is_none")]
    body: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    err: Option<CommandError>,
}

impl From<CmdRes> for CmdResWire {
    fn from(res: CmdRes) -> Self {
        match res.outcome {
            Ok(body) => Self {
                id: res.id,
                ok: true,
                body: Some(body),
                err: None,
            },
            Err(err) => Self {
                id: res.id,
                ok: false,
                body: None,
                err: Some(err),
            },
        }
    }
}

impl TryFrom<CmdResWire> for CmdRes {
    type Error = &'static str;

    fn try_from(wire: CmdResWire) -> Result<Self, Self::Error> {
        let outcome = match (wire.ok, wire.body, wire.err) {
            (true, body, _) => Ok(body.unwrap_or_else(|| Json::Object(Object::new()))),
            (false, _, Some(err)) => Err(err),
            (false, _, None) => return Err("a command that failed says why in `err`"),
        };
        Ok(Self {
            id: wire.id,
            outcome,
        })
    }
}

/// Why a command failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct CommandError {
    /// The stable code: one of [`ErrorCode`] where one applies.
    pub code: String,
    /// A sentence for whoever reads the failure.
    #[serde(default)]
    pub detail: String,
    /// Whether the same command may succeed if it is asked again.
    #[serde(default)]
    pub retryable: bool,
    /// Structured facts captured with the refusal, such as a stale write's current file stat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Json>,
}

impl CommandError {
    /// A failure with `code` that asking again would not change.
    #[must_use]
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code: code.as_str().to_owned(),
            detail: detail.into(),
            retryable: false,
            details: None,
        }
    }

    /// The same failure, marked as worth asking again.
    #[must_use]
    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    /// Carries safe structured facts without replacing the stable code and human-readable detail.
    #[must_use]
    pub fn with_details(mut self, details: Json) -> Self {
        self.details = Some(details);
        self
    }

    /// The code, when it is one of the catalogue's.
    #[must_use]
    pub fn known(&self) -> Option<ErrorCode> {
        self.code.parse().ok()
    }
}

macro_rules! error_codes {
    ($( $(#[$doc:meta])* $variant:ident = $code:literal; )*) => {
        /// The codes a machine answers with where they apply.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        pub enum ErrorCode {
            $( $(#[$doc])* #[serde(rename = $code)] $variant, )*
        }

        impl ErrorCode {
            /// Every code.
            pub const ALL: &'static [ErrorCode] = &[ $( ErrorCode::$variant, )* ];

            /// The code as it is written.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $code, )* }
            }
        }

        impl std::str::FromStr for ErrorCode {
            type Err = UnknownCode;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $( $code => Ok(Self::$variant), )*
                    _ => Err(UnknownCode),
                }
            }
        }
    };
}

/// A code the catalogue does not list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a code of the command catalogue")]
pub struct UnknownCode;

error_codes! {
    /// The machine, its adapter or its platform cannot do that.
    Unsupported = "unsupported";
    /// What the command names does not exist there.
    NotFound = "not_found";
    /// The harness no longer holds that conversation.
    SessionNotFound = "session_not_found";
    /// No adapter or no program for that harness kind on the machine.
    HarnessUnavailable = "harness_unavailable";
    /// The harness failed.
    HarnessError = "harness_error";
    /// The harness is not signed in.
    LoginRequired = "login_required";
    /// The workspace holds a configuration of the harness's own that nobody has answered for yet: the people
    /// who manage the environment are asked whether the harness may use it.
    WorkspaceConfiguration = "workspace_configuration";
    /// The machine's owner does not offer that directory.
    PolicyRootNotAllowed = "policy.root_not_allowed";
    /// The machine did not permit sign-in changes in a person's own harness home.
    PolicySignInNotAllowed = "policy.signin_not_allowed";
    /// The machine's owner does not let an agent work without asking.
    PolicyAutoApprovalNotAllowed = "policy.auto_approval_not_allowed";
    /// The machine's owner does not let model calls go to that address.
    PolicyUpstreamNotAllowed = "policy.upstream_not_allowed";
    /// The machine's owner does not let its credential be used that way.
    PolicyArrangementNotAllowed = "policy.arrangement_not_allowed";
    /// The machine's owner accepts no files written from outside.
    PolicyRemoteWriteDisabled = "policy.remote_write_disabled";
    /// The machine's owner accepts no files that arrive with a message.
    PolicyDeliverFilesDisabled = "policy.deliver_files_disabled";
    /// The machine's owner does not let its exec node offer that.
    PolicyExecNotOffered = "policy.exec_not_offered";
    /// The machine's owner lets no tool server be imported from it.
    PolicyMcpImportDisabled = "policy.mcp_import_disabled";
    /// The machine's owner lets no tool server be written from Orch for it.
    PolicyMcpAddDisabled = "policy.mcp_add_disabled";
    /// The file is no longer the version that was expected.
    Changed = "changed";
    /// An interrupted file apply must finish or roll back before its environment is unlocked.
    RestoreRecoveryRequired = "restore.recovery_required";
    /// The content is larger than may be stored.
    TooLarge = "too_large";
    /// The path is left out by the workspace's ignore rules.
    Ignored = "ignored";
    /// What is there is not what the command expected to find.
    Conflict = "conflict";
    /// A live operation holds the resource; a later attempt may proceed.
    Busy = "busy";
    /// The lease the request ran under is closed.
    LeaseClosed = "lease_closed";
    /// The harness cannot run on the credential it was given.
    CredentialUnusable = "credential_unusable";
    /// The machine at the other end of a tunnel is not connected.
    OfflinePeer = "offline_peer";
    /// It took longer than it may.
    Timeout = "timeout";
    /// The machine itself failed.
    Internal = "internal";
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------- catalogue

/// The part of a machine a command is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Addressee {
    /// Any machine.
    Any,
    /// Any machine that a person connected: one the system made has
    /// nothing of the sort.
    AnyConnected,
    /// A machine with this role.
    Role(Role),
    /// A machine with either role.
    Either(Role, Role),
    /// A command hosted by whichever of these declared roles owns its runtime context.
    Roles(&'static [Role]),
}

impl Addressee {
    /// Whether a machine with `roles` takes the command. `connected`
    /// says that a person connected it, and the system did not make it.
    #[must_use]
    pub fn serves(self, roles: &[Role], connected: bool) -> bool {
        match self {
            Self::Any => true,
            Self::AnyConnected => connected || roles.contains(&Role::Node),
            Self::Role(role) => roles.contains(&role),
            Self::Either(one, other) => roles.contains(&one) || roles.contains(&other),
            Self::Roles(offered) => offered.iter().any(|role| roles.contains(role)),
        }
    }
}

/// What a command's serial key names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyKind {
    /// Nothing: the command waits for no other.
    None,
    /// The machine as a whole.
    Machine,
    /// A thread.
    Thread,
    /// A harness process: a provider with an agent profile.
    Harness,
    /// A provider.
    Provider,
    /// An environment.
    Environment,
    /// A runtime of a host.
    Runtime,
    /// A volume of a host.
    Volume,
    /// What a unit's file system is made from.
    Image,
    /// A tool in a host's cache.
    Tool,
}

/// A command's serial key. A machine only ever compares keys: two
/// commands with the same key never run at once.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SerialKey {
    /// No key.
    None,
    /// The machine as a whole.
    Machine,
    /// A thread.
    Thread(Id),
    /// A harness process.
    Harness {
        /// Its provider.
        provider_id: Id,
        /// Its agent profile.
        agent_id: Id,
    },
    /// A provider.
    Provider(Id),
    /// An environment.
    Environment(Id),
    /// A runtime.
    Runtime(Id),
    /// A volume, by its name.
    Volume(String),
    /// A source of a unit's file system, by whatever names it on its
    /// host.
    Image(String),
    /// A tool, by its name.
    Tool(String),
}

impl SerialKey {
    /// What the key names.
    #[must_use]
    pub const fn kind(&self) -> KeyKind {
        match self {
            Self::None => KeyKind::None,
            Self::Machine => KeyKind::Machine,
            Self::Thread(_) => KeyKind::Thread,
            Self::Harness { .. } => KeyKind::Harness,
            Self::Provider(_) => KeyKind::Provider,
            Self::Environment(_) => KeyKind::Environment,
            Self::Runtime(_) => KeyKind::Runtime,
            Self::Volume(_) => KeyKind::Volume,
            Self::Image(_) => KeyKind::Image,
            Self::Tool(_) => KeyKind::Tool,
        }
    }

    /// The key as it is written in a command: an identifier by itself
    /// where the key is one thing with an identifier, and otherwise a
    /// text that no identifier can equal.
    #[must_use]
    pub fn wire(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Machine => Some("machine".to_owned()),
            Self::Thread(id) | Self::Provider(id) | Self::Environment(id) | Self::Runtime(id) => {
                Some(id.to_string())
            }
            Self::Harness {
                provider_id,
                agent_id,
            } => Some(format!("{provider_id}:{agent_id}")),
            Self::Volume(name) => Some(format!("volume:{name}")),
            Self::Image(name) => Some(format!("image:{name}")),
            Self::Tool(name) => Some(format!("tool:{name}")),
        }
    }
}

/// Whether a command moves content beside the session, and which way.
/// File fetch, snapshot and archive reads upload from the machine; writes, materialization and
/// image build contexts download to it. The command catalogue is the authority for this direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transfer {
    /// It moves none.
    None,
    /// The machine sends content to the system.
    Upload,
    /// The machine fetches content from the system.
    Download,
}

/// A body of the catalogue: it knows the kind it is sent as, and what
/// answers it.
pub trait CommandBody: Serialize + DeserializeOwned {
    /// The kind it is sent as.
    const KIND: CommandKind;
    /// What a machine answers with when the command succeeded.
    type Result: Serialize + DeserializeOwned;
}

/// Why a kind and a body are not a command or an event of the catalogue.
#[derive(Debug, thiserror::Error)]
pub enum CatalogueError {
    /// The kind is not in the catalogue.
    #[error("`{0}` is not a kind of the catalogue")]
    UnknownKind(String),
    /// The body is not what the kind defines.
    #[error("the body of `{kind}` is not what that kind defines: {source}")]
    Body {
        /// The kind.
        kind: &'static str,
        /// What is wrong with the body.
        source: serde_json::Error,
    },
}

const ANY: Addressee = Addressee::Any;
const CONNECTED: Addressee = Addressee::AnyConnected;
const AGENT: Addressee = Addressee::Role(Role::Agent);
const PROVIDER: Addressee = Addressee::Role(Role::Provider);
const AGENT_OR_PROVIDER: Addressee = Addressee::Either(Role::Agent, Role::Provider);
const ENVIRONMENT: Addressee = Addressee::Role(Role::Environment);
const AFTER_UP_HOST: Addressee =
    Addressee::Roles(&[Role::Environment, Role::Provider, Role::Agent]);
const NODE: Addressee = Addressee::Role(Role::Node);
/// The machine's declared data maker: its node, or a directory machine's environment role (`directory` volumes).
const DATA_MAKER: Addressee = Addressee::Roles(&[Role::Node, Role::Environment]);
const PORT_PUBLISHER: Addressee = Addressee::Roles(&[Role::Node, Role::Environment]);

macro_rules! commands {
    ($(
        $(#[$doc:meta])*
        $variant:ident = $kind:literal: $addressee:ident, $key:ident, $secs:literal, $transfer:ident, $body:ty => $result:ty;
    )*) => {
        /// Every kind of command.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        pub enum CommandKind {
            $( $(#[$doc])* #[serde(rename = $kind)] $variant, )*
        }

        impl CommandKind {
            /// Every kind, in the order of the catalogue.
            pub const ALL: &'static [CommandKind] = &[ $( CommandKind::$variant, )* ];

            /// The kind as it is written.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $kind, )* }
            }

            /// The part of a machine it is for.
            #[must_use]
            pub const fn addressee(self) -> Addressee {
                match self { $( Self::$variant => $addressee, )* }
            }

            /// What its serial key names.
            #[must_use]
            pub const fn key(self) -> KeyKind {
                match self { $( Self::$variant => KeyKind::$key, )* }
            }

            /// How many seconds after it was made it is no longer to be
            /// started, and counts as expired without an answer.
            #[must_use]
            pub const fn deadline_secs(self) -> u32 {
                match self { $( Self::$variant => $secs, )* }
            }

            /// Whether it moves content beside the session.
            #[must_use]
            pub const fn transfer(self) -> Transfer {
                match self { $( Self::$variant => Transfer::$transfer, )* }
            }
        }

        impl std::str::FromStr for CommandKind {
            type Err = CatalogueError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $( $kind => Ok(Self::$variant), )*
                    _ => Err(CatalogueError::UnknownKind(text.to_owned())),
                }
            }
        }

        /// A command of the catalogue: its kind with its body.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(tag = "k", content = "b")]
        #[allow(
            clippy::large_enum_variant,
            reason = "a command is made once and sent; boxing its largest body would cost every reader an indirection"
        )]
        pub enum Command {
            $( $(#[$doc])* #[serde(rename = $kind)] $variant($body), )*
        }

        impl Command {
            /// Its kind.
            #[must_use]
            pub const fn kind(&self) -> CommandKind {
                match self { $( Self::$variant(_) => CommandKind::$variant, )* }
            }

            /// Its body, as it is written.
            pub fn body(&self) -> Result<Json, serde_json::Error> {
                match self { $( Self::$variant(body) => serde_json::to_value(body), )* }
            }

            /// The command of `kind` with `body`.
            pub fn decode(kind: &str, body: Json) -> Result<Self, CatalogueError> {
                match kind.parse::<CommandKind>()? {
                    $( CommandKind::$variant => serde_json::from_value::<$body>(body)
                        .map(Self::$variant)
                        .map_err(|source| CatalogueError::Body { kind: $kind, source }), )*
                }
            }
        }

        /// What a command of each kind answers with when it succeeded.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(tag = "k", content = "b")]
        #[allow(
            clippy::large_enum_variant,
            reason = "a result is read once and handed on; boxing its largest body would cost every reader an indirection"
        )]
        pub enum CommandResult {
            $( $(#[$doc])* #[serde(rename = $kind)] $variant($result), )*
        }

        impl CommandResult {
            /// The kind of command it answers.
            #[must_use]
            pub const fn kind(&self) -> CommandKind {
                match self { $( Self::$variant(_) => CommandKind::$variant, )* }
            }

            /// The result, as it is written.
            pub fn body(&self) -> Result<Json, serde_json::Error> {
                match self { $( Self::$variant(result) => serde_json::to_value(result), )* }
            }

            /// The result of a command of `kind`, from the body of its
            /// answer.
            pub fn decode(kind: CommandKind, body: Json) -> Result<Self, CatalogueError> {
                match kind {
                    $( CommandKind::$variant => serde_json::from_value::<$result>(body)
                        .map(Self::$variant)
                        .map_err(|source| CatalogueError::Body { kind: $kind, source }), )*
                }
            }
        }

        $(
            impl CommandBody for $body {
                const KIND: CommandKind = CommandKind::$variant;
                type Result = $result;
            }

            impl From<$body> for Command {
                fn from(body: $body) -> Self {
                    Self::$variant(body)
                }
            }
        )*
    };
}

// Each row: the kind, the part of a machine it is for, what its serial key
// names, the seconds after which it is no longer started, whether it moves
// content beside the session, and its body with its result.
pub use crate::browser_window::{DisplayOpen, DisplayOpenResult};
pub use crate::display::{DisplayAdopt, DisplayAdoptResult, DisplayClose};
pub use orch_api_contract::machine_programs::MachineProgramsCommand;

commands! {
    /// Reads what a machine is and has.
    MachineProbe = "machine.probe": ANY, Machine, 30, None, MachineProbe => MachineFacts;
    /// Installs the recommended verified build after the server has fenced and drained hosted turns.
    /// Local update.auto is checked again on receipt and immediately before installation.
    MachineUpdateNow = "machine.update.now": ANY, Machine, 30, None, MachineUpdateNow => MachineUpdateNowResult;
    /// Changes keys of a machine's local policy that its owner handed over.
    MachinePolicySet = "machine.policy.set": CONNECTED, Machine, 30, None, MachinePolicySet => MachinePolicySetResult;
    /// Changes or probes locally supervised programs, within the existing provider startup budget.
    MachinePrograms = "machine.programs": ANY, Machine, 120, None, MachineProgramsCommand => MachineFacts;
    /// Saves delegated connector roles, then reconnects through the native supervisor.
    MachineRolesSet = "machine.roles.set": ANY, Machine, 30, None, MachineRolesSet => MachineRolesSetResult;
    /// Runs one named command after a managed unit starts, keeping its result for this run.
    /// Its hour-long maximum plus thirty seconds allows bounded drain and durable result storage.
    UnitAfterUp = "unit.after_up": AFTER_UP_HOST, Runtime, 3630, None, UnitAfterUp => UnitAfterUpResult;
    /// Starts a shell of one person's own in an environment.
    PtyOpen = "pty.open": ENVIRONMENT, None, 30, None, PtyOpen => PtyOpenResult;
    /// Looks again for installed browser and display programs and the processes still known.
    ScreenLook = "screen.look": ENVIRONMENT, None, 30, None, ScreenLook => ScreenLookResult;
    /// Opens or joins the browser of a named profile.
    BrowserOpen = "browser.open": ENVIRONMENT, Environment, 60, None, BrowserOpen => BrowserOpenResult;
    /// Takes in a browser an agent started.
    BrowserAdopt = "browser.adopt": ENVIRONMENT, Environment, 15, None, BrowserAdopt => BrowserAdoptResult;
    /// Asks a browser to quit, stopping only a process Orch started.
    BrowserClose = "browser.close": ENVIRONMENT, Environment, 20, None, BrowserClose => Empty;
    /// Makes, renames or removes a kept profile by its name.
    BrowserProfile = "browser.profile": ENVIRONMENT, Environment, 60, None, BrowserProfileCommand => BrowserProfileResult;
    /// Opens the virtual screen needed by a browser with a window.
    DisplayOpen = "display.open": ENVIRONMENT, Environment, 30, None, DisplayOpen => DisplayOpenResult;
    /// Takes in a local VNC server using a machine-sealed password where needed.
    DisplayAdopt = "display.adopt": ENVIRONMENT, Environment, 15, None, DisplayAdopt => DisplayAdoptResult;
    /// Stops an owned virtual screen or forgets an announced server.
    DisplayClose = "display.close": ENVIRONMENT, Environment, 15, None, DisplayClose => Empty;
    /// Ends a shell.
    PtyClose = "pty.close": ENVIRONMENT, None, 15, None, PtyClose => PtyCloseResult;
    /// Measures foreground work in selected live shells of an environment.
    PtyStatus = "pty.status": ENVIRONMENT, None, 10, None, PtyStatus => PtyStatusResult;
    /// Makes a harness's own terminal reachable.
    AgentPtyOpen = "agent_pty.open": AGENT, Thread, 30, None, AgentPtyOpen => AgentPtyOpenResult;
    /// Starts a harness process, or attaches to one that runs.
    AgentStart = "agent.start": AGENT, Harness, 120, None, AgentStart => AgentStartResult;
    /// Stops a harness process.
    AgentStop = "agent.stop": AGENT, Harness, 60, None, AgentStop => Empty;
    /// Lists the conversations a harness already holds.
    AgentDiscoverSessions = "agent.discover_sessions": AGENT, Harness, 30, None, AgentDiscoverSessions => AgentDiscoverSessionsResult;
    /// Reads what a harness recorded of a conversation it holds, for its adoption.
    AgentReadSession = "agent.read_session": AGENT, Harness, 300, Upload, AgentReadSession => AgentReadSessionResult;
    /// Opens a thread's session in its harness.
    SessionOpen = "session.open": AGENT, Thread, 120, Download, SessionOpen => SessionOpenResult;
    /// Inspects the exact retained user input without submitting model work.
    SessionPrefixInspect = "session.prefix.inspect": AGENT, Thread, 120, None, crate::continuations::SessionPrefixInspect => crate::continuations::SessionPrefixInspectResult;
    /// Opens the exact inspected prefix through the ordinary session policy.
    SessionPrefixOpen = "session.prefix.open": AGENT, Thread, 120, Download, crate::continuations::SessionPrefixOpen => crate::continuations::SessionPrefixOpenResult;
    /// Observes one retained idle continuation without autonomous model input.
    SessionContinuationResume = "session.continuation.resume": AGENT, Thread, 120, Download, crate::continuations::SessionContinuationResume => crate::continuations::SessionContinuationResumeResult;
    /// Closes only the exact idle retained native lane.
    SessionContinuationClose = "session.continuation.close": AGENT, Thread, 30, None, crate::continuations::SessionContinuationClose => Empty;
    /// Starts work only on the exact prepared segment, through the ordinary turn policy.
    TurnStartFenced = "turn.start.fenced": AGENT, Thread, 60, Download, crate::continuations::TurnStartFenced => Empty;
    /// Ordinary lifecycle close names its captured segment so a late command cannot close a successor.
    SessionCloseFenced = "session.close.fenced": AGENT, Thread, 30, None, crate::continuations::SessionCloseFenced => Empty;
    /// Replaces only the captured idle predecessor, through the ordinary session policy.
    SessionOpenFenced = "session.open.fenced": AGENT, Thread, 120, Download, crate::continuations::SessionOpenFenced => SessionOpenResult;
    /// Closes a thread's session.
    SessionClose = "session.close": AGENT, Thread, 30, None, SessionClose => Empty;
    /// Measures or removes exact Orch-created native sessions in one managed home.
    AgentHistoryManage = "agent.history.manage": AGENT, Harness, 300, None, AgentHistoryManage => AgentHistoryManageResult;
    /// Removes only documented old cache leftovers from a closed managed home.
    AgentHistoryHomeCleanup = "agent.history.home_cleanup": AGENT, Harness, 300, None, AgentHistoryHomeCleanup => AgentHistoryHomeCleanupResult;
    /// Refreshes only an open session's offered MCP set, where its adapter declares live reload.
    SessionMcpReload = "session.mcp.reload": AGENT, Thread, 60, None, SessionMcpReload => Empty;
    /// Reads the live commands of an existing native session; its result is never journalled.
    SessionBackground = "session.background": AGENT, Thread, 15, None, SessionBackground => crate::background::NativeBackground;
    /// Reads one exact session's effective native actions without starting a model turn.
    SessionNativeActions = "session.native_actions": AGENT, Thread, 30, None, SessionNativeActions => orch_agent_contract::native::NativeCatalog;
    /// Ends one exact captured native session command.
    SessionBackgroundEnd = "session.background.end": AGENT, Thread, 30, None, SessionBackgroundEnd => crate::background::NativeBackgroundEnd;
    /// Has the harness of an open session count what fills its context, between turns; its result is never journalled.
    SessionContext = "session.context": AGENT, Thread, 30, None, SessionContext => SessionContextResult;
    /// Writes a session's history out as an archive.
    SessionExport = "session.export": AGENT, Thread, 600, Upload, SessionExport => SessionExportResult;
    /// Has the adapter report again what its harness holds of a session.
    SessionResync = "session.resync": AGENT, Thread, 300, None, SessionResync => SessionResyncResult;
    /// Starts a turn with one input.
    TurnStart = "turn.start": AGENT, Thread, 60, Download, TurnStart => Empty;
    /// Adds an input to the turn that is running.
    TurnSteer = "turn.steer": AGENT, Thread, 30, Download, TurnSteer => Empty;
    /// Stops the turn that is running.
    TurnInterrupt = "turn.interrupt": AGENT, Thread, 30, None, TurnInterrupt => Empty;
    /// Gives a harness the decision it asked for.
    ApprovalResolve = "approval.resolve": AGENT, Thread, 30, None, ApprovalResolve => Empty;
    /// Gives a harness the outcome of a call to one of the system's own tools.
    OrchToolResult = "orch.tool_result": AGENT, Thread, 30, None, OrchToolResult => Empty;
    /// Starts one of a harness's ways of signing in.
    ProviderLoginStart = "provider.login.start": AGENT, Provider, 60, None, ProviderLoginStart => Empty;
    /// Gives a running sign-in what it asked a person for.
    ProviderLoginInput = "provider.login.input": AGENT, Provider, 30, None, ProviderLoginInput => Empty;
    /// Gives a running sign-in up.
    ProviderLoginCancel = "provider.login.cancel": AGENT, Provider, 30, None, ProviderLoginCancel => Empty;
    /// Signs a harness out.
    ProviderLogout = "provider.logout": AGENT, Provider, 30, None, ProviderLogout => Empty;
    /// Has a provider look at its credential and report its state.
    ProviderRefresh = "provider.refresh": AGENT_OR_PROVIDER, Provider, 30, None, ProviderRefresh => ProviderRefreshResult;
    /// Uses one confirmed reset credit with an idempotency key made by the control plane.
    ProviderResetConsume = "provider.reset": AGENT_OR_PROVIDER, Provider, 30, None, ProviderResetConsume => ProviderResetConsumeResult;
    /// Lists the models a provider's harness or endpoint offers.
    ProviderDiscoverModels = "provider.discover_models": AGENT_OR_PROVIDER, Provider, 30, None, ProviderDiscoverModels => ProviderDiscoverModelsResult;
    /// Gives the provider role what it forwards model calls with.
    ProviderStart = "provider.start": PROVIDER, Provider, 60, None, ProviderStart => Empty;
    /// Has the provider role forget a provider.
    ProviderStop = "provider.stop": PROVIDER, Provider, 30, None, ProviderStop => Empty;
    /// Makes one model request on the system's own behalf.
    ProviderComplete = "provider.complete": PROVIDER, None, 60, None, ProviderComplete => ProviderCompleteResult;
    /// Turns texts into vectors on the system's own behalf, to search by meaning. A batch on a processor of a few
    /// cores takes tens of seconds; two minutes leave room for one that the indexer then halves.
    ProviderEmbed = "provider.embed": PROVIDER, None, 120, None, ProviderEmbed => ProviderEmbedResult;
    /// Opens a lease: one turn's leave to use a provider's credential.
    LeaseOpen = "lease.open": PROVIDER, Provider, 15, None, LeaseOpen => Empty;
    /// Produces a credential for a harness that runs elsewhere, sealed to where it runs.
    ProviderCredentialIssue = "provider.credential.issue": PROVIDER, Provider, 30, None, ProviderCredentialIssue => SealedCredential;
    /// Takes back a credential that had been handed over.
    ProviderCredentialAccept = "provider.credential.accept": PROVIDER, Provider, 60, None, ProviderCredentialAccept => Empty;
    /// Has a harness stop using a credential it was handed and give it back.
    AgentCredentialReturn = "agent.credential.return": AGENT, Harness, 60, None, AgentCredentialReturn => SealedCredential;
    /// Gives a harness a credential to run on.
    AgentCredentialSet = "agent.credential.set": AGENT, Harness, 30, None, AgentCredentialSet => Empty;
    /// Closes a lease.
    LeaseClose = "lease.close": PROVIDER, Provider, 15, None, LeaseClose => LeaseCloseResult;
    /// Attaches an environment to its workspace, its mounts and what its commands are to find.
    EnvAttach = "env.attach": ENVIRONMENT, Environment, 60, None, EnvAttach => EnvironmentFacts;
    /// Detaches an environment.
    EnvDetach = "env.detach": ENVIRONMENT, Environment, 30, None, EnvDetach => Empty;
    /// Removes only the factory-recorded owned directories of a detached environment generation.
    EnvDataRemove = "env.data.remove": ENVIRONMENT, Environment, 600, None, EnvDataRemove => Empty;
    /// Starts an executor in an environment.
    ExecStart = "exec.start": ENVIRONMENT, Environment, 120, None, ExecStart => ExecStartResult;
    /// Starts a tool server in an environment.
    McpStart = "mcp.start": ENVIRONMENT, Environment, 60, None, McpStart => McpStartResult;
    /// Restarts one known server with its own crash budget cleared; a new connector restores just that definition.
    McpRestart = "mcp.restart": ENVIRONMENT, Environment, 180, None, McpRestart => McpRestartResult;
    /// Keeps a copy of a tool server that a harness on the machine has configured.
    McpImport = "mcp.import": ENVIRONMENT, Environment, 30, None, McpImport => McpImportResult;
    /// Deletes the copy of an imported tool server.
    McpForget = "mcp.forget": ENVIRONMENT, Machine, 15, None, McpForget => Empty;
    /// Stops a tool server.
    McpStop = "mcp.stop": ENVIRONMENT, Environment, 30, None, McpStop => Empty;
    /// Lists a directory, and may watch it.
    FsList = "fs.list": ENVIRONMENT, None, 15, None, FsList => FsListResult;
    /// Ends the watch on a directory.
    FsUnwatch = "fs.unwatch": ENVIRONMENT, None, 15, None, FsUnwatch => Empty;
    /// Describes one file or directory.
    FsStat = "fs.stat": ENVIRONMENT, None, 15, None, FsStat => foreign::FileStat;
    /// Uploads a file's content, unless the system holds it already.
    FsFetch = "fs.fetch": ENVIRONMENT, None, 600, Upload, FsFetch => FsFetchResult;
    /// Writes a file from stored content.
    FsWrite = "fs.write": ENVIRONMENT, Environment, 600, Download, FsWrite => foreign::FileStat;
    /// Makes a directory.
    FsMkdir = "fs.mkdir": ENVIRONMENT, Environment, 60, None, FsMkdir => foreign::FileStat;
    /// Renames or moves a file or a directory.
    FsRename = "fs.rename": ENVIRONMENT, Environment, 60, None, FsRename => foreign::FileStat;
    /// Removes a file or a directory.
    FsRemove = "fs.remove": ENVIRONMENT, Environment, 60, None, FsRemove => Empty;
    /// Copies a file, or a directory with what is in it.
    FsCopy = "fs.copy": ENVIRONMENT, Environment, 600, None, FsCopy => foreign::FileStat;
    /// Lists what an archive holds, without unpacking it.
    FsArchiveList = "fs.archive.list": ENVIRONMENT, None, 25, None, FsArchiveList => foreign::ArchiveListing;
    /// Uploads one entry of an archive for a viewer.
    FsArchiveRead = "fs.archive.read": ENVIRONMENT, None, 25, Upload, FsArchiveRead => FsArchiveReadResult;
    /// Searches the text of the files under a directory.
    FsSearch = "fs.search": ENVIRONMENT, None, 25, None, FsSearch => FsSearchResult;
    /// Lists a tree as it is now and uploads the list.
    FsManifest = "fs.manifest": ENVIRONMENT, Environment, 1800, Upload, FsManifest => FsManifestResult;
    /// Uploads a tree: its list, and every file the system lacks.
    FsSnapshot = "fs.snapshot": ENVIRONMENT, Environment, 21600, Upload, FsSnapshot => FsSnapshotResult;
    /// Measures selected paths without hashing, uploading or starting a snapshot job.
    FsMeasure = "fs.measure": ENVIRONMENT, Environment, 30, None, FsMeasure => FsMeasureResult;
    /// Writes the files of a list into a directory.
    FsMaterialize = "fs.materialize": ENVIRONMENT, Environment, 21600, Download, FsMaterialize => FsMaterializeResult;
    /// Reads the state of the repository in a workspace.
    GitStatus = "git.status": ENVIRONMENT, None, 15, None, GitStatus => foreign::GitStatus;
    /// Reads what changed in the repository of a workspace.
    GitDiff = "git.diff": ENVIRONMENT, None, 30, None, GitDiff => GitDiffResult;
    /// Prepares the frozen source in a missing child or verified job-owned empty root.
    GitPrepare = "git.prepare": ENVIRONMENT, Environment, 21600, None, crate::source::GitPrepare => crate::source::GitPrepareResult;
    /// Removes only empty, unchanged stages of the original source preparation.
    GitPrepareCleanup = "git.prepare.cleanup": ENVIRONMENT, Environment, 21600, None, crate::source::GitPrepareCleanup => crate::source::GitPrepareCleanupResult;
    /// Creates the planned new branch and its unused worktree directory.
    GitWorktree = "git.worktree": ENVIRONMENT, Environment, 21600, None, crate::source::GitWorktree => crate::source::GitWorktreeResult;
    /// Removes only an unchanged worktree and branch owned by the original job.
    GitWorktreeCleanup = "git.worktree.cleanup": ENVIRONMENT, Environment, 21600, None, crate::source::GitWorktreeCleanup => crate::source::GitWorktreeCleanupResult;
    /// Repairs copied worktree connections before a moved workspace is published.
    GitWorktreeRepair = "git.worktree.repair": ENVIRONMENT, Environment, 21600, Download, crate::source::GitWorktreeRepair => crate::source::GitWorktreeRepairResult;
    /// Measures repository data before a linked worktree is copied out.
    GitWorktreeCopyPlan = "git.worktree.copy_plan": ENVIRONMENT, None, 60, None, crate::source::GitWorktreeCopyPlan => crate::source::GitWorktreeCopyPlanResult;
    /// Copies raw files plus private HEAD/index and complete common objects/refs.
    GitWorktreeSnapshot = "git.worktree.snapshot": ENVIRONMENT, Environment, 21600, Upload, crate::source::GitWorktreeSnapshot => crate::source::GitWorktreeSnapshotResult;
    /// Checks every imported object and ref locally before its session opens.
    GitWorktreeStandalone = "git.worktree.standalone": ENVIRONMENT, Environment, 21600, None, crate::source::GitWorktreeStandalone => Empty;
    /// Commits exactly the reviewed selected paths as the server's caller identity.
    GitCommit = "git.commit": ENVIRONMENT, Environment, 21600, None, crate::source::GitCommit => crate::source::GitCommitReply;
    /// Pushes the reviewed branch without forcing, pulling or redirecting its source credential.
    GitPush = "git.push": ENVIRONMENT, Environment, 21600, None, crate::source::GitPush => crate::source::GitPushResult;
    /// Records complete included files in the environment-owned immutable store.
    FsRecord = "fs.record": ENVIRONMENT, Environment, 21600, Upload, crate::source::FsRecord => crate::source::FsRecordResult;
    /// Reads verified recorded bytes instead of newer live workspace files.
    FsRecordRead = "fs.record.read": ENVIRONMENT, None, 21600, Upload, crate::source::FsRecordRead => crate::source::FsRecordReadResult;
    /// Releases expired file-state roots unless a running change pins them.
    FsRecordPrune = "fs.record.prune": ENVIRONMENT, Environment, 1800, None, crate::source::FsRecordPrune => crate::source::FsRecordPruneResult;
    /// Verifies carried roots, referenced objects and journals before source release.
    FsRecordVerify = "fs.record.verify": ENVIRONMENT, Environment, 21600, None, crate::source::FsRecordVerify => crate::source::FsRecordVerifyResult;
    /// Measures declared data roots before moving to any permitted machine.
    FsDataMeasure = "fs.data.measure": ENVIRONMENT, Environment, 21600, None, crate::source::FsDataMeasure => crate::source::FsDataMeasureResult;
    /// Exports an environment-owned data root through the common content protocol.
    FsDataExport = "fs.data.export": ENVIRONMENT, Environment, 21600, Upload, crate::source::FsDataExport => FsSnapshotResult;
    /// Imports checked data into the candidate environment's fresh generation.
    FsDataImport = "fs.data.import": ENVIRONMENT, Environment, 21600, Download, crate::source::FsDataImport => FsMaterializeResult;
    /// Plans every path effect and fingerprints the whole included current namespace.
    FsRestorePlan = "fs.restore.plan": ENVIRONMENT, Environment, 21600, Upload, crate::source::FsRestorePlan => crate::source::FsRestorePlanResult;
    /// Rechecks and applies one restore with complete undo and a persisted apply journal.
    FsRestore = "fs.restore": ENVIRONMENT, Environment, 21600, Download, crate::source::FsRestore => crate::source::FsRestoreResult;
    /// Observes complete end hashes from watcher changes, without Git or a tree walk.
    WorkspaceObserve = "workspace.observe": ENVIRONMENT, None, 1800, Upload, crate::source::WorkspaceObserve => crate::source::WorkspaceObserveResult;
    /// Lists the processes of an environment.
    ProcList = "proc.list": ENVIRONMENT, None, 15, None, ProcList => ProcListResult;
    /// Requests a fresh port measurement whose contents travel only as a live report.
    PortsLook = "ports.look": ENVIRONMENT, Environment, 15, None, crate::ports::PortsLook => crate::ports::PortsLookResult;
    /// Signals one process.
    ProcSignal = "proc.signal": ENVIRONMENT, Environment, 15, None, ProcSignal => Empty;
    /// Holds still every process the environment's agents started.
    ProcSuspend = "proc.suspend": ENVIRONMENT, Environment, 30, None, ProcSuspend => ProcHeldResult;
    /// Lets the processes that were held still go on.
    ProcResume = "proc.resume": ENVIRONMENT, Environment, 30, None, ProcResume => ProcHeldResult;
    /// Says which measurements of an environment are wanted, and how often.
    WatchSet = "watch.set": ENVIRONMENT, None, 15, None, WatchSet => Empty;
    /// Makes a unit for a runtime.
    NodeRuntimeCreate = "node.runtime.create": NODE, Runtime, 600, None, NodeRuntimeCreate => NodeRuntimeCreateResult;
    /// Starts a runtime's unit.
    NodeRuntimeStart = "node.runtime.start": NODE, Runtime, 120, None, NodeRuntimeStart => Empty;
    /// Stops a runtime's unit.
    NodeRuntimeStop = "node.runtime.stop": NODE, Runtime, 120, None, NodeRuntimeStop => Empty;
    /// Removes a runtime's unit.
    NodeRuntimeRemove = "node.runtime.remove": NODE, Runtime, 120, None, NodeRuntimeRemove => Empty;
    /// Installs a runtime's network policy and retains its requested restriction before acknowledging.
    NodeNetworkApply = "node.network.apply": NODE, Runtime, 30, None, NodeNetworkApply => orch_api_contract::environment::NetworkState;
    /// Applies placed mounts in an existing unit, or keeps them pending for recreate with a reason.
    NodeMountsApply = "node.mounts.apply": NODE, Runtime, 120, None, NodeMountsApply => NodeMountsApplyResult;
    /// Applies prepared environment grants without replacing its unit.
    NodeCapabilitiesApply = "node.capabilities.apply": NODE, Runtime, 120, None, NodeCapabilitiesApply => NodeCapabilitiesApplyResult;
    /// Applies live native limits to the existing unit.
    NodeResourcesApply = "node.resources.apply": NODE, Runtime, 30, None, NodeResourcesApply => Empty;
    /// Reads current limits and use before a person changes a running unit's limits.
    NodeResourcesInspect = "node.resources.inspect": NODE, Runtime, 30, None, NodeResourcesInspect => orch_driver_contract::runtime::RuntimeChanges;
    /// Lists installation-owned objects, dependencies and any unreadable part of the inventory.
    NodeResourcesList = "node.resources.list": NODE, Machine, 120, None, NodeResourcesList => orch_driver_contract::resources::ResourceInventory;
    /// Rechecks and removes exactly one node-inventoried object by kind and opaque identifier.
    NodeResourcesRemove = "node.resources.remove": NODE, Machine, 120, None, NodeResourcesRemove => Empty;
    /// Measures the installed files an unavoidable same-image recreate keeps, without stopping the unit.
    NodeKeepEstimate = "node.runtime.keep-estimate": NODE, Runtime, 30, None, NodeKeepEstimate => orch_driver_contract::kept::KeepEstimate;
    /// Records a stopped unit's installed files as its next generation's owned image.
    NodeKeepFiles = "node.runtime.keep-files": NODE, Runtime, 3600, None, NodeKeepFiles => orch_driver_contract::kept::KeptImage;
    /// Removes one exact owned kept snapshot, including an archived environment with no unit.
    NodeKeptRemove = "node.image.kept-remove": NODE, Image, 120, None, NodeKeptImage => Empty;
    /// Puts up an owned image archive through the existing content transfer.
    NodeKeptExport = "node.image.kept-export": NODE, Image, 3600, Upload, NodeKeptImageExport => BlobRef;
    /// Loads the transferred owned image before a successor starts on this node.
    NodeKeptImport = "node.image.kept-import": NODE, Image, 3600, Download, NodeKeptImport => Empty;
    /// Removes owned images no current unit, archived environment or active job retains.
    NodeKeptSweep = "node.image.kept-sweep": NODE, None, 3600, None, NodeKeptSweep => Empty;
    /// Makes sure a volume exists on a host.
    NodeVolumeEnsure = "node.volume.ensure": DATA_MAKER, Volume, 60, None, NodeVolumeEnsure => Empty;
    /// Checks restored owned storage without creating or adopting it.
    NodeVolumeInspect = "node.volume.inspect": NODE, Volume, 60, None, NodeVolumeInspect => NodeVolumeInspectResult;
    /// Removes a volume from a host.
    NodeVolumeRemove = "node.volume.remove": DATA_MAKER, Volume, 60, None, NodeVolumeRemove => Empty;
    /// Puts up the files a runtime's unit keeps below one directory of its state, as one archive, for its move.
    NodeRuntimeExport = "node.runtime.export": NODE, Runtime, 3600, Upload, NodeRuntimeExport => NodeRuntimeExportResult;
    /// Puts the files of an archive below one directory of a runtime's unit's state, before it first starts.
    NodeRuntimeImport = "node.runtime.import": NODE, Runtime, 3600, Download, NodeRuntimeImport => Empty;
    /// Measures the old runtime's named home/private layer volumes before a carry is confirmed.
    NodeRuntimeVolumesMeasure = "node.runtime.volumes.measure": NODE, Runtime, 3600, None, NodeRuntimeVolumesMeasure => NodeRuntimeVolumesMeasureResult;
    /// Carries whole owned volumes, preserving owners, modes, extended attributes and deletion markers.
    NodeRuntimeVolumesExport = "node.runtime.volumes.export": NODE, Runtime, 21600, Upload, NodeRuntimeVolumesExport => NodeRuntimeVolumesExportResult;
    /// Restores a carried owned volume before the target unit is created or started.
    NodeRuntimeVolumeImport = "node.runtime.volume.import": NODE, Runtime, 21600, Download, NodeRuntimeVolumeImport => Empty;
    /// Checks whole owned-volume carry support on the target before a plan can be confirmed.
    NodeVolumeCarryCheck = "node.volume.carry-check": NODE, None, 25, None, NodeVolumeCarryCheck => NodeVolumeCarryCheckResult;
    /// Counts a mount's data where it is, before a copy of it is planned.
    NodeMountMeasure = "node.mount.measure": NODE, None, 3600, None, NodeMountMeasure => orch_driver_contract::volumes::VolumeMeasure;
    /// Fingerprints a mount's data in place without following links or changing it.
    NodeMountFingerprint = "node.mount.fingerprint": NODE, None, 3600, None, NodeMountFingerprint => NodeMountFingerprintResult;
    /// Archives a mount's data where it is, as checked parts in the content store, for a copy to another host.
    NodeMountExport = "node.mount.export": NODE, Volume, 21600, Upload, NodeMountExport => VolumeArchive;
    /// Unpacks a copy of a mount's data into a volume made for that mount, which no unit uses.
    NodeMountImport = "node.mount.import": NODE, Volume, 21600, Download, NodeMountImport => Empty;
    /// Makes what a unit's file system comes from available on a host.
    NodeImageEnsure = "node.image.ensure": NODE, Image, 3600, Download, NodeImageEnsure => NodeImageEnsureResult;
    /// Proposes a template's spec from something a person already has.
    NodeTemplateImport = "node.template.import": NODE, None, 60, None, NodeTemplateImport => NodeTemplateImportResult;
    /// Makes sure a tool is in a host's cache.
    NodeToolEnsure = "node.tool.ensure": NODE, Tool, 1800, None, NodeToolEnsure => NodeToolEnsureResult;
    /// Runs a build in a host's cache so that it prints its interface, and says which names it lacks.
    NodeToolCheck = "node.tool.check": NODE, Tool, 180, None, NodeToolCheck => NodeToolCheckResult;
    /// Runs native adapter cases for a cached build in a disposable networkless unit.
    NodeToolConformance = "node.tool.conformance": NODE, Tool, 1800, None, NodeToolConformance => NodeToolConformanceResult;
    /// Fetches a skill's files onto a host, from a Git repository at a reference or from an address of an archive,
    /// and puts them up as one archive: the server itself fetches no address.
    NodeSkillFetch = "node.skill.fetch": NODE, None, 600, Upload, NodeSkillFetch => NodeSkillFetchResult;
    /// Reads what a host has, runs and can make.
    NodeProbe = "node.probe": NODE, Machine, 30, None, NodeProbe => NodeProbeResult;
    /// Checks a declaration of host ports against the node's own ceiling, and proposes one.
    HostPorts = "host.ports": PORT_PUBLISHER, Machine, 30, None, crate::ports::HostPortsCheck => crate::ports::HostPortsCheckResult;
    /// Binds a host opening's reviewed number and carries its callers to the environment.
    PortPublish = "port.publish": PORT_PUBLISHER, Machine, 30, None, crate::ports::PortPublish => crate::ports::PortPublishResult;
    /// Ends a host opening's listeners and connections on its node.
    PortClose = "port.close": PORT_PUBLISHER, Machine, 15, None, crate::ports::PortClose => crate::ports::PortCloseResult;
}

impl std::fmt::Display for CommandKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Cmd {
    /// The frame for a command of the catalogue.
    pub fn new<B: CommandBody>(
        id: Id,
        key: &SerialKey,
        deadline: Time,
        body: &B,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            id,
            kind: B::KIND.as_str().to_owned(),
            key: key.wire(),
            deadline,
            body: serde_json::to_value(body)?,
        })
    }

    /// The command, when its kind is one of the catalogue and its body
    /// is what that kind defines.
    pub fn command(&self) -> Result<Command, CatalogueError> {
        Command::decode(&self.kind, self.body.clone())
    }
}

impl CmdRes {
    /// The answer to a command that succeeded with `result`.
    pub fn ok<R: Serialize>(id: Id, result: &R) -> Result<Self, serde_json::Error> {
        Ok(Self {
            id,
            outcome: Ok(serde_json::to_value(result)?),
        })
    }

    /// The answer to a command that failed.
    #[must_use]
    pub fn failed(id: Id, error: CommandError) -> Self {
        Self {
            id,
            outcome: Err(error),
        }
    }

    /// The result of a command whose body was a `B`, or why it failed.
    pub fn result<B: CommandBody>(
        &self,
    ) -> Result<Result<B::Result, CommandError>, serde_json::Error> {
        match &self.outcome {
            Ok(body) => serde_json::from_value(body.clone()).map(Ok),
            Err(error) => Ok(Err(error.clone())),
        }
    }
}

// ---------------------------------------------------------------- bodies: any machine

/// An answer that says nothing beyond that the command succeeded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Empty {}

/// The body of `machine.probe`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct MachineProbe {}

/// The server's idle grant for one physical program, never a runtime's connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MachineUpdateNow {
    /// The same recommendation announced by ready.versions; no arbitrary download address is accepted.
    pub recommended: String,
}

/// What the physical program did with Update now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineUpdateNowResult {
    /// `scheduled` waits for the local idle fence; `current` changes nothing.
    pub status: String,
    /// The version currently executing, not merely the downloaded version.
    pub running_version: String,
    /// The recommendation that was requested.
    pub recommended_version: String,
    /// Human-readable outcome; a refusal is a CommandError with its local command.
    pub detail: String,
}

/// The body of `machine.policy.set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MachinePolicySet {
    /// The keys to change, each with its new value.
    pub values: BTreeMap<String, Json>,
}

/// The same role selection on the HTTP and native boundaries.
pub type MachineRolesSet = orch_api_contract::resources::MachineRolesSetBody;

/// The command answer is durable before the supervisor switches the active roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineRolesSetResult {
    /// The connector's validated and persisted role names.
    pub roles: Vec<String>,
    /// False for an idempotent request for the currently active roles.
    pub reconnecting: bool,
}

/// The result of `machine.policy.set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MachinePolicySetResult {
    /// The keys that were changed.
    pub applied: Vec<String>,
    /// The keys that were left as they were.
    pub refused: Vec<String>,
    /// The policy as it now stands, which is what is in force.
    pub policy: LocalPolicy,
}

// ---------------------------------------------------------------- bodies: terminals

/// The body of `pty.open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyOpen {
    /// The environment the shell runs in.
    pub environment_id: Id,
    /// The identifier the shell is known by from now on.
    pub terminal_id: Id,
    /// The width of its screen, in characters.
    pub cols: u16,
    /// The height of its screen, in lines.
    pub rows: u16,
    /// The directory it starts in.
    pub cwd: String,
    /// Whether the last of what it prints is kept when it ends.
    pub record: bool,
}

/// The result of `pty.open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyOpenResult {
    /// The service a tunnel to the shell names.
    pub service: String,
    /// The shell that was started.
    pub shell: String,
}

/// The body of `pty.close`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyClose {
    /// The shell.
    pub terminal_id: Id,
}

/// A bounded selection of shell identities; observing it neither attaches nor changes a shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyStatus {
    /// The environment holding the selected shells.
    pub environment_id: Id,
    /// At most two hundred identities from the caller's authorized session page.
    pub terminal_ids: Vec<Id>,
}

/// The host's current foreground observation for one shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyStatusEntry {
    /// The selected shell.
    pub terminal_id: Id,
    /// A foreground command holds the shell instead of its prompt.
    pub busy: bool,
}

/// Observations for selected shells held by this environment host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyStatusResult {
    /// No observation of a shell outside the requested selection is included.
    pub terminals: Vec<PtyStatusEntry>,
}

/// The result of `pty.close`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyCloseResult {
    /// What the shell exited with; `null` where it has no exit code.
    pub exit_code: Option<i64>,
    /// How many bytes it printed in all.
    pub bytes_out: i64,
    /// What was kept of its output, where that was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<BlobRef>,
}

/// The body of `agent_pty.open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentPtyOpen {
    /// The thread whose harness has the terminal.
    pub thread_id: Id,
    /// Whether what is typed is passed on, or the screen is only watched.
    pub control: bool,
}

/// The result of `agent_pty.open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentPtyOpenResult {
    /// The service a tunnel to the terminal names.
    pub service: String,
}

// ---------------------------------------------------------------- bodies: harness processes

/// Provider runtime variables applied over an agent profile on the machine that runs the runtime.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentRuntime {
    /// Declared runtime resources, enforced where its actual process controls support them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<orch_driver_contract::runtime::Resources>,
    /// Plain values or secret references, sealed to the receiving machine before delivery.
    pub variables: BTreeMap<String, SecretOrText>,
    /// The actual runtime's selected harness home, independently of its placement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<orch_driver_contract::runtime::EnvironmentDataSource>,
    /// Named machine storage for a new home; existing data retains its recorded storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_storage: Option<String>,
}

/// The body of `agent.start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentStart {
    /// The provider the harness process serves.
    pub provider_id: Id,
    /// The agent profile it runs.
    pub agent_id: Id,
    /// The harness kind, which names its adapter.
    pub harness_kind: String,
    /// The profile: which build, and how it is configured.
    pub profile: foreign::AgentProfile,
    /// Its home directory, relative to the connector's data directory;
    /// `null` on a person's machine, where the profile or the harness
    /// itself says where its home is.
    pub home: Option<String>,
    /// What the provider's owner configured about the harness as a whole (`providers.config`), as the adapter's
    /// options schema allows it; `{}` where nothing is set. The adapter is started with it, so that what the page
    /// shows as set is what runs.
    #[serde(default)]
    pub provider_config: Json,
    /// Runtime values, separate from adapter options and the profile's environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<AgentRuntime>,
}

/// The result of `agent.start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentStartResult {
    /// The requested process was already running unchanged; after-start commands are not repeated.
    #[serde(default)]
    pub reused: bool,
    /// The version of the harness that runs.
    pub harness_version: String,
    /// What its adapter declares about it.
    pub descriptor: foreign::AgentDescriptor,
}

/// The body of `agent.stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentStop {
    /// The provider of the harness process.
    pub provider_id: Id,
    /// Its agent profile.
    pub agent_id: Id,
    /// A provider deletion requests native sign-out first, subject to the same local sign-in ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logout: Option<bool>,
}

/// The body of `agent.discover_sessions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentDiscoverSessions {
    /// The provider of the harness process.
    pub provider_id: Id,
    /// Its agent profile.
    pub agent_id: Id,
}

/// The result of `agent.discover_sessions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentDiscoverSessionsResult {
    /// The conversations the harness holds.
    pub sessions: Vec<foreign::NativeSession>,
}

/// The body of `agent.read_session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentReadSession {
    /// The provider of the harness process.
    pub provider_id: Id,
    /// Its agent profile.
    pub agent_id: Id,
    /// The harness's own reference to the conversation.
    pub native_session_ref: String,
    /// Whether to write its earlier turns out and upload them; without, only what it ran with and how much
    /// it holds is said.
    pub write: bool,
}

/// What was uploaded of a conversation's record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordRef {
    /// The record: entries, one JSON object to a line, oldest first.
    pub blob: Hash,
    /// Its size in bytes.
    pub size: i64,
}

/// The result of `agent.read_session`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentReadSessionResult {
    /// The model the conversation ran with last, where the harness says.
    pub model: Option<String>,
    /// The reasoning effort it ran with last, where the harness says.
    pub effort: Option<String>,
    /// How many of its turns the record holds.
    pub turns: i64,
    /// How many entries they come to.
    pub entries: i64,
    /// The record, when it was asked for and the conversation holds any.
    pub record: Option<RecordRef>,
    /// What became of what the harness recorded.
    pub report: foreign::HistoryReport,
}

// ---------------------------------------------------------------- bodies: sessions and turns

/// How a session comes to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// A fresh conversation.
    New,
    /// The conversation the harness already holds for this thread.
    Resume,
    /// A copy, made by the harness, of another conversation it holds.
    Fork,
    /// A history written by the same kind of harness, given as an
    /// archive.
    ImportNative,
    /// A history in the system's own neutral form.
    ImportNeutral,
    /// A conversation the harness held before the system knew of it.
    Adopt,
}

/// Who keeps the notes an agent makes from one session to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKeeper {
    /// The harness has a memory of its own, which the adapter points at
    /// the directory.
    Harness,
    /// The agent, with its tools.
    Agent,
}

/// One of the system's own tools that a session may be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrchTool {
    /// Lists what the team shared with the agent.
    ListShared,
    /// Reads a part of it.
    ReadShared,
    /// Searches all of it.
    SearchShared,
    /// Copies a file or a conversation into the workspace.
    DownloadShared,
    /// Makes or changes a document.
    EditDocument,
    /// Organizes documents and folders; only deletion requires a preview and confirmation.
    ManageShared,
    /// Posts to a chat or queues a note to a thread's agent, according to its typed target.
    SendMessage,
    /// Historical chat calls retained for replay; not an offered tool.
    SendChatMessage,
    /// Historical thread calls retained for replay; not an offered tool.
    SendThreadMessage,
    /// Tells the other agents of the topic, or the people who follow the
    /// thread.
    Announce,
    /// Asks for something the environment lacks.
    RequestCapability,
    /// Opens, joins, announces or closes a browser in the agent's environment.
    UseBrowser,
    /// Opens, announces or closes a display in the agent's environment.
    UseDisplay,
    /// Lists the ports of the agent's environment, their outside addresses and who may reach them.
    ListPorts,
    /// Asks for a port of the agent's environment to be opened to more people, which a person decides.
    OpenPort,
    /// Closes an opening of a port that the thread asked for.
    ClosePort,
    /// Lists who reached a port of the agent's environment, with both ends of each connection.
    ListPortConnections,
    /// Reads the thread's goal, and reports where the agent stands on it.
    Goal,
}

/// A native history to import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenArchive {
    /// The format the archive is in, as its harness names it.
    pub format: String,
    /// The archive.
    pub blob: Hash,
    /// Its size in bytes.
    pub size: i64,
}

/// A neutral history to import: one item to a line, oldest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenHistory {
    /// The history.
    pub blob: Hash,
    /// Its size in bytes.
    pub size: i64,
}

/// A mount attached beside the workspace: one more root for the
/// harness's sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenMount {
    /// The mount's name.
    pub name: String,
    /// Where it is in the environment.
    pub path: String,
    /// Whether nothing may be written there.
    pub read_only: bool,
}

/// Where an agent's notes are kept, and by whom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenMemory {
    /// The directory of the notes.
    pub directory: String,
    /// Who writes them.
    pub kept_by: MemoryKeeper,
}

/// How the harness of a session reaches its model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "path", rename_all = "snake_case")]
pub enum SessionOpenProvider {
    /// By the sign-in its own home holds.
    Native {
        /// The provider.
        provider_id: Id,
    },
    /// Through the forwarder, which adds the key where it is kept.
    Forwarded {
        /// The provider.
        provider_id: Id,
        /// The protocol the harness speaks to the forwarder.
        protocol: String,
        /// The ticket that opens the tunnels its model calls travel in.
        ticket: TicketSlot,
    },
    /// On a copy of the credential, which is given to it separately.
    Shared {
        /// The provider.
        provider_id: Id,
        /// The form the copy has.
        form: String,
    },
    /// On the credential itself, handed to where the harness runs.
    Handover {
        /// The provider.
        provider_id: Id,
        /// The form the credential has.
        form: String,
    },
}

/// What carries out the tool calls of a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenExec {
    /// The executor the harness works through; `null` where the harness
    /// runs in the environment and carries out its own tools.
    pub executor: Option<String>,
    /// The tool set, for an executor that offers its tools in several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_set: Option<String>,
    /// The environment the tools run in.
    pub environment_id: Id,
    /// How many times that environment was made anew.
    pub generation: i64,
    /// The ticket that opens the tunnels to the executor. An executor
    /// that offers nothing has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<TicketSlot>,
}

/// A skill a session is given: one version of it, as the archive of its files (gzip tar, `SKILL.md` at the top),
/// fetched like the other files of an open and checked against its hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenSkill {
    /// The skill.
    pub skill_id: Id,
    /// The version given.
    pub version_id: Id,
    /// The name its `SKILL.md` gives, which is the name of its directory.
    pub name: String,
    /// Its archive.
    pub blob: Hash,
    /// The archive's size in bytes.
    pub size: i64,
}

/// A tool server of the environment that the session is given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenMcp {
    /// Stable Orch membership, separate from the name a harness sees; absent on older commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Id>,
    /// The name the server is offered under.
    pub name: String,
    /// The ticket that opens the tunnels to it.
    pub ticket: TicketSlot,
}

/// The body of `session.open`: everything a session is set up with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpen {
    /// The agent profile.
    pub agent_id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The stretch of the thread this session is.
    pub segment_id: Id,
    /// How the session comes to exist.
    pub mode: SessionMode,
    /// The harness's own name for the conversation to resume, to adopt
    /// or to fork from.
    pub native_session_ref: Option<String>,
    /// The native history to import.
    pub archive: Option<SessionOpenArchive>,
    /// The neutral history to import.
    pub history: Option<SessionOpenHistory>,
    /// For a fork and for an import: only through this turn.
    pub upto_turn_ordinal: Option<u32>,
    /// The environment's workspace root, or the registered directory of a personal environment.
    pub workspace_root: String,
    /// Whether the environment host verified its empty, root-only instruction marker for this open.
    pub instruction_root_marker: bool,
    /// The thread's directory in its environment: where its work is.
    pub directory: String,
    /// Where the agent starts inside that directory.
    pub cwd: String,
    /// The mounts attached beside the workspace.
    pub mounts: Vec<SessionOpenMount>,
    /// The model, by the name that is sent.
    pub model: String,
    /// The reasoning effort; `null` where the model takes none.
    pub effort: Option<String>,
    /// The adapter's own parameters, as they resolve for the thread.
    pub params: Object,
    /// When the harness asks before it acts.
    pub approval_mode: ApprovalMode,
    /// The exact models the input's author may use on this provider, bounding its native reviewer.
    #[serde(default)]
    pub available_models: Vec<String>,
    /// Limits over the harness's own sub-agents, independent of its model parameters.
    #[serde(default)]
    pub subagent_limits: orch_agent_contract::session::SubagentLimits,
    /// What the harness's own sandbox allows.
    pub sandbox_mode: SandboxMode,
    /// What the agent is told that is no person's message.
    pub instructions: String,
    /// Where the agent's notes are kept; `null` when none are, and a
    /// memory of the harness's own is to be off as well.
    pub memory: Option<SessionOpenMemory>,
    /// The way harness, tools and credential are placed.
    pub arrangement: ArrangementType,
    /// How the harness reaches its model.
    pub provider: SessionOpenProvider,
    /// What carries out its tool calls.
    pub exec: SessionOpenExec,
    /// The tool servers of the environment it is given.
    pub mcp: Vec<SessionOpenMcp>,
    /// The skills it is given, each one version's archive, fetched and laid out for the thread by the connector.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SessionOpenSkill>,
    /// The system's own tools it is given; none when the list is empty.
    pub orch_tools: Vec<OrchTool>,
    /// Whether the harness's own protocol traffic is reported as well.
    pub raw_events: bool,
    /// The server's last accepted raw-frame number; a new allocator starts above this floor.
    #[serde(default)]
    pub raw_after: i64,
    /// For a side conversation: how it reads without acting, which the adapter enforces or refuses the
    /// session for; `null` for an ordinary thread.
    #[serde(default)]
    pub side_reading: Option<SideReading>,
    /// What the people who manage the environment answered about a configuration of the harness's own that its
    /// workspace holds (a `.codex` or `.claude` with something in it), and for which files; `null` while nobody
    /// was asked. The adapter refuses a workspace that holds one with `workspace_configuration` unless the answer
    /// is for the files it finds now.
    #[serde(default)]
    pub workspace_configuration: Option<WorkspaceConfiguration>,
    /// The descriptions a place set for Orch's tools, by tool name, where they differ from the shipped ones: the
    /// tool server lists them to the harness in place of its own.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub orch_tool_descriptions: BTreeMap<String, String>,
    /// Input schemas narrowed to the place's current policy, by offered tool name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub orch_tool_schemas: BTreeMap<String, Json>,
}

/// The answer about a workspace's own harness configuration, as the environment remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceConfiguration {
    /// Trust or Keep it out.
    pub answer: ConfigTrust,
    /// The hash of the files the answer was given for, as the adapter's refusal named it.
    pub files: String,
}

/// What a person answered about a workspace's own harness configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigTrust {
    /// The harness uses it, as it does in a folder it trusts.
    Trust,
    /// The harness works without it, as it does in a folder it does not trust.
    KeepOut,
}

/// How a side conversation reads and never acts: the mechanism its harness verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SideReading {
    /// The harness's own enforced read-only mode.
    ReadOnly,
    /// Every permission request declined before any effect, with only verified reading admitted.
    ApprovalGated,
    /// No model-callable tool at all.
    NoTools,
}

/// The result of `session.open`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenResult {
    /// The harness's own name for the conversation.
    pub native_session_ref: String,
    /// Whether it is a conversation the harness already held.
    pub resumed: bool,
    /// How faithfully an imported history arrived; `null` without one.
    pub import_report: Option<foreign::HistoryReport>,
    /// The instruction files the harness reads for this session, in its order, as its adapter knows them (reported,
    /// or by the documented loading rule); a file of the person's own home has its label and no path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instruction_sources: Vec<orch_agent_contract::session::InstructionFile>,
    /// The model of the session's native permission reviewer, where Auto was requested.
    #[serde(default)]
    pub approval_reviewer: Option<orch_api_contract::settings::ApprovalReviewer>,
}

/// The body of `session.close`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionClose {
    /// The thread.
    pub thread_id: Id,
}

/// Exact native sessions to measure or remove in the named provider/profile home.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentHistoryManage {
    /// The provider whose managed home holds these sessions.
    pub provider_id: Id,
    /// The profile whose harness owns this home.
    pub agent_id: Id,
    /// Captured native identities; paths are resolved only by the adapter.
    pub sources: Vec<orch_agent_contract::history::NativeHistorySource>,
    /// False measures without changing files; true removes each permitted history.
    pub remove: bool,
}

/// Per-session measurements, completed removals and explicit refusals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentHistoryManageResult {
    /// Exactly one factual result for every requested source.
    pub facts: Vec<orch_agent_contract::history::NativeHistoryFact>,
}

/// A system-retention cutoff for documented leftovers in the exact managed home.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentHistoryHomeCleanup {
    /// The provider whose home is cleaned.
    pub provider_id: Id,
    /// The profile owning this harness home.
    pub agent_id: Id,
    /// Entries modified at or after this instant stay.
    pub before: Time,
}

/// Per-directory confirmed cleanup counts and honest partial failures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentHistoryHomeCleanupResult {
    /// One result per documented cache root inspected by the adapter.
    pub items: Vec<orch_agent_contract::history::NativeHomeCleanupItem>,
}

/// Refreshes tool bindings without opening a session, starting or interrupting a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionMcpReload {
    /// The existing thread.
    pub thread_id: Id,
    /// Its exact open segment; a changed binding refuses the update.
    pub segment_id: Id,
    /// The full offered environment server set, with the usual signed tunnel tickets.
    pub mcp: Vec<SessionOpenMcp>,
}

/// The body of a live session command read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionBackground {
    /// The thread whose existing session is inspected.
    pub thread_id: Id,
    /// The exact retained segment, preventing a changed binding from substituting a session.
    pub segment_id: Id,
    /// Continues the same bounded native snapshot, renewing its idle transfer deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Reads the effective action mapping of the exact open native session generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionNativeActions {
    /// The thread whose existing session is read.
    pub thread_id: Id,
    /// Fences an old mapping against a replacement session.
    pub segment_id: Id,
    /// Invalidates the adapter's cached list, without a model turn.
    pub refresh: bool,
}

/// The body of `session.context`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionContext {
    /// The thread whose open session is asked.
    pub thread_id: Id,
    /// The exact segment, so that a session of a changed binding never answers for it.
    pub segment_id: Id,
}

/// The result of `session.context`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionContextResult {
    /// What fills the context, as the harness counted it; `null` where it cannot count it now (a turn runs,
    /// the session is not open here, or the harness breaks nothing down).
    pub fill: Option<orch_api_contract::context::ContextFill>,
}

/// The body of an exact native session command end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionBackgroundEnd {
    /// The owning thread.
    pub thread_id: Id,
    /// The captured native session generation.
    pub segment_id: Id,
    /// The native session-owned command identity.
    pub command_id: String,
}

/// The form a session's history is written out in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    /// The harness's own.
    Native,
    /// The system's neutral one.
    Neutral,
}

/// The body of `session.export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionExport {
    /// The thread.
    pub thread_id: Id,
    /// The form to write.
    pub kind: ExportKind,
    /// Only through this turn; all of it when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upto_turn_ordinal: Option<u32>,
}

/// The result of `session.export`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionExportResult {
    /// The format of the archive.
    pub format: String,
    /// The archive.
    pub blob: Hash,
    /// Its size in bytes.
    pub size: i64,
}

/// The body of `session.resync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionResync {
    /// The thread.
    pub thread_id: Id,
}

/// The result of `session.resync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionResyncResult {
    /// How many items were reported again.
    pub items: i64,
}

/// An image passed to the model with an input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnImage {
    /// The image.
    pub blob: Hash,
    /// Its media type.
    pub media_type: String,
    /// Its file name.
    pub name: String,
}

/// The body of `turn.start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnStart {
    /// The thread.
    pub thread_id: Id,
    /// The turn that starts.
    pub turn_id: Id,
    /// What the agent is told.
    pub text: String,
    /// The images that go with it.
    pub images: Vec<TurnImage>,
    /// The model, by the name that is sent.
    pub model: String,
    /// The reasoning effort; `null` where the model takes none.
    pub effort: Option<String>,
    /// The adapter's own parameters for this turn.
    pub params: Object,
    /// When the harness asks before it acts.
    pub approval_mode: ApprovalMode,
    /// What the harness's own sandbox allows.
    pub sandbox_mode: SandboxMode,
    /// Explicit verified selector; omitted for an ordinary literal input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_action: Option<orch_agent_contract::native::NativeInvocation>,
    /// Whether the turn plans and changes nothing until a person proceeds (`plan_mode` harnesses only).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plan: bool,
}

/// The body of `turn.steer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnSteer {
    /// The thread.
    pub thread_id: Id,
    /// The turn that is running.
    pub turn_id: Id,
    /// What the agent is told.
    pub text: String,
    /// The images that go with it.
    pub images: Vec<TurnImage>,
    /// A verified native steer, rather than slash-like text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_action: Option<orch_agent_contract::native::NativeInvocation>,
}

/// The body of `turn.interrupt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnInterrupt {
    /// The thread.
    pub thread_id: Id,
    /// The turn to stop.
    pub turn_id: Id,
}

/// The body of `approval.resolve`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalResolve {
    /// The thread.
    pub thread_id: Id,
    /// The adapter's own name for what was asked.
    pub request_ref: String,
    /// What was decided.
    pub decision: foreign::ApprovalDecision,
}

/// Why a call to one of the system's own tools failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolError {
    /// The stable code.
    pub code: String,
    /// A sentence for the agent.
    pub message: String,
}

/// The body of `orch.tool_result`: one of `result` and `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OrchToolResult {
    /// The thread.
    pub thread_id: Id,
    /// The adapter's own name for the call.
    pub call_ref: String,
    /// The transport attempt this answer belongs to; absent on the original adapter-only path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<Id>,
    /// What the tool returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Json>,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolError>,
}

// ---------------------------------------------------------------- bodies: providers

/// The body of `provider.login.start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderLoginStart {
    /// The provider that is signed in.
    pub provider_id: Id,
    /// The agent profile whose harness holds the sign-in.
    pub agent_id: Id,
    /// The way of signing in, by the adapter's own name for it.
    pub method: String,
    /// What that way asks for up front, by field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<BTreeMap<String, SecretOrText>>,
    /// What the provider configures about its harness as a whole.
    pub config: Json,
}

/// The body of `provider.login.input`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderLoginInput {
    /// The provider that is being signed in.
    pub provider_id: Id,
    /// The agent profile whose harness holds the sign-in.
    pub agent_id: Id,
    /// What the person gave for the fields the sign-in asked for.
    pub values: BTreeMap<String, SecretOrText>,
}

/// The body of `provider.login.cancel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderLoginCancel {
    /// The provider.
    pub provider_id: Id,
    /// The agent profile whose harness holds the sign-in.
    pub agent_id: Id,
}

/// The body of `provider.logout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderLogout {
    /// The provider.
    pub provider_id: Id,
    /// The agent profile whose harness holds the sign-in.
    pub agent_id: Id,
}

/// The body of `provider.refresh`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRefresh {
    /// The provider.
    pub provider_id: Id,
    /// An explicit recovery read, outside ordinary page coalescing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<bool>,
    /// Ask the native harness to renew its login before observing its current facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renew: Option<bool>,
    /// Where the provider's facts are read from; absent, its default source, as before sources were chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<orch_provider_contract::SourceSpec>,
}

/// What the selected source said it implements and how its latest attempt ended.
/// Absent on older replies. A failed read preserves observations and availability; a supported partial
/// report can retain a safe note while its valid members act normally.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSourceRead {
    /// The declaration learned while making this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<orch_provider_contract::SourceDescriptor>,
    /// A safe, secret-scrubbed failure or report note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<orch_provider_contract::SourceError>,
    /// True for a valid (possibly partial) report, false for an unsupported model list, absent for a failed
    /// read. A partial malformed note keeps its valid members; an empty model list is supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported: Option<bool>,
}

/// The result of `provider.refresh`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRefreshResult {
    /// The facts source's declaration and safe last-read error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ProviderSourceRead>,
    /// The state the provider is in, in the words a provider's state is
    /// written with everywhere.
    pub status: String,
    /// What it reports about its plan and its limits.
    pub facts: Option<foreign::ProviderFacts>,
}

/// The body of a confirmed provider reset attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderResetConsume {
    /// The provider whose harness holds the sign-in.
    pub provider_id: Id,
    /// The identity of this confirmed operation, unchanged on retry.
    pub request_id: Id,
    /// The exact vendor credit the person confirmed.
    pub reset_id: String,
    /// The count the person reviewed; the controller rechecks it before dispatch.
    pub available_count: i64,
    /// The source whose facts listed the reset, which spends it; absent, the provider's default source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<orch_provider_contract::SourceSpec>,
}

/// What a provider's harness answered to a reset attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderResetConsumeResult {
    /// The declaration and safe error of the facts source that listed and spent the reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ProviderSourceRead>,
    /// The outcome in the vendor's own words.
    pub outcome: String,
    /// The facts read after the attempt.
    pub facts: foreign::ProviderFacts,
}

/// The body of `provider.discover_models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderDiscoverModels {
    /// The provider.
    pub provider_id: Id,
    /// Where its models are listed from; absent, its default source, as before sources were chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<orch_provider_contract::SourceSpec>,
}

/// The result of `provider.discover_models`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderDiscoverModelsResult {
    /// The model source's declaration, support and safe last-read error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ProviderSourceRead>,
    /// The models that were found.
    pub suggestions: Vec<foreign::ModelSuggestion>,
}

/// One endpoint of the model service a provider forwards to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderStartUpstream {
    /// The protocol the endpoint speaks.
    pub protocol: String,
    /// Its address.
    pub base_url: String,
    /// What it configures beyond its address, as its protocol declares.
    pub options: Json,
}

/// The body of `provider.start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderStart {
    /// The provider.
    pub provider_id: Id,
    /// The protocols it serves.
    pub protocols: Vec<String>,
    /// The endpoints it forwards to.
    pub upstreams: Vec<ProviderStartUpstream>,
    /// The key; `null` for one kept on another machine, and where the
    /// service takes none.
    pub credential: Option<Secret>,
    /// The values of the headers that are secrets, by name.
    pub secrets: BTreeMap<String, Secret>,
    /// A program that presents the key on each model request; absent, the
    /// key is presented as each endpoint's options say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presenter: Option<orch_provider_contract::SourceSpec>,
}

/// The body of `provider.stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderStop {
    /// The provider.
    pub provider_id: Id,
    /// A confirmed deletion also erases this provider's installer-managed local key and custodian record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purge: Option<bool>,
}

/// What a model request that the system makes itself is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompletionPurpose {
    /// A title, a description or tags for a piece of work.
    Summary,
}

/// The body of `provider.complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderComplete {
    /// The provider whose key pays for the request.
    pub provider_id: Id,
    /// What the request is for.
    pub purpose: CompletionPurpose,
    /// The protocol to make it in.
    pub protocol: String,
    /// The model, by the name that is sent.
    pub model_name: String,
    /// Additional model parameters selected by an explicit recovery candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Json>,
    /// The instruction.
    pub system: String,
    /// What the instruction is applied to.
    pub input: String,
    /// The most tokens the answer may have.
    pub max_output_tokens: i64,
}

/// The result of `provider.complete`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderCompleteResult {
    /// Model-client measurements using the same definitions as harness requests.
    #[serde(default)]
    pub timing: orch_api_contract::usage::RequestTiming,
    /// The answer.
    pub text: String,
    /// What the request used.
    pub usage: foreign::TokenUsage,
    /// The model service's own name for the request, where it gave one.
    pub request_ref: Option<String>,
}

/// The body of `provider.embed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderEmbed {
    /// The provider whose endpoint makes the vectors and whose key, if it has one, pays for them.
    pub provider_id: Id,
    /// The protocol to make them in.
    pub protocol: String,
    /// The model, by the name that is sent.
    pub model_name: String,
    /// The texts, each with whatever its model wants put before it already in place.
    pub inputs: Vec<String>,
    /// The length of vector asked of a model that can shorten its vectors; absent for its own length.
    pub dimensions: Option<u32>,
}

/// The result of `provider.embed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderEmbedResult {
    /// Model-client measurements using the same definitions as harness requests.
    #[serde(default)]
    pub timing: orch_api_contract::usage::RequestTiming,
    /// One vector for each input, in their order, as [`encode_vector`] writes it.
    pub vectors: Vec<String>,
    /// What the request used.
    pub usage: foreign::TokenUsage,
    /// The model service's own name for the request, where it gave one.
    pub request_ref: Option<String>,
}

/// A vector as `provider.embed` carries it: its numbers as little-endian 32-bit floats, in standard base64. A
/// quarter of the length of the same numbers written as JSON, so that a batch fits a frame.
#[must_use]
pub fn encode_vector(vector: &[f32]) -> String {
    use base64::Engine as _;
    let bytes: Vec<u8> = vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Reads a vector [`encode_vector`] wrote; `None` for text that is not one.
#[must_use]
pub fn decode_vector(text: &str) -> Option<Vec<f32>> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()?;
    if bytes.len() % 4 != 0 {
        return None;
    }
    bytes
        .chunks_exact(4)
        .map(|four| {
            let value = f32::from_le_bytes([four[0], four[1], four[2], four[3]]);
            value.is_finite().then_some(value)
        })
        .collect()
}

/// How a lease lets a credential be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeaseMode {
    /// Model calls pass the forwarder, which adds the key.
    Forwarded,
    /// A copy of the credential is issued to where the harness runs.
    Shared,
    /// The credential itself is handed to where the harness runs.
    Handover,
}

/// A machine, with the key that values meant for it are sealed to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct MachineKey {
    /// The machine.
    pub machine_id: Id,
    /// The public key of its certificate.
    pub public_key: String,
}

/// What a turn may use in credits before the forwarder refuses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseOpenAllowanceCredits {
    /// How many credits, as a decimal number.
    pub amount: String,
    /// The rate the tokens of its requests are counted at.
    pub rate: foreign::Rate,
}

/// What a turn may still use before the forwarder refuses its requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseOpenAllowance {
    /// The room in tokens, where a cap in tokens covers the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    /// The room in credits, where a cap in credits covers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<LeaseOpenAllowanceCredits>,
}

/// The body of `lease.open`, sent to where the credential is kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseOpen {
    /// The lease.
    pub lease_id: Id,
    /// The provider.
    pub provider_id: Id,
    /// The thread whose turn it is for.
    pub thread_id: Id,
    /// How the credential may be used under it.
    pub mode: LeaseMode,
    /// The protocol of the calls that are forwarded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// The form of the credential that is issued or handed over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
    /// The only machine a credential may be issued to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<MachineKey>,
    /// The models a request under the lease may name.
    pub allowed_models: Vec<String>,
    /// What the turn may use before the forwarder refuses; absent when
    /// no cap covers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowance: Option<LeaseOpenAllowance>,
}

/// The body of `provider.credential.issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderCredentialIssue {
    /// The lease under which the credential is issued.
    pub lease_id: Id,
    /// The provider.
    pub provider_id: Id,
    /// The form to issue.
    pub form: String,
    /// The machine to seal it to.
    pub recipient: MachineKey,
}

/// A credential on its way between two machines, readable only by the
/// one it is sealed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SealedCredential {
    /// The form it has.
    pub form: String,
    /// The credential, sealed.
    pub credential: Sealed,
    /// When it lapses by itself; `null` when it does not.
    pub expires_at: Option<Time>,
}

/// The body of `provider.credential.accept`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderCredentialAccept {
    /// The provider the credential comes home to.
    pub provider_id: Id,
    /// The form it has.
    pub form: String,
    /// The credential, sealed to the provider's runtime.
    pub credential: Sealed,
    /// When it lapses by itself; `null` when it does not.
    pub expires_at: Option<Time>,
}

/// The body of `agent.credential.return`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentCredentialReturn {
    /// The provider whose credential the harness holds.
    pub provider_id: Id,
    /// The agent profile of the harness.
    pub agent_id: Id,
    /// The machine that keeps the credential, to seal it to.
    pub custodian: MachineKey,
}

/// The endpoint a harness calls itself with the credential it is given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentCredentialSetUpstream {
    /// The protocol the endpoint speaks.
    pub protocol: String,
    /// Its address.
    pub base_url: String,
}

/// The body of `agent.credential.set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentCredentialSet {
    /// The provider the credential is of.
    pub provider_id: Id,
    /// The agent profile of the harness that is to run on it.
    pub agent_id: Id,
    /// The form it has.
    pub form: String,
    /// The credential, sealed to this machine.
    pub credential: Sealed,
    /// When it lapses by itself; `null` when it does not.
    pub expires_at: Option<Time>,
    /// The endpoint to use it at; `null` for a harness that knows its
    /// own.
    pub upstream: Option<AgentCredentialSetUpstream>,
    /// The machine that keeps the credential and takes it back.
    pub custodian: MachineKey,
}

/// The body of `lease.close`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseClose {
    /// The lease.
    pub lease_id: Id,
}

/// The result of `lease.close`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseCloseResult {
    /// How many model requests ran under the lease.
    pub requests: i64,
    /// What they used together; `null` where that could not be read.
    pub usage: Option<foreign::TokenUsage>,
}

// ---------------------------------------------------------------- bodies: environments

/// Whether a list of the whole tree is kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IndexMode {
    /// The tree is indexed, and its list uploaded as it changes.
    #[default]
    Full,
    /// The tree is too large for that: it is listed as it is, when asked.
    Live,
}

/// How a mount is attached where it already lies on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachMode {
    /// Nothing is written there.
    ReadOnly,
    /// Writable: the very same files for every environment that has it.
    Shared,
}

/// A mount to attach where it lies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvAttachMount {
    /// An owned data reference, resolved by the keeping machine through its declared data maker.
    /// Absent means source is an existing path bounded by environment.roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<orch_driver_contract::runtime::VolumeRef>,
    /// The mount.
    pub mount_id: Id,
    /// Its name.
    pub name: String,
    /// Where its data is on the machine.
    pub source: String,
    /// How it is attached.
    pub mode: AttachMode,
}

/// One command in a unit run. Its position distinguishes the template and scoped layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnitAfterUp {
    /// The start job's stable run identifier; a new unit start has a new identifier.
    pub run_id: Id,
    /// Exactly one of environment_id and provider_id is present.
    pub environment_id: Option<Id>,
    /// A provider runtime's provider; absent for environments.
    pub provider_id: Option<Id>,
    /// A directory provider runtime's profile, so commands use that harness's actual home and variables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<Id>,
    /// Its zero-based position in template-first, then setting name order.
    pub position: u32,
    /// The name shown on the start job and recorded in history.
    pub name: String,
    /// Literal argv and the per-command deadline; there is no implicit shell.
    pub run: orch_settings_contract::unit::AfterUpCommand,
}

/// The kept outcome of a command; output is bounded and scrubbed before it leaves the unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnitAfterUpResult {
    /// The name given by the request.
    pub name: String,
    /// Its exit code; absent when it could not run, was interrupted or timed out.
    pub exit: Option<i32>,
    /// Elapsed wall-clock milliseconds.
    pub duration_ms: u64,
    /// Whether it passed its deadline and its process group was killed.
    pub timed_out: bool,
    /// First ten lines, at most 1000 characters, with known secrets removed.
    pub first_lines: String,
}

impl UnitAfterUpResult {
    /// Whether this command lets the start continue.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.exit == Some(0) && !self.timed_out
    }
}

/// The body of `env.attach`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvAttach {
    /// Declared resources for this environment; absent retains the enclosing unit's limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<orch_driver_contract::runtime::Resources>,
    /// The committed or candidate owned-data generation; absent keeps the original directory layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_generation: Option<i64>,
    /// Named machine storages for newly made data; existing directories keep their location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<orch_settings_contract::unit::UnitStorage>,
    /// Independent workspace/home selections for a directory-running environment. Null keeps legacy roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_sources: Option<orch_driver_contract::runtime::EnvironmentDataSources>,
    /// The environment.
    pub environment_id: Id,
    /// The root of its workspace; `null` where a driver made the unit
    /// and the connector uses what it was given.
    pub workspace_root: Option<String>,
    /// The patterns of what is left out of the workspace.
    pub ignore: Vec<String>,
    /// Whether a list of the whole tree is kept.
    pub index: IndexMode,
    /// The mounts to attach; `null` where a driver attached them.
    pub mounts: Option<Vec<EnvAttachMount>>,
    /// The variables its commands are to find.
    pub variables: BTreeMap<String, String>,
    /// The secrets its commands are to find, by variable name.
    pub secrets: BTreeMap<String, Secret>,
}

/// The body of `env.detach`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvDetach {
    /// The environment.
    pub environment_id: Id,
}

/// Owned directory cleanup after detachment; borrowed paths are never named by this command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvDataRemove {
    /// The environment whose storage records prove ownership.
    pub environment_id: Id,
    /// The exact generation captured before removal was queued.
    pub data_generation: i64,
}

/// The body of `exec.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecStart {
    /// The environment.
    pub environment_id: Id,
    /// The executor to start, by its kind.
    pub executor: String,
}

/// The result of `exec.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecStartResult {
    /// The service a tunnel to the executor names.
    pub service: String,
    /// The build that runs.
    pub version: String,
    /// The tool sets it can offer.
    pub tool_sets: Vec<String>,
}

/// The body of `mcp.start`: a server whose definition the system gives,
/// or one the machine keeps a copy of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum McpStart {
    /// A server that was imported on this machine, started from the copy
    /// of its definition that the connector keeps.
    Local(McpStartLocal),
    /// A server whose definition comes with the command.
    Given(Box<McpStartGiven>),
}

/// A tool server whose definition comes with the command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpStartGiven {
    /// Stable Orch membership, retained for call refusal and actual session dependencies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Id>,
    /// The environment.
    pub environment_id: Id,
    /// The name the server is offered under.
    pub name: String,
    /// How it is started and reached.
    pub spec: foreign::McpServerSpec,
    /// The values of the variables its definition names as secrets.
    pub secrets: BTreeMap<String, Secret>,
}

/// A tool server the machine keeps a copy of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpStartLocal {
    /// The environment.
    pub environment_id: Id,
    /// The name the server is offered under.
    pub name: String,
    /// The server, as the system knows it.
    pub server_id: Id,
    /// Says that the definition is the machine's own copy.
    pub local: True,
}

/// The result of `mcp.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpStartResult {
    /// The service a tunnel to the server names.
    pub service: String,
}

/// The same single-server inputs as start, under a distinct command kind that intentionally resets its budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct McpRestart(pub McpStart);

/// The restarted server's service and fresh metadata-only states for the environment's server list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpRestartResult {
    /// Its named service, unchanged for the sessions that use it.
    pub service: String,
    /// Current states; definitions, tokens and secret values are never included.
    pub servers: Vec<crate::events::EnvironmentFactsMcpServer>,
}

/// The body of `mcp.import`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpImport {
    /// When supplied, the native definition must still match before any protected copy is written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_fingerprint: Option<Hash>,
    /// The environment the server was found in.
    pub environment_id: Id,
    /// The identifier the copy is kept under.
    pub server_id: Id,
    /// The harness kind whose configuration names it.
    pub source: String,
    /// Which of that harness's configurations: its user's own, or one in
    /// a directory of the workspace.
    pub scope: String,
    /// The server's name there.
    pub name: String,
    /// The directory of that configuration; `null` for the user's own.
    pub directory: Option<String>,
}

/// The result of `mcp.import`. The definition itself stays on the
/// machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpImportResult {
    /// Structural identity of the copied definition, excluding credential and variable values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_fingerprint: Option<Hash>,
    /// How the server is reached.
    pub transport: String,
    /// A value that changes when its definition does.
    pub digest: String,
    /// How many tools it lists; `null` where it was not asked.
    pub tools: Option<i64>,
}

/// The body of `mcp.forget`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpForget {
    /// The server whose copy is deleted.
    pub server_id: Id,
}

/// The body of `mcp.stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpStop {
    /// The environment.
    pub environment_id: Id,
    /// The name the server is offered under.
    pub name: String,
}

// ---------------------------------------------------------------- bodies: files

/// The body of `fs.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsList {
    /// The environment.
    pub environment_id: Id,
    /// An attached mount to list in place of the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// The directory, from the root of the workspace or of the mount.
    pub path: String,
    /// When given, the directory is watched under this identifier until
    /// the watch is ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_id: Option<Id>,
    /// A caller's exact installation attempt; echoed by modern peers with its reset and result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_instance: Option<Id>,
    /// A page of one immutable observation; absent starts the usual first page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<FsListPage>,
}

/// An opaque continuation selects metadata, never filesystem authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsListPage {
    /// The preceding page's continuation; absent starts a fresh metadata observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// One through FS_LIST_MAX_ENTRIES; absent keeps the existing maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The controller's current environment generation, bound to every continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<i64>,
}

/// Immutable sorted metadata observed during the returned directory page's capture interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsListPageResult {
    /// Hash of sorted metadata, independent of page size.
    pub revision: Hash,
    /// Null is the actual end of this snapshot.
    pub next_cursor: Option<String>,
    /// When the metadata observation finished, not an instantaneous filesystem cut.
    pub as_of: Time,
    /// When observation began; absent for a peer that cannot report the interval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_from: Option<Time>,
    /// Five minutes after actual last use, unless evicted earlier under quota pressure.
    pub expires_at: Time,
}

/// The result of `fs.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FsListResult {
    /// The entries of the directory.
    pub entries: Vec<foreign::DirEntry>,
    /// Whether there are more than a listing returns.
    pub truncated: bool,
    /// The installation attempt this answer completed; absent for a plain read or legacy peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_instance: Option<Id>,
    /// Absent for a legacy peer; truncation without this is not complete pagination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<FsListPageResult>,
}

/// The most entries one listing returns.
pub const FS_LIST_MAX_ENTRIES: u32 = 5_000;

/// The body of `fs.unwatch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsUnwatch {
    /// The watch to end.
    pub watch_id: Id,
}

/// The body of `fs.stat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsStat {
    /// The environment.
    pub environment_id: Id,
    /// An attached mount to look in, in place of the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// The path.
    pub path: String,
}

/// A version of a file, as far as it can be told without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct FileVersion {
    /// Its size in bytes.
    pub size: i64,
    /// When it was last changed, in nanoseconds since 1970.
    pub mtime_ns: i64,
}

/// The body of `fs.fetch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsFetch {
    /// The environment.
    pub environment_id: Id,
    /// An attached mount to read from, in place of the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// The file.
    pub path: String,
    /// The version that is wanted: a file that is another by now is not
    /// uploaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<FileVersion>,
    /// An additional per-file ceiling in bytes, narrowed by the environment host's
    /// own limit. Zero allows an empty file; negative limits are refused. When
    /// absent, the host's limit applies. Content travels through the command's
    /// streaming upload, never in the result frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<i64>,
}

/// The result of `fs.fetch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsFetchResult {
    /// The hash of the file's content, which the system now holds.
    pub hash: Hash,
    /// Its size in bytes.
    pub size: i64,
    /// When it was last changed, in nanoseconds since 1970.
    pub mtime_ns: i64,
}

/// What has to be at a path for a write to go ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExpectedHash {
    /// Nothing: the file must not exist yet.
    Absent,
    /// A file with this content.
    Is(Hash),
}

const NO_FILE: &str = "none";

impl Serialize for ExpectedHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Absent => serializer.serialize_str(NO_FILE),
            Self::Is(hash) => hash.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ExpectedHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        if text == NO_FILE {
            return Ok(Self::Absent);
        }
        text.parse().map(Self::Is).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ExpectedHash {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ExpectedHash".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let hash = generator.subschema_for::<Hash>();
        schemars::json_schema!({ "anyOf": [{ "const": NO_FILE }, hash] })
    }
}

/// The body of `fs.write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsWrite {
    /// Progress and cooperative cancellation owned by a durable workspace write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkReport>,
    /// The environment.
    pub environment_id: Id,
    /// The file to write.
    pub path: String,
    /// The content.
    pub blob: Hash,
    /// The permission bits to give it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    /// What has to be there now; absent, the file is written whatever is
    /// there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_hash: Option<ExpectedHash>,
}

/// The body of `fs.mkdir`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsMkdir {
    /// The environment.
    pub environment_id: Id,
    /// The directory to make.
    pub path: String,
}

/// The body of `fs.rename`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsRename {
    /// The environment.
    pub environment_id: Id,
    /// What is renamed.
    pub from: String,
    /// Its new path, which must not exist.
    pub to: String,
}

/// The body of `fs.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsRemove {
    /// The environment.
    pub environment_id: Id,
    /// What is removed.
    pub path: String,
    /// Whether a directory goes with what is in it.
    pub recursive: bool,
}

/// The body of `fs.copy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsCopy {
    /// The environment.
    pub environment_id: Id,
    /// What is copied.
    pub from: String,
    /// Where the copy goes, which must not exist.
    pub to: String,
}

/// The body of `fs.archive.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsArchiveList {
    /// The environment.
    pub environment_id: Id,
    /// The archive.
    pub path: String,
    /// A directory inside it to list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<String>,
    /// The most entries to return.
    pub limit: u32,
}

/// The body of `fs.archive.read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsArchiveRead {
    /// The environment.
    pub environment_id: Id,
    /// The archive.
    pub path: String,
    /// The entry to read.
    pub entry: String,
}

/// The result of `fs.archive.read`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsArchiveReadResult {
    /// The entry's content, which the system now holds.
    pub blob: BlobRef,
}

/// The body of `fs.search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsSearch {
    /// The environment.
    pub environment_id: Id,
    /// The directory to stay in.
    pub path: String,
    /// What to look for.
    pub query: String,
    /// Whether the query is a regular expression.
    pub regex: bool,
    /// Whether upper and lower case differ.
    pub case: bool,
    /// Which file names to look in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// The most matches to return.
    pub limit: u32,
}

/// The result of `fs.search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FsSearchResult {
    /// The lines that were found.
    pub matches: Vec<foreign::FileMatch>,
    /// Whether the search stopped before it had seen everything.
    pub truncated: bool,
    /// How many files it looked in.
    pub searched: i64,
}

/// Which paths of a workspace are meant. Patterns are anchored at the workspace root: `*` and
/// `?` match within a component, and a whole `**` component matches any number of directories.
/// A literal directory includes its descendants. The empty path is the root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Selection {
    /// The patterns that are taken, each matching directory with what is under it.
    pub include: Vec<String>,
    /// The paths under those that are left out.
    pub exclude: Vec<String>,
}

/// The existing workspace-path byte bound also bounds each selection pattern.
pub const SELECTION_PATTERN_MAX_BYTES: usize = 4096;

/// Checks the restricted selection grammar without accessing a filesystem or expanding a glob.
/// Other punctuation is literal; alternatives, classes, escaping and parent components have no
/// pattern meaning. The request body's existing limit bounds the number of patterns together.
pub fn validate_selection_pattern(pattern: &str) -> Result<(), &'static str> {
    if pattern.len() > SELECTION_PATTERN_MAX_BYTES || pattern.chars().any(char::is_control) {
        return Err("a pattern has at most 4096 bytes and no control characters");
    }
    if pattern.is_empty() {
        return Ok(());
    }
    if pattern.starts_with('/') || pattern.contains('\\') {
        return Err("a pattern is relative and uses forward slashes");
    }
    for component in pattern.split('/') {
        if matches!(component, "" | "." | "..") {
            return Err("a pattern has no empty, dot or parent component");
        }
        if component.contains("**") && component != "**" {
            return Err("a recursive wildcard occupies a whole component");
        }
    }
    Ok(())
}

/// The body of `fs.manifest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsManifest {
    /// The environment.
    pub environment_id: Id,
    /// The part of the workspace to list.
    pub selection: Selection,
}

/// The result of `fs.manifest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsManifestResult {
    /// The list, which the system now holds.
    pub manifest_blob: Hash,
    /// How many files it names.
    pub file_count: i64,
    /// How many bytes they have together.
    pub total_bytes: i64,
}

/// The body of `fs.snapshot`.
/// The largest selection an ordinary snapshot may take in one bounded scan.
pub const SNAPSHOT_MAX_ENTRIES: u64 = 250_000;
/// A selection scan stops after ten seconds and asks for narrower paths.
pub const SNAPSHOT_SCAN_SECONDS: u64 = 10;

/// The body of `fs.measure`: a bounded, read-only selection preflight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsMeasure {
    /// The attached environment that owns the workspace.
    pub environment_id: Id,
    /// Exactly the selection a snapshot would take, with ignore rules applied.
    pub selection: Selection,
}

/// Why a selection needs to be narrowed before a snapshot can be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectionRefusal {
    /// The selected tree has more than 250,000 entries.
    EntryLimit,
    /// Measuring the selected tree exceeded ten seconds.
    ScanTime,
    /// Some selected entries could not be measured; totals would be incomplete.
    Unreadable,
    /// A live workspace requires explicit paths rather than its whole tree.
    SelectionRequired,
}

/// One selected directory, with the size of its selected descendants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DirectoryMeasure {
    /// Path relative to the workspace, including the selected directory itself.
    pub path: String,
    /// Regular files below this directory that the selection includes.
    pub file_count: i64,
    /// Logical bytes in those files; links and directories add no bytes.
    pub total_bytes: i64,
}

/// The result of `fs.measure`; refused scans report lower bounds and never pretend to be complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsMeasureResult {
    /// Files, directories, links and other selected entries counted together.
    pub entry_count: i64,
    /// Regular files counted.
    pub file_count: i64,
    /// Directories counted.
    pub directory_count: i64,
    /// Logical bytes in the regular files counted.
    pub total_bytes: i64,
    /// At most ten largest selected directories, ordered by bytes, then path.
    pub largest_directories: Vec<DirectoryMeasure>,
    /// Whether every selected entry was measured within the limits.
    pub complete: bool,
    /// Why a snapshot cannot start, or null when the selection fits.
    pub refusal: Option<SelectionRefusal>,
}

/// The body of `fs.snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsSnapshot {
    /// The running job's reporting and configured time allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkReport>,
    /// The environment.
    pub environment_id: Id,
    /// The job whose progress the upload is reported for.
    pub job_id: Id,
    /// The part of the workspace to take.
    pub selection: Selection,
}

/// The result of `fs.snapshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsSnapshotResult {
    /// The list of what was taken.
    pub manifest_blob: Hash,
    /// How many files it names.
    pub file_count: i64,
    /// How many bytes they have together.
    pub total_bytes: i64,
    /// How many bytes had to be uploaded.
    pub uploaded_bytes: i64,
    /// How many files are listed without their content, because they
    /// could not be read or are too large.
    pub missing: i64,
}

/// Why files are written into a workspace from stored content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MaterializePurpose {
    /// They are a workspace, or a part of one, that moves here.
    Move,
    /// Exact saved files, published once into an unused directory by a durable job.
    Exact,
    /// They came with a message to an agent, or were shared with it.
    Delivery,
}

/// The body of `fs.materialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsMaterialize {
    /// The running job's reporting and configured time allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkReport>,
    /// The environment.
    pub environment_id: Id,
    /// The job whose progress the writing is reported for; `null` when
    /// it belongs to none.
    pub job_id: Option<Id>,
    /// The list of the files to write.
    pub manifest_blob: Hash,
    /// The directory they are written under.
    pub target_root: String,
    /// Why they are written, which decides what a machine's owner has to
    /// allow for it.
    pub purpose: MaterializePurpose,
}

/// The result of `fs.materialize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsMaterializeResult {
    /// How many files were written.
    pub files: i64,
    /// How many bytes were written.
    pub bytes: i64,
    /// How many files were already there with the right content.
    pub skipped: i64,
}

/// The body of `git.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitStatus {
    /// The environment.
    pub environment_id: Id,
    /// The repository's directory, relative to the workspace; omitted means the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Also watches this directory until fs.unwatch names the same stable identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_id: Option<Id>,
}

/// The body of `git.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitDiff {
    /// The environment.
    pub environment_id: Id,
    /// The path to keep to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether what is staged is compared, and not the working tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<bool>,
}

/// The result of `git.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitDiffResult {
    /// The differences, as a unified diff.
    pub text: String,
    /// Whether there was more than is returned.
    pub truncated: bool,
}

// ---------------------------------------------------------------- bodies: processes

/// The body of `proc.list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcList {
    /// The environment.
    pub environment_id: Id,
}

/// The result of `proc.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProcListResult {
    /// The moment the list was taken.
    pub as_of: Time,
    /// The processes of the environment.
    pub processes: Vec<foreign::ProcessInfo>,
}

/// A signal a process may be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum Signal {
    /// Interrupt, as a key press would.
    Int,
    /// Ask it to end.
    Term,
    /// End it.
    Kill,
}

/// The body of `proc.signal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcSignal {
    /// The environment.
    pub environment_id: Id,
    /// The process.
    pub pid: i64,
    /// When that process started, so that another one which got the same
    /// number since is never signalled.
    pub started_at: Time,
    /// The signal.
    pub signal: Signal,
}

/// The body of `proc.suspend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcSuspend {
    /// The environment.
    pub environment_id: Id,
}

/// The body of `proc.resume`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcResume {
    /// The environment.
    pub environment_id: Id,
}

/// The result of `proc.suspend` and of `proc.resume`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcHeldResult {
    /// How many processes were held still, or let go.
    pub processes: i64,
}

/// The body of `watch.set`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WatchSet {
    /// The environment.
    pub environment_id: Id,
    /// Whether what it uses is reported.
    pub stats: bool,
    /// Whether its processes are reported.
    pub procs: bool,
    /// Whether native scoped port changes are reported while somebody follows them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ports: bool,
    /// Whether shell foreground changes are reported while their list is shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terminals: bool,
    /// How often, in milliseconds.
    pub interval_ms: u32,
}

// ---------------------------------------------------------------- bodies: hosts

/// The body of `node.runtime.create`: the unit to make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct NodeRuntimeCreate(pub foreign::RuntimeSpec);

/// The result of `node.runtime.create`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeRuntimeCreateResult {
    /// The driver's own reference for what it made.
    pub unit: foreign::UnitRef,
}

/// The body of `node.runtime.start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeRuntimeStart {
    /// The runtime.
    pub runtime_id: Id,
}

/// The body of `node.network.apply`, supplied by the system to the registered host node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeNetworkApply {
    /// The runtime whose policy is installed.
    pub runtime_id: Id,
    /// The mode retained across every later Start and restart.
    pub required_mode: orch_api_contract::environment::EgressMode,
    /// Exact other host names after every explicit list has narrowed them.
    pub allowed_hosts: Vec<String>,
    /// Safe transport references; the node derives the actual trusted endpoints from its configuration.
    pub always: Vec<orch_api_contract::environment::NetworkTransport>,
    /// The revision of the trusted transport references.
    pub always_revision: String,
    /// The revision of the requested mode and other host names.
    pub revision: String,
}

/// The body of `node.runtime.stop`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeMountsApply {
    /// The runtime on this registered node.
    pub runtime_id: Id,
    /// The full desired array of the driver's MountSpec, including this host's locations.
    pub mounts: Json,
}

/// Whether the current unit accepted the complete live change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeMountsApplyResult {
    /// False leaves the desired change pending for recreate, with the reason below.
    pub applied: bool,
    /// The names, targets and actual modes of the attached set; never host paths.
    pub mounts: Json,
    /// Why this unit cannot apply the change live.
    pub reason: Option<String>,
}

/// Desired capabilities for the owned runtime, frozen against its prepared template boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeCapabilitiesApply {
    /// The registered runtime on this node.
    pub runtime_id: Id,
    /// CapabilityState of the driver contract, without enrollment or registry secrets.
    pub capabilities: Json,
}

/// The verified result of a same-unit environment grant change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeCapabilitiesApplyResult {
    /// True only after every helper operation succeeds.
    pub applied: bool,
    /// Withdrawal restarted the same unit to close retained handles.
    pub restarted: bool,
    /// Why the safe change remains pending for recreation.
    pub reason: Option<String>,
}

/// Native live limits for an existing owned runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeResourcesApply {
    /// The registered runtime on this node.
    pub runtime_id: Id,
    /// Resources of the driver contract; a changed fixed disk limit is refused for recreation.
    pub resources: Json,
}

/// The body of `node.runtime.stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeRuntimeStop {
    /// The runtime.
    pub runtime_id: Id,
    /// Why it stops, as the host's operator hooks are told it: a recreate, a move or a removal; absent for an
    /// ordinary stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<orch_driver_contract::runtime::UnitReason>,
}

/// The body of `node.runtime.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeRuntimeRemove {
    /// The runtime.
    pub runtime_id: Id,
    /// Whether only the unit is removed, leaving every owned volume intact.
    #[serde(default = "keep_runtime_volumes")]
    pub keep_volumes: bool,
    /// Frozen owned volume references for explicit purge or obsolete-copy cleanup.
    #[serde(default)]
    pub volume_refs: Vec<Json>,
}

fn keep_runtime_volumes() -> bool {
    true
}

/// The body of `node.volume.ensure`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeVolumeEnsure {
    /// The driver that keeps the volume.
    pub driver: String,
    /// The volume's name.
    pub name: String,
    /// The storage of the host it is made on; `null` for the host's
    /// default.
    pub storage: Option<String>,
    /// The marks it carries.
    pub labels: BTreeMap<String, String>,
}

/// The owned volume checked before recreating a lost environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeVolumeInspect {
    /// The driver holding it.
    pub driver: String,
    /// The expected volume name.
    pub name: String,
    /// Its expected storage.
    pub storage: Option<String>,
    /// The environment that must own it on this system.
    pub environment_id: Id,
}

/// A matching restored volume and its measured logical size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeVolumeInspectResult {
    /// Whether the volume is present with the exact expected ownership.
    pub exists: bool,
    /// Measured logical bytes, or null when the driver cannot measure them.
    pub bytes: Option<i64>,
}

/// The body of `node.runtime.export`: the files below `path` of the unit's state (its data directory), as one archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeRuntimeExport {
    /// The runtime whose unit keeps the files; it is stopped.
    pub runtime_id: Id,
    /// The directory, relative to the unit's state, such as `harness/<provider id>`; never climbing out of it.
    pub path: String,
}

/// The result of `node.runtime.export`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeRuntimeExportResult {
    /// The archive put up, or `null` where the unit keeps nothing there.
    pub archive: Option<BlobRef>,
}

/// The body of `node.runtime.import`: the files of an archive `node.runtime.export` put up, put below `path` of the
/// unit's state with the owners and modes they had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeRuntimeImport {
    /// The runtime whose unit takes the files; it has not run yet.
    pub runtime_id: Id,
    /// The directory, relative to the unit's state, that the archive's files go below.
    pub path: String,
    /// The archive.
    pub archive: BlobRef,
}

/// The before-change measurement of one owned runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeKeepEstimate {
    pub runtime_id: Id,
}

/// One unit whose limits and current use are read without changing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeResourcesInspect {
    pub runtime_id: Id,
}

/// The body of `node.resources.list`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeResourcesList {}

/// The exact node inventory entry to remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeResourcesRemove {
    /// The object's kind.
    pub kind: orch_driver_contract::resources::ResourceKind,
    /// Its opaque node inventory identifier.
    pub id: String,
    /// A deliberate plan may stop an owned unit first; caches and automatic cleanup never set this.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stop_first: bool,
    /// A cleanup rule removes it only if this observed use has not changed since its plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_last_used_at: Option<Time>,
}

impl NodeResourcesRemove {
    /// Selects an exact object for deliberate removal, retaining all native in-use checks.
    #[must_use]
    pub fn new(resource: orch_driver_contract::resources::ResourceRef) -> Self {
        Self {
            kind: resource.kind,
            id: resource.id,
            stop_first: false,
            expected_last_used_at: None,
        }
    }

    /// The object's reference without the conditional use guard.
    #[must_use]
    pub fn reference(&self) -> orch_driver_contract::resources::ResourceRef {
        orch_driver_contract::resources::ResourceRef {
            kind: self.kind,
            id: self.id.clone(),
        }
    }
}

/// A snapshot made by the durable job which owns this next generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeKeepFiles {
    pub runtime_id: Id,
    pub environment_id: Id,
    pub generation: i64,
    pub capture: Id,
}

/// An image operation remains possible after the corresponding unit was removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeKeptImage {
    pub driver: String,
    pub image: orch_driver_contract::kept::KeptImage,
}

/// The same image named for its export: a command body of its own, since each command's body is its own type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeKeptImageExport {
    pub driver: String,
    pub image: orch_driver_contract::kept::KeptImage,
}

/// A streamed archive, verified by content and then by its exact ownership labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeKeptImport {
    pub driver: String,
    pub image: orch_driver_contract::kept::KeptImage,
    pub archive: BlobRef,
}

/// The controller's complete retained set for the exact node inventory it answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeKeptSweep {
    pub epoch: u64,
    pub owners: Vec<orch_driver_contract::kept::KeptOwner>,
}

/// Whether a path given relative to a unit's state stays inside it: plain names, no `..`, nothing absolute.
/// A selector and its frozen physical reference; the source node checks that its runtime owns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectedVolume {
    /// `home` or `private:<mount id>`; never workspace, state or a backing mount location.
    pub selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    /// An already owned volume on this host.
    pub volume: orch_driver_contract::runtime::VolumeRef,
}

/// A read-only check of the target driver's whole-volume transfer implementation and trusted helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeVolumeCarryCheck {
    /// The target driver.
    pub driver: String,
}

/// A reviewable reason when the target cannot preserve a promised volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeVolumeCarryCheckResult {
    /// Whether import preserves the complete Unix volume metadata on this host.
    pub available: bool,
    /// Why it cannot, or null when available.
    pub reason: Option<String>,
}

/// Old-host measurement before any hold, successor or transfer is made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeRuntimeVolumesMeasure {
    /// The job and phase receiving live reports, when run in the background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkReport>,
    /// The old runtime whose volumes are read.
    pub runtime_id: Id,
    /// Individual volumes to measure, including ones the plan leaves behind.
    pub volumes: Vec<SelectedVolume>,
}

/// One measured selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeasuredVolume {
    /// The measured volume.
    pub selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    /// Its file count and bytes.
    #[serde(flatten)]
    pub measure: orch_driver_contract::volumes::VolumeMeasure,
}

/// All requested measurements, made on the old host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeRuntimeVolumesMeasureResult {
    /// One measurement for each requested selector.
    pub volumes: Vec<MeasuredVolume>,
}

/// A consistent archive of each selected volume, using native suspension or the declared driver-owned snapshot/quiescing path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeRuntimeVolumesExport {
    /// The running job's reporting and timeout settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkReport>,
    /// The old runtime; it stays available after an export or failed fork.
    pub runtime_id: Id,
    /// Exactly the whole owned volumes the reviewed plan carries.
    pub volumes: Vec<SelectedVolume>,
    /// Maximum size of each blob part, chosen below the system's blob limit and the node's scratch bound.
    pub part_bytes: i64,
}

/// At most one such file is staged by each direction of a volume stream.
pub const VOLUME_ARCHIVE_PART_MAX_BYTES: i64 = 512 * 1024 * 1024;

/// An ordered stream of checked blob parts, without a whole-volume blob-size or memory limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VolumeArchive {
    /// Parts in stream order, including repeated hashes when their bytes repeat.
    pub parts: Vec<BlobRef>,
    /// Their combined size, checked before importing.
    pub size: i64,
}

/// A metadata-preserving whole-volume archive in the blob store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportedVolume {
    /// The source selector, used to match the target's distinct owned volume.
    pub selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    /// The archive, transferred in bounded chunks by the existing content protocol.
    pub archive: VolumeArchive,
}

/// Archives ready to be imported; no backing mount's data is included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeRuntimeVolumesExportResult {
    /// Exactly one archive per selected volume, even if it is empty.
    pub volumes: Vec<ExportedVolume>,
}

/// Import into a fresh owned volume before the target unit is created and its overlay first mounts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeRuntimeVolumeImport {
    /// The running job's reporting and timeout settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkReport>,
    /// A provider-owned harness home uses its provider label, rather than an environment label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<Id>,
    /// The planned successor runtime, used as the command's serial key.
    pub runtime_id: Id,
    /// The driver on the new host.
    pub driver: String,
    /// The target environment, whose ownership label must match the existing target volume.
    pub environment_id: Id,
    /// Its distinct target volume for this selector.
    pub selected: SelectedVolume,
    /// The corresponding whole-volume archive.
    pub archive: VolumeArchive,
}

/// A mount's data on the machine that keeps it, named exactly: a directory that exists there, or a volume Orch made
/// for that mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MountData {
    /// A directory on the machine, which has to lie inside its `environment.roots`; it is only ever read.
    Path {
        /// The directory.
        path: String,
    },
    /// A volume made for the mount: `orch-mount-<mount id>`, or that name with a suffix of its own.
    Volume {
        /// The volume's name.
        name: String,
        /// The host's storage it is on; `null` for its default.
        storage: Option<String>,
    },
}

impl MountData {
    /// The serial key a transfer of it is ordered by: the directory or the volume's name.
    #[must_use]
    pub fn key(&self) -> SerialKey {
        match self {
            Self::Path { path } => SerialKey::Volume(path.clone()),
            Self::Volume { name, .. } => SerialKey::Volume(name.clone()),
        }
    }
}

/// Whether a volume's name is one Orch makes for this mount: its own name, or that name with a suffix.
#[must_use]
pub fn is_mount_volume(mount: Id, name: &str) -> bool {
    let own = format!("orch-mount-{mount}");
    name == own
        || name
            .strip_prefix(&own)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|suffix| {
                !suffix.is_empty()
                    && suffix
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            })
}

/// The body of `node.mount.measure`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeMountMeasure {
    /// The job and phase receiving live reports, when run in the background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkReport>,
    /// The driver that reads it.
    pub driver: String,
    /// The mount the data is of.
    pub mount_id: Id,
    /// Where it is.
    pub data: MountData,
}

/// The body of `node.mount.fingerprint`, with the same ownership and root checks as a measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeMountFingerprint {
    /// The driver that reads it.
    pub driver: String,
    /// The mount the data is of.
    pub mount_id: Id,
    /// Where it is.
    pub data: MountData,
    /// Live reporting and the command's cooperative filesystem-work allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkReport>,
}

/// The content fingerprint of one checked mount location; file contents never leave its machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeMountFingerprintResult {
    /// BLAKE3 of the canonical file tree, including paths and permission modes.
    pub fingerprint: Hash,
}

/// The body of `node.mount.export`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeMountExport {
    /// The running job's reporting and timeout settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkReport>,
    /// The driver that reads it.
    pub driver: String,
    /// The copy it is read for.
    pub job_id: Id,
    /// The mount the data is of.
    pub mount_id: Id,
    /// Where it is.
    pub data: MountData,
    /// Maximum size of each blob part, chosen below the system's blob limit and the node's scratch bound.
    pub part_bytes: i64,
}

/// The body of `node.mount.import`. What the volume held before is replaced, so a job keeps what has to be kept
/// first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeMountImport {
    /// The running job's reporting and timeout settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkReport>,
    /// The driver that keeps the volume.
    pub driver: String,
    /// The copy it is written for.
    pub job_id: Id,
    /// The mount the data is of; the volume carries its mark.
    pub mount_id: Id,
    /// The volume's name, one Orch makes for this mount.
    pub name: String,
    /// The host's storage it is on; `null` for its default.
    pub storage: Option<String>,
    /// The archive a `node.mount.export` made.
    pub archive: VolumeArchive,
}

/// A long-running command's reporting destination and configured time allowance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkReport {
    /// File count from the reviewed plan, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_total: Option<i64>,
    /// Logical bytes from the reviewed plan, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_total: Option<i64>,
    /// The durable job requesting the work.
    pub job_id: Id,
    /// Its currently running phase, preserved across the command's internal steps.
    pub phase: String,
    /// The configured allowance, in seconds, for this command and its helper.
    pub timeout_secs: u64,
}

/// Whether a path given relative to a unit's state stays inside it: plain names, no `..`, nothing absolute.
#[must_use]
pub fn state_path_is_inside(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// The body of `node.volume.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeVolumeRemove {
    /// The driver that keeps the volume.
    pub driver: String,
    /// The volume's name.
    pub name: String,
    /// The storage of the host it is on; `null` for the host's default.
    pub storage: Option<String>,
    /// The marks it carries.
    pub labels: BTreeMap<String, String>,
}

/// The body of `node.image.ensure`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeImageEnsure {
    /// The driver.
    pub driver: String,
    /// The template's spec, which names the source in the driver's own
    /// terms.
    pub spec: Json,
    /// The job whose progress the work is reported for; `null` when it
    /// belongs to none.
    pub job_id: Option<Id>,
}

/// A base image a build started from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeImageEnsureBase {
    /// The reference the build named.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The digest it resolved to.
    pub digest: String,
}

/// What a build was made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeImageEnsureBuilt {
    /// The commit a repository's reference resolved to; `null` without a
    /// repository.
    pub commit: Option<String>,
    /// The base images it started from.
    pub bases: Vec<NodeImageEnsureBase>,
}

/// The result of `node.image.ensure`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeImageEnsureResult {
    /// The digest of what is now on the host; `null` for a driver that
    /// has no such thing.
    pub digest: Option<String>,
    /// What a build was made from, when one was built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<NodeImageEnsureBuilt>,
    /// Programs observed in the available image; an absent entry means it was not checked.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub programs: BTreeMap<String, bool>,
}

/// The pool a unit is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    /// An environment threads work in.
    Environment,
    /// A provider's runtime.
    Provider,
}

/// The body of `node.template.import`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeTemplateImport {
    /// The driver that reads what is given.
    pub driver: String,
    /// The pool the template is for.
    pub kind: RuntimeKind,
    /// The format of what is given, as the driver names it.
    pub format: String,
    /// What the person gave for the fields that format asks for.
    pub values: Json,
}

/// How much a note of an import matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoteLevel {
    /// Something that was read.
    Info,
    /// Something that could not be carried over.
    Warning,
    /// Something that keeps the template from working.
    Blocker,
}

/// One note of an import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeTemplateImportNote {
    /// How much it matters.
    pub level: NoteLevel,
    /// What it says.
    pub text: String,
}

/// The result of `node.template.import`: a proposal, and nothing made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeTemplateImportResult {
    /// The proposed spec, in the driver's own terms.
    pub spec: Json,
    /// What was read, and what could not be carried over.
    pub notes: Vec<NodeTemplateImportNote>,
}

/// The body of `node.tool.ensure`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeToolEnsure {
    /// The job whose verified download progress is reported; null for cache preparation outside a job.
    #[serde(default)]
    pub job_id: Option<Id>,
    /// The tool's name.
    pub name: String,
    /// The version wanted.
    pub version: String,
    /// Where it is downloaded from.
    pub url: String,
    /// The SHA-256 the download must have, in hexadecimal.
    pub sha256: String,
}

/// The result of `node.tool.ensure`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeToolEnsureResult {
    /// Where the tool is on the host.
    pub path: String,
}

/// Where a program prints its interface for `node.tool.check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolInterface {
    /// On its standard output, as text: a name is there when it stands as a whole word.
    Text,
    /// As JSON Schema files written into the directory `{out}` names: `Type.member` is there when the definition
    /// `Type` has the property `member`, any other name when the files hold it as a string.
    Schema,
}

/// The body of `node.tool.check`: a build already in the cache is run once with `args`, in a scratch directory of
/// the cache that is its home and its working directory, with nothing of the node's environment but `PATH` and
/// with every proxy pointed at a closed port, for at most `timeout_secs`; what it printed is searched for `names`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeToolCheck {
    /// The tool's name.
    pub name: String,
    /// The version in the cache.
    pub version: String,
    /// The names the program may have in the build, in order of preference, at its top or in its `bin`.
    pub programs: Vec<String>,
    /// Its arguments; `{out}` stands for a directory it may write into.
    pub args: Vec<String>,
    /// Where it prints its interface.
    pub printed: ToolInterface,
    /// What is looked for.
    pub names: Vec<String>,
    /// How long it may run.
    pub timeout_secs: u32,
}

/// The result of `node.tool.check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeToolCheckResult {
    /// The names it lacks, in the order they were asked for.
    pub missing: Vec<String>,
}

/// A cached build to hold against the adapter's native behavior in an isolated unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeToolConformance {
    /// Harness kind and tool cache name.
    pub name: String,
    /// Exact immutable version to check.
    pub version: String,
    /// Candidate executable names in the cached build.
    pub programs: Vec<String>,
    /// Deadline for each case, from the system's settings.
    pub case_timeout_secs: u32,
    /// Deadline for the complete stage, from the system's settings.
    pub timeout_secs: u32,
}

/// Native evidence returned by the node's isolated runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeToolConformanceResult {
    /// Every native case, including failures and absent native evidence.
    pub cases: Vec<orch_api_contract::admin::HarnessConformanceCase>,
}

/// Where a skill's files are fetched from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SkillFetchSource {
    /// A Git repository at a branch, a tag or a commit, read by a Git that asks nothing of anybody.
    Git {
        /// The repository's address, of a scheme that needs no credential.
        repository: String,
        /// The branch, tag or commit.
        reference: String,
        /// The folder of the repository whose files are put up; `null` for all of it.
        folder: Option<String>,
    },
    /// An archive at an http or https address, put up as it is.
    Url {
        /// A public address, or a skill-owned secret reference in a stored command that becomes
        /// a value sealed to this machine when the link sends it.
        url: SecretOrText,
    },
}

/// The body of `node.skill.fetch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSkillFetch {
    /// Where the files come from.
    pub source: SkillFetchSource,
    /// The most bytes the archive put up may have; a larger one is refused as `too_large`.
    pub max_bytes: i64,
    /// Private source hosts the system operator permits for an archive download.
    #[serde(default)]
    pub allowed_private_hosts: Vec<String>,
}

/// The result of `node.skill.fetch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeSkillFetchResult {
    /// The archive put up: a tar of the repository's folder, or the archive found at the address.
    pub archive: BlobRef,
    /// For a repository, the commit the reference came to.
    pub commit: Option<String>,
}

/// The body of `node.probe`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct NodeProbe {}

/// A unit a host holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeProbeUnit {
    /// The runtime the unit is of.
    pub runtime_id: Id,
    /// The state its driver finds it in.
    pub state: String,
}

/// A tool in a host's cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeProbeTool {
    /// The tool's name.
    pub name: String,
    /// The version that is there.
    pub version: String,
}

/// The result of `node.probe`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeProbeResult {
    /// What the host has.
    pub capacity: NodeCapacity,
    /// What each driver it loaded declares, by the driver's name.
    pub drivers: BTreeMap<String, foreign::DriverDescriptor>,
    /// The places on it where volumes are made.
    pub storages: Vec<NodeStorage>,
    /// The units it holds for this system.
    pub units: Vec<NodeProbeUnit>,
    /// The tools in its cache.
    pub tools: Vec<NodeProbeTool>,
}

// ---------------------------------------------------------------- lifecycle

/// How far a command is, as the system records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandState {
    /// Made; the machine is not connected, or the frame is not sent yet.
    Queued,
    /// Handed to the gateway.
    Sent,
    /// The machine said that it has it.
    Accepted,
    /// The machine answered with a result.
    Succeeded,
    /// The machine answered with an error.
    Failed,
    /// Its deadline passed before an answer came.
    Expired,
    /// The system gave it up before an answer came.
    Cancelled,
}

/// Something that happens to a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandStep {
    /// It was handed to the gateway.
    Sent,
    /// The machine said that it has it.
    Acknowledged,
    /// The machine answered with a result.
    Succeeded,
    /// The machine answered with an error.
    Failed,
    /// Its deadline passed.
    DeadlinePassed,
    /// The system gave it up.
    Cancelled,
}

impl CommandState {
    /// Whether the command still waits for its answer. An open command
    /// is sent again whenever its machine's session becomes ready, and
    /// content is moved for a command only while it is open.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Queued | Self::Sent | Self::Accepted)
    }

    /// The state after `step`. A command only moves forward: one that
    /// has ended stays as it ended, whatever arrives afterwards, and
    /// sending it again takes nothing back.
    #[must_use]
    pub const fn after(self, step: CommandStep) -> Self {
        if !self.is_open() {
            return self;
        }
        match (self, step) {
            (Self::Queued, CommandStep::Sent) => Self::Sent,
            (_, CommandStep::Sent) => self,
            (_, CommandStep::Acknowledged) => Self::Accepted,
            (_, CommandStep::Succeeded) => Self::Succeeded,
            (_, CommandStep::Failed) => Self::Failed,
            (_, CommandStep::DeadlinePassed) => Self::Expired,
            (_, CommandStep::Cancelled) => Self::Cancelled,
        }
    }
}

/// What a machine's journal says about a command that arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Journalled {
    /// Nothing: it was never seen, or so long ago that it was forgotten.
    Never,
    /// It was written down and has not ended.
    Accepted,
    /// Its result was written down.
    Finished,
}

/// What a machine does with a command that arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Treatment {
    /// Write it down, say so, and run it.
    Run,
    /// Say again that it has it; the answer follows when it has ended.
    AcknowledgeAgain,
    /// Answer with the result that was written down, and run nothing.
    AnswerAgain,
    /// Leave it: the moment after which it may not be started is past.
    Skip,
}

/// What a machine does with a command that arrives at `now`, given what
/// its journal holds for that identifier. A command is run once however
/// often it arrives, and one that arrives too late is not started.
#[must_use]
pub fn treatment(journalled: Journalled, deadline: Time, now: Time) -> Treatment {
    match journalled {
        Journalled::Finished => Treatment::AnswerAgain,
        Journalled::Accepted => Treatment::AcknowledgeAgain,
        Journalled::Never if now > deadline => Treatment::Skip,
        Journalled::Never => Treatment::Run,
    }
}

/// Whether what a journal wrote down at `written` may be forgotten by
/// `now`.
#[must_use]
pub fn journal_lapsed(written: Time, now: Time) -> bool {
    const HOUR_MS: i64 = 3_600_000;
    now.unix_millis() - written.unix_millis() >= JOURNAL_KEEP_HOURS * HOUR_MS
}

/// The order in which a machine runs its commands: those that share a
/// key one at a time as they arrived, all others side by side.
#[derive(Debug, Clone, Default)]
pub struct SerialQueue {
    lines: BTreeMap<String, VecDeque<Id>>,
}

impl SerialQueue {
    /// An order with nothing in it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes in a command. `true`: it runs now. `false`: it waits for
    /// those of its key that came before it.
    pub fn arrive(&mut self, id: Id, key: Option<&str>) -> bool {
        let Some(key) = key else { return true };
        let line = self.lines.entry(key.to_owned()).or_default();
        line.push_back(id);
        line.len() == 1
    }

    /// Takes out a command that ended, or that was given up while it
    /// waited. Returns the command of its key that runs next, when this
    /// one was the one running and another waits.
    pub fn leave(&mut self, id: Id, key: Option<&str>) -> Option<Id> {
        let key = key?;
        let line = self.lines.get_mut(key)?;
        let place = line.iter().position(|waiting| *waiting == id)?;
        line.remove(place);
        let next = if place == 0 {
            line.front().copied()
        } else {
            None
        };
        if line.is_empty() {
            self.lines.remove(key);
        }
        next
    }

    /// How many commands wait or run under a key.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.values().map(VecDeque::len).sum()
    }

    /// Whether no command waits or runs under any key.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_legacy_unit_removal_keeps_files_and_an_explicit_purge_carries_its_frozen_refs() {
        let runtime = orch_types::new_id();
        let legacy: super::NodeRuntimeRemove =
            serde_json::from_value(serde_json::json!({"runtime_id":runtime})).unwrap();
        assert!(legacy.keep_volumes);
        let purge: super::NodeRuntimeRemove = serde_json::from_value(serde_json::json!({
            "runtime_id":runtime,"keep_volumes":false,"volume_refs":[{"name":"orch-ws-owned","driver":"docker"}]
        })).unwrap();
        assert!(!purge.keep_volumes);
        assert_eq!(purge.volume_refs.len(), 1);
        assert_eq!(
            serde_json::to_value(&purge).unwrap()["volume_refs"][0]["name"],
            "orch-ws-owned"
        );
    }
    use super::*;
    use serde_json::json;

    const THREAD: &str = "0199c1a0-0000-7000-8000-000000000001";
    const TURN: &str = "0199c1a0-0000-7000-8000-000000000002";
    const ENVIRONMENT_ID: &str = "0199c1a0-0000-7000-8000-000000000003";
    const PROVIDER_ID: &str = "0199c1a0-0000-7000-8000-000000000004";
    const AGENT_ID: &str = "0199c1a0-0000-7000-8000-000000000005";
    const SEGMENT: &str = "0199c1a0-0000-7000-8000-000000000006";
    const COMMAND: &str = "0199c1a0-0000-7000-8000-000000000007";
    const SECRET: &str = "0199c1a0-0000-7000-8000-000000000008";

    fn id(text: &str) -> Id {
        text.parse().unwrap()
    }

    fn time(text: &str) -> Time {
        text.parse().unwrap()
    }

    /// Reads `example` as a `T`, writes it again and expects the same JSON.
    fn round_trip<T: Serialize + DeserializeOwned>(example: Json) -> T {
        let read: T = serde_json::from_value(example.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), example);
        read
    }

    #[test]
    fn an_embeddings_request_and_its_vectors_travel_as_written_and_a_vector_survives_its_encoding()
    {
        let asked: ProviderEmbed = round_trip(json!({
            "provider_id": PROVIDER_ID, "protocol": "openai-embeddings", "model_name": "local",
            "inputs": ["title: none | text: 发布流程", "title: none | text: rollout"], "dimensions": 256
        }));
        assert_eq!(asked.inputs.len(), 2);
        assert_eq!(CommandKind::ProviderEmbed.as_str(), "provider.embed");
        let vector = vec![0.25_f32, -1.0, 3.5e-3, 0.0];
        let written = encode_vector(&vector);
        assert_eq!(decode_vector(&written), Some(vector.clone()));
        let answered: ProviderEmbedResult = serde_json::from_value(json!({
            "vectors": [written],
            "usage": {"input_tokens": 9, "cached_input_tokens": 0, "output_tokens": 0, "reasoning_tokens": 0, "total_tokens": 9},
            "request_ref": null
        }))
        .unwrap();
        assert_eq!(answered.usage.input_tokens, 9);
        assert_eq!(decode_vector(&answered.vectors[0]), Some(vector));
        assert_eq!(decode_vector("not base64!"), None);
        assert_eq!(
            decode_vector(&encode_vector(&[f32::NAN])),
            None,
            "a number that is none"
        );
    }

    #[test]
    fn directory_pages_carry_an_optional_observation_interval_without_changing_legacy_results() {
        let legacy = json!({"revision":Hash::of(b"observed rows"),"next_cursor":null,
            "as_of":"2026-10-08T17:04:09.456Z","expires_at":"2026-10-08T17:09:09.456Z"});
        let read: FsListPageResult = round_trip(legacy.clone());
        assert_eq!(read.observed_from, None);
        let mut current = legacy;
        current["observed_from"] = json!("2026-10-08T17:04:05.123Z");
        let read: FsListPageResult = round_trip(current);
        assert!(read.observed_from.is_some_and(|start| start < read.as_of));
    }

    #[test]
    fn file_fetch_keeps_the_legacy_wire_shape_and_can_narrow_the_streamed_upload() {
        let legacy: FsFetch = round_trip(json!({
            "environment_id": ENVIRONMENT_ID,
            "path": "output/report.pdf"
        }));
        assert_eq!(legacy.max_bytes, None);
        let bounded: FsFetch = round_trip(json!({
            "environment_id": ENVIRONMENT_ID,
            "path": "output/empty.txt",
            "max_bytes": 0
        }));
        assert_eq!(bounded.max_bytes, Some(0));
        assert_eq!(FsFetch::KIND.transfer(), Transfer::Upload);
        assert_eq!(FsFetch::KIND.key(), KeyKind::None);
    }

    fn session_open_example() -> Json {
        json!({
            "agent_id": AGENT_ID, "thread_id": THREAD, "segment_id": SEGMENT,
            "mode": "import_native",
            "native_session_ref": null,
            "archive": { "format": "codex-rollout-v1", "blob": Hash::of(b"rollout").to_string(), "size": 37309 },
            "history": null,
            "upto_turn_ordinal": 12,
            "workspace_root": "/workspace", "instruction_root_marker": true,
            "directory": "/workspace/web", "cwd": "/workspace/web/api",
            "mounts": [ { "name": "src", "path": "/data/src", "read_only": true } ],
            "model": "gpt-6-sol", "effort": "high", "params": { }, "approval_mode": "on_request",
            "sandbox_mode": "workspace",
            "available_models": [], "side_reading": null, "workspace_configuration": null,
            "subagent_limits": {"at_once": 8, "depth": 1},
            "instructions": "You are working inside Orch.",
            "memory": { "directory": "/workspace/web/.orch/memory", "kept_by": "agent" },
            "arrangement": "separated",
            "provider": { "path": "forwarded", "provider_id": PROVIDER_ID, "protocol": "openai-responses",
                          "ticket": { "$ticket": { "kind": "llm" } } },
            "exec": { "executor": "orch-exec", "tool_set": "claude-code", "environment_id": ENVIRONMENT_ID,
                      "generation": 3, "ticket": { "$ticket": { "kind": "mcp" } } },
            "mcp": [ { "name": "tmux", "ticket": { "$ticket": { "kind": "mcp" } } } ],
            "orch_tools": ["list_shared", "read_shared", "search_shared", "download_shared", "edit_document",
                           "send_message", "announce", "request_capability"],
            "raw_events": true, "raw_after": 42
        })
    }

    #[test]
    fn a_command_frame_reads_and_writes_as_the_protocol_has_it() {
        let cmd: Cmd = round_trip(json!({
            "id": COMMAND, "k": "turn.interrupt", "key": THREAD,
            "deadline": "2026-10-08T17:04:35.000Z", "b": { "thread_id": THREAD, "turn_id": TURN }
        }));
        assert_eq!(
            cmd.command().unwrap(),
            Command::TurnInterrupt(TurnInterrupt {
                thread_id: id(THREAD),
                turn_id: id(TURN)
            })
        );
        round_trip::<CmdAck>(json!({ "id": COMMAND }));
        round_trip::<CmdCancel>(json!({ "id": COMMAND }));
    }

    #[test]
    fn an_answer_is_a_result_or_an_error_and_says_which() {
        let done: CmdRes = round_trip(json!({ "id": COMMAND, "ok": true, "b": { "items": 14 } }));
        assert_eq!(
            done.result::<SessionResync>().unwrap(),
            Ok(SessionResyncResult { items: 14 })
        );
        let failed: CmdRes = round_trip(json!({
            "id": COMMAND, "ok": false,
            "err": { "code": "session_not_found", "detail": "the harness was restarted", "retryable": false }
        }));
        let error = failed.result::<SessionResync>().unwrap().unwrap_err();
        assert_eq!(error.known(), Some(ErrorCode::SessionNotFound));
        assert!(serde_json::from_value::<CmdRes>(json!({ "id": COMMAND, "ok": false })).is_err());
    }

    #[test]
    fn a_code_outside_the_catalogue_is_carried_as_it_came() {
        let failed: CmdRes = serde_json::from_value(json!({
            "id": COMMAND, "ok": false, "err": { "code": "import_native_failed", "detail": "", "retryable": false }
        }))
        .unwrap();
        let error = failed.outcome.unwrap_err();
        assert_eq!(error.code, "import_native_failed");
        assert_eq!(error.known(), None);
    }

    #[test]
    fn a_kind_nobody_knows_still_leaves_a_frame_that_can_be_answered() {
        let cmd: Cmd = serde_json::from_value(json!({
            "id": COMMAND, "k": "fs.teleport", "key": null, "deadline": "2026-10-08T17:04:35.000Z", "b": {}
        }))
        .unwrap();
        assert_eq!(cmd.id, id(COMMAND));
        assert!(
            matches!(cmd.command(), Err(CatalogueError::UnknownKind(kind)) if kind == "fs.teleport")
        );
    }

    #[test]
    fn a_body_that_is_not_its_kinds_is_told_apart_from_an_unknown_kind() {
        let wrong = Command::decode(
            "fs.remove",
            json!({ "environment_id": ENVIRONMENT_ID, "path": "tmp" }),
        );
        assert!(matches!(
            wrong,
            Err(CatalogueError::Body {
                kind: "fs.remove",
                ..
            })
        ));
    }

    #[test]
    fn every_kind_is_found_again_by_the_name_it_is_written_with() {
        for kind in CommandKind::ALL {
            assert_eq!(kind.as_str().parse::<CommandKind>().unwrap(), *kind);
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
    }

    #[test]
    fn a_session_opens_with_everything_it_is_set_up_with() {
        let open: SessionOpen = round_trip(session_open_example());
        assert_eq!(open.mode, SessionMode::ImportNative);
        assert_eq!(open.raw_after, 42);
        assert_eq!(open.exec.tool_set.as_deref(), Some("claude-code"));
        assert_eq!(open.orch_tools.len(), 8);
        assert_eq!(open.orch_tools[5], OrchTool::SendMessage);
        assert!(matches!(
            open.provider,
            SessionOpenProvider::Forwarded { .. }
        ));
    }

    #[test]
    fn a_session_whose_harness_runs_beside_its_tools_names_no_executor_and_no_ticket() {
        let mut example = session_open_example();
        example["arrangement"] = json!("credential_shared");
        example["provider"] =
            json!({ "path": "shared", "provider_id": PROVIDER_ID, "form": "access_token" });
        example["exec"] =
            json!({ "executor": null, "environment_id": ENVIRONMENT_ID, "generation": 3 });
        example["mode"] = json!("new");
        example["archive"] = json!(null);
        example["upto_turn_ordinal"] = json!(null);
        example["memory"] = json!(null);
        let open: SessionOpen = round_trip(example);
        assert_eq!(open.exec.executor, None);
        assert_eq!(open.exec.ticket, None);
        assert_eq!(
            open.provider,
            SessionOpenProvider::Shared {
                provider_id: id(PROVIDER_ID),
                form: "access_token".into()
            }
        );
    }

    #[test]
    fn a_stored_command_holds_references_and_the_sent_one_holds_none() {
        let mut body = json!({
            "provider_id": PROVIDER_ID, "protocols": ["openai-responses"],
            "upstreams": [ { "protocol": "openai-responses", "base_url": "https://api.example.com/v1", "options": {} } ],
            "credential": { "$secret": SECRET },
            "secrets": { "x-gateway-key": { "$secret": SECRET } }
        });
        let stored: ProviderStart = round_trip(body.clone());
        assert_eq!(stored.credential, Some(Secret::Ref(id(SECRET))));
        assert!(has_references(&body));

        let mut asked = Vec::new();
        resolve_references::<()>(&mut body, &mut |found| {
            asked.push(matches!(found, Reference::Secret(secret) if secret == id(SECRET)));
            Ok("c2VhbGVk".to_owned())
        })
        .unwrap();
        assert_eq!(asked, [true, true]);
        assert!(!has_references(&body));
        let sent: ProviderStart = round_trip(body);
        assert_eq!(sent.credential, Some(Secret::Sealed("c2VhbGVk".into())));
    }

    #[test]
    fn a_chosen_source_travels_with_its_secrets_sealed_and_without_one_the_default_is_read() {
        let today: ProviderRefresh = round_trip(json!({ "provider_id": PROVIDER_ID }));
        assert_eq!(today.facts, None);
        let mut body = json!({
            "provider_id": PROVIDER_ID,
            "facts": {
                "by": "address",
                "address": { "url": "https://plan.acme.example/orch", "auth": "bearer", "send_credential": false },
                "secrets": { "token": { "$secret": SECRET } },
                "authorization": { "$secret": SECRET }
            }
        });
        let stored: ProviderRefresh = round_trip(body.clone());
        assert_eq!(
            stored.facts.as_ref().map(|spec| spec.by),
            Some(orch_provider_contract::SourceBy::Address)
        );
        assert!(has_references(&body), "a stored command holds no secret");
        resolve_references::<()>(&mut body, &mut |_| Ok("c2VhbGVk".to_owned())).unwrap();
        let sent: ProviderRefresh = round_trip(body);
        assert_eq!(
            sent.facts.and_then(|spec| spec.authorization),
            Some(orch_provider_contract::SourceSecret::Sealed(
                "c2VhbGVk".into()
            ))
        );
        let models: ProviderDiscoverModels =
            round_trip(json!({ "provider_id": PROVIDER_ID, "models": { "by": "none" } }));
        assert_eq!(
            models.models.map(|spec| spec.by),
            Some(orch_provider_contract::SourceBy::None)
        );
        let reset: ProviderResetConsume = round_trip(json!({
            "provider_id": PROVIDER_ID, "request_id": SECRET, "reset_id": "credit-1", "available_count": 1,
            "facts": { "by": "program", "name": "acme-plan" }
        }));
        assert_eq!(
            reset.facts.and_then(|spec| spec.name).as_deref(),
            Some("acme-plan")
        );
        let start: ProviderStart = round_trip(json!({
            "provider_id": PROVIDER_ID, "protocols": [], "upstreams": [], "credential": null, "secrets": {},
            "presenter": { "by": "program", "name": "acme-token" }
        }));
        assert!(start.presenter.is_some());
    }

    #[test]
    fn a_ticket_that_was_asked_for_is_sent_as_the_signed_ticket() {
        let mut body = session_open_example();
        resolve_references::<()>(&mut body, &mut |found| match found {
            Reference::Ticket(wanted) => Ok(format!("v1.{}.sig", wanted["kind"].as_str().unwrap())),
            Reference::Secret(_) => Err(()),
        })
        .unwrap();
        let open: SessionOpen = round_trip(body);
        assert_eq!(open.mcp[0].ticket, TicketSlot::Signed("v1.mcp.sig".into()));
        assert_eq!(
            open.exec.ticket,
            Some(TicketSlot::Signed("v1.mcp.sig".into()))
        );
    }

    #[test]
    fn a_reference_that_cannot_be_resolved_stops_the_command_from_being_sent() {
        let mut body =
            json!({ "api_key": { "$secret": SECRET }, "ticket": { "$ticket": { "kind": "llm" } } });
        let outcome = resolve_references(&mut body, &mut |found| match found {
            Reference::Secret(_) => Err("the secret was deleted"),
            Reference::Ticket(_) => Ok("v1.a.b".to_owned()),
        });
        assert_eq!(outcome, Err("the secret was deleted"));
    }

    #[test]
    fn a_tool_server_is_started_from_a_given_definition_or_from_the_machines_own_copy() {
        let given: McpStart = round_trip(json!({
            "environment_id": ENVIRONMENT_ID, "name": "tmux",
            "spec": { "transport": "stdio", "command": ["tmux-mcp-rs"],"url":null,
                "env":{},"secret_env":[],"working_dir":"workspace","startup_timeout_secs":30,
                "tools_allow":null,"tools_deny":null,"requires":{"commands":[],"capabilities":[]} },
            "secrets": {}
        }));
        assert!(matches!(given, McpStart::Given(_)));
        let local: McpStart = round_trip(json!({
            "environment_id": ENVIRONMENT_ID, "name": "github", "server_id": SECRET, "local": true
        }));
        assert!(matches!(local, McpStart::Local(_)));
        let neither = json!({ "environment_id": ENVIRONMENT_ID, "name": "github", "server_id": SECRET, "local": false });
        assert!(serde_json::from_value::<McpStart>(neither).is_err());
    }

    #[test]
    fn a_write_may_expect_a_version_or_expect_nothing_to_be_there() {
        let hash = Hash::of(b"before");
        let write: FsWrite = round_trip(json!({
            "environment_id": ENVIRONMENT_ID, "path": "src/main.rs", "blob": Hash::of(b"after").to_string(),
            "expected_hash": hash.to_string()
        }));
        assert_eq!(write.expected_hash, Some(ExpectedHash::Is(hash)));
        let create: FsWrite = round_trip(json!({
            "environment_id": ENVIRONMENT_ID, "path": "src/new.rs", "blob": Hash::of(b"after").to_string(),
            "mode": 420, "expected_hash": "none"
        }));
        assert_eq!(create.expected_hash, Some(ExpectedHash::Absent));
    }

    #[test]
    fn bodies_and_results_of_each_role_read_and_write_as_the_catalogue_has_them() {
        round_trip::<PtyCloseResult>(json!({
            "exit_code": null, "bytes_out": 5120, "kept": { "hash": Hash::of(b"screen").to_string(), "size": 5120 }
        }));
        round_trip::<LeaseOpen>(json!({
            "lease_id": COMMAND, "provider_id": PROVIDER_ID, "thread_id": THREAD, "mode": "forwarded",
            "protocol": "openai-responses", "allowed_models": ["gpt-6-sol"],
            "allowance": { "tokens": 250000, "credits": { "amount": "12.5", "rate": {
                "input_per_mtok": "2","cached_input_per_mtok":"0.2","output_per_mtok":"10" } } }
        }));
        round_trip::<SealedCredential>(json!({
            "form": "access_token", "credential": { "$sealed": "c2VhbGVk" }, "expires_at": "2026-10-08T18:04:05.000Z"
        }));
        round_trip::<EnvAttach>(json!({
            "environment_id": ENVIRONMENT_ID, "workspace_root": null, "ignore": ["node_modules/"], "index": "full",
            "mounts": null, "variables": { "CI": "1" }, "secrets": { "NPM_TOKEN": { "$sealed": "c2VhbGVk" } }
        }));
        round_trip::<FsSearch>(json!({
            "environment_id": ENVIRONMENT_ID, "path": "src", "query": "fn main", "regex": false, "case": true,
            "glob": "*.rs", "limit": 200
        }));
        round_trip::<ProcSignal>(json!({
            "environment_id": ENVIRONMENT_ID, "pid": 4711, "started_at": "2026-10-08T17:00:00.000Z", "signal": "TERM"
        }));
        round_trip::<NodeImageEnsureResult>(json!({
            "digest": "sha256:0a1b", "built": { "commit": null, "bases": [ { "ref": "debian:bookworm-slim", "digest": "sha256:9f" } ] }
        }));
        round_trip::<ProviderLoginStart>(json!({
            "provider_id": PROVIDER_ID, "agent_id": AGENT_ID, "method": "access_token",
            "values": { "token": { "$secret": SECRET }, "expires_at": "2027-10-03" }, "config": {}
        }));
    }

    #[test]
    fn a_runtime_is_created_from_its_spec_and_nothing_around_it() {
        let spec = json!({ "runtime_id": COMMAND, "kind": "environment", "driver": "docker",
            "name":"orch-environment-000000000004", "spec":{"image":"example/environment:1"},
            "resources":{},"roles":["environment"],"gateway_url":"wss://gateway.example",
            "network_policy":serde_json::to_value(orch_driver_contract::network::NetworkPolicy::default()).unwrap(),
            "enroll_token":{"$sealed":"sealed-for-the-node"},"ca_cert":"certificate",
            "tools":[],"volumes":{},"mounts":[],"env":{},"labels":{} });
        let command = Command::decode("node.runtime.create", spec.clone()).unwrap();
        assert_eq!(command.body().unwrap(), spec);
        assert_eq!(command.kind().key(), KeyKind::Runtime);
    }

    #[test]
    fn a_frame_is_made_from_a_body_with_its_kind_and_its_key() {
        let body = FsStat {
            environment_id: id(ENVIRONMENT_ID),
            mount: None,
            path: "README.md".into(),
        };
        let cmd = Cmd::new(
            id(COMMAND),
            &SerialKey::None,
            time("2026-10-08T17:04:20.000Z"),
            &body,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&cmd).unwrap(),
            json!({ "id": COMMAND, "k": "fs.stat", "key": null, "deadline": "2026-10-08T17:04:20.000Z",
                    "b": { "environment_id": ENVIRONMENT_ID, "path": "README.md" } })
        );
    }

    #[test]
    fn commands_for_one_thing_share_a_key_and_commands_for_two_do_not() {
        let environment = SerialKey::Environment(id(ENVIRONMENT_ID));
        assert_eq!(environment.wire(), Some(ENVIRONMENT_ID.to_owned()));
        assert_ne!(
            environment.wire(),
            SerialKey::Environment(id(THREAD)).wire()
        );
        assert_eq!(SerialKey::None.wire(), None);
        let harness = SerialKey::Harness {
            provider_id: id(PROVIDER_ID),
            agent_id: id(AGENT_ID),
        };
        assert_eq!(harness.wire(), Some(format!("{PROVIDER_ID}:{AGENT_ID}")));
        assert_ne!(
            SerialKey::Tool("machine".into()).wire(),
            SerialKey::Machine.wire()
        );
        assert_eq!(CommandKind::FsWrite.key(), environment.kind());
    }

    #[test]
    fn commands_of_one_key_run_one_at_a_time_in_the_order_they_came() {
        let (first, second, third) = (id(THREAD), id(TURN), id(SEGMENT));
        let mut queue = SerialQueue::new();
        assert!(queue.arrive(first, Some("env")));
        assert!(!queue.arrive(second, Some("env")));
        assert!(!queue.arrive(third, Some("env")));
        assert_eq!(queue.leave(first, Some("env")), Some(second));
        assert_eq!(queue.leave(second, Some("env")), Some(third));
        assert_eq!(queue.leave(third, Some("env")), None);
        assert!(queue.is_empty());
    }

    #[test]
    fn commands_of_different_keys_and_commands_without_one_do_not_wait() {
        let mut queue = SerialQueue::new();
        assert!(queue.arrive(id(THREAD), Some("a")));
        assert!(queue.arrive(id(TURN), Some("b")));
        assert!(queue.arrive(id(SEGMENT), None));
        assert!(queue.arrive(id(COMMAND), None));
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn a_command_given_up_while_it_waits_lets_the_others_keep_their_turn() {
        let (first, second, third) = (id(THREAD), id(TURN), id(SEGMENT));
        let mut queue = SerialQueue::new();
        queue.arrive(first, Some("env"));
        queue.arrive(second, Some("env"));
        queue.arrive(third, Some("env"));
        assert_eq!(queue.leave(second, Some("env")), None);
        assert_eq!(queue.leave(first, Some("env")), Some(third));
    }

    #[test]
    fn a_command_that_arrives_again_is_answered_from_the_journal_and_not_run_again() {
        let deadline = time("2026-10-08T17:05:00.000Z");
        let now = time("2026-10-08T17:04:00.000Z");
        assert_eq!(treatment(Journalled::Never, deadline, now), Treatment::Run);
        assert_eq!(
            treatment(Journalled::Accepted, deadline, now),
            Treatment::AcknowledgeAgain
        );
        assert_eq!(
            treatment(Journalled::Finished, deadline, now),
            Treatment::AnswerAgain
        );
    }

    #[test]
    fn a_command_whose_deadline_has_passed_is_not_started_but_its_kept_answer_is_still_given() {
        let deadline = time("2026-10-08T17:05:00.000Z");
        let late = time("2026-10-08T17:05:00.001Z");
        assert_eq!(
            treatment(Journalled::Never, deadline, late),
            Treatment::Skip
        );
        assert_eq!(
            treatment(Journalled::Never, deadline, deadline),
            Treatment::Run
        );
        assert_eq!(
            treatment(Journalled::Finished, deadline, late),
            Treatment::AnswerAgain
        );
    }

    #[test]
    fn a_journal_forgets_a_command_after_a_day() {
        let written = time("2026-10-08T17:00:00.000Z");
        assert!(!journal_lapsed(written, time("2026-10-09T16:59:59.999Z")));
        assert!(journal_lapsed(written, time("2026-10-09T17:00:00.000Z")));
    }

    #[test]
    fn a_command_moves_forward_and_stays_as_it_ended() {
        let sent = CommandState::Queued.after(CommandStep::Sent);
        assert_eq!(sent, CommandState::Sent);
        let accepted = sent.after(CommandStep::Acknowledged);
        assert_eq!(accepted, CommandState::Accepted);
        assert_eq!(accepted.after(CommandStep::Sent), CommandState::Accepted);
        let done = accepted.after(CommandStep::Succeeded);
        assert_eq!(done, CommandState::Succeeded);
        assert_eq!(
            done.after(CommandStep::DeadlinePassed),
            CommandState::Succeeded
        );
        assert_eq!(done.after(CommandStep::Cancelled), CommandState::Succeeded);
    }

    #[test]
    fn an_answer_may_come_before_the_word_that_the_command_was_taken() {
        assert_eq!(
            CommandState::Sent.after(CommandStep::Failed),
            CommandState::Failed
        );
        assert_eq!(
            CommandState::Queued.after(CommandStep::Acknowledged),
            CommandState::Accepted
        );
    }

    #[test]
    fn only_a_command_that_still_waits_is_sent_again() {
        let open = [
            CommandState::Queued,
            CommandState::Sent,
            CommandState::Accepted,
        ];
        let ended = [
            CommandState::Succeeded,
            CommandState::Failed,
            CommandState::Expired,
            CommandState::Cancelled,
        ];
        assert!(open.iter().all(|state| state.is_open()));
        assert!(ended.iter().all(|state| !state.is_open()));
        assert_eq!(
            CommandState::Sent.after(CommandStep::DeadlinePassed),
            CommandState::Expired
        );
        assert_eq!(
            serde_json::to_value(CommandState::Accepted).unwrap(),
            "accepted"
        );
    }

    #[test]
    fn source_reply_metadata_is_optional_and_does_not_confuse_an_unsupported_model_list_with_an_empty_one()
     {
        let old: ProviderRefreshResult = round_trip(json!({"status":"ready","facts":null}));
        assert!(old.source.is_none());
        let failed: ProviderRefreshResult = round_trip(
            json!({"status":"not_told","facts":null,"source":{"error":{"code":"timeout","message":"The source did not answer in time.","retryable":true}}}),
        );
        assert_eq!(
            failed.source.unwrap().error.unwrap().code,
            orch_provider_contract::SourceErrorCode::Timeout
        );
        for supported in [false, true] {
            let listed: ProviderDiscoverModelsResult =
                round_trip(json!({"suggestions":[],"source":{"supported":supported}}));
            assert!(listed.suggestions.is_empty());
            assert_eq!(listed.source.unwrap().supported, Some(supported));
        }
    }

    #[test]
    fn a_command_is_for_the_role_that_can_carry_it_out() {
        let environment = [Role::Environment];
        assert!(CommandKind::FsList.addressee().serves(&environment, false));
        assert!(
            !CommandKind::TurnStart
                .addressee()
                .serves(&environment, false)
        );
        assert!(
            CommandKind::ProviderRefresh
                .addressee()
                .serves(&[Role::Provider], false)
        );
        assert!(
            CommandKind::ProviderRefresh
                .addressee()
                .serves(&[Role::Agent], false)
        );
        assert!(CommandKind::MachineProbe.addressee().serves(&[], false));
    }

    #[test]
    fn only_a_machine_a_person_connected_has_a_policy_to_change() {
        let roles = [Role::Agent, Role::Environment];
        assert!(
            CommandKind::MachinePolicySet
                .addressee()
                .serves(&roles, true)
        );
        assert!(
            !CommandKind::MachinePolicySet
                .addressee()
                .serves(&roles, false)
        );
    }

    #[test]
    fn update_now_is_one_machine_serial_command_for_connector_and_node_with_a_typed_answer() {
        let decoded = Command::decode(
            "machine.update.now",
            serde_json::json!({"recommended":"1.2.3"}),
        )
        .unwrap();
        assert_eq!(decoded.kind(), CommandKind::MachineUpdateNow);
        assert_eq!(CommandKind::MachineUpdateNow.key(), KeyKind::Machine);
        assert!(
            CommandKind::MachineUpdateNow
                .addressee()
                .serves(&[Role::Node], false)
        );
        assert!(
            CommandKind::MachineUpdateNow
                .addressee()
                .serves(&[Role::Environment, Role::Agent], true)
        );
        assert!(
            Command::decode(
                "machine.update.now",
                serde_json::json!({"recommended":"1.2.3","url":"https://elsewhere.test/program"})
            )
            .is_err()
        );
        let answer = MachineUpdateNowResult {
            status: "scheduled".into(),
            running_version: "1.2.2".into(),
            recommended_version: "1.2.3".into(),
            detail: "Waiting for idle".into(),
        };
        let written = serde_json::to_value(&answer).unwrap();
        assert_eq!(
            serde_json::from_value::<MachineUpdateNowResult>(written).unwrap(),
            answer
        );
    }

    #[test]
    fn a_moving_units_files_are_named_only_inside_its_state_and_travel_through_the_blob_store() {
        assert!(state_path_is_inside(
            "harness/0199c1a0-0000-7000-8000-00000000000a"
        ));
        for outside in [
            "",
            "/etc",
            "harness/../identity",
            "harness//x",
            "./harness",
            "harness/.",
        ] {
            assert!(!state_path_is_inside(outside), "{outside:?}");
        }
        assert_eq!(CommandKind::NodeRuntimeExport.transfer(), Transfer::Upload);
        assert_eq!(
            CommandKind::NodeRuntimeImport.transfer(),
            Transfer::Download
        );
        let hash = "0".repeat(64);
        let body = serde_json::json!({ "runtime_id": "0199c1a0-0000-7000-8000-00000000000a", "path": "harness/p",
            "archive": { "hash": hash, "size": 5 } });
        let import: NodeRuntimeImport = serde_json::from_value(body.clone()).unwrap();
        assert_eq!(import.archive.size, 5);
        assert!(
            crate::content::listed_hashes(&body).contains(&import.archive.hash),
            "the archive is what the import may fetch"
        );
    }
}
