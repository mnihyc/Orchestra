//! What an import works with: the source as it was opened, each of its objects with what is to be done with it, and the plan they come to.
//!
//! The same plan is made when an import is asked what it would do and again when its job runs (from the same request, against
//! what is there then), so that what the person reviewed is what the job does as far as nothing changed meanwhile, and a job
//! that was taken over by another process makes the same plan again and goes on.

use super::people::{Credits, Matched};
use super::source::Entries;
use orch_api_contract::imports::{
    ImportAccessPlan, ImportAction, ImportAuthors, ImportBlock, ImportLimits, ImportLoss,
    ImportMatch, ImportRoot, ImportSourceMode,
};
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::package::{
    PackageAccess, PackageHeader, PackageKind, PackageLeftOut, PackageObject, PackagePrincipal,
};
use orch_db::queries::imports::OriginRow;
use orch_types::Id;
use std::collections::HashMap;
use std::sync::Arc;

/// What a plain file is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlainFormat {
    /// Markdown, in any of its extensions.
    Markdown,
    /// HTML.
    Html,
    /// Plain text.
    Text,
    /// A Word document.
    Word,
}

/// What a document of plain files is made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plain {
    /// The entry that holds it.
    pub entry: String,
    /// What it is written in.
    pub format: PlainFormat,
}

/// A source, opened.
pub struct Opened {
    /// What it was read as.
    pub mode: ImportSourceMode,
    /// Its entries.
    pub entries: Arc<dyn Entries>,
    /// The header of the package.
    pub header: Option<PackageHeader>,
    /// Its objects, as the manifest lists them, or as the files make them.
    pub objects: Vec<PackageObject>,
    /// The people it names.
    pub principals: Vec<PackagePrincipal>,
    /// The sharing it carries.
    pub access: Option<Vec<PackageAccess>>,
    /// What it says was left out.
    pub left_out: Vec<PackageLeftOut>,
    /// The identifier of the package: its own, or one made of the files.
    pub package: Id,
    /// How many entries it holds.
    pub entry_count: i64,
    /// How many bytes they come to once expanded.
    pub bytes: i64,
    /// What each document of plain files is made from, by the identifier of its object.
    pub plain: HashMap<Id, Plain>,
    /// The paths of the entries that are files and not directories, for a text of plain files to find what it names among.
    pub entry_names: std::collections::HashSet<String>,
    /// What was found in plain files that does not come back as it was, to tell.
    pub notes: Vec<ImportLoss>,
}

/// An object that is here and meets an object of the source.
#[derive(Debug, Clone, PartialEq)]
pub struct Existing {
    /// Its identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// How it was found.
    pub found_by: ImportMatch,
    /// When it last changed.
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// What an earlier import wrote of it.
    pub origin: Option<OriginRow>,
}

/// An object of the source and what is to be done with it.
#[derive(Debug, Clone)]
pub struct Object {
    /// The object as the source lists it.
    pub object: PackageObject,
    /// What is done with it.
    pub action: ImportAction,
    /// What may be chosen for it.
    pub allowed: Vec<ImportAction>,
    /// The object here that it meets.
    pub existing: Option<Existing>,
    /// Why it cannot be made.
    pub blocked: Option<ImportBlock>,
    /// The node it lies in: for a place or a thread the node it goes into, for a chat, a document folder or a document the
    /// node it belongs to or lies beside.
    pub lies_in: Id,
    /// The document folder a folder or a document lies in, where it lies in one.
    pub container: Option<Id>,
    /// Whether what it lies in is made by this import, which makes the person its owner and settles who may add to it.
    pub parent_made: bool,
    /// Whether what it lies in is skipped, and it with it.
    pub skipped_above: bool,
    /// Its identifier here: the one it has, or the one it will get.
    pub target: Id,
    /// The kind it is made as here: an org comes back as a project.
    pub made_as: PackageKind,
    /// Whether the place a thread has in the source lacks its record, so that it is only a place for what lies in it.
    pub bare: bool,
    /// The names of the places it lies in, from the top of the source.
    pub path: Vec<String>,
}

impl Object {
    /// Whether the import does anything with it.
    #[must_use]
    pub fn goes(&self) -> bool {
        self.blocked.is_none() && self.action != ImportAction::Skip && !self.skipped_above
    }
}

/// The place an import goes into.
#[derive(Debug, Clone)]
pub struct Destination {
    /// Its identifier.
    pub id: Id,
    /// What kind of node it is.
    pub kind: NodeKind,
    /// Its name.
    pub name: String,
    /// Its root org.
    pub org: Id,
    /// Whether it is archived.
    pub archived: bool,
    /// The document folder documents go into.
    pub folder: Option<Id>,
    /// Where the root of a package goes.
    pub root: ImportRoot,
}

