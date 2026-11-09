# Operation binding rows

This design sketch describes a table consumed by the existing native read adapter.
It does not enable additional operations. A declaration is a description of facts
that a native producer must supply, and does not replace ordinary authorization.

The existing Mount family provides a small prototype. Its Get and List operations
share the native Mount preparation, protected names, and physical Machine sources.

| Operation | Primary kind | Identity source | Action | Native adapter |
| --- | --- | --- | --- | --- |
| MountGet | Mount | Path parameter `id` | Read | Mount single-item factory |
| MountList | Mount | Native visible rows after the original org filter | Read | Mount exhaustive collection factory |

Both rows expand the same Mount source declaration. Machine identities come from
the native location and retained-data preparations. Group and node names come from
the prepared access/home projection. An empty source is explicitly bound only when
the producer has established that it is empty.

The typed row pair can be expressed without string dispatch:

```rust,ignore
struct ReadRows<G, L> {
    get: G,
    list: L,
}

const MOUNT_ROWS: ReadRows<MountGet, MountList> = ReadRows {
    get: MountGet,
    list: MountList,
};

fn register_read_rows<F, G, L>(registry: &mut Registry, rows: ReadRows<G, L>)
where
    F: Family,
    G: Endpoint<Query = (), Body = (), Reply = F::Item>,
    L: Endpoint<Query = F::ListQuery, Body = (), Reply = Vec<Listed<F::Item>>>,
{
    // Existing resources::read::register supplies both typed resolvers and handlers.
    resources::read::register::<F, G, L>(registry, rows.get, rows.list);
}
```

The implementation remains the existing `resources::read::Family` contract and
`resources::read::register` function. The family chooses its existing permission,
filter, cardinality, preparation and renderer once. A consuming operation keeps its
own catalogue facets and disclosure check; it cannot borrow another operation's Read.
Current HTTP identity and current operation facts are reconstructed inside the read
snapshot, and the original final deadline check remains after auxiliary Get work.

This sketch is deliberately documentation only. Each further native family must
first preserve its actual Get/List differences, side effects and response sources.

The same approach has three native preparation classes:

| Class | Facts supplied by the native operation | Shared boundary retained |
| --- | --- | --- |
| Stored subject | A stored child identity, its real governor and the ordinary access branch used | Current typed target resolution and the original account permission check |
| Selected rows | The actual selected identities, foreign content sources and protected names | Scope filtering before pagination, aggregation or rendering, then current response projection |
| Prepared effect | The actual source/destination closure and operands consumed by the native writer | Fresh guarded admission before effects, safe receipt, and original credential at later execution/delivery |

Collection cardinality is part of the declaration. A native operation can bind a
checked empty or heterogeneous collection only when it declares a collection role.
A missing singular subject cannot be replaced by an empty collection or an invented
identity. A preview binds its real existing sources without inventing the identity
of an object that would be created later.

Write operations keep the existing native transaction and writer. The adapter
stores prepared operands for that writer, returns a truthful receipt when Read is
absent, and invokes the original current projection only with that same operation's
Read facet. Deferred work and byte delivery retain the originating credential and
accepted bounds and check them against current authority at their actual boundary.
