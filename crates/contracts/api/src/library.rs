//! Authorized library facts for resource pages and immutable skill versions.

use orch_access_contract::decisions::ObjectRef;
use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One resource, optionally narrowed to an immutable skill version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryTarget {
    /// The ordinary resource permission page.
    pub object: ObjectRef,
    /// An exact skill version, or null for its resource total.
    pub version_id: Option<Id>,
}

/// A page reads its visible resource facts together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryReadBody {
    /// Resources and versions being displayed.
    pub targets: Vec<LibraryTarget>,
}

/// The authorized current relationships and reported activity of one target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LibraryEntry {
    /// The target whose parent permission was checked.
    pub target: LibraryTarget,
    /// Meaningful giving relationships and real reported uses.
    pub facts: crate::resource_facts::ResourceFacts,
    /// Distinct threads that ran work, for a provider; absent for other kinds.
    pub threads: Option<i64>,
}

/// A count opens the same bounded, paged people list as other people counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryPeopleBody {
    /// The resource and optional skill version whose use is being inspected.
    pub target: LibraryTarget,
    /// The last visible person on the preceding page.
    pub cursor: Option<Id>,
    /// Only people recorded in the same thirty UTC days as the recent figure.
    pub recent: bool,
}

/// Captured users whose names the caller may see, with the rest counted without identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LibraryPeopleReply {
    /// One bounded page of visible users.
    pub items: Vec<crate::users::UserRef>,
    /// People whose profile audience does not include this reader.
    pub hidden: i64,
    /// The next visible-person cursor, or null when complete.
    pub next_cursor: Option<Id>,
}

crate::endpoints! {
    /// Reads giving and reported use for authorized resources and skill versions in one batch.
    ResourceLibraryRead: POST "/resources/library", crate::routes::Access::User, none, json LibraryReadBody, json Vec<LibraryEntry>, 200;
    /// Lists captured people behind a resource's use count without naming hidden users.
    ResourceLibraryPeopleRead: POST "/resources/library/people", crate::routes::Access::User, none, json LibraryPeopleBody, json LibraryPeopleReply, 200;
}
