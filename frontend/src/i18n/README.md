# Interface words

`en.json` and `zh-CN.json` hold the product's interface text. `index.ts` owns locale selection and typed translation keys; `dev.ts` and the development catalogues keep development-only words separate.

`errors.ts` resolves every API error through its typed catalogue key, keeps diagnostic tokens out of
field explanations, and supplies an older client with a safe fallback for a newer server. Each error
sentence says what happened and the next act. The API response boundary embeds this same English
catalogue as its wire fallback; raw codes, request identifiers and field paths remain structured data.
Contract-wide Rust and web tests require both languages and prevent code-like display tokens.
`error-aliases.json` is shared with the API contract's message projector for native and job failures;
unknown program codes use the translated recovery sentence while structured diagnostics stay intact.

`choices.ts` reads a choice's familiar `name` and its separate `description`. Static product choices use `choiceCatalog.<group>.<value>`; feature-owned pairs may stay with that feature's catalogue section. `choiceCopy` returns the pair, `choiceOption` also supplies the compatible plain label, and `offeredChoice` leaves an unknown plugin term to its caller. Dynamic names are actual resource, model, program or native values, with their address, summary or context in the description.

Use the shared `ui/Choice` through Select, RadioGroup, SegmentedControl, Menu or CheckboxList. A raw Checkbox keeps its native HTML `name`; pass its visible term as `label` and its sentence as `description`. Custom selectors use Choice's render child to associate the name and description with the control. Existing explanations, offered values, disabled reasons, guide behavior and mutation handlers stay intact.

`SETTING_CHOICE_GROUPS` connects registry enum values to their paired copy. A setting editor supplies the key once through `features/settings/choiceCopy.ts`; object, list and ordered choices inherit it. Adding a registry enum requires copy in both languages and this mapping. The catalogue tests require every paired choice in both languages; the copy test walks every registry enum, including nested ones, and the component test exercises the actual generic control without adding any offered values.

Catalogue checks resolve `$t(...)` references with the product's translation engine before comparing
visible words. A missing nested target fails without falling back to another language. Native program,
language and file names may stay the same on labels; explanatory sentences still need translation.
Deliberate copied-English and missing-target cases keep those boundaries checked.
