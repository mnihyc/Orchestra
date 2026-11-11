//! The ledger read at three depths as figures, series and records, in tokens and credits; rates, with the catalogue of estimates and its upload, fetch and restore; caps on tokens, on credits and on turns running at once, with who is waiting and how often each was passed.
//!
//! **Every figure is a view of the ledger.** What a request returns is a sum of
//! the rows that count (`figures`), or the rows themselves (`events`); credits
//! are an estimate in the unit the rates define, a row without a rate has no
//! credits and its tokens are counted apart, and nothing is billed. How deep a
//! reader is shown is decided by the access engine and by nothing here
//! (`scope`): their own use in full, a place's figures without anybody else's
//! records, or everybody's use and every record.
//!
//! **Rates** (`rates`): what each model usable at a node would be counted at,
//! from where that rate comes, and which have none; the shipped catalogue of
//! estimates as it is loaded; and its replacement by a system admin, from a
//! file, from the address an admin set, or from the release.
//!
//! **Caps** (`caps`): made, changed and removed by whoever the place of the cap
//! says, each with how it stands; everything that bounds work at a node; a
//! person's own share of every cap counted per person; and who waits for room in
//! a cap on turns running at once.

orch_api_base::api_family_callback! {
    bind __api_bind_641a976536, __api_install_641a976536, __api_callback_641a976536;
    slot __ApiCallback641a976536, __API_CALLBACK_641A976536, __api_set_641a976536;
    base orch_api_base::routes::usage;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

#[path = "usage/caps.rs"]
pub mod caps;

#[path = "usage/events.rs"]
pub mod events;

#[path = "usage/figures.rs"]
pub mod figures;

#[path = "usage/names.rs"]
pub mod names;

#[path = "usage/native.rs"]
pub mod native;

#[path = "usage/scope.rs"]
pub mod scope;
