# Responses and Messages

The two implementations convert requests and incremental replies through the model contract's translator interface.
The outgoing protocol owns authorization and observes upstream usage before conversion. A response converter belongs to
one request, including its custom tools, token-count or compaction route and signed continuation state.

| Module | Responsibility |
| --- | --- |
| lib | Descriptors, declared fidelity and request-local converter selection |
| requests | Text and images, client tools, output limits, structured formats and opaque signed continuation |
| stream | Incremental events, tool arguments, usage, terminal reasons and error documents |

The request carries the selected model's captured declarations in UpstreamSide.model. When a Responses caller omits its
output allowance, the Messages request uses the offering's reported output maximum or the declared max_tokens default.
Signed reasoning is carried as an opaque string and restored exactly. It is not decrypted or re-signed;
the original upstream validates the restored signature. Custom tool grammars become one string argument, with the grammar in the description.
A custom argument is unwrapped when that tool block ends; other content and ordinary function arguments pass incrementally.
