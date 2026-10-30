//! Files with rename, copy and the search inside them, what an archive holds, uploads in chunks that go on where they stopped, snapshots with their archives, and cached views; the text a new thread's `AGENTS.md` is to hold; jobs with their phases, percentage and what a failed one undid.
//!
//! A workspace is the environment's, whichever thread, topic or page it is
//! reached from: its Files door is on the environment's own page, `files.view`
//! to look and `files.manage` to change, which whoever manages the
//! environment holds without being named. A request that names a thread says
//! beside which one the caller works and gives nothing. Paths are
//! `/`-separated, without `..` or a leading `/`, from the workspace root; the
//! empty string is the root. Reading
//! a file takes two steps, a fetch that reports whether that exact version is
//! in the blob store and then the content by its hash. A write names the
//! version it was made on, so that nobody overwrites what they have not seen.
//! A snapshot is a thread's directory kept as of a moment, and work that takes a
//! while, such as taking one, is a [`Job`].

use orch_types::{Hash, Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Page, PageQuery};
use crate::environment::{EnvironmentRef, ThroughThreadQuery};
use crate::forks::FilesSource;
use crate::nodes::JobState;
use crate::routes::Access::{Permission, Rule};
use crate::threads::FileChange;
use crate::users::UserRef;

/// The most seconds a request to fetch a file waits for it to arrive.
pub const FETCH_WAIT_MAX_SECONDS: u32 = 25;
/// What `expected_hash` is for a file that must not exist yet.
pub const EXPECTED_HASH_NONE: &str = "none";
/// The fewest characters a search for files may be asked.
pub const FILE_SEARCH_QUERY_MIN_CHARS: u32 = 2;
/// The most entries a search by name returns.
pub const FILE_SEARCH_NAMES_MAX: u32 = 200;
/// How many matches a search in content returns when it does not say.
pub const FILE_SEARCH_LIMIT_DEFAULT: u32 = 200;
/// The most matches a search in content may be asked for.
pub const FILE_SEARCH_LIMIT_MAX: u32 = 1_000;
/// How many characters of a line a match carries, cut around the match.
pub const FILE_MATCH_TEXT_CHARS: u32 = 400;
/// How many entries a listing of an archive holds when it is not told.
pub const ARCHIVE_LIST_LIMIT_DEFAULT: u32 = 1_000;
/// The most entries a listing of an archive may be asked for.
pub const ARCHIVE_LIST_LIMIT_MAX: u32 = 5_000;
/// The most bytes of a unified diff of a repository; more is cut and says so.
pub const GIT_DIFF_MAX_BYTES: i64 = 2 * 1024 * 1024;
/// The header that is `1` on a diff that was cut at [`GIT_DIFF_MAX_BYTES`].
pub const TRUNCATED_HEADER: &str = "x-truncated";

/// How far a phase of a job has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PhaseState {
    /// Not begun.
    Pending,
    /// Under way.
    Running,
    /// Done.
    Done,
    /// Left out.
    Skipped,
}

/// One phase of a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobPhase {
    /// Its name.
    pub name: String,
    /// Its share of the progress bar, out of one hundred for all phases.
    pub weight: u32,
    /// How far it has come.
    pub state: PhaseState,
}

/// How far a job has come.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobProgress {
    /// Freshness, directory counts and stopping status for work reported live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<JobActivity>,
    /// The phase it is in.
    pub phase: String,
    /// All its phases, in order.
    pub phases: Vec<JobPhase>,
    /// From 0 to 100, never decreasing.
    pub percent: u32,
    /// Files done.
    pub files_done: i64,
    /// Files in all, where known.
    pub files_total: Option<i64>,
    /// Bytes done.
    pub bytes_done: i64,
    /// Bytes in all, where known.
    pub bytes_total: Option<i64>,
    /// Bytes a second.
    pub rate_bps: Option<i64>,
    /// Seconds to go.
    pub eta_seconds: Option<i64>,
    /// A line to show beside the bar.
    pub message: Option<String>,
}

/// What the controller knows a running job is waiting for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobWaiting {
    /// A machine owns an outstanding command.
    Machine {
        /// Its display name.
        name: String,
    },
    /// A change of the environment must end before file work may begin.
    EnvironmentChange,
    /// Independent recovery or removal work must finish before its parent continues.
    Operations {
        /// Unfinished accepted operations.
        count: i64,
    },
    /// The remaining environments need their own managers to recover or remove them.
    HostEnvironments {
        /// Current environment dependents, counted by the shared facts service.
        count: i64,
    },
    /// A frozen recovery promise has not reached its purge date.
    RetainedData {
        /// The next promised end; the plan retains each item's full date.
        until: Time,
    },
}

