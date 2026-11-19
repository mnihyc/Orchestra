# Provider request options

A provider manager enables a model's tiers and access programs. Threads and auxiliary calls choose within those values. Discovery supplies offers; it does not enable paid choices. Live native model lists supply the available IDs and labels, while the wire protocol supplies its ordinary option shape. Managers can add IDs and schemas for compatible endpoints without declaring that an upstream implements them.

Endpoint `options.params` supplies overridable request defaults. It never fixes a value. A model can additionally declare `parameters.request_options`, a map from protocol names to JSON Schemas over request options. Use `native` for declared adapter parameter keys. This uses the same local schema validation as extension configuration and the existing provider management authority. Option forms support properties, items, local references, enums, constants, defaults and scalar bounds. Conditional schemas are refused on save because the option editor cannot explain their combinations.

```json
{
  "service_tiers": [{ "id": "priority", "label": "Fast" }],
  "request_options": {
    "openai-responses": {
      "type": "object",
      "properties": {
        "service_tier": { "enum": ["default", "priority", "fast"] },
        "vendor": {
          "type": "object",
          "additionalProperties": false,
          "properties": {
            "region": { "const": "local" },
            "mode": { "enum": ["a", "b"], "default": "a" }
          }
        }
      }
    },
    "native": {
      "type": "object",
      "properties": { "service_tier": { "enum": [null, "priority"] } }
    }
  }
}
```

`enum` limits choices, `const` fixes a value, and `default` supplies a starting value that an explicit request can replace. `required` controls absence. Explicit null is a value, subject to the schema; tier/program null selects ordinary processing or no program. Missing fixed values and defaults are filled through properties, local references and array items before validation. Schemas can close custom extensions with `additionalProperties: false`; without that rule, the existing extension surface remains open.

The final outgoing options must satisfy the enabled model lists and the schema after merging and translation. Omission and `auto` resolve locally to the model's configured start or an explicit ordinary value, so an upstream account default cannot widen this provider's choices. A premium default does not prevent choosing Standard or no program. Fixed controls identify the provider as their source; inherited setting locks retain their separate source.

New leases and auxiliary commands retain their model constraints with their request options. Retrying an issued auxiliary command preserves those frozen values. A legacy lease or uncompleted auxiliary request without constraint metadata must be reissued; it is refused before model HTTP rather than acquiring new options retrospectively. Completed journal results remain replayable. An opaque translation without a verifiable final model is refused before HTTP. Internal embeddings retain their float decoder and generation dimensions; raw forwarding retains its own encoding.

## Protocol distinctions

Responses uses `service_tier`, including `fast`, its `priority` alias, and `ultrafast` where available. Its Cyber selector is `access_programs.cyber`, with wire IDs such as `daybreak_blue`. An upstream may resolve omitted selectors from account/model settings, which is why Orch resolves them explicitly. Native Codex keeps its separate `serviceTierForTurn` and `cyberAccessProgram` mapping and uses the actual app-server catalogue. [Responses reference](https://developers.openai.com/api/reference/resources/responses/methods/create), [Fast mode](https://developers.openai.com/api/docs/guides/fast-mode), [Ultrafast](https://developers.openai.com/api/docs/guides/ultrafast-mode), [Daybreak](https://developers.openai.com/api/docs/guides/daybreak), [Codex App Server](https://developers.openai.com/codex/app-server/).

Compaction documents a service-tier selector, but no Cyber-program selector. Orch refuses a generation-only Cyber choice on that endpoint rather than claim it can enforce an undeclared control. [Compaction reference](https://developers.openai.com/api/reference/resources/responses/methods/compact).

Anthropic inference speed is `speed`, separate from capacity `service_tier`. The observed value is in `usage.speed`. Claude Code's `ultracode` enables workflow orchestration with high reasoning effort; it is not an inference tier. Managed sessions keep workflows disabled until their orchestration can preserve issued work ownership and accounting. [Fast mode](https://platform.claude.com/docs/en/build-with-claude/fast-mode), [Dynamic workflows](https://claude.com/blog/introducing-dynamic-workflows-in-claude-code).

Gemini documents `service_tier` for Priority/Flex and its OpenAI-compatible surface. A downgrade is billed at the actual tier; native Gemini exposes that observation through `x-gemini-service-tier`. Orch's shipped wire protocols do not claim a native Gemini adapter. Compatible endpoints and external translators use their declared protocol and configured schemas. [Gemini compatibility](https://ai.google.dev/gemini-api/docs/openai), [Priority inference](https://ai.google.dev/gemini-api/docs/priority-inference).

DeepSeek documents protocol-specific thinking and effort fields, including Responses `reasoning.effort`. An OpenAI-compatible interface alone does not establish support for another vendor's premium tiers or Cyber programs. Configure only the documented options for that endpoint/model. [Thinking mode](https://api-docs.deepseek.com/guides/thinking_mode/), [Responses](https://api-docs.deepseek.com/api/create-response/).

Pricing uses the reported processing tier and the configured rate, including cache/context prices. Requested choices remain separate facts. Missing or ambiguous tier observations remain unknown; they do not prove ordinary pricing or the requested premium price.
