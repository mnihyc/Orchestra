# Orch documentation

Start with [a team working beside an agent](user/getting-started.md): one person
starts the fix, a colleague reads and answers the waiting request, and both
continue the same thread while talking in human chat. The scripted adapter can
exercise that loop without a model; a real provider needs its own successful
sign-in or API-key setup. [Install for your team](../deploy/README.md) after
reviewing the deployment's current support and validation limits.

- [Architecture](development/architecture.md) explains ownership and boundaries.
- [Component inventory](development/components.md) maps every source package and feature area.
- [Extension development](development/extensions/README.md) explains contracts and checks.
- [User guide](user/README.md) explains the screens and common work.
- [Operator guide](operator/README.md) covers deployment and recovery procedures.
- [Security model](security/README.md) describes controls and their limits.

[`openapi.json`](openapi.json) is generated from the HTTP contract types and
route declarations. Its matching browser types are generated in the same
check. The release runner records actual deployment, native harness and browser
results; reserved implementations and unrun proofs do not become supported
merely because an interface is documented.
