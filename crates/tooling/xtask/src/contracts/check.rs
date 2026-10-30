//! Check generated contract drift once generation exists.
//!
//! `cargo xtask contracts --check` generates everything again beside the
//! committed files and fails when one of them differs, naming it.

use std::path::Path;

/// Compares a freshly generated file with the committed one.
pub fn same(fresh: &Path, committed: &Path) -> Result<(), String> {
    let read = |path: &Path| {
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
    };
    if read(fresh)? == read(committed)? {
        Ok(())
    } else {
        Err(format!(
            "{} is out of date: run `cargo xtask contracts` and commit the result",
            committed.display()
        ))
    }
}
