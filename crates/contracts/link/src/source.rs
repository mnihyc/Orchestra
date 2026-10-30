//! Commands for repository preparation, immutable file states, restoration and publication.

use orch_api_contract::source::{
    GitCommitResult, RestorePath, SourceDestination, SourceInfo, WorktreeChoice,
};
use orch_api_contract::workspace::GitStatus;
use orch_types::{Hash, Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::commands::Secret;

/// Prepares a staged source checkout into a still-owned destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitPrepare {
    /// The owning environment.
    pub environment_id: Id,
    /// The durable preparation job.
    pub job_id: Id,
    /// The workspace-relative destination directory.
    pub directory: String,
    /// The frozen repository address without credentials.
    pub repository_url: String,
    /// The requested branch, or the advertised default.
    pub branch: Option<String>,
    /// Destination ownership frozen by the trusted control plane.
    pub destination: SourceDestination,
    /// A credential sealed only for this command and repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<Secret>,
}

/// Removes only empty, unchanged staging directories belonging to this preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitPrepareCleanup {
    /// The original effect and frozen destination; its credential is never needed for cleanup.
    pub original: GitPrepare,
}

/// Whether every owned stage is gone; installed checkouts and unknown bytes are always kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitPrepareCleanupResult {
    /// True only when no stage from this preparation remains and no checkout was installed.
    pub removed: bool,
    /// A bounded reason for anything retained.
    pub reason: Option<String>,
}

/// Creates a new branch and worktree without changing the source's index or working files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitWorktree {
    /// The owning environment.
    pub environment_id: Id,
    /// The durable job that owns the new directory.
    pub job_id: Id,
    /// The exact planned new branch, directory and base.
    pub choice: WorktreeChoice,
    /// The complete included current files for a From now attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<Id>,
}

/// Removes only the unchanged branch and directory owned by this original worktree request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitWorktreeCleanup {
    /// The original effect identity and frozen choice, never a new cleanup destination.
    pub original: GitWorktree,
}

/// Whether the job's worktree was removed or kept because another writer changed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitWorktreeCleanupResult {
    /// True only when no branch or directory owned by this job remains.
    pub removed: bool,
    /// A bounded explanation of anything deliberately kept.
    pub reason: Option<String>,
}

/// Repairs the worktree pointers named by an already copied workspace manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitWorktreeRepair {
    /// The destination environment, before the move publishes it.
    pub environment_id: Id,
    /// The move that owns this copy.
    pub job_id: Id,
    /// The exact copied namespace, with both metadata ends present.
    pub manifest_blob: Hash,
    /// The previous workspace root, used only for lexical translation.
    pub previous_root: String,
    /// The new root inside the destination workspace.
    pub target_root: String,
}

/// The number of copied worktree connections verified and repaired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitWorktreeRepairResult {
    /// Connections made usable at the destination.
    pub repaired: i64,
}

/// Measures the additional repository data needed when a selected worktree is copied out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitWorktreeCopyPlan {
    /// The source environment.
    pub environment_id: Id,
    /// Its selected directory.
    pub directory: String,
    /// The reviewed working-file selection, which must include .git for repository data to travel.
    pub selection: crate::commands::Selection,
}

/// Independent repository bytes are included in the file copy's plan before anything starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitWorktreeCopyPlanResult {
    /// Whether the selected .git file needs conversion to a private repository.
    pub independent: bool,
    /// Regular repository files replacing the one connection file.
    pub additional_files: i64,
    /// Repository bytes replacing the one connection file.
    pub additional_bytes: i64,
}

/// Snapshots selected files and a standalone repository when their .git names a linked worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitWorktreeSnapshot {
    /// The source environment.
    pub environment_id: Id,
    /// The file copy job.
    pub job_id: Id,
    /// The directory copied out.
    pub directory: String,
    /// Its reviewed working-file selection.
    pub selection: crate::commands::Selection,
}

/// The common snapshot and whether it contains an independent repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitWorktreeSnapshotResult {
    /// The actual counts and uploaded common manifest.
    pub snapshot: crate::commands::FsSnapshotResult,
    /// True when the connection file was replaced by complete local metadata.
    pub independent: bool,
}

/// Checks the complete copied repository before the fork can open a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitWorktreeStandalone {
    /// The destination environment.
    pub environment_id: Id,
    /// The copy job whose destination is checked.
    pub job_id: Id,
    /// The copied checkout directory inside this workspace.
    pub directory: String,
}

/// Verifies the carried immutable store before the old host's copy can be released.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsRecordVerify {
    /// The destination environment that owns this store.
    pub environment_id: Id,
}