/// What an import is asked to do, as the job and the plan read it.
#[derive(Debug, Clone)]
pub struct Wants {
    /// Whether the conversations of threads are brought.
    pub conversations: bool,
    /// Whether chats are brought.
    pub chats: bool,
    /// Whether documents and their folders are brought.
    pub documents: bool,
    /// Whether the earlier versions of documents are brought.
    pub versions: bool,
    /// Whether the discussions of documents are brought.
    pub comments: bool,
    /// Whether the sharing of the package is asked to be restored.
    pub access: bool,
    /// By whom things are written, as asked.
    pub authors: Option<ImportAuthors>,
    /// What is done with what an earlier import made.
    pub same_source: ImportAction,
    /// What is done with what has the same name.
    pub same_name: ImportAction,
    /// What was chosen for each object.
    pub objects: HashMap<Id, ImportAction>,
}

/// The plan of an import.
pub struct Plan {
    /// The place it goes into.
    pub destination: Destination,
    /// The source.
    pub opened: Arc<Opened>,
    /// The objects, a place before what lies in it, structure before content.
    pub objects: Vec<Object>,
    /// Where each object of the source is here, by the identifier it has in the source.
    pub index: HashMap<Id, usize>,
    /// The people of the source and who each is taken for.
    pub people: Vec<Matched>,
    /// How things are written.
    pub credits: Credits,
    /// By whom, as it comes to.
    pub authors: ImportAuthors,
    /// Whether the sharing comes back.
    pub access: ImportAccessPlan,
    /// What does not come back.
    pub losses: Vec<ImportLoss>,
    /// What the place allows.
    pub limits: ImportLimits,
    /// Whether the person is an Admin of the place.
    pub admin: bool,
    /// How many objects the import makes or changes, by the kinds that count against the limit.
    pub items: i64,
    /// The bytes it would add.
    pub bytes: i64,
    /// What was asked.
    pub wants: Wants,
}

impl Plan {
    /// The object of the source `source`.
    #[must_use]
    pub fn get(&self, source: Id) -> Option<&Object> {
        self.index.get(&source).map(|at| &self.objects[*at])
    }

    /// Where an object of the source is here, if it goes.
    #[must_use]
    pub fn target_of(&self, source: Id) -> Option<Id> {
        self.get(source)
            .filter(|object| object.goes() || object.existing.is_some())
            .map(|object| object.target)
    }
}

/// The kind an object is made as: an org of a package comes back as a project.
#[must_use]
pub const fn made_as(kind: PackageKind) -> PackageKind {
    match kind {
        PackageKind::Org => PackageKind::Project,
        other => other,
    }
}

/// The kind of node a place of a package is, as a word of the schema.
#[must_use]
pub const fn node_word(kind: PackageKind) -> Option<&'static str> {
    match kind {
        PackageKind::Org | PackageKind::Project => Some("project"),
        PackageKind::Topic => Some("topic"),
        PackageKind::Thread => Some("thread"),
        _ => None,
    }
}

/// Whether a thing of `child` may lie in one of `parent`, both as they are made here.
#[must_use]
pub const fn may_hold(parent: PackageKind, child: PackageKind) -> bool {
    use PackageKind::{Chat, Document, Folder, Org, Project, Thread, Topic};
    match (parent, child) {
        (Org | Project, Project | Topic | Folder | Document | Chat) => true,
        (Topic, Thread | Folder | Document | Chat) => true,
        (Thread, Folder | Document | Chat) => true,
        (Folder, Folder | Document) => true,
        _ => false,
    }
}

/// How many levels deep an object lies, counted by its parents in the source.
#[must_use]
pub fn depth(objects: &HashMap<Id, &PackageObject>, mut id: Id) -> usize {
    let mut depth = 0;
    while let Some(parent) = objects.get(&id).and_then(|object| object.parent) {
        depth += 1;
        id = parent;
        if depth > objects.len() {
            break;
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_holds_what_the_product_lets_it_hold_and_nothing_else() {
        assert!(may_hold(PackageKind::Project, PackageKind::Topic));
        assert!(may_hold(PackageKind::Project, PackageKind::Project));
        assert!(!may_hold(PackageKind::Org, PackageKind::Thread));
        assert!(may_hold(PackageKind::Topic, PackageKind::Thread));
        assert!(!may_hold(PackageKind::Topic, PackageKind::Topic));
        assert!(may_hold(PackageKind::Folder, PackageKind::Document));
        assert!(!may_hold(PackageKind::Folder, PackageKind::Chat));
        assert!(!may_hold(PackageKind::Document, PackageKind::Document));
    }

    #[test]
    fn an_org_of_a_package_comes_back_as_a_project() {
        assert_eq!(made_as(PackageKind::Org), PackageKind::Project);
        assert_eq!(made_as(PackageKind::Topic), PackageKind::Topic);
        assert_eq!(node_word(PackageKind::Org), Some("project"));
        assert_eq!(node_word(PackageKind::Document), None);
    }
}
