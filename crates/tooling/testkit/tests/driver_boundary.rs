//! The shared contracts and production callers remain independent of driver implementation names.

#[test]
fn implementation_names_stay_behind_the_driver_boundary() -> Result<(), Box<dyn std::error::Error>>
{
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let output = std::process::Command::new("python3")
        .arg("-I")
        .arg(root.join("scripts/check-driver-boundary/check.py"))
        .arg("--root")
        .arg(&root)
        .output()?;
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
