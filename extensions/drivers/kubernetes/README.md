# orch-driver-kubernetes

Kubernetes implementation of the runtime driver contract.

The node does not load it in this release: it is built in only with the node's `kubernetes` feature
(`cargo build -p orch-node --features kubernetes`), and a node without it reports the driver as not part of
this release.

This crate defines cluster configuration and the building blocks for admission,
storage identity, connector launch and process isolation. The driver is not yet
registered for runtime work; its checks include configuration and ownership cases.

| Module | Responsibility |
| --- | --- |
| [admission](src/admission.rs) | Admission of a managed unit before any cluster object is created. |
| [authenticated_control](src/authenticated_control.rs) | Protect guard messages independently of the short-lived exec relay. |
| [capacity](src/capacity.rs) | Aggregate capacity of compatible, ready and schedulable cluster workers. |
| [claim_service](src/claim_service.rs) | Serialize claim creation and durably bind the native UID before answering. |
| [claims](src/claims.rs) | Persistent claim requests with an owned creation intent. |
| [class_limits](src/class_limits.rs) | The shared runtime-class limitations for a Kubernetes environment. |
| [client](src/client.rs) | A bounded, explicitly configured connection to the Kubernetes API. |
| [config](src/config.rs) | Cluster connection, storage and runtime-class configuration. |
| [control_protocol](src/control_protocol.rs) | Bounded message types for a unit's trusted guard. |
| [encrypted_transport](src/encrypted_transport.rs) | Bounded transport for encrypted guard messages on an untrusted exec stream. |
| [guard_dispatch](src/guard_dispatch.rs) | Serialize authenticated guard commands without turning an interrupted operation into success. |
| [guard_link](src/guard_link.rs) | A single pinned supervisor channel, permanently fenced by a partial exchange. |
| [guard_stop](src/guard_stop.rs) | Stop exact workload processes only after the trusted supervisor blocks new starts. |
| [k3d_test](src/k3d_test.rs) | Cluster-backed checks for the connection, reported capacity and claim-incarnation primitives. |
| [launch](src/launch.rs) | Managed connector launch paths and private one-run files. |
| [launch_store](src/launch_store.rs) | Guard-owned launch values for the life of one retained Pod. |
| [lib](src/lib.rs) | Kubernetes implementation of the runtime driver contract. |
| [locks](src/locks.rs) | Serialize mutations of the same native name without blocking other names. |
| [logs](src/logs.rs) | Bounded native logs fenced to a Pod and container incarnation. |
| [names](src/names.rs) | Map node volume names to Kubernetes DNS-subdomain names without losing identity. |
| [network_policy](src/network_policy.rs) | Build one unit's restricted NetworkPolicy from exact verified endpoints. |
| [ownership](src/ownership.rs) | Incarnation and consumer checks for cluster-owned objects. |
| [peer](src/peer.rs) | Authenticate the workload supervisor by kernel credentials and its container scope. |
| [platform](src/platform.rs) | Select workers compatible with the node's verified Linux tool artifacts. |
| [pod](src/pod.rs) | A managed Pod request, with no credentials or node paths in its API object. |
| [pod_identity](src/pod_identity.rs) | The native identities of a managed Pod's two long-lived containers. |
| [pod_read](src/pod_read.rs) | Read the admitted Pod's spec without losing fields unknown to the typed client. |
| [private_environment](src/private_environment.rs) | Read private environment values before the supervisor starts the connector. |
| [process_scope](src/process_scope.rs) | Identify a workload's processes without matching command lines. |
| [receipts](src/receipts.rs) | A durable creation-intent and native-UID journal for claims. |
| [relay](src/relay.rs) | An untrusted exec process relays only authenticated ciphertext to the guard. |
| [run_files](src/run_files.rs) | One-run connector files, created through a verified memory-backed directory. |
| [schema](src/schema.rs) | Machine-policy schema for the Kubernetes connection. |
| [stage_exchange](src/stage_exchange.rs) | Two-phase acknowledgement for a private staged tree. |
| [stage_protocol](src/stage_protocol.rs) | A bounded, Pod-bound header for the trusted tool-staging transfer. |
| [stage_worker](src/stage_worker.rs) | Private staging and publication for the trusted init container. |
| [staging](src/staging.rs) | Stream a verified node tool cache into a bounded, digest-checked staging archive. |
| [subreaper](src/subreaper.rs) | Child accounting for a dedicated workload supervisor process. |
| [supervisor_protocol](src/supervisor_protocol.rs) | Messages on the private, pinned guard-to-supervisor connection. |
| [supervisor_state](src/supervisor_state.rs) | Local supervisor transitions; the guard still verifies process scope and channel identity. |
| [template_schema](src/template_schema.rs) | Template fields accepted by a Kubernetes environment. |
| [unit_create](src/unit_create.rs) | Create and identify a Pod without starting its connector. |
| [verify_admission](src/verify_admission.rs) | Compare the security-relevant parts of an admitted Pod with the planned Pod. |
| [verify_claim](src/verify_claim.rs) | Verify an admitted PVC from its raw API representation, including unknown fields. |
