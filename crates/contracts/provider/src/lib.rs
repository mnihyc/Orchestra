//! Provider source contracts: facts, models and credential presentation.
//!
//! What a provider is now (its plan, windows, limits, what is left to spend and what was spent, the resets it
//! offers, who its credential belongs to), which models it offers, and how its key is presented are each read
//! through an interface of their own: [`FactsSource`] (with the resets its facts list), [`ModelsSource`] and
//! [`CredentialPresenter`]. Built-in readers, a program registered beside the credential (plugin kind
//! `provider`) and an address reached over HTTP or HTTPS implement the same interfaces with the same JSON, so
//! that nothing above them asks which vendor or which kind of provider it is.
//!
//! The types of a provider's facts ([`facts`]) and of what a model can be run with ([`models`]) are written
//! here once; the agent contract and the API's contract re-export them, so this crate depends on neither.
//!
//! Every member of a provider's facts is optional and in one of four states (unsupported, not read yet,
//! empty, told: [`members`]). What a provider's managers chose for each element is kept as a
//! [`SourceChoice`] and told to the machine that keeps the credential as a [`SourceSpec`] ([`choice`]), whose
//! default follows one rule ([`choice::default_source`]) over the built-ins a program registers ([`builtins`]).
//! An address speaks the envelope of [`address`]; the method names and their deadlines are in [`wire`], and the
//! bounds every answer is held to in [`limits`].

#![forbid(unsafe_code)]

pub mod address;
pub mod builtins;
pub mod calls;
pub mod choice;
pub mod facts;
pub mod failure;
pub mod forecast;
pub mod limits;
pub mod members;
pub mod models;
pub mod source;
pub mod wire;

pub use address::{AddressAuth, SourceAddress};
pub use choice::{
    ProviderSources, SourceBy, SourceChoice, SourceChoiceLastError, SourceElement, SourceSecret,
    SourceSpec,
};
pub use members::{FactMember, MemberState, member_state};
pub use source::{
    CredentialPresenter, FactsReport, FactsSource, ModelsReport, ModelsSource, PresentAsk,
    Presented, ResetReport, SourceAsk, SourceDescriptor, SourceError, SourceErrorCode,
};
