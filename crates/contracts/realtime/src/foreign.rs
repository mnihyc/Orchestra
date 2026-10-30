//! Row types that belong to the HTTP contract, carried as JSON until it defines them.
//!
//! The socket tells a browser which rows changed, and the rows are the
//! same ones the HTTP API returns. Where the API's contract has a type,
//! this package uses it: a chat, a chat message, a chat draft and the three
//! rows of a document already are those of the HTTP contract, which their
//! feeds carry as they are. Inbox notifications use the same owner too. A
//! display still has a JSON stand-in until its HTTP row is connected here.

/// One notification of a person's inbox.
pub use orch_api_contract::notifications::Notification;

/// A browser, as the HTTP contract returns it.
pub use orch_api_contract::browsers::Browser;

/// A browser profile, as the HTTP contract returns it.
pub use orch_api_contract::browsers::BrowserProfile;

/// A display, carried whole until the HTTP contract defines its row.
pub use orch_api_contract::displays::Display;

/// A chat.
pub use orch_api_contract::chats::Chat;

/// One message of a chat.
pub use orch_api_contract::chats::ChatMessage;

/// What a person has typed in a chat and not sent.
pub use orch_api_contract::chats::ChatDraft;

/// A document, without its blocks.
pub use orch_api_contract::documents::DocumentRef;

/// One block of a document, as it stands.
pub use orch_api_contract::documents::DocumentBlock;

/// A discussion beside a document, with its comments.
pub use orch_api_contract::documents::DocumentDiscussion;
