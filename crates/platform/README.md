# Platform libraries

Shared configuration, observability, trust, transport and plugin mechanisms, and the contract suites.

- [orch-config](config/README.md): Configuration loading shared by service and machine entry points.
- [orch-telemetry](telemetry/README.md): Structured logging and metrics shared by components; what a system records stays inside it.
- [orch-pki](pki/README.md): Certificate authority and machine identity mechanisms.
- [orch-link](link/README.md): Shared reliable link mechanisms; persistence of product state belongs to the API.
- [orch-plugin](plugin/README.md): External plugin transport, supervision and trait bridges.
- [orch-conformance](conformance/README.md): The suite every implementation of a contract passes, for tests and for orchctl plugin check.
