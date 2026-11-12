//! Actual native action subjects shared with the retained lifecycle reader.

use super::*;

pub(super) use orch_engine::jobs::host_delete::reviewed::Footprint;
pub(super) fn of(stored: &Stored) -> Footprint {
    Footprint::of(
        &stored.environments,
        &stored.providers,
        stored
            .data
            .as_ref()
            .map(|data| (&data.custody, data.purges.as_slice())),
    )
}
