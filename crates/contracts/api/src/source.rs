//! Source preparation, worktrees, recorded file restoration and Git publication.
//!
//! Files stay in their owning environment. A restore describes every effect before one job
//! applies it, and its undo is another restore plan. Source credentials are names only; neither
//! these responses nor thread links carry authentication material.

use orch_types::{Hash, Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::environment::{EnvironmentRef, ThroughThreadQuery};
use crate::forks::{AppliedChangeOptions, ChangeOptions};
use crate::nodes::{JobRef, NodeRef};
use crate::routes::Access::{Permission, Rule};
use crate::workspace::{GitStatus, Job, Snapshot};

/// The safe repository settings frozen for one preparation job.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepositorySource {
    /// An HTTPS or SSH repository address without embedded credentials.
    pub repository_url: Option<String>,
    /// The branch; null uses the remote's advertised default.
    pub branch: Option<String>,
    /// A setting-secret name; its value never becomes source metadata.
    pub secret_name: Option<String>,
}

/// The kind of directory a thread uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// An existing directory, whether or not it contains a repository.
    Directory,
    /// A repository prepared for the thread.
    Repository,
    /// A linked worktree with its own files and index.
    Worktree,
}

/// Where preparation stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    /// Waiting for preparation.
    Pending,
    /// Preparation is running.
    Preparing,
    /// The directory is ready.
    Ready,
    /// Preparation failed, retaining its frozen source and waiting input.
    Failed,
}

/// The safe source facts a person sees beside a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceInfo {
    /// How the directory was obtained.
    pub kind: SourceKind,
    /// Its workspace-relative directory.
    pub directory: String,
    /// The repository address without credentials, when known.
    pub repository_url: Option<String>,
    /// The branch, when known.
    pub branch: Option<String>,
    /// The resolved commit, when known.
    pub head: Option<String>,
    /// The current preparation state.
    pub state: SourceState,
    /// Safe progress for its preparation job.
    pub job: Option<JobRef>,
    /// Why preparation is unavailable or failed.
    pub reason: Option<String>,
}

/// A destination authorized by the trusted provisioning job, never asserted by the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceDestination {
    /// A child directory that must still be absent when the staged checkout is installed.
    MissingDirectory,
    /// A verified empty workspace volume owned by the provisioning job.
    OwnedEmptyRoot {
        /// The job that owns the volume.
        provisioning_job_id: Id,
        /// The exact volume name frozen by that job.
        workspace_volume: String,
        /// The generation that owns the root.
        generation: i64,
    },
}

/// A link explicitly saved by an interactor, never fetched by Orch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadLink {
    /// An HTTP or HTTPS URL without user information.
    pub url: String,
    /// What the link is called.
    pub label: String,
}

/// Facts from the last explicitly requested Git status, with unknowns preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DirectoryWork {
    /// Whether included files have staged, unstaged or untracked changes.
    pub uncommitted: Option<bool>,
    /// Whether the branch has commits not sent upstream.
    pub unpushed: Option<bool>,
    /// How many included changed files were observed.
    pub changed_files: Option<i64>,
    /// How many commits were ahead, when known.
    pub ahead: Option<i64>,
    /// When the status was requested.
    pub as_of: Time,
    /// Whether the environment has changed or disconnected since.
    pub stale: bool,
    /// Why a fact is unknown or stale.
    pub reason: Option<String>,
}

/// Independent nullable work flags and an unresolved last failed turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadAttention {
    /// The last known uncommitted flag.
    pub uncommitted: Option<bool>,
    /// The last known unpushed flag.
    pub unpushed: Option<bool>,
    /// The last turn failed and no later input was accepted.
    pub failed: bool,
    /// When the work was observed, if it was.
    pub as_of: Option<Time>,
    /// Whether the work observation is stale.
    pub stale: bool,
    /// Why its nullable facts are unknown or stale.
    pub reason: Option<String>,
}

/// The literal state identifying a worktree made from complete current files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeNow {
    /// Complete current included files.
    Now,
}

/// The base of an own attempt: complete current files or an explicit commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum WorktreeFrom {
    /// Complete current files, whose capture must be available.
    Now {
        /// The current-files discriminator.
        state: WorktreeNow,
    },
    /// One explicit commit, resolved and frozen in the plan.
    Commit {
        /// The commit to start from.
        commit: String,
    },
}