/// Counts describe fully read and verified roots, required objects and persisted journals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsRecordVerifyResult {
    /// Root metadata kept here, including expired menu entries.
    pub roots: i64,
    /// Distinct required file/link objects whose content was verified.
    pub objects: i64,
    /// Parsed recovery/publication journals.
    pub journals: i64,
}

/// Machine-independent data roots; callers never supply a local path to private data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentDataPart {
    /// The environment user's home, excluding the independently carried recorded-state store.
    Home,
    /// Only immutable file states and their recovery journals, never connector credentials.
    FileStates,
}

/// Measures a data root before any move starts, on either a directory connector or a unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsDataMeasure {
    /// The background count's live reports and configured allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<crate::commands::WorkReport>,
    /// The environment whose attached roots are read.
    pub environment_id: Id,
    /// Which declared root is measured.
    pub part: EnvironmentDataPart,
}

/// Complete logical bytes and retained-state counts, with no invented zero for missing data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsDataMeasureResult {
    /// Regular files in the selected root.
    pub files: i64,
    /// Their logical size.
    pub bytes: i64,
    /// Verified retained-state dependencies for FileStates, null for Home.
    pub recorded: Option<FsRecordVerifyResult>,
}

/// Snapshots exactly one attached data root into the ordinary checked content store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsDataExport {
    /// The running job's reporting and configured time allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<crate::commands::WorkReport>,
    /// The source environment.
    pub environment_id: Id,
    /// The durable move that holds it.
    pub job_id: Id,
    /// The root included in the reviewed plan.
    pub part: EnvironmentDataPart,
}

/// Imports a reviewed data snapshot into a fresh destination generation, with hash checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsDataImport {
    /// The running job's reporting and configured time allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<crate::commands::WorkReport>,
    /// The destination environment before it becomes current.
    pub environment_id: Id,
    /// The move owning this fresh generation.
    pub job_id: Id,
    /// The only attached root that may be written.
    pub part: EnvironmentDataPart,
    /// The checked manifest produced by the corresponding export.
    pub manifest_blob: Hash,
}

/// The author and committer supplied by the server from its confirmed caller identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitAuthor {
    /// The caller's display name.
    pub name: String,
    /// The confirmed email, or the caller's generated address at orch.invalid.
    pub email: String,
}

/// Makes exactly the selected-path commit after rechecking the reviewed repository state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitCommit {
    /// The owning environment.
    pub environment_id: Id,
    /// The durable job identity used for reconciliation.
    pub job_id: Id,
    /// The repository directory.
    pub path: String,
    /// Exactly the reviewed selected repository-relative paths.
    pub paths: Vec<String>,
    /// The required commit message.
    pub message: String,
    /// The expected HEAD, null for an unborn branch.
    pub expected_head: Option<String>,
    /// The reviewed index and working-file fingerprint.
    pub expected_status: Hash,
    /// The identity frozen by the server, never the request caller's assertion.
    pub author: GitAuthor,
}

/// Pushes the expected commit to the expected remote, never forcing or implicitly pulling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitPush {
    /// The owning environment.
    pub environment_id: Id,
    /// The durable job identity used after a lost answer.
    pub job_id: Id,
    /// The repository directory.
    pub path: String,
    /// The configured remote name.
    pub remote: String,
    /// The reviewed safe remote address.
    pub expected_remote_url: String,
    /// The branch to send.
    pub branch: String,
    /// The reviewed commit.
    pub expected_head: String,
    /// Whether this explicit push also sets upstream.
    pub set_upstream: bool,
    /// The original source credential, scoped to its exact HTTPS repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<Secret>,
    /// The original exact repository address the sealed source credential may reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_repository_url: Option<String>,
}

/// The local protected status after a push; forge URLs are built by the control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitPushResult {
    /// The status after reconciliation or completion.
    pub status: GitStatus,
    /// The locally recorded remote default branch, never a guessed branch or a forge response.
    #[serde(default)]
    pub base_branch: Option<String>,
}

/// Why an immutable file state is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    /// Immediately before a turn.
    Turn,
    /// Complete undo files immediately before one restore.
    Restore,
}

/// Records from the complete index and watcher changes, never by scanning on a turn's path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsRecord {
    /// The owning environment.
    pub environment_id: Id,
    /// The immutable state identity; retries retain this exact state.
    pub snapshot_id: Id,
    /// The recorded directory.
    pub directory: String,
    /// Turn state or restore undo.
    pub kind: RecordKind,
    /// The frozen keeping period; zero disables recording.
    pub record_days: i64,
    /// The frozen included-byte bound.
    pub max_bytes: i64,
}

/// Whether a recording can be used as a whole directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    /// Every included entry is verified and retained.
    Complete,
    /// Coverage is unavailable, never partially offered as a whole state.
    Unavailable,
}

