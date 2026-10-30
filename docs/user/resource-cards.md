---
title: Read resource cards and choices
screens: /resources, /resources/providers, /resources/providers/$id, /resources/agents, /resources/agents/$id, /resources/environments, /resources/environments/$id, /resources/hosts, /resources/hosts/$id, /resources/machines, /resources/machines/$id, /resources/templates, /resources/templates/$id, /resources/mounts, /resources/mounts/$id, /resources/mcp, /resources/mcp/$id, /resources/skills, /resources/skills/$id, /thread/$threadId, /topic/$topicId, /project/$projectId, /org/$slug, /admin/$section
keywords: resource card, inspect, choice, provider, model, host, environment, load, speed, context, limits, unknown
---

# Read resource cards and choices

## Inspect a choice

A resource name opens the same card wherever you meet it. Hover or focus the
name for a quick look. Click or press Enter to keep the card open. Escape
closes the top card. Its title opens the resource's page.

In a chooser, highlighting an option shows its facts without choosing it.
Use Right Arrow to enter its card and Escape to return to the option. Select
the option when you are ready. Reading a card changes no setting.

## Read facts with their source

A provider can show available windows, resets, active turns and applicable
limits. A model can show context, supported efforts, price and measured speed.
Speed describes eligible requests from the last seven days; it is not a promise
about the next turn. Read [the measurements](speed-first-output-and-errors.md#what-the-figures-mean)
before comparing models.

A host shows capacity, declared use, available storage and whether it takes
new work. An environment shows its host, threads, shells and reported process,
CPU, memory and disk use. Declared capacity and current consumption are separate
figures. Read the observation time before treating an old value as current.

Mount, MCP server, template, profile and skill cards show their own configuration
and permitted use. A template's image reference and digest identify its source.
An agent profile describes the program; it does not own the provider's sign-in.

## Understand missing or blocked facts

A dash means unknown. A loading card is still being read. A failed read offers
Retry and does not turn an unknown measurement into zero. Offline, Full and
At limit explain different reasons a choice may not start work.

You see only facts your access permits. A visible name or card does not grant
Use or Manage. Disabled options keep their reason. Start, Stop or Recycle in
a card follow the same permissions and confirmation as the resource page.