/// An unused directory and new branch for an own attempt in the same environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorktreeChoice {
    /// The workspace-relative repository directory.
    pub repository_dir: String,
    /// The new branch, never an existing checked-out branch.
    pub branch: String,
    /// The chosen base.
    pub from: WorktreeFrom,
    /// The unused workspace-relative target directory.
    pub directory: String,
}

/// A frozen line comment in its author's ordinary private draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewComment {
    /// The comment's identity for saves across devices.
    pub id: Id,
    /// The reviewed workspace-relative file.
    pub path: String,
    /// Which side of the difference its lines name.
    pub side: ReviewSide,
    /// The first reviewed line, starting at one.
    pub from_line: i64,
    /// The last reviewed line, inclusive.
    pub to_line: i64,
    /// The exact difference the person reviewed.
    pub diff_hash: Hash,
    /// Its timeline item, if the difference came from one.
    pub item_id: Option<Id>,
    /// The reviewed hunk, retained even after the live difference changes.
    pub hunk: String,
    /// The person's comment.
    pub text: String,
}

/// Which side of a difference a review comment names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSide {
    /// The old file.
    Old,
    /// The new file.
    New,
}

/// The literal expected version for an absent path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedAbsent {
    /// The path must not exist.
    None,
}

/// The expected current content or expected absence of a restore path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RestoreExpected {
    /// The current content hash.
    Hash(Hash),
    /// An absent path.
    Absent(ExpectedAbsent),
}

/// What one path will undergo in a restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestoreAction {
    /// Make an absent path.
    Add,
    /// Replace an existing path's content, type or executable bit.
    Change,
    /// Remove a path made after the recorded state.
    Remove,
}

/// A restore effect and what is known of later writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestorePath {
    /// The directory-relative path.
    pub path: String,
    /// The proposed effect.
    pub action: RestoreAction,
    /// The version the plan saw.
    pub expected: RestoreExpected,
    /// Whether this path differs from the turn's end; null means comparison is unavailable.
    pub later_change: Option<bool>,
    /// Visible known later writers only.
    pub later_writers: Vec<NodeRef>,
}

/// An overlapping writer, with its name hidden when the caller cannot read its thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestoreWriter {
    /// The visible thread, if any.
    pub thread: Option<NodeRef>,
    /// Whether it currently writes.
    pub active: bool,
}

/// A whole-directory restore that must be rechecked before any effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestorePlan {
    /// The plan's confirmation word.
    pub preflight_id: Id,
    /// When the plan expires.
    pub expires_at: Time,
    /// The complete recorded state being restored.
    pub snapshot: Snapshot,
    /// Its owning environment.
    pub environment: EnvironmentRef,
    /// The expected runtime generation.
    pub generation: i64,
    /// The same directory in which this state was recorded.
    pub directory: String,
    /// The complete included namespace, including absence, types and executable bits.
    pub namespace_fingerprint: Hash,
    /// The current ignore coverage.
    pub ignore_fingerprint: Hash,
    /// Every proposed addition, replacement or removal.
    pub paths: Vec<RestorePath>,
    /// The overlapping threads, with current activity.
    pub writers: Vec<RestoreWriter>,
    /// Paths outside the complete included coverage.
    pub ignored: Vec<String>,
    /// Whether a complete undo can be recorded.
    pub undo_available: bool,
    /// The resolved values frozen by the plan.
    pub options: AppliedChangeOptions,
    /// Reasons this plan cannot be confirmed.
    pub blockers: Vec<String>,
}

/// Retries preparation with the frozen source, or explicitly accepts the currently resolved one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSourcePrepareBody {
    /// The thread whose missing directory is prepared.
    pub thread_id: Id,
    /// Supplied only to accept the exact current resolved source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RepositorySource>,
}

/// Commits exactly the reviewed paths with the caller's server-provided identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentGitCommitBody {
    /// The repository directory.
    pub path: String,
    /// The selected changed paths within that repository.
    pub paths: Vec<String>,
    /// The required commit message.
    pub message: String,
    /// The reviewed HEAD, null in an unborn repository.
    pub expected_head: Option<String>,
    /// The reviewed staged and working state.
    pub expected_status: Hash,
}

