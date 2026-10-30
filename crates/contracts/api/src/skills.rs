//! Skills: directories of files in the Agent Skills format kept as a resource with a home, owners and a permission page; their immutable versions with a manifest and where each came from; a draft its managers edit; and the skills a thread may use now.
//!
//! A skill belongs to an org, a project, or a person's own org, which decides who manages it
//! (`skill.manage`) and who may be given its use (`skill.use`). Its content is a list of versions:
//! each import, update, upload or publish makes one, kept as a gzip-compressed tar of its files in
//! the blob store with a manifest of each file's path, size, SHA-256 and executable bit, and a
//! version never changes. A place switches skills on for its threads with the setting
//! `agents.skills` (each skill off, at its latest version, or at a pinned one); a person's own skills
//! reach the threads they started where the place's `agents.personal_skills` allows them.

use orch_types::{Hash, Id, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::{NewResourceAccess, ResourceAccess};
use crate::common::{Listed, Page};
use crate::nodes::NodeRef;
use crate::routes::Access::{Permission, Rule, User};
use crate::users::UserRef;

/// One file of a skill's version, as its manifest lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillFile {
    /// Where it lies in the skill: a relative path, `/` between its parts.
    pub path: String,
    /// Its size in bytes.
    pub size: i64,
    /// The SHA-256 of what it holds, 64 lower-case hexadecimal characters.
    pub sha256: String,
    /// Whether it is executable.
    pub executable: bool,
}

/// The archive a version is kept as: a gzip-compressed tar of its files, in the order of their
/// paths, in the blob store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillArchive {
    /// The archive's blob.
    pub blob: Hash,
    /// Its size in bytes.
    pub size: i64,
}

/// Where a version came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillOrigin {
    /// An archive somebody uploaded.
    Upload {
        /// The name of the uploaded file.
        name: String,
        /// The folder of the archive the skill was taken from; `null` for its top or its one folder.
        folder: Option<String>,
    },
    /// An archive fetched from an address by a host.
    Url {
        /// The public address. Userinfo, query and fragment are hidden as `?…` when the source
        /// needs them; their value is kept sealed on the skill and never returned.
        url: String,
        /// The folder of the archive the skill was taken from; `null` for its top or its one folder.
        folder: Option<String>,
        /// The host that fetched it.
        host_id: Id,
    },
    /// A Git repository at a commit, fetched by a host.
    Git {
        /// The repository's address.
        repository: String,
        /// The branch, tag or commit that was asked for, which an update asks for again.
        reference: String,
        /// The commit it came to.
        commit: String,
        /// The folder of the repository the skill was taken from; `null` for its top.
        folder: Option<String>,
        /// The host that fetched it.
        host_id: Id,
    },
    /// A directory of an environment, on a host or on a person's machine.
    Environment {
        /// The environment.
        environment_id: Id,
        /// The directory, relative to the environment's workspace.
        path: String,
    },
    /// The skill's draft, published in Orch.
    Draft {
        /// The version the draft was started from; `null` for a draft that began empty.
        based_on: Option<Id>,
    },
}

impl SkillOrigin {
    /// Whether an update can take the skill again from where this version came from: an address, a
    /// repository or a directory can be read again; an upload and a draft cannot.
    #[must_use]
    pub const fn can_be_taken_again(&self) -> bool {
        matches!(
            self,
            Self::Url { .. } | Self::Git { .. } | Self::Environment { .. }
        )
    }
}

/// A version of a skill, without the list of its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillVersionInfo {
    /// The version's identifier, which a pinned setting names.
    pub id: Id,
    /// Its number among the skill's versions, from 1.
    pub number: i64,
    /// The name its `SKILL.md` gives.
    pub name: String,
    /// The description its `SKILL.md` gives.
    pub description: String,
    /// The bytes all its files hold together.
    pub size: i64,
    /// How many files it has.
    pub file_count: i64,
    /// The archive it is kept as.
    pub archive: SkillArchive,
    /// Where it came from.
    pub origin: SkillOrigin,
    /// Who made it; `null` where that person is gone.
    pub created_by: Option<UserRef>,
    /// When it was made.
    pub created_at: Time,
}

/// A version of a skill, with the manifest of its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillVersion {
    /// The version.
    pub version: SkillVersionInfo,
    /// Its files, in the order of their paths.
    pub files: Vec<SkillFile>,
}

