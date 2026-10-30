//! Imports: what a person brings into a place from an Orch package or from an archive of plain files, planned before anything is made, asked for as a job, and kept as a report, which can be undone as a whole.
//!
//! The same flow serves two kinds of source. An **Orch package** (the archive an export makes with
//! `package: true`, [`crate::package`]) brings places, topics, document folders, documents, chats and threads
//! with their ids mapped, their times kept and the people they name matched. An **archive of plain files**, or
//! loose files, of Markdown, HTML, Word and text brings folders as document folders and files as documents, with
//! the links between them and the files they name made into references and document files.
//!
//! A person first asks what an import would do ([`NodeImportPreflight`], an [`ImportPlan`]: nothing is written), may
//! look at any item of it ([`NodeImportPreview`]), then asks for it ([`NodeImportCreate`], a job). The import is
//! the person's own record ([`Import`]) with its report, a row for each object, and it can be undone ([`ImportUndo`])
//! after a plan of what that would take away ([`ImportUndoPreflight`]).

use orch_types::{Hash, Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Page, PageQuery};
use crate::exports::ExportParts;
use crate::nodes::NodeRef;
use crate::package::{PackageHeader, PackageKind};
use crate::routes::Access::{Permission, Rule, User};
use crate::users::UserRef;
use crate::workspace::Job;

/// The most bytes of text a preview of one item shows: enough to judge a document by, and short enough to
/// come back at once.
pub const IMPORT_PREVIEW_MAX_BYTES: i64 = 65_536;

/// What an import is made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportSourceMode {
    /// An Orch package: one archive with a manifest at its root.
    Package,
    /// Files: Markdown, HTML, Word and text, loose or in an archive, which keep their folders.
    Files,
}

/// One file of the source, uploaded already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportSourceFile {
    /// Its name, with the folders it lay in where the person chose a folder: a path relative to the chosen folder.
    pub name: String,
    /// Where its content is in the store: the hash of an upload of the person's.
    pub blob: Hash,
}

/// What an import is made from: the uploaded files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportSource {
    /// The files. One archive, or any number of loose files; a file of a folder keeps its path in its name.
    pub files: Vec<ImportSourceFile>,
    /// What they are, where the person says so. A single archive with a manifest at its root is a package, and
    /// anything else is files, unless this says otherwise: an Orch package can be read as the plain files it
    /// holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ImportSourceMode>,
}

/// What is done with an object whose counterpart is there already, or whose name is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportAction {
    /// A new object: there is nothing it could be taken for.
    Create,
    /// A new object beside the one that is there, under a name that tells them apart.
    KeepBoth,
    /// The one that is there takes the new content, and keeps its identifier, its place and its sharing. A
    /// document cuts a version first, so that what it said stays.
    Replace,
    /// Nothing is done with it. For a place or a folder, nothing of what lies in it either.
    Skip,
}

/// Where the root of a package goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportRoot {
    /// The place the package is of is made inside the place chosen, or found there: a project becomes a project
    /// inside it.
    #[default]
    Inside,
    /// What lay in the place the package is of lies in the place chosen itself.
    Here,
}

/// Where the import goes besides the place of the request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportDestination {
    /// The document folder of the place that documents go into; the place's own documents where it is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<Id>,
    /// Where the root of a package goes; inside the place unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<ImportRoot>,
}

/// Whom the things of the package are written by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportAuthors {
    /// By the accounts the people of the package are matched to. An Admin of the place may ask for it.
    Accounts,
    /// By the importer, with the name of the one who wrote each thing shown as "originally by".
    Labels,
}

/// What the person chose to bring, and how conflicts are settled by default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    /// Which kinds of thing; all three unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<ExportParts>,
    /// The earlier versions of documents, where the package has them; `true` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<bool>,
    /// The discussions of documents; `true` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<bool>,
    /// Whether the sharing the package carries is restored, within what the importer holds and what the
    /// place's locks allow; `false` unless given, and refused for one who does not hold `access.manage` there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<bool>,
    /// By whom things are written: the accounts for an Admin of the place, and the importer otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<ImportAuthors>,
    /// What is done with an object that an earlier import of the same thing made: `replace` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_source: Option<ImportAction>,
    /// What is done with an object whose name another of the place has: `keep_both` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_name: Option<ImportAction>,
}