/// Live activity accompanying a job's counters without changing its durable state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobActivity {
    /// What the controller knows this work is waiting for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<JobWaiting>,
    /// When a counter, phase or waiting reason last changed.
    pub updated_at: Time,
    /// Seconds without a change before the view marks the job as waiting.
    pub stall_after_seconds: i64,
    /// Directories counted so far, or null when this phase does not count them.
    pub directories_done: Option<i64>,
    /// Cancellation was requested and machine cleanup has not finished.
    pub stopping: bool,
}

/// Why a job failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobError {
    /// A stable code.
    pub code: String,
    /// What went wrong.
    pub message: String,
    /// The phase where it failed.
    pub phase: String,
    /// Whether everything it had made was removed again.
    pub undone: bool,
}

/// A job: work that takes a while and is carried out apart from the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Job {
    /// A model recovery awaited by this workflow, with its known due time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_recovery: Option<crate::calls::CallRecovery>,
    /// The job's identifier.
    pub id: Id,
    /// What it does, as a word such as `thread_fork`.
    pub kind: String,
    /// The node it is about.
    pub node_id: Option<Id>,
    /// The environment it is about.
    pub environment_id: Option<Id>,
    /// The provider it is about.
    pub provider_id: Option<Id>,
    /// The batch it belongs to.
    pub batch_id: Option<Id>,
    /// Where it stands.
    pub state: JobState,
    /// How far it has come.
    pub progress: JobProgress,
    /// What it produced; a fork's is the identifier of the new thread.
    pub result: Option<Json>,
    /// Why it failed.
    pub error: Option<JobError>,
    /// The end of what a phase printed, for the phases that print. For an image build this is at
    /// most 64 KiB of its output, which is kept when the build fails.
    pub log: Option<String>,
    /// Who made it.
    pub created_by: Option<UserRef>,
    /// When it was made.
    pub created_at: Time,
    /// When it began.
    pub started_at: Option<Time>,
    /// When it ended.
    pub finished_at: Option<Time>,
}

/// What `result` is on the job that takes a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotJobResult {
    /// The snapshot that was taken.
    pub snapshot_id: Id,
    /// Files listed without a readable copy; a nonzero count requires an incomplete-copy warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_count: Option<i64>,
}

/// What an entry of a directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DirEntryKind {
    /// A file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link.
    Symlink,
    /// Anything else.
    Other,
}

/// One entry of a directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DirEntry {
    /// Its name.
    pub name: String,
    /// Its path from the workspace root.
    pub path: String,
    /// What it is.
    #[serde(rename = "type")]
    pub kind: DirEntryKind,
    /// Its size in bytes.
    pub size: i64,
    /// When it was last changed.
    pub mtime: Time,
    /// Its permission bits.
    pub mode: u32,
    /// For a symbolic link, where it points.
    pub link_target: Option<String>,
    /// Whether its content is in the blob store for this exact version.
    pub cached: bool,
}

/// A directory and what is in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DirListing {
    /// The directory.
    pub path: String,
    /// The mount that was listed, when it was not the workspace.
    pub mount: Option<String>,
    /// Whether it was read from the environment now, or from the last manifest.
    pub source: FilesSource,
    /// Observation completion for live metadata; the retained source's time for cached metadata.
    pub as_of: Time,
    /// When this live metadata observation began; absent when the source cannot report it.
    /// Pages of the same revision keep this interval, not a claim of instantaneous membership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_from: Option<Time>,
    /// Its entries.
    pub entries: Vec<DirEntry>,
    /// Whether there are more than were listed.
    pub truncated: bool,
    /// Sorted immutable observed metadata; absent for an older unpaged source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Read the next page with the same environment/source/path; null means the actual end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Five minutes after page use, subject to earlier bounded-cache eviction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
}

/// One file or directory, with what is known of its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileStat {
    /// What a directory lists of it.
    #[serde(flatten)]
    pub entry: DirEntry,
    /// The hash of its content, where that is known.
    pub hash: Option<Hash>,
    /// Its media type.
    pub media_type: String,
    /// Whether it is text.
    pub is_text: bool,
}

/// Where a request to have a file brought to the blob store stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FetchStatus {
    /// That exact version is in the blob store.
    Ready,
    /// The environment was asked to upload it.
    Fetching,
    /// It cannot be had.
    Unavailable,
}

/// Why a file cannot be had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FetchUnavailable {
    /// The environment is offline and the file is not cached.
    OfflineNotCached,
    /// The ignore rules leave it out.
    Ignored,
    /// It is larger than a blob may be.
    TooLarge,
    /// It is not there.
    NotFound,
}

/// Where a file's way into the blob store stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FetchState {
    /// Where it stands.
    pub state: FetchStatus,
    /// The hash of the version, once it is known.
    pub hash: Option<Hash>,
    /// Its size in bytes, once it is known.
    pub size: Option<i64>,
    /// How many bytes have arrived, while it is fetching.
    pub received: Option<i64>,
    /// Why it cannot be had.
    pub reason: Option<FetchUnavailable>,
}