/// That a skill has a draft, and how it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillDraftInfo {
    /// The version it was started from; `null` for one that began empty.
    pub based_on: Option<Id>,
    /// How many files it holds.
    pub file_count: i64,
    /// The revision a change of it names as `expected_revision`.
    pub revision: Revision,
    /// Who changed it last.
    pub updated_by: Option<UserRef>,
    /// When it was changed last.
    pub updated_at: Time,
}

/// A skill, as a resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Skill {
    /// Places that explicitly enable this skill, excluding off entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_places: Option<i64>,
    /// The skill's identifier.
    pub id: Id,
    /// Its name, as its `SKILL.md` gives it; every version keeps it.
    pub name: String,
    /// What it does and when to use it, as its latest version says.
    pub description: String,
    /// The root org of its home.
    pub org_id: Id,
    /// Its first owner.
    pub owner: Option<UserRef>,
    /// The org or the project it belongs to.
    pub home: NodeRef,
    /// Where it comes from, as short names.
    pub address: String,
    /// Whether threads may be given it.
    pub enabled: bool,
    /// Its latest version; `null` until it has one.
    pub latest: Option<SkillVersionInfo>,
    /// How many versions it has.
    pub versions: i64,
    /// Where an update takes it from again: the last address, repository or directory it was
    /// imported from; `null` where it has none.
    pub origin: Option<SkillOrigin>,
    /// Its draft, for whoever manages it; `null` where there is none or the caller does not
    /// manage it.
    pub draft: Option<SkillDraftInfo>,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
    /// What the caller holds on it.
    pub access: ResourceAccess,
}

/// What a skill's version is made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SkillSource {
    /// An archive somebody uploaded, by the hash its upload ended with.
    Upload {
        /// The uploaded archive: a zip, a tar or a tar.gz.
        blob: Hash,
        /// The uploaded file's name.
        name: String,
        /// The folder of the archive that holds the skill; left out for its top, or its one folder.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
    },
    /// A new skill begun in Orch: a draft whose `SKILL.md` says this, which becomes the first
    /// version when it is published. Only a new skill is made from it.
    New {
        /// Its name.
        name: String,
        /// What it does and when to use it.
        description: String,
    },
    /// A directory of an environment's workspace, read by the environment's machine for whoever
    /// manages the environment: a skill kept in a repository, or among a person's files on their
    /// machine.
    Environment {
        /// The environment.
        environment_id: Id,
        /// The directory, relative to the workspace; empty for the workspace itself.
        path: String,
    },
    /// A Git repository at a branch, a tag or a commit, fetched by a host the caller may use.
    Git {
        /// The host that fetches it.
        host_id: Id,
        /// The repository's address, of a scheme that needs no credential.
        repository: String,
        /// The branch, tag or commit; an update asks for it again.
        reference: String,
        /// The folder of the repository that holds the skill; left out for its top.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
    },
    /// An archive (a zip, a tar or a tar.gz) at an http or https address, fetched by a host the
    /// caller may use.
    Url {
        /// The host that fetches it.
        host_id: Id,
        /// The address given for this request. Its private parts are write-only.
        url: String,
        /// The folder of the archive that holds the skill; left out for its top, or its one folder.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
    },
}

/// The skills the caller may see.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillListQuery {
    /// Only those whose home is that org or lies inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// Makes a skill, whose first version is made from `from`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillCreateBody {
    /// Its home: an org at any level, a person's own org among them, or a project.
    pub home_id: Id,
    /// What its first version is made from.
    pub from: SkillSource,
    /// Who may use it; as the defaults of its home say when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewResourceAccess>,
}

/// Changes a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillUpdateBody {
    /// Whether threads may be given it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// A page of a skill's versions, the newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillVersionListQuery {
    /// How many versions, at most 200; 50 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// One file of a version, by its path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillFileQuery {
    /// The file's path in the skill.
    pub path: String,
}

/// Makes a version of a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillVersionCreateBody {
    /// What the version is made from.
    pub from: SkillSource,
}

/// One file of a skill's draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillDraftFile {
    /// Where it lies in the skill.
    pub path: String,
    /// Its size in bytes.
    pub size: i64,
    /// Whether it is executable.
    pub executable: bool,
    /// Who wrote it last.
    pub updated_by: Option<UserRef>,
    /// When it was written last.
    pub updated_at: Time,
}

/// A skill's draft with its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillDraft {
    /// How it stands.
    pub info: SkillDraftInfo,
    /// Its files, in the order of their paths.
    pub files: Vec<SkillDraftFile>,
}