/// The choice of the person for somebody of the package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportPersonChoice {
    /// The person of the package, by the identifier it names them by.
    pub principal: Id,
    /// The account they are taken for here; `null` takes them for nobody here: their words stay under their
    /// name as "imported person".
    pub user: Option<Id>,
}

/// The choice of the person for one object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportObjectChoice {
    /// The object, by the identifier the package names it by.
    pub source: Id,
    /// What is done with it.
    pub action: ImportAction,
}

/// What the person decided in the review.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportChoices {
    /// Who is who.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<ImportPersonChoice>,
    /// What is done with each object that is not left to the defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<ImportObjectChoice>,
}

/// An import asked for: what it is made from, where it goes and what the person chose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    /// A client-generated identity, one to two hundred characters: a repeated request of the same person
    /// names the same import again and makes nothing twice.
    pub client_id: String,
    /// What it is made from.
    pub source: ImportSource,
    /// Where it goes.
    #[serde(default)]
    pub destination: ImportDestination,
    /// What to bring, and the defaults of conflicts.
    #[serde(default)]
    pub options: ImportOptions,
    /// What the person decided in the review.
    #[serde(default)]
    pub choices: ImportChoices,
}

/// Which part of the objects of a plan a request is for: a plan of ten thousand objects comes a page at a time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportView {
    /// Only the objects that have a counterpart here or a blocker; all of them unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflicts_only: Option<bool>,
    /// Only objects of this kind; all kinds unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PackageKind>,
    /// How many objects, at most [`crate::common::MAX_LIMIT`]; [`crate::common::DEFAULT_LIMIT`] unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// What would an import do: the request, and the part of the objects of the plan the answer should hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportPreflightRequest {
    /// The import as it would be asked for.
    pub request: ImportRequest,
    /// Which objects the answer lists.
    #[serde(default)]
    pub view: ImportView,
}

/// What an import is made of, counted by kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportCounts {
    /// Places: orgs, projects and topics.
    pub places: i64,
    /// Document folders.
    pub folders: i64,
    /// Documents.
    pub documents: i64,
    /// Chats.
    pub chats: i64,
    /// Threads.
    pub threads: i64,
    /// Files that things carry.
    pub files: i64,
    /// Messages of chats.
    pub messages: i64,
    /// Turns of threads.
    pub turns: i64,
    /// Comments of documents.
    pub comments: i64,
    /// Earlier versions of documents.
    pub versions: i64,
}

/// What an import does with its objects, counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportActionCounts {
    /// Objects made new.
    pub create: i64,
    /// Objects made new beside those that were there.
    pub keep_both: i64,
    /// Objects that were there and take the new content.
    pub replace: i64,
    /// Objects left alone.
    pub skip: i64,
}

/// What the place allows of one import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportLimits {
    /// The most bytes of source: `import.max_bytes` as it resolves at the place.
    pub max_bytes: i64,
    /// The most objects: `import.max_items`.
    pub max_items: i64,
}

/// How an object that is there already was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportMatch {
    /// An earlier import of the same object made it, or took it over.
    Source,
    /// It has the same name beside the same things.
    Name,
}

/// An object here that an object of the source meets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportExisting {
    /// Its identifier here.
    pub id: Id,
    /// What it is.
    pub kind: PackageKind,
    /// Its name or title.
    pub name: String,
    /// How it was found.
    pub found_by: ImportMatch,
    /// When it last changed.
    pub updated_at: Option<Time>,
}

/// Why an object cannot be made as it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportBlock {
    /// A stable code: `import.forbidden` (the importer may not make it here), `import.disabled` (documents are
    /// off here), `import.too_deep` (it would pass the depth places may nest to), `import.unsupported` (the
    /// reader does not know it), `import.unreadable` (its record is damaged).
    pub code: String,
    /// What it comes to, in a sentence of English; a client words a code it knows by its own catalogue.
    pub message: String,
    /// What the code is about (`kind`, `parent_kind`, `limit`), for the client's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Json>,
}

