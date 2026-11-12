//! An additional typed operation boundary over native inventory decisions.
//!
//! This interface supplies no permissions. Its caller reuses the shared operation
//! matcher; the inventory still obtains every ordinary decision from its own Frame.

use super::*;

/// Restricts actual native subjects or conservatively narrows an inherited candidate profile.
/// Profile decisions are hints, never real targets that an operation may admit or return.
pub trait Boundary: Sync {
    /// The stored business family is independent of the native governing subject kind.
    fn object(&self, graph: &Graph, site: &Site, business: ObjectKind) -> Result<bool, Error>;

    /// A real Chat ID and the current node/org relation from its stored row.
    fn chat(&self, graph: &Graph, chat: &chats::ChatRow) -> Result<bool, Error>;

    /// Unknown ordinary children under this actual Topic, for main or Chat discovery.
    /// A positive hint still needs actual selected row identities before disclosure.
    fn thread_profile(&self, graph: &Graph, topic: &Site, chat: bool) -> Result<bool, Error>;
}

#[derive(Clone, Copy)]
pub(super) struct Mask {
    pub main: bool,
    pub chat: bool,
}

impl Mask {
    pub fn apply(self, mut native: Grant) -> Grant {
        let chat = native.chat.take();
        if !self.main {
            native = Grant {
                access: "path_only".to_owned(),
                ..Grant::default()
            };
        }
        native.chat = chat.map(|chat| {
            if self.chat {
                chat
            } else {
                Box::new(Grant {
                    access: "path_only".to_owned(),
                    ..Grant::default()
                })
            }
        });
        native
    }

    pub fn encode(self, native: Grant) -> Result<Json, Error> {
        let mut value =
            serde_json::to_value(native).map_err(|error| Error::Stored(error.to_string()))?;
        // Native SQL uses these already-decided booleans when it restores participation
        // or uses a node name for its Chat. It never evaluates key rules or grants here.
        value["scope_main"] = Json::Bool(self.main);
        value["scope_chat"] = Json::Bool(self.chat);
        if let Some(chat) = value.get_mut("chat").filter(|value| value.is_object()) {
            chat["scope_name_fallback"] = Json::Bool(self.main);
        }
        Ok(value)
    }
}
