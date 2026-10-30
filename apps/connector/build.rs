#![forbid(unsafe_code)]

fn main() -> Result<(), orch_build_support::Error> {
    orch_build_support::emit_release_identity()?;
    orch_build_support::emit_component_build_number("connector")?;
    Ok(())
}