/// An object of the source, with what the import would do with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportPlanObject {
    /// Its identifier in the package; for a file of plain files, one made of its path.
    pub source_id: Id,
    /// What it is.
    pub kind: PackageKind,
    /// Its name or title.
    pub name: String,
    /// The object it lies in, by the identifier of the source.
    pub parent: Option<Id>,
    /// The names of the places it lies in, from the top of the source, for a row to show where it is.
    pub path: Vec<String>,
    /// How large its record is.
    pub bytes: i64,
    /// What would be done with it: the default for it, or what the person chose.
    pub action: ImportAction,
    /// What may be chosen for it.
    pub allowed: Vec<ImportAction>,
    /// The object here that it meets.
    pub existing: Option<ImportExisting>,
    /// Why it cannot be made, where it cannot.
    pub blocked: Option<ImportBlock>,
    /// Who made it, as the source names them: a person of `people`.
    pub creator: Option<Id>,
    /// When it was made there.
    pub created_at: Option<Time>,
}

/// How a person of the source is taken here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportPersonMatch {
    /// An account here has the identifier they had there: the package is of this deployment.
    Id,
    /// An account here has their confirmed address.
    Email,
    /// An account here has their username.
    Username,
    /// The person chose the account in the review.
    Choice,
    /// Nobody here is them: they stay "imported person".
    None,
}

/// A person of the source, and who they are taken for here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportPlanPerson {
    /// The identifier the package names them by.
    pub principal: Id,
    /// Their username there.
    pub username: String,
    /// The name they were shown by.
    pub display_name: String,
    /// How the match was made.
    pub matched: ImportPersonMatch,
    /// The account here, where there is one.
    pub user: Option<UserRef>,
    /// How many objects they made or wrote in.
    pub objects: i64,
}

/// Why something that is in the source does not come back, or comes back changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportLoss {
    /// A stable code: `native_sessions`, `workspaces`, `secrets`, `usage_and_audit`, `personal_state`,
    /// `web_images`, `not_exportable`, `too_large`, `sharing_not_restored`, `references_outside`,
    /// `unsupported_block`, `missing_file`, `unknown_person`.
    pub code: String,
    /// How many objects or items it is about; `0` where it is about a kind as a whole.
    pub count: i64,
    /// What it comes to, in a sentence of English; a client words a code it knows by its own catalogue.
    pub message: String,
    /// The object it is about, where it is about one.
    pub object: Option<Id>,
    /// What the code is about, for the client's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Json>,
}

/// Whether the sharing of the package comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportAccessPlan {
    /// Whether the package carries sharing at all: only one made by somebody who held `access.manage` does.
    pub carried: bool,
    /// Whether the importer may restore it: they hold `access.manage` at the place.
    pub allowed: bool,
    /// Whether it would be: asked for, carried and allowed.
    pub restored: bool,
}

/// Something that would stop an import as it is asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportBlocker {
    /// The code of the refusal that asking would get: `import.too_large`, `import.too_many`, `import.nothing`,
    /// `import.invalid` or `import.forbidden`.
    pub code: String,
    /// What it comes to, in a sentence of English; a client words a code it knows by its own catalogue.
    pub message: String,
    /// What the code is about (`limit`, `asked`), for the client's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Json>,
}

/// What the source is, as far as it could be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportSourceFacts {
    /// What it was read as.
    pub mode: ImportSourceMode,
    /// How many files and entries it holds.
    pub entries: i64,
    /// How many bytes they come to once expanded.
    pub bytes: i64,
    /// The header of the package, where it is one.
    pub package: Option<PackageHeader>,
}