/// How far the taking of a snapshot has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotState {
    /// Being taken.
    Running,
    /// All of it was taken.
    Complete,
    /// Some files could not be taken.
    Partial,
    /// Nothing usable came of it.
    Failed,
    /// Recording could not supply a complete state, with its reason retained.
    Unavailable,
    /// The kept state expired; its metadata still explains the turn's fallback.
    Expired,
}

/// Why a snapshot exists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    /// A manually requested blob snapshot.
    #[default]
    Manual,
    /// Files immediately before a turn.
    Turn,
    /// Complete undo files before a restore.
    Restore,
}

/// Where the immutable snapshot bytes belong.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotStorage {
    /// The server's existing blob store.
    #[default]
    Blobs,
    /// The recorded state's owning environment, outside its workspace.
    Environment,
}

/// Which files a snapshot was asked to take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotSelection {
    /// Patterns of what to take.
    pub include: Vec<String>,
    /// Patterns of what to leave out of that.
    pub exclude: Vec<String>,
}

/// A thread's directory kept as of a moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Snapshot {
    /// The snapshot's identifier.
    pub id: Id,
    /// The thread it was taken for; absent for a whole-workspace export.
    pub thread_id: Option<Id>,
    /// The environment it was taken in.
    pub environment: EnvironmentRef,
    /// What it is called.
    pub label: String,
    /// Why this state was kept.
    pub kind: SnapshotKind,
    /// Where its immutable bytes are held.
    pub storage: SnapshotStorage,
    /// The repository's HEAD when recorded, when locally known.
    pub repository_head: Option<String>,
    /// The recorded workspace-relative directory, null for older manual snapshots.
    pub directory: Option<String>,
    /// When an environment-owned state expires; manual snapshots do not expire this way.
    pub expires_at: Option<Time>,
    /// Why a recording cannot supply a whole state.
    pub unavailable_reason: Option<String>,
    /// Where it stands.
    pub state: SnapshotState,
    /// What it was asked to take.
    pub selection: SnapshotSelection,
    /// How many files it holds.
    pub file_count: i64,
    /// How many bytes it holds.
    pub total_bytes: i64,
    /// How many files could not be taken.
    pub missing_count: i64,
    /// The generation of the environment's unit when it was taken.
    pub env_generation: i64,
    /// Who took it.
    pub created_by: Option<UserRef>,
    /// When it was started.
    pub created_at: Time,
    /// When it ended.
    pub completed_at: Option<Time>,
}

/// One line of a file that a search in content found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileMatch {
    /// The file.
    pub path: String,
    /// The line, counted from one.
    pub line: i64,
    /// The line, cut to [`FILE_MATCH_TEXT_CHARS`] characters around the match.
    pub text: String,
    /// Where in `text` it matches, as pairs of start and end.
    pub ranges: Vec<(u32, u32)>,
}

/// What a search in content found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileSearchContent {
    /// The lines that hold it.
    pub matches: Vec<FileMatch>,
    /// Whether there were more than were returned.
    pub truncated: bool,
    /// How many files were searched.
    pub searched: i64,
}

/// What a search in files looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileSearchIn {
    /// A substring of the path, without regard to case, in the latest manifest.
    Names,
    /// The lines of text files, in the environment as it is now.
    Content,
}

/// The kind of compression an archive has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveCompression {
    /// gzip.
    Gzip,
    /// Zstandard.
    Zstd,
    /// xz.
    Xz,
    /// bzip2.
    Bzip2,
}

/// The format of an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    /// A zip file.
    Zip,
    /// A tar file, compressed or not.
    Tar,
}

/// What an entry of an archive is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveEntryKind {
    /// A file.
    File,
    /// A directory.
    Dir,
    /// A link.
    Link,
}

/// One entry of an archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveEntry {
    /// Its path inside the archive.
    pub path: String,
    /// What it is.
    pub kind: ArchiveEntryKind,
    /// Its size in bytes, where the format says.
    pub size: Option<i64>,
    /// When it was changed, where the format says.
    pub modified_at: Option<Time>,
}

/// What an archive holds, read where the file lies and without unpacking it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveListing {
    /// Its format.
    pub format: ArchiveFormat,
    /// How it is compressed, if it is.
    pub compression: Option<ArchiveCompression>,
    /// The entries of the directory asked for, directories first.
    pub entries: Vec<ArchiveEntry>,
    /// Whether there are more than the limit.
    pub truncated: bool,
    /// Whether it asks for a password. It is listed where its format lets names be read, and no
    /// entry of it is opened.
    pub encrypted: bool,
}

/// Why no repository can be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitUnavailable {
    /// The directory is in no repository.
    NotARepository,
    /// The environment has no `git`.
    GitMissing,
    /// The environment is offline.
    Offline,
}

