//! A changing input is separate from a loss of identity, source Read or provider Use.
use super::*;

/// A source-state change, distinct from the ordinary authorization gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SourceChange {
    /// The source is stable now but differs from the prepared prompt.
    #[error("the summary's source no longer matches its prepared prompt")]
    Content,
    /// The accepted summary policy no longer holds.
    #[error("the summary settings changed while it was prepared")]
    Settings,
    /// The unpaid read exhausted its configured repeats.
    #[error("the content kept changing while the summary source was read")]
    Unstable,
}

impl SourceChange {
    pub(crate) fn error(self) -> EngineError {
        EngineError::BeforeEffect(Box::new(self))
    }

    /// Stable projected problem reason.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Content => "source_changed",
            Self::Settings => "settings_changed",
            Self::Unstable => "content_changing",
        }
    }

    /// Plain sentence shown beside any previous paragraph.
    #[must_use]
    pub fn sentence(self) -> &'static str {
        match self {
            Self::Content => "The source changed while this summary was being written.",
            Self::Settings => "The summary settings changed before this summary could finish.",
            Self::Unstable => "The content kept changing before the summary could start.",
        }
    }
}

/// A factual change never becomes a credential or ordinary-permission refusal.
pub fn source_change(error: &EngineError) -> Option<SourceChange> {
    match error {
        EngineError::BeforeEffect(cause) => cause.downcast_ref::<SourceChange>().copied(),
        _ => None,
    }
}

pub(crate) async fn read_repeats(conn: &mut Conn, place: Id) -> Result<i64, EngineError> {
    Ok(orch_settings::get::<i64>(
        conn,
        orch_settings::Place::Node(place),
        orch_settings_contract::registry::keys::SUMMARY_READ_REPEATS,
    )
    .await?
    .unwrap_or(3))
}
