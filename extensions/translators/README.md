# Model translation seam

The home of the translation contract's external implementations. Orch itself
is a plain relay and builds no translator in. The neutral descriptor,
compatibility and translation interface belong to `crates/contracts/model`;
external translator hosting belongs to `crates/platform/plugin` and the
connector's provider role. No concrete translator ships in the current scope.
A translator must preserve streaming, tools, usage and failure semantics or
declare its fidelity limits. Its contract suite belongs in `crates/platform/conformance`.
The [plain-text example](../../examples/plugins/translator/README.md) demonstrates the seam; nothing installs or offers it.
