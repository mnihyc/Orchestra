//! People and places as an extension is told of them: the references that notifications and summaries carry.
//!
//! Several extension points hand an implementation a reference to a person or to a place in the tree of
//! organizations: a notification names whom it is for and where it happened, and a summary is asked for about
//! one node. The shapes are the ones every other part of Orch shows, written once here so that no extension
//! contract restates them.

use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How many colours an avatar can have: the interface keeps twelve, numbered from 0.
pub const AVATAR_COLORS: u8 = 12;

/// What shows for a person where there is no picture.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Avatar {
    /// The letters shown.
    pub text: String,
    /// The colour, a number below [`AVATAR_COLORS`].
    pub color: u8,
}

/// Whether an account is somebody's own or a service's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UserKind {
    /// A person.
    Human,
    /// An account that programs use.
    Service,
}

/// A person or a service, as it is shown beside what it did.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct UserRef {
    /// Its identifier.
    pub id: Id,
    /// What kind of account it is.
    pub kind: UserKind,
    /// The name it signs in under.
    pub username: String,
    /// The name shown.
    pub display_name: String,
    /// What shows beside the name.
    pub avatar: Avatar,
}

/// The kinds of place in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// An organization, which may lie in another.
    Org,
    /// A project.
    Project,
    /// A topic.
    Topic,
    /// A thread.
    Thread,
}

/// A place in the tree, as it is named where it is mentioned.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct NodeRef {
    /// Its identifier.
    pub id: Id,
    /// What kind of place it is.
    pub kind: NodeKind,
    /// Its name; `null` where the reader may not see it.
    pub name: Option<String>,
    /// Whether the name was left out because the reader may not see it.
    pub redacted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_person_and_a_place_read_as_every_other_part_writes_them() {
        let user: UserRef = serde_json::from_value(json!({
            "id": "0199abcd-1234-7000-8000-000000000001", "kind": "human", "username": "dana",
            "display_name": "Dana", "avatar": { "text": "D", "color": 4 }
        }))
        .unwrap();
        assert_eq!(user.kind, UserKind::Human);
        assert!(user.avatar.color < AVATAR_COLORS);
        let node: NodeRef = serde_json::from_value(json!({
            "id": "0199abcd-1234-7000-8000-000000000002", "kind": "topic", "name": null, "redacted": true
        }))
        .unwrap();
        assert_eq!(node.kind, NodeKind::Topic);
        assert_eq!(serde_json::to_value(&node).unwrap()["name"], json!(null));
    }
}