/// One changed path of a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitChange {
    /// The path.
    pub path: String,
    /// The status letter in the index, as `git status --porcelain` writes it.
    pub index: String,
    /// The status letter in the work tree, as `git status --porcelain` writes it.
    pub worktree: String,
}

/// The state of a repository in a workspace. It is read-only and best-effort: it runs the
/// environment's own `git` in the workspace, which has to be online.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitStatus {
    /// Whether it could be read.
    pub available: bool,
    /// Why not, when not.
    pub reason: Option<GitUnavailable>,
    /// The discovered workspace-relative repository directory.
    pub repository_dir: Option<String>,
    /// The canonical reviewed HEAD, index entries and changed working bytes.
    pub status_hash: Option<Hash>,
    /// Configured local remotes with embedded credentials removed.
    pub remotes: Vec<GitStatusRemote>,
    /// The branch's configured upstream, when known.
    pub upstream: Option<String>,
    /// The branch checked out.
    pub branch: Option<String>,
    /// The commit checked out.
    pub head: Option<String>,
    /// How many commits it is ahead of its upstream.
    pub ahead: Option<i64>,
    /// How many commits it is behind its upstream.
    pub behind: Option<i64>,
    /// The changed paths.
    pub changes: Vec<GitChange>,
}

/// A locally configured Git destination, with authentication material removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitStatusRemote {
    /// Its configured name.
    pub name: String,
    /// Its safe display URL.
    pub url: String,
}

/// A file change an agent made, with where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentChange {
    /// The change.
    #[serde(flatten)]
    pub change: FileChange,
    /// The item of the thread whose tool call made it.
    pub item_id: Id,
    /// The thread.
    pub thread_id: Id,
    /// When.
    pub at: Time,
}

/// The latest change to each path, from the file-change tool calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentChangeListReply {
    /// The changes.
    pub changes: Vec<EnvironmentChange>,
}

/// What a search in files found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EnvironmentFileSearchReply {
    /// For a search by name: at most [`FILE_SEARCH_NAMES_MAX`] entries.
    Names(Vec<DirEntry>),
    /// For a search in content.
    Content(FileSearchContent),
}

/// Which listing a directory is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileListSource {
    /// Live when the environment is online, otherwise the last manifest.
    Auto,
    /// The environment as it is now; an offline environment answers `source.offline`.
    Live,
    /// The last manifest.
    Cached,
}

/// A directory to list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFileListQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// The directory; the empty string is the workspace root.
    pub path: String,
    /// Where to read it from; `auto` when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<FileListSource>,
    /// A mount of the environment to list instead of the workspace; `path` is then relative to
    /// it. A mount is read live only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// Opaque continuation of exactly this environment/source/path membership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// One through 5,000 rows, with an independent two-MiB serialized page bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A file or directory to describe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFileStatQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// The path.
    pub path: String,
    /// A mount of the environment, as for a listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
}

/// How long to wait for a file to arrive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFileFetchQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// A mount of the environment, as for a listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// Seconds to hold the call while the state is `fetching`, at most
    /// [`FETCH_WAIT_MAX_SECONDS`]; the call returns at once when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<u32>,
}

/// A path alone: the file to fetch, or the directory to make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilePathBody {
    /// The path from the workspace root.
    pub path: String,
}

/// One version of a file to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFileContentGetQuery {
    /// Return a bounded static preview instead of the original file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<bool>,
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// A mount of the environment, as for a listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// The path.
    pub path: String,
    /// The hash the fetch reported, which has to belong to that path in this environment.
    pub hash: Hash,
    /// Whether to serve it as a download rather than for showing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<bool>,
}

/// A file to write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFileContentPutQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// The path.
    pub path: String,
    /// The hash of the version the writer loaded, or [`EXPECTED_HASH_NONE`] for a file that is
    /// not to exist yet. When the file is another version by then, the write is refused with the
    /// file as it stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_hash: Option<String>,
    /// The hash of an upload that already holds the content, in place of a body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<Hash>,
    /// The browser's upload batch, so its file writes form one history entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_batch: Option<Id>,
}

/// One completed private upload to write through a durable, cancellable job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentUploadWriteBody {
    /// Its workspace-relative destination.
    pub path: String,
    /// Its completed content, which the caller must still be allowed to attach.
    pub blob: Hash,
    /// The expected version, or `none` for a destination which must still be absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_hash: Option<String>,
    /// The batch that groups this actual write with its other files in History.
    pub upload_batch: Id,
}

/// How a browser upload batch or one of its files ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceUploadOutcome {
    /// All requested bytes were written.
    Complete,
    /// A transfer or final write failed.
    Failed,
    /// The person cancelled unfinished work.
    Cancelled,
}

