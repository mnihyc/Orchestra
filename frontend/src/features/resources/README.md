# Resource groups

Display groups reuse each resource's home, first owner, machine of origin and
tags. `ResourceList` preserves its native query, rows and actions, and passes
sections to the existing `Table`. Group counts cover loaded rows. Multiple tag
memberships can repeat a displayed row, while selection still names the original
resource once. View choices and folds use the screen's named device-local key.

`ResourcePicker` and `ResourcePickerLookup` share the existing `Combobox`,
`Choice` and inspection cards. Use the complete picker for native options and
the lookup for a caller-owned search or pager. Give each purpose a stable
`pickerKey`; compound binding, setup and placement responses supply protected
metadata through `PickerContext`. An explicit compound entry, including a
redaction, takes precedence over cached full content. Keep basic options when
metadata is absent. Never infer permission or machine availability from home or
origin, and never make browsing, folding or inspecting commit a value.

Home headings describe actual home defaults through the existing permission
catalogue and Defaults reader. They do not promise that a resource with an
overridden permission page follows those defaults. Mixed or custom policies
keep that qualification and their original sharing editor. A failed or refused
refresh withdraws the cached audience.

Machine registration offers the machine's home as a server-selected default.
Keep `home_id` absent until a person selects an explicit home; a real explicit
context is honoured. The native creation guard chooses and binds the default.
A home move preserves physical bindings and records the override for later
imports. Availability is a native judgement for one actual execution machine;
an empty list means no judgement. Local MCP choices stay visible with their
reason, and enabling them uses the same revisioned settings writer.