/// Starts a skill's draft.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillDraftCreateBody {
    /// The version whose files it begins with; the latest when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_version: Option<Id>,
}

/// Writes one file of a skill's draft: its text, or an uploaded file, or only whether it is
/// executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillDraftFilePutBody {
    /// Where it lies in the skill.
    pub path: String,
    /// Its text, as written in the editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A file the caller uploaded, by the hash its upload ended with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<Hash>,
    /// Whether it is executable; as it was, or not, when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
    /// The draft's revision the caller saw.
    pub expected_revision: Revision,
}

/// Removes one file of a skill's draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillDraftFileDeleteQuery {
    /// Where it lies in the skill.
    pub path: String,
    /// The draft's revision the caller saw.
    pub expected_revision: Revision,
}

/// Publishes a skill's draft as its next version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillDraftPublishBody {
    /// The draft's revision the caller saw.
    pub expected_revision: Revision,
}

/// What taking a skill again from where it came from made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillReimportReply {
    /// The latest version after it: a new one, or the one that already held the same files.
    pub version: SkillVersionInfo,
    /// Whether a new version was made; false when the files were those of the latest.
    pub made: bool,
}

/// Why a skill that a thread's settings name is not given to its agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillSkip {
    /// No skill has that identifier.
    Missing,
    /// It is switched off.
    Disabled,
    /// It was deleted.
    Deleted,
    /// It has no version yet.
    NoVersion,
    /// The pinned version is not one of the skill's.
    VersionNotFound,
    /// Whoever switched it on may not use it now.
    NotUsable,
    /// Another skill given to the thread has the same name.
    NameTaken,
}

/// Where a skill given to a thread was switched on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThreadSkillFrom {
    /// The setting `agents.skills` of a place on the thread's path.
    Place {
        /// The place whose setting named it.
        node_id: Id,
    },
    /// The own skills of whoever started the thread.
    Personal {
        /// Who started the thread.
        user_id: Id,
    },
}

/// A skill a thread's agent may use now, at the version it is given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSkill {
    /// The skill.
    pub skill_id: Id,
    /// The version it is given.
    pub version_id: Id,
    /// That version's number.
    pub number: i64,
    /// The name its `SKILL.md` gives.
    pub name: String,
    /// The description its `SKILL.md` gives.
    pub description: String,
    /// The archive of its files.
    pub archive: SkillArchive,
    /// The manifest of its files.
    pub files: Vec<SkillFile>,
    /// Where it was switched on.
    pub from: ThreadSkillFrom,
}

/// A skill that a thread's settings name but its agent is not given, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkippedSkill {
    /// The skill.
    pub skill_id: Id,
    /// Its name, where it is known.
    pub name: Option<String>,
    /// Why it is left out.
    pub reason: SkillSkip,
    /// Where it was switched on.
    pub from: ThreadSkillFrom,
}

/// The skills a thread's agent may use now: those its places switch on and its starter's own
/// skills where the place allows them, each checked at this moment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSkills {
    /// What its agent is given, by name.
    pub skills: Vec<ThreadSkill>,
    /// What is named but left out, with why.
    pub skipped: Vec<SkippedSkill>,
}

