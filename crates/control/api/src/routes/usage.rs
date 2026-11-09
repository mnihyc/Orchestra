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

pub mod caps;
pub mod csv;
pub mod events;
pub mod figures;
pub mod names;
mod native;
pub mod rates;
pub mod scope;

use crate::registry::Registry;
use orch_api_contract::usage::{
    AdminUsageCatalogueSet, MeLimitsGet, UsageCapCreate, UsageCapDelete, UsageCapList,
    UsageCapUpdate, UsageCapWaitingList, UsageCatalogueGet, UsageEventList, UsageGet,
    UsageHealthGet, UsageLimitsGet, UsageRateTableGet,
};

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.creation(UsageCapCreate, caps::native_effects::replay_binding());
    registry
        .on_raw_read_with_optional_operation(
            UsageGet,
            crate::key_operations::native::selected::<figures::Analytics>,
            figures::get,
        )
        .on_snapshot_read_with_optional_operation(
            UsageHealthGet,
            crate::key_operations::native::stored_query::<native::Subjects, ()>,
            figures::health,
        )
        .on_raw_read_with_optional_operation(
            UsageEventList,
            crate::key_operations::native::selected::<events::Records>,
            events::list,
        )
        .on_snapshot_read_with_optional_operation(
            UsageRateTableGet,
            crate::key_operations::native::selected::<rates::Table>,
            rates::table,
        )
        .on(UsageCatalogueGet, rates::catalogue)
        .on_raw(AdminUsageCatalogueSet, rates::set)
        .on_snapshot_read_with_optional_operation(
            UsageLimitsGet,
            crate::key_operations::native::selected::<caps::native_reads::LimitsReader>,
            caps::limits,
        )
        .on(MeLimitsGet, caps::mine)
        .on_snapshot_read_with_optional_operation(
            UsageCapList,
            crate::key_operations::native::selected::<caps::native_reads::List>,
            caps::list,
        )
        .on_snapshot_read_with_optional_operation(
            UsageCapWaitingList,
            crate::key_operations::native::selected::<caps::native_reads::Waiting>,
            caps::waiting,
        )
        .on_with_operation(
            UsageCapCreate,
            crate::key_operations::native::effects::prepare::<caps::native_effects::Create>,
            caps::create,
        )
        .on_with_operation(
            UsageCapUpdate,
            crate::key_operations::native::effects::prepare::<caps::native_effects::Update>,
            caps::update,
        )
        .on_with_optional_operation(
            UsageCapDelete,
            crate::key_operations::native::stored::<native::Subjects, (), ()>,
            caps::delete,
        );
}