/// One selected file, including the upload session that proves how many bytes arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceUploadAttempt {
    /// Its selected relative name, including folders when a directory was dropped.
    pub name: String,
    /// Its selected size in bytes.
    pub size: i64,
    /// Its upload session, where it began one.
    pub upload_id: Option<Id>,
    /// Its completed bytes, where upload finished before its workspace write failed.
    pub blob: Option<Hash>,
    /// How this attempt ended.
    pub outcome: WorkspaceUploadOutcome,
}

/// Completes the history of one upload without sending a notice to anybody.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentUploadFinishBody {
    /// The chosen workspace folder, empty at the root.
    pub folder: String,
    /// The selected files and the sessions whose partial progress is retained.
    pub files: Vec<WorkspaceUploadAttempt>,
    /// How the batch ended.
    pub outcome: WorkspaceUploadOutcome,
}

/// A path to move or to copy, and where to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileFromToBody {
    /// The path now.
    pub from: String,
    /// The path to put it at; a file or a directory with all that is in it.
    pub to: String,
}

/// A path to delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFileDeleteQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// The path.
    pub path: String,
    /// Whether to delete a directory with what is in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
}

/// Which changes to list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentChangeListQuery {
    /// The thread the caller works beside, which gives nothing: its changes are listed, and without a
    /// thread those of every thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// Only the changes of this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Id>,
    /// With a thread: the changes of every thread of the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,
}

/// What to look for in the files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFileSearchQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// What to look for, at least [`FILE_SEARCH_QUERY_MIN_CHARS`] characters.
    pub q: String,
    /// Names, or content; names when left out.
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub scope: Option<FileSearchIn>,
    /// With content: a directory to stay in; the thread's own unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// With content: which file names to look in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// With content: whether `q` is a regular expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<bool>,
    /// With content: whether case matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<bool>,
    /// With content: how many matches at most, [`FILE_SEARCH_LIMIT_DEFAULT`] when left out and at
    /// most [`FILE_SEARCH_LIMIT_MAX`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// An archive to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentArchiveListQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// The archive: a zip, or a tar with or without gzip, zstd, xz or bzip2.
    pub path: String,
    /// A directory within it to list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<String>,
    /// How many entries at most, [`ARCHIVE_LIST_LIMIT_DEFAULT`] when left out and at most
    /// [`ARCHIVE_LIST_LIMIT_MAX`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// One entry of an archive to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentArchiveEntryGetQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// The archive.
    pub path: String,
    /// A file within it.
    pub entry: String,
}

/// A repository to read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentGitStatusGetQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// The repository's directory; the workspace root when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A diff to read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentGitDiffGetQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// The repository's directory; the workspace root when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether to diff what is staged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<bool>,
}

/// Which files a snapshot takes; the thread's directory when none is given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadSnapshotCreateSelection {
    /// Patterns of what to take.
    pub include: Vec<String>,
    /// Patterns of what to leave out of that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

/// Takes a snapshot of a thread's directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadSnapshotCreateBody {
    /// What to call it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Which files; the thread's directory unless a selection says otherwise. Where the tree is too
    /// large to index, a selection is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<ThreadSnapshotCreateSelection>,
}

/// A path inside a snapshot.
/// Measures a snapshot selection before confirming it. No snapshot or job is made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadSnapshotPreflightBody {
    /// Exactly the paths to take, with ignored paths left out; required for a live tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<ThreadSnapshotCreateSelection>,
}

/// Why a selection cannot yet be taken as a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotSelectionRefusal {
    /// More than 250,000 entries were selected.
    EntryLimit,
    /// The selection could not be measured within ten seconds.
    ScanTime,
    /// Some selected entries could not be measured.
    Unreadable,
    /// A live tree requires explicit paths.
    SelectionRequired,
}

/// A directory among the ten largest in the measured selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotDirectoryMeasure {
    /// Path relative to the environment's workspace.
    pub path: String,
    /// Selected regular files below it.
    pub file_count: i64,
    /// Bytes in those files.
    pub total_bytes: i64,
}

/// A read-only snapshot preflight. Incomplete counts are lower bounds, never an approved size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotPreflight {
    /// The exact normalized selection that was measured.
    pub selection: SnapshotSelection,
    /// Files, directories, links and other selected entries together.
    pub entry_count: i64,
    /// Selected regular files.
    pub file_count: i64,
    /// Selected directories.
    pub directory_count: i64,
    /// Bytes in the selected regular files.
    pub total_bytes: i64,
    /// At most ten directories, ordered by bytes, then path.
    pub largest_directories: Vec<SnapshotDirectoryMeasure>,
    /// Whether the complete selection fits the limits; creation measures it again before making a job.
    pub allowed: bool,
    /// Why it cannot start, or null when allowed.
    pub refusal: Option<SnapshotSelectionRefusal>,
    /// A line explaining a refusal, or null when allowed.
    pub reason: Option<String>,
    /// The maximum number of selected entries.
    pub max_entries: i64,
    /// The maximum time spent measuring a selection.
    pub scan_seconds: u64,
    /// Time to scan, hash and upload, with whether its rates were measured or assumed.
    pub estimate: crate::forks::Estimate,
}