/// Pushes the reviewed branch to an explicitly reviewed safe remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentGitPushBody {
    /// The repository directory.
    pub path: String,
    /// The remote's configured name.
    pub remote: String,
    /// Its reviewed safe URL; changes refuse the push before credentials leave.
    pub expected_remote_url: String,
    /// The branch to push.
    pub branch: String,
    /// The reviewed commit.
    pub expected_head: String,
    /// Whether this explicit push also sets the branch's upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_upstream: Option<bool>,
}

/// Requests a restore plan for a complete recorded state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentRestorePreflightBody {
    /// The target turn or undo state.
    pub snapshot_id: Id,
    /// Options whose omissions take the existing change settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ChangeOptions>,
}

/// Confirms one unexpired restore plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFilesRestoreBody {
    /// The exact plan shown to the caller.
    pub preflight_id: Id,
}

/// Replaces the explicitly accepted links of a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadLinksSetBody {
    /// The complete link list.
    pub links: Vec<ThreadLink>,
}

/// The result of a selected-path commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitCommitResult {
    /// The new commit.
    pub head: String,
    /// The protected status after the commit.
    pub status: GitStatus,
}

crate::endpoints! {
    /// Prepares or retries the frozen repository source before the retained first input.
    EnvironmentSourcePrepare: POST "/environments/{id}/source/prepare", Rule("environment.manage, thread.view, remote-write and local policy"), none, json EnvironmentSourcePrepareBody, json Job, 202;
    /// Commits exactly the reviewed changed paths as the caller.
    EnvironmentGitCommit: POST "/environments/{id}/git/commit", Rule("files.manage, remote-write and local policy"), query ThroughThreadQuery, json EnvironmentGitCommitBody, json Job, 202;
    /// Pushes the reviewed branch to its reviewed remote without force or implicit pull.
    EnvironmentGitPush: POST "/environments/{id}/git/push", Rule("files.manage, remote-write and local policy"), query ThroughThreadQuery, json EnvironmentGitPushBody, json Job, 202;
    /// Plans a complete recorded-file restore and names later changes and active writers.
    EnvironmentRestorePreflight: POST "/environments/{id}/restores/preflight", Rule("files.manage, remote-write and authority over affected active writers"), none, json EnvironmentRestorePreflightBody, json RestorePlan, 200;
    /// Rechecks and applies one restore plan, keeping a complete undo state.
    EnvironmentFilesRestore: POST "/environments/{id}/restores", Rule("files.manage, remote-write and authority over affected active writers"), none, json EnvironmentFilesRestoreBody, json Job, 202;
    /// Replaces the links accepted by an interactor on this thread.
    ThreadLinksSet: PUT "/threads/{id}/links", Permission("thread.interact"), none, json ThreadLinksSetBody, json Vec<ThreadLink>, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn own_attempts_require_one_explicit_base_and_an_unused_directory() {
        let choice: WorktreeChoice = serde_json::from_value(json!({
            "repository_dir": "repo", "directory": "attempt", "branch": "fix/second", "from": { "state": "now" }
        })).unwrap();
        assert!(matches!(
            choice.from,
            WorktreeFrom::Now {
                state: WorktreeNow::Now
            }
        ));
        let ambiguous = json!({ "repository_dir": "repo", "directory": "attempt", "branch": "fix/second", "from": { "state": "now", "commit": "abc" } });
        assert!(serde_json::from_value::<WorktreeChoice>(ambiguous).is_err());
    }

    #[test]
    fn unknown_work_is_nullable_and_never_a_false_claim_that_files_were_reviewed() {
        let attention = ThreadAttention::default();
        let shown = serde_json::to_value(attention).unwrap();
        assert!(shown["uncommitted"].is_null() && shown["unpushed"].is_null());
        assert_eq!(shown["failed"], false);
        assert!(shown.get("reviewed").is_none());
    }

    #[test]
    fn a_restore_path_distinguishes_absence_from_a_reviewed_hash() {
        let absent: RestoreExpected = serde_json::from_value(json!("none")).unwrap();
        assert_eq!(absent, RestoreExpected::Absent(ExpectedAbsent::None));
        assert!(serde_json::from_value::<RestoreExpected>(json!("missing")).is_err());
        let hash = Hash::of(b"reviewed");
        let expected: RestoreExpected = serde_json::from_value(json!(hash)).unwrap();
        assert_eq!(expected, RestoreExpected::Hash(hash));
    }
}
