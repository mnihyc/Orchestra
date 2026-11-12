# Organization cost source

The optional **Admin key** is an encrypted, write-only field of this provider's
facts source. It is independent of the key used for generation and is never
passed to the model forwarder or an agent environment. Removing or replacing it
uses the ordinary source editor and its revision check.

The source reads the current UTC day's `/v1/organization/costs` buckets, grouped
by project and line item. Each row keeps its native decimal amount, currency,
bucket and dimensions; it does not invent a prepaid balance or a combined total.
Priority, credit balances and model-key validity are not inferred from a successful
administrative read. Without an admin key, spending is not offered with the reason
“needs an admin key”; no request is made with the ordinary model key.

Pagination preserves opaque cursors, refuses repeated cursors and stops with a
visible failed read after eight pages. A current-day daily query normally has one
bucket; this bounded guard prevents a faulty endpoint from consuming an unbounded
refresh. Existing source deadlines, one-MiB replies and normalized item limits
remain in force. A failed read preserves the preceding observation and its date.

[Costs API documentation](https://developers.openai.com/api/reference/resources/admin/subresources/organization/subresources/usage/methods/costs)
provides the fixture shape. Test data is authored and anonymized from that shape,
which was independently confirmed on an account; it is not a local live recording.
No test needs a vendor key or model call.

The extension also declares Codex's remote compaction support for the exact
HTTPS Responses endpoint at `api.openai.com`, independently of cost-source
credentials. The capability registry selects the public `v2` setting for a
forwarded generation route; compatible endpoints keep `unsupported` unless
their own extension declares support. Native sign-ins keep Codex's behavior.
The selected setting travels with the thread's existing native configuration
and changes through its normal session reconfiguration. Compaction uses the
ordinary Responses request with `compaction_trigger`, not a separate compact
relay route. The tests use authored endpoint facts and requests; they do not
claim a live compaction observation.

The same Responses endpoint declares Codex's standalone native search form and
the public `supports_standalone_web_search` setting. The program adapter consumes
the capability registry's selected setting without inspecting endpoint or model
names. The forwarder requires the current endpoint declaration, the captured
model, native route and exact mode after option overlays, then uses its ordinary
credential, model ceiling, privacy checks and meter. A saved flag or old binding
cannot authorize a changed endpoint. The authored relay cases exercise an HTTP
fixture; a live native API observation remains a separate qualification.
