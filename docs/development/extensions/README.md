# Extension guides

An extension implements a contract and declares its capabilities. Callers use
that declaration, so a new implementation fits the same forms, permissions
and lifecycle as the existing one. Rust implementations live under
`extensions/`; local external programs use the JSON-lines plugin transport.
Start with the relevant contract README and run its reusable suite before
registering it. A capability requires evidence of its effect, not only a flag.

- [Drivers](drivers.md)
- [Agent adapters](adapters.md)
- [Executors and model protocols](executors-and-models.md)
- [Summarizers and notifiers](summarizers-and-notifiers.md)
- [External plugin transport](plugins.md)

Blob storage and identity use Rust trait seams and have no external plugin
transport in this release. Local blobs are implemented; the OIDC implementation
is reserved. The built-in host driver and model summarizer are also reserved;
their contracts and controlled fixtures are available for implementation work.
See the [component inventory](../components.md) and each package's status.

The [Python examples](../../../examples/plugins/README.md) show a summarizer,
file notifier and Linux process driver without third-party dependencies. Their
transport and behavioral checks run through the same bridges used by hosts.