/// A path inside a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotPathQuery {
    /// The path from the snapshot's root.
    pub path: String,
}

/// How a snapshot is packed as one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum SnapshotArchiveFormat {
    /// A tar compressed with Zstandard.
    #[serde(rename = "tar.zst")]
    TarZst,
    /// A zip, which every desktop opens: for a person who downloads a folder.
    #[serde(rename = "zip")]
    Zip,
}

/// How to pack a snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotArchiveGetQuery {
    /// The format; `tar.zst` when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<SnapshotArchiveFormat>,
}

/// Renames a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotUpdateBody {
    /// The new label.
    pub label: String,
}

crate::endpoints! {
    /// Lists a directory of an environment's workspace, or of one of its mounts, live or from the last manifest.
    EnvironmentFileList: GET "/environments/{id}/files", Permission("files.view"), query EnvironmentFileListQuery, none, json DirListing, 200;
    /// Describes one file or directory of an environment.
    EnvironmentFileStat: GET "/environments/{id}/files/stat", Permission("files.view"), query EnvironmentFileStatQuery, none, json FileStat, 200;
    /// Returns a bounded preview of the current workspace file without downloading its original to the browser.
    EnvironmentFilePreviewGet: GET "/environments/{id}/files/preview", Permission("files.view"), query EnvironmentFileStatQuery, none, raw, 200;
    /// Asks for one version of a file to be in the blob store and says where that stands, waiting a while if asked to.
    EnvironmentFileFetch: POST "/environments/{id}/files/fetch", Permission("files.view"), query EnvironmentFileFetchQuery, json FilePathBody, json FetchState, 200;
    /// Serves the bytes of one version of a file, found by its hash, with the hash as its ETag and ranges supported.
    EnvironmentFileContentGet: GET "/environments/{id}/files/content", Permission("files.view"), query EnvironmentFileContentGetQuery, none, raw, 200;
    /// Writes a file of an environment from the body or from an upload, if the file is still the version the writer saw.
    EnvironmentFileContentPut: PUT "/environments/{id}/files/content", Permission("files.manage"), query EnvironmentFileContentPutQuery, bytes, json FileStat, 200;
    /// Writes completed uploaded content as a job, whose cancellation waits for the machine to stop.
    EnvironmentUploadWrite: POST "/environments/{id}/files/uploads/{upload_id}/write", Permission("files.manage"), query ThroughThreadQuery, json EnvironmentUploadWriteBody, json Job, 202;
    /// Reads the confirmed file receipt, or the actual refusal, after an upload job ends.
    EnvironmentUploadWriteResult: GET "/environments/{id}/files/uploads/{upload_id}/write", Permission("files.view"), query ThroughThreadQuery, none, json FileStat, 200;
    /// Groups the caller's upload writes and retained partial progress into one history entry.
    EnvironmentUploadFinish: POST "/environments/{id}/files/uploads/{upload_id}/finish", Permission("files.manage"), query ThroughThreadQuery, json EnvironmentUploadFinishBody, none, 204;
    /// Makes a directory in an environment's workspace.
    EnvironmentFileMkdir: POST "/environments/{id}/files/mkdir", Permission("files.manage"), query ThroughThreadQuery, json FilePathBody, json FileStat, 200;
    /// Renames a file or a directory, which is also how one is moved.
    EnvironmentFileRename: POST "/environments/{id}/files/rename", Permission("files.manage"), query ThroughThreadQuery, json FileFromToBody, json FileStat, 200;
    /// Copies a file, or a directory with what is in it.
    EnvironmentFileCopy: POST "/environments/{id}/files/copy", Permission("files.manage"), query ThroughThreadQuery, json FileFromToBody, json FileStat, 200;
    /// Deletes a file, or a directory with what is in it.
    EnvironmentFileDelete: DELETE "/environments/{id}/files", Permission("files.manage"), query EnvironmentFileDeleteQuery, none, none, 204;
    /// Lists the latest change to each path that agents made, from their file-change tool calls.
    EnvironmentChangeList: GET "/environments/{id}/changes", Permission("files.view"), query EnvironmentChangeListQuery, none, json EnvironmentChangeListReply, 200;
    /// Searches an environment's files by name, or the lines of its text files.
    EnvironmentFileSearch: GET "/environments/{id}/files/search", Permission("files.view"), query EnvironmentFileSearchQuery, none, json EnvironmentFileSearchReply, 200;
    /// Lists what an archive holds, without unpacking it.
    EnvironmentArchiveList: GET "/environments/{id}/files/archive", Permission("files.view"), query EnvironmentArchiveListQuery, none, json ArchiveListing, 200;
    /// Serves the bytes of one entry of an archive for a viewer; nothing is written anywhere.
    EnvironmentArchiveEntryGet: GET "/environments/{id}/files/archive/entry", Permission("files.view"), query EnvironmentArchiveEntryGetQuery, none, raw, 200;
    /// Reads the state of a repository in the workspace.
    EnvironmentGitStatusGet: GET "/environments/{id}/git/status", Permission("files.view"), query EnvironmentGitStatusGetQuery, none, json GitStatus, 200;
    /// Reads the unified diff of a repository in the workspace, as plain text.
    EnvironmentGitDiffGet: GET "/environments/{id}/git/diff", Permission("files.view"), query EnvironmentGitDiffGetQuery, none, raw, 200;
    /// Lists the snapshots of a thread, newest first.
    ThreadSnapshotList: GET "/threads/{id}/snapshots", Rule("files.view on the thread's environment"), query PageQuery, none, json Page<Snapshot>, 200;
    /// Starts taking a snapshot of a thread's directory as a job whose result names the snapshot.
    ThreadSnapshotCreate: POST "/threads/{id}/snapshots", Rule("files.manage on the thread's environment"), none, json ThreadSnapshotCreateBody, json Job, 202;
    /// Captures an owner's complete workspace through the same leased snapshot job, even without a thread.
    EnvironmentFilesExport: POST "/environments/{id}/files/export", Rule("environment.manage"), none, none, json Job, 202;
    /// Measures selected files and refuses an oversized or slow selection before any snapshot job exists.
    ThreadSnapshotPreflight: POST "/threads/{id}/snapshots/preflight", Rule("files.manage on the thread's environment"), none, json ThreadSnapshotPreflightBody, json SnapshotPreflight, 200;
    /// Reads one snapshot.
    SnapshotGet: GET "/snapshots/{id}", Rule("files.view on the environment its thread runs in now"), none, none, json Snapshot, 200;
    /// Lists a directory of a snapshot.
    SnapshotFileList: GET "/snapshots/{id}/files", Rule("files.view on the environment its thread runs in now"), query SnapshotPathQuery, none, json DirListing, 200;
    /// Serves the bytes of one file of a snapshot.
    SnapshotFileContentGet: GET "/snapshots/{id}/files/content", Rule("files.view on the environment its thread runs in now"), query SnapshotPathQuery, none, raw, 200;
    /// Serves a snapshot as one stream.
    SnapshotArchiveGet: GET "/snapshots/{id}/archive", Rule("files.view on the environment its thread runs in now"), query SnapshotArchiveGetQuery, none, raw, 200;
    /// Renames a snapshot.
    SnapshotUpdate: PATCH "/snapshots/{id}", Rule("files.manage on the environment its thread runs in now"), none, json SnapshotUpdateBody, json Snapshot, 200;
    /// Deletes a snapshot and unpins its blobs.
    SnapshotDelete: DELETE "/snapshots/{id}", Rule("files.manage on the environment its thread runs in now"), none, none, none, 204;
    /// Reads a job.
    JobGet: GET "/jobs/{id}", Rule("node.view on the job's thread, any permission on its environment, or provider.manage on its provider; an export's maker, or current access to a measurement's subject"), none, none, json Job, 200;
    /// Cancels a job that has not passed its commit phase; one that has is undone like a failure.
    JobCancel: POST "/jobs/{id}/cancel", Rule("whoever could have started it"), none, none, json Job, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn directory_observation_start_is_additive_and_legacy_or_cached_times_remain_unknown() {
        let legacy = json!({"path":"src","mount":null,"source":"live","as_of":"2026-10-08T17:04:09.456Z",
            "entries":[],"truncated":false});
        let read: DirListing = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(read.observed_from, None);
        assert_eq!(serde_json::to_value(read).unwrap(), legacy);
        let mut current = legacy;
        current["observed_from"] = json!("2026-10-08T17:04:05.123Z");
        let read: DirListing = serde_json::from_value(current.clone()).unwrap();
        assert!(read.observed_from.is_some_and(|start| start < read.as_of));
        assert_eq!(serde_json::to_value(read).unwrap(), current);
    }

    #[test]
    fn a_job_that_failed_says_whether_everything_it_made_was_removed() {
        let job: Job = serde_json::from_value(json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "thread_fork",
            "node_id": null, "environment_id": null, "provider_id": null, "batch_id": null,
            "state": "failed",
            "progress": {
                "phase": "copy", "phases": [{ "name": "copy", "weight": 100, "state": "running" }], "percent": 40,
                "files_done": 4, "files_total": 10, "bytes_done": 400, "bytes_total": null, "rate_bps": null, "eta_seconds": null, "message": null
            },
            "result": null,
            "error": { "code": "environment.unavailable", "message": "The host went away.", "phase": "copy", "undone": true },
            "log": null, "created_by": null, "created_at": "2026-10-08T17:04:05.123Z", "started_at": null, "finished_at": null
        }))
        .unwrap();
        assert_eq!(job.state, JobState::Failed);
        assert!(job.error.is_some_and(|error| error.undone));
    }

    #[test]
    fn a_snapshot_says_what_it_was_asked_to_take_and_how_many_files_could_not_be() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b",
            "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c",
            "environment": {
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6d", "name": "Main",
                "owner": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6e", "kind": "human", "username": "bo", "display_name": "Bo", "avatar": { "text": "B", "color": 2 } },
                "address": "acme.bo.main", "ports_exposed": false, "placement": "managed", "status": "running"
            },
            "label": "Before the refactor", "kind": "manual", "storage": "blobs",
            "repository_head": null, "directory": null, "expires_at": null, "unavailable_reason": null,
            "state": "partial",
            "selection": { "include": ["src/**"], "exclude": ["target/**"] },
            "file_count": 120, "total_bytes": 4096000, "missing_count": 2, "env_generation": 3,
            "created_by": null, "created_at": "2026-10-08T17:04:05.123Z", "completed_at": "2026-10-08T17:04:09.456Z"
        });
        let snapshot: Snapshot = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(snapshot.state, SnapshotState::Partial);
        assert_eq!(snapshot.missing_count, 2);
        assert_eq!(snapshot.selection.exclude, ["target/**"]);
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), written);
    }

    #[test]
    fn a_file_is_a_directory_entry_with_what_is_known_of_its_content() {
        let written = json!({
            "name": "main.rs", "path": "src/main.rs", "type": "file", "size": 1204,
            "mtime": "2026-10-08T17:04:05.123Z", "mode": 420, "link_target": null, "cached": true,
            "hash": "7a1e0d9c3b5f4a2e8d6c1b0a9f8e7d6c5b4a39281706f5e4d3c2b1a09f8e7d6c",
            "media_type": "text/x-rust", "is_text": true
        });
        let stat: FileStat = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(stat.entry.kind, DirEntryKind::File);
        assert!(stat.is_text && stat.hash.is_some());
        assert_eq!(serde_json::to_value(&stat).unwrap(), written);
    }

    #[test]
    fn a_search_answers_with_names_or_with_the_lines_that_hold_the_text() {
        let by_content: EnvironmentFileSearchReply = serde_json::from_value(json!({
            "matches": [{ "path": "src/lib.rs", "line": 12, "text": "fn parse()", "ranges": [[3, 8]] }],
            "truncated": false, "searched": 41
        }))
        .unwrap();
        assert!(matches!(
            by_content,
            EnvironmentFileSearchReply::Content(FileSearchContent { searched: 41, .. })
        ));
        let by_name: EnvironmentFileSearchReply = serde_json::from_value(json!([])).unwrap();
        assert!(matches!(by_name, EnvironmentFileSearchReply::Names(names) if names.is_empty()));
        let query: EnvironmentFileSearchQuery = serde_json::from_value(
            json!({ "q": "parse", "in": "content", "glob": "*.rs", "case": true }),
        )
        .unwrap();
        assert_eq!(query.scope, Some(FileSearchIn::Content));
        assert_eq!(query.regex, None);
    }

    #[test]
    fn a_write_names_the_version_it_was_made_on_or_that_none_was_there() {
        let query: EnvironmentFileContentPutQuery = serde_json::from_value(
            json!({ "path": "notes.md", "expected_hash": EXPECTED_HASH_NONE }),
        )
        .unwrap();
        assert_eq!(query.expected_hash.as_deref(), Some("none"));
        assert_eq!(query.blob, None);
        assert!(
            serde_json::from_value::<EnvironmentFileContentPutQuery>(
                json!({ "expected_hash": "x" })
            )
            .is_err(),
            "a write names its path"
        );
    }

    #[test]
    fn a_snapshot_is_packed_as_a_zstd_tar_unless_a_zip_is_asked_for() {
        let default: SnapshotArchiveGetQuery = serde_json::from_value(json!({})).unwrap();
        assert_eq!(default.format, None);
        let zip: SnapshotArchiveGetQuery =
            serde_json::from_value(json!({ "format": "zip" })).unwrap();
        assert_eq!(zip.format, Some(SnapshotArchiveFormat::Zip));
        assert_eq!(
            serde_json::to_value(SnapshotArchiveFormat::TarZst).unwrap(),
            json!("tar.zst")
        );
    }

    #[test]
    fn an_archive_that_asks_for_a_password_is_listed_and_no_entry_is_opened() {
        let listing: ArchiveListing = serde_json::from_value(json!({
            "format": "zip", "compression": null,
            "entries": [{ "path": "docs/", "kind": "dir", "size": null, "modified_at": null }],
            "truncated": false, "encrypted": true
        }))
        .unwrap();
        assert!(listing.encrypted);
        assert_eq!(listing.entries[0].kind, ArchiveEntryKind::Dir);
    }
}