crate::endpoints! {
    /// Lists the skills the caller may see, of whatever org.
    SkillList: GET "/skills", User, query SkillListQuery, none, json Vec<Listed<Skill>>, 200;
    /// Makes a skill from an uploaded archive, which is checked and kept as its first version.
    SkillCreate: POST "/skills", Rule("its home is the caller's domain or an org or project inside it and the members of that home may make skills there, a project also asking that the caller may work in it; else settings.manage on that home; a directory also asks environment.manage on its environment, a repository or an address host.use on the host that fetches it"), none, json SkillCreateBody, json Skill, 201;
    /// Reads a skill.
    SkillGet: GET "/skills/{id}", Rule("any permission on the skill"), none, none, json Skill, 200;
    /// Switches a skill on or off.
    SkillUpdate: PATCH "/skills/{id}", Permission("skill.manage"), none, json SkillUpdateBody, json Skill, 200;
    /// Deletes a skill; its versions stay for the turns that were given them.
    SkillDelete: DELETE "/skills/{id}", Permission("skill.manage"), none, none, none, 204;
    /// Lists a skill's versions, the newest first.
    SkillVersionList: GET "/skills/{id}/versions", Permission("skill.view"), query SkillVersionListQuery, none, json Page<SkillVersionInfo>, 200;
    /// Reads one version of a skill with the manifest of its files.
    SkillVersionGet: GET "/skills/{id}/versions/{version_id}", Permission("skill.view"), none, none, json SkillVersion, 200;
    /// One file of a version, as it is.
    SkillVersionFileGet: GET "/skills/{id}/versions/{version_id}/file", Permission("skill.view"), query SkillFileQuery, none, raw, 200;
    /// Makes a version of a skill from an uploaded archive or an environment's directory.
    SkillVersionCreate: POST "/skills/{id}/versions", Permission("skill.manage"), none, json SkillVersionCreateBody, json SkillVersion, 201;
    /// Takes a skill again from the address, repository or directory it was last imported from.
    SkillReimport: POST "/skills/{id}/reimport", Permission("skill.manage"), none, none, json SkillReimportReply, 200;
    /// Reads a skill's draft with its files.
    SkillDraftGet: GET "/skills/{id}/draft", Permission("skill.manage"), none, none, json SkillDraft, 200;
    /// Starts a skill's draft from one of its versions.
    SkillDraftCreate: POST "/skills/{id}/draft", Permission("skill.manage"), none, json SkillDraftCreateBody, json SkillDraft, 201;
    /// Discards a skill's draft.
    SkillDraftDelete: DELETE "/skills/{id}/draft", Permission("skill.manage"), none, none, none, 204;
    /// One file of a skill's draft, as it is.
    SkillDraftFileGet: GET "/skills/{id}/draft/file", Permission("skill.manage"), query SkillFileQuery, none, raw, 200;
    /// Writes one file of a skill's draft.
    SkillDraftFilePut: PUT "/skills/{id}/draft/file", Permission("skill.manage"), none, json SkillDraftFilePutBody, json SkillDraft, 200;
    /// Removes one file of a skill's draft.
    SkillDraftFileDelete: DELETE "/skills/{id}/draft/file", Permission("skill.manage"), query SkillDraftFileDeleteQuery, none, json SkillDraft, 200;
    /// Publishes a skill's draft as its next version, after checking it.
    SkillDraftPublish: POST "/skills/{id}/draft/publish", Permission("skill.manage"), none, json SkillDraftPublishBody, json SkillVersion, 201;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_origin_is_written_with_its_kind() {
        let origin: SkillOrigin = serde_json::from_value(json!({
            "kind": "git",
            "repository": "https://example.org/skills.git",
            "reference": "main",
            "commit": "0123456789abcdef0123456789abcdef01234567",
            "folder": "skills/pdf",
            "host_id": "00000000-0000-0000-0000-000000000001"
        }))
        .unwrap();
        assert!(origin.can_be_taken_again());
        let upload = SkillOrigin::Upload {
            name: "pdf.zip".to_owned(),
            folder: None,
        };
        assert_eq!(
            serde_json::to_value(&upload).unwrap(),
            json!({ "kind": "upload", "name": "pdf.zip", "folder": null })
        );
        assert!(!upload.can_be_taken_again());
        assert!(!SkillOrigin::Draft { based_on: None }.can_be_taken_again());
    }

    #[test]
    fn a_source_is_written_with_its_kind_and_takes_nothing_else() {
        let body: SkillCreateBody = serde_json::from_value(json!({
            "home_id": Id::nil(),
            "from": {
                "kind": "upload",
                "blob": "0000000000000000000000000000000000000000000000000000000000000000",
                "name": "pdf.zip"
            }
        }))
        .unwrap();
        assert!(matches!(
            body.from,
            SkillSource::Upload { folder: None, .. }
        ));
        assert!(
            serde_json::from_value::<SkillSource>(json!({
                "kind": "upload",
                "blob": "0000000000000000000000000000000000000000000000000000000000000000",
                "name": "pdf.zip",
                "extra": true
            }))
            .is_err()
        );
    }

    #[test]
    fn a_skill_named_by_the_settings_and_left_out_says_why() {
        let skipped = SkippedSkill {
            skill_id: Id::nil(),
            name: Some("pdf".to_owned()),
            reason: SkillSkip::NotUsable,
            from: ThreadSkillFrom::Place { node_id: Id::nil() },
        };
        assert_eq!(
            serde_json::to_value(&skipped).unwrap(),
            json!({
                "skill_id": Id::nil(),
                "name": "pdf",
                "reason": "not_usable",
                "from": { "kind": "place", "node_id": Id::nil() }
            })
        );
    }
}
