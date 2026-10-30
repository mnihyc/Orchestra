# Driver boundary

A host is an administrative scheduling destination. Its driver may manage one
machine or many. The unit reference and storage name are opaque to the controller;
placement within a host belongs to the driver. Drivers may decline optional
operations with a reason, and callers must preserve that reason.

The boundary covers the following responsibilities:

| Responsibility | Contract and caller | Rule |
| --- | --- | --- |
| Template validation | Driver spec schemas, template API and schema form | Validate the selected pool and render its schema without selecting behavior by driver name |
| Machine configuration | Driver configuration schema and machine policy | Accept an open map of driver configurations and validate each loaded driver's value |
| Images and builds | Source declarations, machine policy, image commands | Apply image and registry ceilings to every image-based driver; refuse sources the destination does not offer |
| Capabilities | Capability patch map and descriptor | Pick the patch by the selected driver's key; gateway capabilities remain independent |
| Capacity and load | Driver host observations and node facts | Use the driver's scheduling scope; a remote cluster is not the node agent's machine |
| Storage | Driver storage observations and volume requests | Names may denote storage classes; never derive an opaque volume's path in the controller |
| Existing directories | Data storage declaration | Offer local paths only when explicitly supported |
| Connector and tools | Runtime launch inputs and delivery declaration | The driver delivers immutable verified builds; a shared host directory is one implementation |
| Gateway access | Runtime gateway URL and gateway declaration | The driver establishes a reachable trusted route without exposing enrollment material |
| Ports | Gateway carrier and node listener support | Place/link routes use the connector; a host listener needs explicit support and local permission |
| Installed files | Kept-file declaration and owned capture operations | A replacement may preserve a layer only when the driver claims it; volumes remain independent |
| Private views | Private-view and mount declarations | Unsupported private writes remain read-only and carry their reason |
| Live mounts and devices | Live-change declarations and driver operations | A helper or native update is private to its driver; absence stays pending for recreate |
| Restricted egress | Network modes and verified policy revision | Declare restriction only after proving the trusted routes and denied routes |
| Reconciliation | System-scoped list and opaque unit reference | Labels, API selectors and process ledgers are driver implementation choices |
| Build execution | Source declarations and progress | A host builder, remote builder or refusal must be explicit; scheduling does not assume a local daemon |
| Image archives | Source declarations and import/export operations | Loading an archive is optional; an image-reference-only driver need not load one |
| Removal and recovery | Owned resource kinds and opaque references | Review exact resources, keep units separate from volumes, and protect successor and foreign resources |

Built-in implementation glue belongs beside composition and inside driver crates.
It may translate local configuration and adapt driver operations; generic runtime,
policy, API and screen code must use declarations rather than implementation names.
Adding a driver must not add another branch to those callers.
