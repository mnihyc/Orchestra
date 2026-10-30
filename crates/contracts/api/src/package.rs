//! The Orch package: what an export of a place writes at the root of its archive as `manifest.json` so that the archive can be imported again, with every object it holds, the people they name, the sharing it carries where its maker may read that, and what was left out.
//!
//! A package is the archive of [`crate::exports`] asked for with `package: true`. Beside the readable tree of
//! Markdown files it holds one record of each thing as JSON (a document as the sync of its blocks, a chat as its
//! messages, a thread as its turns and items), the files they carry, and this manifest, which lists them all: each
//! object with its kind, its identifier, the object it lies in, where its record is, its name, its times, who made
//! it and the SHA-256 of its record. An importer reads the manifest first and trusts nothing before the hash of
//! each record has been checked against it. The identifiers are those of the deployment the package was made in;
//! an import gives everything fresh ones and keeps the correspondence (`import_origins`).

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::exports::{ExportCounts, ExportLeftOut, ExportRequest};
use crate::nodes::NodeKind;

/// The word a manifest names its format by.
pub const PACKAGE_FORMAT: &str = "orch-package";

/// The version of the manifest this build writes, and the newest it reads. A manifest of a later version is
/// refused with the version it names, because what it adds cannot be known.
pub const PACKAGE_VERSION: u32 = 1;

/// The path of the manifest in the archive.
pub const PACKAGE_MANIFEST: &str = "manifest.json";

/// The kinds of thing a package holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    /// An org inside the place the package is of, which a department is.
    Org,
    /// A project or a folder.
    Project,
    /// A topic.
    Topic,
    /// A thread, with its turns and items as history.
    Thread,
    /// The chat of a place or of a thread, with its replies, reactions and files.
    Chat,
    /// A document folder.
    Folder,
    /// A document, with its files, its discussions and, where asked for, its versions.
    Document,
}

impl PackageKind {
    /// The kind of a node.
    #[must_use]
    pub const fn of_node(kind: NodeKind) -> Self {
        match kind {
            NodeKind::Org => Self::Org,
            NodeKind::Project => Self::Project,
            NodeKind::Topic => Self::Topic,
            NodeKind::Thread => Self::Thread,
        }
    }

    /// Whether it is a place: something other things lie in.
    #[must_use]
    pub const fn is_place(self) -> bool {
        matches!(self, Self::Org | Self::Project | Self::Topic | Self::Folder)
    }
}

/// The deployment a package was made in, as its maker's server names itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageSource {
    /// The name the server gives itself.
    pub name: String,
    /// The address it is reached at.
    pub url: String,
    /// The version of the product that wrote the package.
    pub version: String,
}

/// The place a package is of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageScope {
    /// Its identifier there.
    pub id: Id,
    /// What kind of place it is.
    pub kind: NodeKind,
    /// Its name.
    pub name: String,
}

/// The header of a package: where it comes from, who made it, of what, with which options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageHeader {
    /// The identifier of this package: the one of the export that made it.
    pub package_id: Id,
    /// The deployment it was made in.
    pub source: PackageSource,
    /// Who made it: a person of `principals`.
    pub made_by: Option<Id>,
    /// When it was made.
    pub made_at: Time,
    /// The place it is of.
    pub scope: PackageScope,
    /// What was asked for, as it was asked.
    pub options: ExportRequest,
    /// How many of each kind it holds.
    pub counts: ExportCounts,
}

/// Another form of the same thing, written for people to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageRendering {
    /// `markdown` or `html`.
    pub format: String,
    /// Where it lies in the archive.
    pub path: String,
    /// The SHA-256 of its bytes, in lower case hexadecimal.
    pub sha256: String,
}

/// A file a thing carries, which lies beside its record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageFile {
    /// The identifier it had there; a block of a document names its file by it.
    pub id: Option<Id>,
    /// Its name in the thing that carries it.
    pub name: String,
    /// Where it lies in the archive.
    pub path: String,
    /// Its size in bytes.
    pub bytes: i64,
    /// The SHA-256 of its content, in lower case hexadecimal.
    pub sha256: String,
}

/// An earlier version of a document, which lies beside it as a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageVersion {
    /// Its number, counted from one.
    pub number: i64,
    /// When it was cut.
    pub cut_at: Time,
    /// Where its record lies in the archive.
    pub path: String,
    /// The SHA-256 of the record.
    pub sha256: String,
}

/// One thing in a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageObject {
    /// What it is.
    pub kind: PackageKind,
    /// Its identifier there.
    pub id: Id,
    /// The object it lies in; `null` for the place the package is of. A document lies in a folder or in the
    /// place it is written beside; a chat in the place or the thread it belongs to.
    pub parent: Option<Id>,
    /// Where its record lies in the archive; `null` for a place or a folder, which the manifest describes whole.
    pub path: Option<String>,
    /// Its name or title.
    pub name: String,
    /// What it is about, as its place or document says; empty where nothing is said.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Whether it is a place that its maker could not name: it is kept as a place with no name, so that what lies
    /// in it keeps its position.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub hidden: bool,
    /// When it was made.
    pub created_at: Option<Time>,
    /// When it last changed.
    pub updated_at: Option<Time>,
    /// Who made it: a person of `principals`.
    pub creator: Option<Id>,
    /// The SHA-256 of its record, in lower case hexadecimal; `null` where it has none.
    pub sha256: Option<String>,
    /// The size of its record in bytes.
    pub bytes: i64,
    /// How many messages a chat holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<i64>,
    /// How many turns a thread holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<i64>,
    /// How many comments a document holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<i64>,
    /// The same thing written for people to read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renderings: Vec<PackageRendering>,
    /// The files it carries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<PackageFile>,
    /// The earlier versions of a document.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<PackageVersion>,
}

