# Scoped operation declarations for ports

This is a design prototype. It does not register routes or widen key admission.

The operation contract already declares actions, resource families, target roles and
whether a Read facet is required or only needed for existing response content. A
declaration row should select the native source for each role. The same producer then
serves browser and key requests through the existing registry and authorization code.

There are three preparation shapes:

- Stored subjects: an authoritative stored row identifies the object governing an act.
- Selected rows and projection sources: the ordinary selector supplies each selected
  identity and every distinct source included by its native renderer.
- Prepared effects and creation: the ordinary planner supplies actual sources,
  destinations and newly created identities, and the writer consumes those operands.

The adapter binds those facts into `OperationPlan`, preserving the consuming operation
and variant. Missing facts remain unresolved. An empty collection is bound only after
the selector has proved it empty. Ordinary permissions continue through the native
producer. Optional Read facets select a native full response or a typed receipt.

Preparation is effect-free and may be repeated at the current-authority checkpoint.
External effects execute once, after the final guard, and deferred delivery retains its
accepted credential. A response object is never used to infer its own authority.

## Declaration rows

The following six rows describe the remaining native preparation shapes in this
prototype. Actions come from each endpoint's existing operation metadata. The row
selects facts for those facets; it does not grant permissions. Sources shown as optional
must be declared as collections so an ordinary Link or Host request can have no named
place without an unresolved singleton being silently treated as empty.

| Operation | Native preparation | Action and actual target source | Publication boundary |
| --- | --- | --- | --- |
| `EnvironmentPortCapabilityGrant` | Capability | Manage Environment from the native row selected by path `id`; policy key from the path | Guarded grant; existing policy details need own-operation Read or a typed receipt |
| `EnvironmentPortOpeningPreflight` | OpeningReview | Manage Environment from path `id`; Read actual optional named place from `resolve_request`; native host/publisher facts from `plan_of` | Store the review once after current validation; no listener is bound |
| `EnvironmentPortOpeningCreate` | OpeningConfirmation | Manage Environment; Read optional named place and actual sources from the owned stored review; `client_id` selects native replay | Guarded production, once-only newly generated link proof, origin-bound host publication after commit |
| `NodePortOpeningList` | SelectedOpenings | Manage current typed Node from path `id`; each selected opening supplies its actual Environment, publisher and protected name sources | Current native row projection under this operation's own Read facets |
| `EnvironmentPortUseCreate` | PortUse | Read and Write actual Environment from path `id`; native `may_use` validates an optional stored opening and its current audience | Accepted caller retained through ticket redemption and stream delivery |
| `EnvironmentPortEnter` | BrowserEntry | Read and Write actual Environment; native `may_use` selects the door or named audience | Entry code and browser use retain the accepted credential and route surface |

`EnvironmentPortOpeningCreate` reloads and validates the stored review's creator,
environment, port, expiry and replay state. It does not accept a replacement audience or
publisher from the confirmation body. Generated proof is returned once under the native
rule; a replay cannot read an existing opening without that operation's Read facet.

The three existing opening Get/Update/Delete producers remain consumers of the same
stored-opening and projection facts. Their current receipt, source and name checks are
the reference behavior. The static path readers remain native readers and need the same
source-footprint audit before being described as qualified.

## Adapter signature

The sketch below describes proposed types, not compiled implementation. Each
`RoleBinding` names a facet of the endpoint's canonical metadata and a factual role
provided by the ordinary native preparation. Its action, business family and disclosure
usage are read from that metadata, so the table cannot introduce a second privilege
definition. The native preparation supplies explicit business kinds when a role contains
more than one family.

```rust,ignore
struct PortDeclaration {
    operation: &'static str,
    preparation: PortPreparation,
    bindings: &'static [RoleBinding<PortFact>],
    projection: PortProjection,
    boundary: PublicationBoundary,
}

// Example row; the optional-place facet is a declared collection.
const OPENING_CONFIRMATION: PortDeclaration = PortDeclaration {
    operation: "EnvironmentPortOpeningCreate",
    preparation: PortPreparation::OpeningConfirmation,
    bindings: &[
        RoleBinding::primary(PortFact::ReviewEnvironment),
        RoleBinding::collection("named_place", PortFact::ReviewNamedPlace),
    ],
    projection: PortProjection::Opening,
    boundary: PublicationBoundary::ProducedOpening,
};

fn bind_port_preparation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    declaration: &'static PortDeclaration,
    prepared: PreparedPortOperation,
) -> BoxFuture<'a, ApiResult<OperationPlan>>;
```

`PreparedPortOperation` is the handoff from the existing ordinary planner. It owns the
validated native operands and a complete set of factual role groups. It has no public
serialization or value-bearing debug output. A fact is absent, a proved-empty collection,
or explicit typed targets; those cases cannot be interchanged. The resulting
`OperationPlan::with_data` carries the same operands to the native writer.

`PortProjection::Opening` composes the full opening renderer's declared Environment,
Host, Machine and protected-name roles. It supplies those facts from the native
preparation and requires this operation's optional response Read facets before exposing
existing opening content. The two example bindings above are the effect-admission
bindings; they do not claim to be the full response footprint.

The shared binder looks up each facet, resolves current placement with the existing
target library, validates its declared business kinds, and binds it once using `bind`,
`bind_collection` or `bind_empty`. It then uses the existing admission and ordinary
principal. It does not reimplement `resolve_request`, `may_use`, `produce_prepared`,
capability policy, idempotency or port lifecycle rules.

The existing `opening_operation::returned_sources`, `projection_names` and native
`opening_view` supply full-response facts. They are composed with the same operation's
response-content Read facets and ordinary current reader. If that disclosure is refused
after the effect completed, `completed_projection` returns the accepted receipt; it
never re-executes the opening or rolls back a completed effect.

Opening review, confirmation and selection expose their native facts once at their
shared producer boundaries. Typed registry declarations attach their input/output
contracts to these rows. Adding another compatible port operation is a row and native
role declaration, without a resolver that switches on endpoint names.

## Qualification

The current source has no implementation of these proposed rows or types. Before any
row enables admission, qualification must demonstrate allowed browser/key effect parity,
independent wrong-operation/action/kind/target refusals, genuine write-only receipts on
success/replay/error, and scope/current-source loss at each native wait and publication
boundary. Place, Link and Host choices must all remain available under their original
ordinary rules. A stable browser entry must also preserve the original key and its bounds;
removing a session gate alone is insufficient.
