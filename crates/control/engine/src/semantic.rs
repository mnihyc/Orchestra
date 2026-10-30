//! Search by meaning: what an org chose, its text made into vectors through the API provider it named, and those vectors kept and searched.
//!
//! Search by meaning is off until an org turns it on and names an embeddings provider and model
//! (`search.semantic.*`, at its root org). Vectors are made in the provider's runtime with `provider.embed`, as
//! summaries are, and counted with the purpose `embeddings`. The indexer cuts an org's documents,
//! conversations and chats into pieces and makes their vectors in the background, newest first, within the
//! org's disk share; a query is made into a vector with what made the vectors it is compared with, and the
//! pieces nearest it are joined with the results by words, each checked by its caller against what the reader
//! may read now.
//!
//! | Module | What it does |
//! |--------|--------------|
//! | [`choice`] | The org's choice: on or off, the provider and model and who named them, the disk share, the tuning |
//! | [`embed`] | Texts made into vectors through the named provider, checked and counted |
//! | [`vectors`] | Length, likeness and shortening of vectors |
//! | [`chunks`] | A source cut into pieces of about one size, sections kept together |
//! | [`plan`] | What a change of model means for the vectors there are, and whether a new set fits the disk share |
//! | [`indexer`] | The background turns that cut, make vectors and switch generations |
//! | [`state`] | Where an org's index stands, as an admin reads it, and the choice to rebuild it in place |
//! | [`query`] | The pieces nearest a query in an org |
//! | [`fusion`] | Reciprocal-rank fusion of the order by words and the order by meaning |

pub mod choice;
pub mod chunks;
pub mod embed;
pub mod fusion;
pub mod indexer;
pub mod plan;
pub mod query;
pub mod state;
pub mod vectors;

pub use choice::{Choice, Embeddings, Tuning, choice};
pub use embed::EmbedError;