/// A person or an identity the package names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackagePrincipal {
    /// Its identifier there.
    pub id: Id,
    /// Whether it is a person or a service identity.
    pub kind: crate::users::UserKind,
    /// The username.
    pub username: String,
    /// The name it is shown by.
    pub display_name: String,
    /// The address, where the one who made the package may read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Whether the address was confirmed by its owner there; an address that was not is never matched.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub email_verified: bool,
}

/// What a row of the sharing of a thing gives, as the package carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageAccess {
    /// The object whose page it is on.
    pub object: Id,
    /// The person it names, of `principals`.
    pub principal: Id,
    /// The permission it gives, by its name (`doc.view`, `thread.interact`).
    pub permission: String,
    /// When it stops holding.
    pub expires_at: Option<Time>,
    /// The note its maker left.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// Why something that was asked for is not in a package, beyond what an archive leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackageLeftOutWhat {
    /// What its maker may read and holds no Export row for.
    NotExportable,
    /// A file larger than the largest the system takes into an export.
    TooLarge,
    /// What no export carries: a native session of an agent program.
    NativeSessions,
    /// What no export carries: the workspace files of a thread.
    Workspaces,
    /// What no export carries: secrets.
    Secrets,
    /// What no export carries: usage, and the audit trail.
    UsageAndAudit,
    /// What no export carries: what each person keeps for themselves, and approvals.
    PersonalState,
    /// Images on the web, which stay the links they are.
    WebImages,
}

/// Things a package does not hold, by why and by the place they lay in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageLeftOut {
    /// Why.
    pub what: PackageLeftOutWhat,
    /// How many; `0` where the kind is left out as a whole.
    pub count: i64,
    /// The place they lay in, where that is told.
    pub place: Option<Id>,
}

impl From<&ExportLeftOut> for PackageLeftOut {
    fn from(left: &ExportLeftOut) -> Self {
        Self {
            what: match left.what {
                crate::exports::ExportLeftOutWhat::NotExportable => {
                    PackageLeftOutWhat::NotExportable
                }
                crate::exports::ExportLeftOutWhat::TooLarge => PackageLeftOutWhat::TooLarge,
            },
            count: left.count,
            place: left
                .place
                .as_ref()
                .filter(|place| !place.redacted)
                .map(|place| place.id),
        }
    }
}

/// The manifest of a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageManifest {
    /// Always [`PACKAGE_FORMAT`].
    pub format: String,
    /// The version of the manifest, [`PACKAGE_VERSION`] when it was written.
    pub version: u32,
    /// Where it comes from and what it is of.
    pub header: PackageHeader,
    /// Every thing in it, a place before what lies in it.
    pub objects: Vec<PackageObject>,
    /// Everybody the objects name.
    pub principals: Vec<PackagePrincipal>,
    /// The sharing of the objects, present only where the one who made the package held `access.manage` on
    /// the place: without it a package carries no sharing, and says so by leaving this out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Vec<PackageAccess>>,
    /// What was asked for and is not in it, each with why.
    pub left_out: Vec<PackageLeftOut>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_manifest_without_sharing_says_so_by_leaving_the_member_out() {
        let written = json!({
            "format": "orch-package",
            "version": 1,
            "header": {
                "package_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b01",
                "source": { "name": "Orch", "url": "https://orch.example.com", "version": "1.0.0" },
                "made_by": null,
                "made_at": "2026-10-29T09:00:00.000Z",
                "scope": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "project", "name": "Web" },
                "options": { "what": "all", "package": true },
                "counts": { "places": 1, "conversations": 0, "chats": 0, "documents": 1, "files": 0 }
            },
            "objects": [{
                "kind": "document",
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01",
                "parent": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01",
                "path": "Web/docs/Plan.json",
                "name": "Plan",
                "created_at": "2026-10-01T08:00:00.000Z",
                "updated_at": null,
                "creator": null,
                "sha256": "00",
                "bytes": 2
            }],
            "principals": [],
            "left_out": []
        });
        let manifest: PackageManifest = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(manifest.version, PACKAGE_VERSION);
        assert!(manifest.access.is_none());
        assert_eq!(manifest.objects[0].kind, PackageKind::Document);
        assert!(manifest.objects[0].renderings.is_empty());
        assert_eq!(serde_json::to_value(&manifest).unwrap(), written);
    }

    #[test]
    fn a_node_is_the_kind_of_package_object_with_its_name() {
        assert_eq!(PackageKind::of_node(NodeKind::Thread), PackageKind::Thread);
        assert!(PackageKind::Folder.is_place());
        assert!(!PackageKind::Chat.is_place());
    }
}
