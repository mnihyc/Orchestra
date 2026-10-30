//! Skills as a resource of files: versions made from archives, directories and drafts, checked and kept as blobs, and the skills each thread may use now.
//!
//! A skill is a directory in the Agent Skills format: a `SKILL.md` whose front matter names it and
//! says what it does, with any references, scripts and assets beside it. Orch keeps a skill as a
//! resource with a home, owners and a permission page, and its content as versions: each import,
//! update or publish makes one, an archive of the file tree kept in the blob store with a manifest of
//! its files, and a version never changes. Which skills a thread's agent gets is a setting of its
//! places (`agents.skills`: a skill off, at its latest version, or at a pinned one), together with
//! the own skills of whoever started the thread where the place allows them
//! (`agents.personal_skills`), each checked at the moment it is asked for against the Use of whoever
//! named it.

pub mod archive;
pub mod front_matter;
pub mod resolve;
pub mod versions;

use orch_api_contract::skills::ThreadSkills;
use orch_types::{Id, Time};

use crate::{Engine, EngineError};

impl Engine {
    /// The skills the thread `thread`'s agent may use now, each with the version it is given, its
    /// archive and its manifest, and those its settings name but leave out with why. What a session
    /// is given is asked here at the moment it is opened or refreshed.
    pub async fn skills_for_thread(&self, thread: Id) -> Result<ThreadSkills, EngineError> {
        let mut conn = self.db().acquire().await?;
        resolve::for_thread(&mut conn, thread, Time::new(self.inner.clock.now())).await
    }

    /// Records on the turn `turn` the skill versions it was given when it started.
    pub async fn record_turn_skills(
        &self,
        turn: Id,
        skills: &ThreadSkills,
    ) -> Result<bool, EngineError> {
        let mut conn = self.db().acquire().await?;
        resolve::record_turn(&mut conn, turn, skills).await
    }
}

/// What names the skill versions a thread is given, whatever their order: the session opened with them is current
/// while this stays the same.
#[must_use]
pub fn fingerprint(skills: &orch_api_contract::skills::ThreadSkills) -> orch_types::Hash {
    let mut versions: Vec<String> = skills
        .skills
        .iter()
        .map(|skill| format!("{}:{}", skill.name, skill.version_id))
        .collect();
    versions.sort_unstable();
    orch_types::Hash::of(versions.join("\n").as_bytes())
}

/// The skills of a `session.open`: each version's archive, which the connector fetches and lays out for the thread.
#[must_use]
pub fn session_open_skills(
    skills: &orch_api_contract::skills::ThreadSkills,
) -> Vec<orch_link_contract::commands::SessionOpenSkill> {
    skills
        .skills
        .iter()
        .map(|skill| orch_link_contract::commands::SessionOpenSkill {
            skill_id: skill.skill_id,
            version_id: skill.version_id,
            name: skill.name.clone(),
            blob: skill.archive.blob,
            size: skill.archive.size,
        })
        .collect()
}
