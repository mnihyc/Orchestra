//! Contract artifacts and implementer help generated from the authored Rust contracts.
//!
//! `cargo xtask contracts` writes what other parts are generated from the
//! contract crates: OpenAPI, browser types, settings help and provider interface pages/schemas/examples.

pub mod check;
pub mod openapi;
pub mod providers;
pub mod settings;
pub mod typescript;

use std::path::Path;

/// Generates the contract artifacts under `root`; with `check`, generates
/// them elsewhere and compares.
pub fn run(root: &Path, check: bool) -> Result<(), String> {
    let committed_openapi = root.join("docs/openapi.json");
    let committed_types = root.join("frontend/src/generated/api-types.ts");
    let committed_settings = root.join("frontend/src/generated/settings-registry.ts");
    let scratch = root.join("target/contracts");
    std::fs::create_dir_all(&scratch)
        .map_err(|error| format!("cannot create {}: {error}", scratch.display()))?;
    let (openapi, types) = if check {
        (scratch.join("openapi.json"), scratch.join("api-types.ts"))
    } else {
        (committed_openapi.clone(), committed_types.clone())
    };

    let routes = orch_api_contract::routes::all();
    let document = openapi::document(&routes, env!("CARGO_PKG_VERSION"));
    let mut text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    text.push('\n');
    std::fs::write(&openapi, text)
        .map_err(|error| format!("cannot write {}: {error}", openapi.display()))?;
    eprintln!("{}: {} requests", openapi.display(), routes.len());

    typescript::api_types(&root.join("frontend"), &openapi, &document, &types)?;
    eprintln!("{}", types.display());

    let settings = if check {
        scratch.join("settings-registry.ts")
    } else {
        committed_settings.clone()
    };
    settings::write(&settings)?;
    eprintln!("{}", settings.display());

    providers::run(root, check)?;
    eprintln!(
        "docs/development/extensions/providers: schemas, member tables, constants and examples"
    );

    if check {
        check::same(&openapi, &committed_openapi)?;
        check::same(&types, &committed_types)?;
        check::same(&settings, &committed_settings)?;
    }
    Ok(())
}
