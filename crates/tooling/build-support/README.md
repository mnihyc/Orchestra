# orch-build-support

Build-script release validation and component counter allocation.

See [Contributing](../../../CONTRIBUTING.md) for the implemented commands
and build-counter behavior.

The identity and a component's next build number are also available to a tool that is not a build script
(`release_identity` and `take_build_number`), which is how `cargo xtask identity` answers the browser
application's build with the same figures a Rust build is given.
