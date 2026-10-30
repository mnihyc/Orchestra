# Configurable human broadcasts

Orgs, projects and topics open the same announcement dialog and use the same place-level API. It preserves per-topic delivery defaults and reports actual deliveries and skipped topics. Component and fake-world browser tests cover the shared controls; API tests cover inheritance, private branches and pagination.

Project/topic callers retain the shared trigger; org page menus open the exported same composer body
on demand. It adds no second org header trigger, and unopened menus/dialogs mount no announcement reader.

- `Composer.tsx`: Scope, category, target activity, delivery, fallback and wake choices.
- `Preview.tsx`: Explain the effect of the current selection.
- `Result.tsx`: Recorded delivery counts and per-topic outcomes.