/// What an import would do, as far as the caller may bring it here, and what stands in its way. Nothing is made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ImportPlan {
    /// The place it would go into.
    pub node: NodeRef,
    /// What the source is.
    pub source: ImportSourceFacts,
    /// How many of each kind it would bring.
    pub counts: ImportCounts,
    /// What it would do with its objects.
    pub actions: ImportActionCounts,
    /// How many objects meet one that is here.
    pub conflicts: i64,
    /// An estimate of what it would add, in bytes.
    pub bytes: i64,
    /// What the place allows.
    pub limits: ImportLimits,
    /// By whom the things would be written.
    pub authors: ImportAuthors,
    /// Whether the sharing comes back.
    pub access: ImportAccessPlan,
    /// A page of its objects: those the view asked for.
    pub objects: Page<ImportPlanObject>,
    /// The people of the source, and who each is taken for.
    pub people: Vec<ImportPlanPerson>,
    /// What does not come back.
    pub losses: Vec<ImportLoss>,
    /// Not empty: it would be refused as it is asked.
    pub blockers: Vec<ImportBlocker>,
}

/// One item of the source, to be looked at before it is brought.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportPreviewRequest {
    /// What the import is made from.
    pub source: ImportSource,
    /// The object to show, by the identifier the plan names it by.
    pub object: Id,
}

/// An item of the source as its text reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportPreview {
    /// What it is.
    pub kind: PackageKind,
    /// Its name or title.
    pub name: String,
    /// Its text, as Markdown; at most [`IMPORT_PREVIEW_MAX_BYTES`] bytes of it.
    pub text: String,
    /// Whether there was more than the text shows.
    pub truncated: bool,
}

/// Where an import stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportState {
    /// A job is bringing it.
    Pending,
    /// It was brought, all of it that was asked for; the report says what each object came to.
    Done,
    /// The job failed; what it had made stays, and the report says how far it came.
    Failed,
    /// The job was cancelled; what it had made stays, and the report says how far it came.
    Cancelled,
    /// A job is taking back what it made.
    Undoing,
    /// What it made was taken back.
    Undone,
}

/// Why an import failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportFailure {
    /// A stable code.
    pub code: String,
    /// What went wrong, in a sentence.
    pub message: String,
}

/// An import: a person's own record of what they brought into a place, with the job that did it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Import {
    /// Its identifier.
    pub id: Id,
    /// The place it went into; `null` once the place was purged.
    pub node: Option<NodeRef>,
    /// The request as it was made.
    pub request: ImportRequest,
    /// Where it stands.
    pub state: ImportState,
    /// The job that brings it.
    pub job_id: Option<Id>,
    /// The job that takes it back, once one was asked for.
    pub undo_job_id: Option<Id>,
    /// The names of the files it was made from.
    pub files: Vec<String>,
    /// The header of the package it was made from, where it was one.
    pub package: Option<PackageHeader>,
    /// What it made or changed, counted by kind, once it ends.
    pub counts: Option<ImportCounts>,
    /// What it did with its objects, once it ends.
    pub actions: Option<ImportActionCounts>,
    /// What did not come back.
    pub losses: Vec<ImportLoss>,
    /// Why it failed.
    pub failure: Option<ImportFailure>,
    /// When it was asked for.
    pub created_at: Time,
    /// When it ended.
    pub finished_at: Option<Time>,
    /// When what it made was taken back.
    pub undone_at: Option<Time>,
}

/// An import as the list of a person's imports shows it: where it went and where it stands, the first of the files it
/// was made from and how many there were. The whole record, with what was asked and what did not come back, is
/// `GET /imports/{id}`, and the names of all its files are `GET /imports/{id}/files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportLine {
    /// Its identifier.
    pub id: Id,
    /// The place it went into; `null` once the place was purged.
    pub node: Option<NodeRef>,
    /// Where it stands.
    pub state: ImportState,
    /// The job that brings it.
    pub job_id: Option<Id>,
    /// The job that takes it back, once one was asked for.
    pub undo_job_id: Option<Id>,
    /// The first of the files it was made from.
    pub first_file: Option<String>,
    /// How many files it was made from.
    pub file_count: i64,
    /// What it did with its objects, once it ends.
    pub actions: Option<ImportActionCounts>,
    /// When it was asked for.
    pub created_at: Time,
    /// When it ended.
    pub finished_at: Option<Time>,
    /// When what it made was taken back.
    pub undone_at: Option<Time>,
}

/// What `result` is on the job that brings an import, or takes one back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportJobResult {
    /// The import.
    pub import_id: Id,
}

