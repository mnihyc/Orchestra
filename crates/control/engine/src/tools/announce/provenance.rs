//! Posting provenance is stored material, never a model actor or a continuing permission.

use crate::{EngineError, ids};
use orch_authz::Caller;
use orch_db::queries::threads::{InputRow, tools::AnnouncementRow};
use orch_types::{Id, Time};
use serde::{Deserialize, Serialize};

/// Factual attribution of the accepted posting. It contains no restorable credential,
/// admission proof or native session; model use remains with the receiving thread owner.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnouncementProvenance {
    sender_id: Id,
    operation: Option<String>,
    posted_at: Time,
    place_id: Id,
    topic_id: Id,
    announcement_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_input_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_recipient_id: Option<Id>,
}

impl AnnouncementProvenance {
    /// Record only the operation actually admitted for this native caller; absence stays absent.
    pub(crate) fn posting(
        caller: &Caller,
        place: Id,
        topic: Id,
        announcement: Id,
        at: Time,
    ) -> Self {
        Self {
            sender_id: caller.user(),
            operation: caller.admitted_operation().map(str::to_owned),
            posted_at: at,
            place_id: place,
            topic_id: topic,
            announcement_id: announcement,
            original_input_id: None,
            original_recipient_id: None,
        }
    }

    pub(crate) fn input(
        caller: &Caller,
        place: Id,
        announcement: &AnnouncementRow,
        input: &InputRow,
    ) -> Result<Self, EngineError> {
        let expected = ids::derived(
            "announcement.input",
            &[
                announcement.id.as_bytes().as_slice(),
                input.thread_id.as_bytes().as_slice(),
            ],
        );
        if announcement.from_user_id != Some(caller.user())
            || input.author_id.is_some()
            || input.source != "announcement"
            || input.id != expected
        {
            return Err(EngineError::unreadable(
                "announcement provenance has another native posting",
            ));
        }
        let mut provenance = match &announcement.posting_provenance {
            Some(stored) => serde_json::from_value::<Self>(stored.clone())?,
            None => Self::posting(
                caller,
                place,
                announcement.topic_id,
                announcement.id,
                Time::new(announcement.created_at),
            ),
        };
        if provenance.sender_id != caller.user()
            || provenance.announcement_id != announcement.id
            || provenance.topic_id != announcement.topic_id
            || provenance.place_id != place
        {
            return Err(EngineError::unreadable(
                "announcement provenance changed its native association",
            ));
        }
        provenance.original_input_id = Some(input.id);
        provenance.original_recipient_id = Some(input.thread_id);
        Ok(provenance)
    }
}
