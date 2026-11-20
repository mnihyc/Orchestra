# Limit a provider's request options

Choose which tiers, access programs and other request options a provider's models allow, and write the rules that every request must meet. At the end threads and automatic calls can choose only within what you enabled, whatever the upstream's own defaults are.

A provider manager enables a model's tiers and access programs. Threads and automatic calls choose within those values. Discovery supplies offers; it never enables a paid choice. The live model lists supply the available IDs and labels, and the wire protocol supplies its ordinary option shape. You can add IDs and schemas for compatible endpoints without saying that the upstream implements them. People set the same rules in the model editor, as [Choose and read provider details](../user/provider-details.md#request-option-rules) describes. This page covers the JSON.

## Before you start

You need to manage the provider. Know which protocol each endpoint speaks (`openai-responses`, a completions protocol, Anthropic Messages) and read the vendor's documentation of the options you want: a compatible interface alone does not show that the vendor supports another vendor's premium tiers.

## 1. Write the rules

1. In **Models**, open the model's **Edit options…** and choose **Raw JSON**, or edit `parameters.request_options` through the provider API.
2. Give a map from protocol names to JSON Schemas over the request options. Use `native` for the declared parameter keys of an agent program's adapter.

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

3. Save. The editor checks the schema locally with the same validation as extension configuration. Option forms support properties, items, local references, enums, constants, defaults and scalar bounds. A conditional schema is refused on save, because the editor cannot explain its combinations.

What the keywords do:

- `enum` limits choices, `const` fixes a value, `default` gives a starting value that an explicit request can replace, and `required` controls absence.
- An explicit `null` is a value, subject to the schema. A tier or program of `null` selects ordinary processing or no program.
- A missing fixed value or default is filled in through properties, local references and array items before validation.
- `additionalProperties: false` closes a custom extension. Without it, the extension surface stays open.

An endpoint's `options.params` gives overridable request defaults and never fixes a value.

## 2. Know how it is enforced

- The final outgoing options must satisfy the enabled model lists and the schema, after merging and translation. Omission and `auto` resolve locally to the model's configured start or an explicit ordinary value, so an upstream account default cannot widen this provider's choices. A premium default does not stop anyone from choosing Standard or no program.
- A control that is fixed names the provider as its source. A setting lock keeps its own source.
- A new lease keeps the admitted turn's request options and managed access-program choice apart from the model's constraints. An agent program's own fields cannot replace that frozen choice, and a repeated lease opening or a runtime restart keeps it.
- Automatic commands keep their model constraints with their request options, and retrying an issued automatic command keeps those values. A translation without a verifiable final model is refused before HTTP.
- Internal embeddings keep their float decoder and generation dimensions. Raw forwarding keeps its own encoding.

## 3. Check it

Open a thread on the provider and its **Parameters → Request options**: the fields you opened appear, grouped by interface; a fixed value is read-only. Try a value outside the schema: the request is refused and the error names the field.

## What each vendor supports

| Protocol | Options | Notes |
|---|---|---|
| Responses | `service_tier` (`fast`, its `priority` alias, and `ultrafast` where available); the Cyber selector `access_programs.cyber`, with wire IDs such as `daybreak_blue` | Ordinary access omits the program selector; a paid choice uses the declared field and still satisfies the provider's final constraints. Forwarded Codex sessions use the same managed access-program choice as the program's own sessions, frozen on their lease because the program does not carry it for a custom provider identity. Codex keeps its own `serviceTierForTurn` and `cyberAccessProgram` mapping and the real app-server catalogue. See the [Responses reference](https://developers.openai.com/api/reference/resources/responses/methods/create), [Fast mode](https://developers.openai.com/api/docs/guides/fast-mode), [Ultrafast](https://developers.openai.com/api/docs/guides/ultrafast-mode), [Daybreak](https://developers.openai.com/api/docs/guides/daybreak) and [Codex App Server](https://developers.openai.com/codex/app-server/) |
| Codex remote compaction | A `compaction_trigger` input item on the ordinary Responses endpoint (the `v2` protocol) | The provider extension must declare support first. It keeps the same lease, final request limits and usage observer. The separate `/responses/compact` endpoint is not admitted |
| Anthropic Messages | Inference speed `speed`, apart from capacity `service_tier`; the observed value is in `usage.speed` | Claude Code's `ultracode` turns on workflow orchestration with high reasoning effort; it is not an inference tier, and managed sessions keep workflows off until they can keep issued work ownership and accounting. See [Fast mode](https://platform.claude.com/docs/en/build-with-claude/fast-mode) and [Dynamic workflows](https://claude.com/blog/introducing-dynamic-workflows-in-claude-code) |
| Gemini | `service_tier` for Priority and Flex, on its OpenAI-compatible surface | A downgrade is billed at the actual tier; Gemini itself reports it in `x-gemini-service-tier`. Orch ships no Gemini adapter: compatible endpoints and external translators use their declared protocol and the schemas you configure. See [Gemini compatibility](https://ai.google.dev/gemini-api/docs/openai) and [Priority inference](https://ai.google.dev/gemini-api/docs/priority-inference) |
| DeepSeek | Protocol-specific thinking and effort fields, including Responses `reasoning.effort` | Configure only the options the vendor documents for that endpoint and model. See [Thinking mode](https://api-docs.deepseek.com/guides/thinking_mode/) and [Responses](https://api-docs.deepseek.com/api/create-response/) |

Pricing uses the processing tier that was reported and the configured rate, cache and context prices included. The choices that were requested stay separate facts. A missing or ambiguous tier observation stays unknown: it proves neither ordinary pricing nor the requested premium price.

## If it does not work

| You see | What to do |
|---|---|
| The save is refused | The schema is conditional (for example `if`, `then`, `else`) or does not validate. Simplify it |
| A request is refused although the value looks right | The final options, after translation, do not satisfy the model list or the schema. Compare the refused field with the schema of the model and the endpoint |
| A thread cannot choose a premium tier | A manager has not enabled it on the model, or the model list does not include it. Discovery alone does not enable it |