/// What became of one object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportOutcome {
    /// It was made.
    Created,
    /// It was made beside the one that was there.
    KeptBoth,
    /// The one that was there took its content.
    Replaced,
    /// It, or what it would have changed, was the same already.
    Unchanged,
    /// It was left alone, as chosen or because something above it was.
    Skipped,
    /// It could not be brought; the row says why, and what it held was not.
    Failed,
}

/// A thing that came back changed, or not as it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportWarning {
    /// A stable code, among those of [`ImportLoss`].
    pub code: String,
    /// What it comes to, in a sentence of English; a client words a code it knows by its own catalogue.
    pub message: String,
    /// What the code is about (`name`, `count`), for the client's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Json>,
}

/// The object an import made or changed here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportMade {
    /// Its identifier here.
    pub id: Id,
    /// What it is.
    pub kind: PackageKind,
    /// The place it lies in or is, as the importer may see it.
    pub node: Option<NodeRef>,
}

/// A row of the report of an import: one object and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportReportRow {
    /// Its position in the report.
    pub position: i64,
    /// Its identifier in the source.
    pub source_id: Id,
    /// What it is.
    pub kind: PackageKind,
    /// Its name or title.
    pub name: String,
    /// The names of the places it lay in, from the top of the source.
    pub path: Vec<String>,
    /// What was asked to be done with it.
    pub action: ImportAction,
    /// What became of it.
    pub outcome: ImportOutcome,
    /// What it became here.
    pub made: Option<ImportMade>,
    /// Why it failed or was left alone, where there is something to say, in a sentence of English.
    pub message: Option<String>,
    /// The stable code of that reason (`import.forbidden`, `import.exists`, `import.damaged`, `import.too_large`,
    /// `import.invalid`, `import.parent_failed`), for the client's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// What the code is about, for the client's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Json>,
    /// What came back changed.
    pub warnings: Vec<ImportWarning>,
    /// Who wrote it, as the source names them; `null` where it names nobody.
    pub author: Option<String>,
}

/// Which rows of a report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportReportListQuery {
    /// Only rows that came to this; all of them unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ImportOutcome>,
    /// Only objects of this kind; all kinds unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PackageKind>,
    /// Only rows with a warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warned: Option<bool>,
    /// How many rows, at most [`crate::common::MAX_LIMIT`]; [`crate::common::DEFAULT_LIMIT`] unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The form a report is fetched in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportReportFormat {
    /// One JSON object with the import, its losses and every row.
    Json,
    /// A table of the rows, for a spreadsheet.
    Csv,
}

/// How a report is fetched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportReportDownloadQuery {
    /// `json` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ImportReportFormat>,
}

/// What taking an import back would do to the things it made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportUndoPlan {
    /// The import.
    pub import: Id,
    /// What the import made and still stands, counted by kind: the places, folders, documents, chats and threads. It is what taking the
    /// import back looks at, and takes unless people have added to it or changed it since (`added_since`, `edited_since`).
    pub counts: ImportCounts,
    /// What the import added to things that were there, and would be taken out again: messages and turns.
    pub merged: ImportCounts,
    /// How many things lie in what the import made that it did not make: what people added to its places and folders afterwards. They
    /// stay where they are, and so do the places and folders that hold them.
    pub added_since: i64,
    /// How many documents it made have been edited since by somebody else. They stay.
    pub edited_since: i64,
    /// Until when what is taken back can be brought back, by the retention of deletions.
    pub restore_until: Time,
    /// Not empty: it would be refused as it is asked.
    pub blockers: Vec<ImportBlocker>,
}

/// An import taken back as a whole.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportUndoBody {
    /// A client-generated identity, one to two hundred characters: a repeated request names the same job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