/// A recorded state's safe metadata; raw objects stay with the environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsRecordResult {
    /// Whether a whole state was kept.
    pub state: RecordState,
    /// Its immutable store tree identifier, not an API BLAKE3 identifier.
    pub tree_ref: Option<String>,
    /// The locally known project HEAD, without running Git during recording.
    pub repository_head: Option<String>,
    /// The uploaded common manifest's hash, when available.
    pub manifest_blob: Option<Hash>,
    /// How many included regular files.
    pub files: i64,
    /// How many included file and link-text bytes.
    pub bytes: i64,
    /// When this state expires, or null when recording was disabled.
    pub expires_at: Option<Time>,
    /// Why a complete state could not be made.
    pub reason: Option<String>,
}

/// Reads a verified immutable object, manifest or archive instead of newer live bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsRecordRead {
    /// The owning environment.
    pub environment_id: Id,
    /// The exact recorded state.
    pub snapshot_id: Id,
    /// One directory-relative file; omitted for a manifest or export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether an archive of the whole complete state is requested.
    pub export: bool,
}

/// The uploaded verified recorded bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsRecordReadResult {
    /// The uploaded blob's BLAKE3 identifier.
    pub blob: Hash,
    /// How many bytes it holds.
    pub size: i64,
}

/// Releases expired roots unless a running restore or exact-point fork still pins them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsRecordPrune {
    /// The owning environment.
    pub environment_id: Id,
    /// The states the existing reaper found expired.
    pub expired_snapshot_ids: Vec<Id>,
}

/// How many unpinned expired roots were released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsRecordPruneResult {
    /// The number of removed roots.
    pub removed: i64,
}

/// Compares a complete target with the current included namespace and ignore coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsRestorePlan {
    /// The owning environment.
    pub environment_id: Id,
    /// The target complete recorded state.
    pub snapshot_id: Id,
    /// The same recorded directory.
    pub directory: String,
    /// The turn whose end observation explains later changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Id>,
    /// Current recording policy frozen by the trusted control plane; old commands use the stored policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_days: Option<i64>,
    /// Current complete-undo byte bound, supplied with record_days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<i64>,
}

/// A machine's file plan, before the API adds visible writer names and permission checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsRestorePlanResult {
    /// The complete stored plan, with namespace and ignore fingerprints.
    pub plan_blob: Hash,
    /// Every planned path effect, with unknown later comparisons left null.
    pub paths: Vec<RestorePath>,
    /// Whether tracked writers are active.
    pub writers_active: bool,
    /// Whether a complete undo can be made under the frozen recording limits.
    pub undo_available: bool,
}

/// Rechecks, captures complete undo, stages, journals and applies exactly one whole restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsRestore {
    /// The running job's reporting and configured time allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<crate::commands::WorkReport>,
    /// The owning environment.
    pub environment_id: Id,
    /// The durable restore job.
    pub job_id: Id,
    /// The target complete state.
    pub snapshot_id: Id,
    /// The complete undo state identity reserved before any apply.
    pub undo_snapshot_id: Id,
    /// The exact stored plan.
    pub plan_blob: Hash,
    /// The reviewed environment generation.
    pub generation: i64,
}

/// The result of one applied or safely returned restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsRestoreResult {
    /// How many regular files were applied.
    pub files: i64,
    /// The complete saved prior state.
    pub undo_snapshot_id: Id,
    /// Whether an interrupted apply was returned to the prior state.
    pub undone: bool,
}

/// Observes only the changed paths reported by the index watcher, without Git or a tree walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceObserve {
    /// The owning environment.
    pub environment_id: Id,
    /// The thread whose directory was observed.
    pub thread_id: Id,
    /// The ended turn, when one exists.
    pub turn_id: Option<Id>,
    /// The workspace-relative directory.
    pub directory: String,
}

/// Complete end hashes, or unknown coverage on a live or not-ready tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceObserveResult {
    /// The uploaded common end manifest, or null when it cannot be complete.
    pub end_manifest_blob: Option<Hash>,
}

/// The prepared source's safe metadata.
pub type GitPrepareResult = SourceInfo;
/// The new worktree's safe metadata.
pub type GitWorktreeResult = SourceInfo;
/// The selected-path commit's result.
pub type GitCommitReply = GitCommitResult;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recording_reports_unavailability_without_a_partial_tree_or_claimed_manifest() {
        let result: FsRecordResult = serde_json::from_value(json!({
            "state": "unavailable", "tree_ref": null, "repository_head": null,
            "manifest_blob": null, "files": 0, "bytes": 0, "expires_at": null, "reason": "live_tree"
        }))
        .unwrap();
        assert_eq!(result.state, RecordState::Unavailable);
        assert_eq!(result.reason.as_deref(), Some("live_tree"));
        assert!(result.tree_ref.is_none() && result.manifest_blob.is_none());
    }
}
