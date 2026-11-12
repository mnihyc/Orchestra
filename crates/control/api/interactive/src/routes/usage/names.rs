//! The names that figures and records are shown by: people, providers, models by what they are shown as, agents by kind, and places, each place a redacted reference where the reader may not see it.
//!
//! A figure is about a member of a dimension, and a record is about places,
//! people and resources; the ledger keeps their identifiers as they were. They
//! are named here, for a whole answer at once. A place the reader may not see is
//! a reference without a name (`redacted`), and its figures stay in the totals:
//! nothing says where below it the use was.

use orch_api_contract::users::UserRef;

use orch_types::Id;

pub use orch_api_base::routes::usage::names::Names;

pub use orch_api_base::routes::usage::names::ids_of;

pub use orch_api_base::routes::usage::names::Cut;
/// A word of one contract as the same word of another.
fn same<A: serde::Serialize, B: serde::de::DeserializeOwned>(word: &A) -> Option<B> {
    serde_json::to_value(word)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

/// A person who cannot be read from the JSON above, which cannot happen: the shape is fixed.
fn unreachable_user(id: Id) -> UserRef {
    UserRef {
        id,
        kind: orch_api_contract::users::UserKind::Human,
        username: String::new(),
        address: None,
        display_name: String::new(),
        avatar: orch_api_contract::users::Avatar {
            text: String::new(),
            color: 0,
        },
    }
}
orch_api_base::__api_bind_fd9e5a14be!();

/// A rate of one contract as a rate of the API's.
#[must_use]
pub fn rate_of(value: &orch_types::Json) -> Option<orch_api_contract::resources::Rate> {
    same(value)
}