crate::endpoints! {
    /// Says what an import into a place would do: the action for each object, the people, the conflicts, the losses and what stands in its way. Nothing is made.
    NodeImportPreflight: POST "/nodes/{id}/imports/preflight", Permission("node.view"), none, json ImportPreflightRequest, json ImportPlan, 200;
    /// Shows an item of the source as text, before it is brought.
    NodeImportPreview: POST "/nodes/{id}/imports/preview", Permission("node.view"), none, json ImportPreviewRequest, json ImportPreview, 200;
    /// Brings an Orch package or plain files into a place, written by a job; each thing is made by the caller's creation rights at the moment it is made.
    NodeImportCreate: POST "/nodes/{id}/imports", Permission("node.view"), none, json ImportRequest, json Job, 202;
    /// Lists the caller's own imports, newest first.
    MeImportList: GET "/me/imports", User, query PageQuery, none, json Page<ImportLine>, 200;
    /// Reads one of the caller's imports.
    ImportGet: GET "/imports/{id}", Rule("its person"), none, none, json Import, 200;
    /// Lists the names of the files one of the caller's imports was made from, in the order they were given.
    ImportFileList: GET "/imports/{id}/files", Rule("its person"), query PageQuery, none, json Page<String>, 200;
    /// Lists the report of an import: a row for each object and what became of it.
    ImportReportList: GET "/imports/{id}/report", Rule("its person"), query ImportReportListQuery, none, json Page<ImportReportRow>, 200;
    /// Fetches the whole report of an import as a file.
    ImportReportDownload: GET "/imports/{id}/report/download", Rule("its person, in a browser session or with a token of theirs"), query ImportReportDownloadQuery, none, raw, 200;
    /// Says what taking an import back would remove. Nothing is changed.
    ImportUndoPreflight: POST "/imports/{id}/undo/preflight", Rule("its person"), none, none, json ImportUndoPlan, 200;
    /// Takes an import back as a whole: what it made is deleted, and can be brought back for as long as deletions can.
    ImportUndo: POST "/imports/{id}/undo", Rule("its person"), none, json ImportUndoBody, json Job, 202;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_names_its_files_and_leaves_the_choices_to_the_defaults() {
        let request: ImportRequest = serde_json::from_value(json!({
            "client_id": "a",
            "source": { "files": [{ "name": "notes.zip", "blob": "00".repeat(32) }] }
        }))
        .unwrap();
        assert_eq!(request.source.files.len(), 1);
        assert_eq!(
            request.source.mode, None,
            "a package or files, as the archive says"
        );
        assert_eq!(request.destination, ImportDestination::default());
        assert_eq!(request.options, ImportOptions::default());
        assert_eq!(request.choices, ImportChoices::default());
        assert_eq!(
            serde_json::to_value(&request.options).unwrap(),
            json!({}),
            "what was left out is not written"
        );
    }

    #[test]
    fn a_request_with_a_member_nobody_defined_or_a_choice_that_is_no_action_is_refused() {
        assert!(
            serde_json::from_value::<ImportRequest>(
                json!({ "client_id": "a", "source": { "files": [] }, "zip": true })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<ImportObjectChoice>(
                json!({ "source": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01", "action": "merge" })
            )
            .is_err()
        );
        let choice: ImportObjectChoice = serde_json::from_value(
            json!({ "source": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01", "action": "keep_both" }),
        )
        .unwrap();
        assert_eq!(choice.action, ImportAction::KeepBoth);
    }

    #[test]
    fn somebody_the_person_takes_for_nobody_is_written_as_null_and_is_not_left_out() {
        let choice: ImportPersonChoice = serde_json::from_value(json!({
            "principal": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e01", "user": null
        }))
        .unwrap();
        assert_eq!(choice.user, None);
        assert_eq!(
            serde_json::to_value(&choice).unwrap()["user"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn an_import_that_is_not_over_has_no_counts_and_no_end() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b01",
            "node": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "project", "name": "Web", "redacted": false },
            "request": { "client_id": "a", "source": { "files": [] }, "destination": {}, "options": {}, "choices": {} },
            "state": "pending",
            "job_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5c01",
            "undo_job_id": null,
            "files": ["notes.zip"],
            "package": null,
            "counts": null, "actions": null,
            "losses": [], "failure": null,
            "created_at": "2026-10-29T09:00:00.000Z",
            "finished_at": null, "undone_at": null
        });
        let import: Import = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(import.state, ImportState::Pending);
        assert!(import.counts.is_none() && import.finished_at.is_none());
        assert_eq!(serde_json::to_value(&import).unwrap(), written);
    }
}
