//! Session decisions keep references and their giver/source, independently of mutable skill resources.

use chrono::{DateTime, Utc};
use orch_api_contract::skills::ThreadSkills;
use orch_db::{Conn, queries::session_skills};
use orch_types::Id;
use serde_json::json;

pub(crate) struct Opening<'a> {
    pub revision: Id,
    pub thread: Id,
    pub segment: Id,
    pub at: DateTime<Utc>,
    pub supported: bool,
    pub skills: &'a ThreadSkills,
}

/// Called before the command is first created, in that command's transaction.
pub(crate) async fn prepare(
    conn: &mut Conn,
    opening: Opening<'_>,
) -> Result<bool, crate::EngineError> {
    let selected = json!(
        opening
            .skills
            .skills
            .iter()
            .map(|skill| json!({
                "skill_id":skill.skill_id,"version_id":skill.version_id,"number":skill.number,
                "contributor":skill.contributor,"from":skill.from,
            }))
            .collect::<Vec<_>>()
    );
    let skipped=json!(opening.skills.skipped.iter().map(|skill|json!({
        "skill_id":skill.skill_id,"reason":skill.reason,"contributor":skill.contributor,"from":skill.from,
    })).collect::<Vec<_>>());
    let fingerprint = super::fingerprint(opening.skills).to_string();
    Ok(session_skills::prepare(
        conn,
        &session_skills::NewOutcome {
            revision: opening.revision,
            thread: opening.thread,
            segment: opening.segment,
            at: opening.at,
            supported: opening.supported,
            fingerprint: &fingerprint,
            selected: &selected,
            skipped: &skipped,
        },
    )
    .await?)
}
