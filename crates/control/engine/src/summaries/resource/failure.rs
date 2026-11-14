//! Why a summary that was asked for could not be written, in the words the person who asked is shown.
//!
//! A refusal inside the work only says that something no longer holds. After a request ends without
//! a paragraph, the engine looks again at what the person can change (who asked, what the summary is
//! about, the provider, the model, the machine that keeps the provider's key, the usage limits) and
//! names the first thing that does not hold. The code is stored with the request and shown by the
//! page; the sentence is for the server's log and for readers that know no codes.

/// The first thing that stopped a summary, by what the person can do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The sign-in or key that asked for the summary has ended.
    AskerEnded,
    /// What the summary describes is gone.
    SourceGone,
    /// What the summary describes can no longer be read by whoever asked.
    SourceUnreadable,
    /// There is nothing to summarize: no unread message, nothing written, no activity.
    NothingNew,
    /// No summarizer is chosen any more.
    NoSummarizer,
    /// The provider that was to write it is gone.
    ProviderGone,
    /// The provider is switched off or not ready.
    ProviderNotReady,
    /// The provider offers no way to write text.
    NoInterface,
    /// The model is switched off or no longer offered by the provider.
    ModelOff,
    /// The place does not allow the model.
    ModelNotAllowed,
    /// The place does not allow the provider.
    ProviderForbidden,
    /// Whoever pays for the summary may not use the provider.
    NotAllowed,
    /// The model's reasoning effort is not one the person may use.
    EffortNotOffered,
    /// No machine keeps the provider's key.
    NoKeeper,
    /// The machine that keeps the provider's key is offline.
    KeeperOffline,
    /// A usage limit leaves no room for a summary.
    CapNoRoom,
    /// The model used its output allowance before its answer was whole.
    OutputCut,
    /// The model did not answer, or answered with an error.
    ModelFailed,
    /// The model answered, but what it answered cannot be used as a summary.
    UnusableAnswer,
    /// Nothing that can be named failed.
    Unexpected,
}

impl Reason {
    /// The stable word the page keys its sentence by.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::AskerEnded => "asker_ended",
            Self::SourceGone => "source_gone",
            Self::SourceUnreadable => "source_unreadable",
            Self::NothingNew => "nothing_new",
            Self::NoSummarizer => "no_summarizer",
            Self::ProviderGone => "provider_gone",
            Self::ProviderNotReady => "provider_not_ready",
            Self::NoInterface => "no_interface",
            Self::ModelOff => "model_off",
            Self::ModelNotAllowed => "model_not_allowed",
            Self::ProviderForbidden => "provider_forbidden",
            Self::NotAllowed => "not_allowed",
            Self::EffortNotOffered => "effort_not_offered",
            Self::NoKeeper => "no_keeper",
            Self::KeeperOffline => "keeper_offline",
            Self::CapNoRoom => "cap_no_room",
            Self::OutputCut => "output_cut",
            Self::ModelFailed => "model_failed",
            Self::UnusableAnswer => "unusable_answer",
            Self::Unexpected => "unexpected",
        }
    }

    /// One English sentence, with what to do about it.
    #[must_use]
    pub fn sentence(self) -> &'static str {
        match self {
            Self::AskerEnded => {
                "The sign-in or key that asked for this summary has ended. Ask again."
            }
            Self::SourceGone => "What this summary describes is gone.",
            Self::SourceUnreadable => {
                "Whoever asked can no longer read what this summary describes. Ask someone who can."
            }
            Self::NothingNew => "There is nothing new to summarize.",
            Self::NoSummarizer => "No summarizer is chosen. Choose one for summaries.",
            Self::ProviderGone => {
                "The provider that writes this summary was removed. Choose another provider for summaries."
            }
            Self::ProviderNotReady => {
                "The provider that writes this summary is switched off or not ready. Switch it on or choose another."
            }
            Self::NoInterface => {
                "The provider that writes this summary does not offer text generation. Choose another provider."
            }
            Self::ModelOff => {
                "The model that writes this summary is switched off or no longer offered. Switch it on or choose another model."
            }
            Self::ModelNotAllowed => {
                "This place does not allow the model that writes this summary. Choose another model or allow this one."
            }
            Self::ProviderForbidden => {
                "This place does not allow the provider that writes this summary. Choose another provider or allow this one."
            }
            Self::NotAllowed => {
                "Whoever pays for this summary may not use its provider. Give them use of it or choose another provider."
            }
            Self::EffortNotOffered => {
                "The model's reasoning effort chosen for this summary is not available to its user. Choose another effort."
            }
            Self::NoKeeper => {
                "No machine keeps the key of the provider that writes this summary. Give the provider a machine."
            }
            Self::KeeperOffline => {
                "The machine that keeps the key of the provider is offline. Bring it online or move the key."
            }
            Self::CapNoRoom => {
                "A usage limit leaves no room for summaries. Wait for it to reset or raise it."
            }
            Self::OutputCut => {
                "The summarizer's model used its output allowance before finishing. Raise the summarizer's output allowance or choose another model."
            }
            Self::ModelFailed => "The model did not answer. Try again in a moment.",
            Self::UnusableAnswer => {
                "The model's answer could not be used as a summary. Try again or choose another model."
            }
            Self::Unexpected => {
                "Something went wrong while writing the summary. Try again in a moment."
            }
        }
    }
}
