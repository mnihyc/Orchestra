//! What can go wrong inside the engine, and does not belong to one of its requests.
//!
//! Each request of the engine has its own error type, which names what the
//! caller can act on (a refusal, a missing thing, a conflict). What is left
//! is the failure of a part the engine stands on: the database, the access
//! engine, the settings, the link. Those are not the caller's to repair and
//! travel together as an [`EngineError`], which a request type wraps as its
//! `Internal` case.

use crate::access::AccessFault;
use crate::commands::CommandsError;
use orch_db::DbError;
use orch_types::Id;

/// Why Ask's window leaves nothing for the conversation after the answer's reserve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskNoRoom {
    /// `ask.context_tokens` lowered the window to the answer's reserve or below, or the reserve is not above zero.
    Setting {
        /// The window Ask was given.
        window: i64,
        /// The reserve for the answer.
        output: i64,
    },
    /// The model's own window is the answer's reserve or less.
    Window {
        /// The model.
        model: String,
        /// Its window, as stated or as assumed.
        window: i64,
        /// The reserve for the answer.
        output: i64,
        /// Whether the model states no window and this one is assumed.
        assumed: bool,
    },
}

impl std::fmt::Display for AskNoRoom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setting { window, output } => write!(
                f,
                "ask.context_tokens ({window}) is not larger than ask.max_output_tokens ({output}), so no room is left for the conversation. Increase ask.context_tokens or reduce ask.max_output_tokens."
            ),
            Self::Window {
                model,
                window,
                output,
                assumed: false,
            } => write!(
                f,
                "The context window of {model} ({window} tokens) is not larger than ask.max_output_tokens ({output}), so no room is left for the conversation. Use a model with a larger window or reduce ask.max_output_tokens."
            ),
            Self::Window {
                model,
                window,
                output,
                assumed: true,
            } => write!(
                f,
                "The context window of {model} is not recorded and is taken as {window} tokens, which is not larger than ask.max_output_tokens ({output}), so no room is left for the conversation. Record the model's context window or reduce ask.max_output_tokens."
            ),
        }
    }
}

/// A part the engine stands on failed, or what it stores does not read.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A capability returned a safe reported refusal, including its actual allowance reset.
    #[error("{0}")]
    Capability(Box<crate::tools::capability_service::CapabilityFailure>),
    /// A prepared capability follows an existing call-rule pause without an accepted result.
    #[error("The capability is paused under its current call rules.")]
    CapabilityPaused(Box<crate::tools::capability_service::CapabilityPause>),
    /// Ask's window leaves nothing after the answer's reserve.
    #[error("{0}")]
    AskContextNoRoom(AskNoRoom),
    /// Even a summary and the current question cannot fit the selected model.
    #[error("This conversation is too long for {0}; start a new one or keep it as a thread.")]
    AskContextTooLong(String),
    /// A lifecycle change must first review and end the thread's live native work.
    #[error(
        "Background tasks are still running. Review and end them before changing this session."
    )]
    BackgroundRunning(Id),
    /// An entry point refused its first effect. Preserve the opaque original error for that caller.
    #[error(transparent)]
    BeforeEffect(Box<dyn std::error::Error + Send + Sync>),
    /// A requested informational mount comparison could not read one of its locations.
    #[error("{0}")]
    MountFingerprint(String),
    /// A fresh own-tool call no longer belongs to the selected active execution.
    #[error(
        "This call belongs to a turn that is no longer selected or running. No further work was admitted; make a new call from the current turn."
    )]
    StaleToolSource,
    /// A logical call reference already has another retained source or argument identity.
    #[error(
        "This call reference already identifies different work. No new work was admitted; use a new call from the current turn."
    )]
    ToolCallIdentity,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The access engine could not answer.
    #[error(transparent)]
    Authz(#[from] orch_authz::Error),
    /// The resource side of the access engine could not answer.
    #[error(transparent)]
    Access(#[from] AccessFault),
    /// The settings could not be read.
    #[error(transparent)]
    Settings(#[from] orch_settings::Error),
    /// A command could not be sent, or its end could not be learned.
    #[error(transparent)]
    Commands(#[from] CommandsError),
    /// Something stored, or something a machine said, does not read as what
    /// it should be.
    #[error("{0}")]
    Unreadable(String),
}

impl EngineError {
    /// A stored value or a machine's word that does not read.
    pub fn unreadable(what: impl std::fmt::Display) -> Self {
        Self::Unreadable(what.to_string())
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(error: serde_json::Error) -> Self {
        Self::Unreadable(error.to_string())
    }
}
